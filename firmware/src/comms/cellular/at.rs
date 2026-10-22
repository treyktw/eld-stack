//! AT command response parsing.
//!
//! Pure — no UART, no esp-idf — so the host harness can exercise every branch.
//! The modem is a serial device that speaks lines; this decides what a line
//! means. Getting it wrong does not crash, it silently mis-reads the link state,
//! which is the failure mode that ends with records queued behind a modem
//! everyone believes is connected.
//!
//! The subtle part is **unsolicited result codes**. A modem emits URCs — network
//! registration changes, incoming data, power warnings — whenever it likes,
//! including in the middle of another command's response. Code that assumes the
//! next line after a command is that command's answer will eventually read a URC
//! as the answer. So parsing is explicit about which lines terminate a response
//! and which are asynchronous noise to be handled separately.

/// One line, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtLine<'a> {
    /// Command completed successfully.
    Ok,
    /// Bare `ERROR`.
    Error,
    /// `+CME ERROR: <code>` — equipment error with a specific cause.
    CmeError(u16),
    /// `+CMS ERROR: <code>` — SMS-related error.
    CmsError(u16),
    /// A response payload line, e.g. `+CSQ: 17,99`.
    Response(&'a str),
    /// An unsolicited result code — arrives without a command.
    Urc(&'a str),
    /// Empty line, or the echoed command. Carries no meaning.
    Ignorable,
}

/// URC prefixes this firmware recognises.
///
/// Anything not listed is treated as a command response, because misreading a
/// URC as a response is recoverable (the command times out and retries) while
/// misreading a response as a URC loses the answer entirely.
const URC_PREFIXES: &[&str] = &[
    "+CREG:",
    "+CGREG:",
    "+CEREG:", // registration changed
    "RDY",     // modem finished booting
    "+CPIN:",  // SIM state changed
    "NORMAL POWER DOWN",
    "+CGEV:",    // packet-domain event
    "+SIMCARD:", // SIMCom card status
];

/// Whether a line terminates a command's response.
pub fn is_terminal(line: &AtLine<'_>) -> bool {
    matches!(
        line,
        AtLine::Ok | AtLine::Error | AtLine::CmeError(_) | AtLine::CmsError(_)
    )
}

/// Classify one line of modem output.
///
/// `echo` is the command that was sent, if echo is on — the modem repeats it
/// before answering, and treating that repeat as a response line would shift
/// every subsequent parse by one.
pub fn classify<'a>(raw: &'a str, echo: Option<&str>) -> AtLine<'a> {
    let line = raw.trim_matches(|c| c == '\r' || c == '\n' || c == ' ');

    if line.is_empty() {
        return AtLine::Ignorable;
    }
    if let Some(sent) = echo {
        if line == sent.trim_end_matches(['\r', '\n']) {
            return AtLine::Ignorable;
        }
    }

    if line == "OK" {
        return AtLine::Ok;
    }
    if line == "ERROR" {
        return AtLine::Error;
    }
    if let Some(rest) = line.strip_prefix("+CME ERROR:") {
        return AtLine::CmeError(parse_code(rest));
    }
    if let Some(rest) = line.strip_prefix("+CMS ERROR:") {
        return AtLine::CmsError(parse_code(rest));
    }

    // Several SIMCom result prefixes are used both for unsolicited state
    // changes and for the direct answer to a query. Preserve them as URCs in
    // every other context, but do not hide a requested answer from the caller.
    if let Some(command) = echo {
        let is_queried_response = [
            ("AT+CPIN?", "+CPIN:"),
            ("AT+CREG?", "+CREG:"),
            ("AT+CGREG?", "+CGREG:"),
            ("AT+CEREG?", "+CEREG:"),
        ]
        .iter()
        .any(|(query, prefix)| command.trim() == *query && line.starts_with(prefix));

        if is_queried_response {
            return AtLine::Response(line);
        }
    }

    for prefix in URC_PREFIXES {
        if line.starts_with(prefix) {
            return AtLine::Urc(line);
        }
    }

    AtLine::Response(line)
}

/// Parse a numeric error code, defaulting to 0 when the modem sends a textual
/// one (`+CME ERROR: SIM not inserted`). The distinction between "unknown code"
/// and "code 0" does not change behaviour — both are failures — so this stays
/// simple rather than growing a second error type.
fn parse_code(rest: &str) -> u16 {
    rest.trim().parse::<u16>().unwrap_or(0)
}

/// Signal quality from `+CSQ: <rssi>,<ber>`.
///
/// `rssi` is an index, not dBm: 0..=31 maps linearly onto -113..=-51 dBm, and
/// 99 means "not known or not detectable". Returning `None` for 99 keeps the
/// caller from reporting a strong-looking 99 as excellent signal.
pub fn parse_csq(line: &str) -> Option<i16> {
    let rest = line.strip_prefix("+CSQ:")?;
    let rssi: u8 = rest.split(',').next()?.trim().parse().ok()?;
    if rssi == 99 || rssi > 31 {
        return None;
    }
    Some(-113 + 2 * rssi as i16)
}

/// Network registration state, from `+CREG:`/`+CGREG:`/`+CEREG:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    NotRegistered,
    Home,
    Searching,
    Denied,
    Unknown,
    Roaming,
}

impl Registration {
    /// Whether the modem can carry traffic. Home and roaming both can — an ELD
    /// crossing a state line onto a partner network must keep uploading, so
    /// roaming is not treated as a failure.
    pub fn is_usable(self) -> bool {
        matches!(self, Registration::Home | Registration::Roaming)
    }
}

/// Parse a registration line.
///
/// The status is the *last* numeric field before any location fields, and its
/// position depends on whether unsolicited reporting is enabled:
///
/// - `+CREG: <stat>`             (URC form, one field)
/// - `+CREG: <n>,<stat>`         (query form, two fields)
/// - `+CREG: <n>,<stat>,<lac>,<ci>` (with location info)
///
/// Taking "the second field" would misread the URC form, and taking "the first"
/// would misread the query form. So the arity decides.
pub fn parse_registration(line: &str) -> Option<Registration> {
    let rest = line
        .strip_prefix("+CREG:")
        .or_else(|| line.strip_prefix("+CGREG:"))
        .or_else(|| line.strip_prefix("+CEREG:"))?;

    let fields: heapless::Vec<&str, 8> = rest.split(',').map(str::trim).collect();
    let stat_field = match fields.len() {
        0 => return None,
        1 => fields[0],
        _ => fields[1],
    };

    Some(match stat_field.parse::<u8>().ok()? {
        0 => Registration::NotRegistered,
        1 => Registration::Home,
        2 => Registration::Searching,
        3 => Registration::Denied,
        5 => Registration::Roaming,
        _ => Registration::Unknown,
    })
}

/// Parse the HTTP status from SIMCom's `+HTTPACTION: <method>,<status>,<len>`.
pub fn parse_http_action(line: &str) -> Option<(u16, u32)> {
    let rest = line.strip_prefix("+HTTPACTION:")?;
    let mut fields = rest.split(',').map(str::trim);
    let _method = fields.next()?;
    let status: u16 = fields.next()?.parse().ok()?;
    let length: u32 = fields.next().unwrap_or("0").parse().unwrap_or(0);
    Some((status, length))
}
