//! Assembling modem output into complete lines across chunked reads.
//!
//! Pure, and separated from the UART for a reason: a serial read returns
//! whatever happened to be buffered, which splits lines at arbitrary points.
//! `OK\r\n` can arrive as `O` then `K\r\n`, and a response line can be cut in
//! half by a read boundary. Code that treats each read as a whole message works
//! perfectly on a bench and fails intermittently in a truck.
//!
//! This holds the partial tail between calls so a line is only emitted once it
//! is genuinely complete.

/// Longest line kept. AT responses are short; anything longer is noise, a wedged
/// modem, or line noise on a long harness run.
pub const MAX_LINE: usize = 256;

/// Lines buffered per drain before the caller must consume them.
pub const MAX_LINES: usize = 8;

pub struct LineAssembler {
    partial: heapless::Vec<u8, MAX_LINE>,
    /// Set when a line exceeded MAX_LINE and is being discarded to its end.
    overflowed: bool,
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl LineAssembler {
    pub const fn new() -> Self {
        Self {
            partial: heapless::Vec::new(),
            overflowed: false,
        }
    }

    /// Bytes held from a line not yet terminated.
    pub fn pending(&self) -> usize {
        self.partial.len()
    }

    /// Drop any partial line.
    ///
    /// Used when a transaction times out: whatever fragment is buffered belongs
    /// to a command nobody is waiting for any more, and carrying it into the
    /// next transaction prefixes that response with garbage.
    pub fn reset(&mut self) {
        self.partial.clear();
        self.overflowed = false;
    }

    /// Feed bytes, appending every completed line to `out`.
    ///
    /// Lines are split on `\n`; a trailing `\r` is stripped. Empty lines are
    /// skipped — modems pad liberally with CRLF and an empty line carries no
    /// meaning.
    ///
    /// Returns the number of lines produced.
    pub fn feed(
        &mut self,
        bytes: &[u8],
        out: &mut heapless::Vec<heapless::String<MAX_LINE>, MAX_LINES>,
    ) -> usize {
        let mut produced = 0;

        for &byte in bytes {
            if byte == b'\n' {
                if self.overflowed {
                    // The line we were dropping has now ended.
                    self.overflowed = false;
                    self.partial.clear();
                    continue;
                }

                // Strip a trailing CR, then emit if anything is left.
                let mut end = self.partial.len();
                if end > 0 && self.partial[end - 1] == b'\r' {
                    end -= 1;
                }

                if end > 0 {
                    if let Ok(text) = core::str::from_utf8(&self.partial[..end]) {
                        let mut line = heapless::String::new();
                        // Non-UTF8 is dropped rather than lossily converted:
                        // a mangled line is not a line, and inventing
                        // replacement characters would let it match a prefix.
                        if line.push_str(text).is_ok() && out.push(line).is_ok() {
                            produced += 1;
                        }
                    }
                }
                self.partial.clear();
                continue;
            }

            if self.overflowed {
                continue;
            }

            if self.partial.push(byte).is_err() {
                // Too long: discard this line, but keep consuming until its
                // terminator so the *next* line still starts clean.
                self.overflowed = true;
                self.partial.clear();
            }
        }

        produced
    }
}
