//! What to ask the ECU next, and when.
//!
//! The pacing and the discovery-to-polling transition decide whether the device
//! produces useful telemetry or floods the bus with questions nothing answers.

use eld_decoders_test::obd::poller::{
    ObdPoller, PollAction, DISCOVERY_TIMEOUT_MS, MAX_DISCOVERY_ATTEMPTS, POLL_INTERVAL_MS,
    WANTED_PIDS,
};

/// The captured 2013 Volvo: 0x05, 0x0C, 0x0D, 0x11 in the first range, plus
/// 0x20 and 0x40 advertised so 0x42 can be reached.
fn answer_like_the_volvo(p: &mut ObdPoller) {
    p.on_support_response(0x00, [0x08, 0x18, 0x80, 0x01]);
    p.on_support_response(0x20, [0x00, 0x00, 0x00, 0x01]);
    p.on_support_response(0x40, [0x40, 0x00, 0x00, 0x00]);
}

#[test]
fn asks_for_the_first_support_range_first() {
    let mut p = ObdPoller::new();
    assert!(p.discovering());
    assert_eq!(p.poll(POLL_INTERVAL_MS), PollAction::Discover(0x00));
}

#[test]
fn paces_requests() {
    // One in flight at a time. OBD-II has no flow control for concurrent
    // requests to a broadcast address.
    let mut p = ObdPoller::new();
    let t = POLL_INTERVAL_MS;
    assert_eq!(p.poll(t), PollAction::Discover(0x00));
    assert_eq!(p.poll(t + 1), PollAction::Idle, "too soon for another");
    assert_eq!(p.poll(t + POLL_INTERVAL_MS - 1), PollAction::Idle);
}

#[test]
fn moves_to_polling_once_discovery_completes() {
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;
    p.poll(t);
    answer_like_the_volvo(&mut p);

    // Walk until it leaves discovery.
    for _ in 0..10 {
        t += POLL_INTERVAL_MS;
        if let PollAction::Read(_) = p.poll(t) {
            break;
        }
    }
    assert!(!p.discovering(), "should be polling data by now");
    assert!(
        !p.is_blind(),
        "the ECU answered, so this is not a blind fallback"
    );
}

#[test]
fn only_polls_pids_the_vehicle_supports() {
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;
    p.poll(t);
    answer_like_the_volvo(&mut p);

    let mut seen = Vec::new();
    for _ in 0..40 {
        t += POLL_INTERVAL_MS;
        if let PollAction::Read(pid) = p.poll(t) {
            if !seen.contains(&pid) {
                seen.push(pid);
            }
        }
    }

    seen.sort_unstable();
    // Of the wanted list, this truck offers exactly these.
    assert_eq!(seen, vec![0x05, 0x0C, 0x0D, 0x11, 0x42]);

    // Asking for these forever would cost a frame and a timeout every cycle.
    assert!(!seen.contains(&0xA6), "odometer is not supported here");
    assert!(!seen.contains(&0x2F), "fuel level is not supported here");
}

#[test]
fn rotates_rather_than_repeating_one_pid() {
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;
    p.poll(t);
    answer_like_the_volvo(&mut p);

    let mut reads = Vec::new();
    for _ in 0..30 {
        t += POLL_INTERVAL_MS;
        if let PollAction::Read(pid) = p.poll(t) {
            reads.push(pid);
        }
    }

    assert!(reads.len() >= 10, "should be reading steadily");
    let first = reads[0];
    assert!(
        reads.iter().any(|&p| p != first),
        "a single PID polled forever starves everything else"
    );
}

#[test]
fn retries_discovery_before_giving_up() {
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;

    assert_eq!(p.poll(t), PollAction::Discover(0x00));
    // No response: after the timeout it should ask again, not stall.
    t += DISCOVERY_TIMEOUT_MS + POLL_INTERVAL_MS;
    assert_eq!(p.poll(t), PollAction::Discover(0x00));
    assert!(p.discovering());
}

#[test]
fn falls_back_to_blind_polling_when_the_ecu_never_answers() {
    // A vehicle that ignores 0x00 may still answer real PIDs. Reporting
    // nothing at all would be the worse failure.
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;

    for _ in 0..(MAX_DISCOVERY_ATTEMPTS as usize + 2) {
        p.poll(t);
        t += DISCOVERY_TIMEOUT_MS + POLL_INTERVAL_MS;
    }

    assert!(p.is_blind(), "should have given up on discovery");
    assert!(!p.discovering());

    // And it polls the wanted list unconditionally.
    let mut got_read = false;
    for _ in 0..5 {
        t += POLL_INTERVAL_MS;
        if let PollAction::Read(pid) = p.poll(t) {
            assert!(WANTED_PIDS.contains(&pid));
            got_read = true;
        }
    }
    assert!(got_read, "blind mode still has to ask for something");
}

#[test]
fn a_vehicle_supporting_none_of_the_wanted_pids_goes_idle_rather_than_spinning() {
    let mut p = ObdPoller::new();
    let mut t = POLL_INTERVAL_MS;
    p.poll(t);
    // Supports 0x01 and 0x02 only — nothing on the wanted list.
    p.on_support_response(0x00, [0xC0, 0x00, 0x00, 0x00]);

    let mut actions = Vec::new();
    for _ in 0..6 {
        t += POLL_INTERVAL_MS;
        actions.push(p.poll(t));
    }

    assert!(
        actions.iter().all(|a| matches!(a, PollAction::Idle)),
        "nothing wanted is supported, so there is nothing to ask for"
    );
}

#[test]
fn speed_is_polled_first() {
    // It drives trip detection and mileage; a stale speed is the reading most
    // likely to produce a wrong duty-status inference.
    assert_eq!(WANTED_PIDS[0], 0x0D);
}
