#!/usr/bin/env bash
# Prompt regression eval harness for koto skills.
#
# Sends SKILL.md content plus a user prompt to the Anthropic Messages API,
# then checks the model response for expected koto command patterns.
#
# Usage:
#   ANTHROPIC_API_KEY=sk-... ./eval.sh [eval-dir ...]
#
# If no eval directories are given, all directories under evals/ are run.
#
# Each eval directory contains:
#   prompt.txt     -- the user prompt (e.g. "/hello-koto Hasami")
#   skill_path.txt -- path to a SKILL.md file (relative to repo root)
#                     OR
#   skill.txt      -- inline skill content (used if skill_path.txt is absent)
#   patterns.txt   -- one regex pattern per line; all must match the response
#
# Cost: Each eval case makes one Anthropic API call using claude-sonnet-4-20250514.
# At ~$3/M input tokens and ~$15/M output tokens, a single eval costs roughly
# $0.01-0.03 depending on SKILL.md length and response size. A PR touching
# plugins/ with 5 eval cases costs ~$0.05-0.15 per run.
#
# Exit codes:
#   0 -- all evals passed
#   1 -- one or more evals failed or configuration error

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# --- Configuration ---

MODEL="${KOTO_EVAL_MODEL:-claude-sonnet-4-20250514}"
MAX_TOKENS="${KOTO_EVAL_MAX_TOKENS:-1024}"
API_URL="https://api.anthropic.com/v1/messages"

# --- Validation ---

if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "ERROR: ANTHROPIC_API_KEY environment variable is required"
    exit 1
fi

# --- Helpers ---

# send_prompt sends a skill + user prompt to the Anthropic API and prints the
# model's text response to stdout. Returns non-zero on API error.
send_prompt() {
    local skill_content="$1"
    local user_prompt="$2"

    # Build the system message from the skill content.
    # Build the user message from the prompt.
    local payload
    payload=$(jq -n \
        --arg model "$MODEL" \
        --argjson max_tokens "$MAX_TOKENS" \
        --arg system "$skill_content" \
        --arg user "$user_prompt" \
        '{
            model: $model,
            max_tokens: $max_tokens,
            system: $system,
            messages: [
                { role: "user", content: $user }
            ]
        }')

    local response
    response=$(curl -s -w "\n%{http_code}" \
        -X POST "$API_URL" \
        -H "Content-Type: application/json" \
        -H "x-api-key: $ANTHROPIC_API_KEY" \
        -H "anthropic-version: 2023-06-01" \
        -d "$payload")

    # Split response body and HTTP status code.
    local http_code
    http_code=$(echo "$response" | tail -1)
    local body
    body=$(echo "$response" | sed '$d')

    if [ "$http_code" != "200" ]; then
        echo "API error (HTTP $http_code):" >&2
        echo "$body" >&2
        return 1
    fi

    # Extract text from the first content block.
    local text
    text=$(echo "$body" | jq -r '.content[0].text // empty')
    if [ -z "$text" ]; then
        echo "No text in API response:" >&2
        echo "$body" >&2
        return 1
    fi

    echo "$text"
}

# check_patterns checks that every pattern in patterns_file matches the
# response text. Prints pass/fail for each pattern. Returns non-zero if
# any pattern fails.
check_patterns() {
    local response="$1"
    local patterns_file="$2"
    local all_passed=0

    while IFS= read -r pattern || [ -n "$pattern" ]; do
        # Skip empty lines and comments.
        [[ -z "$pattern" || "$pattern" == \#* ]] && continue

        if echo "$response" | grep -qP "$pattern"; then
            echo "  PASS: /$pattern/"
        else
            echo "  FAIL: /$pattern/"
            echo "    Pattern not found in model response."
            all_passed=1
        fi
    done < "$patterns_file"

    return $all_passed
}

# --- Main ---

# Collect eval directories.
eval_dirs=()
if [ $# -gt 0 ]; then
    eval_dirs=("$@")
else
    if [ ! -d "$SCRIPT_DIR/evals" ]; then
        echo "No evals/ directory found at $SCRIPT_DIR/evals"
        exit 1
    fi
    for d in "$SCRIPT_DIR"/evals/*/; do
        [ -d "$d" ] && eval_dirs+=("$d")
    done
fi

if [ ${#eval_dirs[@]} -eq 0 ]; then
    echo "No eval cases found."
    exit 1
fi

total=0
passed=0
failed=0

for eval_dir in "${eval_dirs[@]}"; do
    eval_dir="${eval_dir%/}"
    eval_name="$(basename "$eval_dir")"
    total=$((total + 1))

    echo "=== Eval: $eval_name ==="

    # Validate required files.
    for required in prompt.txt patterns.txt; do
        if [ ! -f "$eval_dir/$required" ]; then
            echo "  ERROR: missing $required in $eval_dir"
            failed=$((failed + 1))
            continue 2
        fi
    done

    # Load user prompt.
    user_prompt=$(cat "$eval_dir/prompt.txt")

    # Load skill content: either from a path reference or from skill.txt.
    skill_content=""
    if [ -f "$eval_dir/skill_path.txt" ]; then
        skill_path=$(cat "$eval_dir/skill_path.txt")
        # Resolve relative to repo root.
        if [[ "$skill_path" != /* ]]; then
            skill_path="$REPO_ROOT/$skill_path"
        fi
        if [ ! -f "$skill_path" ]; then
            echo "  ERROR: skill file not found: $skill_path"
            failed=$((failed + 1))
            continue
        fi
        skill_content=$(cat "$skill_path")
    elif [ -f "$eval_dir/skill.txt" ]; then
        skill_content=$(cat "$eval_dir/skill.txt")
    else
        echo "  ERROR: need either skill_path.txt or skill.txt in $eval_dir"
        failed=$((failed + 1))
        continue
    fi

    echo "  Sending to $MODEL..."

    # Call the API.
    response=""
    if ! response=$(send_prompt "$skill_content" "$user_prompt"); then
        echo "  FAIL: API call failed"
        failed=$((failed + 1))
        continue
    fi

    echo "  Response received (${#response} chars)."
    echo ""
    echo "  --- Model Response (truncated) ---"
    echo "$response" | head -30 | sed 's/^/  | /'
    if [ "$(echo "$response" | wc -l)" -gt 30 ]; then
        echo "  | ... (truncated)"
    fi
    echo "  --- End Response ---"
    echo ""

    # Check patterns.
    if check_patterns "$response" "$eval_dir/patterns.txt"; then
        echo "  RESULT: PASS"
        passed=$((passed + 1))
    else
        echo ""
        echo "  RESULT: FAIL"
        failed=$((failed + 1))
    fi

    echo ""
done

echo "=== Summary: $passed/$total passed, $failed failed ==="

if [ "$failed" -gt 0 ]; then
    exit 1
fi
