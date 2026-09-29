#!/usr/bin/env bash
# Checks that koto v0.14.1 reads a session log the new build writes while a
# template uses clear_on_entry and a polling gate, and never brings a cleared
# key back (DESIGN-koto-ci-wait-stale-keys.md).
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/ci-wait-stale-keys-v0_14_1.sh [--self-test]
#
# Session: the new build drives fixtures-ci-wait/ci-wait-stale-keys.md. The
# review state clears `verdict` on entry; a stale verdict is written, the
# workflow goes to fix and back, and the verdict must be gone with one
# context_cleared recorded. A fresh verdict then passes review into ci, whose
# polling gate reports pending once. The log must hold every event in
# EVENT_CHECKS. v0.14.1 must then run `koto status`, `koto next` and
# `koto context get` on the session, exit 0, report the state the new build
# reports, return the fresh verdict, and report the cleared key absent in a
# second session that stops right after its clearing.
#
# Templates that declare neither feature are covered by the failure-reporting
# job, which compiles every other fixture under both builds and checks their
# logs hold no pending outcome.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N      delete the log lines matching EVENT_CHECKS[N]
#   strip-field:K     delete payload field K from the events that carry it
#   resurrect-key     put the stale verdict back in the second session's
#                     store before v0.14.1 reads it
#   drop-transition   delete the last `transitioned` event of the main session
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash and jq. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURE="$SCRIPT_DIR/fixtures-ci-wait/ci-wait-stale-keys.md"
FLOOR_VERSION="0.14.1"
STALE="stale verdict from the last attempt"
FRESH="fresh verdict"

# Events the main session must leave in its log, as "label|jq filter".
EVENT_CHECKS=(
  'context cleared on re-entry|.type? == "context_cleared" and .payload.state == "review" and .payload.keys == ["verdict"] and (.payload.entry_seq | type) == "number"'
  'pending polling gate|.type? == "gate_evaluated" and .payload.gate == "checks" and .payload.outcome == "pending" and .payload.poll.status == "pending" and .payload.poll.evaluations == 1 and (.payload.poll.since | type) == "string" and (.payload | has("attempt") | not)'
  'resolved polling gate|.type? == "gate_evaluated" and .payload.gate == "checks" and .payload.outcome == "failed" and .payload.poll.status == "failed" and .payload.poll.evaluations == 2 and .payload.attempt == 1'
)

# Fields the new events carry, as "field|jq filter selecting the events".
STRIPPED_FIELDS=(
  'entry_seq|.type? == "context_cleared"'
  'poll|.type? == "gate_evaluated"'
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
  echo resurrect-key
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

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat-ciwait.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
HOME_DIR="$SCRATCH/home"
WORK_DIR="$SCRATCH/work"
mkdir -p "$HOME_DIR" "$WORK_DIR"
cp "$FIXTURE" "$WORK_DIR/ci-wait-stale-keys.md"

# The polling gate's command: pending (75) on its first run, failed (1) after,
# so the session can stop at ci with a resolved result for v0.14.1 to re-run.
cat >"$WORK_DIR/ci-status.sh" <<'EOF'
#!/bin/sh
n=$(cat runs.log 2>/dev/null | wc -l)
echo run >> runs.log
if [ "$n" -eq 0 ]; then exit 75; fi
exit 1
EOF
chmod +x "$WORK_DIR/ci-status.sh"

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

add_verdict() {
  printf '%s' "$2" | new_koto context add "$1" verdict || fail "session: koto context add to $1 failed"
}

# --- session steps (new build) -----------------------------------------------

TEMPLATE="$WORK_DIR/ci-wait-stale-keys.md"
new_koto init wf --template "$TEMPLATE" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "session: koto init failed: $(cat "$SCRATCH/init.err")"
new_next wf "review blocks on its verdict" review
add_verdict wf "$STALE"
new_next wf "retry goes to fix" fix --with-data '{"outcome": "retry"}'
new_next wf "the return into review clears the verdict" review --with-data '{"fixed": "yes"}'
if new_koto context exists wf verdict >/dev/null 2>&1; then
  fail "session: the stale verdict survived the return into review"
fi
pass "session: the stale verdict is gone after the return into review"
add_verdict wf "$FRESH"
new_next wf "a fresh verdict passes review; the checks are pending" ci --with-data '{"outcome": "pass"}'
new_next wf "the checks fail on the next run" ci

NEW_STATE="$(new_koto status wf | jq -er '.current_state')" || fail "session: koto status failed"

# A second session that stops right after its clearing.
new_koto init wf2 --template "$TEMPLATE" >/dev/null 2>"$SCRATCH/init2.err" \
  || fail "session: koto init wf2 failed: $(cat "$SCRATCH/init2.err")"
new_next wf2 "wf2 review" review
add_verdict wf2 "$STALE"
new_next wf2 "wf2 retry" fix --with-data '{"outcome": "retry"}'
new_next wf2 "wf2 return into review clears" review --with-data '{"fixed": "yes"}'

LOG_FILE="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "session: no koto-wf.state.jsonl was written"
CTX_DIR2="$(dirname "$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf2.state.jsonl' -type f | head -n 1)")/ctx"

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
  resurrect-key)
    echo "MUTATION: putting the stale verdict back in wf2's store behind koto's back"
    printf '%s' "$STALE" >"$CTX_DIR2/verdict"
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
n="$(jq -s '[.[] | select(.type? == "context_cleared")] | length' "$LOG_FILE")"
[ "$n" -eq 1 ] || fail "events: expected one context_cleared, found $n"
pass "events: exactly one clearing for the one re-entry"

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

out="$(floor_koto context get wf verdict 2>&1)" || fail "floor: koto context get exited non-zero: $out"
[ "$out" = "$FRESH" ] || fail "floor: koto context get returned '$out'"
pass "v$FLOOR_VERSION koto context get returns the verdict written after the clearing"

out="$(floor_koto status wf2 2>&1)" || fail "floor: koto status wf2 exited non-zero: $out"
reject_errors "koto status wf2" "$out"
if out="$(floor_koto context get wf2 verdict 2>&1)"; then
  fail "floor: v$FLOOR_VERSION returned the cleared verdict: '$out'"
fi
pass "v$FLOOR_VERSION reports the cleared verdict absent"

pass "all checks passed"
