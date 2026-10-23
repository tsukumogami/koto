#!/usr/bin/env bash
#
# check-inline-evidence_test.sh - the inline-evidence check, on the tree and
# on fixtures
#
# The first case runs the check against this repository, so one CI step
# covers both the scan and its own regressions. The rest build a throwaway
# tree under a mktemp directory and assert what the check refuses and what it
# lets through.
#
# Usage: scripts/check-inline-evidence_test.sh
#
# Exit 0 when every case passes, 1 otherwise.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/check-inline-evidence.sh"

PASS_COUNT=0
FAIL_COUNT=0
pass() { echo "PASS: $*"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { echo "FAIL: $*"; FAIL_COUNT=$((FAIL_COUNT + 1)); }

WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/inline-evidence.XXXXXX") || { echo "mktemp failed" >&2; exit 1; }
trap 'rm -rf "$WORKDIR"' EXIT

# --- the tree -----------------------------------------------------------------

out=$(bash "$CHECK" 2>&1)
rc=$?
if [ "$rc" -eq 0 ]; then
  pass "the koto-skills plugin passes no koto evidence inline"
else
  fail "the koto-skills plugin has inline koto evidence (exit $rc):
$out"
fi

# --- fixtures -----------------------------------------------------------------

# run_on <name> <relative path> <line>: a fresh tree holding one file with one
# line; sets OUT and RC from the check over it.
OUT=""
RC=0
run_on() {
  local root="$WORKDIR/$1"
  mkdir -p "$root/$(dirname "$2")"
  printf '%s\n' "$3" > "$root/$2"
  OUT=$(bash "$CHECK" "$root" 2>/dev/null)
  RC=$?
}

refused() { # <name> <path> <line>
  run_on "$1" "$2" "$3"
  if [ "$RC" -eq 1 ] && grep -q -F "$2:1:" <<<"$OUT"; then
    pass "$1: refused, naming $2:1"
  else
    fail "$1: expected exit 1 naming $2:1, got exit $RC: [$OUT]"
  fi
}

allowed() { # <name> <path> <line>
  run_on "$1" "$2" "$3"
  if [ "$RC" -eq 0 ] && [ -z "$OUT" ]; then
    pass "$1: let through"
  else
    fail "$1: expected exit 0 and no sites, got exit $RC: [$OUT]"
  fi
}

S=plugins/p/skills/x

refused single-quoted-json "$S/SKILL.md" \
  "koto next <name> --with-data '{\"field_name\": \"value\"}'"
refused escaped-double-quoted-json "$S/references/r.md" \
  'koto next <name> --with-data "{\"outcome\": \"success\"}"'
refused env-prefix "$S/SKILL.md" \
  "env -u GH_TOKEN koto next issue_1 --with-data '{\"mode\": \"issue_backed\"}' --no-cleanup"
refused empty-assignment-prefix "$S/SKILL.md" \
  "GH_TOKEN= koto next issue_1 --with-data '{\"mode\": \"issue_backed\"}' --no-cleanup"
refused decisions-record "$S/SKILL.md" \
  "koto decisions record <name> --with-data '{\"choice\": \"a\", \"rationale\": \"b\"}'"
refused overrides-record "$S/references/r.md" \
  "koto overrides record s --gate g --rationale \"r\" --with-data '{\"exit_code\": 0}'"
refused equals-form "$S/references/r.md" \
  "koto next s --with-data='{\"k\": \"v\"}'"
refused variable-value "$S/koto-templates/t.md" \
  'koto next s --with-data "$DATA"'
refused command-substitution "$S/SKILL.md" \
  'koto next s --with-data "$(jq -n -c "{k: 1}")"'
refused shell-loop-line "$S/SKILL.md" \
  "      result=\$(koto next task-42 --with-data '{\"decision\": \"proceed\"}')"
refused cursor-rule plugins/p/.cursor/rules/koto.mdc \
  "koto next <session> --with-data '{\"mode\": \"issue_backed\"}'"
refused request-resolve "$S/SKILL.md" \
  "koto request resolve <request-id> <leg> --with-data '{\"status\":\"success\"}'"
refused request-create "$S/references/r.md" \
  "koto request create --with-data '{\"legs\":[]}' --requested-by a --coordinator-of-record a"
refused next-beside-request "$S/SKILL.md" \
  "koto request get <id>; koto next s --with-data '{\"k\": \"v\"}'"

allowed file-form "$S/SKILL.md" \
  'koto next <name> --with-data @"$EV/evidence.json"'
allowed file-form-bare "$S/SKILL.md" \
  'koto decisions record <name> --with-data @<file>'
allowed prose-mention "$S/SKILL.md" \
  'Call `koto next <name>` without `--with-data` to re-read the schema.'
allowed json-written-to-file "$S/SKILL.md" \
  "printf '%s\\n' '{\"field_name\": \"value\"}' > \"\$EV/evidence.json\""
allowed request-file-form "$S/SKILL.md" \
  'koto request resolve <request-id> <leg> --with-data @<file>'
allowed reserved-action-invocation "$S/references/r.md" \
  "      \"invocation\": \"koto next 'coord' --with-data '{\\\"retry_failed\\\":{}}'\""
allowed not-koto "$S/SKILL.md" \
  "some-tool --with-data '{\"k\": \"v\"}'"
allowed evals-fixture "$S/evals/fixtures/f.md" \
  "koto next s --with-data '{\"k\": \"v\"}'"
allowed outside-plugins docs/guides/g.md \
  "koto next s --with-data '{\"k\": \"v\"}'"
allowed not-markdown "$S/scripts/s.sh" \
  "koto next s --with-data '{\"k\": \"v\"}'"

# Every site is reported, not only the first.
root="$WORKDIR/two-sites"
mkdir -p "$root/$S"
printf '%s\n' \
  "koto next a --with-data '{\"k\": 1}'" \
  "prose" \
  "koto next b --with-data '{\"k\": 2}'" > "$root/$S/SKILL.md"
OUT=$(bash "$CHECK" "$root" 2>/dev/null)
RC=$?
if [ "$RC" -eq 1 ] && grep -q -F "$S/SKILL.md:1:" <<<"$OUT" && grep -q -F "$S/SKILL.md:3:" <<<"$OUT"; then
  pass "two sites in one file are both reported"
else
  fail "expected both sites reported, got exit $RC: [$OUT]"
fi

# Run on its own tree (no ROOT argument), a copy of the check that finds no
# plugins/ beside it has scanned nothing, which is a failed scan, not a pass.
mkdir -p "$WORKDIR/moved/scripts"
cp "$CHECK" "$WORKDIR/moved/scripts/check-inline-evidence.sh"
bash "$WORKDIR/moved/scripts/check-inline-evidence.sh" >/dev/null 2>&1
RC=$?
if [ "$RC" -eq 2 ]; then pass "its own tree with no plugins/ exits 2"; else fail "its own tree with no plugins/ exited $RC, expected 2"; fi

# A root that isn't a directory, or more than one argument, is a usage error.
bash "$CHECK" "$WORKDIR/no-such-dir" >/dev/null 2>&1
RC=$?
if [ "$RC" -eq 2 ]; then pass "a missing root exits 2"; else fail "a missing root exited $RC, expected 2"; fi
bash "$CHECK" "$WORKDIR" "$WORKDIR" >/dev/null 2>&1
RC=$?
if [ "$RC" -eq 2 ]; then pass "two arguments exit 2"; else fail "two arguments exited $RC, expected 2"; fi

echo
echo "Results: $PASS_COUNT passed, $FAIL_COUNT failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
