-- ELD ingest schema. Idempotent (safe to run at every startup).

CREATE TABLE IF NOT EXISTS events (
    id           BIGSERIAL PRIMARY KEY,
    device_id    TEXT        NOT NULL,
    stream       TEXT        NOT NULL,
    seq          BIGINT      NOT NULL,
    ts_ms        BIGINT      NOT NULL,
    type         TEXT        NOT NULL,
    payload      JSONB       NOT NULL,
    payload_hash TEXT        NOT NULL,
    event_id     TEXT        NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Legacy key retained for existing databases; canonical dedup is the
    -- device-wide sequence index below.
    UNIQUE (device_id, stream, seq)
);

CREATE INDEX IF NOT EXISTS events_device_ts_idx ON events (device_id, ts_ms);
CREATE INDEX IF NOT EXISTS events_stream_idx ON events (stream);
CREATE UNIQUE INDEX IF NOT EXISTS events_device_seq_unique_idx ON events (device_id, seq);

-- Trip dedup by trip id (payload.id), independent of seq, so the same trip
-- arriving via ELD-direct and phone-relay collapses to one row.
CREATE UNIQUE INDEX IF NOT EXISTS events_trip_unique_idx
    ON events (device_id, ((payload ->> 'id')))
    WHERE type = 'driver_event';

-- Lightweight per-device status, updated on each accepted batch.
CREATE TABLE IF NOT EXISTS devices (
    device_id     TEXT PRIMARY KEY,
    last_seq      BIGINT,
    last_ts_ms    BIGINT,
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    events_total  BIGINT NOT NULL DEFAULT 0
);
