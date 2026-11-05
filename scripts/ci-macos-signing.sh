#!/bin/bash
# Run only on an ephemeral macOS CI runner. Never export an existing user's keys.
set -euo pipefail
: "${RUNNER_TEMP:?}"
keychain="$RUNNER_TEMP/ros-studio-signing.keychain-db"
if [[ "${1:-}" == cleanup ]]; then
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -f "$RUNNER_TEMP/ros-studio-signing.p12" "$RUNNER_TEMP/ros-studio-notary.p8"
  exit
fi
: "${APPLE_DEVELOPER_ID_P12_BASE64:?}"
: "${APPLE_DEVELOPER_ID_P12_PASSWORD:?}"
: "${APPLE_API_PRIVATE_KEY:?}"
: "${APPLE_API_KEY_ID:?}"
: "${APPLE_API_ISSUER_ID:?}"
umask 077
printf '%s' "$APPLE_DEVELOPER_ID_P12_BASE64" | base64 --decode > "$RUNNER_TEMP/ros-studio-signing.p12"
printf '%s' "$APPLE_API_PRIVATE_KEY" > "$RUNNER_TEMP/ros-studio-notary.p8"
keychain_password=$(openssl rand -hex 32)
printf '::add-mask::%s\n' "$keychain_password"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$RUNNER_TEMP/ros-studio-signing.p12" -P "$APPLE_DEVELOPER_ID_P12_PASSWORD" -k "$keychain" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" "$HOME/Library/Keychains/login.keychain-db"
xcrun notarytool store-credentials ros-studio-ci --key "$RUNNER_TEMP/ros-studio-notary.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" --keychain "$keychain" >/dev/null
rm -f "$RUNNER_TEMP/ros-studio-signing.p12" "$RUNNER_TEMP/ros-studio-notary.p8"
