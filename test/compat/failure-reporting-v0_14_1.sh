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
# Event log: the new build drives the session in
# fixtures/failure-reporting-findings.md through a command gate that prints a
# finding and fails twice before passing, a default_action that prints a
# warning finding and fails before succeeding, a context-exists gate satisfied
# by `koto context add`, and a `koto context get`. The log must hold every
# event listed in EVENT_CHECKS, including the fields the check events gained
# (attempt stamps, findings, rule_counts, duration_ms and a failed command
# gate's leading stdout/stderr) and the context events (the reads a gate and
# `koto context get` log, and the writer `koto context add` records).
# v0.14.1 must then run `koto status`, `koto next` and `koto context get` on
# that session, exit 0, and report the state the new build reports.
#
# Context writers: a second session (fixtures/failure-reporting-context.md)
# has a transition assign the published-location key, then koto itself
# overwrites the key (`koto workflows publish`), which it now logs as a
# `context_added` with writer koto. Its log must hold every event in
# WRITER_CHECKS, and v0.14.1's `koto context get` of the key must return the
# koto-written value: an older koto repairs the store from the log, and must
# not restore the stale assigned value over a later koto write.
#
# Sync writes: a `context_added` with writer sync comes from a sync pull,
# which needs cloud storage, so the script derives one from koto's write
# above (writer changed to sync, next seq) and appends it to the wf2 log.
# v0.14.1 must then run `koto status` and `koto next` on wf2 without error and
# still return the written value from `koto context get`, not the stale one.
#
# To cover a new event kind, make the session emit it (SESSION STEPS below)
# and add a line to EVENT_CHECKS. The self-test picks the new line up.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:N     delete the log lines matching EVENT_CHECKS[N] (one
#                    mutation per entry) before the checks read the log
#   drop-writer-event:N
#                    the same for WRITER_CHECKS[N], in the second session's log
#   hide-koto-write  after the event checks, point koto's write of the
#                    published-location key at another key (keeping its seq),
#                    so only the v0.14.1 read can notice: with no later write
#                    of the key in the log it restores the stale assigned value
#   drop-sync-write  delete the derived sync line after it is appended
#   strip-sync-writer
#                    delete the writer from the derived sync line
#   strip-field:K    delete payload field K from every check event that
#                    carries it (one mutation per STRIPPED_FIELDS entry), so
#                    the events stay but a check-event field goes missing
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
FIXTURE="$SCRIPT_DIR/fixtures/failure-reporting-findings.md"
CONTEXT_FIXTURE="$SCRIPT_DIR/fixtures/failure-reporting-context.md"
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
  'attempt stamps|.type? == "gate_evaluated" and .payload.gate == "tests" and .payload.attempt == 2 and .payload.visit_attempt == 2'
  'gate findings|.type? == "gate_evaluated" and .payload.outcome == "failed" and (.payload.findings // [] | map(select(.rule_id == "E501" and .message_source == "check")) | length) == 1'
  'gate rule_counts|.type? == "gate_evaluated" and .payload.rule_counts.E501.visit == 2 and .payload.rule_counts.E501.session == 2'
  'gate captured streams|.type? == "gate_evaluated" and ((.payload.stdout // "") | contains("build-stdout-line")) and ((.payload.stderr // "") | contains("build-stderr-line")) and .payload.duration_ms >= 0'
  'default_action findings and rule_counts|.type? == "default_action_executed" and .payload.attempt == 1 and (.payload.findings // [] | map(select(.rule_id == "W291")) | length) == 1 and .payload.rule_counts.__action__.session == 1 and .payload.duration_ms >= 0'
  'context add writer|.type? == "context_added" and .payload.key == "review_note" and .payload.writer == "agent"'
  'context read by a gate|.type? == "context_read" and .payload.reader == "gate" and .payload.gate == "note" and .payload.key == "review_note" and .payload.access == "presence" and .payload.state == "review"'
  'context read by koto context get|.type? == "context_read" and .payload.reader == "cli" and .payload.key == "review_note" and .payload.present == true and ((.payload.hash // "") | test("^[0-9a-f]{64}$"))'
)

# Events the context-writers session must leave in its log, as above.
WRITER_CHECKS=(
  'transition assignment|.type? == "transitioned" and .payload.context_assignments["workflows/publish-location"] == "stale-from-transition"'
  'koto write|.type? == "context_added" and .payload.key == "workflows/publish-location" and .payload.writer == "koto"'
)

