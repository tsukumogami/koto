#!/usr/bin/env bash
# Checks that koto v0.14.1 reads a session and a decider ledger the new build
# writes while a template uses a decider check
# (DESIGN-koto-decider-checks.md), including a criterion that falls back to
# the agent (DESIGN-decider-fallback.md).
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/decider-checks-v0_14_1.sh [--self-test]
#
# Session: the new build drives fixtures-decider-checks/decider-checks.md
# against the loopback stub (decider_stub.py answering "fail" and reporting
# 50 input and 2 output tokens), opted in at `auto`. The veto criterion
# blocks review, the check is overridden, and the session moves on to work, a
# state with no decider check whose field the routing decider answers in
# shadow, so its `decider_consulted` carries the answer's tokens and model. A
# second session, nv, points at an endpoint nothing listens on: the criterion
# gets no verdict, so it falls back to the agent. Its gate is pending with the
# criterion under output.awaiting and a decider_fallback event, and review
# waits. The script answers pass with `koto decider answer`, which appends a
# decider_check_answered event, and the next tick passes review on that
# answer and moves on to work, where the routing consultation fails to
# connect and bills nothing. The logs must hold every event in EVENT_CHECKS
# and NV_EVENT_CHECKS, and the new build's
# `koto template validate-feed` must accept them against
# docs/reference/session-feed.md. The ledger must hold a `checked` and a
# `check_overridden` line, which the new build's `koto decider report`
# tallies.
#
# A third session, un, drives fixtures-decider-checks/decider-units.md: a
# veto check in unit mode (`units: jsonl`, with `unit_document`,
# `unit_section`, and a criterion declaring `summary` and `rule_text`) over
# two units, under its own home so its ledger stands apart. The stub fails
# ac-1, and ac-2 is over `max_bytes`, so it falls back to the agent. The
# first tick hands back the rule at `summary` for ac-1, the script answers
# ac-2 with `koto decider answer --unit`, the second tick hands the rule back
# at `full`, and the check is overridden so the session moves on to work.
# The log must hold every event in UN_EVENT_CHECKS: per-unit
# `decider_checked`, `decider_fallback` and `decider_check_answered`, and
# `rule_delivered`. The ledger must hold a per-unit `checked` line.
#
# v0.14.1 can't compile a template with a decider check, so it is handed
# each session after it left the check: it must run `koto status` and
# `koto next` on each, exit 0 and report the state the new build reports
# (skipping the `rule_delivered` event it doesn't know), and run
# `koto decider report` over each ledger, exit 0, and count the new line
# kinds as unknown rather than failing.
#
# The other direction: v0.14.1 drives fixtures-decider-checks/decider-routing.md
# (a routing consultation, no decider check) against the stub, and the new
# build must read the log and ledger it wrote unchanged: `koto status`, and
# `koto decider report` tallying the consultation, leave both files as they
# were, and `koto next` reports the state v0.14.1 reported.
#
# Templates without a decider check are covered by the failure-reporting job,
# which compiles every fixture under both builds.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N       delete the log lines matching EVENT_CHECKS[N]
#   drop-nv-event:N    delete nv's log lines matching NV_EVENT_CHECKS[N]
#   drop-un-event:N    delete un's log lines matching UN_EVENT_CHECKS[N]
#   drop-un-ledger     delete un's per-unit `checked` ledger lines
#   strip-field:K      delete payload field K from the events that carry it
#   alter-value:N      change one value as ALTERED_VALUES[N] says, leaving
#                      every key in place
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
UNIT_FIXTURE="$SCRIPT_DIR/fixtures-decider-checks/decider-units.md"
ROUTING_FIXTURE="$SCRIPT_DIR/fixtures-decider-checks/decider-routing.md"
STUB="$SCRIPT_DIR/decider_stub.py"
FLOOR_VERSION="0.14.1"
KEY="compat-decider-checks-key-7c1e"

