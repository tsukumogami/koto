#!/usr/bin/env bash
# run-evals_test.sh - tests for scripts/run-evals.sh and its session classifier.
#
# Makes no model calls: a stub `claude` goes first on PATH and plays a nested
# session, and RUN_EVALS_PLUGINS_DIR aims the runner at a throwaway suite. The
# stub records how it was started and, per skill, writes the transcript and
# grades that one kind of session would leave. STUB_MODES maps skill to kind:
#   pass       executes, grades every eval, all assertions pass
#   fail       executes, grades every eval, one assertion fails
#   none       executes (its commands run), grades nothing
#   partial    executes, grades only the first eval
#   empty      executes, writes grading.json files with no expectations
#   plan       starts in plan mode, writes only its plan file, grades nothing
#   denied     every command it tries is denied, grades nothing
#   silent     leaves an empty transcript and grades nothing
#
# Usage: scripts/run-evals_test.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "ok   $1"; }
not_ok() { fail=$((fail + 1)); echo "FAIL $1"; [ -n "${2:-}" ] && echo "$2" | sed 's/^/     /'; }

# --- the stub claude ---------------------------------------------------------
mkdir -p "$WORK/bin"
cat >"$WORK/bin/claude" <<'STUB'
#!/usr/bin/env python3
import json, os, re, sys

args = sys.argv[1:]
prompt = args[args.index("-p") + 1] if "-p" in args else ""
skill = re.search(r"for the (\S+) skill", prompt).group(1)
iter_dir = re.search(r"The eval workspace is prepared at: (\S+)", prompt).group(1)
modes = dict(p.split(":") for p in os.environ.get("STUB_MODES", "").split(",") if p)
mode = modes.get(skill, "pass")

with open(os.environ["STUB_LOG"], "a") as log:
    log.write(json.dumps({"skill": skill, "argv": args, "cwd": os.getcwd(),
                          "tmpdir": os.environ.get("TMPDIR"),
                          "tmpdir_exists": os.path.isdir(os.environ.get("TMPDIR", ""))}) + "\n")

def emit(event):
    print(json.dumps(event))

if mode == "silent":
    sys.exit(1)

requested = args[args.index("--permission-mode") + 1] if "--permission-mode" in args else "default"
emit({"type": "system", "subtype": "init", "permissionMode": "plan" if mode == "plan" else requested})

def call(n, name, denied=False, error=False):
    emit({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "id": f"t{n}", "name": name, "input": {}}]}})
    if denied:
        emit({"type": "system", "subtype": "permission_denied", "tool_use_id": f"t{n}"})
        emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": f"t{n}",
              "is_error": True, "content": "This command requires approval"}]}})
    else:
        emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": f"t{n}",
              "is_error": error, "content": "exit 1" if error else "done"}]}})

if mode == "plan":
    call(1, "Bash")    # a read-only look around, which plan mode allows
    call(2, "Write")   # its own plan file
    emit({"type": "result", "subtype": "success", "result": "Plan mode is on, so I haven't run anything yet."})
    sys.exit(0)
if mode == "denied":
    call(1, "Bash", denied=True)
    call(2, "Write", denied=True)
    emit({"type": "result", "subtype": "success", "result": "I could not run anything.",
          "permission_denials": [{"tool_use_id": "t1"}, {"tool_use_id": "t2"}]})
    sys.exit(0)

call(1, "Bash", error=(mode == "none"))
evals = sorted(d for d in os.listdir(iter_dir) if os.path.isdir(os.path.join(iter_dir, d)))
for i, name in enumerate(evals):
    base = os.path.join(iter_dir, name)
    for side in ("with_skill", "without_skill"):
        with open(os.path.join(base, side, "outputs", "out.md"), "w") as f:
            f.write("output\n")
    if mode == "none" or (mode == "partial" and i > 0):
        continue
    exps = [] if mode == "empty" else [
        {"text": "does the thing", "passed": not (mode == "fail" and i == 0), "evidence": "stub"}]
    with open(os.path.join(base, "with_skill", "grading.json"), "w") as f:
        json.dump({"expectations": exps}, f)
