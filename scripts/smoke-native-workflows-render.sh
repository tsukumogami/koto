#!/usr/bin/env bash
# Rendered smoke check: does a real Claude Code draw koto's /workflows entry?
#
# Run by a maintainer, on their own machine with their own Claude Code login,
# before a release. It starts a real interactive `claude` in a detached tmux
# session, from an empty scratch project directory, with the koto-skills
# session-start-workflows.sh hook registered through a --settings file. The
# hook names the session's /workflows directory; koto writes an entry there;
# the harness opens /workflows and reads the pane.
#
# Usage: scripts/smoke-native-workflows-render.sh [--no-entry] [--koto <path>]
#
#   --no-entry     skip writing the koto entry (the screen should show the
#                  workflows list or its empty state, and the run reports
#                  `not rendered`)
#   --koto <path>  the koto binary to use; by default target/debug/koto, then
#                  target/release/koto, as scripts/verify-native-workflows.sh
#
# It prints one line, `<outcome>: Claude Code <version>`, and on anything but
# `rendered` the reason and the captured pane after it:
#
#   0  rendered      the pane shows the entry's workflowName
#   1  not rendered  the workflows screen is shown without it
#   2  usage error
#   3  unavailable   a prerequisite is missing, a screen other than the
#                    folder-trust prompt came up, the hook printed nothing, or
#                    /workflows is unknown or disabled
#
# What it types, through `tmux send-keys -l`, is the folder-trust answer (only
# when the pane shows that prompt) and `/workflows`. It sends no prompt to the
# model: /workflows is submitted only after the autocomplete shows it is a
# known command, because Claude Code can hand an unknown command to the model.
#
# The captured pane and the saved hook payload can hold the maintainer's
# email, organization and home path. They are kept only in the scratch
# directory, which is removed on exit, and the pane is printed to the terminal
# only.
#
# Cleanup, after every outcome and on SIGINT, SIGTERM or SIGHUP, removes only
# what the run created: its private tmux server, its scratch directory, and,
# under ${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/, the `<uuid>.jsonl` file
# and `<uuid>/` directory for the run's own session UUID, plus that project's
# directory when it is left empty (rmdir). No removal uses a glob or follows a
# symlink.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOK="$REPO_ROOT/plugins/koto-skills/hooks/session-start-workflows.sh"
TEMPLATE="$REPO_ROOT/test/functional/fixtures/templates/hello-koto.md"
WITHOUT_SESSION_ENV="$REPO_ROOT/scripts/without-session-env.sh"

UUID_RE='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
TRUST_TEXT='Yes, I trust this folder'
POINTER=$'\xe2\x9d\xaf' # U+276F, the selection pointer
READY_TEXT='? for shortcuts'
COMMAND_TEXT='Browse running and completed workflows'
PANEL_TEXT='Dynamic workflows'
LOADING_TEXT='Loading dynamic workflow history'
HOOK_WAIT_SECS=30

usage() {
  echo "usage: smoke-native-workflows-render.sh [--no-entry] [--koto <path>]" >&2
  exit 2
}

NO_ENTRY=0
KOTO=""
KOTO_GIVEN=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-entry) NO_ENTRY=1 ;;
    --koto)
      [ "$#" -ge 2 ] && [ -n "$2" ] || usage
      KOTO="$2"
      KOTO_GIVEN=1
      shift
      ;;
    -h | --help)
      echo "usage: smoke-native-workflows-render.sh [--no-entry] [--koto <path>]"
      exit 0
      ;;
    *) usage ;;
  esac
  shift
done

# ---- State the EXIT trap reads; empty means "not created" ----
SCRATCH=""
UUID=""
TMUX_SOCKET=""
TMUX_STARTED=0
CLAUDE_PID=""
PROJECTS_ROOT="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects"
VERSION="unknown"
PANE_FILE=""

# ---- Reporting ----
report() {
  local outcome="$1" code="$2" reason="${3:-}"
  printf '%s: Claude Code %s\n' "$outcome" "$VERSION"
  if [ "$code" -ne 0 ]; then
    [ -z "$reason" ] || printf '%s\n' "$reason"
    if [ -n "$PANE_FILE" ] && [ -s "$PANE_FILE" ]; then
      echo "--- captured pane ---"
      cat "$PANE_FILE"
      echo "--- end of pane ---"
    fi
  fi
  exit "$code"
}

# ---- Prerequisites, all checked before tmux or Claude Code starts ----
CLAUDE_BIN="$(command -v claude 2>/dev/null || true)"
if [ -n "$CLAUDE_BIN" ]; then
  v="$(claude --version </dev/null 2>/dev/null | head -n1 || true)"
  v="${v%% *}"
  if [[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*$ ]]; then
    VERSION="$v"
  fi
