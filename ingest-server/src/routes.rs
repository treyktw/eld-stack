//! HTTP handlers + router.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use sqlx::postgres::PgPool;
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::db;
use crate::dedup::DedupCache;
use crate::error::AppError;
use crate::model::{Ack, IngestBody};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub dedup: Arc<Mutex<DedupCache>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ingest", post(ingest))
        .route("/devices/{id}/status", get(device_status))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// The bearer token, if the header is well formed.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Identify the caller from its credential alone.
///
/// The token names the device; the caller does not get to claim an identity and
/// have us look up the matching secret. That ordering is what allows
/// authentication to finish before the request body is deserialized, and it
/// removes the question "does this device id exist?" from the attack surface —
/// there is nothing to probe but the token itself.
fn authenticate(config: &Config, headers: &HeaderMap) -> Result<String, AppError> {
    let presented = bearer(headers).ok_or_else(|| {
        // WARN, so it lands as a Sentry breadcrumb rather than an issue.
        // Individually this is noise; in a burst attached to a real error it is
        // the context that explains it.
        tracing::warn!("ingest: request with no bearer credential");
        AppError::Unauthorized
    })?;
    config
        .device_for_token(presented)
        .map(str::to_owned)
        .ok_or_else(|| {
            // Deliberately says nothing about the token. A device whose
            // credential was rotated without updating the unit will sit here
            // forever, queueing to a full SD card while every upload 401s — so
            // the count matters even though the identity cannot be known.
            tracing::warn!("ingest: bearer credential matched no configured device");
            AppError::Unauthorized
        })
}

/// Authenticate a request as one specific device.
fn check_auth(config: &Config, headers: &HeaderMap, device_id: &str) -> Result<(), AppError> {
    let authenticated = authenticate(config, headers)?;
    // Compared after authentication, so this only decides whether an already
    // valid credential is being used for someone else's data.
    if authenticated.as_bytes().ct_eq(device_id.as_bytes()).into() {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    // Raw bytes, not `Json<IngestBody>`: an extractor would deserialize the
    // whole batch before the handler runs, so an unauthenticated caller could
    // make the server parse arbitrary JSON. Taking bytes lets the credential be
    // checked first and the body only be looked at once the caller is known.
    body: axum::body::Bytes,
) -> Result<Json<Ack>, AppError> {
    let authenticated_device = authenticate(&state.config, &headers)?;

    // Every log line for this batch carries the device and byte count. Without
    // a span the per-record failures below are unattributable once more than
    // one device is uploading, which in production is always.
    let span = tracing::info_span!(
        "ingest",
        device_id = %authenticated_device,
        bytes = body.len()
    );
    let _entered = span.enter();

    let parsed: IngestBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("body is not a valid ingest payload".into()))?;

    let events = parsed.into_vec();
    if events.is_empty() {
        return Err(AppError::BadRequest("empty ingest batch".into()));
    }
    if events.len() > 250 {
        return Err(AppError::BadRequest(
            "ingest batch exceeds 250 records".into(),
        ));
    }
    // Every record must belong to the authenticated device. Previously the
    // batch declared its own device and the token was looked up to match; now
    // the token decides, and a batch carrying anyone else's records is refused
    // rather than trusted.
    if events
        .iter()
        .any(|event| event.device_id != authenticated_device)
    {
        return Err(AppError::Unauthorized);
    }

    let mut ack = Ack::default();
    let mut max_seq = 0i64;
    let mut max_ts: Option<i64> = None;
    let mut device_id: Option<String> = None;

    for env in events {
        if env.validate().is_err() {
            ack.rejected += 1;
            continue;
        }

        let data_hash = env.data_hash();
        let event_id = env.event_id(&data_hash);

        // Fast path: skip the DB round-trip for an id we know is stored.
        // Read-only — the cache is written after the database accepts it, never
        // before, or a failed insert would poison it and turn the device's
        // retry into a "duplicate" for a record that exists nowhere.
        let seen = state
            .dedup
            .lock()
            .map(|c| c.contains(&event_id))
            .unwrap_or(false);
        if seen {
            ack.duplicate += 1;
            continue;
        }

        // Authoritative: Postgres UNIQUE decides inserted vs duplicate.
        match db::insert_event(&state.pool, &env, &data_hash, &event_id).await {
            Ok(true) => {
                if let Ok(mut c) = state.dedup.lock() {
                    c.record(&event_id);
                }
                ack.accepted += 1;
                max_seq = max_seq.max(env.seq);
                if let Some(ts_ms) = env.ts_ms {
                    max_ts = Some(max_ts.map_or(ts_ms, |current| current.max(ts_ms)));
                }
                device_id.get_or_insert_with(|| env.device_id.clone());
            }
            Ok(false) => {
                if let Ok(mut c) = state.dedup.lock() {
                    c.record(&event_id);
                }
                ack.duplicate += 1;
            }
            Err(e) => {
                // Ours, not the device's. Counting this as `rejected` and
                // answering 200 told the device the record was handled, and the
                // device then deleted it from the SD queue — losing a
                // compliance record to a transient database blip.
                //
                // Reported with the record's identity attached: this is the one
                // failure in this service that can destroy a legally required
                // record, and "insert failed" alone does not say which one.
                tracing::error!(
                    device_id = %authenticated_device,
                    seq = env.seq,
                    ts_ms = env.ts_ms,
                    event_id = %event_id,
                    "ingest: storing an event failed; the device will be told to retry: {e}"
                );
                ack.failed += 1;
            }
        }
    }

    // Newly inserted events were placed in the pipeline outbox by the same DB
    // transaction. The background worker forwards them independently.
    if let Some(dev) = device_id {
        if let Err(e) =
            db::upsert_device(&state.pool, &dev, max_seq, max_ts, ack.accepted as i64).await
        {
            // The events themselves are stored; this only updates the device's
            // last-seen cursor. Reported rather than ignored because a stale
            // cursor makes a live device look silent on the fleet view, which
            // is how a real outage gets dismissed as a reporting glitch.
            tracing::error!(
                device_id = %dev,
                max_seq,
                "ingest: device cursor update failed after storing events: {e}"
            );
        }
    }

    // Anything we failed to store must not be reported as success. The device
    // only inspects the HTTP status — it never parses this body — so a 200 here
    // is an instruction to delete the record. Fail the whole batch instead: the
    // records that did land are deduplicated on retry, so a repeat is cheap and
    // a loss is not.
    if ack.failed > 0 {
        tracing::warn!(
            "ingest: {} of {} records failed to store; failing the batch so the device retries",
            ack.failed,
            ack.accepted + ack.duplicate + ack.rejected + ack.failed
        );
        return Err(AppError::Unavailable);
    }

    Ok(Json(ack))
}

async fn device_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    check_auth(&state.config, &headers, &id)?;
    match db::device_status(&state.pool, &id).await? {
        Some(v) => Ok(Json(v)),
        None => Err(AppError::BadRequest("unknown device".into())),
    }
}
