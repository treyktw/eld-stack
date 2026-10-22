//! UART transport for the cellular modem.
//!
//! UART2, because UART0 is the console and UART1 is the GPS. If the modem
//! supplies GNSS itself (BG95-M3, SIM7080G class), UART1 frees up and the
//! separate GPS module disappears.
//!
//! Everything that decides *meaning* lives elsewhere and is host-tested:
//! [`super::lines`] assembles bytes into lines, [`super::at`] says what a line
//! means, [`super::link`] decides what to do about it. This module only moves
//! bytes and enforces timeouts — deliberately, because it is the one part that
//! cannot be exercised without hardware.

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::uart::config::Config as UartConfig;
use esp_idf_svc::hal::uart::{UartDriver, UART2};
use esp_idf_svc::hal::units::Hertz;

use super::at::{self, AtLine};
use super::lines::{LineAssembler, MAX_LINE, MAX_LINES};
use crate::error::EldError;

/// SIM7670G and most SIMCom parts autobaud but settle at 115200.
pub const MODEM_BAUD: u32 = 115_200;

/// Scratch per read.
const READ_CHUNK: usize = 512;

/// Ticks to block on a single UART read.
///
/// Small but non-zero. A zero-tick read spins the CPU busy-waiting on a device
/// that answers in milliseconds; blocking forever would wedge the whole task on
/// a modem that has stopped talking.
const READ_TICKS: u32 = 20;

/// Maximum UART reads used to discard traffic that predates a command.
///
/// The modem can retain NMEA output settings across resets. A single read is
/// therefore not enough to clear the UART after a long operation such as CAN
/// probing, and a response terminator behind that backlog can be dropped when
/// the fixed-capacity line buffer fills. At 115200 baud this bound is ample to
/// catch up while still preventing a noisy modem from blocking forever.
const STALE_DRAIN_READS: usize = 64;

pub type Line = heapless::String<MAX_LINE>;
pub type LineBuf = heapless::Vec<Line, MAX_LINES>;

/// Outcome of one command/response exchange.
#[derive(Debug)]
pub struct Transaction {
    /// Payload lines, in order. Terminators and URCs are not included.
    pub responses: LineBuf,
    /// URCs seen while waiting. Returned rather than dropped: a registration
    /// change arriving mid-command is exactly the event the link machine needs,
    /// and discarding it because it was inconveniently timed loses it forever.
    pub urcs: LineBuf,
    /// The terminating line, or `None` on timeout.
    pub terminator: Option<Terminator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    Ok,
    Error,
    CmeError(u16),
    CmsError(u16),
}

impl Transaction {
    pub fn succeeded(&self) -> bool {
        matches!(self.terminator, Some(Terminator::Ok))
    }

    /// First response line starting with `prefix`.
    pub fn find(&self, prefix: &str) -> Option<&str> {
        self.responses
            .iter()
            .map(|l| l.as_str())
            .find(|l| l.starts_with(prefix))
    }
}

pub struct Modem<'d> {
    uart: UartDriver<'d>,
    assembler: LineAssembler,
}

