#!/usr/bin/env bash
# Checks that koto v0.12.2 stays compatible with field-level decider
# declarations and with session logs a decider-aware koto writes.
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.12.2 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/decider-v0_12_2.sh [--self-test]
#
# Declarations: v0.12.2 must compile the fixture in fixtures/ and a copy with
# every `decider` block stripped (yq v4) to the same cache path, and a
# scripted session must route identically three ways (v0.12.2 on the
# declared template, v0.12.2 on the stripped one, the new build on the
# declared one with KOTO_DECIDER=off). No response may carry the enum's
# escape value, and submitting it must fail like any value outside `values`.
#
# Event logs: the new build, pointed at the loopback stub in
# decider_stub.py, writes one session holding a shadow `decider_consulted`
# event and one where the decider's answer was applied (`source: "decider"`
# evidence). v0.12.2, with no decider settings, must read both through
# `koto status` and `koto next`.
#
# COMPAT_MUTATION breaks the fixture on purpose, to show the checks bite:
#   escape-in-values     add the escape value to the enum's `values`
#   when-target          retarget one transition in the stripped copy only
#   new-when-target      retarget one transition in the new build's copy only
#   escape-in-directive  put the escape value in a directive the new build
#                        prints (new build's copy only)
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash, jq, python3, and mikefarah yq v4. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURE="$SCRIPT_DIR/fixtures/decider-declared.md"
STUB="$SCRIPT_DIR/decider_stub.py"
FLOOR_VERSION="0.12.2"
# The enum's escape value. It appears only there in the fixture.
ESCAPE_TOKEN="zq-escape-7kd2"
# Fixed test key for the loopback stub. Not a secret.
STUB_KEY="koto-compat-test-key-0000"
MUTATIONS="escape-in-values when-target new-when-target escape-in-directive"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

# --- self-test ---------------------------------------------------------------

if [ "${1:-}" = "--self-test" ]; then
  echo "== self-test: clean run (must pass)"
  env -u COMPAT_MUTATION "$0" >/dev/null || fail "self-test: the clean run failed"
  pass "self-test: clean run passed"
  for m in $MUTATIONS; do
    echo "== self-test: COMPAT_MUTATION=$m (must fail)"
    if COMPAT_MUTATION="$m" "$0"; then
      fail "self-test: mutation '$m' did not make the script fail"
    fi
    pass "self-test: mutation '$m' was caught"
  done
  exit 0
elif [ $# -gt 0 ]; then
  fail "usage: $0 [--self-test]"
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

for tool in jq yq python3; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is not on PATH"
done
[ -f "$FIXTURE" ] || fail "fixture $FIXTURE is missing"
[ -f "$STUB" ] || fail "stub $STUB is missing"

yq_version="$(yq --version 2>&1 || true)"
case "$yq_version" in
  *mikefarah*" version v4."*) pass "yq is mikefarah v4 ($yq_version)" ;;
  *) fail "yq check: need mikefarah yq v4, got '$yq_version'" ;;
esac

floor_version="$("$KOTO_FLOOR_BIN" version 2>&1)" || fail "floor version: '$KOTO_FLOOR_BIN version' failed"
case "$floor_version" in
  "koto $FLOOR_VERSION "* | "koto $FLOOR_VERSION") pass "floor binary reports $floor_version" ;;
  *) fail "floor version: expected koto $FLOOR_VERSION, got '$floor_version'" ;;
esac
new_version="$("$KOTO_NEW_BIN" version 2>&1)" || fail "new version: '$KOTO_NEW_BIN version' failed"
echo "new build: $new_version"

