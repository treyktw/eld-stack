//! Cable relay over the device console serial stream.
//!
//! Commands are newline-delimited ASCII and must start with `ELD ` so normal
//! monitor input is ignored. Responses also start with `ELD ` so the phone app
//! can filter firmware logs from protocol frames.

use std::io::{self, Read, Write};

use log::warn;

use super::phone::PhoneCommand;
use super::PhoneResponse;

const MAX_LINE: usize = 128;

#[derive(Debug, Default)]
pub struct SerialRelay {
    buf: heapless::String<MAX_LINE>,
    commands_rx: u32,
    responses_tx: u32,
    parse_errors: u32,
}

impl SerialRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn commands_rx(&self) -> u32 {
        self.commands_rx
    }

    pub fn responses_tx(&self) -> u32 {
        self.responses_tx
    }

    pub fn parse_errors(&self) -> u32 {
        self.parse_errors
    }

    pub fn poll_command(&mut self) -> Option<PhoneCommand> {
        let mut byte = [0u8; 1];
        loop {
            match io::stdin().read(&mut byte) {
                Ok(0) => return None,
                Ok(_) => match byte[0] {
                    b'\n' | b'\r' => {
                        if self.buf.is_empty() {
                            continue;
                        }
                        let line = self.buf.as_str().trim();
                        let command = PhoneCommand::parse(line);
                        self.buf.clear();
                        match command {
                            Some(command) => {
                                self.commands_rx = self.commands_rx.wrapping_add(1);
                                return Some(command);
                            }
                            None => {
                                self.parse_errors = self.parse_errors.wrapping_add(1);
                                self.write_error("bad_command");
                                return None;
                            }
                        }
                    }
                    byte => {
                        if self.buf.push(byte as char).is_err() {
                            self.buf.clear();
                            self.parse_errors = self.parse_errors.wrapping_add(1);
                            self.write_error("line_too_long");
                            return None;
                        }
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return None,
                Err(e) => {
                    warn!("comms: serial relay read failed ({e})");
                    return None;
                }
            }
        }
    }

    pub fn write_response(&mut self, response: &PhoneResponse) {
        // Reuse the single canonical line formatter shared with the BLE relay.
        for line in super::response_lines(response) {
            self.write_line(&line);
        }
    }

    fn write_error(&mut self, reason: &str) {
        self.write_line(&format!("ELD ERROR {reason}"));
    }

    fn write_line(&mut self, line: &str) {
        let mut out = io::stdout();
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_ok() {
            self.responses_tx = self.responses_tx.wrapping_add(1);
        }
    }
}
