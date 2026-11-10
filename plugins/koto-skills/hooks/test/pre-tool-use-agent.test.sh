#!/usr/bin/env bash
# Tests for hooks/pre-tool-use-agent.sh, on a fixed payload matrix.
#
#   pre-tool-use-agent.test.sh
#
# Every case runs the hook under `env -i`, so the only environment it sees is
# the PATH, AI_AGENT and CLAUDE_CODE_SESSION_ATTENDED the case gives it; a
# copy of the test runner's own harness variables can't leak in. The PATH is
# a directory of symlinks to bash, cat and jq plus a stub `koto`, or the same
# without jq, or without koto.
#
# A case either expects a rewrite or expects silence. For a rewrite, the hook
# must exit 0 and print one JSON object whose only key is hookSpecificOutput,
# holding exactly hookEventName PreToolUse and updatedInput, and updatedInput
# must equal the payload's tool_input with `command` replaced by the expected
# prefix followed by the original command. For silence, the hook must exit 0
# and print nothing.
#
# Some rewrite cases then run the rewritten command with bash, in a scratch
# directory whose stub koto writes the KOTO_AGENT_ID and KOTO_RUN_MODE it
# was started with, and check that koto saw exactly the prefix's values.
# Hostile payload values carry `$(...)` and backticks that would create a
# sentinel file if anything evaluated them; no case may create it.
#
# The suite exits 1 if any case fails.
#
# shellcheck disable=SC2016
# (payloads hold `$(` and backticks that must reach the hook unexpanded.)

set -u
export LC_ALL=C

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/../pre-tool-use-agent.sh"
NAME="pre-tool-use-agent.test.sh"

if ! command -v jq >/dev/null 2>&1; then
  echo "$NAME: jq is not on the PATH; the suite needs it to build and read payloads" >&2
  exit 2
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

SENT="$TMP/sentinel"
SEEN="$TMP/seen"

# Three PATHs: full (bash, cat, jq, koto), no-jq, no-koto.
mkdir "$TMP/full" "$TMP/nojq" "$TMP/nokoto" "$TMP/run"
for c in bash cat jq; do
  p="$(command -v "$c")" || {
    echo "$NAME: $c is not on the PATH" >&2
    exit 2
  }
  ln -s "$p" "$TMP/full/$c"
  [ "$c" = jq ] || ln -s "$p" "$TMP/nojq/$c"
  ln -s "$p" "$TMP/nokoto/$c"
done
printf '#!%s\nprintf "%%s|%%s\\n" "${KOTO_AGENT_ID-<unset>}" "${KOTO_RUN_MODE-<unset>}" >>"%s"\n' \
  "$(command -v bash)" "$SEEN" >"$TMP/koto"
chmod +x "$TMP/koto"
ln -s "$TMP/koto" "$TMP/full/koto"
ln -s "$TMP/koto" "$TMP/nojq/koto"
ln -s "$TMP/koto" "$TMP/run/koto"
# The rewritten commands run with the full PATH, and some start with `env`.
ln -s "$(command -v env)" "$TMP/full/env"

NEW='claude-code_2-1-322_harness'
OLD='claude-code_2-1-68_harness'

failures=0
passed=0

# payload COMMAND [AGENT_ID_JSON]: a PreToolUse Bash payload. AGENT_ID_JSON is
# a JSON value for agent_id, or empty for none.
payload() {
  jq -cn --arg cmd "$1" --argjson aid "${2:-null}" --arg has "${2:+1}" '
    {session_id: "s-1", cwd: "/tmp", hook_event_name: "PreToolUse",
     tool_name: "Bash", tool_input: {command: $cmd, description: "d"},
     tool_use_id: "toolu_1"}
    + (if $has == "1" then {agent_id: $aid} else {} end)'
}

