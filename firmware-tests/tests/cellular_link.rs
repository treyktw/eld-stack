//! Cellular link bring-up and recovery.
//!
//! The requirement is not cleverness, it is never wedging. An ELD that silently
//! stops uploading is worse than one reporting itself offline, because the
//! carrier believes records are reaching the server when they are on an SD card.

use eld_decoders_test::cellular::at::Registration;
use eld_decoders_test::cellular::link::{
    Link, LinkAction, LinkEvent, LinkState, BACKOFF_INITIAL_MS, BACKOFF_MAX_MS,
    MAX_TRANSFER_FAILURES, REGISTRATION_TIMEOUT_MS,
};

/// Drive a link from cold to Ready, returning it and the clock.
fn bring_up(start: u64) -> (Link, u64) {
    let mut link = Link::new(start);
    let mut t = start;

    assert_eq!(link.poll(t), LinkAction::PowerCycle);
    assert_eq!(link.state(), LinkState::Booting);

    t += 1_000;
    link.on_event(LinkEvent::ModemResponded, t);
    assert_eq!(link.state(), LinkState::WaitingForSim);

    t += 500;
    link.on_event(LinkEvent::SimReady, t);
    assert_eq!(link.state(), LinkState::Registering);

    t += 8_000;
    link.on_event(LinkEvent::Registration(Registration::Home), t);
    assert_eq!(link.state(), LinkState::ActivatingData);

    t += 500;
    link.on_event(LinkEvent::DataReady, t);
    assert_eq!(link.state(), LinkState::Ready);

    (link, t)
}

#[test]
fn reaches_ready_from_cold() {
    let (mut link, t) = bring_up(0);
    assert!(link.is_ready());
    assert_eq!(link.poll(t), LinkAction::Ready);
}

#[test]
fn roaming_registration_is_accepted() {
    let mut link = Link::new(0);
    link.poll(0);
    link.on_event(LinkEvent::ModemResponded, 1_000);
    link.on_event(LinkEvent::SimReady, 1_500);
    link.on_event(LinkEvent::Registration(Registration::Roaming), 9_000);
    assert_eq!(link.state(), LinkState::ActivatingData);
    link.on_event(LinkEvent::DataReady, 9_500);
    assert_eq!(link.state(), LinkState::Ready);
}

#[test]
fn losing_signal_falls_back_to_registering_not_a_teardown() {
    // Tunnels, terrain and tower handoff happen constantly. Power-cycling the
    // modem every time would mean it spends the drive rebooting.
    let (mut link, t) = bring_up(0);
    link.on_event(LinkEvent::Registration(Registration::Searching), t + 100);
    assert_eq!(link.state(), LinkState::Registering);

    // And it recovers without a power cycle.
    link.on_event(LinkEvent::Registration(Registration::Home), t + 5_000);
    assert_eq!(link.state(), LinkState::ActivatingData);
    link.on_event(LinkEvent::DataReady, t + 5_500);
    assert_eq!(link.state(), LinkState::Ready);
}

#[test]
fn registration_that_never_completes_does_not_hang_forever() {
    // The wedge case: searching for a network that never appears.
    let (mut link, t) = bring_up(0);
    link.on_event(LinkEvent::Registration(Registration::Searching), t);
    assert_eq!(link.state(), LinkState::Registering);

    // Still trying, inside the timeout.
    assert_eq!(
        link.poll(t + REGISTRATION_TIMEOUT_MS - 1),
        LinkAction::Probe
    );

    // Past it, the link gives up and backs off rather than searching forever.
    link.poll(t + REGISTRATION_TIMEOUT_MS);
    assert_eq!(link.state(), LinkState::Backoff);
}

#[test]
fn a_denied_sim_backs_off_instead_of_hammering_the_network() {
    let mut link = Link::new(0);
    link.poll(0);
    link.on_event(LinkEvent::ModemResponded, 1_000);
    link.on_event(LinkEvent::SimReady, 1_500);
    link.on_event(LinkEvent::Registration(Registration::Denied), 2_000);
    assert_eq!(link.state(), LinkState::Backoff);
}

