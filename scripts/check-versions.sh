#!/usr/bin/env bash
# Version consistency check script
# Ensures all version files are in sync

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "🔍 Checking version consistency across project files..."

# Extract version from Cargo.toml (workspace.package.version)
CARGO_VERSION=$(grep -E '^\s*version\s*=\s*"[^"]+"' Cargo.toml | tail -1 | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$CARGO_VERSION" ]; then
    echo -e "${RED}❌ Error: Could not find version in Cargo.toml${NC}"
    exit 1
fi

echo "   Cargo.toml workspace version: $CARGO_VERSION"

# Check installer.wxs version
WIX_FILE="packaging/windows/installer.wxs"
if [ -f "$WIX_FILE" ]; then
    WIX_VERSION=$(grep -E "^\s*Version=['\"][^'\"]+['\"]" "$WIX_FILE" | head -1 | sed -E "s/.*Version=['\"]([^'\"]+)['\"].*/\1/")

    if [ -z "$WIX_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find Version in $WIX_FILE${NC}"
        exit 1
    fi

    echo "   $WIX_FILE version: $WIX_VERSION"

    if [ "$CARGO_VERSION" != "$WIX_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $WIX_FILE: $WIX_VERSION"
        echo ""
        echo "   To fix, update the Version attribute in $WIX_FILE to match Cargo.toml"
        exit 1
    fi

    echo -e "${GREEN}✅ $WIX_FILE version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $WIX_FILE not found, skipping check${NC}"
fi

echo -e "${GREEN}✅ All version checks passed${NC}"
