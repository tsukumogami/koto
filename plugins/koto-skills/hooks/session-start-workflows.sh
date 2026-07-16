#!/usr/bin/env bash
# SessionStart hook: enable koto's native Claude Code /workflows rendering.
#
# Claude Code's /workflows screen renders `*.json` files under
# `<projectDir>/<sessionId>/workflows/`. koto cannot self-discover that
# per-session directory, so this hook -- which receives `session_id` and
# `transcript_path` on stdin -- derives it and hands it to koto through the
# `KOTO_WORKFLOWS_DIR` environment variable. koto sessions driven in this
# Claude Code session then materialize their own `koto-<uuid>.json` into that
# directory on each state-commit (see the koto-user skill and
# docs/designs/DESIGN-native-workflows-render.md).
#
# The hook is best-effort and always exits 0: a parse failure or a missing
# field disables the feature silently rather than disrupting the session.

set -euo pipefail

payload="$(cat)"

read_field() {
  # $1 = field name. Prefer jq; fall back to a minimal grep/sed extractor.
  local name="$1"
  if command -v jq >/dev/null 2>&1; then
    printf '%s' "$payload" | jq -r --arg k "$name" '.[$k] // empty' 2>/dev/null
  else
    printf '%s' "$payload" \
      | grep -oE "\"$name\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" \
      | head -n1 \
      | sed -E "s/.*:[[:space:]]*\"([^\"]*)\".*/\1/"
  fi
}

session_id="$(read_field session_id || true)"
transcript_path="$(read_field transcript_path || true)"

# Fall back to cwd-derived project dir if transcript_path is absent.
if [ -n "${transcript_path:-}" ]; then
  project_dir="$(dirname "$transcript_path")"
else
  project_dir="$(read_field cwd || true)"
fi

if [ -z "${session_id:-}" ] || [ -z "${project_dir:-}" ]; then
  # Not enough to derive the directory; leave the feature disabled.
  exit 0
fi

workflows_dir="${project_dir%/}/${session_id}/workflows"

# Announce the location so the operator/agent (and koto sessions that inherit
# the environment) can render into it. The additionalContext also carries the
# exact export for environments where the hook cannot mutate the session env
# directly; the koto-user skill documents the same contract.
context="koto native /workflows rendering is available for this session. \
koto sessions run here will render into ${workflows_dir} when \
KOTO_WORKFLOWS_DIR is set for koto processes: export \
KOTO_WORKFLOWS_DIR=\"${workflows_dir}\""

if command -v jq >/dev/null 2>&1; then
  jq -n --arg ctx "$context" '{
    hookSpecificOutput: {
      hookEventName: "SessionStart",
      additionalContext: $ctx
    }
  }'
else
  # Minimal hand-rolled JSON (context has no embedded quotes to escape).
  printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$context"
fi

exit 0
