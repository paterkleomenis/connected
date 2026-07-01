#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

ok()   { printf "${GREEN}✓ %s${NC}\n" "$*"; }
warn() { printf "${YELLOW}⚠ %s${NC}\n" "$*"; }
err()  { printf "${RED}✗ %s${NC}\n" "$*"; exit 1; }

usage() {
  cat <<EOF
Usage: $(basename "$0") <version> [--android-code <code>] [--date <date>]

Bump the app version across all packaging and config files.

Arguments:
  <version>         New version in X.Y.Z format (e.g., 3.2.1)
  --android-code    Android versionCode (default: auto from semver)
  --date            Release date (default: today, YYYY-MM-DD)
EOF
  exit 1
}

[[ $# -lt 1 ]] && usage

NEW_VER="$1"; shift
[[ "$NEW_VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || err "Version must be X.Y.Z (e.g., 3.2.1)"

ANDROID_CODE=""
RELEASE_DATE="$(date +%Y-%m-%d)"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --android-code) ANDROID_CODE="$2"; shift 2 ;;
    --date)         RELEASE_DATE="$2"; shift 2 ;;
    *)              echo "Unknown: $1"; usage ;;
  esac
done

# Find current version from Cargo.toml
CARGO_TOML="$ROOT_DIR/Cargo.toml"
OLD_VER="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"[^"]+"' "$CARGO_TOML" | tail -1 | sed -E 's/.*"([^"]+)".*/\1/')"
[[ -n "$OLD_VER" ]] || err "Could not find version in Cargo.toml"

if [[ "$OLD_VER" == "$NEW_VER" ]]; then
  warn "Version is already $NEW_VER — nothing to do"
  exit 0
fi

# Escape dots for regex matching (sed uses . as wildcard)
OLD_VER_RE="${OLD_VER//\./\\.}"
NEW_VER_RE="${NEW_VER//\./\\.}"

# Default Android versionCode: encode major.minor.patch as MMMMmmpp
if [[ -z "$ANDROID_CODE" ]]; then
  IFS='.' read -r major minor patch <<< "$NEW_VER"
  ANDROID_CODE=$(( (10#$major * 10000) + (10#$minor * 100) + 10#$patch ))
fi

echo "Bumping version: ${OLD_VER} → ${NEW_VER}"
echo "  Android code:   ${ANDROID_CODE}"
echo "  Release date:   ${RELEASE_DATE}"
echo ""

# ────────────────────────────────────────────────────────────────────
# 1. Cargo.toml — workspace.package.version
# ────────────────────────────────────────────────────────────────────
if grep -q "^version = \"$OLD_VER_RE\"" "$CARGO_TOML"; then
  sed -i "s/^version = \"$OLD_VER_RE\"/version = \"$NEW_VER\"/" "$CARGO_TOML"
  ok "Cargo.toml"
else
  warn "Cargo.toml: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 2. PERMISSION_JUSTIFICATION.md — Version + Date
# ────────────────────────────────────────────────────────────────────
PERM="$ROOT_DIR/PERMISSION_JUSTIFICATION.md"
if grep -q "\\*\\*Version:\\*\\* $OLD_VER_RE" "$PERM"; then
  sed -i "s/\\*\\*Version:\\*\\* $OLD_VER_RE/**Version:** $NEW_VER/" "$PERM"
  sed -i "s/\\*\\*Date:\\*\\* .*/**Date:** $RELEASE_DATE/" "$PERM"
  ok "PERMISSION_JUSTIFICATION.md"
else
  warn "PERMISSION_JUSTIFICATION.md: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 3. android/app/build.gradle.kts — versionName + versionCode
# ────────────────────────────────────────────────────────────────────
GRADLE="$ROOT_DIR/android/app/build.gradle.kts"
if grep -q "versionName = \"$OLD_VER_RE\"" "$GRADLE"; then
  sed -i "s/versionName = \"$OLD_VER_RE\"/versionName = \"$NEW_VER\"/" "$GRADLE"
  sed -ri "s/(versionCode = )[0-9]+/\1$ANDROID_CODE/" "$GRADLE"
  ok "android/app/build.gradle.kts"
else
  warn "android/app/build.gradle.kts: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 4. packaging/aur/.SRCINFO — pkgver + hardcoded URLs
# ────────────────────────────────────────────────────────────────────
SRCINFO="$ROOT_DIR/packaging/aur/.SRCINFO"
if grep -q "pkgver = $OLD_VER_RE" "$SRCINFO"; then
  sed -i "s/$OLD_VER_RE/$NEW_VER/g" "$SRCINFO"
  ok "packaging/aur/.SRCINFO"
else
  warn "packaging/aur/.SRCINFO: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 5. packaging/aur/PKGBUILD — pkgver
# ────────────────────────────────────────────────────────────────────
PKGBUILD="$ROOT_DIR/packaging/aur/PKGBUILD"
if grep -q "^pkgver=$OLD_VER_RE" "$PKGBUILD"; then
  sed -i "s/^pkgver=$OLD_VER_RE/pkgver=$NEW_VER/" "$PKGBUILD"
  ok "packaging/aur/PKGBUILD"
else
  warn "packaging/aur/PKGBUILD: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 6. packaging/flatpak/com.paterkleomenis.Connected.metainfo.xml
# ────────────────────────────────────────────────────────────────────
METAINFO="$ROOT_DIR/packaging/flatpak/com.paterkleomenis.Connected.metainfo.xml"
if grep -q "<release version=\"$OLD_VER_RE\"" "$METAINFO"; then
  sed -i "s|<release version=\"$OLD_VER_RE\" date=\"[^\"]*\"|<release version=\"$NEW_VER\" date=\"$RELEASE_DATE\"|" "$METAINFO"
  ok "flatpak metainfo.xml"
else
  warn "flatpak metainfo.xml: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 7. packaging/rpm/connected-desktop.spec — Version + changelog
# ────────────────────────────────────────────────────────────────────
SPEC="$ROOT_DIR/packaging/rpm/connected-desktop.spec"
if grep -qE "^Version:[[:space:]]+$OLD_VER_RE" "$SPEC"; then
  sed -ri "s/^(Version:[[:space:]]*)$OLD_VER_RE/\1$NEW_VER/" "$SPEC"
  sed -i "/^\\* /s/$OLD_VER_RE/$NEW_VER/g" "$SPEC"
  ok "packaging/rpm/connected-desktop.spec"
else
  warn "packaging/rpm/connected-desktop.spec: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 8. packaging/windows/installer.wxs
# ────────────────────────────────────────────────────────────────────
WXS="$ROOT_DIR/packaging/windows/installer.wxs"
if grep -q "Version='$OLD_VER_RE'" "$WXS"; then
  sed -i "s/Version='$OLD_VER_RE'/Version='$NEW_VER'/" "$WXS"
  ok "packaging/windows/installer.wxs"
else
  warn "packaging/windows/installer.wxs: pattern not matched"
fi

# ────────────────────────────────────────────────────────────────────
# 9. scripts/ios/sync-version.sh — reads from Cargo.toml, no change needed
# ────────────────────────────────────────────────────────────────────

echo ""
ok "All files updated. Run \`scripts/check-versions.sh\` to verify."
echo ""
echo "Notes:"
echo "  - scripts/ios/sync-version.sh reads from Cargo.toml dynamically — no change needed"
echo "  - AUR sha256sums may need updating — run scripts/update-aur.sh if publishing"
echo "  - RPM changelog date updated to ${RELEASE_DATE}"
