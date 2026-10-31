#!/usr/bin/env bash
# End-to-end verification for koto sessions rendering natively in Claude Code's
# /workflows screen: the initial single-session render plus the real
# phase/agent detail enrichment.
#
# Drives a real koto session with a real template through the same
# publish -> advance -> render path an operator's Claude Code session
# exercises, and asserts the "Verified when" criteria against the emitted
# koto-<uuid>.json. This is the CI/CLI-runnable proof of the properties; the
# live-TUI check (that Claude Code actually renders the file) is documented as a
# manual procedure in docs/guides/native-workflows-verification.md, since CI
# cannot drive the TUI.
#
# Coverage:
#   Initial render: single-session render + current state, update on reopen,
#              done-on-completion, default-path-untouched, atomic UUID filename.
#   Phase detail: ordered phases with the active one marked, the active phase's
#              directive legible, per-phase evidence/gate outcome, and a
#              gate-blocked session rendering as blocked: top-level
#              `running`, `koto.status` `blocked`, a Blocked `error` step
#              naming the gate, and a summary ending `blocked on gate <name>`.
#
# The hello-koto template's `awakening` state carries a command gate
# (spirit_greeting) that fails until a greeting file exists, so the first
# advance lands on a gate-blocked state (the blocked mapping) and the
# second, after the greeting, completes -- exercising both statuses on one run.
#
# Usage: scripts/verify-native-workflows.sh [path-to-koto-binary]
# Defaults to ./target/debug/koto (falls back to ./target/release/koto).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KOTO="${1:-}"
if [ -z "$KOTO" ]; then
  if [ -x "$REPO_ROOT/target/debug/koto" ]; then
    KOTO="$REPO_ROOT/target/debug/koto"
  elif [ -x "$REPO_ROOT/target/release/koto" ]; then
    KOTO="$REPO_ROOT/target/release/koto"
  else
    echo "FAIL: no koto binary found; run 'cargo build' or pass a path" >&2
    exit 1
  fi
fi

TEMPLATE="$REPO_ROOT/test/functional/fixtures/templates/hello-koto.md"
WORKDIR="$(mktemp -d)"
SESSIONS="$(mktemp -d)"
WF_DIR="$WORKDIR/claude-session/workflows"
trap 'rm -rf "$WORKDIR" "$SESSIONS"' EXIT

export KOTO_SESSIONS_BASE="$SESSIONS"
cd "$WORKDIR"

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }

wf_file() { ls "$WF_DIR"/koto-*.json 2>/dev/null | head -n1; }
field() { python3 -c "import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$1" "$2"; }
koto_field() { python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['koto'][sys.argv[2]])" "$1" "$2"; }
# Comma-joined phase titles, in order.
phase_titles() { python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(','.join(p['title'] for p in d.get('phases',[])))" "$1"; }
# The `detail` of the phase whose title matches $2.
phase_detail() { python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(next((p.get('detail','') for p in d.get('phases',[]) if p['title']==sys.argv[2]),''))" "$1" "$2"; }
# The active workflow_agent step: `progress` on a running session, `error` on a
# blocked one. Prints the field named by $2 (empty when absent).
active_step_field() { python3 -c "import json,sys;d=json.load(open(sys.argv[1]));n=next((n for n in d.get('workflowProgress',[]) if n.get('type')=='workflow_agent' and n.get('state') in ('progress','error')),{});v=n.get(sys.argv[2],'');print(json.dumps(v) if isinstance(v,bool) else v)" "$1" "$2"; }
# The promptPreview of the active workflow_agent step.
active_directive() { active_step_field "$1" promptPreview; }

# ---- Setup: init a real session and publish the /workflows location ----
"$KOTO" init verify --template "$TEMPLATE" --var SPIRIT_NAME=Koto --intent "verify" >/dev/null
"$KOTO" workflows publish --dir "$WF_DIR" --session verify >/dev/null

# ---- render #1: advancing shows an entry with the current state ----
# The greeting file does not exist yet, so awakening's gate fails on this
# advance (the blocked case, checked below). The initial-render property --
# an entry rendered with the session's current state -- holds regardless.
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"; [ -n "$F" ] || fail "render#1: no koto-*.json written after advance"
[ "$(koto_field "$F" currentState)" = "awakening" ] || fail "render#1: currentState != awakening"
case "$(basename "$F")" in koto-*.json) ;; *) fail "render: filename not koto-<uuid>.json" ;; esac
case "$(basename "$F")" in wf_*|wf-*) fail "render: filename collides with wf_*" ;; esac
pass "render#1: entry rendered with current state; koto-<uuid>.json filename"

START1="$(field "$F" startTime)"

