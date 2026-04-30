#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IOS_DIR="$ROOT_DIR/ios"
ARCHIVE_DIR="$ROOT_DIR/target/ios/archive"
EXPORT_DIR="$ROOT_DIR/target/ios/export"
IPA_DIR="$ROOT_DIR/target/ios/ipa"
CONFIGURATION="${IOS_CONFIGURATION:-Release}"
IPA_NAME="${IOS_IPA_NAME:-connected-ios.ipa}"
ARCHIVE_PATH="$ARCHIVE_DIR/$CONFIGURATION/Connected.xcarchive"
EXPORT_OPTIONS_PLIST="$EXPORT_DIR/$CONFIGURATION/ExportOptions.plist"
IPA_OUTPUT_DIR="$IPA_DIR/$CONFIGURATION"
BUILD_DIR="$ROOT_DIR/target/ios/build/$CONFIGURATION"
APP_PATH="$BUILD_DIR/Build/Products/$CONFIGURATION-iphoneos/Connected.app"
PAYLOAD_DIR="$IPA_OUTPUT_DIR/Payload"
BUNDLE_ID="${IOS_BUNDLE_IDENTIFIER:-com.connected.app.sync}"
EXPORT_METHOD="${IOS_EXPORT_METHOD:-app-store-connect}"
CODE_SIGN_IDENTITY="${IOS_CODE_SIGN_IDENTITY:-Apple Distribution}"
SIGN_IPA=false

if [[ -n "${APPLE_TEAM_ID:-}" && -n "${IOS_PROVISIONING_PROFILE_NAME:-}" ]]; then
    SIGN_IPA=true
fi

rm -rf "$ARCHIVE_PATH" "$EXPORT_DIR/$CONFIGURATION" "$IPA_OUTPUT_DIR" "$BUILD_DIR"
mkdir -p "$ARCHIVE_DIR/$CONFIGURATION" "$EXPORT_DIR/$CONFIGURATION" "$IPA_OUTPUT_DIR"

"$ROOT_DIR/scripts/ios/sync-version.sh"
xcodegen generate --spec "$IOS_DIR/project.yml"

if [[ "$SIGN_IPA" != "true" ]]; then
    echo "Signing inputs not found; creating unsigned $CONFIGURATION IPA."
    xcodebuild \
        -project "$IOS_DIR/Connected.xcodeproj" \
        -scheme Connected \
        -configuration "$CONFIGURATION" \
        -destination "generic/platform=iOS" \
        -derivedDataPath "$BUILD_DIR" \
        PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" \
        CODE_SIGNING_ALLOWED=NO \
        CODE_SIGN_IDENTITY="" \
        build

    if [[ ! -d "$APP_PATH" ]]; then
        echo "No app bundle produced at $APP_PATH" >&2
        exit 1
    fi

    mkdir -p "$PAYLOAD_DIR"
    cp -R "$APP_PATH" "$PAYLOAD_DIR/"
    rm -f "$ROOT_DIR/$IPA_NAME"
    (cd "$IPA_OUTPUT_DIR" && /usr/bin/ditto -c -k --sequesterRsrc --keepParent Payload "$ROOT_DIR/$IPA_NAME")
    echo "Created unsigned $ROOT_DIR/$IPA_NAME"
    exit 0
fi

echo "Signing inputs found; creating signed $CONFIGURATION IPA."

cat > "$EXPORT_OPTIONS_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>$EXPORT_METHOD</string>
    <key>teamID</key>
    <string>$APPLE_TEAM_ID</string>
    <key>signingStyle</key>
    <string>manual</string>
    <key>signingCertificate</key>
    <string>$CODE_SIGN_IDENTITY</string>
    <key>provisioningProfiles</key>
    <dict>
        <key>$BUNDLE_ID</key>
        <string>$IOS_PROVISIONING_PROFILE_NAME</string>
    </dict>
    <key>destination</key>
    <string>export</string>
    <key>stripSwiftSymbols</key>
    <true/>
    <key>compileBitcode</key>
    <false/>
</dict>
</plist>
EOF

xcodebuild archive \
    -project "$IOS_DIR/Connected.xcodeproj" \
    -scheme Connected \
    -configuration "$CONFIGURATION" \
    -destination "generic/platform=iOS" \
    -archivePath "$ARCHIVE_PATH" \
    CODE_SIGN_STYLE=Manual \
    DEVELOPMENT_TEAM="$APPLE_TEAM_ID" \
    PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" \
    CODE_SIGN_IDENTITY="$CODE_SIGN_IDENTITY" \
    PROVISIONING_PROFILE_SPECIFIER="$IOS_PROVISIONING_PROFILE_NAME"

xcodebuild -exportArchive \
    -archivePath "$ARCHIVE_PATH" \
    -exportPath "$IPA_OUTPUT_DIR" \
    -exportOptionsPlist "$EXPORT_OPTIONS_PLIST"

IPA_PATHS=("$IPA_OUTPUT_DIR"/*.ipa)
if [[ ! -f "${IPA_PATHS[0]:-}" ]]; then
    echo "No IPA produced in $IPA_OUTPUT_DIR" >&2
    exit 1
fi

rm -f "$ROOT_DIR/$IPA_NAME"
cp "${IPA_PATHS[0]}" "$ROOT_DIR/$IPA_NAME"
echo "Created signed $ROOT_DIR/$IPA_NAME"
