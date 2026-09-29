#!/usr/bin/env bash
# Checks that the polling-gate and clear-on-entry code stays free of any forge
# and of credentials (DESIGN-koto-ci-wait-stale-keys.md): koto runs the
# command a template names and reads only its exit status and output, so
# nothing in this code may name a forge or reach for a token.
#
#   scripts/check-poll-forge-neutral.sh [--self-test]
#
# The checked files are the two modules the features added. A match of any
# pattern below, case-insensitively, fails the check and prints the line.
# --self-test copies the files, injects a forge reference into the copy, and
# passes only if the clean copy passes and the injected one fails.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FILES=(
  "src/engine/poll.rs"
  "src/engine/clear_on_entry.rs"
)
# Forge names, forge API hosts and CLIs, and credential lookups.
PATTERN='github|gitlab|bitbucket|api\.github|\bgh +(pr|api|run)|\btoken\b|GH_TOKEN|GITHUB_TOKEN|credential|password|env::var'

check() {
  local root="$1" hits=0 f
  for f in "${FILES[@]}"; do
    [ -f "$root/$f" ] || { echo "FAIL: $f is missing" >&2; return 1; }
    if grep -nEi "$PATTERN" "$root/$f"; then
      hits=1
    fi
  done
  if [ "$hits" -ne 0 ]; then
    echo "FAIL: the polling or clearing code names a forge or reads a credential (lines above)" >&2
    return 1
  fi
  echo "PASS: ${FILES[*]} name no forge and read no credential"
}

if [ "${1:-}" = "--self-test" ]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/forge-neutral.XXXXXX")"
  trap 'rm -rf "$scratch"' EXIT
  for f in "${FILES[@]}"; do
    mkdir -p "$scratch/$(dirname "$f")"
    cp "$REPO_ROOT/$f" "$scratch/$f"
  done
  check "$scratch" >/dev/null || { echo "FAIL: self-test: the clean copy failed" >&2; exit 1; }
  echo 'const API: &str = "https://api.github.com/repos";' >>"$scratch/${FILES[0]}"
  if check "$scratch" >/dev/null 2>&1; then
    echo "FAIL: self-test: an injected forge reference was not caught" >&2
    exit 1
  fi
  echo "PASS: self-test: the clean copy passes and an injected forge reference fails"
  exit 0
elif [ $# -gt 0 ]; then
  echo "usage: $0 [--self-test]" >&2
  exit 2
fi

check "$REPO_ROOT"
