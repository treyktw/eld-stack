#!/usr/bin/env bash
#
# Generate the firmware signing keypair.
#
# Run this ONCE. The private key signs every firmware image this fleet will ever
# accept, and the public half is burned into eFuse or compiled into the image.
#
#   ./tools/generate-signing-key.sh ~/eld-firmware-signing-key.pem
#
# Two failure modes, both permanent, both worth reading before running this:
#
#   LOSING the private key means no device can ever be updated again. A unit
#   with a write-protected eFuse key will reject every future image, and there
#   is no recovery short of physical reflashing.
#
#   LEAKING the private key means anyone can sign firmware your trucks will
#   install and run. Rotating away from it requires reaching every device with
#   an update signed by the OLD key — so a leak discovered after units ship is
#   a recall, not a config change.
#
# Back it up offline. Do not put it in the repository, a password manager note,
# or any cloud drive that syncs automatically.
set -euo pipefail

OUT="${1:-}"
if [[ -z "$OUT" ]]; then
  echo "usage: $0 <output-path.pem>" >&2
  echo "  e.g. $0 ~/eld-firmware-signing-key.pem" >&2
  exit 2
fi

if [[ -e "$OUT" ]]; then
  # Never silently overwrite: the existing file may be the only copy of a key
  # that thousands of devices trust.
  echo "refusing to overwrite existing file: $OUT" >&2
  exit 1
fi

umask 077
openssl genpkey -algorithm ed25519 -out "$OUT"
chmod 600 "$OUT"

PUBHEX="$(openssl pkey -in "$OUT" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d " \n")"

echo "private key written to: $OUT (mode 600)"
echo
echo "Public key, as a Rust array for firmware/src/ota/key.rs:"
echo
echo "pub const BUILD_TIME_PUBLIC_KEY: [u8; PUBLIC_KEY_LEN] = ["
echo "$PUBHEX" | fold -w2 | paste -sd' ' - | fold -w 72 -s | sed 's/^/    /; s/\([0-9a-f][0-9a-f]\)/0x\1,/g'
echo "];"
echo
echo "Public key, hex (for eFuse provisioning):"
echo "$PUBHEX"
