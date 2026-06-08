#!/bin/bash
# Build Linux AppImage
#
# Prerequisites:
# - Rust toolchain with the target architecture available
# - linuxdeploy, linuxdeploy-plugin-gtk, appimagetool
#   (all downloaded automatically if missing)
#
# This script creates:
#   target/connected-desktop-${ARCH}.AppImage
#   target/connected-desktop-${ARCH}.AppImage.zsync
#
# Usage:
#   ./build-appimage.sh [--release] [--arch x86_64|aarch64]
#
# Environment:
#   DEBUG=1                         print every command as it runs (bash -x style)
#   APPIMAGE_UPDATE_INFORMATION=... override embedded AppImage update information

set -euo pipefail

if [[ "${DEBUG:-}" != "" ]]; then
    set -x
fi

BUILD_TYPE="debug"
CARGO_PROFILE=""
ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_TYPE="release"
            CARGO_PROFILE="--release"
            shift
            ;;
        --arch)
            ARCH="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# Validate architecture
case "$ARCH" in
    x86_64|aarch64)
        ;;
    *)
        echo "Error: unsupported architecture '$ARCH'. Use x86_64 or aarch64." >&2
        exit 1
        ;;
esac

RUST_TARGET="${ARCH}-unknown-linux-gnu"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION=$(grep -E '^\s*version\s*=\s*"[^"]+"' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
if [[ -z "$VERSION" ]]; then
    echo "Failed to extract version from Cargo.toml" >&2
    exit 1
fi

echo "Building Connected AppImage $BUILD_TYPE (version: $VERSION, arch: $ARCH)"

APPDIR="$PROJECT_ROOT/target/appdir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/512x512/apps"

echo "Building release binary..."
cargo build $CARGO_PROFILE --target "$RUST_TARGET" --verbose -p connected-desktop

cp "$PROJECT_ROOT/target/$RUST_TARGET/$BUILD_TYPE/connected-desktop" "$APPDIR/usr/bin/connected-desktop"
chmod +x "$APPDIR/usr/bin/connected-desktop"

cp "$PROJECT_ROOT/packaging/connected-desktop.desktop" "$APPDIR/usr/share/applications/connected-desktop.desktop"

cp "$PROJECT_ROOT/packaging/flatpak/com.paterkleomenis.Connected.png" \
   "$APPDIR/usr/share/icons/hicolor/512x512/apps/connected-desktop.png"

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${ARCH}.AppImage"
LINUXDEPLOY_GTK_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"

TOOLS_DIR="$PROJECT_ROOT/target/appimage-tools"
mkdir -p "$TOOLS_DIR"

LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage"
if [[ ! -f "$LINUXDEPLOY" ]]; then
    echo "Downloading linuxdeploy..."
    curl -fSL "$LINUXDEPLOY_URL" -o "$LINUXDEPLOY"
fi
chmod +x "$LINUXDEPLOY"

# The plugin must be named `linuxdeploy-plugin-gtk*` and live next to the
# linuxdeploy AppImage so that linuxdeploy can discover it by name (--plugin gtk).
GTK_PLUGIN="$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"
if [[ ! -f "$GTK_PLUGIN" ]]; then
    echo "Downloading linuxdeploy-plugin-gtk..."
    curl -fSL "$LINUXDEPLOY_GTK_URL" -o "$GTK_PLUGIN"
fi
chmod +x "$GTK_PLUGIN"

APPIMAGETOOL="$TOOLS_DIR/appimagetool-${ARCH}.AppImage"
if [[ ! -f "$APPIMAGETOOL" ]]; then
    echo "Downloading appimagetool..."
    curl -fSL "$APPIMAGETOOL_URL" -o "$APPIMAGETOOL"
fi
chmod +x "$APPIMAGETOOL"

# Build the AppDir (bundles the binary, icons, .desktop, and all GTK/GDK
# dependencies via the gtk plugin). We do NOT pass --output appimage here:
# we'll package the AppDir into an AppImage ourselves with appimagetool so
# that we can capture a clean error if packaging fails.
APPIMAGETOOL_LOG="$PROJECT_ROOT/target/appimagetool.log"

echo "Bundling dependencies into AppDir..."
cd "$PROJECT_ROOT"

# Force the AppImage runtime to extract on the fly instead of mounting via FUSE.
# Required on CI runners where FUSE for unprivileged users is not available.
export APPIMAGE_EXTRACT_AND_RUN=1

# linuxdeploy bundles a binutils 2.35 `strip` (from 2020) which cannot parse
# the `.relr.dyn` section added in glibc 2.36, causing it to fail with exit 1
# on Ubuntu 22.04+ system libraries. Skip stripping; release builds are
# already stripped by the Rust profile.
export NO_STRIP=1

if ! "$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --desktop-file "$APPDIR/usr/share/applications/connected-desktop.desktop" \
    --icon-file "$APPDIR/usr/share/icons/hicolor/512x512/apps/connected-desktop.png" \
    --plugin gtk \
    2>&1 | tee "$APPIMAGETOOL_LOG"; then
    echo "Error: linuxdeploy failed to build the AppDir" >&2
    echo "Log: $APPIMAGETOOL_LOG" >&2
    exit 1
fi

# Bundle WebKit2GTK

WEBKIT_DIR="$(pkg-config --variable=libdir webkit2gtk-4.1)/webkit2gtk-4.1"

if [[ ! -d "$WEBKIT_DIR" ]]; then
    echo "Error: Could not find webkit2gtk-4.1. Install webkit2gtk-4.1 and re-run." >&2
    exit 1
fi

WEBKIT_TARGET="$APPDIR/${WEBKIT_DIR#/usr/}"
mkdir -p "$WEBKIT_TARGET/injected-bundle"
cp "$WEBKIT_DIR"/WebKit*Process "$WEBKIT_TARGET/"
cp "$WEBKIT_DIR"/injected-bundle/libwebkit2gtkinjectedbundle.so "$WEBKIT_TARGET/injected-bundle/"

# Patch libwebkit and libjavascriptcore: "/usr" → "././" (same byte count).
# This makes the hardcoded install paths relative to the AppImage mount point.
for f in "$APPDIR/usr/lib"/lib{webkit2gtk,javascriptcoregtk}-4.1*.so*; do
    [[ -f "$f" ]] && sed -i 's|/usr|././|g' "$f"
done

sed -i "/^exec .*AppRun\.wrapped/i export LD_LIBRARY_PATH=\"\$APPDIR/usr/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}\"\ncd \"\$this_dir\"" "$APPDIR/AppRun"

OUTPUT="$PROJECT_ROOT/target/connected-desktop-${ARCH}.AppImage"
ZSYNC_OUTPUT="$OUTPUT.zsync"
UPDATE_INFORMATION="${APPIMAGE_UPDATE_INFORMATION:-gh-releases-zsync|paterkleomenis|connected|latest|connected-desktop-${ARCH}.AppImage.zsync}"
rm -f "$OUTPUT" "$ZSYNC_OUTPUT"

echo "Packaging AppDir into AppImage..."
cd "$PROJECT_ROOT"

# appimagetool auto-detects the architecture; set it explicitly to be safe.
export ARCH=$ARCH

if ! command -v zsyncmake >/dev/null 2>&1; then
    echo "Error: zsyncmake is required to generate $ZSYNC_OUTPUT" >&2
    echo "Install the zsync package and re-run this script." >&2
    exit 1
fi

echo "Embedding AppImage update information: $UPDATE_INFORMATION"

if ! "$APPIMAGETOOL" \
    --no-appstream \
    --updateinformation "$UPDATE_INFORMATION" \
    "$APPDIR" \
    "$OUTPUT" \
    2>&1 | tee -a "$APPIMAGETOOL_LOG"; then
    echo "Error: appimagetool failed to package the AppImage" >&2
    echo "Log: $APPIMAGETOOL_LOG" >&2
    echo "AppDir contents:" >&2
    find "$APPDIR" -maxdepth 4 -print >&2
    exit 1
fi

if [[ ! -s "$OUTPUT" ]]; then
    echo "Error: AppImage at $OUTPUT is missing or empty" >&2
    exit 1
fi

if [[ ! -s "$ZSYNC_OUTPUT" ]]; then
    echo "Generating zsync metadata..."
    zsyncmake -o "$ZSYNC_OUTPUT" "$OUTPUT" 2>&1 | tee -a "$APPIMAGETOOL_LOG"
fi

if [[ ! -s "$ZSYNC_OUTPUT" ]]; then
    echo "Error: zsync metadata at $ZSYNC_OUTPUT is missing or empty" >&2
    exit 1
fi

echo ""
echo "AppImage created: $OUTPUT"
echo "Size: $(du -h "$OUTPUT" | cut -f1)"
echo "zsync metadata created: $ZSYNC_OUTPUT"
echo "zsync size: $(du -h "$ZSYNC_OUTPUT" | cut -f1)"
