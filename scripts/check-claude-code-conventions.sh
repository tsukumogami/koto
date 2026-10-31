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
#   scripts/check-claude-code-conventions.sh [--version <MAJOR.MINOR.PATCH>] [--alter <slug>]
#   scripts/check-claude-code-conventions.sh --evidence <file> [--alter <slug>]
#   scripts/check-claude-code-conventions.sh --self-test
#
# With no arguments it checks the release recorded in
# tests/fixtures/native-workflows/claude-code-release. --version checks the
# named release instead, and the recorded-release file isn't read. --evidence
# scans a local Claude Code executable (one already extracted from a package,
# say) with no download; it replaces --version.
#
# --alter <slug> applies that convention's demonstration mutation (a
# same-length rename, listed in the expectations file) to an in-memory copy of
# the evidence first, so the run shows the guard failing on that convention by
# name. The slugs are file-shape, directory, session-variable and hook-input.
# It combines with either source.
#
# --self-test takes no other flag and needs no network: it checks the probes
# against synthetic evidence built from the expectations file alone, then
# checks that each convention's demonstration mutation makes that convention
# fail.
#
# Output: `checking Claude Code <version>` (or `checking Claude Code executable
# <file>`), then one FAIL line per failing probe, then `<convention>: ok` or
# `<convention>: FAIL` for each convention.
#
# Exit statuses:
#   0   every convention holds (--self-test: every mutation was caught)
#   1   at least one convention failed (--self-test: a mutation went uncaught)
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
VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+$'

usage() {
  echo "usage: check-claude-code-conventions.sh [--version <MAJOR.MINOR.PATCH> | --evidence <file>] [--alter <slug>]" >&2
  echo "       check-claude-code-conventions.sh --self-test" >&2
  exit 64
}

alter=""
version=""
evidence=""
self_test=0
while [ $# -gt 0 ]; do
  case "$1" in
    --alter | --version | --evidence)
      { [ $# -ge 2 ] && [ -n "$2" ]; } || usage
      case "$1" in
        --alter) alter=$2 ;;
        --version) version=$2 ;;
        --evidence) evidence=$2 ;;
      esac
      shift 2
      ;;
    --alter=* | --version=* | --evidence=*)
      value=${1#*=}
      [ -n "$value" ] || usage
      case "$1" in
        --alter=*) alter=$value ;;
        --version=*) version=$value ;;
        --evidence=*) evidence=$value ;;
      esac
      shift
      ;;
    --self-test)
      self_test=1
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
if [ "$self_test" -eq 1 ] && [ -n "$alter$version$evidence" ]; then
  usage
fi
if [ -n "$version" ] && [ -n "$evidence" ]; then
  usage
fi
# The version becomes part of an npm package spec, so it is held to
# MAJOR.MINOR.PATCH before anything uses it.
if [ -n "$version" ] && ! [[ $version =~ $VERSION_RE ]]; then
  echo "check-claude-code-conventions: --version takes a MAJOR.MINOR.PATCH version" >&2
  exit 64
fi
# --evidence is named relative to where the guard was started, not the
# repository root it moves to below.
if [ -n "$evidence" ] && [[ $evidence != /* ]]; then
  evidence="$PWD/$evidence"
fi

# Relative paths from here on, so every message names the file the way the
# repository does.
cd "$REPO_ROOT"

if [ "$self_test" -eq 1 ]; then
  status=0
  python3 -I "$HELPER" self-test "$EXPECTATIONS" || status=$?
  exit "$status"
fi

# The recorded release: one line holding MAJOR.MINOR.PATCH. $(<file) drops
# trailing newlines; anything else on the line, or a second line, fails the
# match, since =~ anchors ^ and $ to the whole string. Read only when no other
# source was named, so a broken file never blocks checking another release.
if [ -z "$version" ] && [ -z "$evidence" ]; then
  if [ ! -f "$RELEASE_FILE" ]; then
    echo "check-claude-code-conventions: $RELEASE_FILE is missing" >&2
    exit 3
  fi
  version=$(<"$RELEASE_FILE")
  if ! [[ $version =~ $VERSION_RE ]]; then
    echo "check-claude-code-conventions: $RELEASE_FILE must hold one MAJOR.MINOR.PATCH version" >&2
    exit 3
  fi
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

if [ -n "$evidence" ]; then
  echo "checking Claude Code executable $evidence"
  if [ ! -f "$evidence" ]; then
    echo "could not obtain Claude Code executable $evidence: not a regular file" >&2
    exit 2
  fi
  status=0
  python3 -I "$HELPER" scan "$EXPECTATIONS" "$evidence" ${alter_args[@]+"${alter_args[@]}"} || status=$?
  exit "$status"
fi

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
# output stays in the work directory and is never echoed. An unpublished
# version, a registry error and no network all land here, and the message
# names the release and no convention.
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
