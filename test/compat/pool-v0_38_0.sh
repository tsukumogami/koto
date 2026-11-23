#!/usr/bin/env bash
# Checks that koto v0.38.0, the release before host resource pools, runs a
# template that names a pool, and reads a session log holding every event
# the feature adds.
#
#   KOTO_FLOOR_BIN=/abs/path/to/koto-0.38.0 \
#   KOTO_NEW_BIN=/abs/path/to/target/release/koto \
#     test/compat/pool-v0_38_0.sh [--self-test]
#
# Pools: the user config defines `test-suite` with size 1 and wait_secs 0.
#
# Unthrottled: the new build starts session `holder` on
# fixtures-pool/pooled-background.md, whose background command takes the
# pool's only slot and keeps it until its release file appears; `koto pool
# status` must show the slot held. While it is held, v0.38.0 compiles
# fixtures-pool/pooled-ordinary.md (an ordinary action with
# `pool: test-suite`) and runs session `old` to `done` in one tick: the old
# release ignores the field, so the full pool doesn't hold it back.
#
# Events: the new build then drives session `waiter` on the background
# template. It queues behind `holder`, takes the slot once `holder` ends,
# runs its own command and finishes, so its log holds pool_queued,
# pool_acquired, pool_released, background_started and background_finished.
# v0.38.0 must run `koto status` on `waiter` and `holder`, exit 0 and report
# the state the new build reports.
#
# COMPAT_MUTATION breaks things on purpose, to show the checks bite:
#   drop-event:T     delete `waiter`'s log lines of event type T
#   free-pool        release `holder` before v0.38.0 runs, so the pool isn't
#                    full when the old release is said to ignore it
#   drop-transition  delete the last `transitioned` event from `waiter`'s log
#                    after the new build read it, so v0.38.0 reports another
#                    state
# --self-test runs the script clean, then once per mutation, and passes only
# if the clean run passes and every mutated run fails.
#
# Needs bash and jq. Runs on Linux and macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES="$SCRIPT_DIR/fixtures-pool"
FLOOR_VERSION="0.38.0"
NEW_EVENTS=(pool_queued pool_acquired pool_released background_started background_finished)

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

mutations() {
  local t
  for t in "${NEW_EVENTS[@]}"; do
    echo "drop-event:$t"
  done
  echo free-pool
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
for f in pooled-ordinary.md pooled-background.md; do
  [ -f "$FIXTURES/$f" ] || fail "fixture $FIXTURES/$f is missing"
done

floor_version="$("$KOTO_FLOOR_BIN" version 2>&1)" || fail "floor version: '$KOTO_FLOOR_BIN version' failed"
case "$floor_version" in
  "koto $FLOOR_VERSION "* | "koto $FLOOR_VERSION") pass "floor binary reports $floor_version" ;;
  *) fail "floor version: expected koto $FLOOR_VERSION, got '$floor_version'" ;;
esac
echo "new build: $("$KOTO_NEW_BIN" version 2>&1)"

# --- scratch space -----------------------------------------------------------

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-compat-pool.XXXXXX")"
HOME_DIR="$SCRATCH/home"
WORK_DIR="$SCRATCH/work"
mkdir -p "$HOME_DIR/.koto" "$WORK_DIR"
cp "$FIXTURES/pooled-ordinary.md" "$FIXTURES/pooled-background.md" "$WORK_DIR/"
printf '[pools.test-suite]\nsize = 1\nwait_secs = 0\n' >"$HOME_DIR/.koto/config.toml"

