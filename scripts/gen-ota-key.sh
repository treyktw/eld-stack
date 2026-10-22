#!/usr/bin/env bash
#
# Generate your own OTA signing keypair, locally.
#
#   scripts/gen-ota-key.sh                      # writes ./ota-signing-key.pem
#   scripts/gen-ota-key.sh ~/keys/eld-ota.pem   # or wherever you keep keys
#   scripts/gen-ota-key.sh --apply ~/keys/eld-ota.pem
#
# The repository ships with the public half of a throwaway TEST key in
# firmware/src/ota/key.rs. Its private half no longer exists, so nothing signed
# by anyone can verify against it except the committed test fixture. To sign
# real images, generate your own pair with this script and either paste the
# printed Rust array into key.rs or pass --apply to rewrite it in place.
#
# The private key is written with mode 600 and is NEVER to be committed:
# .gitignore excludes *.pem, and this script refuses to write inside the repo
# unless you insist (ALLOW_IN_REPO=1). Sign manifests with
# firmware/tools/sign-manifest.sh; the canonical message format is pinned by
# firmware-tests/tests/ota_signing_chain.rs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEY_RS="$ROOT/firmware/src/ota/key.rs"

APPLY=0
if [[ "${1:-}" == "--apply" ]]; then APPLY=1; shift; fi
OUT="${1:-./ota-signing-key.pem}"

command -v openssl >/dev/null || { echo "openssl is required" >&2; exit 1; }

case "$(cd "$(dirname "$OUT")" && pwd)/" in
  "$ROOT"/*)
    if [[ "${ALLOW_IN_REPO:-0}" != "1" ]]; then
      echo "refusing to write a private key inside the repository ($OUT)." >&2
      echo "pick a path outside $ROOT, or set ALLOW_IN_REPO=1 if you really mean it." >&2
      exit 1
    fi ;;
esac

# Generation, the Rust-array printout and the refusal to overwrite all live in
# the firmware's own tool so there is one implementation.
"$ROOT/firmware/tools/generate-signing-key.sh" "$OUT"

if [[ "$APPLY" == "1" ]]; then
  HEX="$(openssl pkey -in "$OUT" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n')"
  python3 - "$KEY_RS" "$HEX" <<'PY'
import re, sys
path, hexkey = sys.argv[1], sys.argv[2]
src = open(path).read()
b = [f"0x{hexkey[i:i+2]}" for i in range(0, 64, 2)]
arr = ("pub const BUILD_TIME_PUBLIC_KEY: [u8; PUBLIC_KEY_LEN] = [\n    "
       + ", ".join(b[:16]) + ",\n    " + ", ".join(b[16:]) + ",\n];")
new, n = re.subn(r"pub const BUILD_TIME_PUBLIC_KEY: \[u8; PUBLIC_KEY_LEN\] = \[.*?\];", arr, src, flags=re.S)
assert n == 1, "could not find BUILD_TIME_PUBLIC_KEY in key.rs"
open(path, "w").write(new)
PY
  echo
  echo "Rewrote BUILD_TIME_PUBLIC_KEY in firmware/src/ota/key.rs."
  echo "firmware-tests' signing-chain fixture was signed with the old test key;"
  echo "re-sign it with firmware/tools/sign-manifest.sh (see that test's docs)."
fi
