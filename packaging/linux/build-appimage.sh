#!/bin/bash
# Build Linux AppImage
#
# Prerequisites:
# - Rust toolchain with x86_64-unknown-linux-gnu target
# - linuxdeploy and linuxdeploy-plugin-gtk (downloaded automatically if missing)
#
# This script creates:
#   connected-desktop-x86_64.AppImage
#
# Usage:
#   ./build-appimage.sh [--release]

set -euo pipefail

BUILD_TYPE="debug"
CARGO_PROFILE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_TYPE="release"
            CARGO_PROFILE="--release"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=$(grep -E '^\s*version\s*=\s*"[^"]+"' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
if [[ -z "$VERSION" ]]; then
    echo "Failed to extract version from Cargo.toml" >&2
    exit 1
fi

echo "Building Connected AppImage $BUILD_TYPE (version: $VERSION)"

APPDIR="$PROJECT_ROOT/target/appdir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/512x512/apps"

echo "Building release binary..."
cargo build $CARGO_PROFILE --verbose -p connected-desktop

cp "$PROJECT_ROOT/target/$BUILD_TYPE/connected-desktop" "$APPDIR/usr/bin/connected-desktop"
chmod +x "$APPDIR/usr/bin/connected-desktop"

cp "$PROJECT_ROOT/packaging/connected-desktop.desktop" "$APPDIR/usr/share/applications/connected-desktop.desktop"

cp "$PROJECT_ROOT/packaging/flatpak/com.paterkleomenis.Connected.png" \
   "$APPDIR/usr/share/icons/hicolor/512x512/apps/connected-desktop.png"

# The .desktop file Exec line must match the binary name in usr/bin/
sed -i 's|^Exec=.*|Exec=connected-desktop|' "$APPDIR/usr/share/applications/connected-desktop.desktop"
sed -i 's|^Icon=.*|Icon=connected-desktop|' "$APPDIR/usr/share/applications/connected-desktop.desktop"

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
LINUXDEPLOY_GTK_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"

TOOLS_DIR="$PROJECT_ROOT/target/appimage-tools"
mkdir -p "$TOOLS_DIR"

# linuxdeploy is distributed as an AppImage. On CI (no FUSE) we must extract it first.
LINUXDEPLOY_APPIMAGE="$TOOLS_DIR/linuxdeploy-x86_64.AppImage"
LINUXDEPLOY_EXTRACTED="$TOOLS_DIR/linuxdeploy"
if [[ ! -d "$LINUXDEPLOY_EXTRACTED" ]]; then
    if [[ ! -f "$LINUXDEPLOY_APPIMAGE" ]]; then
        echo "Downloading linuxdeploy..."
        curl -fSL "$LINUXDEPLOY_URL" -o "$LINUXDEPLOY_APPIMAGE"
        chmod +x "$LINUXDEPLOY_APPIMAGE"
    fi
    echo "Extracting linuxdeploy..."
    cd "$TOOLS_DIR"
    "$LINUXDEPLOY_APPIMAGE" --appimage-extract > /dev/null 2>&1
    # --appimage-extract creates squashfs-root/; rename it
    mv squashfs-root linuxdeploy
    cd "$PROJECT_ROOT"
fi
LINUXDEPLOY="$LINUXDEPLOY_EXTRACTED/usr/bin/linuxdeploy"

GTK_PLUGIN="$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"
if [[ ! -f "$GTK_PLUGIN" ]]; then
    echo "Downloading linuxdeploy-plugin-gtk..."
    curl -fSL "$LINUXDEPLOY_GTK_URL" -o "$GTK_PLUGIN"
    chmod +x "$GTK_PLUGIN"
fi

OUTPUT="$PROJECT_ROOT/target/connected-desktop-x86_64.AppImage"

echo "Creating AppImage..."
cd "$PROJECT_ROOT"
SKIP_DESKTOP_FILE_INSTALL=1 \
    "$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --desktop-file "$APPDIR/usr/share/applications/connected-desktop.desktop" \
    --icon-file "$APPDIR/usr/share/icons/hicolor/512x512/apps/connected-desktop.png" \
    --plugin gtk \
    --output appimage

# Rename output to consistent name
BUILT=$(find "$PROJECT_ROOT" -maxdepth 1 -name 'Connected-*-x86_64.AppImage' -print -quit 2>/dev/null)
if [[ -n "$BUILT" ]]; then
    mv "$BUILT" "$OUTPUT"
fi

echo ""
echo "AppImage created: $OUTPUT"
echo "Size: $(du -h "$OUTPUT" | cut -f1)"
