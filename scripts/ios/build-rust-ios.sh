#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VENDOR_DIR="$ROOT_DIR/ios/Vendor"
WORK_DIR="$ROOT_DIR/target/ios"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"

mkdir -p "$VENDOR_DIR"
mkdir -p "$WORK_DIR"

echo "Building Rust FFI for iOS device + simulator targets with deployment target $IPHONEOS_DEPLOYMENT_TARGET..."
cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml" --target aarch64-apple-ios
cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml" --target aarch64-apple-ios-sim
cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml" --target x86_64-apple-ios

DEVICE_LIB="$ROOT_DIR/target/aarch64-apple-ios/debug/libconnected_ffi.a"
SIM_ARM_LIB="$ROOT_DIR/target/aarch64-apple-ios-sim/debug/libconnected_ffi.a"
SIM_X64_LIB="$ROOT_DIR/target/x86_64-apple-ios/debug/libconnected_ffi.a"

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
