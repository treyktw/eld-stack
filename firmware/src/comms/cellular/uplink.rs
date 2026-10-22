//! Cellular uplink: bring-up driven by the link state machine, uploads over
//! the modem's HTTP stack.
//!
//! This is the seam where the tested pure pieces meet hardware. [`link::Link`]
//! decides *what* to do, [`at`] says what the modem's answers mean, and this
//! turns those decisions into AT commands. Keeping the ordering here thin is
//! deliberate: it is the part that cannot be exercised without a modem, so it
//! should contain as little judgement as possible.
//!
//! The bring-up and HTTPS upload sequence has been verified on the
//! T-SIM7670G-S3 with a Hologram SIM. Pure parsing and recovery logic remains
//! host-tested so failure paths do not require live cellular service.

use log::{info, warn};

use super::at;
use super::link::{Link, LinkAction, LinkEvent};
use super::modem::{Modem, Terminator};
use crate::error::EldError;

/// Per-command timeouts. Registration queries are slow on a cold modem;
/// an HTTP action can take seconds on a weak signal.
const SHORT_MS: u64 = 1_000;
const LONG_MS: u64 = 10_000;
const HTTP_MS: u64 = 60_000;
const DATA_ACTIVATION_MS: u64 = 75_000;

pub struct CellularLink {
    modem: Modem<'static>,
    link: Link,
    /// Last signal reading, for status reporting.
    rssi_dbm: Option<i16>,
}

impl CellularLink {
    pub fn new(modem: Modem<'static>, now_ms: u64) -> Self {
        Self {
            modem,
            link: Link::new(now_ms),
            rssi_dbm: None,
        }
    }

    pub fn is_up(&self) -> bool {
        self.link.is_ready()
    }

    pub fn rssi_dbm(&self) -> Option<i16> {
        self.rssi_dbm
    }

    pub fn backoff_ms(&self) -> u64 {
        self.link.backoff_ms()
    }

    /// Advance bring-up. Call every loop iteration; it returns quickly when
    /// there is nothing due.
    pub fn tick(&mut self, now_ms: u64) {
        // URCs arrive unprompted, and a registration change is the one event
        // that must not wait for a command to be issued.
        self.drain_urcs(now_ms);

        match self.link.poll(now_ms) {
            LinkAction::Wait | LinkAction::Ready => {}
            LinkAction::PowerCycle => {
                // Board bring-up has already powered and configured this
                // modem. ATZ is not a harmless liveness probe on SIM7670G: it
                // restores the saved command profile and can leave the UART
                // unresponsive until autobaud settles. Verify the existing
                // channel and preserve GNSS/network configuration instead.
                info!("cellular: initializing modem command channel");
                let responded = match self.modem.transact("AT", SHORT_MS, crate::time::now_ms) {
                    Ok(transaction) if transaction.succeeded() => true,
                    Ok(transaction) => {
                        warn!(
                            "cellular: AT probe failed terminator={:?} responses={} urcs={}",
                            transaction.terminator,
                            transaction.responses.len(),
                            transaction.urcs.len()
                        );
                        false
                    }
                    Err(error) => {
                        warn!("cellular: AT probe transport failed ({error})");
                        false
                    }
                };
                if responded {
                    let _ = self.modem.transact("ATE0", SHORT_MS, crate::time::now_ms);
                    info!("cellular: modem command channel ready");
                    self.link.on_event(LinkEvent::ModemResponded, now_ms);
                }
            }
            LinkAction::Probe => self.probe(now_ms),
        }
    }

    /// Read any pending URCs and feed registration changes to the link.
    fn drain_urcs(&mut self, now_ms: u64) {
        let mut lines = super::modem::LineBuf::new();
        self.modem.poll_lines(&mut lines);

        for line in lines.iter() {
            if let at::AtLine::Urc(text) = at::classify(line, None) {
                if let Some(reg) = at::parse_registration(text) {
                    self.link.on_event(LinkEvent::Registration(reg), now_ms);
                }
            }
        }
    }

