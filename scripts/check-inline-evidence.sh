#!/usr/bin/env bash
#
# check-inline-evidence.sh - refuse a koto command in the koto-skills plugin
# that passes --with-data an inline value instead of a file
#
# Agents run the koto commands a skill shows them, often behind a prefix that
# strips a token (`env -u GH_TOKEN koto ...`, `GH_TOKEN= koto ...`). Claude
# Code's worktree-isolation check reads a quoted argument after `env` as the
# program `env` runs, so
#     env -u GH_TOKEN koto next issue_1 --with-data '{"mode": "issue_backed"}'
# is refused ("what it runs cannot be shown not to be git"), and with
# `GH_TOKEN=` it is refused whenever the JSON holds certain words. The file
# form, `--with-data @<file>`, gives the check nothing quoted to misread and
# hands koto the same bytes.
#
# Skill text shows bare `koto` commands and the agent adds the prefix itself,
# so this check doesn't wait for a prefix to be written: a line that names
# `koto` and gives `--with-data` (or `--with-data=`) a value starting with a
# quote or `$` is refused, with or without anything in front of `koto`. That
# covers '{...}', "{\"...\"}", "$DATA" and "$(jq ...)". A value starting with
# `@` passes, and so does a mention of the flag with no value after it.
#
# What is scanned is the text an agent reads and copies commands from: every
# *.md and *.mdc under plugins/ (skills, their references and templates, and
# the Cursor rule), outside evals/, whose fixtures are data. docs/ and the
# README are not; no skill loads them.
#
# Two kinds of line are let through on purpose:
#   - `koto request ...`: the request verbs read --with-data only as inline
#     JSON (src/cli/request.rs parses it with parse_json_flag and never
#     resolves an `@` path), so there is no file form to show. Drop this
#     exemption, and the notes saying so in koto-user's SKILL.md and
#     command-reference.md, once those verbs read `@<file>` (koto#404).
#   - `"invocation": "koto ..."`: a response example quoting the retry
#     command koto itself prints in `reserved_actions` (src/cli/retry.rs).
#     The skill text around it tells the agent to submit that payload from a
#     file (koto#405).
#
# Not seen, and left to review: a command split over lines with `\` where
# `koto` and `--with-data` land on different lines, inline JSON handed to
# koto through a variable set on an earlier line, and an inline `koto next`
# on a line that also names `koto request` (the exemption covers the whole
# line).
#
# Usage:
#   scripts/check-inline-evidence.sh            the repository's own tree
#   scripts/check-inline-evidence.sh ROOT       the same paths under ROOT
#
# Each site is printed as path:line: text. Exit 0 when there is none, 1 when
# there is one or more, 2 on a usage error or when the scan itself failed.
# On the repository's own tree a missing plugins/ or an empty file list is a
# failed scan (exit 2), so moving the plugin can't make the check pass having
# read nothing; under a ROOT given as an argument it is a clean exit 0, which
# the fixture tests rely on.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "${1:-}" in
  -h|--help) sed -n '2,/^$/{s/^# \{0,1\}//;p;}' "$0"; exit 0 ;;
esac

if [ "$#" -gt 1 ]; then
  echo "usage: check-inline-evidence.sh [ROOT]" >&2
  exit 2
fi
OWN_TREE=1
[ "$#" -eq 1 ] && OWN_TREE=0
ROOT="${1:-$SCRIPT_DIR/..}"
if [ ! -d "$ROOT" ]; then
  echo "check-inline-evidence.sh: no such directory: $ROOT" >&2
  exit 2
fi
ROOT="$(cd "$ROOT" && pwd)"

# nothing_to_scan <what>: a failed scan on the repository's own tree, a clean
# pass under a fixture ROOT (see the header).
nothing_to_scan() {
  if [ "$OWN_TREE" -eq 1 ]; then
    echo "check-inline-evidence.sh: $1 under $ROOT; nothing was scanned" >&2
    exit 2
  fi
  exit 0
}

[ -d "$ROOT/plugins" ] || nothing_to_scan "no plugins/ directory"

if ! LIST=$(find "$ROOT/plugins" -type f \( -name '*.md' -o -name '*.mdc' \) -not -path '*/evals/*' | LC_ALL=C sort); then
  echo "check-inline-evidence.sh: listing $ROOT/plugins failed" >&2
  exit 2
fi
[ -n "$LIST" ] || nothing_to_scan "no .md or .mdc files in plugins/"

# The single quote reaches awk as a variable, so the program needs no escape
# for it that one awk reads and another doesn't.
if ! SITES=$(printf '%s\n' "$LIST" | tr '\n' '\0' | xargs -0 awk -v q="'" '
  BEGIN { inline = "--with-data([ \t]+|=)[" q "\"$]" }
  /(^|[^A-Za-z0-9_-])koto[ \t]/ && $0 ~ inline {
    if ($0 ~ /(^|[^A-Za-z0-9_-])koto[ \t]+request[ \t]/) next
    if ($0 ~ /"invocation":[ \t]*"koto[ \t]/) next
    line = $0
    sub(/^[ \t]+/, "", line)
    printf "%s:%d: %s\n", FILENAME, FNR, line
  }'); then
  echo "check-inline-evidence.sh: scanning $ROOT/plugins failed" >&2
  exit 2
fi

if [ -n "$SITES" ]; then
  printf '%s\n' "$SITES" | sed "s|^$ROOT/||"
  n=$(printf '%s\n' "$SITES" | wc -l | tr -d ' ')
  echo "check-inline-evidence: $n site(s) pass koto evidence inline; write it to a file and pass --with-data @<file> (see this script's header)" >&2
  exit 1
fi
exit 0
