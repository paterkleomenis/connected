#!/bin/bash
# Build macOS .app bundle and create DMG installer
#
# The version is automatically fetched from Cargo.toml.
#
# Prerequisites:
# - Rust toolchain with aarch64-apple-darwin and x86_64-apple-darwin targets
#
# This script creates:
# 1. Connected.app - macOS application bundle
# 2. Connected-<version>.dmg - Disk image installer

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PACKAGING_DIR="$SCRIPT_DIR"

# Version from Cargo.toml
VERSION=$(grep -E '^\s*version\s*=\s*"[^"]+"' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
if [[ -z "$VERSION" ]]; then
    echo "❌ Failed to extract version from Cargo.toml" >&2
    exit 1
fi

echo "📦 Building Connected macOS release (version: $VERSION)"

# Directories
BUILD_DIR="$PROJECT_ROOT/target/macos-build"
APP_DIR="$BUILD_DIR/Connected.app/Contents"
UNIVERSAL_DIR="$PROJECT_ROOT/target/macos-universal"

# Clean and create directories
rm -rf "$BUILD_DIR"
mkdir -p "$APP_DIR/MacOS"
mkdir -p "$APP_DIR/Resources"
mkdir -p "$UNIVERSAL_DIR"

# Step 1: Build universal binary
echo "🔨 Building universal binary..."
cargo build --release --target aarch64-apple-darwin --verbose
cargo build --release --target x86_64-apple-darwin --verbose

# Create universal binary using lipo
lipo -create \
    "$PROJECT_ROOT/target/aarch64-apple-darwin/release/connected-desktop" \
    "$PROJECT_ROOT/target/x86_64-apple-darwin/release/connected-desktop" \
    -output "$UNIVERSAL_DIR/connected-desktop"

chmod +x "$UNIVERSAL_DIR/connected-desktop"
echo "✅ Universal binary created: $UNIVERSAL_DIR/connected-desktop"

# Step 2: Create .app bundle structure
echo "📱 Creating .app bundle..."

# Copy Info.plist (update version)
sed "s/0.0.0/$VERSION/g" "$PACKAGING_DIR/Info.plist" > "$APP_DIR/Info.plist"

# Copy binary
cp "$UNIVERSAL_DIR/connected-desktop" "$APP_DIR/MacOS/connected-desktop"

# Create PkgInfo (APPL???? = Application bundle)
echo -n "APPL????" > "$APP_DIR/PkgInfo"

# Step 3: Copy app icon
cp "$PACKAGING_DIR/Connected.icns" "$APP_DIR/Resources/Connected.icns"
echo "✅ App icon copied"

# Step 4: Code sign the app
# If APPLE_DEVELOPER_ID_CERTIFICATE is provided, use it for distribution signing
# Otherwise, use ad-hoc signing for local testing
if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE:-}" ]] && [[ -n "${APPLE_DEVELOPER_ID_PASSWORD:-}" ]] && [[ -n "${APPLE_DEVELOPER_ID_TEAM_ID:-}" ]]; then
    echo "🔐 Code signing with Developer ID..."

    # Create temporary keychain
    KEYCHAIN_PATH="$BUILD_DIR/signing.keychain-db"
    rm -f "$KEYCHAIN_PATH"
    security create-keychain -p "" "$KEYCHAIN_PATH"
    security default-keychain -s "$KEYCHAIN_PATH"
    security unlock-keychain -p "" "$KEYCHAIN_PATH"
    security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"

    # Import certificate
    CERT_PATH="$BUILD_DIR/developer-id.p12"
    echo "$APPLE_DEVELOPER_ID_CERTIFICATE" | base64 -d > "$CERT_PATH"
    security import "$CERT_PATH" -k "$KEYCHAIN_PATH" -P "$APPLE_DEVELOPER_ID_PASSWORD" -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "" "$KEYCHAIN_PATH"

    # Find the Developer ID certificate
    CERT_NAME=$(security find-identity -v -p codesigning "$KEYCHAIN_PATH" | grep "Developer ID Application:" | head -1 | sed 's/.*"/"/' | sed 's/".*/"/')

    if [[ -n "$CERT_NAME" ]]; then
        # Sign the app
        codesign --force --deep --sign "$CERT_NAME" --timestamp --options runtime "$BUILD_DIR/Connected.app"
        echo "✅ Code signed with Developer ID: $CERT_NAME"
    else
        echo "⚠️  Developer ID certificate not found, falling back to ad-hoc signing"
        codesign --force --deep --sign - "$BUILD_DIR/Connected.app"
    fi

    # Clean up
    rm -f "$CERT_PATH"
    security delete-keychain "$KEYCHAIN_PATH"