impl Modem<'static> {
    /// Open UART2 on the supplied pins.
    pub fn open(
        uart2: UART2<'static>,
        tx: AnyIOPin<'static>,
        rx: AnyIOPin<'static>,
    ) -> Result<Self, EldError> {
        let cfg = UartConfig::default().baudrate(Hertz(MODEM_BAUD));
        let uart = UartDriver::new(
            uart2,
            tx,
            rx,
            Option::<AnyIOPin>::None, // cts
            Option::<AnyIOPin>::None, // rts
            &cfg,
        )
        .map_err(|_| EldError::Comms("modem uart init failed"))?;

        Ok(Self {
            uart,
            assembler: LineAssembler::new(),
        })
    }

    /// Send a command, terminated with CR.
    fn write_command(&mut self, command: &str) -> Result<(), EldError> {
        self.uart
            .write(command.as_bytes())
            .map_err(|_| EldError::Comms("modem write failed"))?;
        self.uart
            .write(b"\r")
            .map_err(|_| EldError::Comms("modem write failed"))?;
        Ok(())
    }

    /// Write raw bytes with no CR appended.
    ///
    /// For payload bodies — AT+HTTPDATA expects exactly the byte count it was
    /// promised, and a trailing CR would make it one short of the body and one
    /// long of the count.
    pub fn write_raw(&mut self, bytes: &[u8]) -> Result<(), EldError> {
        self.uart
            .write(bytes)
            .map_err(|_| EldError::Comms("modem write failed"))?;
        Ok(())
    }

    /// Send a command whose success response is a prompt rather than `OK`.
    /// `AT+HTTPDATA`, for example, answers `DOWNLOAD` and then waits for an
    /// exact number of raw bytes.
    pub fn command_until<F: FnMut() -> u64>(
        &mut self,
        command: &str,
        expected_prefix: &str,
        timeout_ms: u64,
        clock_ms: F,
    ) -> Result<bool, EldError> {
        let mut stale_urcs = LineBuf::new();
        self.drain_stale(&mut stale_urcs);
        self.write_command(command)?;
        Ok(self
            .wait_for_line(expected_prefix, timeout_ms, clock_ms)?
            .is_some())
    }

    /// Wait for an asynchronous response line after a command's immediate
    /// `OK`. SIMCom uses this two-stage shape for `NETOPEN` and `HTTPACTION`.
    pub fn wait_for_line<F: FnMut() -> u64>(
        &mut self,
        expected_prefix: &str,
        timeout_ms: u64,
        mut clock_ms: F,
    ) -> Result<Option<Line>, EldError> {
        let started = clock_ms();
        let mut lines = LineBuf::new();
        while clock_ms().saturating_sub(started) < timeout_ms {
            lines.clear();
            self.poll_lines(&mut lines);
            for line in &lines {
                if line.starts_with(expected_prefix) {
                    return Ok(Some(line.clone()));
                }
                if matches!(
                    at::classify(line, None),
                    AtLine::Error | AtLine::CmeError(_)
                ) {
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Drain buffered bytes into complete lines.
    ///
    /// Safe to call when idle: this is how URCs are collected between commands,
    /// and a modem that reports a dropped registration while nothing is being
    /// asked still needs to be heard.
    pub fn poll_lines(&mut self, out: &mut LineBuf) -> usize {
        let mut buf = [0u8; READ_CHUNK];
        match self.uart.read(&mut buf, READ_TICKS) {
            Ok(0) | Err(_) => 0,
            Ok(n) => {
                self.assembler.feed(&buf[..n], out);
                n
            }
        }
    }

    /// Drain bytes that arrived before a new command, stopping after the first
    /// quiet UART read. Registration URCs remain observable by the caller.
    fn drain_stale(&mut self, urcs: &mut LineBuf) {
        let mut batch = LineBuf::new();

        for _ in 0..STALE_DRAIN_READS {
            batch.clear();
            let bytes_read = self.poll_lines(&mut batch);
            for line in &batch {
                if matches!(at::classify(line, None), AtLine::Urc(_)) {
                    let _ = urcs.push(line.clone());
                }
            }
            if bytes_read == 0 {
                break;
            }
        }
    }

    /// Run one command and collect its response.
    ///
    /// `now_ms` and `timeout_ms` are supplied by the caller rather than read
    /// from a clock here, so the timeout is testable and this module keeps no
    /// notion of time of its own.
    ///
    /// A timeout resets the line assembler. Whatever fragment is buffered
    /// belongs to a command nobody is waiting for now, and carrying it forward
    /// would prefix the next response with garbage.
    pub fn transact<F: FnMut() -> u64>(
        &mut self,
        command: &str,
        timeout_ms: u64,
        mut clock_ms: F,
    ) -> Result<Transaction, EldError> {
        // Anything still buffered predates this command and is not its answer.
        let mut stale_urcs = LineBuf::new();
        self.drain_stale(&mut stale_urcs);

        self.write_command(command)?;

        let started = clock_ms();
        let mut result = Transaction {
            responses: LineBuf::new(),
            // URCs that arrived just before the command still belong to the
            // caller — they were only stale with respect to the response.
            urcs: stale_urcs,
            terminator: None,
        };

        let mut batch = LineBuf::new();

        while clock_ms().saturating_sub(started) < timeout_ms {
            batch.clear();
            self.poll_lines(&mut batch);

            for line in batch.iter() {
                let classified = at::classify(line, Some(command));
                if at::is_terminal(&classified) {
                    result.terminator = Some(match classified {
                        AtLine::Ok => Terminator::Ok,
                        AtLine::Error => Terminator::Error,
                        AtLine::CmeError(code) => Terminator::CmeError(code),
                        AtLine::CmsError(code) => Terminator::CmsError(code),
                        _ => unreachable!("is_terminal accepted a non-terminal AT line"),
                    });
                    return Ok(result);
                }
                match classified {
                    AtLine::Ignorable => {}
                    AtLine::Urc(_) => {
                        let _ = result.urcs.push(line.clone());
                    }
                    AtLine::Response(_) => {
                        let _ = result.responses.push(line.clone());
                    }
                    AtLine::Ok | AtLine::Error | AtLine::CmeError(_) | AtLine::CmsError(_) => {
                        unreachable!("terminal AT line was not handled")
                    }
                }
            }
        }

        log::warn!(
            "cellular: command timed out with {} buffered bytes",
            self.assembler.pending()
        );
        self.assembler.reset();
        Ok(result)
    }
}
