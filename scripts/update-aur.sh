#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

usage() {
  cat <<'USAGE'
Usage: scripts/update-aur.sh <version> [--hash-bin] [--skip-srcinfo] [--push]
       scripts/update-aur.sh --latest [--hash-bin] [--skip-srcinfo] [--push]

Updates packaging/aur/PKGBUILD (pkgver/pkgrel + sha256sums) and .SRCINFO.

Options:
  --latest        Resolve latest GitHub release tag and use that version
  --hash-bin      Compute hash for the binary asset (otherwise keep SKIP if present)
  --skip-srcinfo  Do not regenerate .SRCINFO
  --push          Commit and push changes to the AUR remote
USAGE
}

die() {
  echo "Error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

hash_url() {
  local url="$1"
  curl -fsSL "$url" | sha256sum | awk '{print $1}'
}

resolve_latest_version() {
  local location
  location=$(curl -fsI "https://github.com/paterkleomenis/connected/releases/latest" | tr -d '\r' | awk -F': ' '/^location:/ {print $2}')
  [ -n "$location" ] || die "failed to resolve latest release redirect"
  local tag="${location##*/}"
  echo "${tag#v}"
}

version=""
hash_bin=0
skip_srcinfo=0
do_push=0
commit_msg=""
push_remote=""
push_branch=""

while [ $# -gt 0 ]; do
  case "$1" in
    --latest)
      version="latest"
      shift
      ;;
    --hash-bin)
      hash_bin=1
      shift
      ;;
    --skip-srcinfo)
      skip_srcinfo=1
      shift
      ;;
    --push)
      do_push=1
      shift
      ;;
    --commit-msg)
      commit_msg="${2:-}"
      [ -n "$commit_msg" ] || die "--commit-msg requires a value"
      shift 2
      ;;
    --remote)
      push_remote="${2:-}"
      [ -n "$push_remote" ] || die "--remote requires a value"
      shift 2
      ;;
    --branch)
      push_branch="${2:-}"
      [ -n "$push_branch" ] || die "--branch requires a value"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      if [ -z "$version" ]; then
        version="$1"
        shift
      else
        die "unexpected argument: $1"
      fi
      ;;
  esac
done

[ -n "$version" ] || { usage; exit 1; }

need_cmd curl
need_cmd sha256sum
need_cmd awk
need_cmd sed
need_cmd perl

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUR_DIR="${ROOT_DIR}/packaging/aur"
PKGBUILD="${AUR_DIR}/PKGBUILD"

[ -f "$PKGBUILD" ] || die "PKGBUILD not found at ${PKGBUILD}"

if [ "$version" = "latest" ]; then
  version="$(resolve_latest_version)"
fi

echo "Updating AUR package to version: ${version}"

# Update pkgver and reset pkgrel
sed -i "s/^pkgver=.*/pkgver=${version}/" "$PKGBUILD"
sed -i "s/^pkgrel=.*/pkgrel=1/" "$PKGBUILD"

# Build source URLs (must stay in sync with PKGBUILD)
bin_url="https://github.com/paterkleomenis/connected/releases/download/${version}/connected-desktop"
desktop_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/packaging/aur/connected-desktop.desktop"
icon_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/android/app/src/main/ic_launcher-playstore.png"
license_mit_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/LICENSE-MIT"
license_apache_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/LICENSE-APACHE"

# Decide whether to hash binary or keep SKIP
current_first_sum=$(awk '/^sha256sums=/{flag=1;next} flag{gsub(/[()'\'' ]/,""); if ($0!=""){print $0; exit}}' "$PKGBUILD" || true)
if [ "$hash_bin" -eq 1 ]; then
  echo "Hashing binary asset..."
  bin_sum="$(hash_url "$bin_url")"
elif [ -n "$current_first_sum" ] && [ "$current_first_sum" != "SKIP" ]; then
  echo "Hashing binary asset..."
  bin_sum="$(hash_url "$bin_url")"
else
  bin_sum="SKIP"
fi

echo "Hashing auxiliary sources..."
desktop_sum="$(hash_url "$desktop_url")"
icon_sum="$(hash_url "$icon_url")"
license_mit_sum="$(hash_url "$license_mit_url")"
license_apache_sum="$(hash_url "$license_apache_url")"

SHA_BLOCK_FILE="${AUR_DIR}/.sha256sums.tmp"
cat > "$SHA_BLOCK_FILE" <<EOF
sha256sums=('${bin_sum}'
            '${desktop_sum}'
            '${icon_sum}'
            '${license_mit_sum}'
            '${license_apache_sum}')
EOF

# Replace or append sha256sums block
export SHA_BLOCK_FILE
perl -0777 -i -pe 'BEGIN{ local $/; open my $fh, "<", $ENV{SHA_BLOCK_FILE} or die $!; $b=<$fh>; chomp $b; } if (s/sha256sums=\([^)]*\)/$b/s) { } else { $_ .= "\n\n$b\n"; }' "$PKGBUILD"
rm -f "$SHA_BLOCK_FILE"

if [ "$skip_srcinfo" -eq 0 ]; then
  if command -v makepkg >/dev/null 2>&1; then
    (cd "$AUR_DIR" && makepkg --printsrcinfo > .SRCINFO)
  else
    echo "Warning: makepkg not found; .SRCINFO not regenerated"
  fi
else
  echo "Skipping .SRCINFO regeneration"
fi

echo "AUR update complete."

if [ "$do_push" -eq 1 ]; then
  need_cmd git
  if ! git -C "$AUR_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    die "AUR directory is not a git repository: $AUR_DIR"
  fi

  if git -C "$AUR_DIR" diff --quiet --exit-code; then
    echo "No changes to commit."
    exit 0
  fi

  if [ -z "$commit_msg" ]; then
    commit_msg="Update to v${version}"
  fi

  git -C "$AUR_DIR" add PKGBUILD .SRCINFO || true
  if git -C "$AUR_DIR" diff --cached --quiet --exit-code; then
    echo "No staged changes to commit."
    exit 0
  fi

  git -C "$AUR_DIR" commit -m "$commit_msg"

  if [ -z "$push_remote" ]; then
    if git -C "$AUR_DIR" remote | grep -qx "aur"; then
      push_remote="aur"
    else
      push_remote="origin"
    fi
  fi

  if [ -z "$push_branch" ]; then
    push_branch="master"
  fi

  echo "Pushing to ${push_remote} ${push_branch}..."
  git -C "$AUR_DIR" push "$push_remote" "$push_branch"
fi