# Whatever happens, let every background command end before the scratch
# space goes.
cleanup() {
  touch "$WORK_DIR/release-holder" "$WORK_DIR/release-waiter" 2>/dev/null || true
  sleep 1
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# koto_in BIN ARGS...: run BIN in the work dir with its own HOME, so the
# koto root is $HOME_DIR/.koto, and with nothing from the caller's koto
# environment.
UNSET=()
for name in KOTO_HOME KOTO_SESSIONS_BASE KOTO_DECIDER KOTO_DECIDER_API_KEY \
  KOTO_DECIDER_ENDPOINT KOTO_TICK_SESSION KOTO_POOL_HELD KOTO_POOL_ROOT \
  CLAUDE_CODE_SESSION_ID $(env | sed -n 's/^\(KOTO_SESSION_[A-Z_]*\)=.*/\1/p'); do
  UNSET+=(-u "$name")
done
koto_in() {
  (cd "$WORK_DIR" && env "${UNSET[@]}" HOME="$HOME_DIR" "$@")
}
new_koto() { koto_in "$KOTO_NEW_BIN" "$@"; }
floor_koto() { koto_in "$KOTO_FLOOR_BIN" "$@"; }

# tick_until BIN NAME JQ: tick NAME with BIN until the response matches JQ,
# for up to 60 seconds; prints the last response.
tick_until() {
  local bin="$1" name="$2" filter="$3" out _
  for _ in $(seq 1 120); do
    out="$(koto_in "$bin" next "$name" --no-cleanup 2>/dev/null)" || true
    if printf '%s' "$out" | jq -e "$filter" >/dev/null 2>&1; then
      printf '%s' "$out"
      return 0
    fi
    sleep 0.5
  done
  printf '%s' "$out"
  return 1
}

# --- the new build holds the pool's only slot --------------------------------

new_koto init holder --template "$WORK_DIR/pooled-background.md" \
  --var "RELEASE=$WORK_DIR/release-holder" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "holder: koto init failed: $(cat "$SCRATCH/init.err")"
out="$(tick_until "$KOTO_NEW_BIN" holder \
  '.blocking_conditions | map(select(.name == "__action__" and .output.status == "running")) | length == 1')" \
  || fail "holder: its background run never reported running: $out"
pass "holder: the new build's background run is running"

out="$(new_koto pool status --json)" || fail "pool status failed: $out"
printf '%s' "$out" | jq -e '.pools | map(select(.name == "test-suite" and .size == 1 and .held == 1 and .free == 0)) | length == 1' >/dev/null \
  || fail "pool status: test-suite isn't full: $out"
pass "pool status: test-suite's only slot is held by holder"

if [ "$MUTATION" = "free-pool" ]; then
  touch "$WORK_DIR/release-holder"
  tick_until "$KOTO_NEW_BIN" holder '.action == "done"' >/dev/null || true
fi

# --- v0.38.0 runs a pooled template unthrottled ------------------------------

# The precondition is checked again just before the old release runs, so a
# pool that isn't full can't pass for one the old release ignored.
out="$(new_koto pool status --json)" || fail "pool status failed: $out"
printf '%s' "$out" | jq -e '.pools | map(select(.name == "test-suite" and .held == 1 and .free == 0)) | length == 1' >/dev/null \
  || fail "unthrottled: test-suite isn't full when v0.38.0 runs, so its run proves nothing: $out"

out="$(floor_koto template compile "$WORK_DIR/pooled-ordinary.md" 2>&1)" \
  || fail "unthrottled: v0.38.0 refused to compile a template with pool: test-suite: $out"
pass "unthrottled: v0.38.0 compiles a template with pool: test-suite"

floor_koto init old --template "$WORK_DIR/pooled-ordinary.md" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "unthrottled: v0.38.0 koto init failed: $(cat "$SCRATCH/init.err")"
started=$(date +%s)
out="$(floor_koto next old --no-cleanup 2>&1)" || fail "unthrottled: v0.38.0 koto next failed: $out"
elapsed=$(($(date +%s) - started))
printf '%s' "$out" | jq -e '.action == "done" and .state == "done"' >/dev/null \
  || fail "unthrottled: v0.38.0 didn't run the pooled action through to done: $out"
[ "$elapsed" -lt 10 ] || fail "unthrottled: v0.38.0 took ${elapsed}s, as though it waited for the pool"
OLD_LOG="$(find "$WORK_DIR" "$HOME_DIR" -name 'koto-old.state.jsonl' -type f | head -n 1)"
[ -n "$OLD_LOG" ] || fail "unthrottled: no koto-old.state.jsonl was written"
jq -se '[.[] | select(.type == "default_action_executed" and .payload.exit_code == 0)] | length == 1' "$OLD_LOG" >/dev/null \
  || fail "unthrottled: v0.38.0's log shows no successful action run"
pass "unthrottled: v0.38.0 ran the action to done while the pool was full"

# --- the new build writes every new event type into one log ------------------

new_koto init waiter --template "$WORK_DIR/pooled-background.md" \
  --var "RELEASE=$WORK_DIR/release-waiter" >/dev/null 2>"$SCRATCH/init.err" \
  || fail "waiter: koto init failed: $(cat "$SCRATCH/init.err")"
out="$(new_koto next waiter --no-cleanup 2>&1)" || fail "waiter: koto next failed: $out"
printf '%s' "$out" | jq -e '.blocking_conditions | map(select(.name == "__pool__" and .output.status == "queued")) | length == 1' >/dev/null \
  || fail "waiter: it wasn't queued behind holder: $out"
pass "waiter: queued behind holder"

touch "$WORK_DIR/release-holder"
out="$(tick_until "$KOTO_NEW_BIN" holder '.action == "done"')" \
  || fail "holder: it never reached done after its release: $out"
pass "holder: finished and released its slot"

out="$(tick_until "$KOTO_NEW_BIN" waiter \
  '.blocking_conditions | map(select(.name == "__action__" and .output.status == "running")) | length == 1')" \
  || fail "waiter: it never took the slot and started its run: $out"
# Mid-run, with the wait, the acquisition and the start in its log, the
# session is still in its pooled state.
out="$(floor_koto status waiter 2>&1)" || fail "status: v0.38.0 koto status waiter mid-run failed: $out"
printf '%s' "$out" | jq -e '.current_state == "test"' >/dev/null \
  || fail "status: v0.38.0 reads waiter mid-run at the wrong state: $out"
pass "status: v0.38.0 reads waiter mid-run, holding its slot, at state 'test'"
touch "$WORK_DIR/release-waiter"
out="$(tick_until "$KOTO_NEW_BIN" waiter '.action == "done"')" \
  || fail "waiter: it never reached done: $out"
pass "waiter: took the slot, ran in the background and finished"

LOG_FILE="$(find "$WORK_DIR" "$HOME_DIR" -name 'koto-waiter.state.jsonl' -type f | head -n 1)"
[ -n "$LOG_FILE" ] || fail "waiter: no koto-waiter.state.jsonl was written"

case "$MUTATION" in
  drop-event:*)
    t="${MUTATION#drop-event:}"
    jq -c "select(.type != \"$t\")" "$LOG_FILE" >"$LOG_FILE.mut"
    mv "$LOG_FILE.mut" "$LOG_FILE"
    ;;
