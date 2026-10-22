-- Durable bridge from the device-facing ingest database to a downstream webhook.
-- The device receives success only after its event and this outbox row commit
-- together, so a realtime outage cannot create a silent data gap.
CREATE TABLE IF NOT EXISTS pipeline_outbox (
    event_id        TEXT PRIMARY KEY,
    envelope        JSONB       NOT NULL,
    attempts        INTEGER     NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS pipeline_outbox_pending_idx
    ON pipeline_outbox (next_attempt_at, created_at)
    WHERE delivered_at IS NULL;