# --- scratch space -----------------------------------------------------------

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat.XXXXXX")"
STUB_PID=""
cleanup() {
  if [ -n "$STUB_PID" ]; then
    kill "$STUB_PID" 2>/dev/null || true
    wait "$STUB_PID" 2>/dev/null || true
  fi
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# new_dir LABEL: a fresh directory under the scratch root.
new_dir() {
  mktemp -d "$SCRATCH/$1.XXXXXX"
}

# The decider variables are unset for every call unless a caller opts in.
unset KOTO_DECIDER KOTO_DECIDER_API_KEY KOTO_DECIDER_ENDPOINT

# koto_in HOME WORKDIR BIN ARGS...: run BIN in WORKDIR with its own HOME
# and no decider settings. Extra VAR=value words before BIN go through env.
koto_in() {
  local home="$1" work="$2"
  shift 2
  (cd "$work" && env -u KOTO_DECIDER -u KOTO_DECIDER_API_KEY -u KOTO_DECIDER_ENDPOINT \
    -u KOTO_SESSIONS_BASE HOME="$home" "$@")
}

# --- templates ---------------------------------------------------------------

TPL_DIR="$(new_dir templates)"
DECLARED="$TPL_DIR/declared.md"
# What the new build runs in the three-way scenario. Only the new-build
# mutations make it differ from DECLARED.
NEW_DECLARED="$TPL_DIR/new-declared.md"
STRIPPED="$TPL_DIR/stripped.md"
cp "$FIXTURE" "$DECLARED"

case "${COMPAT_MUTATION:-}" in
  "") ;;
  escape-in-values)
    echo "MUTATION: adding $ESCAPE_TOKEN to triage.route.values"
    yq --front-matter=process -i ".states.triage.accepts.route.values += [\"$ESCAPE_TOKEN\"]" "$DECLARED"
    ;;
  when-target | new-when-target | escape-in-directive) ;; # applied below
  *) fail "unknown COMPAT_MUTATION '${COMPAT_MUTATION}' (known: $MUTATIONS)" ;;
esac

yq --front-matter=process 'del(.states[].accepts[]?.decider)' "$DECLARED" >"$STRIPPED" \
  || fail "yq strip: could not strip decider blocks"
if [ "${COMPAT_MUTATION:-}" = "when-target" ]; then
  echo "MUTATION: retargeting build's passed:true transition from done to dropped in the stripped copy"
  yq --front-matter=process -i '.states.build.transitions[0].target = "dropped"' "$STRIPPED"
fi

cp "$DECLARED" "$NEW_DECLARED"
case "${COMPAT_MUTATION:-}" in
  new-when-target)
    echo "MUTATION: retargeting build's passed:true transition from done to dropped in the new build's copy"
    yq --front-matter=process -i '.states.build.transitions[0].target = "dropped"' "$NEW_DECLARED"
    ;;
  escape-in-directive)
    echo "MUTATION: adding $ESCAPE_TOKEN to the build directive in the new build's copy"
    sed "s/^Build the change and run the tests\.\$/Build the change and run the tests. $ESCAPE_TOKEN/" \
      "$DECLARED" >"$NEW_DECLARED"
    cmp -s "$DECLARED" "$NEW_DECLARED" && fail "self-test: the escape-in-directive mutation changed nothing"
    ;;
esac

cmp -s "$DECLARED" "$STRIPPED" && fail "yq strip: the stripped copy is identical to the declared template"
grep -q 'decider:' "$STRIPPED" && fail "yq strip: the stripped copy still holds a decider block"
grep -q 'decider:' "$DECLARED" || fail "yq strip: the declared template holds no decider block"
pass "stripped copy differs from the declared template and holds no decider block"

# --- compile -----------------------------------------------------------------

COMPILE_HOME="$(new_dir compile-home)"
COMPILE_WORK="$(new_dir compile-work)"
floor_declared_cache="$(koto_in "$COMPILE_HOME" "$COMPILE_WORK" "$KOTO_FLOOR_BIN" template compile "$DECLARED")" \
  || fail "floor compile: v$FLOOR_VERSION rejected the declared template"
floor_stripped_cache="$(koto_in "$COMPILE_HOME" "$COMPILE_WORK" "$KOTO_FLOOR_BIN" template compile "$STRIPPED")" \
  || fail "floor compile: v$FLOOR_VERSION rejected the stripped template"
[ -n "$floor_declared_cache" ] || fail "floor compile: no cache path printed"
[ "$floor_declared_cache" = "$floor_stripped_cache" ] \
  || fail "floor cache path: declared '$floor_declared_cache' != stripped '$floor_stripped_cache'"
pass "v$FLOOR_VERSION compiles both templates to $floor_declared_cache"

new_compile="$(koto_in "$COMPILE_HOME" "$COMPILE_WORK" "$KOTO_NEW_BIN" template compile "$NEW_DECLARED" 2>&1)" \
  || fail "new compile: the new build rejected the declared template: $new_compile"
