//! Assembling modem output into lines across chunked reads.
//!
//! A serial read returns whatever happened to be buffered, so lines split at
//! arbitrary points. Code that treats each read as one message works on a bench
//! and fails intermittently in a truck — which is the worst way for this to
//! fail, because it looks like a flaky modem rather than a software bug.

use eld_decoders_test::cellular::lines::{LineAssembler, MAX_LINE, MAX_LINES};

type Buf = heapless::Vec<heapless::String<MAX_LINE>, MAX_LINES>;

fn collect(chunks: &[&str]) -> Vec<String> {
    let mut a = LineAssembler::new();
    let mut out: Buf = heapless::Vec::new();
    for c in chunks {
        a.feed(c.as_bytes(), &mut out);
    }
    out.iter().map(|s| s.as_str().to_string()).collect()
}

#[test]
fn splits_a_simple_response() {
    assert_eq!(
        collect(&["+CSQ: 17,99\r\nOK\r\n"]),
        vec!["+CSQ: 17,99", "OK"]
    );
}

#[test]
fn joins_a_line_split_across_reads() {
    // The case this module exists for.
    assert_eq!(collect(&["+CSQ: 1", "7,99\r\n"]), vec!["+CSQ: 17,99"]);
}

#[test]
fn joins_a_terminator_split_across_reads() {
    // `OK\r\n` arriving as `O` then `K\r\n` must not be two lines, and must not
    // be dropped — it is the terminator the whole transaction waits for.
    assert_eq!(collect(&["O", "K", "\r\n"]), vec!["OK"]);
}

#[test]
fn handles_a_split_inside_the_crlf() {
    assert_eq!(collect(&["OK\r", "\n"]), vec!["OK"]);
}

#[test]
fn one_byte_at_a_time_still_assembles() {
    let text = "+CREG: 0,1\r\nOK\r\n";
    let chunks: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    assert_eq!(collect(&refs), vec!["+CREG: 0,1", "OK"]);
}

#[test]
fn skips_empty_padding_lines() {
    // Modems pad liberally with CRLF; an empty line carries no meaning and
    // would otherwise be classified and counted.
    assert_eq!(collect(&["\r\n\r\nOK\r\n\r\n"]), vec!["OK"]);
}

#[test]
fn tolerates_bare_newlines() {
    assert_eq!(collect(&["OK\nERROR\n"]), vec!["OK", "ERROR"]);
}

#[test]
fn a_partial_line_is_not_emitted_until_terminated() {
    let mut a = LineAssembler::new();
    let mut out: Buf = heapless::Vec::new();

    a.feed(b"+CSQ: 17,99", &mut out);
    assert!(out.is_empty(), "an unterminated line is not a line");
    assert_eq!(a.pending(), 11);

    a.feed(b"\r\n", &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(a.pending(), 0);
}

#[test]
fn reset_discards_a_partial_line() {
    // On timeout the buffered fragment belongs to a command nobody is waiting
    // for. Carrying it forward prefixes the next response with garbage.
    let mut a = LineAssembler::new();
    let mut out: Buf = heapless::Vec::new();

    a.feed(b"+CSQ: 1", &mut out);
    a.reset();
    a.feed(b"7,99\r\n", &mut out);

    assert_eq!(
        out.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["7,99"],
        "the stale fragment must not be prepended"
    );
}

#[test]
fn an_overlong_line_is_dropped_without_corrupting_the_next() {
    // Line noise on a long harness run, or a wedged modem. The oversized line
    // goes, but the following one must still parse — otherwise one burst of
    // noise desynchronises the stream permanently.
    let long = "X".repeat(MAX_LINE * 2);
    let input = format!("{long}\r\nOK\r\n");
    assert_eq!(collect(&[&input]), vec!["OK"]);
}

#[test]
fn an_overlong_line_split_across_reads_still_recovers() {
    let long = "X".repeat(MAX_LINE * 2);
    let a = &long[..MAX_LINE];
    let b = &long[MAX_LINE..];
    let tail = "\r\n+CSQ: 17,99\r\n";
    assert_eq!(collect(&[a, b, tail]), vec!["+CSQ: 17,99"]);
}

#[test]
fn non_utf8_is_dropped_rather_than_lossily_converted() {
    // A mangled line is not a line. Substituting replacement characters could
    // let noise match a prefix like "+CREG:" and be treated as real.
    let mut a = LineAssembler::new();
    let mut out: Buf = heapless::Vec::new();
    a.feed(&[0xFF, 0xFE, b'\r', b'\n'], &mut out);
    assert!(out.is_empty());

    a.feed(b"OK\r\n", &mut out);
    assert_eq!(out.len(), 1, "the stream recovers on the next line");
}
