#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VENDOR_DIR="$ROOT_DIR/ios/Vendor"
WORK_DIR="$ROOT_DIR/target/ios"
RUST_PROFILE="${IOS_RUST_PROFILE:-debug}"

if [[ "$RUST_PROFILE" != "debug" && "$RUST_PROFILE" != "release" ]]; then
    echo "IOS_RUST_PROFILE must be debug or release." >&2
    exit 1
fi

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"

cargo_build_target() {
    local target="$1"
    if [[ "$RUST_PROFILE" == "release" ]]; then
        cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml" --target "$target" --release
    else
        cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml" --target "$target"
    fi
}

mkdir -p "$VENDOR_DIR"
mkdir -p "$WORK_DIR"

echo "Building Rust FFI for iOS device + simulator targets with deployment target $IPHONEOS_DEPLOYMENT_TARGET ($RUST_PROFILE)..."
cargo_build_target aarch64-apple-ios
cargo_build_target aarch64-apple-ios-sim
cargo_build_target x86_64-apple-ios

DEVICE_LIB="$ROOT_DIR/target/aarch64-apple-ios/$RUST_PROFILE/libconnected_ffi.a"
SIM_ARM_LIB="$ROOT_DIR/target/aarch64-apple-ios-sim/$RUST_PROFILE/libconnected_ffi.a"
SIM_X64_LIB="$ROOT_DIR/target/x86_64-apple-ios/$RUST_PROFILE/libconnected_ffi.a"

if [[ ! -f "$DEVICE_LIB" || ! -f "$SIM_ARM_LIB" || ! -f "$SIM_X64_LIB" ]]; then
    echo "Expected one or more Rust static libraries are missing."
    exit 1
fi

SIM_UNIVERSAL_LIB="$WORK_DIR/libconnected_ffi_sim.a"
lipo -create "$SIM_ARM_LIB" "$SIM_X64_LIB" -output "$SIM_UNIVERSAL_LIB"

XCFRAMEWORK_PATH="$VENDOR_DIR/connected_ffi.xcframework"
rm -rf "$XCFRAMEWORK_PATH"

xcodebuild -create-xcframework \
    -library "$DEVICE_LIB" \
    -library "$SIM_UNIVERSAL_LIB" \
    -output "$XCFRAMEWORK_PATH"

echo "Created $XCFRAMEWORK_PATH"
