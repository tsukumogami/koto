#!/usr/bin/env bash
# PreToolUse hook (matcher Bash): tell koto which agent is running it.
#
# A decision koto records names the agent inside the deciding session and
# the run mode, and koto can't see either: both live in the harness. This
# hook reads them from the PreToolUse payload and environment and, on a Bash
# call that runs the `koto` binary, puts them in front of the command as
# environment assignments:
#
#   KOTO_AGENT_ID=<agent>[ KOTO_RUN_MODE=<mode>] <command, byte for byte>
#
# The assignments reach only the first simple command, the koto call, and
# no later child. The hook:
#
#   1. gives up unless jq and koto are both on the PATH, the payload is a
#      Bash call whose `tool_input.command` is a string, and that command
#      names neither KOTO_AGENT_ID nor KOTO_RUN_MODE already;
#   2. takes the agent from `agent_id`, which the harness sends only from
#      inside an agent the main agent started, when it matches koto's
#      identifier pattern. With no `agent_id`, the agent is `main` only when
#      `AI_AGENT` shows a harness of at least 2.1.69, the release that
#      added `agent_id` to hook input, and shows that the harness set it for
#      this hook (the `_harness` suffix; a shell's copy ends `_agent`).
#      Anything else names no agent, and the hook changes nothing;
#   3. takes the run mode from `CLAUDE_CODE_SESSION_ATTENDED`, which the
#      harness sets for every hook to 1 or 0, under the same `AI_AGENT`
#      check: 1 is attended, 0 unattended, anything else no run mode;
#   4. rewrites the command only when its first simple command runs koto:
#      optional plain `NAME=value` assignments, an optional bare `env` with
#      more of them, then `koto`, `"koto"`, `'koto'` or a plain path ending
#      in `/koto`, then the end of the line, a blank, `;`, `|` or `&`.
#      Anything it can't classify (a `cd` first, a subshell, a group,
#      `time`, `sudo`, quoting or expansion in an assignment) is left alone;
#   5. prints `updatedInput` with the whole `tool_input`, only `command`
#      changed, and no permission decision, so the usual permission checks
#      still run on the rewritten command.
#
# It exits 0 and prints nothing whenever it changes nothing.

trap 'exit 0' EXIT
exec 2>/dev/null
export LC_ALL=C

command -v jq >/dev/null 2>&1 || exit 0
command -v koto >/dev/null 2>&1 || exit 0

payload="$(cat)"

# One jq pass returns three lines: whether the call is a candidate, the
# agent (`absent`, `bad`, or `id:<id>` with the id already pattern-checked)
# and the command's first line, which is all the matcher reads.
fields="$(printf '%s' "$payload" | jq -r '
  def is_id: type == "string" and length <= 255
    and test("\\A[a-zA-Z0-9][a-zA-Z0-9._-]*\\z");
  if type == "object" and .tool_name == "Bash"
     and (.tool_input | type) == "object"
     and (.tool_input.command | type) == "string"
     and (.tool_input.command | test("KOTO_AGENT_ID|KOTO_RUN_MODE") | not)
     and (.tool_input.command | explode | any(. == 0) | not)
  then
    "go",
    (if has("agent_id") | not then "absent"
     elif .agent_id | is_id then "id:" + .agent_id
     else "bad" end),
    (.tool_input.command | split("\n") | .[0] // "")
  else "no" end
')" || exit 0

{
  IFS= read -r candidate
  IFS= read -r agent_field
  IFS= read -r first_line
} <<<"$fields"

[ "$candidate" = "go" ] || exit 0

id_re='^[a-zA-Z0-9][a-zA-Z0-9._-]*$'

# The harness that runs this hook, when it says so itself.
harness_ok=0
if [[ "${AI_AGENT:-}" =~ ^claude-code_([0-9]{1,6})-([0-9]{1,6})-([0-9]{1,6})_harness$ ]]; then
  major=$((10#${BASH_REMATCH[1]}))
  minor=$((10#${BASH_REMATCH[2]}))
  patch=$((10#${BASH_REMATCH[3]}))
  if [ "$major" -gt 2 ] ||
    { [ "$major" -eq 2 ] && [ "$minor" -gt 1 ]; } ||
    { [ "$major" -eq 2 ] && [ "$minor" -eq 1 ] && [ "$patch" -ge 69 ]; }; then
    harness_ok=1
  fi
fi

case "$agent_field" in
  id:*)
    agent="${agent_field#id:}"
    ;;
  absent)
    [ "$harness_ok" -eq 1 ] || exit 0
    agent="main"
    ;;
  *)
    exit 0
    ;;
esac
[[ "$agent" =~ $id_re ]] && [ "${#agent}" -le 255 ] || exit 0

run_mode=""
if [ "$harness_ok" -eq 1 ]; then
  case "${CLAUDE_CODE_SESSION_ATTENDED:-}" in
    1) run_mode="attended" ;;
    0) run_mode="unattended" ;;
  esac
fi

# The first simple command must run koto. Values in leading assignments use
# only characters that mean nothing to the shell, so nothing in them can
# expand, quote or end the command early.
assign='[A-Za-z_][A-Za-z0-9_]*=[A-Za-z0-9_./:@%+,=-]*[ 	]+'
koto_word="(koto|\"koto\"|'koto'|~?[A-Za-z0-9_./+-]*/koto)"
koto_re="^[ 	]*(${assign})*(env[ 	]+(${assign})*)?${koto_word}([ 	;|&]|\$)"
[[ "$first_line" =~ $koto_re ]] || exit 0

prefix="KOTO_AGENT_ID=$agent "
[ -n "$run_mode" ] && prefix+="KOTO_RUN_MODE=$run_mode "

printf '%s' "$payload" | jq -c --arg p "$prefix" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    updatedInput: (.tool_input | .command = $p + .command)
  }
}'