#[test]
fn backoff_doubles_and_is_capped() {
    let mut link = Link::new(0);
    let mut t = 0u64;
    let mut seen = Vec::new();

    for _ in 0..10 {
        link.poll(t);
        link.on_event(LinkEvent::Timeout, t);
        seen.push(link.backoff_ms());
        t += BACKOFF_MAX_MS * 2;
    }

    assert_eq!(seen[0], BACKOFF_INITIAL_MS * 2);
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0]),
        "backoff must not shrink"
    );
    assert_eq!(*seen.last().unwrap(), BACKOFF_MAX_MS);
    // Capped, so a truck leaving a dead zone resumes in minutes, not hours.
    assert!(seen.iter().all(|&b| b <= BACKOFF_MAX_MS));
}

#[test]
fn backoff_waits_then_retries() {
    let mut link = Link::new(0);
    link.poll(0);
    link.on_event(LinkEvent::Timeout, 1_000);
    assert_eq!(link.state(), LinkState::Backoff);

    let wait = link.backoff_ms();
    assert_eq!(link.poll(1_000 + wait - 1), LinkAction::Wait);
    assert_eq!(link.poll(1_000 + wait), LinkAction::PowerCycle);
    assert_eq!(link.state(), LinkState::Booting);
}

#[test]
fn a_single_transfer_failure_does_not_tear_down_the_link() {
    let (mut link, t) = bring_up(0);
    link.on_event(LinkEvent::TransferFailed, t + 1);
    assert!(link.is_ready(), "one failure is a blip, not a broken link");
}

#[test]
fn repeated_transfer_failures_rebuild_the_link() {
    // Nominally Ready but nothing gets through: our view and the modem's have
    // diverged, and the fix is to stop trusting the state.
    let (mut link, t) = bring_up(0);
    for i in 0..MAX_TRANSFER_FAILURES {
        link.on_event(LinkEvent::TransferFailed, t + i as u64);
    }
    assert_eq!(link.state(), LinkState::Backoff);
}

#[test]
fn a_successful_registration_resets_backoff_and_failures() {
    let mut link = Link::new(0);
    let mut t = 0u64;

    // Fail a few times to grow the backoff.
    for _ in 0..3 {
        link.poll(t);
        link.on_event(LinkEvent::Timeout, t);
        t += BACKOFF_MAX_MS;
    }
    assert!(link.backoff_ms() > BACKOFF_INITIAL_MS);

    link.poll(t);
    link.on_event(LinkEvent::ModemResponded, t + 1);
    link.on_event(LinkEvent::SimReady, t + 2);
    link.on_event(LinkEvent::Registration(Registration::Home), t + 3);
    link.on_event(LinkEvent::DataReady, t + 4);

    assert!(link.is_ready());
    assert_eq!(
        link.backoff_ms(),
        BACKOFF_INITIAL_MS,
        "a good connection must not inherit the previous failure's penalty"
    );
}

#[test]
fn a_failed_sim_is_retried_rather_than_treated_as_terminal() {
    // Nobody is going to press a button on a box behind a dash, so even an
    // unpromising failure has to keep retrying.
    let mut link = Link::new(0);
    link.poll(0);
    link.on_event(LinkEvent::SimFailed, 1_000);
    assert_eq!(link.state(), LinkState::Backoff);

    let wait = link.backoff_ms();
    assert_eq!(link.poll(1_000 + wait), LinkAction::PowerCycle);
}

#[test]
fn never_reports_ready_before_registering() {
    // The states that must never claim usability.
    let mut link = Link::new(0);
    assert!(!link.is_ready());

    link.poll(0);
    assert!(!link.is_ready(), "booting is not ready");

    link.on_event(LinkEvent::ModemResponded, 100);
    assert!(
        !link.is_ready(),
        "a responding modem is not a registered one"
    );

    link.on_event(LinkEvent::SimReady, 200);
    assert!(!link.is_ready(), "a ready SIM is not a network connection");

    link.on_event(LinkEvent::Registration(Registration::Home), 300);
    assert!(!link.is_ready(), "registration is not an open data context");
}

#[test]
fn data_activation_failure_backs_off() {
    let mut link = Link::new(0);
    link.poll(0);
    link.on_event(LinkEvent::ModemResponded, 100);
    link.on_event(LinkEvent::SimReady, 200);
    link.on_event(LinkEvent::Registration(Registration::Home), 300);
    assert_eq!(link.state(), LinkState::ActivatingData);

    link.on_event(LinkEvent::DataFailed, 400);
    assert_eq!(link.state(), LinkState::Backoff);
}
