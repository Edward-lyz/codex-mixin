#!/usr/bin/env bash
# Writes <archive>.sig: a base64 Ed25519 signature over the archive's SHA-256
# digest, the format the desktop updater verifies against its embedded key.
# The PKCS#8 PEM private key comes from UPDATE_SIGNING_KEY and never touches argv.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: sign_update.sh <archive>..." >&2
  exit 2
fi
# Ed25519 signing needs OpenSSL 3 (`pkeyutl -rawin`); macOS's LibreSSL lacks it.
OPENSSL="${OPENSSL:-openssl}"
if ! "$OPENSSL" version | grep -q '^OpenSSL 3'; then
  echo "sign_update.sh needs OpenSSL 3; set OPENSSL to its path" >&2
  exit 1
fi
if [[ -z "${UPDATE_SIGNING_KEY:-}" ]]; then
  echo "UPDATE_SIGNING_KEY is not set" >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
umask 077
printf '%s\n' "$UPDATE_SIGNING_KEY" > "$WORK_DIR/key.pem"

for archive in "$@"; do
  "$OPENSSL" dgst -sha256 -binary "$archive" > "$WORK_DIR/digest"
  "$OPENSSL" pkeyutl -sign -rawin -inkey "$WORK_DIR/key.pem" -in "$WORK_DIR/digest" \
    | "$OPENSSL" base64 -A > "$archive.sig"
  # Check the signature with the public half before publishing it.
  "$OPENSSL" pkey -in "$WORK_DIR/key.pem" -pubout > "$WORK_DIR/public.pem"
  "$OPENSSL" base64 -d -A -in "$archive.sig" > "$WORK_DIR/signature"
  "$OPENSSL" pkeyutl -verify -rawin -pubin -inkey "$WORK_DIR/public.pem" \
    -in "$WORK_DIR/digest" -sigfile "$WORK_DIR/signature" >/dev/null
  echo "signed $archive"
done
