#!/usr/bin/env bash
#
# Burn the firmware signing public key into eFuse, then write-protect it.
#
#   ./tools/provision-efuse-key.sh --port /dev/cu.usbserial-XXXX --check
#   ./tools/provision-efuse-key.sh --port /dev/cu.usbserial-XXXX --burn
#
# This is a MANUFACTURE step and it is IRREVERSIBLE. eFuse bits burn from 0 to 1
# and never back. Once the write-protect bit is set:
#
#   - The key in that block is permanent for the life of the chip.
#   - A wrong key means the unit can never accept a signed update again. Not
#     "until we fix it" — ever. The only recovery is physical reflashing, on a
#     device that is behind a dash.
#
# Which is why --check exists and why --burn refuses to proceed unless the value
# you supply matches the key compiled into the firmware. Run --check first, on
# every unit.
#
# Why bother, when the key is already compiled in: a compiled-in key is only as
# trustworthy as the image holding it. Anyone who can reflash the device
# replaces the key and the firmware together. An eFuse key survives a firmware
# replacement, so it is what makes signing meaningful against an attacker with
# physical access — provided Secure Boot is also enabled, which is a separate
# step.
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

KEY_RS="$(dirname "$0")/../src/ota/key.rs"

# The general-purpose user block is BLOCK3 on both parts, but espefuse names it
# per chip. Detected rather than assumed: passing an ESP32 block name to an S3
# either errors or, worse, matches a different fuse — and burning is permanent.
detect_block() {
  local chip
  chip="$("$ESPEFUSE" --port "$PORT" summary 2>/dev/null \
    | grep -oiE 'esp32-?s3|esp32' | head -1 | tr 'A-Z' 'a-z' | tr -d '-')"
  case "$chip" in
    esp32s3) BLOCK="BLOCK_USR_DATA"; WR_DIS="USER_DATA" ;;
    esp32)   BLOCK="BLOCK3";         WR_DIS="BLOCK3" ;;
    *) die "could not identify the chip on $PORT (saw: '${chip:-nothing}'). \
Refusing to guess which eFuse block to burn." ;;
  esac
  echo "Detected chip:            ${chip}"
}

PORT=""; MODE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)  PORT="${2:-}"; shift 2 ;;
    --check) MODE="check"; shift ;;
    --burn)  MODE="burn";  shift ;;
    *) die "unknown argument: $1" ;;
  esac
done
[[ -n "$PORT" ]] || die "--port is required"
[[ -n "$MODE" ]] || die "one of --check or --burn is required"

command -v espefuse.py >/dev/null 2>&1 || command -v espefuse >/dev/null 2>&1 \
  || die "espefuse not found. It ships with esptool: pip install esptool"
ESPEFUSE="$(command -v espefuse.py || command -v espefuse)"

# The key the firmware will verify against. Read from source rather than passed
# in, so the burned value cannot silently disagree with the shipped image.
[[ -r "$KEY_RS" ]] || die "cannot read $KEY_RS"
EXPECTED="$(
  sed -n '/BUILD_TIME_PUBLIC_KEY/,/\];/p' "$KEY_RS" \
    | grep -oE '0x[0-9a-fA-F]{2}' | sed 's/0x//' | tr -d '\n' | tr 'A-F' 'a-f'
)"
[[ ${#EXPECTED} -eq 64 ]] || die "could not parse a 32-byte key from $KEY_RS (got ${#EXPECTED} hex chars)"

if [[ "$EXPECTED" == "0000000000000000000000000000000000000000000000000000000000000000" ]]; then
  die "BUILD_TIME_PUBLIC_KEY is the unconfigured sentinel — generate a keypair first"
fi

echo "Key from firmware source: $EXPECTED"
detect_block
echo "Target block:             $BLOCK (write-protect field: $WR_DIS)"
echo

read_block() {
  # Summary output includes the block contents; extract them as flat hex.
  "$ESPEFUSE" --port "$PORT" summary 2>/dev/null \
    | sed -n "/$BLOCK/,/^$/p" \
    | grep -oE '[0-9a-f]{2}( [0-9a-f]{2})+' \
    | tr -d ' \n' | tr 'A-F' 'a-f' || true
}

case "$MODE" in
  check)
    echo "Reading eFuse (no changes will be made)..."
    CURRENT="$(read_block)"
    if [[ -z "$CURRENT" || "$CURRENT" =~ ^0+$ ]]; then
      echo "  $BLOCK is empty — this unit is not yet provisioned."
      echo "  Run again with --burn to write the key."
    elif [[ "$CURRENT" == "$EXPECTED"* ]]; then
      echo "  $BLOCK already holds the expected key."
      echo "  Confirm the write-protect bit is set in the summary above."
    else
      echo "  $BLOCK holds a DIFFERENT value:"
      echo "    on chip:  $CURRENT"
      echo "    expected: $EXPECTED"
      echo
      die "refusing to touch a unit already provisioned with another key"
    fi
    ;;

  burn)
    CURRENT="$(read_block)"
    if [[ -n "$CURRENT" && ! "$CURRENT" =~ ^0+$ ]]; then
      die "$BLOCK is not empty — refusing to burn over an existing value"
    fi

    echo "About to burn — IRREVERSIBLY — into $BLOCK:"
    echo "  $EXPECTED"
    echo
    echo "After this the block is write-protected and can never be changed."
    echo "A wrong key here permanently ends this unit's ability to be updated."
    echo
    read -r -p "Type the last 8 characters of the key to confirm: " CONFIRM
    [[ "$CONFIRM" == "${EXPECTED: -8}" ]] || die "confirmation did not match — nothing was burned"

    TMPKEY="$(mktemp)"; trap 'rm -f "$TMPKEY"' EXIT
    # espefuse takes raw bytes, not hex.
    echo "$EXPECTED" | xxd -r -p > "$TMPKEY"
    [[ $(wc -c < "$TMPKEY") -eq 32 ]] || die "key file is not 32 bytes"

    "$ESPEFUSE" --port "$PORT" burn_block_data "$BLOCK" "$TMPKEY" \
      || die "burning the key failed — the write-protect bit was NOT set"

    echo
    echo "Key burned. Verifying before write-protecting..."
    AFTER="$(read_block)"
    [[ "$AFTER" == "$EXPECTED"* ]] \
      || die "read-back does not match. NOT setting write-protect; the block is still writable."

    echo "Read-back matches. Setting the write-protect bit."
    "$ESPEFUSE" --port "$PORT" write_protect_efuse "$WR_DIS" \
      || die "write-protect failed. The key is burned but the block is still writable, \
so the firmware will REFUSE it (key::resolve returns EfuseNotWriteProtected). Retry this step."

    echo
    echo "Provisioned. The firmware will now prefer the eFuse key over the"
    echo "compiled-in one. Enabling Secure Boot is a separate step and is what"
    echo "stops an attacker replacing the whole image."
    ;;
esac
