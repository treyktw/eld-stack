//! UART line reader for the GPS module.
//!
//! Non-blocking: each `read_into` drains whatever bytes are buffered, assembles
//! complete `\n`-terminated NMEA lines, and parses them. Partial lines carry
//! over to the next call.

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::uart::config::Config as UartConfig;
use esp_idf_svc::hal::uart::{UartDriver, UART1};
use esp_idf_svc::hal::units::Hertz;

use super::nmea::{self, Nmea};
use crate::config;
use crate::error::EldError;

/// Max line length we tolerate before flushing junk (NMEA lines are <82 chars).
const MAX_LINE: usize = 128;
/// UART read scratch size per poll.
const READ_CHUNK: usize = 256;

pub struct GpsDriver<'d> {
    uart: UartDriver<'d>,
    line: heapless::Vec<u8, MAX_LINE>,
}

impl GpsDriver<'static> {
    /// Open UART1 on the supplied pins. Peripherals come from `App::boot`.
    pub fn open(
        uart1: UART1<'static>,
        tx: AnyIOPin<'static>,
        rx: AnyIOPin<'static>,
    ) -> Result<Self, EldError> {
        let cfg = UartConfig::default().baudrate(Hertz(config::GPS_BAUD));
        let uart = UartDriver::new(
            uart1,
            tx,
            rx,
            Option::<AnyIOPin>::None, // cts
            Option::<AnyIOPin>::None, // rts
            &cfg,
        )
        .map_err(|_| EldError::Gps("uart init failed"))?;

        Ok(Self {
            uart,
            line: heapless::Vec::new(),
        })
    }

    /// Read available bytes and append every fully-parsed sentence to `out`.
    pub fn read_into(&mut self, out: &mut heapless::Vec<Nmea, 8>) {
        let mut buf = [0u8; READ_CHUNK];
        // Non-blocking read (0-tick timeout).
        let n = match self.uart.read(&mut buf, 0) {
            Ok(n) => n,
            Err(_) => return,
        };
        for &b in &buf[..n] {
            match b {
                b'\n' => {
                    if let Ok(s) = core::str::from_utf8(&self.line) {
                        if let Some(sentence) = nmea::parse(s) {
                            let _ = out.push(sentence);
                        }
                    }
                    self.line.clear();
                }
                b'\r' => {}
                _ => {
                    if self.line.push(b).is_err() {
                        self.line.clear(); // overlong/garbage line: drop it
                    }
                }
            }
        }
    }
}
