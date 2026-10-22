#!/usr/bin/env bash
# Clean build artifacts.
#
# Default: `cargo clean` only (fast, safe).
# --full : ALSO delete .embuild (the ESP-IDF toolchain + IDF checkout).
#          This forces a multi-GB re-download on the next build. Use only when
#          the toolchain itself is broken.
set -euo pipefail

cd "$(dirname "$0")/.."

FULL=0
[[ "${1:-}" == "--full" ]] && FULL=1

echo ">> cargo clean"
cargo clean

if [[ "$FULL" == "1" ]]; then
  echo ""
  echo "!! --full: about to delete .embuild (ESP-IDF toolchain + IDF)."
  echo "!! Next build will re-download several GB."
  read -r -p "Type 'yes' to continue: " confirm
  if [[ "$confirm" == "yes" ]]; then
    rm -rf .embuild
    echo ">> removed .embuild"
  else
    echo ">> skipped .embuild"
  fi
fi

echo "done."
