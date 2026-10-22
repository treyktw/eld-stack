//! AT response parsing.
//!
//! The modem is a serial device that speaks lines. Misreading one does not
//! crash — it silently mis-reports the link, which is how records end up queued
//! on an SD card behind a modem everyone believes is connected.

use eld_decoders_test::cellular::at::{
    classify, is_terminal, parse_csq, parse_http_action, parse_registration, AtLine, Registration,
};

#[test]
fn classifies_terminators() {
    assert_eq!(classify("OK", None), AtLine::Ok);
    assert_eq!(classify("ERROR", None), AtLine::Error);
    assert_eq!(classify("+CME ERROR: 10", None), AtLine::CmeError(10));
    assert_eq!(classify("+CMS ERROR: 305", None), AtLine::CmsError(305));

    assert!(is_terminal(&AtLine::Ok));
    assert!(is_terminal(&AtLine::Error));
    assert!(is_terminal(&AtLine::CmeError(10)));
    assert!(!is_terminal(&AtLine::Response("+CSQ: 17,99")));
    assert!(!is_terminal(&AtLine::Urc("+CREG: 1")));
}

#[test]
fn handles_crlf_and_blank_lines() {
    assert_eq!(classify("\r\n", None), AtLine::Ignorable);
    assert_eq!(classify("OK\r\n", None), AtLine::Ok);
    assert_eq!(classify("   ", None), AtLine::Ignorable);
}

#[test]
fn discards_the_command_echo() {
    // With echo on, the modem repeats the command before answering. Counting
    // that as a response line shifts every subsequent parse by one.
    assert_eq!(classify("AT+CSQ", Some("AT+CSQ\r\n")), AtLine::Ignorable);
    // A different command's text is not the echo and must not be swallowed.
    assert_eq!(
        classify("AT+CREG?", Some("AT+CSQ\r\n")),
        AtLine::Response("AT+CREG?")
    );
}

#[test]
fn separates_urcs_from_responses() {
    // The whole point: a URC can land in the middle of another command's
    // response, and must not be mistaken for its answer.
    assert_eq!(classify("+CREG: 1", None), AtLine::Urc("+CREG: 1"));
    assert_eq!(classify("RDY", None), AtLine::Urc("RDY"));
    assert_eq!(classify("+CPIN: READY", None), AtLine::Urc("+CPIN: READY"));
    assert_eq!(
        classify("+CSQ: 17,99", None),
        AtLine::Response("+CSQ: 17,99")
    );
}

#[test]
fn a_urc_arriving_mid_response_does_not_terminate_it() {
    // Realistic interleaving: registration changes while +CSQ is in flight.
    let stream = ["AT+CSQ", "+CREG: 1", "+CSQ: 17,99", "OK"];
    let mut response = None;
    let mut urcs = 0;
    let mut terminated = false;

    for raw in stream {
        let line = classify(raw, Some("AT+CSQ"));
        match line {
            AtLine::Urc(_) => urcs += 1,
            AtLine::Response(text) => response = Some(text),
            _ if is_terminal(&line) => {
                terminated = true;
                break;
            }
            _ => {}
        }
    }

    assert_eq!(urcs, 1);
    assert_eq!(response, Some("+CSQ: 17,99"));
    assert!(terminated);
}

#[test]
fn sim_query_answer_is_a_response_not_a_urc() {
    assert_eq!(
        classify("+CPIN: READY", Some("AT+CPIN?")),
        AtLine::Response("+CPIN: READY")
    );
    assert!(matches!(
        classify("+CPIN: READY", Some("AT+CSQ")),
        AtLine::Urc(_)
    ));
}

#[test]
fn registration_query_answer_is_a_response_not_a_urc() {
    for (command, line) in [
        ("AT+CREG?", "+CREG: 0,1"),
        ("AT+CGREG?", "+CGREG: 0,5"),
        ("AT+CEREG?", "+CEREG: 0,2"),
    ] {
        assert_eq!(classify(line, Some(command)), AtLine::Response(line));
        assert!(matches!(classify(line, None), AtLine::Urc(_)));
    }
}

#[test]
fn parses_signal_quality_as_dbm() {
    assert_eq!(parse_csq("+CSQ: 0,99"), Some(-113));
    assert_eq!(parse_csq("+CSQ: 17,99"), Some(-79));
    assert_eq!(parse_csq("+CSQ: 31,99"), Some(-51));
}

#[test]
fn treats_unknown_signal_as_unknown_not_excellent() {
    // 99 means "not known or not detectable". Read as a number it looks like
    // the strongest possible signal.
    assert_eq!(parse_csq("+CSQ: 99,99"), None);
    assert_eq!(parse_csq("+CSQ: 45,99"), None);
    assert_eq!(parse_csq("garbage"), None);
}

#[test]
fn parses_registration_in_urc_form() {
    // `+CREG: <stat>` — one field. Taking "the second field" misreads this.
    assert_eq!(parse_registration("+CREG: 1"), Some(Registration::Home));
    assert_eq!(parse_registration("+CREG: 5"), Some(Registration::Roaming));
}

#[test]
fn parses_registration_in_query_form() {
    // `+CREG: <n>,<stat>` — two fields. Taking "the first field" misreads this,
    // and would report every modem as NotRegistered because <n> is usually 0.
    assert_eq!(parse_registration("+CREG: 0,1"), Some(Registration::Home));
    assert_eq!(
        parse_registration("+CREG: 2,5"),
        Some(Registration::Roaming)
    );
    assert_eq!(parse_registration("+CREG: 0,3"), Some(Registration::Denied));
}

#[test]
fn parses_registration_with_location_fields() {
    assert_eq!(
        parse_registration("+CREG: 2,1,\"A1B2\",\"0000C3D4\""),
        Some(Registration::Home)
    );
}

#[test]
fn accepts_the_packet_domain_variants() {
    assert_eq!(parse_registration("+CGREG: 0,1"), Some(Registration::Home));
    assert_eq!(
        parse_registration("+CEREG: 0,5"),
        Some(Registration::Roaming)
    );
}

#[test]
fn roaming_counts_as_usable() {
    // A truck crossing onto a partner network must keep uploading.
    assert!(Registration::Home.is_usable());
    assert!(Registration::Roaming.is_usable());
    assert!(!Registration::Searching.is_usable());
    assert!(!Registration::Denied.is_usable());
    assert!(!Registration::NotRegistered.is_usable());
}

#[test]
fn parses_http_action() {
    assert_eq!(parse_http_action("+HTTPACTION: 1,200,17"), Some((200, 17)));
    assert_eq!(parse_http_action("+HTTPACTION: 1,500,0"), Some((500, 0)));
    assert_eq!(parse_http_action("nonsense"), None);
}

#[test]
fn textual_cme_errors_do_not_panic() {
    // Some modems answer with words rather than a code.
    assert_eq!(
        classify("+CME ERROR: SIM not inserted", None),
        AtLine::CmeError(0)
    );
}
