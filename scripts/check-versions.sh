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
CARGO_VERSION=$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"[^"]+"' Cargo.toml | tail -1 | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$CARGO_VERSION" ]; then
    echo -e "${RED}❌ Error: Could not find version in Cargo.toml${NC}"
    exit 1
fi

echo "   Cargo.toml workspace version: $CARGO_VERSION"

# Check Android derives versionName from Cargo.toml instead of hardcoding it.
ANDROID_GRADLE_FILE="android/app/build.gradle.kts"
if [ -f "$ANDROID_GRADLE_FILE" ]; then
    if ! grep -q 'val workspaceVersion' "$ANDROID_GRADLE_FILE" || ! grep -q 'versionName = workspaceVersion' "$ANDROID_GRADLE_FILE"; then
        echo -e "${RED}❌ Error: Android versionName must come from Cargo.toml via workspaceVersion${NC}"
        exit 1
    fi

    echo -e "${GREEN}✅ $ANDROID_GRADLE_FILE versionName derives from Cargo.toml${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $ANDROID_GRADLE_FILE not found, skipping check${NC}"
fi

# Check iOS version config is either the committed placeholder or generated from Cargo.toml.
IOS_VERSION_CONFIG="ios/Config/Version.xcconfig"
if [ -f "$IOS_VERSION_CONFIG" ]; then
    IOS_VERSION=$(grep -E '^[[:space:]]*MARKETING_VERSION[[:space:]]*=[[:space:]]*[^[:space:]]+' "$IOS_VERSION_CONFIG" | head -1 | sed -E 's/.*=[[:space:]]*([^[:space:]]+).*/\1/')
    IOS_BUILD_VERSION=$(grep -E '^[[:space:]]*CURRENT_PROJECT_VERSION[[:space:]]*=[[:space:]]*[^[:space:]]+' "$IOS_VERSION_CONFIG" | head -1 | sed -E 's/.*=[[:space:]]*([^[:space:]]+).*/\1/')

    if [ -z "$IOS_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find MARKETING_VERSION in $IOS_VERSION_CONFIG${NC}"
        exit 1
    fi

    if [ -z "$IOS_BUILD_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find CURRENT_PROJECT_VERSION in $IOS_VERSION_CONFIG${NC}"
        exit 1
    fi

    echo "   $IOS_VERSION_CONFIG MARKETING_VERSION: $IOS_VERSION"

    if [ "$IOS_VERSION" = "0.0.0" ] && [ "$IOS_BUILD_VERSION" = "0" ]; then
        echo -e "${GREEN}✅ $IOS_VERSION_CONFIG uses placeholder values for generated iOS builds${NC}"
    elif [ "$CARGO_VERSION" != "$IOS_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $IOS_VERSION_CONFIG: $IOS_VERSION"
        echo ""
        echo "   To fix, run scripts/ios/sync-version.sh"
        exit 1
    else
        echo -e "${GREEN}✅ $IOS_VERSION_CONFIG generated version matches${NC}"
    fi
else
    echo -e "${YELLOW}⚠️  Warning: $IOS_VERSION_CONFIG not found, skipping check${NC}"
fi

# Check installer.wxs version
WIX_FILE="packaging/windows/installer.wxs"
if [ -f "$WIX_FILE" ]; then
    WIX_VERSION=$(grep -E "^[[:space:]]*Version=['\"][^'\"]+['\"]" "$WIX_FILE" | head -1 | sed -E "s/.*Version=['\"]([^'\"]+)['\"].*/\1/")

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
