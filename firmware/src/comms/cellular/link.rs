//! Cellular link state machine.
//!
//! Pure: it takes events and time, and returns the next action. No UART here, so
//! every transition — including the ones that only happen in a tunnel at 2am —
//! is reachable from a host test.
//!
//! The job is narrower than it looks. The link does not need to be clever; it
//! needs to never wedge. An ELD that silently stops uploading is worse than one
//! that reports itself offline, because the carrier believes records are
//! reaching the server when they are queued on an SD card.

use super::at::Registration;

/// Where the modem is in bring-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Powered down or not yet talked to.
    Down,
    /// Powered, waiting for it to answer `AT`.
    Booting,
    /// Answering, waiting for the SIM.
    WaitingForSim,
    /// SIM ready, waiting to register on a network.
    Registering,
    /// Registered, configuring the APN and opening packet data.
    ActivatingData,
    /// Registered and able to carry traffic.
    Ready,
    /// Failed; waiting out a backoff before trying again.
    Backoff,
}

/// What the caller should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    /// Nothing to do yet.
    Wait,
    /// Power-cycle the modem and start over.
    PowerCycle,
    /// Send the probe for the current state.
    Probe,
    /// The link is usable.
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    /// Modem answered `AT` with `OK`.
    ModemResponded,
    /// `+CPIN: READY`.
    SimReady,
    /// SIM missing, locked, or otherwise unusable.
    SimFailed,
    /// Registration state reported.
    Registration(Registration),
    /// APN/PDP setup completed and the modem has an open data service.
    DataReady,
    /// APN/PDP setup failed.
    DataFailed,
    /// A probe timed out.
    Timeout,
    /// A data operation failed while Ready.
    TransferFailed,
}

/// First retry delay. Short, because most bring-up failures are transient.
pub const BACKOFF_INITIAL_MS: u64 = 5_000;

/// Ceiling on the backoff.
///
/// Five minutes, not an hour. A truck that drives out of a dead zone should
/// resume within a few minutes, and an ELD is not a battery-powered sensor where
/// aggressive backoff buys meaningful life — it runs off the truck.
pub const BACKOFF_MAX_MS: u64 = 300_000;

/// How long to wait for registration before giving up and power-cycling.
///
/// Generous: a cold modem in a weak-signal area can legitimately take a minute
/// or more to find a network, and power-cycling too eagerly guarantees it never
/// finishes searching.
pub const REGISTRATION_TIMEOUT_MS: u64 = 90_000;

/// Consecutive transfer failures tolerated before the link is torn down.
///
/// One failure is a blip. Repeated failures while nominally "Ready" mean the
/// modem's view of the world and ours have diverged, and the fix is to stop
/// trusting the state and rebuild it.
pub const MAX_TRANSFER_FAILURES: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct Link {
    state: LinkState,
    /// When the current state was entered.
    since_ms: u64,
    /// Current backoff, doubling on each failure.
    backoff_ms: u64,
    transfer_failures: u8,
}

impl Link {
    pub fn new(now_ms: u64) -> Self {
        Self {
            state: LinkState::Down,
            since_ms: now_ms,
            backoff_ms: BACKOFF_INITIAL_MS,
            transfer_failures: 0,
        }
    }

    pub fn state(&self) -> LinkState {
        self.state
    }

    pub fn is_ready(&self) -> bool {
        self.state == LinkState::Ready
    }

    fn enter(&mut self, state: LinkState, now_ms: u64) {
        self.state = state;
        self.since_ms = now_ms;
    }

    /// Advance on an event.
    pub fn on_event(&mut self, event: LinkEvent, now_ms: u64) {
        match (self.state, event) {
            (LinkState::Booting, LinkEvent::ModemResponded) => {
                self.enter(LinkState::WaitingForSim, now_ms);
            }
            (LinkState::WaitingForSim, LinkEvent::SimReady) => {
                self.enter(LinkState::Registering, now_ms);
            }

            // A missing or locked SIM is not something retrying fixes quickly,
            // but it must still be retried: a SIM can be seated badly and settle,
            // and a truck is not somewhere anyone will go press a button.
            (_, LinkEvent::SimFailed) => self.fail(now_ms),

            (LinkState::Registering, LinkEvent::Registration(reg)) if reg.is_usable() => {
                self.enter(LinkState::ActivatingData, now_ms);
            }

            (LinkState::ActivatingData, LinkEvent::DataReady) => {
                self.backoff_ms = BACKOFF_INITIAL_MS;
                self.transfer_failures = 0;
                self.enter(LinkState::Ready, now_ms);
            }

            (LinkState::ActivatingData, LinkEvent::DataFailed) => self.fail(now_ms),

            // Denied is terminal for this attempt — the network has actively
            // refused this SIM, so retrying immediately just repeats it.
            (LinkState::Registering, LinkEvent::Registration(Registration::Denied)) => {
                self.fail(now_ms)
            }

            // Losing registration while Ready happens constantly in real driving:
            // tunnels, terrain, tower handoff. Drop back to Registering rather
            // than tearing the modem down.
            (LinkState::Ready, LinkEvent::Registration(reg)) if !reg.is_usable() => {
                self.enter(LinkState::Registering, now_ms);
            }

            (LinkState::Ready, LinkEvent::TransferFailed) => {
                self.transfer_failures = self.transfer_failures.saturating_add(1);
                if self.transfer_failures >= MAX_TRANSFER_FAILURES {
                    self.fail(now_ms);
                }
            }

            (_, LinkEvent::Timeout) => self.fail(now_ms),

            // Any other pairing is a state/event combination that carries no
            // meaning — ignore rather than guess.
            _ => {}
        }
    }

    fn fail(&mut self, now_ms: u64) {
        self.backoff_ms = (self.backoff_ms.saturating_mul(2)).min(BACKOFF_MAX_MS);
        self.transfer_failures = 0;
        self.enter(LinkState::Backoff, now_ms);
    }

    /// What to do now.
    pub fn poll(&mut self, now_ms: u64) -> LinkAction {
        let elapsed = now_ms.saturating_sub(self.since_ms);

        match self.state {
            LinkState::Down => {
                self.enter(LinkState::Booting, now_ms);
                LinkAction::PowerCycle
            }
            LinkState::Backoff => {
                if elapsed >= self.backoff_ms {
                    self.enter(LinkState::Booting, now_ms);
                    LinkAction::PowerCycle
                } else {
                    LinkAction::Wait
                }
            }
            LinkState::Registering => {
                if elapsed >= REGISTRATION_TIMEOUT_MS {
                    self.fail(now_ms);
                    LinkAction::Wait
                } else {
                    LinkAction::Probe
                }
            }
            LinkState::Booting | LinkState::WaitingForSim | LinkState::ActivatingData => {
                LinkAction::Probe
            }
            LinkState::Ready => LinkAction::Ready,
        }
    }

    /// Current backoff, for logging and tests.
    pub fn backoff_ms(&self) -> u64 {
        self.backoff_ms
    }
}
