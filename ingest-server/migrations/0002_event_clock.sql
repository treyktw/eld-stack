-- Preserve unsynchronized events without pretending uptime is UTC.
ALTER TABLE events ALTER COLUMN ts_ms DROP NOT NULL;
ALTER TABLE events ADD COLUMN IF NOT EXISTS monotonic_ms BIGINT;