# run_hook PATHDIR AI_AGENT ATTENDED PAYLOAD: sets $out to the hook's stdout
# and $rc to its exit status.
run_hook() {
  printf '%s' "$4" | env -i PATH="$1" AI_AGENT="$2" CLAUDE_CODE_SESSION_ATTENDED="$3" \
    "$TMP/full/bash" "$HOOK" >"$TMP/out"
  rc=$?
  out="$(cat "$TMP/out")"
}

fail() {
  failures=$((failures + 1))
  echo "FAIL [$1] $2"
}

# expect_silent NAME PAYLOAD [AI_AGENT] [ATTENDED] [PATHDIR]
expect_silent() {
  run_hook "${5:-$TMP/full}" "${3-$NEW}" "${4-1}" "$2"
  if [ "$rc" -ne 0 ]; then
    fail "$1" "exit $rc, want 0"
  elif [ -n "$out" ]; then
    fail "$1" "printed $out, want nothing"
  else
    passed=$((passed + 1))
  fi
}

# expect_prefix NAME PAYLOAD AI_AGENT ATTENDED PREFIX [SEEN_LINE]
# SEEN_LINE, when given, is what the stub koto must record when the rewritten
# command runs.
expect_prefix() {
  local cmd
  run_hook "$TMP/full" "$3" "$4" "$2"
  if [ "$rc" -ne 0 ]; then
    fail "$1" "exit $rc, want 0"
    return
  fi
  if ! jq -e --argjson in "$2" --arg p "$5" '
      (keys == ["hookSpecificOutput"])
      and (.hookSpecificOutput | keys == ["hookEventName", "updatedInput"])
      and .hookSpecificOutput.hookEventName == "PreToolUse"
      and .hookSpecificOutput.updatedInput == ($in.tool_input | .command = $p + .command)
    ' <<<"$out" >/dev/null 2>&1; then
    fail "$1" "output $out, want the command prefixed with '$5'"
    return
  fi
  if [ $# -ge 6 ]; then
    cmd="$(jq -j '.hookSpecificOutput.updatedInput.command' <<<"$out")"
    rm -f "$SEEN"
    (cd "$TMP/run" && env -i PATH="$TMP/full" "$TMP/full/bash" -c "$cmd" >/dev/null 2>&1)
    if [ "$(head -n 1 "$SEEN" 2>/dev/null)" != "$6" ]; then
      fail "$1" "koto saw '$(head -n 1 "$SEEN" 2>/dev/null)', want '$6'"
      return
    fi
  fi
  passed=$((passed + 1))
}

# --- The agent ---------------------------------------------------------------

expect_prefix sub-agent "$(payload 'koto next wf' '"sub-1"')" "" "" \
  'KOTO_AGENT_ID=sub-1 ' 'sub-1|<unset>'
expect_prefix sub-agent-harness-attended "$(payload 'koto next wf' '"a2bda8d7dba15842f"')" "$NEW" 1 \
  'KOTO_AGENT_ID=a2bda8d7dba15842f KOTO_RUN_MODE=attended ' 'a2bda8d7dba15842f|attended'
expect_prefix sub-agent-old-harness "$(payload 'koto next wf' '"sub-1"')" "$OLD" 1 \
  'KOTO_AGENT_ID=sub-1 '
expect_prefix main-attended "$(payload 'koto next wf')" "$NEW" 1 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=attended ' 'main|attended'
expect_prefix main-unattended "$(payload 'koto next wf')" "$NEW" 0 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=unattended ' 'main|unattended'
expect_prefix main-run-mode-unreported "$(payload 'koto next wf')" "$NEW" "" \
  'KOTO_AGENT_ID=main ' 'main|<unset>'
expect_prefix main-run-mode-odd "$(payload 'koto next wf')" "$NEW" "yes" \
  'KOTO_AGENT_ID=main '
expect_prefix main-exactly-2-1-69 "$(payload 'koto next wf')" 'claude-code_2-1-69_harness' 0 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=unattended '
expect_prefix main-3-0-0 "$(payload 'koto next wf')" 'claude-code_3-0-0_harness' 1 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=attended '
expect_prefix agent-id-255 "$(payload 'koto x' "\"$(printf 'a%.0s' $(seq 255))\"")" "" "" \
  "KOTO_AGENT_ID=$(printf 'a%.0s' $(seq 255)) "