pass "the new build compiles the declared template"


# --- scripted sessions -------------------------------------------------------

# Set by run_scenario for tick and status_tick.
RUN_HOME=""
RUN_WORK=""
RUN_RAW=""
RUN_TRANSCRIPT=""
RUN_CMD=()

# tick LABEL SESSION [ARGS...]: one `koto next`; the raw JSON goes to
# RUN_RAW and "LABEL<TAB>state<TAB>action<TAB>advanced" to RUN_TRANSCRIPT.
tick() {
  local label="$1"
  shift
  local out line
  out="$(koto_in "$RUN_HOME" "$RUN_WORK" "${RUN_CMD[@]}" next "$@" --no-cleanup 2>&1)" \
    || fail "scenario: '$label' (koto next $*) exited non-zero: $out"
  printf '%s\n' "$out" >>"$RUN_RAW"
  line="$(printf '%s' "$out" | jq -er '[.state, .action, (.advanced | tostring)] | @tsv')" \
    || fail "scenario: '$label' did not print state, action, and advanced: $out"
  printf '%s\t%s\n' "$label" "$line" >>"$RUN_TRANSCRIPT"
}

# status_tick LABEL SESSION: `koto status`; records the current state.
status_tick() {
  local label="$1" session="$2"
  local out line
  out="$(koto_in "$RUN_HOME" "$RUN_WORK" "${RUN_CMD[@]}" status "$session" 2>&1)" \
    || fail "scenario: '$label' (koto status $session) exited non-zero: $out"
  printf '%s\n' "$out" >>"$RUN_RAW"
  line="$(printf '%s' "$out" | jq -er '.current_state')" \
    || fail "scenario: '$label' status has no current_state: $out"
  printf '%s\t%s\n' "$label" "$line" >>"$RUN_TRANSCRIPT"
}

# run_scenario NAME TEMPLATE [VAR=value...] BIN: drive every enum value but
# the escape, and both booleans, to a terminal state in a fresh HOME and
# working directory. Writes $SCRATCH/NAME.transcript and NAME.raw.jsonl.
run_scenario() {
  local name="$1" template="$2"
  shift 2
  RUN_CMD=("$@")
  RUN_HOME="$(new_dir "$name-home")"
  RUN_WORK="$(new_dir "$name-work")"
  RUN_RAW="$SCRATCH/$name.raw.jsonl"
  RUN_TRANSCRIPT="$SCRATCH/$name.transcript"
  : >"$RUN_RAW"
  : >"$RUN_TRANSCRIPT"

  # Session one: auto -> build, false -> triage, manual -> review,
  # approved -> build, true -> done.
  koto_in "$RUN_HOME" "$RUN_WORK" "${RUN_CMD[@]}" init one --template "$template" >/dev/null \
    || fail "scenario $name: koto init one failed"
  tick "one:start" one
  tick "one:route=auto" one --with-data '{"route":"auto"}'
  tick "one:passed=false" one --with-data '{"passed":false}'
  tick "one:route=manual" one --with-data '{"route":"manual"}'
  tick "one:approved=true" one --with-data '{"approved":true}'
  tick "one:passed=true" one --with-data '{"passed":true}'
  status_tick "one:status" one

  # Session two: drop -> dropped.
  koto_in "$RUN_HOME" "$RUN_WORK" "${RUN_CMD[@]}" init two --template "$template" >/dev/null \
    || fail "scenario $name: koto init two failed"
  tick "two:start" two
  tick "two:route=drop" two --with-data '{"route":"drop"}'
  status_tick "two:status" two

  [ -s "$RUN_TRANSCRIPT" ] || fail "scenario $name: the transcript is empty"
  [ -s "$RUN_RAW" ] || fail "scenario $name: no raw output was recorded"
}

run_scenario floor-declared "$DECLARED" "$KOTO_FLOOR_BIN"
run_scenario floor-stripped "$STRIPPED" "$KOTO_FLOOR_BIN"
run_scenario new-declared "$NEW_DECLARED" KOTO_DECIDER=off "$KOTO_NEW_BIN"

grep -q $'\tdone$' "$SCRATCH/floor-declared.transcript" \
  || fail "scenario: session one never reached done"
