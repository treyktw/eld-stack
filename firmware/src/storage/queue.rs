//! Pending-upload queue.
//!
//! Records destined for upload are appended as JSONL to `upload/pending.jsonl`.
//! Comms sends the first line and rewrites the file without that line only after
//! the transport returns a 2xx response.

use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;

use super::writer::Writer;

pub struct UploadQueue {
    path: Option<PathBuf>,
}

impl UploadQueue {
    pub fn new(writer: &Writer) -> Self {
        let path = writer.root().map(super::files::upload_path);
        if let Some(path) = &path {
            let _ = OpenOptions::new().create(true).append(true).open(path);
        }
        Self { path }
    }

    /// Append one JSONL record to the pending queue. Best-effort.
    pub fn enqueue(&self, writer: &Writer, json_line: &str) {
        if let Some(path) = &self.path {
            writer.append_path(path, json_line);
        }
    }

    /// The first `limit` pending lines, oldest first.
    ///
    /// Batching is what makes a metered link affordable: each POST pays a TLS
    /// handshake several kilobytes long, while an envelope is a few hundred
    /// bytes. Sending one record per request means the handshake dominates the
    /// data bill.
    pub fn front_batch(&self, limit: usize) -> io::Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self.read_lines()?.into_iter().take(limit).collect())
    }

    /// Drop `sent` from the head of the queue, but only if it is still the head
    /// and still identical. A mismatch means the file moved under us, so
    /// nothing is removed — losing an unsent record is worse than sending it
    /// twice, and the server deduplicates.
    pub fn ack_matching_front_batch(&self, sent: &[String]) -> io::Result<bool> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        if sent.is_empty() {
            return Ok(false);
        }

        let lines = self.read_lines()?;
        if lines.len() < sent.len() {
            return Ok(false);
        }
        if lines.iter().zip(sent).any(|(have, want)| have != want) {
            return Ok(false);
        }

        let kept = &lines[sent.len()..];
        if kept.is_empty() {
            fs::write(path, "")?;
        } else {
            fs::write(path, format!("{}\n", kept.join("\n")))?;
        }
        Ok(true)
    }

    pub fn ack_matching_front(&self, sent_line: &str) -> io::Result<bool> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if is_empty_queue_error(&e) => return Ok(false),
            Err(e) => return Err(e),
        };

        let mut lines = content.lines();
        let Some(front) = lines.next() else {
            return Ok(false);
        };
        if front != sent_line {
            return Ok(false);
        }

        let rest = lines.collect::<Vec<_>>().join("\n");
        if rest.is_empty() {
            fs::write(path, "")?;
        } else {
            fs::write(path, format!("{rest}\n"))?;
        }
        Ok(true)
    }

    pub fn latest_seq(&self) -> io::Result<Option<u64>> {
        Ok(self
            .read_lines()?
            .iter()
            .filter_map(|line| seq_of(line))
            .max())
    }

    pub fn range(&self, from_seq: u64, to_seq: u64, limit: usize) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        for line in self.read_lines()? {
            let Some(seq) = seq_of(&line) else {
                continue;
            };
            if seq >= from_seq && seq <= to_seq {
                out.push(line);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Count pending (non-blank) lines still in the queue.
    pub fn count(&self) -> io::Result<usize> {
        Ok(self.read_lines()?.len())
    }

    /// Drop every queued record whose seq falls in `[from_seq, to_seq]`.
    /// Returns how many lines were removed. Unlike `ack_through`, this only
    /// touches the given range so the phone can ack non-contiguous chunks.
    pub fn ack_range(&self, from_seq: u64, to_seq: u64) -> io::Result<usize> {
        let Some(path) = &self.path else {
            return Ok(0);
        };

        let lines = self.read_lines()?;
        let mut acked = 0usize;
        let mut kept = Vec::new();
        for line in lines {
            match seq_of(&line) {
                Some(seq) if seq >= from_seq && seq <= to_seq => acked += 1,
                _ => kept.push(line),
            }
        }

        if kept.is_empty() {
            fs::write(path, "")?;
        } else {
            fs::write(path, format!("{}\n", kept.join("\n")))?;
        }
        Ok(acked)
    }

    fn read_lines(&self) -> io::Result<Vec<String>> {
        let Some(path) = &self.path else {
            return Ok(Vec::new());
        };
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if is_empty_queue_error(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect())
    }
}

fn is_empty_queue_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
    )
}

fn seq_of(line: &str) -> Option<u64> {
    let marker = "\"seq\":";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let digits = rest
        .as_bytes()
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    rest[..digits].parse().ok()
}
