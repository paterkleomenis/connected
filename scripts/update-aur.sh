#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

usage() {
  cat <<'USAGE'
Usage: scripts/update-aur.sh [--latest] [--hash-bin] [--skip-srcinfo] [--push]
       scripts/update-aur.sh <version> [--hash-bin] [--skip-srcinfo] [--push]
       scripts/update-aur.sh --verify

Updates packaging/aur/PKGBUILD (pkgver/pkgrel + sha256sums) and .SRCINFO.

Options:
  (no args)       Resolve latest GitHub release tag and use that version
  --latest        Resolve latest GitHub release tag and use that version
  --hash-bin      Compute hash for the binary asset (default)
  --no-hash-bin   Skip hashing the binary asset (uses SKIP)
  --skip-srcinfo  Do not regenerate .SRCINFO
  --push          Commit and push changes to the AUR remote (default)
  --no-push       Do not commit or push changes
  --verify        Re-download all sources and verify they match PKGBUILD hashes
  --no-verify     Skip the post-update verification step
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

count_sources() {
  awk 'BEGIN{c=0} /^source=\(/ {block=1} block { if ($0 ~ /\)/) {block=0} if ($0 ~ /"/) c++ } END{print c}' "$1"
}

count_sums() {
  awk 'BEGIN{c=0} /^sha256sums=\(/ {block=1} block { if ($0 ~ /\)/) {block=0} if ($0 ~ /'\''/) c++ } END{print c}' "$1"
}

resolve_latest_version() {
  local location
  location=$(curl -fsI "https://github.com/paterkleomenis/connected/releases/latest" | tr -d '\r' | awk -F': ' '/^location:/ {print $2}')
  [ -n "$location" ] || die "failed to resolve latest release redirect"
  local tag="${location##*/}"
  echo "${tag#v}"
}