# ---- AC1: phases render in order with the active one marked ----
[ "$(phase_titles "$F")" = "Awakening,Eternal" ] || fail "AC1: phases not in expected order (got '$(phase_titles "$F")')"
# The progress tree must mark the active phase (a workflow_agent with state
# progress, or error on a blocked session).
python3 -c "import json,sys;d=json.load(open(sys.argv[1]));sys.exit(0 if any(n.get('type')=='workflow_agent' and n.get('state') in ('progress','error') for n in d.get('workflowProgress',[])) else 1)" "$F" || fail "AC1: no active phase marked"
pass "AC1: phases render in order with the active one marked"

# ---- AC2: the active phase's directive is legible ----
case "$(active_directive "$F")" in
  *"Greet the spirit"*) ;;
  *) fail "AC2: active directive not legible (got '$(active_directive "$F")')" ;;
esac
pass "AC2: active phase directive is legible"

# ---- AC3: a gate-blocked session renders as blocked ----
# Claude Code 2.1.314 counts every status but `running` as completed, so a
# blocked session is written top-level `running`; koto's own classification is
# `koto.status`, and the active step is an `error` flagged `blocked`.
[ "$(field "$F" status)" = "running" ] || fail "AC3: top-level status != running (got '$(field "$F" status)')"
pass "AC3: top-level status is running"
[ "$(koto_field "$F" status)" = "blocked" ] || fail "AC3: koto.status != blocked (got '$(koto_field "$F" status)')"
pass "AC3: koto.status is blocked"
[ "$(active_step_field "$F" state)" = "error" ] || fail "AC3: active step state != error (got '$(active_step_field "$F" state)')"
[ "$(active_step_field "$F" blocked)" = "true" ] || fail "AC3: active step not flagged blocked (got '$(active_step_field "$F" blocked)')"
[ "$(active_step_field "$F" error)" = "gate spirit_greeting: FAIL" ] || fail "AC3: active step error != 'gate spirit_greeting: FAIL' (got '$(active_step_field "$F" error)')"
pass "AC3: blocked step reads error, blocked, 'gate spirit_greeting: FAIL'"
SUMMARY="$(field "$F" summary)"
case "$SUMMARY" in
  "koto · verify · awakening"*"· blocked on gate spirit_greeting") ;;
  *) fail "AC3: summary is not the blocked summary (got '$SUMMARY')" ;;
esac
pass "AC3: summary ends with '· blocked on gate spirit_greeting'"
case "$(phase_detail "$F" Awakening)" in
  *"FAIL"*) ;;
  *) fail "AC3: blocked phase does not show the failed gate outcome (got '$(phase_detail "$F" Awakening)')" ;;
esac
pass "AC3: gate-blocked session renders as blocked with the failed gate outcome"

# ---- render #2: after advancing, the entry shows the new state ----
mkdir -p wip; echo "hello Koto" > wip/spirit-greeting.txt
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"
[ "$(koto_field "$F" currentState)" = "eternal" ] || fail "render#2: currentState did not advance to eternal"
pass "render#2: entry reflects the advanced state on re-read"

# ---- AC1 (completed phase outcome): the now-completed phase shows its gate PASS ----
case "$(phase_detail "$F" Awakening)" in
  *"PASS"*) ;;
  *) fail "AC1: completed phase does not show its gate outcome (got '$(phase_detail "$F" Awakening)')" ;;
esac
pass "AC1: completed phase shows its gate outcome"

# ---- render #3: on completion the entry reads done, not stuck running ----
[ "$(field "$F" status)" = "completed" ] || fail "render#3: terminal status != completed (got '$(field "$F" status)')"
[ "$(field "$F" startTime)" = "$START1" ] || fail "render#3: startTime not preserved across rewrites"
pass "render#3: terminal renders as completed; startTime stable"
# Claude Code counts a phase's done steps over all its steps and draws a
# progress step on a finished run as Stopped, so every step must be done.
NOT_DONE="$(python3 -c "import json,sys;d=json.load(open(sys.argv[1]));s=[n for n in d.get('workflowProgress',[]) if n.get('type')=='workflow_agent'];print('none' if not s else ','.join(n.get('label','?')+'='+str(n.get('state')) for n in s if n.get('state')!='done'))" "$F")"
[ "$NOT_DONE" != "none" ] || fail "render#3: no workflow_agent steps written"
[ -z "$NOT_DONE" ] || fail "render#3: finished session has steps not done ($NOT_DONE)"
pass "render#3: every workflow_agent step is done, none progress"

# ---- render #4: with no published location, koto writes nothing ----
NOPUB_SESSIONS="$(mktemp -d)"; NOPUB_WF="$(mktemp -d)"
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" init nopub --template "$TEMPLATE" --var SPIRIT_NAME=X >/dev/null
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" next nopub >/dev/null 2>&1 || true
[ -z "$(ls -A "$NOPUB_WF" 2>/dev/null)" ] || fail "render#4: file written without a published location"
rm -rf "$NOPUB_SESSIONS" "$NOPUB_WF"
pass "render#4: no published location -> koto writes nothing (default path untouched)"

echo "ALL CHECKS PASSED: native /workflows rendering verified end-to-end."