# Events the session must leave in its log, as "label|jq filter".
EVENT_CHECKS=(
  'decider_checked for the failed criterion|.type? == "decider_checked" and .payload.gate == "comments" and .payload.rule_id == "comment_reason" and .payload.outcome == "fail" and .payload.mode == "veto" and .payload.blocked == true and (.payload.visit_seq | type) == "number" and (.payload.declaration_hash | type) == "string" and (.payload.input_sha256 | type) == "string"'
  'gate_evaluated for the failed check|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "failed" and .payload.output.failed == ["comment_reason"] and .payload.output.unanswered == [] and .payload.findings[0].message_source == "decider" and (.payload | has("stdout") | not)'
  'the override of the check|.type? == "gate_override_recorded" and .payload.gate == "comments" and .payload.actual_output.failed == ["comment_reason"]'
  'decider_consulted with the billed tokens and model|.type? == "decider_consulted" and .payload.state == "work" and .payload.input_tokens == 50 and .payload.output_tokens == 2 and .payload.unread_usage_attempts == 0 and .payload.provider == "jev" and .payload.model == "compat-stub-1.0.0"'
)

# Events the no-verdict session must leave in its log: the criterion falls
# back to the agent, the gate is pending on it, the agent's pass is recorded,
# and only then does the gate pass, on that answer. The pending gate lists the
# criterion under both unanswered (veto criteria with no verdict and no
# answer) and awaiting (any criterion waiting on the agent, shadow included).
NV_EVENT_CHECKS=(
  'decider_checked for the unanswered criterion|.type? == "decider_checked" and .payload.rule_id == "comment_reason" and .payload.outcome == "unanswered" and .payload.reason == "provider_error" and .payload.mode == "veto" and .payload.blocked == false'
  'decider_fallback handing the criterion to the agent|.type? == "decider_fallback" and .payload.gate == "comments" and .payload.rule_id == "comment_reason" and .payload.reason == "provider_error" and .payload.mode == "veto" and (.payload.visit_seq | type) == "number" and (.payload.declaration_hash | type) == "string" and (.payload.input_sha256 | type) == "string" and (.payload.input_bytes | type) == "number"'
  'gate_evaluated pending on the waiting criterion|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "pending" and .payload.output.failed == [] and .payload.output.unanswered == ["comment_reason"] and .payload.output.awaiting == ["comment_reason"] and .payload.findings[0].level == "warning"'
  'decider_check_answered recording pass|.type? == "decider_check_answered" and .payload.gate == "comments" and .payload.rule_id == "comment_reason" and .payload.answer == "pass" and (.payload.fallback_seq | type) == "number" and (.payload.declaration_hash | type) == "string"'
  'gate_evaluated passing on the answer|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "passed" and .payload.output.agent_answered == ["comment_reason"] and .payload.output.unanswered == [] and .payload.output.awaiting == []'
  'decider_consulted that billed nothing|.type? == "decider_consulted" and .payload.state == "work" and .payload.outcome == "error" and .payload.error_class == "connect" and (.payload | has("input_tokens") | not) and .payload.unread_usage_attempts == 0'
)

# Events the unit-mode session must leave in its log: each unit graded on its
# own with the check's declared labels, the over-budget unit handed to the
# agent and answered for that unit alone, and the rule handed back at summary
# and then in full.
UN_EVENT_CHECKS=(
  'per-unit decider_checked for the failed unit|.type? == "decider_checked" and .payload.gate == "acceptance" and .payload.rule_id == "ac_binary" and .payload.unit.key == "ac-1" and .payload.outcome == "fail" and .payload.blocked == true and .payload.unit_labels.document == "prd" and .payload.unit_labels.section == "acceptance-criteria"'
  'per-unit decider_checked for the over-budget unit|.type? == "decider_checked" and .payload.unit.key == "ac-2" and .payload.outcome == "unanswered" and .payload.reason == "over_budget"'
  'per-unit decider_fallback handing ac-2 to the agent|.type? == "decider_fallback" and .payload.gate == "acceptance" and .payload.unit.key == "ac-2" and .payload.reason == "over_budget" and .payload.unit_labels.document == "prd"'
  'per-unit decider_check_answered for ac-2|.type? == "decider_check_answered" and .payload.gate == "acceptance" and .payload.unit.key == "ac-2" and .payload.answer == "pass" and (.payload.fallback_seq | type) == "number"'
  'gate_evaluated failing on ac-1 alone|.type? == "gate_evaluated" and .payload.gate == "acceptance" and .payload.outcome == "failed" and ([(.payload.output.units // [])[] | select(.failed == ["ac_binary"]) | .key] == ["ac-1"])'
  'rule_delivered at summary|.type? == "rule_delivered" and .payload.gate == "acceptance" and .payload.rule_id == "ac_binary" and .payload.level == "summary" and .payload.unit_count == 1'
  'rule_delivered in full|.type? == "rule_delivered" and .payload.gate == "acceptance" and .payload.rule_id == "ac_binary" and .payload.level == "full" and .payload.unit_count == 1'
)

