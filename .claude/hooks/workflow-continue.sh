#!/usr/bin/env bash
set -euo pipefail

# Workflow continuation safety net for Claude Code Stop events.
#
# Checks if there's an active workflow state file with incomplete work.
# If so, nudges the agent with a non-blocking reminder about the controller.
# The agent decides whether to continue or stop -- this avoids infinite loops.
#
# Exit behavior:
#   exit 0 with no output         -> allow stop
#   exit 0 with JSON decision     -> block stop with reason
#
# Requires: jq

# Read hook input from stdin
INPUT=$(cat)

# Parse input fields
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || CWD=""
STOP_ACTIVE=$(echo "$INPUT" | jq -r '.stop_hook_active // empty' 2>/dev/null) || STOP_ACTIVE=""

# Not a stop event or no working directory -> allow
if [[ "$STOP_ACTIVE" != "true" ]] || [[ -z "$CWD" ]]; then
    exit 0
fi

# Find a state file in wip/
STATE_FILE=""
if [[ -d "$CWD/wip" ]]; then
    for f in "$CWD"/wip/*-state.json; do
        if [[ -f "$f" ]]; then
            STATE_FILE="$f"
            break
        fi
    done
fi

# No state file -> allow stop (no active workflow)
if [[ -z "$STATE_FILE" ]]; then
    exit 0
fi

# Check if the state file has incomplete work
HAS_PENDING=$(jq '
    [.issues[]? | select(.status != "completed" and .status != "ci_blocked")] | length > 0
' "$STATE_FILE" 2>/dev/null) || HAS_PENDING="false"

if [[ "$HAS_PENDING" != "true" ]]; then
    exit 0
fi

# There's incomplete work. Nudge the agent -- but give it agency.
REASON="It looks like there's an active workflow with incomplete issues ($(basename "$STATE_FILE")). If you meant to continue, you can call \`workflow-tool controller next\` to get the next step. If you're intentionally stopping (e.g., waiting for user input or hitting a blocker), go ahead."

echo "{\"decision\": \"block\", \"reason\": $(echo "$REASON" | jq -Rs .)}"
exit 0
