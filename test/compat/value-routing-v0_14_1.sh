#!/usr/bin/env bash
# Checks that koto v0.14.1 reads a session log the new build writes while a
# template routes on variable values (DESIGN-koto-value-routing.md).
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/value-routing-v0_14_1.sh [--self-test]
#
# Session: the new build drives fixtures-value-routing/value-routing.md with
# MODE=auto and ARM=with-rule. `route` takes its value route on MODE,
# `arm_check` its skip_if value route on ARM, and the session stops at
# `work`. An attach then rebinds MODE to interactive, a note is written to
# the context store, and the note is submitted: `reroute` takes its value
# route on the new MODE, and the session stops at `review`, a state with no
# value routes. The log must hold every event in EVENT_CHECKS (the
# `vars_matched` record on each value-routed transition and the rebind's
# `previous` value), and the new build's `koto template validate-feed` must
# accept it against docs/reference/session-feed.md.
#
# v0.14.1 can't compile a template with a value route, so it is handed the
# session the new build created, stopped where no value route is pending: it
# must run `koto status` and `koto next` on it, exit 0 and report the state
# the new build reports, and return the new build's value from
# `koto context get`.
#
# Templates without value routes are covered by the failure-reporting job,
# which compiles every fixture under both builds, and by
# tests/compat_baseline_test.rs.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N       delete the log lines matching EVENT_CHECKS[N]
#   strip-field:K      delete payload field K from the events that carry it
#   drop-transition    delete the last `transitioned` event
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash and jq. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FEED_SPEC="$REPO_ROOT/docs/reference/session-feed.md"
FIXTURE="$SCRIPT_DIR/fixtures-value-routing/value-routing.md"
FLOOR_VERSION="0.14.1"
NOTE="notes from the work state"

# Events the session must leave in its log, as "label|jq filter".
EVENT_CHECKS=(
  'value route on MODE|.type? == "transitioned" and .payload.from == "route" and .payload.to == "arm_check" and .payload.vars_matched == {"MODE": "auto"}'
  'skip_if value route on ARM|.type? == "transitioned" and .payload.from == "arm_check" and .payload.condition_type == "skip_if" and .payload.vars_matched == {"ARM": "with-rule"}'
  'rebind with its old value|.type? == "variables_rebound" and .payload.variables == {"MODE": "interactive"} and .payload.previous == {"MODE": "auto"}'
  'value route on the rebound MODE|.type? == "transitioned" and .payload.from == "reroute" and .payload.to == "review" and .payload.vars_matched == {"MODE": "interactive"}'
  'init records every variable|.type? == "workflow_initialized" and .payload.variables == {"ARM": "with-rule", "MODE": "auto"}'
)

# Fields the change adds, as "field|jq filter selecting the events".
STRIPPED_FIELDS=(
  'vars_matched|.type? == "transitioned"'
  'previous|.type? == "variables_rebound"'
)

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
  for i in "${!STRIPPED_FIELDS[@]}"; do
    echo "strip-field:${STRIPPED_FIELDS[$i]%%|*}"
  done
  echo drop-transition
}

# --- self-test ---------------------------------------------------------------

if [ "${1:-}" = "--self-test" ]; then
  echo "== self-test: clean run (must pass)"
  env -u COMPAT_MUTATION "$0" >/dev/null || fail "self-test: the clean run failed"
  pass "self-test: clean run passed"
  for m in $(mutations); do
    echo "== self-test: COMPAT_MUTATION=$m (must fail)"
    if COMPAT_MUTATION="$m" "$0" >/dev/null 2>&1; then
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
echo "new build: $("$KOTO_NEW_BIN" version 2>&1)"

# --- scratch space -----------------------------------------------------------

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat-valueroute.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
HOME_DIR="$SCRATCH/home"
WORK_DIR="$SCRATCH/work"
mkdir -p "$HOME_DIR" "$WORK_DIR"
cp "$FIXTURE" "$WORK_DIR/value-routing.md"

