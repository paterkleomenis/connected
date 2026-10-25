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

# Check Android versionName matches Cargo.toml.
ANDROID_GRADLE_FILE="android/app/build.gradle.kts"
if [ -f "$ANDROID_GRADLE_FILE" ]; then
    ANDROID_VERSION=$(grep -E '^[[:space:]]*versionName[[:space:]]*=[[:space:]]*"[^"]+"' "$ANDROID_GRADLE_FILE" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')

    if [ -z "$ANDROID_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find versionName in $ANDROID_GRADLE_FILE${NC}"
        exit 1
    fi

    echo "   $ANDROID_GRADLE_FILE versionName: $ANDROID_VERSION"

    if [ "$CARGO_VERSION" != "$ANDROID_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $ANDROID_GRADLE_FILE versionName: $ANDROID_VERSION"
        echo ""
        echo "   To fix, update versionName in $ANDROID_GRADLE_FILE to match Cargo.toml"
        exit 1
    fi

    echo -e "${GREEN}✅ $ANDROID_GRADLE_FILE versionName matches${NC}"

    ANDROID_CODE=$(grep -E '^[[:space:]]*versionCode[[:space:]]*=' "$ANDROID_GRADLE_FILE" | head -1 | sed -E 's/.*=[[:space:]]*([0-9]+).*/\1/')
    if [ -z "$ANDROID_CODE" ]; then
        echo -e "${RED}❌ Error: Could not find versionCode in $ANDROID_GRADLE_FILE${NC}"
        exit 1
    fi

    IFS=. read -r VERSION_MAJOR VERSION_MINOR VERSION_PATCH <<< "$CARGO_VERSION"
    EXPECTED_ANDROID_CODE=$((10#$VERSION_MAJOR * 10000 + 10#$VERSION_MINOR * 100 + 10#$VERSION_PATCH))
    echo "   $ANDROID_GRADLE_FILE versionCode: $ANDROID_CODE (expected $EXPECTED_ANDROID_CODE)"
    if [ "$ANDROID_CODE" != "$EXPECTED_ANDROID_CODE" ]; then
        echo -e "${RED}❌ Error: versionCode does not match $CARGO_VERSION (expected $EXPECTED_ANDROID_CODE)${NC}"
        exit 1
    fi

    echo -e "${GREEN}✅ $ANDROID_GRADLE_FILE versionCode matches${NC}"
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

# Check AUR PKGBUILD (pkgver)
AUR_PKGBUILD="packaging/aur/PKGBUILD"
if [ -f "$AUR_PKGBUILD" ]; then
    AUR_VERSION=$(grep -E '^pkgver=' "$AUR_PKGBUILD" | head -1 | cut -d= -f2)
    if [ -z "$AUR_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find pkgver in $AUR_PKGBUILD${NC}"
        exit 1
    fi

    echo "   $AUR_PKGBUILD pkgver: $AUR_VERSION"
    if [ "$CARGO_VERSION" != "$AUR_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $AUR_PKGBUILD: $AUR_VERSION"
        echo ""
        echo "   To fix, run scripts/bump-version.sh $CARGO_VERSION"
        exit 1
    fi

    echo -e "${GREEN}✅ $AUR_PKGBUILD version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $AUR_PKGBUILD not found, skipping check${NC}"
fi

# Check AUR .SRCINFO (pkgver)
AUR_SRCINFO="packaging/aur/.SRCINFO"
if [ -f "$AUR_SRCINFO" ]; then
    SRCINFO_VERSION=$(grep -E '^[[:space:]]*pkgver[[:space:]]*=' "$AUR_SRCINFO" | head -1 | sed -E 's/.*=[[:space:]]*//' | tr -d '\r')
    if [ -z "$SRCINFO_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find pkgver in $AUR_SRCINFO${NC}"
        exit 1
    fi

    echo "   $AUR_SRCINFO pkgver: $SRCINFO_VERSION"
    if [ "$CARGO_VERSION" != "$SRCINFO_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $AUR_SRCINFO: $SRCINFO_VERSION"
        echo ""
        echo "   To fix, run scripts/update-aur.sh $CARGO_VERSION --no-push"
        exit 1
    fi

    echo -e "${GREEN}✅ $AUR_SRCINFO version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $AUR_SRCINFO not found, skipping check${NC}"
fi

# Check Flatpak metainfo (latest <release> entry)
FLATPAK_METAINFO="packaging/flatpak/io.github.paterkleomenis.Connected.metainfo.xml"
if [ -f "$FLATPAK_METAINFO" ]; then
    FLATPAK_VERSION=$(grep -m1 -oE '<release version="[^"]+"' "$FLATPAK_METAINFO" | sed -E 's/.*"([^"]+)"/\1/')
    if [ -z "$FLATPAK_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find a <release> entry in $FLATPAK_METAINFO${NC}"
        exit 1
    fi

    echo "   $FLATPAK_METAINFO latest release: $FLATPAK_VERSION"
    if [ "$CARGO_VERSION" != "$FLATPAK_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $FLATPAK_METAINFO: $FLATPAK_VERSION"
        echo ""
        echo "   To fix, add a <release version=\"$CARGO_VERSION\" ...> entry to $FLATPAK_METAINFO"
        exit 1
    fi

    echo -e "${GREEN}✅ $FLATPAK_METAINFO version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $FLATPAK_METAINFO not found, skipping check${NC}"
fi

# Check RPM spec (Version)
RPM_SPEC="packaging/rpm/connected-desktop.spec"
if [ -f "$RPM_SPEC" ]; then
    RPM_VERSION=$(grep -E '^Version:' "$RPM_SPEC" | head -1 | awk '{print $2}')
    if [ -z "$RPM_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find Version in $RPM_SPEC${NC}"
        exit 1
    fi

    echo "   $RPM_SPEC version: $RPM_VERSION"
    if [ "$CARGO_VERSION" != "$RPM_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $RPM_SPEC: $RPM_VERSION"
        echo ""
        echo "   To fix, update the Version field in $RPM_SPEC"
        exit 1
    fi

    echo -e "${GREEN}✅ $RPM_SPEC version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $RPM_SPEC not found, skipping check${NC}"
fi

# Check Play Store permission justification document (Version)
PLAY_DOC="PERMISSION_JUSTIFICATION.md"
if [ -f "$PLAY_DOC" ]; then
    PLAY_VERSION=$(grep -E '^\*\*Version:\*\*' "$PLAY_DOC" | head -1 | sed -E 's/.*\*\*Version:\*\*[[:space:]]*//' | tr -d ' \r')
    if [ -z "$PLAY_VERSION" ]; then
        echo -e "${RED}❌ Error: Could not find **Version:** in $PLAY_DOC${NC}"
        exit 1
    fi

    echo "   $PLAY_DOC version: $PLAY_VERSION"
    if [ "$CARGO_VERSION" != "$PLAY_VERSION" ]; then
        echo -e "${RED}❌ Error: Version mismatch!${NC}"
        echo "   Cargo.toml: $CARGO_VERSION"
        echo "   $PLAY_DOC: $PLAY_VERSION"
        echo ""
        echo "   To fix, update the **Version:** line in $PLAY_DOC"
        exit 1
    fi

    echo -e "${GREEN}✅ $PLAY_DOC version matches${NC}"
else
    echo -e "${YELLOW}⚠️  Warning: $PLAY_DOC not found, skipping check${NC}"
fi

echo -e "${GREEN}✅ All version checks passed${NC}"
