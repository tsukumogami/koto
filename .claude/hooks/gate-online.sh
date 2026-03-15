#!/bin/bash
# Gate script for online operations.
# Runs as a PreToolUse hook on Bash commands. Works in bypassPermissions mode
# because hooks are external to the permission system.
#
# Exit behavior:
#   exit 0 with no output    → allow (default)
#   exit 0 with JSON decision → deny or ask per decision
#   exit 2                    → hard block
#
# Requires: jq

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

[ -z "$COMMAND" ] && exit 0

# --- DENY: block entirely ---
case "$COMMAND" in
    gh\ pr\ merge*|\
    gh\ repo\ delete*|\
    curl\ *|\
    wget\ *|\
    nc\ *|\
    ncat\ *|\
    netcat\ *)
        echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Blocked by gate-online hook"}}'
        exit 0
        ;;
esac

# --- ASK: prompt for confirmation ---
case "$COMMAND" in
    gh\ auth\ switch*|\
    gh\ release\ create*|\
    gh\ issue\ close*|\
    unset\ GH_TOKEN*)
        echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"Online operation requires confirmation"}}'
        exit 0
        ;;
esac

# Everything else: allow
exit 0
