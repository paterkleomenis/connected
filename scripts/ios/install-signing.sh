#!/usr/bin/env bash

set -euo pipefail

if [[ -z "${IOS_CERTIFICATE:-}" ]]; then
    echo "IOS_CERTIFICATE is required. It must contain the base64-encoded .p12 certificate." >&2
    exit 1
fi

if [[ -z "${IOS_CERTIFICATE_PASSWORD:-}" ]]; then
    echo "IOS_CERTIFICATE_PASSWORD is required." >&2
    exit 1
fi

if [[ -z "${IOS_PROVISIONING_PROFILE:-}" ]]; then
    echo "IOS_PROVISIONING_PROFILE is required. It must contain the base64-encoded provisioning profile." >&2
    exit 1
fi

WORK_DIR="${RUNNER_TEMP:-/tmp}/connected-ios-signing"
KEYCHAIN_PATH="$WORK_DIR/connected-signing.keychain-db"
KEYCHAIN_PASSWORD="${IOS_KEYCHAIN_PASSWORD:-$(uuidgen)}"
CERTIFICATE_PATH="$WORK_DIR/distribution.p12"
PROFILE_PATH="$WORK_DIR/profile.mobileprovision"
PROFILE_PLIST="$WORK_DIR/profile.plist"

mkdir -p "$WORK_DIR" "$HOME/Library/MobileDevice/Provisioning Profiles"

decode_base64() {
    local value="$1"
    local output="$2"
    if ! printf '%s' "$value" | base64 --decode > "$output" 2>/dev/null; then
        printf '%s' "$value" | base64 -D > "$output"
    fi
}

decode_base64 "$IOS_CERTIFICATE" "$CERTIFICATE_PATH"
decode_base64 "$IOS_PROVISIONING_PROFILE" "$PROFILE_PATH"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security import "$CERTIFICATE_PATH" -P "$IOS_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$KEYCHAIN_PATH"
security list-keychain -d user -s "$KEYCHAIN_PATH" $(security list-keychain -d user | sed 's/[\" ]//g')
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"

security cms -D -i "$PROFILE_PATH" > "$PROFILE_PLIST"
PROFILE_UUID="$(/usr/libexec/PlistBuddy -c 'Print UUID' "$PROFILE_PLIST")"
PROFILE_NAME="$(/usr/libexec/PlistBuddy -c 'Print Name' "$PROFILE_PLIST")"
cp "$PROFILE_PATH" "$HOME/Library/MobileDevice/Provisioning Profiles/$PROFILE_UUID.mobileprovision"

if [[ -n "${GITHUB_ENV:-}" ]]; then
    {
        echo "IOS_KEYCHAIN_PATH=$KEYCHAIN_PATH"
        echo "IOS_PROVISIONING_PROFILE_NAME=$PROFILE_NAME"
    } >> "$GITHUB_ENV"
fi

echo "Installed iOS signing certificate and provisioning profile: $PROFILE_NAME ($PROFILE_UUID)"
