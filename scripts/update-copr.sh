#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

usage() {
  cat <<'USAGE'
Usage: scripts/update-copr.sh [--latest] [--push]
       scripts/update-copr.sh <version> [--push]

Updates the COPR package for connected-desktop.

Options:
  (no args)       Resolve latest GitHub release tag and use that version
  --latest        Resolve latest GitHub release tag and use that version
  --push          Submit build to COPR (default)
  --no-push       Dry run, don't submit to COPR
  --dry-run       Same as --no-push
  --chroot        Specify chroot (default: all configured chroots)
USAGE
}

die() {
  echo "Error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

resolve_latest_version() {
  local location
  location=$(curl -fsI "https://github.com/paterkleomenis/connected/releases/latest" | tr -d '\r' | awk -F': ' '/^location:/ {print $2}')
  [ -n "$location" ] || die "failed to resolve latest release redirect"
  local tag="${location##*/}"
  echo "${tag#v}"
}

version=""
do_push=1
chroots=""

while [ $# -gt 0 ]; do
  case "$1" in
    --latest)
      version="latest"
      shift
      ;;
    --push)
      do_push=1
      shift
      ;;
    --no-push|--dry-run)
      do_push=0
      shift
      ;;
    --chroot)
      chroots="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      if [ -z "$version" ]; then
        version="${1#v}"
        shift
      else
        die "unexpected argument: $1"
      fi
      ;;
  esac
done

if [ -z "$version" ]; then
  version="latest"
fi

need_cmd curl
need_cmd rpmbuild

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COPR_CFG="${ROOT_DIR}/packaging/rpm/copr-package.cfg"

[ -f "$COPR_CFG" ] || die "COPR config not found at ${COPR_CFG}"

# Source the config
# shellcheck source=packaging/rpm/copr-package.cfg
source "$COPR_CFG"

if [ "$version" = "latest" ]; then
  version="$(resolve_latest_version)"
fi

echo "Updating COPR package to version: ${version}"

# Default chroots from config if not specified
if [ -z "$chroots" ]; then
  chroots="$COPR_CHROOTS"
fi

# Create temp directory for SRPM builds
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

# Build SRPM and submit for each architecture
# Each architecture gets its own SRPM containing the correct binary
IFS=' ' read -ra ARCH_ARRAY <<< "$ARCHITECTURES"
for arch in "${ARCH_ARRAY[@]}"; do
  echo ""
  echo "=== Building SRPM for $arch ==="

  # Download the binary from GitHub releases
  rpm_url="https://github.com/paterkleomenis/connected/releases/download/${version}/connected-desktop-linux-${arch}"
  binary_file="$TMPDIR/connected-desktop-linux-${arch}"

  echo "Downloading binary from: $rpm_url"
  curl -fSL "$rpm_url" -o "$binary_file"

  if [ ! -f "$binary_file" ]; then
    die "Failed to download binary for $arch"
  fi

  # Create RPM build directory structure for this architecture
  RPM_DIR="$TMPDIR/rpm-$arch"
  mkdir -p "$RPM_DIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

  # Copy spec file and update version
  cp "$ROOT_DIR/packaging/rpm/connected-desktop.spec" "$RPM_DIR/SPECS/"
  sed -i "s/^Version:.*/Version:        $version/" "$RPM_DIR/SPECS/connected-desktop.spec"

  # Copy the binary
  cp "$binary_file" "$RPM_DIR/SOURCES/connected-desktop-linux-${arch}"

  # Copy source files (Source1-Source4)
  cp "$ROOT_DIR/packaging/connected-desktop.desktop" "$RPM_DIR/SOURCES/"
  cp "$ROOT_DIR/packaging/flatpak/com.paterkleomenis.Connected.png" "$RPM_DIR/SOURCES/"
  cp "$ROOT_DIR/LICENSE-MIT" "$RPM_DIR/SOURCES/"
  cp "$ROOT_DIR/LICENSE-APACHE" "$RPM_DIR/SOURCES/"

  # Build SRPM
  rpmbuild -bs \
    --define "_topdir $RPM_DIR" \
    --define "_arch $arch" \
    "$RPM_DIR/SPECS/connected-desktop.spec"

  SRPM=$(find "$RPM_DIR/SRPMS" -name "*.src.rpm" -type f | head -1)
  [ -n "$SRPM" ] || die "SRPM build failed for $arch"

  echo "SRPM built: $SRPM"

  # Filter chroots for this architecture
  arch_chroots=""
  IFS=' ' read -ra CHROOT_ARRAY <<< "$chroots"
  for chroot in "${CHROOT_ARRAY[@]}"; do
    case "$chroot" in
      *"$arch"*) arch_chroots="$arch_chroots $chroot" ;;
    esac
  done

  if [ -z "$arch_chroots" ]; then
    die "No chroots found for architecture: $arch"
  fi

  echo "  Chroots:$arch_chroots"

  if [ "$do_push" -eq 1 ]; then
    need_cmd copr-cli

    echo "Submitting build to COPR project: $COPR_PROJECT"

    # Build copr-cli --chroot flags
    chroot_args=""
    IFS=' ' read -ra FILTERED_CHROOTS <<< "$arch_chroots"
    for chroot in "${FILTERED_CHROOTS[@]}"; do
      chroot_args="$chroot_args --chroot $chroot"
    done

    # shellcheck disable=SC2086
    copr-cli build \
      --nowait \
      $chroot_args \
      "$COPR_PROJECT" \
      "$SRPM"

    echo "Build submitted successfully for $arch"
  else
    echo "Dry run: would submit build to COPR project: $COPR_PROJECT"
    echo "  SRPM: $SRPM"
    echo "  Chroots:$arch_chroots"
  fi
done

echo ""
echo "COPR update complete for version $version"

if [ "$do_push" -eq 1 ]; then
  echo "Builds submitted to: https://copr.fedorainfracloud.org/coprs/$COPR_PROJECT/"
  echo "Users can install with:"
  echo "  sudo dnf copr enable $COPR_PROJECT"
  echo "  sudo dnf install connected-desktop"
fi
