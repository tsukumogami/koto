#!/usr/bin/env bash
# Checks that koto v0.14.1 reads a session and a decider ledger the new build
# writes while a template uses a decider check
# (DESIGN-koto-decider-checks.md).
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/decider-checks-v0_14_1.sh [--self-test]
#
# Session: the new build drives fixtures-decider-checks/decider-checks.md
# against the loopback stub (decider_stub.py answering "fail"), opted in at
# `auto`. The veto criterion blocks review, the check is overridden, and the
# session moves on to work, a state with no decider check. A second session,
# nv, points at an endpoint nothing listens on: the criterion gets no
# verdict, which never blocks, so its gate passes with the criterion under
# output.unanswered and the session moves on to work on its own. The log must hold
# every event in EVENT_CHECKS, and the new build's `koto template
# validate-feed` must accept it against docs/reference/session-feed.md. The
# ledger must hold a `checked` and a `check_overridden` line, which the new
# build's `koto decider report` tallies.
#
# v0.14.1 can't compile a template with a decider check, so it is handed the
# session after it left the check: it must run `koto status` and `koto next`
# on each, exit 0 and report the state the new build reports, and run
# `koto decider report` over the ledger, exit 0, and count the two new line
# kinds as unknown rather than failing.
#
# Templates without a decider check are covered by the failure-reporting job,
# which compiles every fixture under both builds.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N       delete the log lines matching EVENT_CHECKS[N]
#   drop-nv-event:N    delete nv's log lines matching NV_EVENT_CHECKS[N]
#   strip-field:K      delete payload field K from the events that carry it
#   drop-ledger:KIND   delete the ledger's lines of kind KIND
#   drop-transition    delete the last `transitioned` event
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash, jq and python3. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FEED_SPEC="$REPO_ROOT/docs/reference/session-feed.md"
FIXTURE="$SCRIPT_DIR/fixtures-decider-checks/decider-checks.md"
STUB="$SCRIPT_DIR/decider_stub.py"
FLOOR_VERSION="0.14.1"
KEY="compat-decider-checks-key-7c1e"

# Events the session must leave in its log, as "label|jq filter".
EVENT_CHECKS=(
  'decider_checked for the failed criterion|.type? == "decider_checked" and .payload.gate == "comments" and .payload.rule_id == "comment_reason" and .payload.outcome == "fail" and .payload.mode == "veto" and .payload.blocked == true and (.payload.visit_seq | type) == "number" and (.payload.declaration_hash | type) == "string" and (.payload.input_sha256 | type) == "string"'
  'gate_evaluated for the failed check|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "failed" and .payload.output.failed == ["comment_reason"] and .payload.output.unanswered == [] and .payload.findings[0].message_source == "decider" and (.payload | has("stdout") | not)'
  'the override of the check|.type? == "gate_override_recorded" and .payload.gate == "comments" and .payload.actual_output.failed == ["comment_reason"]'
)

# Events the no-verdict session must leave in its log: a pass that keeps
# the criterion listed as unanswered, never read as compliance.
NV_EVENT_CHECKS=(
  'decider_checked for the unanswered criterion|.type? == "decider_checked" and .payload.rule_id == "comment_reason" and .payload.outcome == "unanswered" and .payload.reason == "provider_error" and .payload.mode == "veto" and .payload.blocked == false'
  'gate_evaluated passing with the criterion unanswered|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "passed" and .payload.output.failed == [] and .payload.output.unanswered == ["comment_reason"] and (.payload | has("findings") | not)'
)

# Required fields of the new event, as "field|jq filter selecting the events".
STRIPPED_FIELDS=(
  'visit_seq|.type? == "decider_checked"'
  'outcome|.type? == "decider_checked"'
)

LEDGER_KINDS=(checked check_overridden)

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
  for i in "${!NV_EVENT_CHECKS[@]}"; do
    echo "drop-nv-event:$i"
  done
  for i in "${!STRIPPED_FIELDS[@]}"; do
    echo "strip-field:${STRIPPED_FIELDS[$i]%%|*}"
  done
  for i in "${LEDGER_KINDS[@]}"; do
    echo "drop-ledger:$i"
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
command -v python3 >/dev/null 2>&1 || fail "python3 is not on PATH"
[ -f "$FIXTURE" ] || fail "fixture $FIXTURE is missing"
[ -f "$STUB" ] || fail "stub $STUB is missing"

floor_version="$("$KOTO_FLOOR_BIN" version 2>&1)" || fail "floor version: '$KOTO_FLOOR_BIN version' failed"
case "$floor_version" in
  "koto $FLOOR_VERSION "* | "koto $FLOOR_VERSION") pass "floor binary reports $floor_version" ;;
  *) fail "floor version: expected koto $FLOOR_VERSION, got '$floor_version'" ;;