grep -q $'\tdropped$' "$SCRATCH/floor-declared.transcript" \
  || fail "scenario: session two never reached dropped"

for other in floor-stripped new-declared; do
  if ! cmp -s "$SCRATCH/floor-declared.transcript" "$SCRATCH/$other.transcript"; then
    echo "--- floor-declared vs $other" >&2
    diff "$SCRATCH/floor-declared.transcript" "$SCRATCH/$other.transcript" >&2 || true
    fail "transcripts: floor-declared and $other differ"
  fi
done
pass "transcripts are byte-identical across floor-declared, floor-stripped, and new-declared ($(wc -l <"$SCRATCH/floor-declared.transcript" | tr -d ' ') lines)"
cat "$SCRATCH/floor-declared.transcript"

for run in floor-declared floor-stripped new-declared; do
  if grep -qF "$ESCAPE_TOKEN" "$SCRATCH/$run.raw.jsonl"; then
    grep -F "$ESCAPE_TOKEN" "$SCRATCH/$run.raw.jsonl" >&2
    fail "escape token: '$ESCAPE_TOKEN' appears in a raw $run response"
  fi
done
pass "no raw koto next or koto status response carries the escape token"

# --- escape submission -------------------------------------------------------

# submit_rc HOME WORK DATA [VAR=value...] BIN: exit code of `koto next
# --with-data DATA` on a fresh session sitting in triage.
submit_rc() {
  local home="$1" work="$2" data="$3"
  shift 3
  local rc=0
  koto_in "$home" "$work" "$@" next esc --with-data "$data" --no-cleanup >/dev/null 2>&1 || rc=$?
  echo "$rc"
}

check_escape_rejected() {
  local name="$1" template="$2"
  shift 2
  local home work escape_rc outside_rc
  home="$(new_dir "$name-esc-home")"
  work="$(new_dir "$name-esc-work")"
  koto_in "$home" "$work" "$@" init esc --template "$template" >/dev/null \
    || fail "escape submission ($name): koto init failed"
  koto_in "$home" "$work" "$@" next esc --no-cleanup >/dev/null \
    || fail "escape submission ($name): first koto next failed"
  escape_rc="$(submit_rc "$home" "$work" "{\"route\":\"$ESCAPE_TOKEN\"}" "$@")"
  outside_rc="$(submit_rc "$home" "$work" '{"route":"not-a-listed-value"}' "$@")"
  [ "$escape_rc" -ne 0 ] || fail "escape submission ($name): submitting '$ESCAPE_TOKEN' succeeded"
  [ "$escape_rc" = "$outside_rc" ] \
    || fail "escape submission ($name): escape exit $escape_rc != outside-values exit $outside_rc"
  pass "escape submission ($name): rejected with exit $escape_rc, same as a value outside values"
}

check_escape_rejected floor "$DECLARED" "$KOTO_FLOOR_BIN"
check_escape_rejected new "$DECLARED" KOTO_DECIDER=off "$KOTO_NEW_BIN"

# --- event-log forward compatibility ----------------------------------------

STUB_DIR="$(new_dir stub)"
python3 "$STUB" --port-file "$STUB_DIR/port" --key "$STUB_KEY" --log "$STUB_DIR/requests.jsonl" \
  >"$STUB_DIR/stub.out" 2>&1 &
STUB_PID=$!
i=0
while [ ! -s "$STUB_DIR/port" ]; do
  kill -0 "$STUB_PID" 2>/dev/null || fail "stub: exited early: $(cat "$STUB_DIR/stub.out")"
  i=$((i + 1))
  [ "$i" -le 100 ] || fail "stub: no port after 10 seconds"
  sleep 0.1
done
ENDPOINT="http://127.0.0.1:$(cat "$STUB_DIR/port")/v1/systemone"
pass "stub decider listening at $ENDPOINT"

