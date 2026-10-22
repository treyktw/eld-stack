# ELD ingest contract

The wire contract between the firmware (`firmware/`) and the ingest server
(`ingest-server/`). Both the ELD-direct and phone-relay paths POST the same
envelope; the server dedups so duplicates collapse.

## Endpoint

```
POST /ingest
Content-Type: application/json
Authorization: Bearer <PER_DEVICE_TOKEN>
```

Body is one envelope **or** a JSON array of envelopes (batch from a phone relay).

## Authentication

The token identifies the device. The server resolves the bearer credential to a
device id first, then requires every envelope in the batch to carry that id — a
batch containing anyone else's records is rejected as unauthorized, not merely
as a bad request.

Two consequences worth knowing:

- **Authentication completes before the body is parsed.** An unauthenticated
  caller cannot make the server deserialize a batch.
- **`device_id` in the body is not an identity claim.** It is checked against
  the authenticated device. Sending someone else's id does not impersonate them;
  it fails.

Token comparison is constant-time and compares against every configured device
without an early exit, so response timing does not reveal whether a device id
exists.

## Responses

| Status | Meaning | What the device should do |
|---|---|---|
| `200` | Batch handled. Body reports `accepted` / `duplicate` / `rejected`. | Drop the records; they are stored or permanently unusable. |
| `401` | Unknown token, or records for another device. | Do not retry blindly — the credential or configuration is wrong. |
| `400` | Malformed body, empty batch, or over 250 records. | Do not retry; the payload will never be accepted. |
| `503` | One or more records could not be stored, through no fault of the device. | **Retry the whole batch.** Records that landed dedupe, so a repeat is cheap. |

The `503` case matters: the device only inspects the status code, so a `200`
is an instruction to delete the records from its queue. The server must never
answer `200` for a batch it failed to store.

## Envelope

```json
{
  "device_id": "eld-prototype-001",
  "seq": 1099511627776,       // per-device monotonic dedup sequence
  "ts_ms": 1780000000000,     // UTC Unix epoch milliseconds, or null before sync
  "monotonic_ms": 42000,      // observation time within this boot session
  "type": "engine_snapshot",  // validated canonical record type
  "data": { ... }             // record-specific object
}
```

`seq` is `(boot_epoch << 30) | counter`: a 23-bit boot epoch in the high bits
and a 30-bit per-boot counter in the low bits, 53 bits in total. The boot epoch
is advanced in NVS before records are issued, keeping sequences ordered across
boots. 53 bits keeps every `seq` exact as a JavaScript `number` (at most
`Number.MAX_SAFE_INTEGER`, 2^53 - 1) and well below the signed `BIGINT` maximum
used by Postgres.

`ts_ms` is never uptime disguised as UTC. It is `null` before GPS time is
trusted or after holdover expires; `monotonic_ms` remains available so the
record is retained and its ordering within the boot is unambiguous.

### Payloads

`engine_snapshot`:
```json
{ "rpm": 1450.0, "speed_mph": 55.0, "fuel_percent": 73.0,
  "odometer_mi": 124488.7, "engine_hours": 4123.5,
  "lat": 39.8283, "lon": -98.5795, "fix_age_ms": 800,
  "utc_ms": 1780000000000, "clock": "utc" }
```
(missing signals are JSON `null`)

`driver_event` (completed trip):
```json
{ "id": 1, "start_ms": 1000, "end_ms": 843000, "miles": 12.4, "source": "odometer", "confidence": 0.98 }
```

`heartbeat`:
```json
{ "reset": "power-on", "uptime_s": 42, "free_heap": 180000, "min_free_heap": 170000,
  "power": "unknown", "sd": false, "can_up": true, "bitrate": "250k", "listen_only": false,
  "rx": 1203, "ext": 1180, "j1939": 1180, "sources": 4, "known_pgns": 5, "proprietary": 2,
  "gps_enabled": false, "gps_fix": false, "wifi_online": true, "upload_queue": 0, "trip": "driving" }
```

`vin` is `{ "vin": "..." }`. `fault_code` is
`{ "faults": [{ "spn": 123, "fmi": 4, "oc": 1 }] }`.

`bus_census` carries each observed J1939 PGN/source pair, counts, timing,
known/decoded flags, and the last raw payload as hex. It is discovery evidence,
not an engine snapshot.

## Dedup

- Primary key: `UNIQUE (device_id, seq)`.
- Trips also dedup on `UNIQUE (device_id, payload.id)` so the same trip from
  both paths collapses regardless of seq.
- Server computes `data_hash = sha256(data)` and
  `event_id = sha256(device_id|seq|type|data_hash)` as a fallback
  idempotency key. An in-memory recent-`event_id` cache short-circuits obvious
  replays before the DB.

## Response (ACK)

```json
{ "accepted": 3, "duplicate": 1, "rejected": 0 }
```

Only **newly inserted** events enter the durable `pipeline_outbox`. The worker
forwards them to the `PIPELINE_URL` webhook and marks them delivered only after a 2xx
response. Network failures and unregistered devices remain queued for retry.

## Other endpoints

- `GET /health` → `{ "status": "ok" }`
- `GET /devices/{id}/status` → `{ device_id, last_seq, last_ts_ms, events_total }`

## Observability

Errors are reported to Sentry when `SENTRY_DSN` is set, and logged as
line-delimited JSON either way. `tracing::error!` is the reporting API — the
`sentry-tracing` layer forwards ERROR events as issues and WARN events as
breadcrumbs, so there is no second call to keep in sync beside each log line.

Without a DSN the service logs a warning at startup saying so. A service that
reports nowhere looks exactly like one with no errors, and that ambiguity is
worth one line at boot.

### What is reported, and why these

| Event | Level | Why it matters |
|---|---|---|
| Storing an event failed | error | The only failure here that can destroy a legally required record. Logged with device, seq and event id, because "insert failed" does not say which record was at risk. |
| Device cursor update failed | error | Events are safe, but a stale cursor makes a live device look silent on the fleet view — which is how a real outage gets dismissed as a display glitch. |
| Startup: config, database, migration, bind | error | These happen before anything serves, so otherwise the only symptom is a container that never becomes healthy. |
| Serve loop exited | error | Devices keep uploading into a closed port and queue locally until their storage fills. |
| Credential matched no device | warn | A unit whose token was rotated without being updated sits here forever, queueing to a full card while every upload 401s. |

Environment: `SENTRY_DSN`, `SENTRY_ENVIRONMENT` (default `production`),
`SENTRY_RELEASE`, `SENTRY_TRACES_SAMPLE_RATE` (default `0.1`), `RUST_LOG`.
The realtime bridge additionally uses `PIPELINE_URL` and
`PIPELINE_TOKEN`; both must be set together.

`send_default_pii` is off. Ingest handles vehicle telemetry, not people, and
there is no user context worth attaching to a request.
