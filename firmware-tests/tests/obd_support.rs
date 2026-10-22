//! Runtime PID discovery.
//!
//! This is what makes the puller universal rather than tuned to one truck. The
//! bit arithmetic is worth testing precisely because getting it wrong is silent:
//! an off-by-one drops a PID the vehicle does support, and the symptom is
//! "that truck doesn't report fuel" rather than a crash.

use eld_decoders_test::obd::support::{next_range_to_query, SupportedPids, SUPPORT_RANGE_PIDS};

#[test]
fn starts_empty() {
    let s = SupportedPids::new();
    assert!(s.is_empty());
    assert_eq!(s.count(), 0);
    assert!(!s.contains(0x0C));
}

#[test]
fn the_msb_of_the_first_byte_is_the_pid_after_the_base() {
    // The single most error-prone line in OBD discovery. Bit 7 of byte 0 in the
    // response to PID 0x00 means PID 0x01 — not 0x00, and not bit 0.
    let mut s = SupportedPids::new();
    s.absorb_range(0x00, [0x80, 0x00, 0x00, 0x00]);
    assert!(s.contains(0x01));
    assert!(!s.contains(0x00));
    assert!(!s.contains(0x02));
    assert_eq!(s.count(), 1);
}

#[test]
fn the_lsb_of_the_last_byte_is_the_pid_thirty_two_along() {
    let mut s = SupportedPids::new();
    s.absorb_range(0x00, [0x00, 0x00, 0x00, 0x01]);
    assert!(s.contains(0x20));
    assert_eq!(s.count(), 1);
}

#[test]
fn decodes_the_captured_trucks_pid_set() {
    // The 2013 Volvo answered 0x05, 0x0C, 0x0D, 0x11 and 0x42.
    //
    // Bits for 0x01..0x20, MSB-first:
    //   byte0 = 0x01..0x08 -> 0x05 set          -> 0b0000_1000 = 0x08
    //   byte1 = 0x09..0x10 -> 0x0C, 0x0D set    -> 0b0001_1000 = 0x18
    //   byte2 = 0x11..0x18 -> 0x11 set          -> 0b1000_0000 = 0x80
    //   byte3 = 0x19..0x20 -> 0x20 set (next)   -> 0b0000_0001 = 0x01
    let mut s = SupportedPids::new();
    let more = s.absorb_range(0x00, [0x08, 0x18, 0x80, 0x01]);

    for pid in [0x05, 0x0C, 0x0D, 0x11] {
        assert!(s.contains(pid), "{pid:#04x} should be supported");
    }
    assert!(
        !s.contains(0x2F),
        "fuel level was not offered by this truck"
    );
    assert!(!s.contains(0xA6), "odometer was not offered by this truck");
    assert!(more, "0x20 was set, so the next range should be walked");

    // Second range carries 0x42.
    //   byte0 = 0x21..0x28, byte1 = 0x29..0x30, byte2 = 0x31..0x38,
    //   byte3 = 0x39..0x40 — 0x42 is beyond this window, so it comes from the
    //   0x40 range instead.
    let more2 = s.absorb_range(0x20, [0x00, 0x00, 0x00, 0x01]);
    assert!(s.contains(0x40));
    assert!(more2);

    //   0x41..0x48 in byte0: 0x42 is the second bit.
    s.absorb_range(0x40, [0x40, 0x00, 0x00, 0x00]);
    assert!(s.contains(0x42));
}

#[test]
fn reports_whether_the_next_range_is_advertised() {
    let mut s = SupportedPids::new();
    // Nothing set: no further ranges.
    assert!(!s.absorb_range(0x00, [0x00, 0x00, 0x00, 0x00]));

    let mut t = SupportedPids::new();
    assert!(t.absorb_range(0x00, [0x00, 0x00, 0x00, 0x01]));
}

#[test]
fn the_walk_stops_when_the_ecu_stops_advertising() {
    // A truck that only implements the first range must be asked exactly once
    // for support, not seven times on every discovery cycle forever.
    let mut s = SupportedPids::new();
    let mut asked: Vec<u8> = Vec::new();

    let first = next_range_to_query(&s, &asked).expect("0x00 is always askable");
    assert_eq!(first, 0x00);
    asked.push(first);

    s.absorb_range(0x00, [0x08, 0x18, 0x80, 0x00]); // no 0x20 bit

    assert_eq!(
        next_range_to_query(&s, &asked),
        None,
        "no further ranges advertised, so the walk is done"
    );
}

#[test]
fn the_walk_follows_the_chain_while_it_is_advertised() {
    let mut s = SupportedPids::new();
    let mut asked: Vec<u8> = Vec::new();

    // Every range advertises the next.
    for expected in [0x00u8, 0x20, 0x40] {
        let next = next_range_to_query(&s, &asked).expect("should continue");
        assert_eq!(next, expected);
        asked.push(next);
        s.absorb_range(next, [0x00, 0x00, 0x00, 0x01]);
    }

    assert_eq!(next_range_to_query(&s, &asked), Some(0x60));
}

#[test]
fn a_range_is_never_asked_twice() {
    let mut s = SupportedPids::new();
    s.absorb_range(0x00, [0x00, 0x00, 0x00, 0x01]);
    let asked = vec![0x00u8, 0x20];
    // 0x20 is supported but already asked, so it must not come back.
    assert_ne!(next_range_to_query(&s, &asked), Some(0x20));
}

#[test]
fn iteration_yields_supported_pids_in_order() {
    let mut s = SupportedPids::new();
    s.absorb_range(0x00, [0x08, 0x18, 0x80, 0x00]);
    let found: Vec<u8> = s.iter().collect();
    assert_eq!(found, vec![0x05, 0x0C, 0x0D, 0x11]);
}

#[test]
fn a_bitmap_cannot_claim_pids_outside_its_own_window() {
    // The top range would otherwise wrap past 0xFF and set low PIDs, inventing
    // support for things the ECU never mentioned.
    let mut s = SupportedPids::new();
    s.absorb_range(0xC0, [0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(!s.contains(0x01), "must not wrap into the low range");
    assert!(s.contains(0xC1));
    assert!(s.contains(0xE0));
}

#[test]
fn every_documented_range_pid_is_covered() {
    assert_eq!(
        SUPPORT_RANGE_PIDS,
        [0x00, 0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0]
    );
}