# Required fields of the new event, and the usage field written on every
# consultation, as "field|jq filter selecting the events".
STRIPPED_FIELDS=(
  'visit_seq|.type? == "decider_checked"'
  'outcome|.type? == "decider_checked"'
  'unread_usage_attempts|.type? == "decider_consulted"'
  'reason|.type? == "decider_fallback"'
  'answer|.type? == "decider_check_answered"'
  'fallback_seq|.type? == "decider_check_answered"'
)

# Values changed in place, every key kept, as "label|log|jq filter selecting
# the events|jq program". log is wf or nv.
ALTERED_VALUES=(
  'a failed verdict read as a pass|wf|.type? == "decider_checked"|.payload.outcome = "pass"'
  'a no-verdict criterion marked as blocking|nv|.type? == "decider_checked"|.payload.blocked = true'
  'the waiting gate read as passed|nv|.type? == "gate_evaluated" and .payload.gate == "comments" and .payload.outcome == "pending"|.payload.outcome = "passed"'
  'the agent answer read as fail|nv|.type? == "decider_check_answered"|.payload.answer = "fail"'
  'the routing tokens miscounted|wf|.type? == "decider_consulted"|.payload.input_tokens = 49'
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
  for i in "${!UN_EVENT_CHECKS[@]}"; do
    echo "drop-un-event:$i"
  done
  echo drop-un-ledger
  for i in "${!STRIPPED_FIELDS[@]}"; do
    echo "strip-field:${STRIPPED_FIELDS[$i]%%|*}"
  done
  for i in "${!ALTERED_VALUES[@]}"; do
    echo "alter-value:$i"
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
for f in "$FIXTURE" "$UNIT_FIXTURE" "$ROUTING_FIXTURE"; do
  [ -f "$f" ] || fail "fixture $f is missing"
done
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

python3 "$STUB" --port-file "$SCRATCH/port" --key "$KEY" --log "$SCRATCH/stub.log" --choice fail --usage 50,2 &
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
[ "$got" = "review" ] || fail "session: a missing verdict should stop review for the agent, got: $out"
printf '%s' "$out" | jq -e '[.blocking_conditions[] | select(.name == "comments" and .status == "pending")] | length == 1' >/dev/null \
  || fail "session: review shows no pending comments check: $out"
pass "session: a missing verdict falls back to the agent -> $got"
out="$(nv_koto decider answer nv --gate comments --criterion comment_reason --answer pass 2>&1)" \
  || fail "session: koto decider answer failed: $out"
pass "session: the agent answered pass"
out="$(nv_koto next nv --no-cleanup 2>&1)" || fail "session: nv's second koto next exited non-zero: $out"
got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: nv's second koto next printed no state: $out"
[ "$got" = "work" ] || fail "session: the agent's pass should let review through, got: $out"
pass "session: the agent's answer passes review -> $got"
NV_STATE="$(nv_koto status nv | jq -er '.current_state')" || fail "session: koto status nv failed"
NV_LOG="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-nv.state.jsonl' -type f | head -n 1)"
[ -n "$NV_LOG" ] || fail "session: no koto-nv.state.jsonl was written"
LEDGER="$HOME_DIR/.koto/_decider_ledger.jsonl"
[ -f "$LEDGER" ] || fail "session: no ledger was written"

# in_dir HOME WORK BIN ARGS...: run BIN in WORK with its own HOME and no
# decider settings or sessions-base override.
in_dir() {
  local home="$1" work="$2"
  shift 2
  (cd "$work" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$home" "$@")
}
# stub_in HOME WORK BIN ARGS...: the same, opted in at auto against the stub.
stub_in() {
  local home="$1" work="$2"
  shift 2
  (cd "$work" && env -u KOTO_SESSIONS_BASE HOME="$home" KOTO_DECIDER=auto \
    KOTO_DECIDER_API_KEY="$KEY" KOTO_DECIDER_ENDPOINT="$ENDPOINT" "$@")
}

# The unit-mode session, under its own home and work dir. ac-1 is graded and
# the stub fails it; ac-2 is over the check's 64-byte max_bytes, so it is
# never sent and falls back to the agent.
UN_HOME="$SCRATCH/un-home"
UN_WORK="$SCRATCH/un-work"
mkdir -p "$UN_HOME" "$UN_WORK"
cp "$UNIT_FIXTURE" "$UN_WORK/decider-units.md"
{
  printf '{"key":"ac-1","text":"The export finishes."}\n'
  printf '{"key":"ac-2","text":"%s"}\n' "$(printf '%080d' 0)"
} >"$UN_WORK/units.jsonl"
un_koto() { stub_in "$UN_HOME" "$UN_WORK" "$KOTO_NEW_BIN" "$@"; }

# un_review LABEL LEVEL: tick un, which must stay at review with the check
# failed on ac-1 and the rule handed back at LEVEL for ac-1 alone.
un_review() {
  local label="$1" level="$2" out got
  out="$(un_koto next un --no-cleanup 2>&1)" || fail "session: un's '$label' exited non-zero: $out"
  got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: un's '$label' printed no state: $out"
  [ "$got" = "review" ] || fail "session: un's '$label' expected state 'review', got: $out"
  printf '%s' "$out" | jq -e --arg level "$level" '[.blocking_conditions[] | select(.name == "acceptance" and .status == "failed")] | length == 1 and (.[0].rules | length) == 1 and .[0].rules[0].rule_id == "ac_binary" and .[0].rules[0].level == $level and .[0].rules[0].units == ["ac-1"]' >/dev/null \
    || fail "session: un's '$label' doesn't hand the rule back at $level for ac-1: $out"
  pass "session: $label -> $got (rule at $level)"
}

un_koto init un --template "$UN_WORK/decider-units.md" >/dev/null 2>"$SCRATCH/init-un.err" \
  || fail "session: koto init un failed: $(cat "$SCRATCH/init-un.err")"
un_review "a unit fails and the over-budget unit waits" summary
out="$(un_koto decider answer un --gate acceptance --criterion ac_binary --unit ac-2 --answer pass 2>&1)" \
  || fail "session: koto decider answer --unit failed: $out"
printf '%s' "$out" | jq -e '.unit == "ac-2" or .unit.key == "ac-2"' >/dev/null \
  || fail "session: the answer doesn't name its unit: $out"
pass "session: the agent answered ac-2 alone"
un_review "the check fails again with ac-2 answered" full
out="$(un_koto overrides record un --gate acceptance --rationale "compat: the criterion is fine" 2>&1)" \
  || fail "session: un's override failed: $out"
out="$(un_koto next un --no-cleanup 2>&1)" || fail "session: un's last koto next exited non-zero: $out"
got="$(printf '%s' "$out" | jq -er '.state')" || fail "session: un's last koto next printed no state: $out"
[ "$got" = "work" ] || fail "session: the override should move un on to work, got: $out"
pass "session: the override moves un on to work"
UN_STATE="$(un_koto status un | jq -er '.current_state')" || fail "session: koto status un failed"
UN_LOG="$(find "$UN_HOME" "$UN_WORK" -name 'koto-un.state.jsonl' -type f | head -n 1)"
[ -n "$UN_LOG" ] || fail "session: no koto-un.state.jsonl was written"
UN_LEDGER="$UN_HOME/.koto/_decider_ledger.jsonl"
[ -f "$UN_LEDGER" ] || fail "session: un wrote no ledger"

# The older writer: v0.14.1 drives a routing consultation against the stub
# and writes a log and a ledger from before unit mode and rule delivery.
OLD_HOME="$SCRATCH/old-home"
OLD_WORK="$SCRATCH/old-work"
mkdir -p "$OLD_HOME" "$OLD_WORK"
cp "$ROUTING_FIXTURE" "$OLD_WORK/decider-routing.md"
stub_in "$OLD_HOME" "$OLD_WORK" "$KOTO_FLOOR_BIN" init old --template "$OLD_WORK/decider-routing.md" \
  >/dev/null 2>"$SCRATCH/init-old.err" \
  || fail "older writer: v$FLOOR_VERSION koto init failed: $(cat "$SCRATCH/init-old.err")"
out="$(stub_in "$OLD_HOME" "$OLD_WORK" "$KOTO_FLOOR_BIN" next old --no-cleanup 2>&1)" \
  || fail "older writer: v$FLOOR_VERSION koto next exited non-zero: $out"
OLD_STATE="$(in_dir "$OLD_HOME" "$OLD_WORK" "$KOTO_FLOOR_BIN" status old | jq -er '.current_state')" \
  || fail "older writer: v$FLOOR_VERSION koto status failed"
OLD_LOG="$(find "$OLD_HOME" "$OLD_WORK" -name 'koto-old.state.jsonl' -type f | head -n 1)"
[ -n "$OLD_LOG" ] || fail "older writer: v$FLOOR_VERSION wrote no koto-old.state.jsonl"
OLD_LEDGER="$OLD_HOME/.koto/_decider_ledger.jsonl"
[ -f "$OLD_LEDGER" ] || fail "older writer: v$FLOOR_VERSION wrote no ledger"
n="$(jq -s '[.[] | select(.kind? == "consulted")] | length' "$OLD_LEDGER")"
[ "$n" -ge 1 ] || fail "older writer: v$FLOOR_VERSION's ledger has no consulted line"
pass "older writer: v$FLOOR_VERSION wrote a log and a ledger (state $OLD_STATE)"

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
  drop-un-event:*)
    entry="${UN_EVENT_CHECKS[${MUTATION#drop-un-event:}]}"
    echo "MUTATION: deleting un's log lines for '${entry%%|*}'"
    rewrite_lines "$UN_LOG" "${entry#*|}" ""
    ;;
  drop-un-ledger)
    echo "MUTATION: deleting un's per-unit checked ledger lines"
    rewrite_lines "$UN_LEDGER" '.kind? == "checked" and .unit != null' ""
    ;;
  strip-field:*)
    key="${MUTATION#strip-field:}"
    for entry in "${STRIPPED_FIELDS[@]}"; do
      [ "${entry%%|*}" = "$key" ] || continue
      echo "MUTATION: deleting '$key' from every event it can appear on"
      rewrite_lines "$LOG_FILE" "${entry#*|}" "del(.payload[\"$key\"])"
      rewrite_lines "$NV_LOG" "${entry#*|}" "del(.payload[\"$key\"])"
    done
    ;;
  alter-value:*)
    entry="${ALTERED_VALUES[${MUTATION#alter-value:}]}"
    label="${entry%%|*}"; rest="${entry#*|}"
    which="${rest%%|*}"; rest="${rest#*|}"
    filter="${rest%%|*}"; program="${rest#*|}"
    case "$which" in
      wf) target="$LOG_FILE" ;;
      nv) target="$NV_LOG" ;;
      *) fail "alter-value: unknown log '$which'" ;;
    esac
    echo "MUTATION: $label"
    before="$(cksum <"$target")"
    rewrite_lines "$target" "$filter" "$program"
    [ "$before" != "$(cksum <"$target")" ] || fail "alter-value: '$label' changed nothing"
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