    /// Issue the probe appropriate to the current state.
    fn probe(&mut self, now_ms: u64) {
        use super::link::LinkState;

        match self.link.state() {
            LinkState::Booting => {
                let ok = self
                    .modem
                    .transact("AT", SHORT_MS, crate::time::now_ms)
                    .map(|t| t.succeeded())
                    .unwrap_or(false);
                if ok {
                    self.link.on_event(LinkEvent::ModemResponded, now_ms);
                } else {
                    self.link.on_event(LinkEvent::Timeout, now_ms);
                }
            }

            LinkState::WaitingForSim => {
                match self
                    .modem
                    .transact("AT+CPIN?", LONG_MS, crate::time::now_ms)
                {
                    Ok(t) if t.find("+CPIN: READY").is_some() => {
                        info!("cellular: SIM ready");
                        self.link.on_event(LinkEvent::SimReady, now_ms);
                    }
                    // A definite error means the SIM is missing or locked. A
                    // timeout is not the same thing and is treated as one.
                    Ok(t) if matches!(t.terminator, Some(Terminator::CmeError(_))) => {
                        warn!("cellular: SIM not usable");
                        self.link.on_event(LinkEvent::SimFailed, now_ms);
                    }
                    Ok(t) => {
                        warn!(
                            "cellular: SIM query failed terminator={:?} responses={} urcs={}",
                            t.terminator,
                            t.responses.len(),
                            t.urcs.len()
                        );
                        self.link.on_event(LinkEvent::Timeout, now_ms);
                    }
                    Err(error) => {
                        warn!("cellular: SIM query transport failed ({error})");
                        self.link.on_event(LinkEvent::Timeout, now_ms);
                    }
                }
            }

            LinkState::Registering => {
                if let Ok(t) = self
                    .modem
                    .transact("AT+CREG?", LONG_MS, crate::time::now_ms)
                {
                    if let Some(line) = t.find("+CREG:") {
                        if let Some(reg) = at::parse_registration(line) {
                            info!("cellular: LTE registration state {reg:?}");
                            self.link.on_event(LinkEvent::Registration(reg), now_ms);
                        }
                    }
                }
                // Signal strength is informational; a failure here must not
                // affect the link state.
                if let Ok(t) = self.modem.transact("AT+CSQ", SHORT_MS, crate::time::now_ms) {
                    if let Some(line) = t.find("+CSQ:") {
                        self.rssi_dbm = at::parse_csq(line);
                    }
                }
            }

            LinkState::ActivatingData => {
                let ready = self.activate_data();
                if ready {
                    info!("cellular: packet-data service ready");
                    self.log_connected_summary();
                }
                self.link.on_event(
                    if ready {
                        LinkEvent::DataReady
                    } else {
                        LinkEvent::DataFailed
                    },
                    now_ms,
                );
            }

            _ => {}
        }
    }

    /// Print a compact field-service summary once the complete cellular path
    /// is usable. These stable labels are intentionally easy to recognize in
    /// a serial terminal without understanding the state-machine logs.
    fn log_connected_summary(&self) {
        info!("MODEM: OK");
        info!("SIM: DETECTED");
        info!("NETWORK: REGISTERED");
        match self.rssi_dbm {
            Some(rssi_dbm) => info!("SIGNAL: {rssi_dbm} dBm"),
            None => info!("SIGNAL: UNKNOWN"),
        }
        info!("LTE DATA: CONNECTED");
    }

    /// Configure Hologram's APN and open the modem's IPv4 packet-data service.
    fn activate_data(&mut self) -> bool {
        let clock = crate::time::now_ms;

        // A warm modem may already have an open service from the prior link
        // attempt. Query first so recovery does not turn success into an error.
        if self
            .modem
            .transact("AT+NETOPEN?", SHORT_MS, clock)
            .ok()
            .and_then(|t| t.find("+NETOPEN:").map(str::to_owned))
            .is_some_and(|line| line.starts_with("+NETOPEN: 1"))
        {
            return true;
        }

        let apn = format!("AT+CGDCONT=1,\"IP\",\"{}\"", crate::config::CELLULAR_APN);
        // CSOCKSETPN selects the PDP context used by SIMCom's NETOPEN stack.
        // The SIM7670G rejects the older SIM7600 `AT+CIPCFG="CID",1` form;
        // selecting that same CID twice is unnecessary and prevents an
        // otherwise valid Hologram session from opening.
        for command in [apn.as_str(), "AT+CSOCKSETPN=1,1"] {
            if !self
                .modem
                .transact(command, LONG_MS, clock)
                .map(|t| t.succeeded())
                .unwrap_or(false)
            {
                warn!("cellular: packet-data setup failed at {command}");
                return false;
            }
        }

        let opened = match self.modem.transact("AT+NETOPEN", LONG_MS, clock) {
            Ok(t) if t.find("+NETOPEN: 0").is_some() => true,
            Ok(t) if t.succeeded() => self
                .modem
                .wait_for_line("+NETOPEN: 0", DATA_ACTIVATION_MS, clock)
                .ok()
                .flatten()
                .is_some(),
            _ => false,
        };
        if !opened {
            warn!("cellular: packet-data service did not open");
        }
        opened
    }

    /// POST a payload. `Err(())` means keep it queued and try again.
    ///
    /// Failures are reported to the link so repeated ones tear it down and
    /// rebuild: a modem that accepts commands while nothing reaches the server
    /// is worse than one that admits it is offline, because the queue drains
    /// into nowhere.
    pub fn post(&mut self, url: &str, body: &[u8], now_ms: u64) -> Result<(), ()> {
        if !self.link.is_ready() {
            return Err(());
        }

        match self.try_post(url, body) {
            Ok(status) if (200..300).contains(&status) => Ok(()),
            Ok(status) => {
                warn!("cellular: upload HTTP {status}");
                self.link.on_event(LinkEvent::TransferFailed, now_ms);
                Err(())
            }
            Err(e) => {
                warn!("cellular: upload failed ({e})");
                self.link.on_event(LinkEvent::TransferFailed, now_ms);
                Err(())
            }
        }
    }

