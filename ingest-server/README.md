# ingest-server

Rust axum service that ingests ELD upload envelopes, dedups them, and stores
them in Postgres. Newly accepted events are forwarded to a downstream webhook
through a transactional outbox.

## Run

```bash
cp .env.example .env          # set DATABASE_URL and per-device credentials
cargo run                     # applies schema on startup, then serves
```

Env (see `.env.example`):
- `DATABASE_URL` — any Postgres 14+ (`?sslmode=require` for a hosted one).
- `BIND_ADDR` — default `0.0.0.0:8080`.
- `INGEST_DEVICE_TOKENS_JSON` — required JSON object mapping each provisioned
  device ID to its unique bearer credential (minimum 32 bytes).
- `PIPELINE_URL` and `PIPELINE_TOKEN` — optional pair: a bearer-gated webhook
  that receives every newly inserted event, exactly once per `event_id` from
  the outbox's point of view (at-least-once on the wire; dedupe on `event_id`).
- `SENTRY_DSN` — optional error reporting.

## Endpoints

| Method | Path                   | Purpose                              |
|--------|------------------------|--------------------------------------|
| POST   | `/ingest`              | Accept one envelope or an array      |
| GET    | `/health`              | Liveness                             |
| GET    | `/devices/{id}/status` | Last seq/ts + total events           |

Contract + payload shapes: `../docs/ingest-contract.md`.

## Behaviour

- Authenticates the bearer token **before** parsing the body; the token, not
  the body, decides the device identity.
- Validates → fast-path in-memory dedup → `INSERT ... ON CONFLICT DO NOTHING`.
- Postgres `UNIQUE (device_id, seq)` is authoritative; trips also unique
  on `(device_id, payload.id)`.
- Returns `{ accepted, duplicate, rejected, failed }`; any `failed` turns the
  whole batch into a 503 so the device retries.
- Atomically adds each new event to a Postgres outbox, then retries delivery to
  the webhook with exponential backoff.

## Smoke test

`../scripts/replay.sh` does this end to end against docker compose. By hand:

```bash
TOKEN="$(jq -r '."eld-development-001"' <<< "$INGEST_DEVICE_TOKENS_JSON")"
curl -s localhost:8080/health
curl -s -X POST localhost:8080/ingest -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" -d '{
  "device_id":"eld-development-001","seq":1,"ts_ms":1780000000000,
  "type":"engine_snapshot","data":{"rpm":1450,"speed_mph":55}}'
# second identical POST → {"accepted":0,"duplicate":1,"rejected":0,"failed":0}
```

## Roadmap

- Replace bearer provisioning with hardware-backed request signatures or mTLS.
