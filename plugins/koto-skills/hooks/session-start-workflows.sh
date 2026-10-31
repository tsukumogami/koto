#!/usr/bin/env bash
# SessionStart hook script, not registered: enable koto's native Claude Code
# /workflows rendering.
#
# Not registered in hooks/hooks.json, so Claude Code never runs it: registering
# it would start a command at every session start for every koto-skills user.
# The rest of this header describes what it does when run as a SessionStart
# hook.
#
# Claude Code's /workflows screen renders `*.json` files under
# `<projectDir>/<sessionId>/workflows/`. koto cannot self-discover that
# per-session directory, so this script -- which receives `session_id` and
# `transcript_path` on stdin -- derives it and hands it to koto through the
# `KOTO_WORKFLOWS_DIR` environment variable. koto sessions driven in this
# Claude Code session then materialize their own `koto-<uuid>.json` into that
# directory on each state-commit (see the koto-user skill and
# docs/designs/DESIGN-native-workflows-render.md).
#
# The directory is `<dirname of transcript_path>/<session_id>/workflows`, or
# `<cwd>/<session_id>/workflows` when transcript_path is absent or empty. The
# script reads the payload with jq when jq is on the PATH, and otherwise with
# bash, cat, dirname, grep, sed and head alone; the two paths print the same
# output. Without jq, a string field may hold the escapes \", \\ and \/; any
# other escape refuses the field. Input rules, each of which makes the script
# print nothing:
#
#   - no session_id, or one outside [A-Za-z0-9_-]+ (stdin that isn't JSON has
#     none), so the id can't add path components;
#   - a refused session_id, transcript_path or cwd, with no fallback to another
#     field, since a fallback would name a directory Claude Code doesn't use;
#   - a control character (below U+0020) or DEL in the directory.
#
# The context names the directory unquoted in its sentence and single-quotes
# it in the export line (an embedded ' written '\''), so a directory holding
# `$(`, a backtick or a quote runs nothing when the line is evaluated. Without
# jq, json_escape escapes the context before it is written into the JSON.
#
# The hook is best-effort and always exits 0: a parse failure or a missing
# field disables the feature silently rather than disrupting the session.

set -euo pipefail

# Byte-wise matching for bash patterns, grep and sed.
LC_ALL=C
export LC_ALL

CONTROL_CHARS=$'\001\002\003\004\005\006\007\010\011\012\013\014\015\016\017\020\021\022\023\024\025\026\027\030\031\032\033\034\035\036\037\177'

payload="$(cat)"

have_jq=0
if command -v jq >/dev/null 2>&1; then
  have_jq=1
fi

# decode_json_string RAW: sets FIELD to RAW (a JSON string's contents, without
# its quotes) with \", \\ and \/ decoded in one pass. Returns 2 for any other
# escape.
decode_json_string() {
  local raw="$1" i c n
  FIELD=""
  for ((i = 0; i < ${#raw}; i++)); do
    c="${raw:i:1}"
    if [ "$c" = "\\" ]; then
      i=$((i + 1))
      n="${raw:i:1}"
      case "$n" in
        '"' | "\\" | '/') FIELD+="$n" ;;
        *) return 2 ;;
      esac
    else
      FIELD+="$c"
    fi
  done
  return 0
}

# read_field NAME: sets FIELD to the top-level string field NAME. Returns 0
# with FIELD set (possibly empty), 1 when the field is absent, 2 when it is
# refused.
read_field() {
  local name="$1" match raw
  FIELD=""
  if [ "$have_jq" = 1 ]; then
    # Strings only, as the fallback reads. The trailing x keeps a value's own
    # trailing newlines from being stripped.
    FIELD="$(printf '%s' "$payload" | jq -j --arg k "$name" '.[$k] | strings' 2>/dev/null; printf x)" || true
    FIELD="${FIELD%x}"
    [ -n "$FIELD" ] || return 1
    return 0
  fi
  match="$(printf '%s' "$payload" \
    | grep -oE "\"$name\"[[:space:]]*:[[:space:]]*\"([^\"\\\\]|\\\\.)*\"" \
    | head -n1)" || true
  [ -n "$match" ] || return 1
  raw="$(printf '%s' "$match" | sed -E 's/^"[^"]*"[[:space:]]*:[[:space:]]*"//; s/"$//')"
  decode_json_string "$raw"
}

# json_escape TEXT: sets ESCAPED to TEXT escaped for a JSON string: backslash,
# double quote and every control character below U+0020.
json_escape() {
  local s="$1" i c code
  ESCAPED=""
  for ((i = 0; i < ${#s}; i++)); do
    c="${s:i:1}"
    case "$c" in
      "\\") ESCAPED+="\\\\" ;;
      '"') ESCAPED+='\"' ;;
      [$'\001'-$'\037'])
        printf -v code '%04x' "'$c"
        ESCAPED+="\\u$code"
        ;;
      *) ESCAPED+="$c" ;;
    esac
  done
}

# shell_quote TEXT: sets QUOTED to TEXT in single quotes, each ' written '\''.
shell_quote() {
  local s="$1" i c
  QUOTED="'"
  for ((i = 0; i < ${#s}; i++)); do
    c="${s:i:1}"
    if [ "$c" = "'" ]; then
      QUOTED+="'\\''"
    else
      QUOTED+="$c"
    fi
  done
  QUOTED+="'"
}

has_control_char() {
  case "$1" in
    *["$CONTROL_CHARS"]*) return 0 ;;
  esac
  return 1
}

# read_or_quit NAME: sets FIELD, or exits silently when the field is refused.
# Returns 1 when the field is absent.
read_or_quit() {
  local rc=0
  read_field "$1" || rc=$?
  if [ "$rc" -eq 2 ]; then
    exit 0
  fi
  return "$rc"
}

session_id=""
if read_or_quit session_id; then
  session_id="$FIELD"
fi
if ! [[ "$session_id" =~ ^[A-Za-z0-9_-]+$ ]]; then
  # No usable session id; leave the feature disabled.
  exit 0
fi

transcript_path=""
if read_or_quit transcript_path; then
  transcript_path="$FIELD"
fi

# Fall back to cwd-derived project dir if transcript_path is absent or empty.
if [ -n "$transcript_path" ]; then
  # Checked before dirname, whose output loses trailing newlines in $(...).
  if has_control_char "$transcript_path"; then
    exit 0
  fi
  project_dir="$(dirname -- "$transcript_path")"
else
  project_dir=""
  if read_or_quit cwd; then
    project_dir="$FIELD"
  fi
fi

if [ -z "$project_dir" ]; then
  # Not enough to derive the directory; leave the feature disabled.
  exit 0
fi

workflows_dir="${project_dir%/}/${session_id}/workflows"
if has_control_char "$workflows_dir"; then
  exit 0
fi

# Announce the location so the operator/agent (and koto sessions that inherit
# the environment) can render into it. The additionalContext also carries the
# exact export for environments where the hook cannot mutate the session env
# directly; the koto-user skill documents the same contract.
shell_quote "$workflows_dir"
context="koto native /workflows rendering is available for this session. \
koto sessions run here will render into ${workflows_dir} when \
KOTO_WORKFLOWS_DIR is set for koto processes: export \
KOTO_WORKFLOWS_DIR=${QUOTED}"

if [ "$have_jq" = 1 ]; then
  jq -n --arg ctx "$context" '{
    hookSpecificOutput: {
      hookEventName: "SessionStart",
      additionalContext: $ctx
    }
  }'
else
  json_escape "$context"
  printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$ESCAPED"
fi

exit 0