for entry in "${UN_EVENT_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$UN_LOG")" || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: un's log holds no event for '$label'"
  pass "events: $label ($n)"
done

for log in "$LOG_FILE" "$NV_LOG" "$UN_LOG"; do
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

n="$(jq -s '[.[] | select(.kind? == "checked" and .unit.key? == "ac-1" and .outcome == "fail")] | length' "$UN_LEDGER")"
[ "$n" -ge 1 ] || fail "ledger: un's ledger has no per-unit checked line for ac-1"
pass "ledger: un's per-unit checked lines ($n for ac-1)"

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

un_floor() { in_dir "$UN_HOME" "$UN_WORK" "$KOTO_FLOOR_BIN" "$@"; }

out="$(un_floor status un 2>&1)" || fail "floor: koto status un exited non-zero: $out"
reject_errors "koto status un" "$out"
state="$(printf '%s' "$out" | jq -er '.current_state')" || fail "floor: koto status un has no current_state: $out"
[ "$state" = "$UN_STATE" ] || fail "floor: koto status un says '$state', the new build said '$UN_STATE'"
pass "v$FLOOR_VERSION koto status reads the unit-mode log (state $state)"

out="$(un_floor next un --no-cleanup 2>&1)" || fail "floor: koto next un exited non-zero: $out"
reject_errors "koto next un" "$out"
printf '%s' "$out" | jq -e '.error == null' >/dev/null || fail "floor: koto next un returned an error: $out"
state="$(printf '%s' "$out" | jq -er '.state')" || fail "floor: koto next un has no state: $out"
[ "$state" = "$UN_STATE" ] || fail "floor: koto next un says '$state', the new build said '$UN_STATE'"
pass "v$FLOOR_VERSION koto next reads the unit-mode log past rule_delivered (state $state)"