# Extract sha256sums from a PKGBUILD file into an indexed bash array
# Usage: extract_pkgbuild_sums <pkgbuild_path>
# Sets global array PKGBUILD_SUMS
extract_pkgbuild_sums() {
  local pkgbuild="$1"
  PKGBUILD_SUMS=()
  local in_block=0
  while IFS= read -r line; do
    if [[ "$line" =~ ^sha256sums= ]]; then
      in_block=1
    fi
    if [ "$in_block" -eq 1 ]; then
      # Extract hash values between single quotes
      while [[ "$line" =~ \'([a-fA-F0-9]{64}|SKIP)\' ]]; do
        PKGBUILD_SUMS+=("${BASH_REMATCH[1]}")
        line="${line#*"${BASH_REMATCH[0]}"}"
      done
      if [[ "$line" =~ \) ]]; then
        in_block=0
      fi
    fi
  done < "$pkgbuild"
}

# Extract source URLs from a PKGBUILD file
# Usage: extract_pkgbuild_sources <pkgbuild_path> <pkgver>
# Sets global array PKGBUILD_SOURCES
extract_pkgbuild_sources() {
  local pkgbuild="$1"
  local pkgver="$2"
  PKGBUILD_SOURCES=()
  local in_block=0
  while IFS= read -r line; do
    if [[ "$line" =~ ^source= ]]; then
      in_block=1
    fi
    if [ "$in_block" -eq 1 ]; then
      # Extract URLs between double quotes
      while [[ "$line" =~ \"([^\"]+)\" ]]; do
        local url="${BASH_REMATCH[1]}"
        # Strip localname:: prefix (makepkg rename syntax)
        if [[ "$url" == *"::"* ]]; then
          url="${url#*::}"
        fi
        # Expand ${pkgver} in the URL
        url="${url//\$\{pkgver\}/$pkgver}"
        url="${url//\$pkgver/$pkgver}"
        PKGBUILD_SOURCES+=("$url")
        line="${line#*"${BASH_REMATCH[0]}"}"
      done
      if [[ "$line" =~ \) ]]; then
        in_block=0
      fi
    fi
  done < "$pkgbuild"
}

# Verify that all source URLs match their expected sha256sums from the PKGBUILD
# Returns 0 if all match, 1 if any mismatch
verify_pkgbuild_hashes() {
  local pkgbuild="$1"
  local ver="$2"
  local source_names=("connected-desktop-${ver}" "connected-desktop.desktop" "ic_launcher-playstore.png" "LICENSE-MIT" "LICENSE-APACHE")

  extract_pkgbuild_sums "$pkgbuild"
  extract_pkgbuild_sources "$pkgbuild" "$ver"

  local num_sources=${#PKGBUILD_SOURCES[@]}
  local num_sums=${#PKGBUILD_SUMS[@]}

  if [ "$num_sources" -eq 0 ]; then
    echo "  Error: no sources found in PKGBUILD"
    return 1
  fi
  if [ "$num_sources" -ne "$num_sums" ]; then
    echo "  Error: source count ($num_sources) != sha256sums count ($num_sums)"
    return 1
  fi

  local all_ok=1
  for i in $(seq 0 $((num_sources - 1))); do
    local url="${PKGBUILD_SOURCES[$i]}"
    local expected="${PKGBUILD_SUMS[$i]}"
    local name="${source_names[$i]:-source_$i}"

    if [ "$expected" = "SKIP" ]; then
      echo "  ${name}: SKIP (not verified)"
      continue
    fi

    echo -n "  ${name}: downloading... "
    local actual
    if ! actual="$(hash_url "$url" 2>/dev/null)"; then
      echo "FAILED (download error)"
      echo "    URL: $url"
      all_ok=0
      continue
    fi

    if [ "$actual" = "$expected" ]; then
      echo "OK (${expected:0:16}...)"
    else
      echo "MISMATCH!"
      echo "    Expected: $expected"
      echo "    Got:      $actual"
      echo "    URL:      $url"
      all_ok=0
    fi
  done

  if [ "$all_ok" -eq 1 ]; then
    return 0
  else
    return 1
  fi
}

# Clean cached source/build files from the AUR directory
clean_aur_cache() {
  local aur_dir="$1"
  rm -f "${aur_dir}"/connected-desktop-*
  rm -f "${aur_dir}/connected-desktop.desktop"
  rm -f "${aur_dir}/ic_launcher-playstore.png"
  rm -f "${aur_dir}/LICENSE-MIT"
  rm -f "${aur_dir}/LICENSE-APACHE"
  rm -rf "${aur_dir}/src" "${aur_dir}/pkg"
  rm -f "${aur_dir}"/*.pkg.tar.*
}

version=""
hash_bin=1
skip_srcinfo=0
do_push=1
do_verify=1
verify_only=0
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
    --no-hash-bin)
      hash_bin=0
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
    --no-push)
      do_push=0
      shift
      ;;
    --verify)
      verify_only=1
      shift
      ;;
    --no-verify)
      do_verify=0
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

if [ -z "$version" ]; then
  version="latest"
fi

need_cmd curl
need_cmd sha256sum
need_cmd awk
need_cmd sed
need_cmd perl

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUR_DIR="${ROOT_DIR}/packaging/aur"
PKGBUILD="${AUR_DIR}/PKGBUILD"

[ -f "$PKGBUILD" ] || die "PKGBUILD not found at ${PKGBUILD}"

# --verify mode: just verify existing PKGBUILD hashes and exit
if [ "$verify_only" -eq 1 ]; then
  # Read current pkgver from PKGBUILD
  cur_ver="$(grep '^pkgver=' "$PKGBUILD" | cut -d= -f2)"
  [ -n "$cur_ver" ] || die "could not read pkgver from PKGBUILD"
  echo "Verifying PKGBUILD hashes for version ${cur_ver}..."
  if verify_pkgbuild_hashes "$PKGBUILD" "$cur_ver"; then
    echo ""
    echo "All checksums verified. Safe to install."
    # Clean cached sources so makepkg downloads fresh copies
    echo "Cleaning cached source files..."
    clean_aur_cache "$AUR_DIR"
    exit 0
  else
    echo ""
    echo "Checksum verification FAILED!"
    echo "The binary on GitHub has changed since the PKGBUILD was last updated."
    echo "Run 'scripts/update-aur.sh' to recompute hashes before installing."
    exit 1
  fi
fi

if [ "$version" = "latest" ]; then
  version="$(resolve_latest_version)"
fi

echo "Updating AUR package to version: ${version}"

# Clean cached source files so makepkg re-downloads them
echo "Cleaning cached source files..."
clean_aur_cache "$AUR_DIR"

# Update pkgver and reset pkgrel
sed -i "s/^pkgver=.*/pkgver=${version}/" "$PKGBUILD"
sed -i "s/^pkgrel=.*/pkgrel=1/" "$PKGBUILD"

# Build source URLs (must stay in sync with PKGBUILD)
bin_url="https://github.com/paterkleomenis/connected/releases/download/${version}/connected-desktop"
desktop_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/packaging/connected-desktop.desktop"
icon_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/android/app/src/main/ic_launcher-playstore.png"
license_mit_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/LICENSE-MIT"
license_apache_url="https://raw.githubusercontent.com/paterkleomenis/connected/main/LICENSE-APACHE"

# Decide whether to hash binary or keep SKIP
if [ "$hash_bin" -eq 1 ]; then
  echo "Hashing binary asset..."
  bin_sum="$(hash_url "$bin_url")"
  echo "  Binary hash: ${bin_sum}"
else
  bin_sum="SKIP"
  echo "  Binary hash: SKIP"
fi

echo "Hashing auxiliary sources..."
desktop_sum="$(hash_url "$desktop_url")"
echo "  Desktop file: ${desktop_sum:0:16}..."
icon_sum="$(hash_url "$icon_url")"
echo "  Icon:         ${icon_sum:0:16}..."
license_mit_sum="$(hash_url "$license_mit_url")"
echo "  LICENSE-MIT:  ${license_mit_sum:0:16}..."
license_apache_sum="$(hash_url "$license_apache_url")"
echo "  LICENSE-APACHE: ${license_apache_sum:0:16}..."

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

src_count="$(count_sources "$PKGBUILD")"
sum_count="$(count_sums "$PKGBUILD")"
if [ "$src_count" -eq 0 ] || [ "$sum_count" -eq 0 ]; then
  die "source/sha256sums block missing after update"
fi
if [ "$src_count" -ne "$sum_count" ]; then
  die "source/sha256sums count mismatch: sources=${src_count} sums=${sum_count}"
fi

# Post-update verification: re-download all sources and confirm hashes match
# This catches the case where a source (especially the binary) changes between
# the initial hash computation and when makepkg would download it.
if [ "$do_verify" -eq 1 ]; then
  echo ""
  echo "Verifying hashes against live URLs (post-update check)..."
  if verify_pkgbuild_hashes "$PKGBUILD" "$version"; then
    echo "Post-update verification passed."
  else
    echo ""
    echo "WARNING: Post-update verification FAILED!"
    echo "A source file (likely the binary) has changed on GitHub between"
    echo "when it was first hashed and the verification download just now."
    echo ""
    echo "This is exactly what causes the 'FAILED' error when running makepkg."
    echo "The binary was likely re-uploaded to the GitHub release."
    echo ""
    echo "To fix: ensure the binary at the release URL is final, then re-run:"
    echo "  scripts/update-aur.sh ${version}"
    die "post-update hash verification failed"
  fi
  echo ""
fi

if [ "$skip_srcinfo" -eq 0 ]; then
  if command -v makepkg >/dev/null 2>&1; then
    (cd "$AUR_DIR" && makepkg --printsrcinfo > .SRCINFO)
  else
    echo "Warning: makepkg not found; .SRCINFO not regenerated"
  fi
else
  echo "Skipping .SRCINFO regeneration"
fi

# Clean cached sources again after everything is done, so that when the user
# later runs `makepkg -si` in this directory, it downloads fresh copies instead
# of using stale cached files (the #1 cause of checksum mismatches).
echo "Cleaning cached source files (post-update)..."
clean_aur_cache "$AUR_DIR"

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