# koto_in BIN ARGS...: run BIN in the work dir with its own HOME and no
# decider settings or sessions-base override.
koto_in() {
  (cd "$WORK_DIR" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$HOME_DIR" "$@")
}
new_koto() { koto_in "$KOTO_NEW_BIN" "$@"; }
floor_koto() { koto_in "$KOTO_FLOOR_BIN" "$@"; }

# new_next SESSION LABEL WANT-STATE [ARGS...]: one `koto next` by the new build.
new_next() {
  local session="$1" label="$2" want="$3" out got
  shift 3
  out="$(new_koto next "$session" --no-cleanup "$@" 2>&1)" || fail "session: '$label' exited non-zero: $out"
  got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: '$label' printed no state: $out"
  [ "$got" = "$want" ] || fail "session: '$label' expected state '$want', got: $out"
  pass "session: $label -> $got"
}

# --- session steps (new build) -----------------------------------------------

TEMPLATE="$WORK_DIR/value-routing.md"
new_koto init wf --template "$TEMPLATE" --var MODE=auto --var ARM=with-rule >/dev/null 2>"$SCRATCH/init.err" \
  || fail "session: koto init failed: $(cat "$SCRATCH/init.err")"
new_next wf "route on MODE=auto, skip on ARM=with-rule, stop at work" work
out="$(new_koto init wf --template "$TEMPLATE" --attach-live --var MODE=interactive --var ARM=with-rule 2>&1)" \
  || fail "session: the attach that rebinds MODE failed: $out"
printf '%s' "$out" | jq -e '.rebound == {"MODE": "interactive"}' >/dev/null \
  || fail "session: the attach did not rebind MODE: $out"
pass "session: the attach rebinds MODE to interactive"
printf '%s' "$NOTE" | new_koto context add wf note || fail "session: koto context add failed"
new_next wf "the note routes on the rebound MODE to review" review --with-data '{"note": "done"}'

NEW_STATE="$(new_koto status wf | jq -er '.current_state')" || fail "session: koto status failed"

LOG_FILE="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "session: no koto-wf.state.jsonl was written"

# --- mutations (self-test only) ----------------------------------------------

rewrite_lines() {
  local filter="$1" program="$2" tmp="$LOG_FILE.mut" line
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
      [ -n "$program" ] && printf '%s' "$line" | jq -c "$program" >>"$tmp"
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
    rewrite_lines "${entry#*|}" ""
    ;;
  strip-field:*)
    key="${MUTATION#strip-field:}"
    for entry in "${STRIPPED_FIELDS[@]}"; do
      [ "${entry%%|*}" = "$key" ] || continue
      echo "MUTATION: deleting '$key' from every event it can appear on"
      rewrite_lines "${entry#*|}" "del(.payload[\"$key\"])"
    done
    ;;
  drop-transition)
    last_seq="$(jq -s '[.[] | select(.type? == "transitioned") | .seq] | max' "$LOG_FILE")"
    echo "MUTATION: deleting the transitioned event with seq $last_seq"
    rewrite_lines ".type? == \"transitioned\" and .seq == $last_seq" ""
    ;;
esac

# --- event coverage ----------------------------------------------------------

for entry in "${EVENT_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$LOG_FILE")" || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: the log holds no event for '$label'"
  pass "events: $label ($n)"
done
n="$(jq -s '[.[] | select(.type? == "transitioned" and .payload.from == "work")] | .[0].payload | has("vars_matched")' "$LOG_FILE")"
[ "$n" = "false" ] || fail "events: the evidence-routed transition out of work carries vars_matched"
pass "events: a transition that didn't route on a value carries no vars_matched"

# The published contract describes every event and field in the log.
out="$(KOTO_FEED_SPEC="$FEED_SPEC" new_koto template validate-feed "$LOG_FILE" 2>&1)" \
  || fail "contract: koto template validate-feed rejected the log: $out"
pass "contract: the session-feed spec accepts the log"

# --- v0.14.1 reads the log ---------------------------------------------------

reject_errors() {
  if printf '%s' "$2" | grep -Eiq 'corrupt|pars(e|ing)|mismatch|unknown event'; then
    fail "floor: $1 reported an error: $2"
  fi
}

out="$(floor_koto status wf 2>&1)" || fail "floor: koto status exited non-zero: $out"
reject_errors "koto status" "$out"
state="$(printf '%s' "$out" | jq -er '.current_state')" || fail "floor: koto status has no current_state: $out"
[ "$state" = "$NEW_STATE" ] || fail "floor: koto status says '$state', the new build said '$NEW_STATE'"
pass "v$FLOOR_VERSION koto status reads the log (state $state)"

out="$(floor_koto next wf --no-cleanup 2>&1)" || fail "floor: koto next exited non-zero: $out"
reject_errors "koto next" "$out"
printf '%s' "$out" | jq -e '.error == null' >/dev/null || fail "floor: koto next returned an error: $out"
state="$(printf '%s' "$out" | jq -er '.state')" || fail "floor: koto next has no state: $out"
[ "$state" = "$NEW_STATE" ] || fail "floor: koto next says '$state', the new build said '$NEW_STATE'"
pass "v$FLOOR_VERSION koto next reads the log (state $state)"

want="$(new_koto context get wf note 2>&1)" || fail "session: koto context get failed: $want"
out="$(floor_koto context get wf note 2>&1)" || fail "floor: koto context get exited non-zero: $out"
[ "$out" = "$want" ] || fail "floor: koto context get returned '$out', the new build returned '$want'"
pass "v$FLOOR_VERSION koto context get returns the new build's value"

pass "all checks passed"
