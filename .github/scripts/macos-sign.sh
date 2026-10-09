#!/usr/bin/env bash
# Developer ID-sign (hardened runtime) and, when App Store Connect API credentials are
# present, notarise a standalone macOS binary. Uses the same secrets Tauri uses:
#   APPLE_CERTIFICATE (base64 .p12), APPLE_CERTIFICATE_PASSWORD, APPLE_SIGNING_IDENTITY,
#   APPLE_API_ISSUER, APPLE_API_KEY (key id), APPLE_API_KEY_PATH (path to the .p8).
set -euo pipefail
bin="${1:?binary path}"
kc="$RUNNER_TEMP/loupecam-signing.keychain-db"
pw=$(openssl rand -hex 16)
if [ ! -f "$kc" ]; then
  security create-keychain -p "$pw" "$kc"
  security set-keychain-settings -lut 21600 "$kc"
  security unlock-keychain -p "$pw" "$kc"
  printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$RUNNER_TEMP/cert.p12"
  security import "$RUNNER_TEMP/cert.p12" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$kc"
  rm -f "$RUNNER_TEMP/cert.p12"
  security set-key-partition-list -S apple-tool:,apple: -s -k "$pw" "$kc" >/dev/null
  # shellcheck disable=SC2046
  security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')
fi
codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$bin"
codesign --verify --strict --verbose=2 "$bin"
if [ -n "${APPLE_API_KEY:-}" ]; then
  # A bare executable cannot be stapled, but Gatekeeper checks notarisation online.
  zip="$RUNNER_TEMP/$(basename "$bin").zip"
  ditto -c -k --keepParent "$bin" "$zip"
  xcrun notarytool submit "$zip" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" --wait
fi