# produce_session NAME MODE: the new build, opted in with MODE against the
# stub, runs init and one koto next. Sets LOG_HOME, LOG_WORK, LOG_FILE.
produce_session() {
  local name="$1" mode="$2" out
  LOG_HOME="$(new_dir "$name-home")"
  LOG_WORK="$(new_dir "$name-work")"
  koto_in "$LOG_HOME" "$LOG_WORK" "$KOTO_NEW_BIN" init wf --template "$DECLARED" >/dev/null \
    || fail "$name session: koto init failed"
  out="$(koto_in "$LOG_HOME" "$LOG_WORK" KOTO_DECIDER="$mode" KOTO_DECIDER_API_KEY="$STUB_KEY" \
    KOTO_DECIDER_ENDPOINT="$ENDPOINT" "$KOTO_NEW_BIN" next wf 2>&1)" \
    || fail "$name session: koto next with KOTO_DECIDER=$mode failed: $out"
  LOG_FILE="$(find "$LOG_HOME" "$LOG_WORK" -name 'koto-wf.state.jsonl' -type f | head -n 1)"
  [ -n "$LOG_FILE" ] || fail "$name session: no koto-wf.state.jsonl was written"
  LOG_NEXT="$out"
}

# count_events FILE JQ-FILTER: number of log lines matching the filter.
count_events() {
  jq -s "[.[] | select($2)] | length" "$1"
}

# floor_reads NAME EXPECTED-STATE: v0.12.2 runs koto status, then koto next,
# on the session in LOG_HOME/LOG_WORK with no decider settings.
floor_reads() {
  local name="$1" want="$2" out state
  out="$(koto_in "$LOG_HOME" "$LOG_WORK" "$KOTO_FLOOR_BIN" status wf 2>&1)" \
    || fail "floor reads $name log: koto status exited non-zero: $out"
  if printf '%s' "$out" | grep -Eiq 'corrupt|pars(e|ing)|mismatch'; then
    fail "floor reads $name log: koto status reported an error: $out"
  fi
  state="$(printf '%s' "$out" | jq -er '.current_state')" \
    || fail "floor reads $name log: koto status has no current_state: $out"
  [ "$state" = "$want" ] || fail "floor reads $name log: koto status says '$state', expected '$want'"

  out="$(koto_in "$LOG_HOME" "$LOG_WORK" "$KOTO_FLOOR_BIN" next wf --no-cleanup 2>&1)" \
    || fail "floor reads $name log: koto next exited non-zero: $out"
  if printf '%s' "$out" | grep -Eiq 'corrupt|pars(e|ing)|mismatch'; then
    fail "floor reads $name log: koto next reported an error: $out"
  fi
  printf '%s' "$out" | jq -e '.error == null' >/dev/null \
    || fail "floor reads $name log: koto next returned an error: $out"
  state="$(printf '%s' "$out" | jq -er '.state')" \
    || fail "floor reads $name log: koto next has no state: $out"
  [ "$state" = "$want" ] || fail "floor reads $name log: koto next says '$state', expected '$want'"
  pass "v$FLOOR_VERSION runs koto status and koto next on the $name log (state $want)"
}

# Shadow: the decider is consulted, nothing is applied, triage still waits.
produce_session shadow shadow
[ "$(count_events "$LOG_FILE" '.type? == "decider_consulted"')" -ge 1 ] \
  || fail "shadow session: the log holds no decider_consulted event"
[ "$(printf '%s' "$LOG_NEXT" | jq -r '.state + " " + .action')" = "triage evidence_required" ] \
  || fail "shadow session: expected triage evidence_required, got: $LOG_NEXT"
pass "shadow session log holds a decider_consulted event"
floor_reads shadow triage

# Applied: the stub answers auto at 0.95 and the engine routes to build.
produce_session applied auto
[ "$(count_events "$LOG_FILE" '.type? == "decider_consulted"')" -ge 1 ] \
  || fail "applied session: the log holds no decider_consulted event"
[ "$(count_events "$LOG_FILE" '.type? == "evidence_submitted" and .payload.source? == "decider"')" -ge 1 ] \
  || fail "applied session: the log holds no evidence_submitted event with source decider"
grep -q '"source":"decider"' "$LOG_FILE" \
  || fail "applied session: the log has no literal \"source\":\"decider\""
[ "$(printf '%s' "$LOG_NEXT" | jq -r '.state')" = "build" ] \
  || fail "applied session: expected the decider to route to build, got: $LOG_NEXT"
pass "applied session log holds evidence_submitted with source decider"
floor_reads applied build

[ -s "$STUB_DIR/requests.jsonl" ] || fail "stub: recorded no requests"
pass "all checks passed"