# Fields the check events gained, as "field|jq filter selecting the events
# that carry it". A strip-field mutation deletes each one in turn, and one of
# the EVENT_CHECKS entries above has to notice.
STRIPPED_FIELDS=(
  'attempt|.type? == "gate_evaluated" or .type? == "default_action_executed"'
  'visit_attempt|.type? == "gate_evaluated" or .type? == "default_action_executed"'
  'findings|.type? == "gate_evaluated" or .type? == "default_action_executed"'
  'rule_counts|.type? == "gate_evaluated" or .type? == "default_action_executed"'
  'duration_ms|.type? == "gate_evaluated" or .type? == "default_action_executed"'
  'stdout|.type? == "gate_evaluated"'
  'stderr|.type? == "gate_evaluated"'
  'writer|.type? == "context_added"'
  'hash|.type? == "context_read"'
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
  for i in "${!WRITER_CHECKS[@]}"; do
    echo "drop-writer-event:$i"
  done
  for i in "${!STRIPPED_FIELDS[@]}"; do
    echo "strip-field:${STRIPPED_FIELDS[$i]%%|*}"
  done
  echo drop-transition
  echo hide-koto-write
  echo drop-sync-write
  echo strip-sync-writer
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
[ -f "$CONTEXT_FIXTURE" ] || fail "fixture $CONTEXT_FIXTURE is missing"

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

# Command gate: fails twice (prints a finding and a line to each stream,
# exits 1), then passes.
new_next "failing command gate" build gate_blocked
new_next "failing command gate, second attempt" build gate_blocked
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

# Context writers: a transition assigns the published-location key, then koto
# overwrites it.
cp "$CONTEXT_FIXTURE" "$WORK_DIR/failure-reporting-context.md"
new_koto init wf2 --template "$WORK_DIR/failure-reporting-context.md" >/dev/null 2>"$SCRATCH/init2.err" \
  || fail "session: koto init wf2 failed: $(cat "$SCRATCH/init2.err")"
out="$(new_koto next wf2 --with-data '{"step": "go"}' 2>&1)" \
  || fail "session: wf2 transition failed: $out"
[ "$(printf '%s' "$out" | jq -r '.state')" = "hold" ] || fail "session: wf2 expected hold, got: $out"
PUBLISHED_DIR="$WORK_DIR/published-workflows"
new_koto workflows publish --session wf2 --dir "$PUBLISHED_DIR" \
  || fail "session: koto workflows publish failed"
got="$(new_koto context get wf2 workflows/publish-location)" \
  || fail "session: koto context get of the published location failed"
[ "$got" = "$PUBLISHED_DIR" ] || fail "session: the published location reads '$got'"
pass "session: koto overwrote a transition-assigned key"

LOG_FILE2="$(find "$HOME_DIR" "$WORK_DIR" -name 'koto-wf2.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE2" ] || fail "session: no koto-wf2.state.jsonl was written"

# --- log mutations (self-test only) ------------------------------------------

# drop_lines FILTER: delete every log line the jq filter selects, leaving the
# bytes of every other line as they are.
# drop_lines FILTER [LOG]: the same for LOG instead of the main session's log.
drop_lines() {
  local filter="$1" log="${2:-$LOG_FILE}" tmp line
  tmp="$log.mut"
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
      continue
    fi
    printf '%s\n' "$line" >>"$tmp"
  done <"$log"
  mv "$tmp" "$log"
}

# strip_field KEY FILTER: delete .payload.KEY from every log line the jq
# filter selects, leaving the bytes of every other line as they are.
strip_field() {
  local key="$1" filter="$2" tmp="$LOG_FILE.mut" line
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
      printf '%s' "$line" | jq -c --arg k "$key" 'del(.payload[$k])' >>"$tmp"
      continue
    fi
    printf '%s\n' "$line" >>"$tmp"
  done <"$LOG_FILE"
  mv "$tmp" "$LOG_FILE"
}

case "$MUTATION" in
  strip-field:*)
    key="${MUTATION#strip-field:}"
    for entry in "${STRIPPED_FIELDS[@]}"; do
      [ "${entry%%|*}" = "$key" ] || continue
      echo "MUTATION: deleting '$key' from every event it can appear on"
      strip_field "$key" "${entry#*|}"
    done
    ;;
  drop-event:*)
    entry="${EVENT_CHECKS[${MUTATION#drop-event:}]}"
    echo "MUTATION: deleting log lines for '${entry%%|*}'"
    drop_lines "${entry#*|}"
    ;;
  drop-writer-event:*)
    entry="${WRITER_CHECKS[${MUTATION#drop-writer-event:}]}"
    echo "MUTATION: deleting wf2 log lines for '${entry%%|*}'"
    drop_lines "${entry#*|}" "$LOG_FILE2"
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
for entry in "${WRITER_CHECKS[@]}"; do
  label="${entry%%|*}"
  filter="${entry#*|}"
  n="$(jq -s "[.[] | select($filter)] | length" "$LOG_FILE2")" \
    || fail "events: the filter for '$label' did not run"
  [ "$n" -ge 1 ] || fail "events: the wf2 log holds no event for '$label'"
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

