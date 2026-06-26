#!/bin/bash
# Build Linux RPM package for Fedora
#
# Prerequisites:
# - Rust toolchain with the target architecture available
# - rpm-build, rpmdevtools (Fedora)
# - mock (optional, for clean builds)
#
# This script creates:
#   target/connected-desktop-${ARCH}.rpm
#
# Usage:
#   ./build-rpm.sh [--release] [--arch x86_64|aarch64] [--mock]
#
# Environment:
#   DEBUG=1                         print every command as it runs (bash -x style)

set -euo pipefail

if [[ "${DEBUG:-}" != "" ]]; then
    set -x
fi

BUILD_TYPE="debug"
CARGO_PROFILE=""
ARCH="x86_64"
USE_MOCK=0
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
        --mock)
            USE_MOCK=1
            shift
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

echo "Building Connected RPM $BUILD_TYPE (version: $VERSION, arch: $ARCH)"

echo "Building release binary..."
cargo build $CARGO_PROFILE --target "$RUST_TARGET" --verbose -p connected-desktop

BINARY="$PROJECT_ROOT/target/$RUST_TARGET/$BUILD_TYPE/connected-desktop"

if [[ ! -f "$BINARY" ]]; then
    echo "Error: binary not found at $BINARY" >&2
    exit 1
fi

# Create RPM build directory structure
RPM_DIR="$PROJECT_ROOT/target/rpm"
rm -rf "$RPM_DIR"
mkdir -p "$RPM_DIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

# Copy spec file
cp "$PROJECT_ROOT/packaging/rpm/connected-desktop.spec" "$RPM_DIR/SPECS/"

# Update version in spec file
sed -i "s/^Version:.*/Version:        $VERSION/" "$RPM_DIR/SPECS/connected-desktop.spec"

# Prepare source files
cp "$BINARY" "$RPM_DIR/SOURCES/connected-desktop-linux-$ARCH"
cp "$PROJECT_ROOT/packaging/connected-desktop.desktop" "$RPM_DIR/SOURCES/"
cp "$PROJECT_ROOT/packaging/flatpak/com.paterkleomenis.Connected.png" "$RPM_DIR/SOURCES/com.paterkleomenis.Connected.png"
cp "$PROJECT_ROOT/LICENSE-MIT" "$RPM_DIR/SOURCES/"
cp "$PROJECT_ROOT/LICENSE-APACHE" "$RPM_DIR/SOURCES/"

# Create source tarball
cd "$PROJECT_ROOT"
tar -czf "$RPM_DIR/SOURCES/connected-desktop-$VERSION.tar.gz" \
    --transform "s,^,connected-desktop-$VERSION/," \
    packaging/rpm/connected-desktop.spec \
    packaging/connected-desktop.desktop \
    packaging/flatpak/com.paterkleomenis.Connected.png \
    LICENSE-MIT \
    LICENSE-APACHE

echo "Building RPM package..."
cd "$RPM_DIR"

if [[ "$USE_MOCK" -eq 1 ]]; then
    echo "Using mock for clean build..."
    mock -r fedora-43-x86_64 --rebuild "$RPM_DIR/SRPMS/"*.src.rpm 2>&1 || true
    echo "Mock build completed. Check mock logs for details."
else
    # Build RPM using rpmbuild
    rpmbuild -bb \
        --define "_topdir $RPM_DIR" \
        --define "_arch $ARCH" \
        --define "version $VERSION" \
        "$RPM_DIR/SPECS/connected-desktop.spec"
fi

# Find the built RPM
RPM_FILE=$(find "$RPM_DIR/RPMS" -name "*.rpm" -type f | head -1)

if [[ -z "$RPM_FILE" ]]; then
    echo "Error: RPM package not found" >&2
    exit 1
fi

# Copy RPM to target directory with standard name
OUTPUT="$PROJECT_ROOT/target/connected-desktop-${ARCH}.rpm"
cp "$RPM_FILE" "$OUTPUT"

echo ""
echo "RPM package created: $OUTPUT"
echo "Size: $(du -h "$OUTPUT" | cut -f1)"
