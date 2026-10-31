#!/usr/bin/env bash
# Runs a command with none of koto's session, anchor or authority variables
# in its environment: KOTO_SESSIONS_BASE, every KOTO_SESSION_* variable,
# KOTO_ANCHOR, KOTO_DISPATCHER_ANCHOR and KOTO_AUTHORITY. That is the
# environment hosted CI gives a suite.
#
# Every command in .claude/shirabe-extensions/verification-map.json runs
# through this script. The koto driving a /work-on run sets
# KOTO_SESSIONS_BASE on every command its gate starts, and passes on the
# KOTO_SESSION_* values that chose its backend, so a suite started there
# would otherwise run against that run's store. A session started through
# a workspace manager can also export the anchor variables, and a test that
# inherits them reads the caller's anchor instead of its own fixture.
#
#   without-session-env.sh <command> [<arg>...]
#       removes the variables, then runs --check on the result.
#   without-session-env.sh --check <command> [<arg>...]
#       exits 1 naming each such variable that is set, without starting
#       <command>; runs <command> when none is.
#
# Names are read from the environment itself (env -0), not from the shell's
# variables, so a name the shell can't hold is still found and removed.
set -euo pipefail

usage() {
  echo "usage: without-session-env.sh [--check] <command> [<arg>...]" >&2
  exit 64
}

# Lists the names. The listing goes through a file so a failed env -0 stops
# the script instead of reading as an empty environment: under --check the
# failure exits non-zero and the command never starts.
session_names() {
  local listing entry
  listing=$(mktemp)
  if ! env -0 >"$listing"; then
    rm -f "$listing"
    echo "without-session-env: can't list the environment with env -0" >&2
    exit 70
  fi
  while IFS= read -r -d '' entry; do
    case "${entry%%=*}" in
      KOTO_SESSIONS_BASE | KOTO_SESSION_* | KOTO_ANCHOR | KOTO_DISPATCHER_ANCHOR | KOTO_AUTHORITY)
        printf '%s\n' "${entry%%=*}"
        ;;
    esac
  done <"$listing"
  rm -f "$listing"
}

if [ "${1:-}" = "--check" ]; then
  shift
  [ "$#" -gt 0 ] || usage
  left=$(session_names)
  if [ -n "$left" ]; then
    while IFS= read -r name; do
      echo "without-session-env: $name is set; not running $1" >&2
    done <<<"$left"
    exit 1
  fi
  exec "$@"
fi

[ "$#" -gt 0 ] || usage
unset_args=()
while IFS= read -r name; do
  unset_args+=(-u "$name")
done < <(session_names)
exec env ${unset_args[@]+"${unset_args[@]}"} "$BASH" "$0" --check "$@"
