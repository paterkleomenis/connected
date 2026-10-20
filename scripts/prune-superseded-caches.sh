#!/usr/bin/env bash
# Remove the cache entries that a job's upcoming cache save will supersede.
#
# Run this after a job's build steps have succeeded, but before the cache
# action's post step saves the replacement entry. Deleting the previous entry
# first means a cache miss does not temporarily double that job's cache
# footprint, which is what otherwise pushes the repository over the 10 GB
# limit and makes GitHub evict unrelated caches (quality, security, ...).
#
# Only entries created before the current workflow run started are removed, so
# a cache saved concurrently by another run is never touched.
#
# Usage: prune-superseded-caches.sh <key-prefix>
#
# Environment:
#   GITHUB_REPOSITORY  Repository to prune (provided by Actions).
#   GITHUB_RUN_ID      Workflow run ID (provided by Actions), used to resolve
#                      the run start time when RUN_STARTED_AT is not set.
#   RUN_STARTED_AT     Optional ISO 8601 cutoff; resolved from the API if unset.
#   GH_TOKEN           Token with actions:write (provided by Actions).
#   CACHE_REF          Ref to inspect. Defaults to refs/heads/main.
#   DRY_RUN            Set to 1 to log deletions without performing them.
#
# Written for Bash 3.2 so it also runs on the macOS runners.

set -euo pipefail
export LC_ALL=C

prefix="${1:-}"
if [[ -z "$prefix" ]]; then
  echo "usage: $0 <key-prefix>" >&2
  exit 2
fi

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"

if [[ -z "${RUN_STARTED_AT:-}" ]]; then
  : "${GITHUB_RUN_ID:?GITHUB_RUN_ID must be set}"
  if ! RUN_STARTED_AT="$(gh api \
    "repos/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}" \
    --jq '.run_started_at // empty')" || [[ -z "$RUN_STARTED_AT" ]]; then
    echo "::error::Failed to resolve the workflow run start time."
    exit 1
  fi
fi

ref="${CACHE_REF:-refs/heads/main}"
dry_run="${DRY_RUN:-0}"

echo "Pruning caches with prefix '${prefix}' for ${GITHUB_REPOSITORY}@${ref}"
echo "Entries created before ${RUN_STARTED_AT} are superseded."

if ! raw="$(gh api --paginate \
  "repos/${GITHUB_REPOSITORY}/actions/caches?ref=${ref}&per_page=100" \
  --jq '.actions_caches[] | [.id, .key, .created_at] | @tsv')"; then
  echo "::error::Failed to list GitHub Actions caches."
  exit 1
fi

if [[ -z "$raw" ]]; then
  echo "No caches found."
  exit 0
fi

deleted=0
while IFS=$'\t' read -r id key created; do
  [[ -n "$id" && -n "$key" && -n "$created" ]] || continue
  # Guard against CRLF output on Windows runners.
  key="${key%$'\r'}"
  created="${created%$'\r'}"
  [[ "$key" == "$prefix"* ]] || continue

  # ISO 8601 UTC timestamps sort lexicographically.
  [[ "$created" < "$RUN_STARTED_AT" ]] || continue

  echo "Deleting superseded cache ${id}: ${key} (created ${created})"
  if [[ "$dry_run" == "1" ]]; then
    continue
  fi

  if gh api -X DELETE "repos/${GITHUB_REPOSITORY}/actions/caches/${id}" >/dev/null; then
    deleted=$((deleted + 1))
  else
    echo "::warning::Failed to delete cache ${id} (${key}); it may already have been removed."
  fi
done <<< "$raw"

echo "Deleted ${deleted} superseded cache(s) for prefix '${prefix}'."
