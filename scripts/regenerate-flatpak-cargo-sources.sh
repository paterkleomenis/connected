#!/usr/bin/env bash
# Regenerate packaging/flatpak/cargo-sources.json from Cargo.lock.
#
# cargo runs with `--offline` against the sources listed in this file, so it
# must be regenerated whenever Cargo.lock changes. CI runs this from the Flatpak
# build job and again from the job that commits the result back to the branch.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${repo_root}/packaging/flatpak/cargo-sources.json"

python="${PYTHON:-python3}"
if ! command -v "${python}" >/dev/null 2>&1; then
    python=python
fi

generator="$(mktemp)"
deps_file="$(mktemp)"
trap 'rm -f "${generator}" "${deps_file}"' EXIT

curl -fsSL -o "${generator}" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"

# Install exactly the dependencies declared in the generator's PEP 723 metadata.
"${python}" - "${generator}" >"${deps_file}" <<'PY'
import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
block = re.search(r"(?ms)^# /// script[ \t]*$(.*?)^# ///[ \t]*$", text)
if block is None:
    sys.exit("no PEP 723 metadata block in generator")

metadata = "\n".join(
    line[2:] if line.startswith("# ") else line.lstrip("#")
    for line in block.group(1).splitlines()
)
dependencies = re.search(r"(?ms)^dependencies\s*=\s*\[(.*?)\]", metadata)
if dependencies is None:
    sys.exit("no dependencies in generator metadata")

for dependency in re.findall(r"""["']([^"']+)["']""", dependencies.group(1)):
    print(dependency)
PY

dependency_list=()
while IFS= read -r dependency; do
    dependency_list+=("${dependency}")
done <"${deps_file}"
if [ "${#dependency_list[@]}" -eq 0 ]; then
    echo "no dependencies found in generator metadata" >&2
    exit 1
fi

"${python}" -m pip install --quiet --disable-pip-version-check \
    "${dependency_list[@]}"

"${python}" "${generator}" "${repo_root}/Cargo.lock" -o "${output}"

# The generator writes no trailing newline; keep the repo's EOF newline so the
# end-of-file-fixer hook doesn't rewrite the file later.
if [ -n "$(tail -c 1 "${output}")" ]; then
    printf '\n' >>"${output}"
fi
