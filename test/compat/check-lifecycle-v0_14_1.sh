#!/usr/bin/env bash
# Checks that koto v0.14.1 reads a session log the new build writes once
# checks report findings at every level and agents dispute them: a passing
# gate's warning findings and a `finding_disputed` event.
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.14.1 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/check-lifecycle-v0_14_1.sh [--self-test]
#
# Session: the new build drives fixtures-check-lifecycle/check-lifecycle.md.
# Its `style` gate prints a warning and an info finding and exits 0, so the
# tick passes `lint` and stops at `work`, and the response carries the
# warning under `advisories`. `koto disputes record` then disputes the
# warning's rule at `style` in `lint`, a state the session has left, and a
# second dispute names `work`'s missing gate and must be refused without
# touching the log. The log must hold every event in EVENT_CHECKS, the new
# build's `koto template validate-feed` must accept it against
# docs/reference/session-feed.md, and the run journal must hold the
# finding's `check_finding` and the dispute's `check_disputed`, never its
# reason.
#
# v0.14.1 is then handed that session: it must run `koto status` and
# `koto next` on it, exit 0 and report the state the new build reports.
# The fixture has no field v0.14.1's template reader doesn't know, and
# `work` has no gate, so v0.14.1 judges nothing the new build judged
# differently.
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
FIXTURE="$SCRIPT_DIR/fixtures-check-lifecycle/check-lifecycle.md"
FLOOR_VERSION="0.14.1"
REASON="the file is generated; the whitespace is upstream's"

# Events the session must leave in its log, as "label|jq filter".
EVENT_CHECKS=(
  'passing gate with a warning|.type? == "gate_evaluated" and .payload.gate == "style" and .payload.outcome == "passed" and (.payload.findings // [] | map(select(.rule_id == "W291" and .level == "warning")) | length) == 1'
  'dispute of an earlier state|.type? == "finding_disputed" and .payload.state == "lint" and .payload.gate == "style" and .payload.rule_id == "W291" and .payload.source == "agent" and .payload.attempt == 1 and .payload.visit_attempt == 1 and (.payload.reason // "" | length) > 0'
)

# Fields the change relies on, as "field|jq filter selecting the events".
STRIPPED_FIELDS=(
  'findings|.type? == "gate_evaluated"'
  'source|.type? == "finding_disputed"'
  'attempt|.type? == "finding_disputed"'
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

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat-checklifecycle.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
HOME_DIR="$SCRATCH/home"
WORK_DIR="$SCRATCH/work"
mkdir -p "$HOME_DIR" "$WORK_DIR"
cp "$FIXTURE" "$WORK_DIR/check-lifecycle.md"
cat >"$WORK_DIR/style.sh" <<'EOF'
echo '::koto-finding::{"rule_id":"W291","level":"warning","message":"trailing whitespace","path":"gen/out.py","line":4}'
echo '::koto-finding::{"rule_id":"S100","level":"info","message":"style note"}'
exit 0
EOF

# koto_in BIN ARGS...: run BIN in the work dir with its own HOME and no
# decider settings or sessions-base override.
koto_in() {
  (cd "$WORK_DIR" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$HOME_DIR" "$@")
}
new_koto() { koto_in "$KOTO_NEW_BIN" "$@"; }
floor_koto() { koto_in "$KOTO_FLOOR_BIN" "$@"; }

# --- session steps (new build) -----------------------------------------------

new_koto init wf --template "$WORK_DIR/check-lifecycle.md" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "session: koto init failed: $(cat "$SCRATCH/init.err")"

out="$(new_koto next wf --no-cleanup 2>&1)" || fail "session: koto next exited non-zero: $out"
printf '%s' "$out" | jq -e '.state == "work"' >/dev/null \
  || fail "session: koto next expected state work, got: $out"
printf '%s' "$out" | jq -e '.advisories == [{"state": "lint", "gate": "style", "findings": [.advisories[0].findings[0]]}] and .advisories[0].findings[0].rule_id == "W291" and .advisories[0].findings[0].level == "warning"' >/dev/null \
  || fail "session: the response carries no advisory for style's warning: $out"
pass "session: style passes with a warning, the tick stops at work, the warning rides advisories"

LOG_FILE="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "session: no koto-wf.state.jsonl was written"

out="$(new_koto disputes record wf --gate style --rule W291 --reason "$REASON" --state lint 2>&1)" \
  || fail "session: koto disputes record failed: $out"
printf '%s' "$out" | jq -e '. == {"status": "recorded"}' >/dev/null \
  || fail "session: koto disputes record printed: $out"
pass "session: the warning's rule is disputed in lint"

before="$(cat "$LOG_FILE")"
set +e
out="$(new_koto disputes record wf --gate style --rule W291 --reason "$REASON" 2>&1)"
code=$?
set -e
[ "$code" -eq 2 ] || fail "session: a dispute of a gate work never evaluated exited $code: $out"
printf '%s' "$out" | jq -e '.error.code == "gate_not_evaluated"' >/dev/null \
  || fail "session: the refused dispute printed: $out"
[ "$(cat "$LOG_FILE")" = "$before" ] || fail "session: the refused dispute changed the log"
pass "session: a dispute of a gate not evaluated in work is refused and appends nothing"

NEW_STATE="$(new_koto status wf | jq -er '.current_state')" || fail "session: koto status failed"
[ "$NEW_STATE" = "work" ] || fail "session: the dispute moved the session to '$NEW_STATE'"

# The run journal relays the finding and the dispute, never the reason.
JOURNAL="$HOME_DIR/.koto/_run_journal.jsonl"
[ -f "$JOURNAL" ] || fail "journal: $JOURNAL was not written"
jq -se '[.[] | select(.kind == "check_finding" and .["koto.check.name"] == "style" and .["koto.rule.id"] == "W291" and .["koto.finding.level"] == "warning" and .["koto.finding.count"] == 1)] | length == 1' "$JOURNAL" >/dev/null \
  || fail "journal: no check_finding for style's warning"
jq -se '[.[] | select(.kind == "check_evaluated" and .["koto.check"] == "style" and .["koto.check.finding_count"] == 2)] | length == 1' "$JOURNAL" >/dev/null \
  || fail "journal: style's check_evaluated doesn't count its two findings"
jq -se '[.[] | select(.kind == "check_disputed" and .["koto.state"] == "lint" and .["koto.check.name"] == "style" and .["koto.attempt"] == 1 and .["koto.visit_attempt"] == 1 and .["koto.rule.id"] == "W291" and .["koto.dispute.source"] == "agent")] | length == 1' "$JOURNAL" >/dev/null \
  || fail "journal: no check_disputed for the dispute"
if grep -q "generated" "$JOURNAL"; then
  fail "journal: the dispute's reason reached the journal"
fi
pass "journal: check_finding and check_disputed are written, the reason isn't"

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

# The published contract describes every event and field in the log, the
# dispute included.
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

pass "all checks passed"