esac

for t in "${NEW_EVENTS[@]}"; do
  jq -se --arg t "$t" '[.[] | select(.type == $t)] | length >= 1' "$LOG_FILE" >/dev/null \
    || fail "events: waiter's log holds no $t event"
done
pass "events: waiter's log holds ${NEW_EVENTS[*]}"

# --- v0.38.0 reads the logs --------------------------------------------------

new_state_of() {
  new_koto status "$1" | jq -er '.current_state'
}
WAITER_STATE="$(new_state_of waiter)" || fail "status: the new build's koto status waiter failed"
HOLDER_STATE="$(new_state_of holder)" || fail "status: the new build's koto status holder failed"

# The comparison below has to catch a log the old release reads differently.
if [ "$MUTATION" = "drop-transition" ]; then
  last="$(grep -n '"type":"transitioned"' "$LOG_FILE" | tail -n 1 | cut -d: -f1)"
  [ -n "$last" ] || fail "drop-transition: waiter's log has no transitioned event"
  sed "${last}d" "$LOG_FILE" >"$LOG_FILE.mut"
  mv "$LOG_FILE.mut" "$LOG_FILE"
fi

for name in waiter holder; do
  if [ "$name" = waiter ]; then new_state="$WAITER_STATE"; else new_state="$HOLDER_STATE"; fi
  out="$(floor_koto status "$name" 2>"$SCRATCH/status.err")" \
    || fail "status: v0.38.0 koto status $name exited non-zero: $(cat "$SCRATCH/status.err")"
  floor_state="$(printf '%s' "$out" | jq -er '.current_state')" \
    || fail "status: v0.38.0 koto status $name printed no current_state: $out"
  [ "$floor_state" = "$new_state" ] \
    || fail "status: v0.38.0 reports $name at '$floor_state', the new build at '$new_state'"
  pass "status: v0.38.0 reads $name's log and reports state '$floor_state'"
done

echo "OK: koto v$FLOOR_VERSION runs a pooled template unthrottled and reads the pool and background events"
