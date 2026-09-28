#!/usr/bin/env bash
# Checks that koto v0.14.1 stays compatible with what the failure-reporting
# work changes: compiled templates and the session log.
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/failure-reporting-v0_14_1.sh [--self-test]
#
# Templates: every fixture template the Rust baseline test pins (the
# TEMPLATE_DIRS below mirror tests/compat_baseline_test.rs) must compile to
# the same cache path, and so the same template hash, under v0.14.1 and the
# new build.
#
# Event log: the new build drives the session in fixtures/failure-reporting.md
# through a failing then passing command gate, a failing then succeeding
# default_action, a context-exists gate satisfied by `koto context add`, and a
# `koto context get`. The log must hold every event listed in EVENT_CHECKS.
# v0.14.1 must then run `koto status`, `koto next` and `koto context get` on
# that session, exit 0, and report the state the new build reports.
#
# To cover a new event kind, make the session emit it (SESSION STEPS below)
# and add a line to EVENT_CHECKS. The self-test picks the new line up.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N     delete the log lines matching EVENT_CHECKS[N] (one
#                    mutation per entry) before the checks read the log
#   drop-transition  delete the last `transitioned` event, so v0.14.1 sees a
#                    different current state than the new build reported
#   template-drift   the new build compiles an edited copy of one template
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash and jq. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIXTURE="$SCRIPT_DIR/fixtures/failure-reporting.md"
FLOOR_VERSION="0.14.1"

# Fixture template directories, relative to the repository root.
TEMPLATE_DIRS=(
  "test/functional/fixtures/templates"
  "tests/fixtures/template-hash"
  "tests/fixtures/decider"
  "test/compat/fixtures"
)

# Events the exercised session must leave in its log, as "label|jq filter".
# Each filter is applied to one log line. Append new kinds here.
EVENT_CHECKS=(
  'failing command gate|.type? == "gate_evaluated" and .payload.gate == "tests" and .payload.outcome == "failed"'
  'passing command gate|.type? == "gate_evaluated" and .payload.gate == "tests" and .payload.outcome == "passed"'
  'failing context gate|.type? == "gate_evaluated" and .payload.gate == "note" and .payload.outcome == "failed"'
  'failing default_action|.type? == "default_action_executed" and .payload.exit_code != 0'
  'succeeding default_action|.type? == "default_action_executed" and .payload.exit_code == 0'
  'context add|.type? == "context_added" and .payload.key == "review_note"'
)

REVIEW_NOTE="compat review note: looks good"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

mutations() {
  local i
  for i in "${!EVENT_CHECKS[@]}"; do
    echo "drop-event:$i"
  done
  echo drop-transition
  echo template-drift
}

# --- self-test ---------------------------------------------------------------

if [ "${1:-}" = "--self-test" ]; then
  echo "== self-test: clean run (must pass)"
  env -u COMPAT_MUTATION "$0" >/dev/null || fail "self-test: the clean run failed"
  pass "self-test: clean run passed"
  for m in $(mutations); do
    echo "== self-test: COMPAT_MUTATION=$m (must fail)"
    if COMPAT_MUTATION="$m" "$0" >/dev/null; then
      fail "self-test: mutation '$m' did not make the script fail"
    fi
    pass "self-test: mutation '$m' was caught"
  done
  exit 0
