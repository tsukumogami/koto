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
#     sub-agent, so it is the main agent. Set KOTO_AGENT_ID=main. The
#     harness's hook reference says a sub-agent's tool call always carries
#     `agent_id`, and that `agent_type` is sent for a session started with
#     a named agent; no field names the main agent outright, so this is the
#     only payload that says so.
#   - Neither key: the payload doesn't say (an older harness sends neither
#     for any agent), so set nothing. koto records `unknown` rather than
#     over-claim `main`.
#
# The value is single-quoted, and since it has passed the pattern it holds
# no quote to escape. It is exported ahead of the whole command, so it
# reaches koto in a pipeline or after `cd dir &&` as well as at the start.
# An inline `KOTO_AGENT_ID=... cmd` prefix would reach only the first simple
# command. The harness runs each Bash call with a fresh environment, so the
# export doesn't carry over to a later call; a harness that kept one shell's
# environment across calls could show koto a stale value on a call this hook
# leaves unchanged.
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

# One jq call decides the agent: the sub-agent's id when it is a string
# that passes koto's identifier rule (`is_identifier` in
# src/decision_store/decider.rs; keep the two in step), `main` when only
# agent_type is present, and nothing otherwise. The check runs inside jq on
# the raw string, with \A and \z anchors, so a trailing newline can't be
# stripped into a valid-looking value on the way out.
agent="$(printf '%s' "$payload" | jq -r '
  if (has("agent_id") and .agent_id != null) then
    .agent_id
    | if type == "string"
         and length <= 128
         and test("\\A[A-Za-z0-9._-]+\\z")
         and (startswith(".") | not)
         and (contains("..") | not)
      then . else empty end
  elif has("agent_type") then "main"
  else empty end' 2>/dev/null)" || exit 0
[ -n "$agent" ] || exit 0

new_cmd="export KOTO_AGENT_ID='${agent}'; ${cmd}"

printf '%s' "$payload" | jq -c --arg cmd "$new_cmd" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    updatedInput: (.tool_input + {command: $cmd})
  }
}' 2>/dev/null || exit 0

exit 0