fi

missing=()
[ -n "$CLAUDE_BIN" ] || missing+=("claude is not on the PATH")
command -v tmux >/dev/null 2>&1 || missing+=("tmux is not on the PATH")
command -v python3 >/dev/null 2>&1 || missing+=("python3 is not on the PATH")
if [ "$KOTO_GIVEN" = 1 ]; then
  if ! { [ -f "$KOTO" ] && [ -x "$KOTO" ]; }; then
    missing+=("--koto $KOTO is not an executable file")
  fi
elif [ -x "$REPO_ROOT/target/debug/koto" ]; then
  KOTO="$REPO_ROOT/target/debug/koto"
elif [ -x "$REPO_ROOT/target/release/koto" ]; then
  KOTO="$REPO_ROOT/target/release/koto"
else
  missing+=("no koto binary: run cargo build or pass --koto <path>")
fi
if [ "${#missing[@]}" -gt 0 ]; then
  reason="missing prerequisite:"
  for m in "${missing[@]}"; do
    reason+=$'\n'"  $m"
  done
  report "unavailable" 3 "$reason"
fi
case "$KOTO" in /*) ;; *) KOTO="$PWD/$KOTO" ;; esac

# ---- Cleanup ----
remove_session_files() {
  # Only the run's own UUID, validated, under a projects root that is a real
  # directory. Project directories are listed with find -type d, which never
  # matches a symlink; each candidate path is built exactly, with no pattern.
  [ -n "$UUID" ] && [[ "$UUID" =~ $UUID_RE ]] || return 0
  [ -n "$PROJECTS_ROOT" ] && [ -d "$PROJECTS_ROOT" ] && [ ! -L "$PROJECTS_ROOT" ] || return 0
  local project transcript session_dir removed
  while IFS= read -r -d '' project; do
    [ -n "$project" ] || continue
    removed=0
    transcript="$project/$UUID.jsonl"
    session_dir="$project/$UUID"
    if [ -L "$transcript" ] || [ -f "$transcript" ]; then
      rm -f -- "$transcript" && removed=1
    fi
    if [ -L "$session_dir" ]; then
      rm -f -- "$session_dir" && removed=1
    elif [ -d "$session_dir" ]; then
      rm -rf -- "$session_dir" && removed=1
    fi
    if [ "$removed" = 1 ]; then
      rmdir -- "$project" 2>/dev/null || true
    fi
  done < <(find "$PROJECTS_ROOT" -mindepth 1 -maxdepth 1 -type d -print0 2>/dev/null)
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM HUP
  if [ "$TMUX_STARTED" = 1 ] && [ -n "$TMUX_SOCKET" ]; then
    tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  fi
  # Wait for Claude Code to exit before removing its files, so a write on its
  # way out can't bring them back.
  if [ -n "$CLAUDE_PID" ]; then
    local i
    for i in $(seq 1 20); do
      kill -0 "$CLAUDE_PID" 2>/dev/null || break
      [ "$i" != 10 ] || kill -TERM "$CLAUDE_PID" 2>/dev/null || true
      sleep 0.5
    done
    if kill -0 "$CLAUDE_PID" 2>/dev/null; then
      kill -KILL "$CLAUDE_PID" 2>/dev/null || true
      sleep 0.5
    fi
  fi
  remove_session_files
  if [ -n "$SCRATCH" ] && [ -d "$SCRATCH" ] && [ ! -L "$SCRATCH" ]; then
    rm -rf -- "$SCRATCH"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# ---- Scratch: mode 700, outside the repository ----
umask 077
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/koto-smoke.XXXXXX")"
PROJECT="$(mktemp -d "$SCRATCH/project.XXXXXX")"
PANE_FILE="$SCRATCH/pane.txt"
HOOK_STDIN="$SCRATCH/hook-stdin.json"
HOOK_OUT="$SCRATCH/hook-output.json"
SETTINGS="$SCRATCH/settings.json"
WRAPPER="$SCRATCH/hook-wrapper.sh"
KOTO_HOME_DIR="$SCRATCH/koto-home"
SESSIONS_DIR="$SCRATCH/koto-sessions"
KOTO_WORK="$SCRATCH/koto-work"
mkdir -p "$KOTO_HOME_DIR" "$SESSIONS_DIR" "$KOTO_WORK"

UUID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
[[ "$UUID" =~ $UUID_RE ]] || report "unavailable" 3 "could not make a session UUID"
SHORT_ID="${UUID:0:8}"
TMUX_SOCKET="koto-smoke-$SHORT_ID"
TMUX_TARGET="koto-smoke-$SHORT_ID"
INTENT="koto render smoke $SHORT_ID"

# The wrapper saves the hook's stdin and output, each through a rename, so the
# harness never reads half a file.
{
  echo '#!/usr/bin/env bash'
  printf 'stdin_file=%q\n' "$HOOK_STDIN"
  printf 'out_file=%q\n' "$HOOK_OUT"
  printf 'hook=%q\n' "$HOOK"
  cat <<'WRAPPER_BODY'
cat >"$stdin_file.tmp"
mv "$stdin_file.tmp" "$stdin_file"
"$hook" <"$stdin_file" >"$out_file.tmp" 2>/dev/null || true
mv "$out_file.tmp" "$out_file"
cat "$out_file"
exit 0
WRAPPER_BODY
} >"$WRAPPER"
chmod 700 "$WRAPPER"

python3 - "$WRAPPER" "$SETTINGS" <<'PY'
import json, shlex, sys
wrapper, settings = sys.argv[1], sys.argv[2]
doc = {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": shlex.quote(wrapper)}]}]}}
with open(settings, "w") as f:
    json.dump(doc, f)
PY

# ---- tmux helpers, all on the run's private server ----
tm() { tmux -L "$TMUX_SOCKET" "$@"; }

capture() {
  tm capture-pane -p -J -t "$TMUX_TARGET" >"$PANE_FILE.new" 2>/dev/null || return 1
  mv "$PANE_FILE.new" "$PANE_FILE"
}

pane_has() { [ -s "$PANE_FILE" ] && grep -qF -- "$1" "$PANE_FILE"; }

# True when a fresh capture shows the selection pointer on the trust option.
trust_selected() {
  capture || return 1
  grep -F -- "$TRUST_TEXT" "$PANE_FILE" | grep -qF -- "$POINTER"
}

session_alive() { tm has-session -t "$TMUX_TARGET" >/dev/null 2>&1; }

type_text() { tm send-keys -t "$TMUX_TARGET" -l -- "$1"; }

press_enter() { tm send-keys -t "$TMUX_TARGET" -l $'\r'; }

# Captures until the pane stops changing (three equal captures in a row) or
# $1 seconds pass.
settle() {
  local deadline=$((SECONDS + $1)) last="" now="" same=0
  while [ "$SECONDS" -lt "$deadline" ]; do
    capture || return 1
    now="$(cat "$PANE_FILE")"
    if [ "$now" = "$last" ]; then
      same=$((same + 1))
      [ "$same" -lt 2 ] || return 0
    else
      same=0
      last="$now"
    fi
    sleep 0.5
  done
  return 0
}

# ---- Start Claude Code ----
# Claude Code is started directly, not through a shell, so the pane's pid is
# Claude Code's. Variables a calling Claude Code session sets are dropped so
# the new session is a top-level one; CLAUDE_CONFIG_DIR and
# CLAUDE_CODE_DISABLE_WORKFLOWS pass through.
tm -f /dev/null new-session -d -s "$TMUX_TARGET" -x 220 -y 60 -c "$PROJECT" \
  env -u CLAUDECODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_ENTRYPOINT -u TMUX \
  "$CLAUDE_BIN" --session-id "$UUID" --settings "$SETTINGS"
TMUX_STARTED=1
CLAUDE_PID="$(tm display-message -p -t "$TMUX_TARGET" '#{pane_pid}' 2>/dev/null || true)"
[[ "$CLAUDE_PID" =~ ^[0-9]+$ ]] || CLAUDE_PID=""

# ---- Wait for the hook, answering only the folder-trust prompt ----
answered=0
deadline=$((SECONDS + HOOK_WAIT_SECS))
while [ ! -f "$HOOK_OUT" ]; do
  if [ "$SECONDS" -ge "$deadline" ]; then
    report "unavailable" 3 "no SessionStart hook output within ${HOOK_WAIT_SECS}s (a login, onboarding or other screen, or the hook did not run)"
  fi
  if ! session_alive; then
    report "unavailable" 3 "Claude Code exited before the SessionStart hook ran"
  fi
  capture || true
  if [ "$answered" = 0 ] && pane_has "$TRUST_TEXT"; then
    # Claude Code 2.1.316 lists "No, exit" first and focuses it. `j` moves
    # the selection down (the Select context's default binding); Enter is
    # sent only once the pointer is on the trust option.
    settle 2 || true
    if ! trust_selected; then
      type_text "j"
      tries=0
      until trust_selected; do
        tries=$((tries + 1))
        [ "$tries" -lt 6 ] \
          || report "unavailable" 3 "could not move the folder-trust selection to \"$TRUST_TEXT\""
        sleep 0.5
      done
    fi
    press_enter
    answered=1
  fi
  sleep 0.5
done

# ---- The directory the hook derived ----
# Prints the directory from the hook output's export line, only when it is
# <dirname of the payload's transcript_path>/<uuid>/workflows and that
# transcript sits in a project directory directly under the projects root,
# where cleanup looks.
derived_dir() {
  python3 - "$HOOK_OUT" "$HOOK_STDIN" "$PROJECTS_ROOT" "$UUID" <<'PY'
import json, os, shlex, sys
out_file, stdin_file, root, uuid = sys.argv[1:5]
try:
    ctx = json.load(open(out_file))["hookSpecificOutput"]["additionalContext"]
    payload = json.load(open(stdin_file))
except Exception:
    sys.exit(1)
marker = "export KOTO_WORKFLOWS_DIR="
if marker not in ctx:
    sys.exit(1)
words = shlex.split(ctx.split(marker, 1)[1])
if len(words) != 1:
    sys.exit(1)
derived = words[0]
transcript = payload.get("transcript_path") or ""
project = os.path.dirname(transcript)
if not transcript or derived != os.path.join(project, uuid, "workflows"):
    sys.exit(1)
if os.path.realpath(os.path.dirname(project)) != os.path.realpath(root):
    sys.exit(1)
print(derived)
PY
}

DERIVED="$(derived_dir || true)"
if [ -z "$DERIVED" ]; then
  capture || true
  report "unavailable" 3 "the hook output names no workflows directory for this session under the Claude Code projects directory"
fi

# ---- The koto entry ----
run_koto() {
  (
    cd "$KOTO_WORK"
    "$WITHOUT_SESSION_ENV" env \
      -u KOTO_WORKFLOWS_DIR -u KOTO_WORKFLOWS_HOST_SESSION -u KOTO_TICK_SESSION \
      -u CLAUDE_CODE_SESSION_ID \
      KOTO_HOME="$KOTO_HOME_DIR" KOTO_SESSIONS_BASE="$SESSIONS_DIR" \
      KOTO_SESSION_BACKEND=local \
      "$KOTO" "$@"
  ) >>"$SCRATCH/koto.log" 2>&1
}

# Prints the workflowName of the one koto-*.json koto wrote into $1.
entry_name() {
  python3 - "$1" <<'PY'
import json, os, sys
d = sys.argv[1]
files = sorted(f for f in os.listdir(d) if f.startswith("koto-") and f.endswith(".json"))
if len(files) != 1:
    sys.exit(1)
print(json.load(open(os.path.join(d, files[0])))["workflowName"])
PY
}

ENTRY_NAME=""
if [ "$NO_ENTRY" = 0 ]; then
  run_koto init smoke --template "$TEMPLATE" --var SPIRIT_NAME=Koto --intent "$INTENT" \
    || report "unavailable" 3 "koto init failed"
  run_koto workflows publish --dir "$DERIVED" --session smoke \
    || report "unavailable" 3 "koto workflows publish failed"
  # hello-koto's first state blocks on its gate, which still writes the entry.
  run_koto next smoke || true
  ENTRY_NAME="$(entry_name "$DERIVED" 2>/dev/null || true)"
  [ -n "$ENTRY_NAME" ] || report "unavailable" 3 "koto wrote no readable entry into the hook's directory"
fi

# ---- Open /workflows ----
deadline=$((SECONDS + 20))
until settle 3 && pane_has "$READY_TEXT" && ! pane_has "$TRUST_TEXT"; do
  [ "$SECONDS" -lt "$deadline" ] || report "unavailable" 3 "the prompt did not become ready"
  session_alive || report "unavailable" 3 "Claude Code exited before /workflows was sent"
done

type_text "/workflows"
settle 5 || true
if ! pane_has "$COMMAND_TEXT"; then
  # Never submitted: an unknown command can be handed to the model.
  report "unavailable" 3 "the /workflows command is not offered (unknown or disabled)"
fi
press_enter

deadline=$((SECONDS + 15))
while :; do
  settle 3 || true
  if pane_has "$PANEL_TEXT" && ! pane_has "$LOADING_TEXT"; then
    break
  fi
  if pane_has "Unknown command: /workflows" || pane_has "workflows are disabled"; then
    break
  fi
  [ "$SECONDS" -lt "$deadline" ] || break
  session_alive || break
done

if pane_has "Unknown command: /workflows" || pane_has "workflows are disabled"; then
  report "unavailable" 3 "/workflows is unknown or disabled"
fi
if [ -n "$ENTRY_NAME" ] && pane_has "$ENTRY_NAME"; then
  report "rendered" 0
fi
if pane_has "$PANEL_TEXT"; then
  if [ "$NO_ENTRY" = 1 ]; then
    report "not rendered" 1 "the workflows screen is shown (--no-entry: no koto entry was written)"
  fi
  report "not rendered" 1 "the workflows screen does not show the entry \"$ENTRY_NAME\""
fi
report "unavailable" 3 "the workflows screen did not open"
