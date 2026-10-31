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
#   scripts/check-claude-code-conventions.sh [--version <MAJOR.MINOR.PATCH>|latest] [--cache-dir <dir>] [--alter <slug>]
#   scripts/check-claude-code-conventions.sh --evidence <file> [--alter <slug>]
#   scripts/check-claude-code-conventions.sh --self-test
#
# With no arguments it checks the release recorded in
# tests/fixtures/native-workflows/claude-code-release. --version checks the
# named release instead, and the recorded-release file isn't read. --version
# latest asks the registry for the newest release first and checks that one; the
# version it resolved to is the one every later step and the output name.
#
# --evidence scans a local Claude Code executable (one already extracted from a
# package, say) with no download; it replaces --version.
#
# --alter <slug> applies that convention's demonstration mutation (a
# same-length rename, listed in the expectations file) to an in-memory copy of
# the evidence first, so the run shows the guard failing on that convention by
# name. The slugs are file-shape, directory, session-variable and hook-input.
# It combines with either source.
#
# --cache-dir <dir> keeps the downloaded tarball in <dir>, an existing directory
# outside the repository, under the name npm pack gives it, and nothing else:
# the executable is still extracted into the temporary directory. A cached
# tarball is reused only when its sha512 matches the dist.integrity the registry
# reports for the release; one that doesn't match is named, deleted and fetched
# again. The check reads a private copy of the tarball, hashed before it is
# extracted, so a run that reuses the cached file only reads it.
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
  echo "usage: check-claude-code-conventions.sh [--version <MAJOR.MINOR.PATCH>|latest] [--cache-dir <dir>] [--alter <slug>]" >&2
  echo "       check-claude-code-conventions.sh --evidence <file> [--alter <slug>]" >&2
  echo "       check-claude-code-conventions.sh --self-test" >&2
  exit 64
}

alter=""
version=""
evidence=""
cache_dir=""
self_test=0
while [ $# -gt 0 ]; do
  case "$1" in
    --alter | --version | --evidence | --cache-dir)
      { [ $# -ge 2 ] && [ -n "$2" ]; } || usage
      case "$1" in
        --alter) alter=$2 ;;
        --version) version=$2 ;;
        --evidence) evidence=$2 ;;
        --cache-dir) cache_dir=$2 ;;
      esac
      shift 2
      ;;
    --alter=* | --version=* | --evidence=* | --cache-dir=*)
      value=${1#*=}
      [ -n "$value" ] || usage
      case "$1" in
        --alter=*) alter=$value ;;
        --version=*) version=$value ;;
        --evidence=*) evidence=$value ;;
        --cache-dir=*) cache_dir=$value ;;
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
if [ "$self_test" -eq 1 ] && [ -n "$alter$version$evidence$cache_dir" ]; then
  usage
fi
if [ -n "$evidence" ] && [ -n "$version$cache_dir" ]; then
  usage
fi
# The version becomes part of an npm package spec, so it is held to
# MAJOR.MINOR.PATCH (or the one word latest, resolved below to such a version)
# before anything uses it.
if [ -n "$version" ] && [ "$version" != latest ] && ! [[ $version =~ $VERSION_RE ]]; then
  echo "check-claude-code-conventions: --version takes a MAJOR.MINOR.PATCH version or latest" >&2
  exit 64
