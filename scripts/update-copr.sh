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
need_cmd copr-cli

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COPR_CFG="${ROOT_DIR}/packaging/rpm/copr-package.cfg"

[ -f "$COPR_CFG" ] || die "COPR config not found at ${COPR_CFG}"

# Source the config
source "$COPR_CFG"

if [ "$version" = "latest" ]; then
  version="$(resolve_latest_version)"
fi

echo "Updating COPR package to version: ${version}"

# Default chroots from config if not specified
if [ -z "$chroots" ]; then
  chroots="$COPR_CHROOTS"
fi

# Build for each architecture
for arch in $ARCHITECTURES; do
  echo ""
  echo "=== Building for $arch ==="
  
  # Download the RPM from GitHub release
  rpm_url="https://github.com/paterkleomenis/connected/releases/download/${version}/connected-desktop-${arch}.rpm"
  rpm_file="/tmp/connected-desktop-${version}-${arch}.rpm"
  
  echo "Downloading RPM from: $rpm_url"
  curl -fSL "$rpm_url" -o "$rpm_file"
  
  if [ ! -f "$rpm_file" ]; then
    die "Failed to download RPM for $arch"
  fi
  
  echo "Downloaded: $(ls -lh "$rpm_file" | awk '{print $5}')"
  
  # Determine chroot for this architecture
  case "$arch" in
    x86_64)
      build_chroot="fedora-43-x86_64"
      ;;
    aarch64)
      build_chroot="fedora-43-aarch64"
      ;;
    *)
      die "Unsupported architecture: $arch"
      ;;
  esac
  
  if [ "$do_push" -eq 1 ]; then
    echo "Submitting build to COPR project: $COPR_PROJECT"
    echo "  Chroot: $build_chroot"
    
    # Create a temporary directory for the SRPM-like structure
    temp_dir=$(mktemp -d)
    trap "rm -rf $temp_dir" EXIT
    
    # For COPR, we can submit a URL to the RPM directly
    # This tells COPR to build from the GitHub release RPM
    copr-cli build \
      --nowait \
      --chroot "$build_chroot" \
      "$COPR_PROJECT" \
      "$rpm_url"
    
    echo "Build submitted successfully for $arch"
  else
    echo "Dry run: would submit build to COPR project: $COPR_PROJECT"
    echo "  RPM URL: $rpm_url"
    echo "  Chroot: $build_chroot"
  fi
  
  # Clean up downloaded RPM
  rm -f "$rpm_file"
done

echo ""
echo "COPR update complete for version $version"

if [ "$do_push" -eq 1 ]; then
  echo "Builds submitted to: https://copr.fedorainfracloud.org/coprs/$COPR_PROJECT/"
  echo "Users can install with:"
  echo "  sudo dnf copr enable $COPR_PROJECT"
  echo "  sudo dnf install connected-desktop"
fi
