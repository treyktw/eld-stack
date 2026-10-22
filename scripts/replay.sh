#!/usr/bin/env bash
#
# End-to-end dedup demo against the docker compose stack.
#
#   scripts/replay.sh                 # whole stack in docker (builds the image)
#   scripts/replay.sh --host-server   # postgres + webhook in docker, ingest-server
#                                     # built with cargo and run on the host
#   scripts/replay.sh --down          # tear the stack (and its volume) down after
#
# Flags combine, e.g. `scripts/replay.sh --host-server --down`.
#
# Simulates what the two delivery paths do in the field:
#   - the ELD uploading directly over LTE, retrying after a lost response;
#   - the phone relay later uploading an overlapping backlog it pulled over BLE.
# Both carry the same (device_id, seq) pairs, and the server must store each
# record exactly once and forward each one to the webhook exactly once.
#
# Requires: docker (compose v2), curl, jq; plus cargo for --host-server.
set -euo pipefail

HOST_SERVER=0; DOWN=0
for arg in "$@"; do
  case "$arg" in
    --host-server) HOST_SERVER=1 ;;
    --down) DOWN=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/compose.yaml")
BASE="http://127.0.0.1:${INGEST_PORT:-8080}"
SINK="http://127.0.0.1:${SINK_PORT:-9000}"

# Demo credentials; they match the defaults in deploy/compose.yaml.
DEV1=eld-demo-001; TOK1=demo-token-device-001-FAKEFAKEFAKEFAKE
DEV2=eld-demo-002; TOK2=demo-token-device-002-FAKEFAKEFAKEFAKE

for tool in docker curl jq; do
  command -v "$tool" >/dev/null || { echo "missing dependency: $tool" >&2; exit 1; }
done

SERVER_PID=""
cleanup() {
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  if [[ "$DOWN" == "1" ]]; then "${COMPOSE[@]}" --profile server down -v >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

# Start from an empty database so the expected counts below hold on re-runs.
"${COMPOSE[@]}" --profile server down -v >/dev/null 2>&1 || true

if [[ "$HOST_SERVER" == "1" ]]; then
  echo "==> starting postgres + webhook-sink (docker), ingest-server (host)"
  "${COMPOSE[@]}" up -d --wait postgres webhook-sink
  ( cd "$ROOT/ingest-server" && cargo build --release --quiet )
  DATABASE_URL="postgresql://eld_ingest:${POSTGRES_PASSWORD:-eld-demo-password}@127.0.0.1:${POSTGRES_PORT:-55432}/eld_ingest" \
  BIND_ADDR="127.0.0.1:${INGEST_PORT:-8080}" \
  INGEST_DEVICE_TOKENS_JSON="{\"$DEV1\":\"$TOK1\",\"$DEV2\":\"$TOK2\"}" \
  PIPELINE_URL="$SINK/events" \
  PIPELINE_TOKEN="${PIPELINE_TOKEN:-demo-pipeline-token-FAKEFAKEFAKEFAKEFAKE}" \
  RUST_LOG="warn" \
    "$ROOT/ingest-server/target/release/eld-ingest-server" >"$ROOT/ingest-server/target/replay-server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 60); do
    curl -sf "$BASE/health" >/dev/null && break
    kill -0 "$SERVER_PID" 2>/dev/null || { cat "$ROOT/ingest-server/target/replay-server.log" >&2; exit 1; }
    sleep 0.5
  done
else
  echo "==> starting postgres + ingest-server + webhook-sink (docker)"
  "${COMPOSE[@]}" --profile server up -d --build --wait
fi
curl -sf "$BASE/health" >/dev/null || { echo "ingest-server did not become healthy" >&2; exit 1; }

# One record. $1 device, $2 seq, $3 type, $4 data JSON.
rec() {
  jq -cn --arg d "$1" --argjson s "$2" --arg t "$3" --argjson data "$4" \
    '{device_id:$d, seq:$s, ts_ms:(1780000000000 + $s*1000), monotonic_ms:($s*1000), type:$t, data:$data}'
}
snapshot() { rec "$1" "$2" engine_snapshot "{\"rpm\":$((1300 + $2)),\"speed_mph\":59.7,\"fuel_percent\":72.4}"; }
batch() { jq -cs . ; }

# POST a batch, print "<http status> <body>".
post() {
  local token="$1" body="$2" out code
  out="$(mktemp)"
  code="$(curl -s -o "$out" -w '%{http_code}' -X POST "$BASE/ingest" \
    -H 'content-type: application/json' -H "authorization: Bearer $token" --data "$body")"
  echo "$code $(cat "$out")"; rm -f "$out"
}

step() { printf '\n--- %s\n' "$1"; }
show() { printf '    %-58s -> %s\n' "$1" "$2"; }

FAILURES=0
# POST and compare the full "<status> <body>" with what the contract says.
check() {
  local label="$1" token="$2" body="$3" want="$4" got
  got="$(post "$token" "$body")"
  if [[ "$got" == "$want" ]]; then
    show "$label" "$got"
  else
    show "$label" "$got   <-- expected: $want"
    FAILURES=$((FAILURES + 1))
  fi
}
OK200() { echo "200 {\"accepted\":$1,\"duplicate\":$2,\"rejected\":$3,\"failed\":0}"; }
UNAUTH='401 {"error":"unauthorized"}'

# Device 1, records 1..5 over LTE.
LTE_1_5="$( { for s in 1 2 3 4 5; do snapshot $DEV1 $s; done; rec $DEV1 6 vin '{"vin":"TSTDEMX00T0000042"}'; } | batch)"
# Phone relay backlog for device 1: overlaps 4..6 and adds 7..10 (incl. a trip).
PHONE_4_10="$( { for s in 4 5; do snapshot $DEV1 $s; done; rec $DEV1 6 vin '{"vin":"TSTDEMX00T0000042"}'
                 for s in 7 8 9; do snapshot $DEV1 $s; done
                 rec $DEV1 10 driver_event '{"id":1,"start_ms":1000,"end_ms":61000,"miles":0.91,"source":"odometer","confidence":0.96}'; } | batch)"