emit({"type": "result", "subtype": "success", "result": f"stub session for {skill} ({mode}) done"})
STUB
chmod +x "$WORK/bin/claude"

# --- a throwaway suite --------------------------------------------------------
make_skill() {
  local dir="$WORK/plugins/stub-plugin/skills/$1"
  mkdir -p "$dir/evals"
  echo "# $1" >"$dir/SKILL.md"
  cat >"$dir/evals/evals.json" <<'JSON'
{"evals": [
  {"id": 1, "name": "first", "prompt": "do one", "assertions": ["does the thing"]},
  {"id": 2, "name": "second", "prompt": "do two", "assertions": ["does the thing"]}
]}
JSON
}
make_skill skill-a
make_skill skill-b

# A HOME with its own .claude, so the refusal check has something to compare
# against that the checkout is not under.
mkdir -p "$WORK/home/.claude"

# run_runner <modes> <args...>: sets OUT, RC; STUB_LOG is fresh per call.
run_runner() {
  local modes="$1"; shift
  export STUB_LOG="$WORK/stub.log"
  : >"$STUB_LOG"
  OUT=$(HOME="$WORK/home" PATH="$WORK/bin:$PATH" STUB_MODES="$modes" \
    RUN_EVALS_PLUGINS_DIR="$WORK/plugins" "$SCRIPT_DIR/run-evals.sh" "$@" 2>&1)
  RC=$?
}

expect_rc() { # name want
  if [ "$RC" -eq "$2" ]; then ok "$1 (exit $2)"; else not_ok "$1: want exit $2, got $RC" "$OUT"; fi
}
expect_out() { # name pattern
  if printf '%s\n' "$OUT" | grep -qE -- "$2"; then ok "$1"; else not_ok "$1: output lacks /$2/" "$OUT"; fi
}
expect_no_out() { # name pattern
  if printf '%s\n' "$OUT" | grep -qE -- "$2"; then not_ok "$1: output has /$2/" "$OUT"; else ok "$1"; fi
}

# --- one skill ----------------------------------------------------------------
run_runner "skill-a:pass" skill-a
expect_rc "graded run passes" 0
expect_out "graded run says so" "All assertions passed"

run_runner "skill-a:fail" skill-a
expect_rc "failing assertion fails" 1

run_runner "skill-a:none" skill-a
expect_rc "zero graded fails" 2
expect_out "zero graded is named" "NO EVALS GRADED"
expect_no_out "an executed session is not called not-executed" "DID NOT EXECUTE"

run_runner "skill-a:partial" skill-a
expect_rc "partly graded fails" 2
expect_out "partly graded is named" "only 1 of the 2 evals"

run_runner "skill-a:empty" skill-a
expect_rc "grading with no expectations fails" 2

run_runner "skill-a:plan" skill-a
expect_rc "plan-mode session is not-executed" 4
expect_out "not-executed block is printed" "NESTED SESSION DID NOT EXECUTE"
expect_out "mode in effect is reported" "Permission mode in effect: plan"
expect_out "requested mode is reported" "Permission mode requested: acceptEdits"

run_runner "skill-a:denied" skill-a
expect_rc "all-denied session is not-executed" 4
expect_out "denials are counted" "Permission denials: 2"

run_runner "skill-a:silent" skill-a
expect_rc "session with no transcript fails as ungraded" 2
expect_out "missing transcript is named" "left no transcript"

# --- how the session is started --------------------------------------------
run_runner "skill-a:pass" skill-a
LOG=$(head -n1 "$STUB_LOG")
# flag <name>: the value the runner passed after that flag, or empty.
flag() {
  python3 -c 'import json,sys
a = json.loads(sys.argv[1])["argv"]
print(a[a.index(sys.argv[2]) + 1] if sys.argv[2] in a else "")' "$LOG" "$1"
}
logged() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])' "$LOG" "$1"; }
if [ "$(flag --permission-mode)" = acceptEdits ]; then ok "permission mode acceptEdits is passed"
else not_ok "permission mode acceptEdits is passed" "$LOG"; fi
if [ "$(flag --allowedTools)" = Bash ]; then ok "Bash is the one allowed tool"
else not_ok "Bash is the one allowed tool" "$LOG"; fi
if [ "$(flag --output-format)" = stream-json ]; then ok "transcript is requested as stream-json"
else not_ok "transcript is requested as stream-json" "$LOG"; fi
if [ -n "$(flag --add-dir)" ] && [ "$(flag --add-dir)" = "$(logged tmpdir)" ]; then
  ok "--add-dir is the session's TMPDIR"
