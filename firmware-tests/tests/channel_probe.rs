//! Which connector pin pair to believe.
//!
//! The 2013 Volvo settled this the hard way: vendor documentation put J1939 on
//! J1962 pins 3/11 and the OBD channel on 6/14, and the pair that actually
//! produced frames was 6/14. So the rule encoded here is that observation wins
//! over the datasheet, and "no traffic on the pair I happen to be wired to" is
//! not evidence about any other pair.

use eld_decoders_test::channel_probe::{best, ChannelResult};
use eld_decoders_test::pins::ObdPinSet;

fn result(pair: ObdPinSet, extended: u32, standard: u32) -> ChannelResult {
    ChannelResult {
        pair,
        bitrate_label: if extended + standard > 0 {
            Some("250k")
        } else {
            None
        },
        extended_frames: extended,
        standard_frames: standard,
    }
}

#[test]
fn extended_frames_mean_j1939() {
    let r = result(ObdPinSet::J1962Pins6And14, 42, 0);
    assert!(r.carries_j1939());
    assert_eq!(r.summary(), "j1939");
}

#[test]
fn standard_frames_alone_are_can_but_not_j1939() {
    // Live bus, wrong protocol — an ISO 15765 / OBD channel looks like this.
    // Very different from a dead pair, and worth reporting differently.
    let r = result(ObdPinSet::J1962Pins6And14, 0, 30);
    assert!(!r.carries_j1939());
    assert!(r.carries_non_j1939_can());
    assert_eq!(r.summary(), "can-but-not-j1939");
}

#[test]
fn a_silent_pair_is_distinguished_from_one_that_never_locked() {
    let silent = ChannelResult {
        bitrate_label: Some("250k"),
        ..ChannelResult::none(ObdPinSet::DeutschCAndD)
    };
    assert_eq!(silent.summary(), "locked-but-silent");
    assert_eq!(
        ChannelResult::none(ObdPinSet::DeutschCAndD).summary(),
        "no-traffic"
    );
}

#[test]
fn the_documented_pair_does_not_win_over_the_one_with_traffic() {
    // Exactly the Volvo case: 3/11 is documented as J1939 and produced nothing,
    // 6/14 is documented as OBD and produced J1939 frames.
    let results = [
        result(ObdPinSet::J1962Pins3And11, 0, 0),
        result(ObdPinSet::J1962Pins6And14, 128, 0),
    ];
    assert_eq!(best(&results).unwrap().pair, ObdPinSet::J1962Pins6And14);
}

#[test]
fn j1939_beats_a_merely_live_pair() {
    let results = [
        result(ObdPinSet::J1962Pins6And14, 0, 500),
        result(ObdPinSet::J1962Pins3And11, 12, 0),
    ];
    assert_eq!(best(&results).unwrap().pair, ObdPinSet::J1962Pins3And11);
}

#[test]
fn a_live_non_j1939_pair_is_still_reported_when_nothing_better_exists() {
    // Knowing the wiring is good and the protocol is unexpected is far more
    // actionable than "nothing found".
    let results = [
        result(ObdPinSet::J1962Pins3And11, 0, 0),
        result(ObdPinSet::J1962Pins6And14, 0, 90),
    ];
    assert_eq!(best(&results).unwrap().pair, ObdPinSet::J1962Pins6And14);
}

#[test]
fn nothing_anywhere_yields_nothing() {
    let results = [
        result(ObdPinSet::J1962Pins3And11, 0, 0),
        result(ObdPinSet::J1962Pins6And14, 0, 0),
    ];
    assert!(best(&results).is_none());
}

#[test]
fn deutsch_f_and_g_is_not_a_can_pair() {
    // J1708. Its signalling still toggles a transceiver's RXD line, which is
    // how it burned a field day looking like a live-but-undecodable bus.
    assert!(!ObdPinSet::DeutschFAndG.is_can());
    assert!(!ObdPinSet::DeutschFAndG.j1939_candidate());
}

#[test]
fn every_can_pair_stays_a_j1939_candidate() {
    // Ruling a CAN pair out on paper is what the Volvo punished.
    for pair in [
        ObdPinSet::J1962Pins6And14,
        ObdPinSet::J1962Pins3And11,
        ObdPinSet::DeutschCAndD,
    ] {
        assert!(
            pair.j1939_candidate(),
            "{} should stay a candidate",
            pair.label()
        );
    }
}

#[test]
fn probe_order_leads_with_the_pair_that_worked() {
    assert_eq!(ObdPinSet::PROBE_ORDER[0], ObdPinSet::J1962Pins6And14);
    assert!(ObdPinSet::PROBE_ORDER.contains(&ObdPinSet::J1962Pins3And11));
    // The J1708 pair is never probed as CAN.
    assert!(!ObdPinSet::PROBE_ORDER.contains(&ObdPinSet::DeutschFAndG));
}
