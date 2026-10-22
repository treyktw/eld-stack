#!/usr/bin/env bash
#
# Back up the firmware signing key, encrypted, to somewhere durable.
#
#   ./tools/backup-signing-key.sh backup ~/eld-firmware-signing-key.pem /mnt/offline-backup/eld
#   ./tools/backup-signing-key.sh verify /mnt/offline-backup/eld/eld-firmware-signing-key-<fp>.pem.enc
#   ./tools/backup-signing-key.sh restore <backup.pem.enc> ~/eld-firmware-signing-key.pem
#
# Why encrypted rather than a straight copy: a NAS is another machine that can
# be compromised, borrowed, resold, or backed up somewhere else again. A plain
# private key sitting on it is the same exposure as a leak, and a leak of this
# particular key is a recall — every device has to be reached with an update
# signed by the OLD key before the new one means anything.
#
# The passphrase is prompted for by openssl. This script never sees it, never
# stores it, and cannot recover it. Put it in a password manager BEFORE you run
# this: a backup you cannot decrypt is not a backup.
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

# High iteration count: the passphrase is the only thing between an attacker who
# has the file and a key that signs firmware for every truck.
ITER=600000

pubkey_hex() {
  openssl pkey -in "$1" -pubout -outform DER 2>/dev/null | tail -c 32 | xxd -p -c 32
}

cmd_backup() {
  local key="${1:-}" dest="${2:-}"
  [[ -n "$key" && -n "$dest" ]] || die "usage: $0 backup <key.pem> <destination-dir>"
  [[ -r "$key" ]] || die "cannot read key: $key"
  [[ -d "$dest" ]] || die "destination is not a directory: $dest"

  local fp; fp="$(pubkey_hex "$key")" || die "not a valid Ed25519 private key: $key"
  [[ -n "$fp" ]] || die "could not derive public key from: $key"

  local short="${fp:0:16}"
  local out="$dest/eld-firmware-signing-key-$short.pem.enc"
  local pubout="$dest/eld-firmware-signing-key-$short.pub.txt"

  # Never clobber an existing backup: it may be the only surviving copy.
  [[ -e "$out" ]] && die "refusing to overwrite existing backup: $out"

  echo "Signing key fingerprint (public, safe to record): $fp"
  echo
  echo "You will be asked for a passphrase twice. Store it in a password"
  echo "manager first — this script cannot recover it."
  echo

  umask 077
  openssl enc -aes-256-cbc -pbkdf2 -iter "$ITER" -salt -in "$key" -out "$out" \
    || die "encryption failed"

  # The public half and fingerprint travel alongside, unencrypted on purpose:
  # they are not secret, and they are what lets a future restore be checked
  # against the key compiled into the firmware.
  {
    echo "ELD firmware signing key — public half"
    echo "fingerprint: $fp"
    echo "created:     $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo
    echo "This must match BUILD_TIME_PUBLIC_KEY in firmware/src/ota/key.rs,"
    echo "and the value burned into eFuse on provisioned units."
  } > "$pubout"

  echo
  echo "Now verifying the backup can actually be restored."
  echo "You will be prompted for the passphrase a third time. That is"
  echo "deliberate: re-entering it proves you can reproduce it, which reusing"
  echo "the one already in memory would not. A passphrase you cannot retype is"
  echo "a key you have lost."
  echo
  verify_backup "$out" "$fp" || {
    rm -f "$out" "$pubout"
    die "backup failed verification and was deleted — nothing was saved"
  }

  chmod 600 "$out"
  echo
  echo "Backup verified: $out"
  echo "Public record:   $pubout"
  echo
  echo "An encrypted copy in one place is not redundancy. Put a second copy"
  echo "somewhere that fails independently of this NAS."

  # Open the folder so the two files can be dragged straight into a NAS share,
  # web UI, or sync app. The encrypted blob is safe to move by any of those —
  # that is the point of encrypting before it leaves this machine.
  if [[ "$(uname)" == "Darwin" ]] && command -v open >/dev/null; then
    echo
    echo "Opening $dest in Finder. Drag both files to your NAS."
    echo "Do NOT store the passphrase alongside them — that would undo the"
    echo "encryption entirely."
    open "$dest" 2>/dev/null || true
  fi
}

# Decrypt to a temp file and confirm the recovered key yields the same public
# key. An unverified backup is a guess, not a backup.
verify_backup() {
  local enc="$1" expect="${2:-}"
  local tmp; tmp="$(mktemp)"
  # shellcheck disable=SC2064
  trap "rm -f '$tmp'" RETURN

  umask 077
  openssl enc -d -aes-256-cbc -pbkdf2 -iter "$ITER" -in "$enc" -out "$tmp" \
    || { echo "decryption failed (wrong passphrase, or a damaged file)" >&2; return 1; }

  local got; got="$(pubkey_hex "$tmp")" \
    || { echo "decrypted file is not a valid private key" >&2; return 1; }

  if [[ -n "$expect" && "$got" != "$expect" ]]; then
    echo "restored key does not match: expected $expect, got $got" >&2
    return 1
  fi

  echo "  decrypts cleanly, fingerprint: $got"
  return 0
}

cmd_verify() {
  local enc="${1:-}"
  [[ -n "$enc" ]] || die "usage: $0 verify <backup.pem.enc>"
  [[ -r "$enc" ]] || die "cannot read backup: $enc"
  echo "Enter the backup passphrase."
  verify_backup "$enc" "" || die "verification failed"
  echo "Backup is restorable."
}

cmd_restore() {
  local enc="${1:-}" out="${2:-}"
  [[ -n "$enc" && -n "$out" ]] || die "usage: $0 restore <backup.pem.enc> <output.pem>"
  [[ -r "$enc" ]] || die "cannot read backup: $enc"
  [[ -e "$out" ]] && die "refusing to overwrite existing file: $out"

  umask 077
  openssl enc -d -aes-256-cbc -pbkdf2 -iter "$ITER" -in "$enc" -out "$out" \
    || die "decryption failed"
  chmod 600 "$out"
  echo "restored to $out"
  echo "fingerprint: $(pubkey_hex "$out")"
  echo "Confirm that matches BUILD_TIME_PUBLIC_KEY in firmware/src/ota/key.rs."
}

case "${1:-}" in
  backup)  shift; cmd_backup  "$@" ;;
  verify)  shift; cmd_verify  "$@" ;;
  restore) shift; cmd_restore "$@" ;;
  *)
    echo "usage: $0 {backup|verify|restore} ..." >&2
    echo "  backup  <key.pem> <destination-dir>" >&2
    echo "  verify  <backup.pem.enc>" >&2
    echo "  restore <backup.pem.enc> <output.pem>" >&2
    exit 2
    ;;
esac