# The same trip re-sent under a different seq (e.g. after a queue rebuild):
# the per-trip unique index must collapse it too.
TRIP_AGAIN="$(rec $DEV1 11 driver_event '{"id":1,"start_ms":1000,"end_ms":61000,"miles":0.91,"source":"odometer","confidence":0.96}' | batch)"
# Device 2 with one malformed record (data must be an object).
DEV2_MIXED="$( { snapshot $DEV2 1; snapshot $DEV2 2; rec $DEV2 3 heartbeat 'null'; } | batch)"
# Device 2's valid token carrying device 1's record.
SPOOF="$(snapshot $DEV1 99 | batch)"

step "1. ELD uploads seq 1-6 over LTE"
check "POST 6 records" $TOK1 "$LTE_1_5" "$(OK200 6 0 0)"

step "2. Response was lost; ELD retries the identical batch"
check "POST same 6 records again" $TOK1 "$LTE_1_5" "$(OK200 0 6 0)"

step "3. Phone relay uploads its BLE backlog (seq 4-10, overlaps 4-6)"
check "POST 7 records" $TOK1 "$PHONE_4_10" "$(OK200 4 3 0)"

step "4. Same trip (payload.id=1) arrives again under a new seq"
check "POST 1 record" $TOK1 "$TRIP_AGAIN" "$(OK200 0 1 0)"

step "5. Second device, one malformed record"
check "POST 3 records" $TOK2 "$DEV2_MIXED" "$(OK200 2 0 1)"

step "6. Auth checks (both happen before the body is trusted)"
check "device 2's token, device 1's record" $TOK2 "$SPOOF" "$UNAUTH"
check "no token" '' "$SPOOF" "$UNAUTH"
check "unknown token" not-a-real-token-FAKEFAKEFAKEFAKEFAKE "$SPOOF" "$UNAUTH"

step "7. Server state"
show "GET /devices/$DEV1/status" "$(curl -s "$BASE/devices/$DEV1/status" -H "authorization: Bearer $TOK1")"
show "GET /devices/$DEV2/status" "$(curl -s "$BASE/devices/$DEV2/status" -H "authorization: Bearer $TOK2")"
ROWS="$("${COMPOSE[@]}" exec -T postgres psql -U eld_ingest -d eld_ingest -tAc \
  "SELECT device_id || ': ' || count(*) || ' rows, seqs ' || string_agg(seq::text, ',' ORDER BY seq) FROM events GROUP BY device_id ORDER BY device_id")"
while IFS= read -r line; do show "events table" "$line"; done <<< "$ROWS"

# The outbox worker polls every 5 s; wait until everything is delivered.
for _ in $(seq 1 30); do
  PENDING="$("${COMPOSE[@]}" exec -T postgres psql -U eld_ingest -d eld_ingest -tAc \
    "SELECT count(*) FROM pipeline_outbox WHERE delivered_at IS NULL")"
  [[ "$PENDING" == "0" ]] && break
  sleep 1
done
show "outbox rows still pending" "$PENDING"
show "webhook-sink GET /stats" "$(curl -s "$SINK/stats")"

TOTAL="$("${COMPOSE[@]}" exec -T postgres psql -U eld_ingest -d eld_ingest -tAc "SELECT count(*) FROM events")"
UNIQUE="$(curl -s "$SINK/stats" | jq .unique)"
echo
if [[ "$FAILURES" == "0" && "$TOTAL" == "12" && "$UNIQUE" == "12" && "$PENDING" == "0" ]]; then
  echo "OK: 23 records posted across retries and both paths -> 12 stored, 12 forwarded, 0 pending"
  STATUS=0
else
  echo "UNEXPECTED: $FAILURES response mismatch(es); stored=$TOTAL forwarded_unique=$UNIQUE pending=$PENDING (expected 0; 12/12/0)"
  STATUS=1
fi
exit $STATUS
