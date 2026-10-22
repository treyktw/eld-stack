//! Error reporting and structured logging.
//!
//! This service is the one place a compliance record can be lost. A device
//! uploads, deletes its local copy on a 2xx, and moves on; if the server drops
//! the record and nobody is told, the loss is discovered during an audit —
//! months later, with no way to recover it. Logging that to stdout is not
//! enough, because nobody reads stdout on a healthy-looking service.
//!
//! `tracing::error!` is the reporting API. `sentry-tracing` forwards every
//! ERROR-level event as a Sentry event, so the existing call sites report
//! without a second call beside each one — the kind of duplication that decays
//! until only some errors are covered.

use std::borrow::Cow;

use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Keeps the Sentry client alive for the process lifetime.
///
/// Held by `main` and dropped on shutdown so queued events are flushed. Dropped
/// early, the last errors before exit — which are usually the interesting ones —
/// are discarded.
pub struct TelemetryGuard(#[allow(dead_code)] Option<sentry::ClientInitGuard>);

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Initialise Sentry, then logging.
///
/// Order matters: the `sentry-tracing` layer needs a client to forward to, and
/// a subscriber installed first would silently drop everything logged before
/// the client existed.
///
/// Without `SENTRY_DSN` this is logging only. That is the correct behaviour for
/// local development and it is stated in the startup log, because a service
/// that silently reports nowhere looks exactly like one that has no errors.
pub fn init() -> TelemetryGuard {
    let dsn = std::env::var("SENTRY_DSN").unwrap_or_default();

    let guard = if dsn.trim().is_empty() {
        None
    } else {
        let release = std::env::var("SENTRY_RELEASE")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(Cow::Owned);

        let client = sentry::init(sentry::ClientOptions {
            dsn: dsn.parse().ok(),
            environment: Some(Cow::Owned(env_or("SENTRY_ENVIRONMENT", "production"))),
            release,
            // Ingest handles telemetry, not people. There is no user context to
            // attach and nothing about a request that should be collected by
            // default.
            send_default_pii: false,
            attach_stacktrace: true,
            traces_sample_rate: env_or("SENTRY_TRACES_SAMPLE_RATE", "0.1")
                .parse()
                .unwrap_or(0.1),
            ..Default::default()
        });

        sentry::configure_scope(|scope| {
            scope.set_tag("service", "eld-ingest-server");
        });

        Some(client)
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,sqlx=warn,h2=warn"));

    // JSON, so this and any other structured-logging service can be
    // ingested by one pipeline without per-service parsing rules.
    let fmt_layer = fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true)
        .with_timer(UtcTime::rfc_3339());

    let sentry_layer = sentry_tracing::layer().event_filter(|metadata| {
        // WARN becomes a breadcrumb rather than an issue. Warnings here are
        // routine (a device retrying, a batch partially deduplicated) and would
        // drown the failures that matter; as breadcrumbs they still show up
        // attached to the error that follows.
        match *metadata.level() {
            tracing::Level::ERROR => sentry_tracing::EventFilter::Event,
            tracing::Level::WARN => sentry_tracing::EventFilter::Breadcrumb,
            _ => sentry_tracing::EventFilter::Ignore,
        }
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(sentry_layer)
        .try_init();

    if guard.is_none() {
        tracing::warn!("SENTRY_DSN is not set — errors are logged locally and reported nowhere");
    }

    TelemetryGuard(guard)
}