expect_prefix agent-id-dots "$(payload 'koto x' '"Sub_2.b-c"')" "" "" 'KOTO_AGENT_ID=Sub_2.b-c '
# A sub-agent's id needs no harness check, but its run mode does.
expect_prefix sub-agent-no-ai-agent-attended "$(payload 'koto next wf' '"sub-1"')" "" 1 \
  'KOTO_AGENT_ID=sub-1 ' 'sub-1|<unset>'
expect_prefix main-2-2-0 "$(payload 'koto next wf')" 'claude-code_2-2-0_harness' 1 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=attended '
expect_prefix main-zero-padded "$(payload 'koto next wf')" 'claude-code_02-01-069_harness' 1 \
  'KOTO_AGENT_ID=main KOTO_RUN_MODE=attended '
expect_prefix trailing-newlines-kept "$(payload $'koto next wf\n\n' '"sub-1"')" "" "" 'KOTO_AGENT_ID=sub-1 '

# Ambiguous: nothing says the caller is the main agent.
expect_silent main-no-ai-agent "$(payload 'koto next wf')" ""
expect_silent main-old-harness "$(payload 'koto next wf')" "$OLD"
expect_silent main-2-0-999 "$(payload 'koto next wf')" 'claude-code_2-0-999_harness'
expect_silent main-shell-copy "$(payload 'koto next wf')" 'claude-code_2-1-322_agent'
expect_silent main-ai-agent-junk "$(payload 'koto next wf')" 'claude-code_2-1-322_harness; touch x'
expect_silent main-ai-agent-other "$(payload 'koto next wf')" 'cursor_1-0-0_harness'
expect_silent main-1-9-99 "$(payload 'koto next wf')" 'claude-code_1-9-99_harness'
expect_silent main-no-suffix "$(payload 'koto next wf')" 'claude-code_2-1-322'
expect_silent main-huge-component "$(payload 'koto next wf')" 'claude-code_2-1-99999999999999999999_harness'

