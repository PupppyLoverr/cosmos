#!/usr/bin/env bash
# Create the stable local signing identity used by macOS development bundles.

set -euo pipefail

IDENTITY_NAME="Cosmos Local Signing"
KEYCHAIN="$HOME/Library/Keychains/cosmos-signing.keychain-db"
LOGIN_KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if [[ -n "${COSMOS_SIGNING_KEYCHAIN_PASSWORD:-}" ]]; then
  KEYCHAIN_PASSWORD="$COSMOS_SIGNING_KEYCHAIN_PASSWORD"
else
  read -r -s -p "Password for $KEYCHAIN: " KEYCHAIN_PASSWORD
  echo
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cosmos-signing.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

if [[ ! -e "$KEYCHAIN" ]]; then
  security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
fi
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings "$KEYCHAIN"
security list-keychains -d user -s "$KEYCHAIN" "$LOGIN_KEYCHAIN"

if ! security find-certificate -a -c "$IDENTITY_NAME" "$KEYCHAIN" >/dev/null 2>&1; then
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
    -T /usr/bin/codesign \
    -T /usr/bin/security
else
  security find-certificate -a -c "$IDENTITY_NAME" -p "$KEYCHAIN" >"$TMP_DIR/cert.pem"
fi

security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s -k "$KEYCHAIN_PASSWORD" \
  "$KEYCHAIN"
sudo security add-trusted-cert \
  -d -r trustRoot -p codeSign \
  -k /Library/Keychains/System.keychain "$TMP_DIR/cert.pem"

if ! security find-identity -v -p codesigning 2>/dev/null |
  grep -Fq "\"$IDENTITY_NAME\""; then
  echo "error: signing identity was not valid after setup: $IDENTITY_NAME" >&2
  exit 1
fi

echo "signing identity ready: $IDENTITY_NAME"