esac
echo "new build: $("$KOTO_NEW_BIN" version 2>&1)"

# --- scratch space and the stub ----------------------------------------------

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat-checks.XXXXXX")"
STUB_PID=""
cleanup() {
  if [ -n "$STUB_PID" ]; then
    kill "$STUB_PID" 2>/dev/null || true
    wait "$STUB_PID" 2>/dev/null || true
  fi
  rm -rf "$SCRATCH"
}
trap cleanup EXIT
HOME_DIR="$SCRATCH/home"
WORK_DIR="$SCRATCH/work"
mkdir -p "$HOME_DIR" "$WORK_DIR"
cp "$FIXTURE" "$WORK_DIR/decider-checks.md"
printf 'let total = a + b; // add a and b\n' >"$WORK_DIR/slice.txt"

python3 "$STUB" --port-file "$SCRATCH/port" --key "$KEY" --log "$SCRATCH/stub.log" --choice fail &
STUB_PID=$!
for _ in $(seq 1 100); do
  [ -s "$SCRATCH/port" ] && break
  sleep 0.1
done
[ -s "$SCRATCH/port" ] || fail "stub: no port was written"
ENDPOINT="http://127.0.0.1:$(cat "$SCRATCH/port")/v1/systemone"

# koto_in BIN ARGS...: run BIN in the work dir with its own HOME and no
# decider settings or sessions-base override.
koto_in() {
  (cd "$WORK_DIR" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$HOME_DIR" "$@")
}
# The new build, opted in at auto against the stub.
new_koto() {
  (cd "$WORK_DIR" && env -u KOTO_SESSIONS_BASE HOME="$HOME_DIR" KOTO_DECIDER=auto \
    KOTO_DECIDER_API_KEY="$KEY" KOTO_DECIDER_ENDPOINT="$ENDPOINT" "$KOTO_NEW_BIN" "$@")
}
floor_koto() { koto_in "$KOTO_FLOOR_BIN" "$@"; }

new_next() {
  local label="$1" want="$2" out got
  shift 2
  out="$(new_koto next wf --no-cleanup "$@" 2>&1)" || fail "session: '$label' exited non-zero: $out"
  got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: '$label' printed no state: $out"
  [ "$got" = "$want" ] || fail "session: '$label' expected state '$want', got: $out"
  pass "session: $label -> $got"
}

# --- session steps (new build) -----------------------------------------------

new_koto init wf --template "$WORK_DIR/decider-checks.md" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "session: koto init failed: $(cat "$SCRATCH/init.err")"
new_next "the veto criterion fails and blocks review" review
out="$(new_koto overrides record wf --gate comments --rationale "compat: the comment is fine" 2>&1)" \
  || fail "session: the override failed: $out"
pass "session: the check was overridden"
new_next "the override moves on to work" work

NEW_STATE="$(new_koto status wf | jq -er '.current_state')" || fail "session: koto status failed"
LOG_FILE="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "session: no koto-wf.state.jsonl was written"

# The no-verdict session: nothing listens on the discard port, so both
# attempts fail to connect.
nv_koto() {
  (cd "$WORK_DIR" && env -u KOTO_SESSIONS_BASE HOME="$HOME_DIR" KOTO_DECIDER=auto \
    KOTO_DECIDER_API_KEY="$KEY" KOTO_DECIDER_ENDPOINT="http://127.0.0.1:9/v1/systemone" \
    "$KOTO_NEW_BIN" "$@")
}
nv_koto init nv --template "$WORK_DIR/decider-checks.md" >/dev/null 2>"$SCRATCH/init-nv.err" \
  || fail "session: koto init nv failed: $(cat "$SCRATCH/init-nv.err")"
out="$(nv_koto next nv --no-cleanup 2>&1)" || fail "session: nv's koto next exited non-zero: $out"
got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: nv's koto next printed no state: $out"
[ "$got" = "work" ] || fail "session: a missing verdict should not block review, got: $out"
pass "session: a missing verdict passes review -> $got"
NV_STATE="$(nv_koto status nv | jq -er '.current_state')" || fail "session: koto status nv failed"
NV_LOG="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-nv.state.jsonl' -type f | head -n 1)"
[ -n "$NV_LOG" ] || fail "session: no koto-nv.state.jsonl was written"
LEDGER="$HOME_DIR/.koto/_decider_ledger.jsonl"
[ -f "$LEDGER" ] || fail "session: no ledger was written"

# --- mutations (self-test only) ----------------------------------------------

rewrite_lines() {
  local file="$1" filter="$2" program="$3" tmp="$1.mut" line
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
      [ -n "$program" ] && printf '%s' "$line" | jq -c "$program" >>"$tmp"
      continue
    fi
    printf '%s\n' "$line" >>"$tmp"
  done <"$file"
  mv "$tmp" "$file"
}

case "$MUTATION" in
  drop-event:*)
    entry="${EVENT_CHECKS[${MUTATION#drop-event:}]}"
    echo "MUTATION: deleting log lines for '${entry%%|*}'"
    rewrite_lines "$LOG_FILE" "${entry#*|}" ""
    ;;
  drop-nv-event:*)
    entry="${NV_EVENT_CHECKS[${MUTATION#drop-nv-event:}]}"
    echo "MUTATION: deleting nv's log lines for '${entry%%|*}'"
    rewrite_lines "$NV_LOG" "${entry#*|}" ""
    ;;
  strip-field:*)
    key="${MUTATION#strip-field:}"
    for entry in "${STRIPPED_FIELDS[@]}"; do
      [ "${entry%%|*}" = "$key" ] || continue
      echo "MUTATION: deleting '$key' from every event it can appear on"
      rewrite_lines "$LOG_FILE" "${entry#*|}" "del(.payload[\"$key\"])"
    done
    ;;
  drop-ledger:*)
    kind="${MUTATION#drop-ledger:}"
    echo "MUTATION: deleting the ledger's '$kind' lines"
    rewrite_lines "$LEDGER" ".kind? == \"$kind\"" ""
    ;;
  drop-transition)
    last_seq="$(jq -s '[.[] | select(.type? == "transitioned") | .seq] | max' "$LOG_FILE")"
    echo "MUTATION: deleting the transitioned event with seq $last_seq"
    rewrite_lines "$LOG_FILE" ".type? == \"transitioned\" and .seq == $last_seq" ""
    ;;