    fn try_post(&mut self, url: &str, body: &[u8]) -> Result<u16, EldError> {
        let clock = crate::time::now_ms;

        if crate::config::UPLOAD_TOKEN.is_empty() {
            return Err(EldError::Comms("upload token empty"));
        }

        // HTTPTERM first: a session left open by a previous failure makes
        // HTTPINIT fail, and then every subsequent upload fails for a reason
        // that has nothing to do with the network.
        let _ = self.modem.transact("AT+HTTPTERM", SHORT_MS, clock);

        if !self
            .modem
            .transact("AT+HTTPINIT", LONG_MS, clock)?
            .succeeded()
        {
            return Err(EldError::Comms("HTTP initialization failed"));
        }

        // The SIM7670G needs an explicit TLS context and SNI for hostname
        // certificates. These are harmless for plain HTTP but mandatory for
        // the production HTTPS endpoint.
        for command in [
            "AT+CSSLCFG=\"sslversion\",0,4",
            "AT+CSSLCFG=\"enableSNI\",0,1",
            "AT+HTTPPARA=\"SSLCFG\",\"0\"",
        ] {
            if !self.modem.transact(command, SHORT_MS, clock)?.succeeded() {
                return Err(EldError::Comms("TLS setup failed"));
            }
        }

        let mut url_cmd: heapless::String<320> = heapless::String::new();
        url_cmd
            .push_str("AT+HTTPPARA=\"URL\",\"")
            .map_err(|_| EldError::Comms("url too long"))?;
        url_cmd
            .push_str(url)
            .map_err(|_| EldError::Comms("url too long"))?;
        url_cmd
            .push('"')
            .map_err(|_| EldError::Comms("url too long"))?;
        if !self.modem.transact(&url_cmd, SHORT_MS, clock)?.succeeded() {
            return Err(EldError::Comms("HTTP URL rejected"));
        }

        if !self
            .modem
            .transact(
                "AT+HTTPPARA=\"CONTENT\",\"application/json\"",
                SHORT_MS,
                clock,
            )?
            .succeeded()
        {
            return Err(EldError::Comms("HTTP content type rejected"));
        }

        // SIMCom exposes arbitrary request headers through USERDATA. The
        // ingest server resolves this credential to a device before parsing
        // the body, so cellular must send the same bearer header as WiFi.
        let mut auth_cmd: heapless::String<256> = heapless::String::new();
        auth_cmd
            .push_str("AT+HTTPPARA=\"USERDATA\",\"Authorization: Bearer ")
            .map_err(|_| EldError::Comms("upload token too long"))?;
        auth_cmd
            .push_str(crate::config::UPLOAD_TOKEN)
            .map_err(|_| EldError::Comms("upload token too long"))?;
        auth_cmd
            .push_str("\"")
            .map_err(|_| EldError::Comms("upload token too long"))?;
        if !self.modem.transact(&auth_cmd, SHORT_MS, clock)?.succeeded() {
            return Err(EldError::Comms("HTTP authorization header rejected"));
        }

        let mut data_cmd: heapless::String<48> = heapless::String::new();
        let mut len_buf = itoa_u32(body.len() as u32);
        data_cmd
            .push_str("AT+HTTPDATA=")
            .map_err(|_| EldError::Comms("cmd too long"))?;
        data_cmd
            .push_str(len_buf.as_str())
            .map_err(|_| EldError::Comms("cmd too long"))?;
        data_cmd
            .push_str(",10000")
            .map_err(|_| EldError::Comms("cmd too long"))?;
        if !self
            .modem
            .command_until(&data_cmd, "DOWNLOAD", HTTP_MS, clock)?
        {
            return Err(EldError::Comms("modem did not request HTTP body"));
        }

        self.modem.write_raw(body)?;

        if self.modem.wait_for_line("OK", HTTP_MS, clock)?.is_none() {
            return Err(EldError::Comms("modem did not accept HTTP body"));
        }

        let action = self.modem.transact("AT+HTTPACTION=1", LONG_MS, clock)?;
        if !action.succeeded() {
            return Err(EldError::Comms("HTTP action rejected"));
        }
        let status = self
            .modem
            .wait_for_line("+HTTPACTION:", HTTP_MS, clock)?
            .as_deref()
            .and_then(at::parse_http_action)
            .map(|(status, _)| status)
            .unwrap_or(0);

        let _ = self.modem.transact("AT+HTTPTERM", SHORT_MS, clock);

        len_buf.clear();
        Ok(status)
    }
}

/// Decimal render without `alloc`.
fn itoa_u32(mut value: u32) -> heapless::String<10> {
    let mut out: heapless::String<10> = heapless::String::new();
    if value == 0 {
        let _ = out.push('0');
        return out;
    }
    let mut digits = [0u8; 10];
    let mut n = 0;
    while value > 0 {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        let _ = out.push(digits[i] as char);
    }
    out
}