out="$(un_floor decider report --json --ledger "$UN_LEDGER" 2>&1)" || fail "floor: koto decider report over un's ledger exited non-zero: $out"
unknown="$(printf '%s' "$out" | jq -er '.header.unknown_kind')" || fail "floor: un's report has no unknown_kind: $out"
want="$(jq -s '[.[] | select(.kind? == "checked" or .kind? == "check_overridden")] | length' "$UN_LEDGER")"
[ "$unknown" -ge "$want" ] && [ "$want" -ge 1 ] \
  || fail "floor: expected un's $want checked and check_overridden lines counted as unknown, got $unknown"
pass "v$FLOOR_VERSION koto decider report reads the per-unit ledger ($unknown lines of an unknown kind)"

# --- the new build reads what v0.14.1 wrote ----------------------------------

old_new() { in_dir "$OLD_HOME" "$OLD_WORK" "$KOTO_NEW_BIN" "$@"; }
before="$(cksum <"$OLD_LOG") $(cksum <"$OLD_LEDGER")"

out="$(old_new status old 2>&1)" || fail "older writer: the new build's koto status exited non-zero: $out"
state="$(printf '%s' "$out" | jq -er '.current_state')" || fail "older writer: koto status has no current_state: $out"
[ "$state" = "$OLD_STATE" ] || fail "older writer: the new build says '$state', v$FLOOR_VERSION said '$OLD_STATE'"
pass "older writer: the new build's koto status reads v$FLOOR_VERSION's log (state $state)"

