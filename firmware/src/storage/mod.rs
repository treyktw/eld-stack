//! Storage engine: structured logging to SD (or disabled when no card).
//!
//! Flow: a `LogRecord` is serialized to a line and appended to the right file
//! via the `Writer`. SD bring-up lives in `sd`; if it's off or fails, the
//! engine runs in a disabled state where every log call is a cheap no-op, so
//! the rest of the firmware doesn't care whether a card is present.

pub mod files;
pub mod queue;
pub mod recovery;
pub mod rotation;
pub mod schema;
pub mod sd;
pub mod writer;

use std::cell::Cell;
use std::path::Path;

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::spi::SPI2;
use log::{info, warn};

use crate::can::frame::CanFrame;
use queue::UploadQueue;
use schema::{LogRecord, RecordStamp};
use writer::Writer;

pub struct StorageEngine {
    writer: Writer,
    queue: UploadQueue,
    /// Clock applied to records, refreshed once per app-loop iteration.
    /// `Cell` because logging takes `&self` from the single app task.
    stamp: Cell<RecordStamp>,
    /// Keeps the SD mount alive; dropping it unmounts the card.
    _sd: Option<sd::SdMount>,
}

impl StorageEngine {
    /// Disabled engine — no SD, every log is a no-op. Used before `init` and
    /// as the fallback when SD logging is off or the card won't mount.
    pub fn disabled() -> Self {
        let writer = Writer::disabled();
        let queue = UploadQueue::new(&writer);
        Self {
            writer,
            queue,
            stamp: Cell::new(RecordStamp::default()),
            _sd: None,
        }
    }

