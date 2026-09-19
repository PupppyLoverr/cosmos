#!/usr/bin/env bash
# Create the stable local signing identity used by macOS development bundles.

set -euo pipefail

IDENTITY_NAME="Cosmos Local Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-certificate -a -c "$IDENTITY_NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "signing identity already exists: $IDENTITY_NAME"
  exit 0
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cosmos-signing.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

openssl genrsa -out "$TMP_DIR/key.pem" 2048 >/dev/null 2>&1
P12_PASSWORD="$(openssl rand -hex 32)"
openssl req -new -x509 -sha256 -days 3650 \
  -key "$TMP_DIR/key.pem" \
  -out "$TMP_DIR/cert.pem" \
  -subj "/CN=$IDENTITY_NAME" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "subjectKeyIdentifier=hash" \
  -addext "authorityKeyIdentifier=keyid,issuer"
openssl pkcs12 -legacy -export \
  -out "$TMP_DIR/identity.p12" \
  -inkey "$TMP_DIR/key.pem" \
  -in "$TMP_DIR/cert.pem" \
  -name "$IDENTITY_NAME" \
  -passout "pass:$P12_PASSWORD"

security import "$TMP_DIR/identity.p12" \
  -k "$KEYCHAIN" \
  -P "$P12_PASSWORD" \
  -T /usr/bin/codesign
sudo security add-trusted-cert \
  -d -r trustRoot -p codeSign \
  -k "$KEYCHAIN" "$TMP_DIR/cert.pem"

if ! security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null |
  grep -Fq "\"$IDENTITY_NAME\""; then
  echo "error: signing identity was not found after import: $IDENTITY_NAME" >&2
  exit 1
fi

echo "created signing identity: $IDENTITY_NAME"