out="$(old_new decider report --json --ledger "$OLD_LEDGER" 2>&1)" \
  || fail "older writer: the new build's koto decider report exited non-zero: $out"
printf '%s' "$out" | jq -e '.header.unknown_kind == 0' >/dev/null \
  || fail "older writer: the new build doesn't know a line kind v$FLOOR_VERSION wrote: $out"
pass "older writer: the new build's koto decider report reads v$FLOOR_VERSION's ledger"

[ "$before" = "$(cksum <"$OLD_LOG") $(cksum <"$OLD_LEDGER")" ] \
  || fail "older writer: reading changed v$FLOOR_VERSION's log or ledger"
pass "older writer: the log and ledger are unchanged by reading"

out="$(old_new next old --no-cleanup 2>&1)" || fail "older writer: the new build's koto next exited non-zero: $out"
printf '%s' "$out" | jq -e '.error == null' >/dev/null || fail "older writer: koto next returned an error: $out"
state="$(printf '%s' "$out" | jq -er '.state')" || fail "older writer: koto next has no state: $out"
[ "$state" = "$OLD_STATE" ] || fail "older writer: the new build's koto next says '$state', v$FLOOR_VERSION said '$OLD_STATE'"
pass "older writer: the new build's koto next reads v$FLOOR_VERSION's log (state $state)"

pass "all checks passed"