esac

# --- event coverage and the contract ------------------------------------------

for entry in "${EVENT_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$LOG_FILE")" || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: the log holds no event for '$label'"
  pass "events: $label ($n)"
done

for entry in "${NV_EVENT_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$NV_LOG")" || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: nv's log holds no event for '$label'"
  pass "events: $label ($n)"
done

for log in "$LOG_FILE" "$NV_LOG"; do
  out="$(KOTO_FEED_SPEC="$FEED_SPEC" koto_in "$KOTO_NEW_BIN" template validate-feed "$log" 2>&1)" \
    || fail "contract: koto template validate-feed rejected $(basename "$log"): $out"
  pass "contract: the session-feed spec accepts $(basename "$log")"
done

for kind in "${LEDGER_KINDS[@]}"; do
  n="$(jq -s "[.[] | select(.kind? == \"$kind\")] | length" "$LEDGER")"
  [ "$n" -ge 1 ] || fail "ledger: no '$kind' line"
  pass "ledger: $kind ($n)"
done
out="$(koto_in "$KOTO_NEW_BIN" decider report --json --ledger "$LEDGER" 2>&1)" \
  || fail "ledger: the new build's report failed: $out"
printf '%s' "$out" | jq -e '.checks[0].rule_id == "comment_reason" and .checks[0].fail == 1 and .checks[0].candidate_false_fail == 1 and .checks[0].unanswered == 1 and .checks[0].unanswered_by_cause.provider == 1' >/dev/null \
  || fail "ledger: the new build's report doesn't tally the check: $out"
pass "ledger: the new build tallies the check"

# --- v0.14.1 reads the log and the ledger ------------------------------------

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

out="$(floor_koto status nv 2>&1)" || fail "floor: koto status nv exited non-zero: $out"
reject_errors "koto status nv" "$out"
state="$(printf '%s' "$out" | jq -er '.current_state')" || fail "floor: koto status nv has no current_state: $out"
[ "$state" = "$NV_STATE" ] || fail "floor: koto status nv says '$state', the new build said '$NV_STATE'"
pass "v$FLOOR_VERSION koto status reads the no-verdict log (state $state)"

out="$(floor_koto next nv --no-cleanup 2>&1)" || fail "floor: koto next nv exited non-zero: $out"
reject_errors "koto next nv" "$out"
printf '%s' "$out" | jq -e '.error == null' >/dev/null || fail "floor: koto next nv returned an error: $out"
state="$(printf '%s' "$out" | jq -er '.state')" || fail "floor: koto next nv has no state: $out"
[ "$state" = "$NV_STATE" ] || fail "floor: koto next nv says '$state', the new build said '$NV_STATE'"
pass "v$FLOOR_VERSION koto next reads the no-verdict log (state $state)"

out="$(floor_koto decider report --json --ledger "$LEDGER" 2>&1)" || fail "floor: koto decider report exited non-zero: $out"
unknown="$(printf '%s' "$out" | jq -er '.header.unknown_kind')" || fail "floor: the report has no unknown_kind: $out"
[ "$unknown" -ge 2 ] || fail "floor: expected the two new line kinds counted as unknown, got $unknown"
pass "v$FLOOR_VERSION koto decider report reads the ledger ($unknown lines of an unknown kind)"

pass "all checks passed"