elif [ $# -gt 0 ]; then
  fail "usage: $0 [--self-test]"
fi

MUTATION="${COMPAT_MUTATION:-}"
if [ -n "$MUTATION" ]; then
  mutations | grep -qxF "$MUTATION" \
    || fail "unknown COMPAT_MUTATION '$MUTATION' (known: $(mutations | tr '\n' ' '))"
fi

# --- inputs and tools --------------------------------------------------------

for var in KOTO_FLOOR_BIN KOTO_NEW_BIN; do
  path="${!var:-}"
  [ -n "$path" ] || fail "$var is not set"
  case "$path" in
    /*) ;;
    *) fail "$var must be an absolute path, got '$path'" ;;
  esac
  [ -x "$path" ] || fail "$var ($path) is not an executable file"
done
[ "$KOTO_FLOOR_BIN" != "$KOTO_NEW_BIN" ] || fail "KOTO_FLOOR_BIN and KOTO_NEW_BIN are the same file"

command -v jq >/dev/null 2>&1 || fail "jq is not on PATH"
[ -f "$FIXTURE" ] || fail "fixture $FIXTURE is missing"

floor_version="$("$KOTO_FLOOR_BIN" version 2>&1)" || fail "floor version: '$KOTO_FLOOR_BIN version' failed"
case "$floor_version" in
  "koto $FLOOR_VERSION "* | "koto $FLOOR_VERSION") pass "floor binary reports $floor_version" ;;
  *) fail "floor version: expected koto $FLOOR_VERSION, got '$floor_version'" ;;
esac
new_version="$("$KOTO_NEW_BIN" version 2>&1)" || fail "new version: '$KOTO_NEW_BIN version' failed"
echo "new build: $new_version"

# --- scratch space -----------------------------------------------------------

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT

new_dir() {
  mktemp -d "$SCRATCH/$1.XXXXXX"
}

# koto_in HOME WORKDIR BIN ARGS...: run BIN in WORKDIR with its own HOME,
# no decider settings and no sessions-base override.
koto_in() {
  local home="$1" work="$2"
  shift 2
  (cd "$work" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$home" "$@")
}

# --- compiled templates ------------------------------------------------------

COMPILE_HOME="$(new_dir compile-home)"
COMPILE_WORK="$(new_dir compile-work)"
compiled=0
drifted=""
for dir in "${TEMPLATE_DIRS[@]}"; do
  [ -d "$REPO_ROOT/$dir" ] || fail "templates: directory $dir is missing"
  for src in "$REPO_ROOT/$dir"/*.md; do
    [ -f "$src" ] || continue
    new_src="$src"
    if [ "$MUTATION" = "template-drift" ] && [ -z "$drifted" ]; then
      drifted="$src"
      new_src="$(new_dir drift)/$(basename "$src")"
      { cat "$src"; printf '\nAn extra line only the new build sees.\n'; } >"$new_src"
      echo "MUTATION: the new build compiles an edited copy of $dir/$(basename "$src")"
    fi
    # Non-strict, as `koto init` compiles; stdout is the cache path, whose
    # file name is the template hash.
    floor_out="$(koto_in "$COMPILE_HOME" "$COMPILE_WORK" "$KOTO_FLOOR_BIN" \
      template compile --allow-legacy-gates "$src" 2>"$SCRATCH/compile.err")" \
      || fail "templates: v$FLOOR_VERSION rejected $dir/$(basename "$src"): $floor_out $(cat "$SCRATCH/compile.err")"
    new_out="$(koto_in "$COMPILE_HOME" "$COMPILE_WORK" "$KOTO_NEW_BIN" \
      template compile --allow-legacy-gates "$new_src" 2>"$SCRATCH/compile.err")" \
      || fail "templates: the new build rejected $dir/$(basename "$src"): $new_out $(cat "$SCRATCH/compile.err")"
    floor_hash="$(basename "$floor_out" .json)"
    new_hash="$(basename "$new_out" .json)"
    [ -n "$floor_hash" ] || fail "templates: v$FLOOR_VERSION printed no cache path for $dir/$(basename "$src")"
    [ "$floor_hash" = "$new_hash" ] \
      || fail "templates: $dir/$(basename "$src") compiles to $new_hash, v$FLOOR_VERSION to $floor_hash"
    compiled=$((compiled + 1))
  done
done
[ "$compiled" -gt 0 ] || fail "templates: no fixture template was found"
pass "all $compiled fixture templates compile to the same template hash under v$FLOOR_VERSION and the new build"

# --- session steps (new build) -----------------------------------------------

HOME_DIR="$(new_dir session-home)"
WORK_DIR="$(new_dir session-work)"
TEMPLATE="$WORK_DIR/failure-reporting.md"
cp "$FIXTURE" "$TEMPLATE"

new_koto() {
  koto_in "$HOME_DIR" "$WORK_DIR" "$KOTO_NEW_BIN" "$@"
}

# new_next LABEL WANT-STATE WANT-ACTION [ARGS...]: one `koto next` by the new
# build, which must land in WANT-STATE with WANT-ACTION.
new_next() {
  local label="$1" want_state="$2" want_action="$3" out got
  shift 3
  out="$(new_koto next wf "$@" 2>&1)" || fail "session: '$label' exited non-zero: $out"
  got="$(printf '%s' "$out" | jq -er '.state + " " + .action')" \
    || fail "session: '$label' printed no state and action: $out"
  [ "$got" = "$want_state $want_action" ] \
    || fail "session: '$label' expected '$want_state $want_action', got: $out"
  pass "session: $label -> $got"
}

new_koto init wf --template "$TEMPLATE" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "session: koto init failed: $(cat "$SCRATCH/init.err")"

# Command gate: fails (prints to both streams, exits 1), then passes.
new_next "failing command gate" build gate_blocked
touch "$WORK_DIR/ready"
# The gate passes and the tick moves on to setup, whose default_action fails.
new_next "passing command gate, failing default_action" setup gate_blocked
touch "$WORK_DIR/setup-ok"
# The action succeeds and the tick stops at review's unmet context gate.
new_next "succeeding default_action, failing context gate" review gate_blocked

# Context: add the key the gate waits on, read it back.
printf '%s' "$REVIEW_NOTE" | new_koto context add wf review_note \
  || fail "session: koto context add failed"
got_note="$(new_koto context get wf review_note)" || fail "session: koto context get failed"
[ "$got_note" = "$REVIEW_NOTE" ] || fail "session: koto context get returned '$got_note'"
pass "session: koto context add and get round-trip review_note"

new_next "context gate passes" wrapup evidence_required

new_status="$(new_koto status wf 2>&1)" || fail "session: new build koto status failed: $new_status"
NEW_STATE="$(printf '%s' "$new_status" | jq -er '.current_state')" \
  || fail "session: new build koto status has no current_state: $new_status"
pass "the new build reports current state $NEW_STATE"

LOG_FILE="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "session: no koto-wf.state.jsonl was written"

# --- log mutations (self-test only) ------------------------------------------

# drop_lines FILTER: delete every log line the jq filter selects, leaving the
# bytes of every other line as they are.
drop_lines() {
  local filter="$1" tmp="$LOG_FILE.mut" line
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
      continue
    fi
    printf '%s\n' "$line" >>"$tmp"
  done <"$LOG_FILE"
  mv "$tmp" "$LOG_FILE"
}

case "$MUTATION" in
  drop-event:*)
    entry="${EVENT_CHECKS[${MUTATION#drop-event:}]}"
    echo "MUTATION: deleting log lines for '${entry%%|*}'"
    drop_lines "${entry#*|}"
    ;;
  drop-transition)
    last_seq="$(jq -s '[.[] | select(.type? == "transitioned") | .seq] | max' "$LOG_FILE")"
    [ "$last_seq" != "null" ] || fail "self-test: the log holds no transitioned event to drop"
    echo "MUTATION: deleting the transitioned event with seq $last_seq"
    drop_lines ".type? == \"transitioned\" and .seq == $last_seq"
    ;;
esac

# --- event coverage ----------------------------------------------------------

for entry in "${EVENT_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$LOG_FILE")" \
    || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: the log holds no event for '$label'"
  pass "events: $label ($n)"
done

# --- v0.14.1 reads the log ---------------------------------------------------

floor_koto() {
  koto_in "$HOME_DIR" "$WORK_DIR" "$KOTO_FLOOR_BIN" "$@"
}

# reject_errors WHAT OUTPUT: the output may not mention a parse or corruption
# problem even when the command exited 0.
reject_errors() {
  if printf '%s' "$2" | grep -Eiq 'corrupt|pars(e|ing)|mismatch|unknown event'; then
    fail "floor: $1 reported an error: $2"
  fi
}

out="$(floor_koto status wf 2>&1)" || fail "floor: koto status exited non-zero: $out"
reject_errors "koto status" "$out"
state="$(printf '%s' "$out" | jq -er '.current_state')" \
  || fail "floor: koto status has no current_state: $out"
[ "$state" = "$NEW_STATE" ] || fail "floor: koto status says '$state', the new build said '$NEW_STATE'"
pass "v$FLOOR_VERSION koto status reads the log (state $state)"

out="$(floor_koto next wf --no-cleanup 2>&1)" || fail "floor: koto next exited non-zero: $out"
reject_errors "koto next" "$out"
printf '%s' "$out" | jq -e '.error == null' >/dev/null \
  || fail "floor: koto next returned an error: $out"
state="$(printf '%s' "$out" | jq -er '.state')" || fail "floor: koto next has no state: $out"
[ "$state" = "$NEW_STATE" ] || fail "floor: koto next says '$state', the new build said '$NEW_STATE'"
pass "v$FLOOR_VERSION koto next reads the log (state $state)"

out="$(floor_koto context get wf review_note 2>&1)" || fail "floor: koto context get exited non-zero: $out"
[ "$out" = "$REVIEW_NOTE" ] || fail "floor: koto context get returned '$out'"
pass "v$FLOOR_VERSION koto context get returns the note the new build stored"

pass "all checks passed"
