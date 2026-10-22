#!/usr/bin/env bash
# Open the serial monitor without flashing.
#
# Usage:
#   tools/monitor.sh                 # auto-detect port
#   tools/monitor.sh -p /dev/ttyUSB0 # explicit port
set -euo pipefail

cd "$(dirname "$0")/.."

PORT_ARG=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    -p|--port) PORT_ARG=(--port "$2"); shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

exec espflash monitor "${PORT_ARG[@]}"