if [ "$MUTATION" = "hide-koto-write" ]; then
  echo "MUTATION: pointing koto's write of the published-location key at another key"
  tmp="$LOG_FILE2.mut"
  : >"$tmp"
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s' "$line" | jq -e "${WRITER_CHECKS[1]#*|}" >/dev/null 2>&1; then
      printf '%s' "$line" | jq -c '.payload.key = "unrelated-key"' >>"$tmp"
      continue
    fi
    printf '%s\n' "$line" >>"$tmp"
  done <"$LOG_FILE2"
  mv "$tmp" "$LOG_FILE2"
fi
out="$(floor_koto status wf2 2>&1)" || fail "floor: koto status wf2 exited non-zero: $out"
reject_errors "koto status wf2" "$out"
out="$(floor_koto context get wf2 workflows/publish-location 2>&1)" \
  || fail "floor: koto context get of the published location exited non-zero: $out"
[ "$out" = "$PUBLISHED_DIR" ] \
  || fail "floor: v$FLOOR_VERSION restored '$out' over koto's later write of the key"
pass "v$FLOOR_VERSION keeps koto's write over the stale transition value"

# --- a sync write in the log -------------------------------------------------

# A real `context_added` with writer sync comes from a sync pull, which needs
# cloud storage this script doesn't have. So the line is derived: a copy of
# koto's write of the transition-assigned key, with only the writer changed to
# "sync" and the seq set to the next one, written the way the new build writes
# its log (one compact JSON object per line, fields in the same order).
source_line="$(jq -c "select(${WRITER_CHECKS[1]#*|})" "$LOG_FILE2" | tail -n 1)"
[ -n "$source_line" ] || fail "sync: the wf2 log holds no koto write to derive the sync line from"
next_seq="$(jq -s '[.[] | .seq? // empty] | max + 1' "$LOG_FILE2")"
SYNC_LINE="$(printf '%s' "$source_line" | jq -c --argjson seq "$next_seq" '.seq = $seq | .payload.writer = "sync"')"
printf '%s\n' "$SYNC_LINE" >>"$LOG_FILE2"
case "$MUTATION" in
  drop-sync-write)
    echo "MUTATION: deleting the derived sync line"
    drop_lines '.type? == "context_added" and .payload.writer? == "sync"' "$LOG_FILE2"
    ;;
  strip-sync-writer)
    echo "MUTATION: deleting the writer from the derived sync line"
    tmp="$LOG_FILE2.mut"
    : >"$tmp"
    while IFS= read -r line || [ -n "$line" ]; do
      if [ "$line" = "$SYNC_LINE" ]; then
        printf '%s' "$line" | jq -c 'del(.payload.writer)' >>"$tmp"
        continue
      fi
      printf '%s\n' "$line" >>"$tmp"
    done <"$LOG_FILE2"
    mv "$tmp" "$LOG_FILE2"
    ;;
esac
# The last event is the sync write of the key, at the seq after the one before
# it, carrying the same content hash and size as koto's write.
jq -se --arg key workflows/publish-location --argjson src "$source_line" '
    [.[] | select(.seq?)] as $ev
    | ($ev | last) as $s
    | $s.type == "context_added" and $s.payload.writer == "sync"
      and $s.payload.key == $key
      and $s.payload.hash == $src.payload.hash and $s.payload.size == $src.payload.size
      and $s.seq == ($ev[-2].seq + 1)' "$LOG_FILE2" >/dev/null \
  || fail "sync: the wf2 log does not end with a sync write of workflows/publish-location"
pass "events: sync write of the transition-assigned key (seq $next_seq)"

out="$(floor_koto status wf2 2>&1)" || fail "floor: koto status wf2 with a sync write exited non-zero: $out"
reject_errors "koto status wf2 with a sync write" "$out"
state="$(printf '%s' "$out" | jq -er '.current_state')" \
  || fail "floor: koto status wf2 has no current_state: $out"
[ "$state" = "hold" ] || fail "floor: koto status wf2 says '$state', expected hold"
out="$(floor_koto next wf2 --no-cleanup 2>&1)" || fail "floor: koto next wf2 with a sync write exited non-zero: $out"
reject_errors "koto next wf2 with a sync write" "$out"
printf '%s' "$out" | jq -e '.error == null and .state == "hold"' >/dev/null \
  || fail "floor: koto next wf2 with a sync write returned: $out"
pass "v$FLOOR_VERSION koto status and koto next read a log holding a sync write (state hold)"
out="$(floor_koto context get wf2 workflows/publish-location 2>&1)" \
  || fail "floor: koto context get after the sync write exited non-zero: $out"
[ "$out" = "$PUBLISHED_DIR" ] \
  || fail "floor: v$FLOOR_VERSION restored '$out' over the sync write of the key"
pass "v$FLOOR_VERSION keeps the sync write over the stale transition value"

pass "all checks passed"
