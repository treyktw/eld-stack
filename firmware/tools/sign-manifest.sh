#!/usr/bin/env bash
#
# Sign a firmware update manifest.
#
#   ./tools/sign-manifest.sh <key.pem> <version> <image.bin> <url>
#
# Prints the signed manifest JSON to stdout. Publish that document at the
# endpoint the devices poll.
#
# The canonical message MUST match `canonical_message()` in
# firmware/src/ota/signature.rs byte for byte:
#
#   j1939-eld-fw-v1\n<version>\n<sha256>\n<size>\n<url>\n
#
# If these two ever diverge, every device rejects every update — a fleet-wide
# outage with no remote fix, because the fix would itself need to be signed.
# `firmware-tests/tests/ota_signing_chain.rs` pins the format against this
# script's output so a divergence fails in CI instead of in the field.
set -euo pipefail

KEY="${1:-}"; VERSION="${2:-}"; IMAGE="${3:-}"; URL="${4:-}"
if [[ -z "$KEY" || -z "$VERSION" || -z "$IMAGE" || -z "$URL" ]]; then
  echo "usage: $0 <key.pem> <version> <image.bin> <url>" >&2
  exit 2
fi
[[ -r "$KEY"   ]] || { echo "cannot read key: $KEY" >&2; exit 1; }
[[ -r "$IMAGE" ]] || { echo "cannot read image: $IMAGE" >&2; exit 1; }

# Reject anything the device's parser would refuse, here rather than after a
# fleet has downloaded it.
[[ "$VERSION" =~ ^(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})$ ]] \
  || { echo "version must be MAJOR.MINOR.PATCH with no leading zeros: $VERSION" >&2; exit 1; }
[[ "$URL" == https://* ]] \
  || { echo "url must be https: $URL" >&2; exit 1; }

DIGEST="$(openssl dgst -sha256 -hex "$IMAGE" | awk '{print $NF}')"
SIZE="$(wc -c < "$IMAGE" | tr -d ' ')"

MAX=$((3 * 1024 * 1024))
if (( SIZE == 0 || SIZE > MAX )); then
  echo "image size $SIZE does not fit an OTA slot (max $MAX)" >&2
  exit 1
fi

# printf, not echo -e: the trailing newline and the absence of any other
# whitespace are part of the signed bytes.
MSG="$(mktemp)"; SIG="$(mktemp)"
trap 'rm -f "$MSG" "$SIG"' EXIT
printf 'j1939-eld-fw-v1\n%s\n%s\n%s\n%s\n' "$VERSION" "$DIGEST" "$SIZE" "$URL" > "$MSG"

# Ed25519 signs the message directly; -rawin is required and there is no
# separate digest step.
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$MSG" -out "$SIG"

SIGB64="$(base64 < "$SIG" | tr -d '\n')"

printf '{"version":"%s","url":"%s","sha256":"%s","size":%s,"required":false,"sig":"%s"}\n' \
  "$VERSION" "$URL" "$DIGEST" "$SIZE" "$SIGB64"
