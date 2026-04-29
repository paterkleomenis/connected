#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="$ROOT_DIR/ios/Connected/Generated/Uniffi"

mkdir -p "$OUT_DIR"

case "$(uname -s)" in
Darwin)
    LIB_EXT="dylib"
    ;;
Linux)
    LIB_EXT="so"
    ;;
*)
    echo "Unsupported host platform for UniFFI generation."
    exit 1
    ;;
esac

echo "Building host FFI library..."
cargo build -p connected-ffi --manifest-path "$ROOT_DIR/Cargo.toml"

LIB_PATH="$ROOT_DIR/target/debug/libconnected_ffi.$LIB_EXT"
if [[ ! -f "$LIB_PATH" ]]; then
    echo "Expected UniFFI metadata library not found: $LIB_PATH"
    exit 1
fi

echo "Generating Swift bindings..."
cargo run --release -p connected-ffi --bin uniffi-bindgen --manifest-path "$ROOT_DIR/Cargo.toml" -- \
    generate \
    --library "$LIB_PATH" \
    --language swift \
    --out-dir "$OUT_DIR" \
    --no-format

echo "Swift bindings generated in $OUT_DIR"