# Invalid agent ids change nothing, even when the harness check passes.
for bad in '"-lead"' '".lead"' '""' '"a b"' '"a;b"' '"a\nb"' '"sub-1\n"' '"a/b"' '"a$b"' \
  '"$(touch SENT)"' '"`touch SENT`"' '"a'"'"'b"' '"é"' 'null' '7' '["a"]' '{"a":1}' \
  "\"$(printf 'a%.0s' $(seq 256))\""; do
  expect_silent "bad-agent-id ${bad:0:24}" "$(payload 'koto next wf' "${bad//SENT/$SENT}")"
done

# --- Commands that don't run koto ------------------------------------------

for cmd in 'ls -la' 'echo koto' 'kotox next' 'koto-like x' 'ko to' 'xkoto' 'mykoto/kotoo' \
  '/usr/bin/kotox next' 'koto.sh next' \
  'cd x && koto next' '(koto next)' '{ koto next; }' 'time koto next' 'nohup koto next' \
  'exec koto next' 'command koto next' 'sudo koto next' '! koto next' 'env -i koto next' \
  'env -u X koto next' 'FOO="a b" koto next' 'FOO=$(id) koto next' 'FOO=`id` koto next' \
  'FOO=$HOME koto next' "FOO='x' koto next" 'FOO=a\ b koto next' 'FOO=* koto next' \
  'FOO=~/x koto next' '"ko"to next' 'k\oto next' '$(echo koto) next' 'koto>out' \
  'koto(x)' '# koto next' '' ' ' 'KOTO_AGENT_ID=x koto next' 'KOTO_RUN_MODE=attended koto next' \
  'koto next; echo $KOTO_AGENT_ID'; do
  expect_silent "not-koto: $cmd" "$(payload "$cmd" '"sub-1"')"
done
expect_silent "leading newline" "$(payload $'\nkoto next' '"sub-1"')"
expect_silent "carriage return after koto" "$(payload $'koto\r\nnext' '"sub-1"')"
expect_silent "nul in command" '{"tool_name":"Bash","tool_input":{"command":"koto next\u0000"},"agent_id":"sub-1"}'

# --- Commands that do run koto -----------------------------------------------

P='KOTO_AGENT_ID=sub-1 KOTO_RUN_MODE=attended '
S='sub-1|attended'
# koto_case runs the rewritten command too; koto_case_text only checks the
# text, for commands whose own arguments would run `$(...)` when executed.
koto_case() { expect_prefix "koto: $1" "$(payload "$1" '"sub-1"')" "$NEW" 1 "$P" "$S"; }
koto_case_text() { expect_prefix "koto: $1" "$(payload "$1" '"sub-1"')" "$NEW" 1 "$P"; }
koto_case 'koto'
koto_case 'koto next wf'
koto_case '  koto next wf'
koto_case $'\tkoto next wf'
koto_case 'koto next wf | cat'
koto_case 'koto next wf|cat'
koto_case 'koto next wf && echo done'
koto_case 'koto next wf; koto status wf'
koto_case 'koto next wf & wait'
koto_case 'koto next wf;true'
koto_case_text 'koto next wf&'
koto_case 'koto a && koto b'
koto_case_text '/usr/local/bin/koto next wf'
koto_case_text 'koto next "a b" '"'"'c\d'"'"' e\ f'
koto_case 'koto next wf 2>&1 | cat'
koto_case '"koto" next wf'
koto_case "'koto' next wf"
koto_case 'env koto next wf'
koto_case 'env FOO=1 koto next wf'
koto_case 'FOO=1 BAR=x/y:z koto next wf'
koto_case 'FOO= koto next wf'
koto_case 'FOO=1 env BAR=2 koto next wf'
koto_case "$TMP/koto next wf"
koto_case './koto next wf'
koto_case_text 'koto next "$(touch '"$SENT"')" `touch '"$SENT"'`'
koto_case $'koto next wf\nkoto status wf'
koto_case_text $'koto context add wf k <<\'EOF\'\n$(touch '"$SENT"')\nEOF'
koto_case 'koto decisions record wf --with-data '"'"'{"choice":"a;b|c&d$(x)`y`"}'"'"''

# Fields beside command survive untouched, including hostile ones.
expect_prefix "extra tool_input fields" \
  "$(jq -cn '{tool_name:"Bash",agent_id:"sub-1",
     tool_input:{command:"koto next",timeout:5000,run_in_background:true,description:"$(x) `y`; z"}}')" \
  "" "" 'KOTO_AGENT_ID=sub-1 '

# --- The call isn't a Bash call, or can't be read ----------------------------

expect_silent "not bash" '{"tool_name":"Read","tool_input":{"command":"koto next"},"agent_id":"sub-1"}'
expect_silent "no tool_input" '{"tool_name":"Bash","agent_id":"sub-1"}'
expect_silent "command not a string" '{"tool_name":"Bash","tool_input":{"command":["koto"]},"agent_id":"sub-1"}'
expect_silent "payload not an object" '["Bash"]'
expect_silent "malformed json" '{"tool_name":"Bash","tool_input":{"command":"koto next"'
expect_silent "empty payload" ''

# --- Missing tools -------------------------------------------------------------

expect_silent "koto not installed" "$(payload 'koto next wf' '"sub-1"')" "$NEW" 1 "$TMP/nokoto"
expect_silent "koto not installed, main" "$(payload 'koto next wf')" "$NEW" 1 "$TMP/nokoto"
expect_silent "jq not installed" "$(payload 'koto next wf' '"sub-1"')" "$NEW" 1 "$TMP/nojq"

if [ -e "$SENT" ]; then
  fail "sentinel" "a payload value was evaluated: $SENT exists"
fi

echo "$NAME: $passed passed, $failures failed"
[ "$failures" -eq 0 ]
