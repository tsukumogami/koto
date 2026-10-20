#!/usr/bin/env bash
# Tests for pre-tool-use-agent-id.sh, the PreToolUse hook that sets
# KOTO_AGENT_ID for koto calls.
#
# Usage: bash plugins/koto-skills/hooks/pre-tool-use-agent-id_test.sh
#
# Needs jq. When KOTO_BIN names a koto binary (or `koto` is on PATH), it
# also runs a rewritten command against a throwaway HOME and checks that the
# decision it records lists the sub-agent as `observed`.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
hook="$here/pre-tool-use-agent-id.sh"

command -v jq >/dev/null 2>&1 || { echo "FAIL: jq is required" >&2; exit 1; }

pass=0
fail=0

ok() { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1" >&2; }

# payload <command> [extra-jq-object]
payload() {
  jq -nc --arg cmd "$1" --argjson extra "${2:-{\}}" '{
    session_id: "s-1",
    hook_event_name: "PreToolUse",
    tool_name: "Bash",
    tool_input: {command: $cmd, description: "d", timeout: 5000}
  } + $extra'
}

run_hook() { printf '%s' "$1" | bash "$hook"; }

# expect_unchanged <name> <payload>
expect_unchanged() {
  local out status
  out="$(run_hook "$2")"
  status=$?
  if [ "$status" -eq 0 ] && [ -z "$out" ]; then ok "$1"; else bad "$1 (status $status, output: $out)"; fi
}

# expect_command <name> <payload> <expected-command>
expect_command() {
  local out got
  out="$(run_hook "$2")"
  got="$(printf '%s' "$out" | jq -r '.hookSpecificOutput.updatedInput.command // empty' 2>/dev/null)"
  if [ "$got" = "$3" ]; then ok "$1"; else bad "$1 (want: $3, got: $got)"; fi
}

# Sub-agent: the payload's agent_id, quoted.
expect_command "sub-agent koto call gets its agent id" \
  "$(payload 'koto next wf' '{"agent_id":"a1b2c3-d4","agent_type":"Explore"}')" \
  "export KOTO_AGENT_ID='a1b2c3-d4'; koto next wf"

# The rest of tool_input survives the rewrite, and no permission decision is made.
out="$(run_hook "$(payload 'koto next wf' '{"agent_id":"a1"}')")"
if printf '%s' "$out" | jq -e '.hookSpecificOutput.hookEventName == "PreToolUse"
    and .hookSpecificOutput.updatedInput.description == "d"
    and .hookSpecificOutput.updatedInput.timeout == 5000
    and (.hookSpecificOutput | has("permissionDecision") | not)' >/dev/null 2>&1; then
  ok "rewrite keeps other tool_input fields and makes no permission decision"
else
  bad "rewrite keeps other tool_input fields and makes no permission decision (output: $out)"
fi

# koto later in the command, or by path.
expect_command "koto after cd && is covered" \
  "$(payload 'cd /tmp && koto decisions list' '{"agent_id":"a1"}')" \
  "export KOTO_AGENT_ID='a1'; cd /tmp && koto decisions list"
expect_command "koto in a pipeline is covered" \
  "$(payload 'koto next wf | jq .action' '{"agent_id":"a1"}')" \
  "export KOTO_AGENT_ID='a1'; koto next wf | jq .action"
expect_command "koto by path is covered" \
  "$(payload './target/release/koto next wf' '{"agent_id":"a1"}')" \
  "export KOTO_AGENT_ID='a1'; ./target/release/koto next wf"

# Main agent: only when the payload positively says it isn't a sub-agent.
expect_command "agent_type without agent_id is the main agent" \
  "$(payload 'koto next wf' '{"agent_type":"reviewer"}')" \
  "export KOTO_AGENT_ID='main'; koto next wf"
expect_command "empty agent_type without agent_id is the main agent" \
  "$(payload 'koto next wf' '{"agent_type":""}')" \
  "export KOTO_AGENT_ID='main'; koto next wf"

# Ambiguous: neither field, or a null agent_id with no agent_type.
expect_unchanged "payload with no agent fields is left unchanged" \
  "$(payload 'koto next wf')"
expect_unchanged "null agent_id without agent_type is left unchanged" \
  "$(payload 'koto next wf' '{"agent_id":null}')"

# Invalid identifiers leave the command unchanged.
for bad_id in 'has space' "has'quote" 'semi;colon' 'dollar$x' '$(id)' '.hidden' 'a..b' 'a/b' ''; do
  expect_unchanged "invalid agent_id [$bad_id] is left unchanged" \
    "$(payload 'koto next wf' "$(jq -nc --arg id "$bad_id" '{agent_id: $id}')")"
done
expect_unchanged "agent_id longer than 128 is left unchanged" \
  "$(payload 'koto next wf' "$(jq -nc --arg id "$(printf 'a%.0s' $(seq 1 129))" '{agent_id: $id}')")"
expect_unchanged "numeric agent_id is left unchanged" \
  "$(payload 'koto next wf' '{"agent_id":42}')"
expect_command "agent_id of exactly 128 is accepted" \
  "$(payload 'koto next wf' "$(jq -nc --arg id "$(printf 'a%.0s' $(seq 1 128))" '{agent_id: $id}')")" \
  "export KOTO_AGENT_ID='$(printf 'a%.0s' $(seq 1 128))'; koto next wf"

# Calls that don't invoke koto.
expect_unchanged "non-koto Bash call is left unchanged" \
  "$(payload 'ls -la' '{"agent_id":"a1"}')"
expect_unchanged "a word containing koto is not koto" \
  "$(payload 'cat kotonoha.txt mykoto' '{"agent_id":"a1"}')"
expect_unchanged "non-Bash tool is left unchanged" \
  "$(jq -nc '{tool_name: "Read", tool_input: {file_path: "koto"}, agent_id: "a1"}')"
expect_unchanged "malformed payload is left unchanged" "not json"

# End to end: a decision recorded through a rewritten command lists the
# sub-agent as observed.
koto_bin="${KOTO_BIN:-$(command -v koto 2>/dev/null || true)}"
if [ -n "$koto_bin" ] && [ -x "$koto_bin" ]; then
  tmp_home="$(mktemp -d)"
  trap 'rm -rf "$tmp_home"' EXIT
  rec_cmd="'$koto_bin' decisions record --kind judgment --what 'hook test' --by session:hook-test-session --reason 'hook test'"
  rewritten="$(run_hook "$(payload "$rec_cmd" '{"agent_id":"sub-agent-7"}')" \
    | jq -r '.hookSpecificOutput.updatedInput.command // empty')"
  if [ -z "$rewritten" ]; then
    bad "end to end: hook did not rewrite the koto call"
  elif ! (unset KOTO_AGENT_ID; HOME="$tmp_home" bash -c "$rewritten" >"$tmp_home/err" 2>&1); then
    bad "end to end: koto decisions record failed: $(cat "$tmp_home/err")"
  else
    listing="$(env -u KOTO_AGENT_ID HOME="$tmp_home" "$koto_bin" decisions list --kind judgment 2>/dev/null)"
    if printf '%s' "$listing" | grep -q 'sub-agent-7' \
      && printf '%s' "$listing" | jq -e '[.. | objects | select(.value? == "sub-agent-7" and .source? == "observed")] | length > 0' >/dev/null 2>&1; then
      ok "end to end: the decision lists the sub-agent as observed"
    else
      bad "end to end: listing does not show sub-agent-7 as observed: $listing"
    fi
  fi
else
  echo "SKIP: end to end (no koto binary; set KOTO_BIN)"
fi

echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