else not_ok "--add-dir is the session's TMPDIR" "$LOG"; fi
if [ "$(logged tmpdir_exists)" = True ] && [ ! -e "$(logged tmpdir)" ]; then
  ok "scratch dir exists during the session and is removed after"
else not_ok "scratch dir exists during the session and is removed after" "$LOG"; fi
if [ "$(logged cwd)" = "$(cd "$SCRIPT_DIR/.." && pwd -P)" ]; then
  ok "session runs from the repo root"
else not_ok "session runs from the repo root" "$LOG"; fi
if [ -s "$(find "$WORK/plugins/stub-plugin/skills/skill-a/evals/workspace" -name runner_session.jsonl | sort | tail -n1)" ]; then
  ok "transcript is kept in the iteration directory"
else not_ok "transcript is kept in the iteration directory"; fi

# --- --all ------------------------------------------------------------------
run_runner "skill-a:pass,skill-b:pass" --all
expect_rc "--all with every skill graded passes" 0
expect_out "--all says all passed" "All skills passed"

# The defect in issue 273: a skill that graded nothing was reported as passing.
run_runner "skill-a:pass,skill-b:none" --all
expect_rc "--all with a skill that graded nothing fails" 2
expect_out "--all names the ungraded skill" "No graded result for every eval: skill-b"
expect_no_out "--all does not claim success" "All skills passed"

run_runner "skill-a:fail,skill-b:pass" --all
expect_rc "--all with a failing assertion fails" 1
expect_out "--all names the failing skill" "Failed assertions: skill-a"

run_runner "skill-a:plan,skill-b:plan" --all
expect_rc "--all where no session executed" 4
expect_out "--all names skills whose session did not execute" "Nested session did not execute: skill-a skill-b"

run_runner "skill-a:fail,skill-b:none" --all
expect_rc "--all reports the more severe status" 2
expect_out "--all lists both causes (failed)" "Failed assertions: skill-a"
expect_out "--all lists both causes (ungraded)" "No graded result for every eval: skill-b"

# --- modes that start no session ------------------------------------------
run_runner "" --prep-only skill-a
if [ "$RC" -eq 0 ] && [ ! -s "$STUB_LOG" ]; then ok "--prep-only starts no session"
else not_ok "--prep-only starts no session" "$OUT"; fi
run_runner "" --validate skill-a
if [ ! -s "$STUB_LOG" ] && [ "$RC" -eq 2 ]; then ok "--validate starts no session and fails an ungraded iteration"
else not_ok "--validate starts no session and fails an ungraded iteration (exit $RC)" "$OUT"; fi

# --- a checkout under ~/.claude -------------------------------------------
fake_home="$WORK/claude-home"
mkdir -p "$fake_home/.claude/jobs/x/checkout/scripts"
cp "$SCRIPT_DIR/run-evals.sh" "$SCRIPT_DIR/classify-eval-session.py" "$fake_home/.claude/jobs/x/checkout/scripts/"
for args in "skill-a" "--all"; do
  : >"$STUB_LOG"
  # shellcheck disable=SC2086
  OUT=$(HOME="$fake_home" PATH="$WORK/bin:$PATH" RUN_EVALS_PLUGINS_DIR="$WORK/plugins" \
    "$fake_home/.claude/jobs/x/checkout/scripts/run-evals.sh" $args 2>&1)
  RC=$?
  if [ "$RC" -eq 5 ] && [ ! -s "$STUB_LOG" ]; then ok "refuses $args from a checkout under ~/.claude"
  else not_ok "refuses $args from a checkout under ~/.claude (exit $RC)" "$OUT"; fi
done
expect_out "refusal is named" "REFUSED, CHECKOUT UNDER ~/.claude"

echo ""
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
