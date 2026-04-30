#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO_TOML="$ROOT_DIR/Cargo.toml"
VERSION_CONFIG="$ROOT_DIR/ios/Config/Version.xcconfig"

VERSION="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"[^"]+"' "$CARGO_TOML" | tail -1 | sed -E 's/.*"([^"]+)".*/\1/')"

if [[ -z "$VERSION" ]]; then
    echo "Could not find workspace package version in $CARGO_TOML" >&2
    exit 1
fi

BUILD_NUMBER="${IOS_BUILD_NUMBER:-1}"

if [[ ! "$BUILD_NUMBER" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
    echo "Invalid iOS build number: $BUILD_NUMBER" >&2
    echo "CURRENT_PROJECT_VERSION must be one to three period-separated integers, such as 1, 2.1, or 3.0.4." >&2
    exit 1
fi

mkdir -p "$(dirname "$VERSION_CONFIG")"
cat > "$VERSION_CONFIG" <<EOF
MARKETING_VERSION = $VERSION
CURRENT_PROJECT_VERSION = $BUILD_NUMBER
EOF

echo "Synced iOS version: MARKETING_VERSION=$VERSION CURRENT_PROJECT_VERSION=$BUILD_NUMBER"
