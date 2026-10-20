#!/usr/bin/env bash
# PreToolUse hook (Bash): tell koto which agent within the session is acting.
#
# koto's environment is the same for a session's main agent and every agent
# it starts, so koto can't tell them apart. The harness can: its PreToolUse
# payload carries `agent_id` (and `agent_type`) when the call comes from a
# sub-agent. For a Bash call that invokes `koto`, this hook rewrites the
# command so koto sees that identifier in KOTO_AGENT_ID, which koto records
# as the decider's agent, labelled `observed`.
#
# The rules, in order:
#   - Not a Bash call, no command, or a command that doesn't invoke koto:
#     no output, the call runs unchanged.
#   - `agent_id` present: set KOTO_AGENT_ID to it, but only when it matches
#     koto's identifier pattern (1-128 of letters, digits, `.`, `_`, `-`; not
#     starting with `.`; no `..`). Anything else (a space, a quote, `;`, `$`)
#     leaves the command unchanged, so koto records `unknown`.
#   - No `agent_id`, but the payload carries the `agent_type` key: the
#     harness is reporting agent context and says this call is not from a
#     sub-agent, so it is the main agent. Set KOTO_AGENT_ID=main.
#   - Neither key: the payload doesn't say (an older harness sends neither
#     for any agent), so set nothing. koto records `unknown` rather than
#     over-claim `main`.
#
# The value is single-quoted, and since it has passed the pattern it holds
# no quote to escape. It is exported ahead of the whole command, so it
# reaches koto in a pipeline or after `cd dir &&` as well as at the start.
#
# The rewritten input goes back as hookSpecificOutput.updatedInput with no
# permissionDecision, so the call still goes through the normal permission
# flow. This is attribution, not authentication: a command that reassigns
# KOTO_AGENT_ID itself, or hides koto behind another shell, defeats it.
#
# Best-effort: needs jq, and always exits 0. Any failure leaves the call
# unchanged.

set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0

payload="$(cat)" || exit 0

tool_name="$(printf '%s' "$payload" | jq -r '.tool_name // empty' 2>/dev/null)" || exit 0
[ "$tool_name" = "Bash" ] || exit 0

cmd="$(printf '%s' "$payload" | jq -r '.tool_input.command // empty' 2>/dev/null)" || exit 0
[ -n "$cmd" ] || exit 0

# Does the command invoke koto? `koto` (or a path ending in /koto), maybe
# quoted, as a word at the start of the command or after a shell separator.
koto_re='(^|[[:space:];&|(`"'"'"'])([^[:space:];&|()`"'"'"']*/)?koto([[:space:];&|)`"'"'"']|$)'
[[ "$cmd" =~ $koto_re ]] || exit 0

is_identifier() {
  local v="$1"
  [ "${#v}" -ge 1 ] && [ "${#v}" -le 128 ] || return 1
  [[ "$v" =~ ^[A-Za-z0-9._-]+$ ]] || return 1
  [[ "$v" != .* ]] || return 1
  [[ "$v" != *..* ]] || return 1
  return 0
}

has_agent_id="$(printf '%s' "$payload" | jq -r 'has("agent_id") and .agent_id != null' 2>/dev/null)" || exit 0
has_agent_type="$(printf '%s' "$payload" | jq -r 'has("agent_type")' 2>/dev/null)" || exit 0

agent=""
if [ "$has_agent_id" = "true" ]; then
  # Only a string can be an identifier; a number or object is not one.
  is_string="$(printf '%s' "$payload" | jq -r '.agent_id | type == "string"' 2>/dev/null)" || exit 0
  [ "$is_string" = "true" ] || exit 0
  raw="$(printf '%s' "$payload" | jq -r '.agent_id' 2>/dev/null)" || exit 0
  is_identifier "$raw" || exit 0
  agent="$raw"
elif [ "$has_agent_type" = "true" ]; then
  agent="main"
else
  exit 0
fi

new_cmd="export KOTO_AGENT_ID='${agent}'; ${cmd}"

printf '%s' "$payload" | jq -c --arg cmd "$new_cmd" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    updatedInput: (.tool_input + {command: $cmd})
  }
}' 2>/dev/null || exit 0

exit 0