    /// Bring up SD-backed storage with peripherals supplied by `App::boot`.
    /// Falls back to disabled on mount/fs failure.
    pub fn with_sd(
        spi2: SPI2<'static>,
        sclk: AnyIOPin<'static>,
        mosi: AnyIOPin<'static>,
        miso: AnyIOPin<'static>,
        cs: AnyIOPin<'static>,
    ) -> Self {
        let (mount, capacity) = match sd::mount(spi2, sclk, mosi, miso, cs) {
            Ok(result) => result,
            Err(e) => {
                warn!("storage: SD mount failed ({e}); logging disabled");
                return Self::disabled();
            }
        };

        match Writer::fs(files::SD_ROOT) {
            Ok(writer) => {
                if let Err(e) = sd::verify_read_write(Path::new(files::SD_ROOT)) {
                    warn!("storage: SD write/read verification failed ({e}); logging disabled");
                    let writer = Writer::disabled();
                    let queue = UploadQueue::new(&writer);
                    return Self {
                        writer,
                        queue,
                        stamp: Cell::new(RecordStamp::default()),
                        _sd: Some(mount),
                    };
                }
                let report = recovery::recover(Path::new(files::SD_ROOT));
                info!(
                    "storage: SD mounted at {} | capacity={:.2}GB rw=ok prior_session={} events={}B pending={}B",
                    files::SD_ROOT,
                    capacity.total_bytes as f64 / 1_000_000_000.0,
                    report.prior_session,
                    report.events_bytes,
                    report.pending_upload_bytes,
                );
                let queue = UploadQueue::new(&writer);
                Self {
                    writer,
                    queue,
                    stamp: Cell::new(RecordStamp::default()),
                    _sd: Some(mount),
                }
            }
            Err(e) => {
                warn!("storage: fs init failed ({e}); logging disabled");
                // Keep the mount alive but disable writes.
                let writer = Writer::disabled();
                let queue = UploadQueue::new(&writer);
                Self {
                    writer,
                    queue,
                    stamp: Cell::new(RecordStamp::default()),
                    _sd: Some(mount),
                }
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.writer.is_enabled()
    }

    /// Set the clock used to stamp subsequent records.
    ///
    /// Updated once per app-loop iteration from the GPS-anchored UTC clock.
    /// Until the first valid fix this stays unsynchronized, and records say so
    /// rather than presenting uptime as a wall-clock time.
    pub fn set_stamp(&self, stamp: RecordStamp) {
        self.stamp.set(stamp);
    }

    /// The stamp currently applied to records.
    pub fn stamp(&self) -> RecordStamp {
        self.stamp.get()
    }

    /// Serialize and append a record to its log file.
    pub fn log(&self, rec: &LogRecord) {
        let line = rec.to_line(self.stamp.get());
        self.writer.append(rec.kind(), &line);
    }

    /// Append a bus census snapshot to the system log.
    pub fn log_census(&self, json: &str) {
        self.writer
            .append(files::LogKind::System, &format!("CENSUS {json}"));
    }

    /// Enqueue a raw JSONL line for later upload.
    pub fn enqueue_upload(&self, json_line: &str) {
        self.queue.enqueue(&self.writer, json_line);
    }

    /// Up to `limit` queued upload lines, oldest first.
    pub fn pending_upload_batch(&self, limit: usize) -> Vec<String> {
        match self.queue.front_batch(limit) {
            Ok(batch) => batch,
            Err(e) => {
                warn!("storage: pending upload batch read failed ({e})");
                Vec::new()
            }
        }
    }

    /// Ack a whole batch. False means the queue head no longer matches, so
    /// nothing was removed and the records stay queued.
    pub fn ack_pending_upload_batch(&self, sent: &[String]) -> bool {
        match self.queue.ack_matching_front_batch(sent) {
            Ok(acked) => acked,
            Err(e) => {
                warn!("storage: pending upload batch ack failed ({e})");
                false
            }
        }
    }

    pub fn ack_pending_upload_if_front_matches(&self, sent_line: &str) -> bool {
        match self.queue.ack_matching_front(sent_line) {
            Ok(acked) => acked,
            Err(e) => {
                warn!("storage: pending upload ack failed ({e})");
                false
            }
        }
    }

    pub fn latest_pending_seq(&self) -> Option<u64> {
        match self.queue.latest_seq() {
            Ok(seq) => seq,
            Err(e) => {
                warn!("storage: pending upload latest seq failed ({e})");
                None
            }
        }
    }

    pub fn pending_upload_range(&self, from_seq: u64, to_seq: u64, limit: usize) -> Vec<String> {
        match self.queue.range(from_seq, to_seq, limit) {
            Ok(lines) => lines,
            Err(e) => {
                warn!("storage: pending upload range failed ({e})");
                Vec::new()
            }
        }
    }

    /// Remove queued records in `[from_seq, to_seq]`; returns how many.
    pub fn ack_pending_upload_range(&self, from_seq: u64, to_seq: u64) -> usize {
        match self.queue.ack_range(from_seq, to_seq) {
            Ok(acked) => acked,
            Err(e) => {
                warn!("storage: pending upload ack-range failed ({e})");
                0
            }
        }
    }

    /// Number of records still pending upload.
    pub fn pending_upload_count(&self) -> usize {
        match self.queue.count() {
            Ok(n) => n,
            Err(e) => {
                warn!("storage: pending upload count failed ({e})");
                0
            }
        }
    }

    // --- convenience wrappers ---------------------------------------------

    pub fn log_boot(&self, firmware: &'static str, board: &'static str) {
        self.log(&LogRecord::Boot { firmware, board });
    }

    pub fn log_signal(&self, name: &'static str, value: f32, source: u8) {
        self.log(&LogRecord::Signal {
            name,
            value,
            source,
        });
    }

    pub fn log_can_frame(&self, frame: &CanFrame) {
        self.log(&LogRecord::CanFrame {
            id: frame.id,
            extended: frame.extended,
            remote: frame.remote,
            dlc: frame.dlc,
            data: frame.data,
        });
    }

    pub fn log_heartbeat(&self, bitrate: &'static str, rx: u32, j1939: u32, errors: u32) {
        self.log(&LogRecord::Heartbeat {
            bitrate,
            rx,
            j1939,
            errors,
        });
    }

    pub fn log_trip(&self, what: &'static str) {
        self.log(&LogRecord::TripEvent { what });
    }

    pub fn log_mileage(&self, miles: f32, confidence: f32) {
        self.log(&LogRecord::VerifiedMileage { miles, confidence });
    }
}
