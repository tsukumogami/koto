#!/usr/bin/env bash
# Checks a Claude Code release against the conventions koto's /workflows entry
# depends on: the workflows file shape, the per-session directory convention,
# the CLAUDE_CODE_SESSION_ID variable and the SessionStart hook's input fields.
# Claude Code doesn't publish any of them, so the evidence is the release
# itself: the executable in its Linux x64 npm package, whose embedded
# JavaScript keeps string literals and object keys.
#
# The package is someone else's code and is never run. npm fetches the tarball
# with install scripts disabled into a fresh temporary directory outside the
# checkout; scripts/claude_code_conventions.py, started with `python3 -I`,
# copies out only the one expected member (not executable, size-capped) and
# searches its bytes for the probes in
# tests/fixtures/native-workflows/claude-code-conventions.json. The temporary
# directory is removed on exit, whatever the outcome.
#
# Usage:
#   scripts/check-claude-code-conventions.sh [--alter <slug>]
#
# With no arguments it checks the release recorded in
# tests/fixtures/native-workflows/claude-code-release. --alter <slug> applies
# that convention's demonstration mutation (a same-length rename, listed in
# the expectations file) to an in-memory copy of the evidence first, so the
# run shows the guard failing on that convention by name. The slugs are
# file-shape, directory, session-variable and hook-input.
#
# Output: `checking Claude Code <version>`, then one FAIL line per failing
# probe, then `<convention>: ok` or `<convention>: FAIL` for each convention.
#
# Exit statuses:
#   0   every convention holds
#   1   at least one convention failed
#   2   the release could not be obtained (`could not obtain Claude Code ...`)
#   3   the recorded-release or expectations file is missing or malformed
#   64  usage
#   70  the probe helper failed unexpectedly
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RELEASE_FILE=tests/fixtures/native-workflows/claude-code-release
EXPECTATIONS=tests/fixtures/native-workflows/claude-code-conventions.json
HELPER=scripts/claude_code_conventions.py
# The 2.1.316 executable is 328 MB; the cap leaves room for growth while
# stopping a runaway member well before it fills a runner's disk.
MAX_MEMBER_BYTES=$((1024 * 1024 * 1024))

usage() {
  echo "usage: check-claude-code-conventions.sh [--alter <slug>]" >&2
  exit 64
}

alter=""
while [ $# -gt 0 ]; do
  case "$1" in
    --alter)
      [ $# -ge 2 ] || usage
      alter=$2
      shift 2
      ;;
    --alter=*)
      alter=${1#--alter=}
      shift
      ;;
    *)
      usage
      ;;
  esac
done
if [ -n "$alter" ] && ! [[ $alter =~ ^[a-z][a-z0-9-]*$ ]]; then
  usage
fi

# Relative paths from here on, so every message names the file the way the
# repository does.
cd "$REPO_ROOT"

# The recorded release: one line holding MAJOR.MINOR.PATCH. $(<file) drops
# trailing newlines; anything else on the line, or a second line, fails the
# match, since =~ anchors ^ and $ to the whole string.
if [ ! -f "$RELEASE_FILE" ]; then
  echo "check-claude-code-conventions: $RELEASE_FILE is missing" >&2
  exit 3
fi
version=$(<"$RELEASE_FILE")
if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "check-claude-code-conventions: $RELEASE_FILE must hold one MAJOR.MINOR.PATCH version" >&2
  exit 3
fi

# Validates the expectations file (exit 3) and the --alter slug (exit 64)
# before anything is downloaded, and reads the package name and member.
alter_args=()
if [ -n "$alter" ]; then
  alter_args=(--alter "$alter")
fi
status=0
fields=$(python3 -I "$HELPER" expectations "$EXPECTATIONS" ${alter_args[@]+"${alter_args[@]}"}) || status=$?
if [ "$status" -ne 0 ]; then
  exit "$status"
fi
package=$(printf '%s\n' "$fields" | sed -n 1p)
member=$(printf '%s\n' "$fields" | sed -n 2p)

echo "checking Claude Code $version"

work=$(mktemp -d "${TMPDIR:-/tmp}/claude-code-conventions.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT TERM
# A TMPDIR inside the checkout would put the package in the repository.
case "$(cd "$work" && pwd -P)/" in
  "$(pwd -P)"/*)
    echo "check-claude-code-conventions: TMPDIR is inside the repository; point it elsewhere" >&2
    exit 64
    ;;
esac
mkdir "$work/pack"

# npm runs from the empty pack directory so no project .npmrc applies; its
# output stays in the work directory and is never echoed.
if ! (cd "$work/pack" && npm pack --ignore-scripts --silent "$package@$version" >"$work/npm.out" 2>"$work/npm.err"); then
  echo "could not obtain Claude Code $version: npm pack $package@$version failed" >&2
  exit 2
fi
tarballs=("$work/pack"/*.tgz)
if [ "${#tarballs[@]}" -ne 1 ] || [ ! -f "${tarballs[0]}" ]; then
  echo "could not obtain Claude Code $version: npm pack left no single tarball" >&2
  exit 2
fi

status=0
reason=$(python3 -I "$HELPER" extract "${tarballs[0]}" "$member" "$work/evidence" "$MAX_MEMBER_BYTES" 2>&1) || status=$?
if [ "$status" -ne 0 ]; then
  if [ "$status" -eq 2 ]; then
    echo "could not obtain Claude Code $version: $reason" >&2
  else
    echo "$reason" >&2
  fi
  exit "$status"
fi
# The tarball is no longer needed; free its space before the scan.
rm -f "${tarballs[0]}"

status=0
python3 -I "$HELPER" scan "$EXPECTATIONS" "$work/evidence" ${alter_args[@]+"${alter_args[@]}"} || status=$?
exit "$status"
