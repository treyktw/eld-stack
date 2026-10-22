//! Postgres access (sqlx, runtime-checked queries — no compile-time DB needed).

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;

use crate::model::Envelope;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Apply the schema (idempotent).
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Insert an event, ignoring duplicates. Returns true if a new row was written.
///
/// `ON CONFLICT DO NOTHING` without a target, deliberately. Three unique
/// indexes can each say "already stored": the legacy (device_id, stream, seq),
/// the canonical (device_id, seq), and the per-trip (device_id, payload.id).
/// Naming only the first let a conflict on either of the others surface as a
/// database error, so a trip re-sent under a new seq was answered 503 and the
/// device retried that batch forever.
pub async fn insert_event(
    pool: &PgPool,
    env: &Envelope,
    data_hash: &str,
    event_id: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "INSERT INTO events
            (device_id, stream, seq, ts_ms, monotonic_ms, type, payload, payload_hash, event_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT DO NOTHING",
    )
    .bind(&env.device_id)
    .bind(env.type_.stream())
    .bind(env.seq)
    .bind(env.ts_ms)
    .bind(env.monotonic_ms)
    .bind(
        serde_json::to_string(&env.type_)
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned(),
    )
    .bind(Json(&env.data))
    .bind(data_hash)
    .bind(event_id)
    .execute(&mut *tx)
    .await?;
    let inserted = result.rows_affected() > 0;
    if inserted {
        let envelope = serde_json::json!({
            "event_id": event_id,
            "payload_hash": data_hash,
            "device_id": env.device_id,
            "seq": env.seq,
            "ts_ms": env.ts_ms,
            "monotonic_ms": env.monotonic_ms,
            "type": env.type_,
            "data": env.data,
        });
        sqlx::query(
            "INSERT INTO pipeline_outbox (event_id, envelope)
             VALUES ($1, $2)
             ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(event_id)
        .bind(Json(envelope))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(inserted)
}

#[derive(Debug)]
pub struct PendingPipelineEvent {
    pub event_id: String,
    pub envelope: serde_json::Value,
    pub attempts: i32,
}

pub async fn pending_pipeline_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<PendingPipelineEvent>, sqlx::Error> {
    let rows: Vec<(String, Json<serde_json::Value>, i32)> = sqlx::query_as(
        "SELECT event_id, envelope, attempts
         FROM pipeline_outbox
         WHERE delivered_at IS NULL AND next_attempt_at <= now()
         ORDER BY created_at
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(event_id, envelope, attempts)| PendingPipelineEvent {
            event_id,
            envelope: envelope.0,
            attempts,
        })
        .collect())
}

pub async fn mark_pipeline_delivered(pool: &PgPool, event_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pipeline_outbox
         SET delivered_at = now(), last_error = NULL
         WHERE event_id = $1",
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn defer_pipeline_event(
    pool: &PgPool,
    event_id: &str,
    retry_after_seconds: i64,
    error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pipeline_outbox
         SET attempts = attempts + 1,
             next_attempt_at = now() + make_interval(secs => $2::double precision),
             last_error = $3
         WHERE event_id = $1",
    )
    .bind(event_id)
    .bind(retry_after_seconds as f64)
    .bind(error.chars().take(500).collect::<String>())
    .execute(pool)
    .await?;
    Ok(())
}

/// Update the per-device status row.
pub async fn upsert_device(
    pool: &PgPool,
    device_id: &str,
    seq: i64,
    ts_ms: Option<i64>,
    new_events: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO devices (device_id, last_seq, last_ts_ms, last_seen_at, events_total)
         VALUES ($1, $2, $3, now(), $4)
         ON CONFLICT (device_id) DO UPDATE SET
            last_seq     = GREATEST(devices.last_seq, EXCLUDED.last_seq),
            last_ts_ms   = CASE
                WHEN EXCLUDED.last_ts_ms IS NULL THEN devices.last_ts_ms
                WHEN devices.last_ts_ms IS NULL THEN EXCLUDED.last_ts_ms
                ELSE GREATEST(devices.last_ts_ms, EXCLUDED.last_ts_ms)
            END,
            last_seen_at = now(),
            events_total = devices.events_total + EXCLUDED.events_total",
    )
    .bind(device_id)
    .bind(seq)
    .bind(ts_ms)
    .bind(new_events)
    .execute(pool)
    .await?;
    Ok(())
}

/// Status row for a device (for GET /devices/:id/status).
pub async fn device_status(
    pool: &PgPool,
    device_id: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(Option<i64>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT last_seq, last_ts_ms, events_total FROM devices WHERE device_id = $1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(last_seq, last_ts_ms, events_total)| {
        serde_json::json!({
            "device_id": device_id,
            "last_seq": last_seq,
            "last_ts_ms": last_ts_ms,
            "events_total": events_total,
        })
    }))
}