fi
# --evidence is named relative to where the guard was started, not the
# repository root it moves to below.
if [ -n "$evidence" ] && [[ $evidence != /* ]]; then
  evidence="$PWD/$evidence"
fi

# The cache directory must already exist and sit outside the repository, for
# the same reason as TMPDIR below: a package kept in the checkout could be
# committed. It is resolved to its physical path before the guard moves, so a
# relative name means the directory the guard was started in.
if [ -n "$cache_dir" ]; then
  if [ ! -d "$cache_dir" ]; then
    echo "check-claude-code-conventions: --cache-dir $cache_dir is not an existing directory" >&2
    exit 64
  fi
  cache_dir=$(cd "$cache_dir" && pwd -P)
  case "$cache_dir/" in
    "$(cd "$REPO_ROOT" && pwd -P)"/*)
      echo "check-claude-code-conventions: --cache-dir is inside the repository; point it elsewhere" >&2
      exit 64
      ;;
  esac
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

work=$(mktemp -d "${TMPDIR:-/tmp}/claude-code-conventions.XXXXXX")
cache_tmp=""
cleanup() {
  rm -rf "$work"
  if [ -n "$cache_tmp" ]; then
    rm -f "$cache_tmp"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
# A TMPDIR inside the checkout would put the package in the repository.
case "$(cd "$work" && pwd -P)/" in
  "$(pwd -P)"/*)
    echo "check-claude-code-conventions: TMPDIR is inside the repository; point it elsewhere" >&2
    exit 64
    ;;
esac
mkdir "$work/pack" "$work/npm"

# npm view, like npm pack below, runs from an empty directory so no project
# .npmrc applies; its answer is used only once it matches the pattern it must.
npm_view() {
  (cd "$work/npm" && npm view --silent "$@" 2>"$work/npm.err")
}

# latest is resolved once, here, and only the concrete version is used from
# now on: the header, the package spec and the cache all name the same release.
if [ "$version" = latest ]; then
  resolved=$(npm_view "$package" version) || resolved=""
  if ! [[ $resolved =~ $VERSION_RE ]]; then
    echo "could not obtain Claude Code latest: npm view $package version gave no MAJOR.MINOR.PATCH version" >&2
    exit 2
  fi
  version=$resolved
fi

echo "checking Claude Code $version"

# npm runs from the empty pack directory so no project .npmrc applies; its
# output stays in the work directory and is never echoed. An unpublished
# version, a registry error and no network all land here, and the message
# names the release and no convention.
fetch() {
  if ! (cd "$work/pack" && npm pack --ignore-scripts --silent "$package@$version" >"$work/npm.out" 2>"$work/npm.err"); then
    echo "could not obtain Claude Code $version: npm pack $package@$version failed" >&2
    exit 2
  fi
  local tarballs=("$work/pack"/*.tgz)
  if [ "${#tarballs[@]}" -ne 1 ] || [ ! -f "${tarballs[0]}" ]; then
    echo "could not obtain Claude Code $version: npm pack left no single tarball" >&2
    exit 2
  fi
  tarball=${tarballs[0]}
}

# Prints the file's sha512 integrity string. It runs in a command
# substitution, so a failure only empties the string, which never matches.
integrity_of() {
  python3 -I "$HELPER" integrity "$1"
}

tarball=""
if [ -n "$cache_dir" ]; then
  # The registry's record of the release decides whether a cached tarball is
  # the published one; a cache hit is never trusted on its name alone.
  want=$(npm_view "$package@$version" dist.integrity) || want=""
  if ! [[ $want =~ ^sha512-[A-Za-z0-9+/]+=*$ ]]; then
    echo "could not obtain Claude Code $version: npm view $package@$version dist.integrity gave no sha512 integrity" >&2
    exit 2
  fi
  # The name npm pack gives the tarball: the scope's @ dropped, / made -.
  cache_name=${package#@}
  cache_name="${cache_name//\//-}-$version.tgz"
  cached="$cache_dir/$cache_name"
  if [ -L "$cached" ]; then
    echo "cached tarball $cached is a symbolic link; fetching again"
    rm -f "$cached"
  elif [ -e "$cached" ] && [ ! -f "$cached" ]; then
    echo "check-claude-code-conventions: $cached is in the way and is not a regular file" >&2
    exit 64
  elif [ -f "$cached" ]; then
    # Hash and extract a private copy, so what is checked is exactly what was
    # verified and the cached file itself is only read.
    cp "$cached" "$work/cached.tgz"
    if [ "$(integrity_of "$work/cached.tgz")" = "$want" ]; then
      tarball=$work/cached.tgz
    else
      echo "cached tarball $cached failed its integrity check against $package@$version dist.integrity; fetching again"
      rm -f "$cached" "$work/cached.tgz"
    fi
  fi
  if [ -z "$tarball" ]; then
    fetch
    if [ "$(integrity_of "$tarball")" != "$want" ]; then
      echo "could not obtain Claude Code $version: the fetched tarball failed its integrity check" >&2
      exit 2
    fi
    # Written under a temporary name and renamed, so an interrupted run never
    # leaves a partial tarball under the cached name.
    cache_tmp="$cache_dir/.$cache_name.partial.$$"
    cp "$tarball" "$cache_tmp"
    mv -f "$cache_tmp" "$cached"
    cache_tmp=""
  fi
else
  fetch
fi

status=0
reason=$(python3 -I "$HELPER" extract "$tarball" "$member" "$work/evidence" "$MAX_MEMBER_BYTES" 2>&1) || status=$?
if [ "$status" -ne 0 ]; then
  if [ "$status" -eq 2 ]; then
    echo "could not obtain Claude Code $version: $reason" >&2
  else
    echo "$reason" >&2
  fi
  exit "$status"
fi
# The working copy of the tarball is no longer needed; free its space before
# the scan. A cached tarball stays where it is.
rm -f "$tarball"

status=0
python3 -I "$HELPER" scan "$EXPECTATIONS" "$work/evidence" ${alter_args[@]+"${alter_args[@]}"} || status=$?
exit "$status"
