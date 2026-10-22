#!/usr/bin/env bash
# Build (release by default) and flash the ELD firmware, then open the monitor.
#
# Usage:
#   tools/flash.sh                 # release build, auto-detect port
#   tools/flash.sh --debug         # debug build
#   tools/flash.sh -p /dev/ttyUSB0 # explicit serial port
set -euo pipefail

cd "$(dirname "$0")/.."

# Provisioning values are build-time inputs because the ESP32 has no process
# environment at runtime. The local file is gitignored and must never be
# printed: its bearer token authenticates this physical ELD to ingest.
if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

PROFILE="release"
PORT_ARG=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug) PROFILE="debug"; shift ;;
    --release) PROFILE="release"; shift ;;
    -p|--port) PORT_ARG=(--port "$2"); shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

if [[ "$PROFILE" == "release" ]]; then
  echo ">> cargo build --release"
  cargo build --release
  BIN="target/xtensa-esp32s3-espidf/release/eld-firmware"
else
  echo ">> cargo build"
  cargo build
  BIN="target/xtensa-esp32s3-espidf/debug/eld-firmware"
fi

# Build the espflash argument list incrementally. macOS ships bash 3.2, where
# `set -u` makes expanding an empty array (e.g. "${PORT_ARG[@]}") an "unbound
# variable" error — so only append arrays that are actually populated.
ARGS=(flash --monitor)

# Custom partition table (partitions.csv) is applied here at flash time, not in
# the IDF build (esp-idf-sys ignores CONFIG_PARTITION_TABLE_CUSTOM).
[[ -f partitions.csv ]] && ARGS+=(--partition-table partitions.csv)
[[ ${#PORT_ARG[@]} -gt 0 ]] && ARGS+=("${PORT_ARG[@]}")
ARGS+=("$BIN")

echo ">> espflash ${ARGS[*]}"
espflash "${ARGS[@]}"