else
    echo "🔐 Code signing (ad-hoc)..."
    codesign --force --deep --sign - "$BUILD_DIR/Connected.app" 2>/dev/null || {
        echo "⚠️  Code signing skipped (not on macOS or codesign not available)"
    }
fi

# Step 5: Create DMG
echo "💿 Creating DMG..."
DMG_PATH="$BUILD_DIR/Connected-$VERSION.dmg"

# Create a temporary DMG with proper layout
# Using hdiutil for native macOS DMG creation
if command -v hdiutil &> /dev/null; then
    # Create temporary folder with symlink to /Applications
    DMG_CONTENT_DIR="$BUILD_DIR/dmg-content"
    rm -rf "$DMG_CONTENT_DIR"
    mkdir -p "$DMG_CONTENT_DIR"

    # Copy app to DMG content
    cp -R "$BUILD_DIR/Connected.app" "$DMG_CONTENT_DIR/"

    # Create Applications symlink
    ln -s /Applications "$DMG_CONTENT_DIR/Applications"

    # Calculate size (app size + overhead)
    APP_SIZE=$(du -sm "$DMG_CONTENT_DIR" | cut -f1)
    DMG_SIZE=$((APP_SIZE + 50))

    # Create DMG with custom layout
    hdiutil create -volname "Connected" \
        -srcfolder "$DMG_CONTENT_DIR" \
        -ov -format UDZO \
        -size "${DMG_SIZE}m" \
        "$DMG_PATH"

    # Clean up temp content
    rm -rf "$DMG_CONTENT_DIR"

    echo "✅ DMG created: $DMG_PATH"
else
    # Fallback: create a simple tar.gz if hdiutil not available
    echo "⚠️  hdiutil not available (not on macOS). Creating tar.gz instead..."
    TAR_PATH="$BUILD_DIR/Connected-$VERSION-macos.tar.gz"
    tar -czf "$TAR_PATH" -C "$BUILD_DIR" Connected.app
    echo "✅ Archive created: $TAR_PATH"
    DMG_PATH="$TAR_PATH"
fi

# Step 6: Notarization (optional, requires Apple Developer account)
# This would be done in CI with environment variables:
# - APPLE_ID
# - APPLE_PASSWORD (app-specific password)
# - APPLE_TEAM_ID
if [[ -n "${APPLE_ID:-}" ]] && [[ -n "${APPLE_PASSWORD:-}" ]] && [[ -n "${APPLE_TEAM_ID:-}" ]]; then
    echo "🔒 Notarizing DMG..."
    xcrun notarytool submit "$DMG_PATH" \
        --apple-id "$APPLE_ID" \
        --password "$APPLE_PASSWORD" \
        --team-id "$APPLE_TEAM_ID" \
        --wait

    # Staple the notarization ticket
    xcrun stapler staple "$DMG_PATH"
    echo "✅ DMG notarized and stapled"
else
    echo "⚠️  Notarization skipped (set APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID for production)"
fi

echo ""
echo "✅ macOS build complete!"
echo "   App: $BUILD_DIR/Connected.app"
echo "   DMG: $DMG_PATH"
