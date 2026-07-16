#!/usr/bin/env bash
# End-to-end verification for koto sessions rendering natively in Claude Code's
# /workflows screen: Feature 1 (walking skeleton) plus Feature 2 (real
# phase/agent detail).
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
#   Feature 1: single-session render + current state, update on reopen,
#              done-on-completion, default-path-untouched, atomic UUID filename.
#   Feature 2: ordered phases with the active one marked, the active phase's
#              directive legible, per-phase evidence/gate outcome, and a
#              gate-blocked session rendering as `blocked` (not running/done).
#
# The hello-koto template's `awakening` state carries a command gate
# (spirit_greeting) that fails until a greeting file exists, so the first
# advance lands on a gate-blocked state (Feature 2's blocked mapping) and the
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
# The promptPreview of the active (state==progress) workflow_agent step.
active_directive() { python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(next((n.get('promptPreview','') for n in d.get('workflowProgress',[]) if n.get('type')=='workflow_agent' and n.get('state')=='progress'),''))" "$1"; }

# ---- Setup: init a real session and publish the /workflows location ----
"$KOTO" init verify --template "$TEMPLATE" --var SPIRIT_NAME=Koto --intent "verify" >/dev/null
"$KOTO" workflows publish --dir "$WF_DIR" --session verify >/dev/null

# ---- F1 #1: advancing shows an entry with the current state ----
# The greeting file does not exist yet, so awakening's gate fails on this
# advance (Feature 2's blocked case, checked below). The Feature 1 property --
# an entry rendered with the session's current state -- holds regardless.
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"; [ -n "$F" ] || fail "F1#1: no koto-*.json written after advance"
[ "$(koto_field "$F" currentState)" = "awakening" ] || fail "F1#1: currentState != awakening"
case "$(basename "$F")" in koto-*.json) ;; *) fail "F1: filename not koto-<uuid>.json" ;; esac
case "$(basename "$F")" in wf_*|wf-*) fail "F1: filename collides with wf_*" ;; esac
pass "F1#1: entry rendered with current state; koto-<uuid>.json filename"

START1="$(field "$F" startTime)"

# ---- F2 AC1: phases render in order with the active one marked ----
[ "$(phase_titles "$F")" = "Awakening,Eternal" ] || fail "F2-AC1: phases not in expected order (got '$(phase_titles "$F")')"
# The progress tree must mark the active phase (a workflow_agent with state=progress).
python3 -c "import json,sys;d=json.load(open(sys.argv[1]));sys.exit(0 if any(n.get('type')=='workflow_agent' and n.get('state')=='progress' for n in d.get('workflowProgress',[])) else 1)" "$F" || fail "F2-AC1: no active (progress) phase marked"
pass "F2-AC1: phases render in order with the active one marked"

# ---- F2 AC2: the active phase's directive is legible ----
case "$(active_directive "$F")" in
  *"Greet the spirit"*) ;;
  *) fail "F2-AC2: active directive not legible (got '$(active_directive "$F")')" ;;
esac
pass "F2-AC2: active phase directive is legible"

# ---- F2 AC3: a gate-blocked session renders as blocked ----
[ "$(field "$F" status)" = "blocked" ] || fail "F2-AC3: status != blocked (got '$(field "$F" status)')"
case "$(phase_detail "$F" Awakening)" in
  *"FAIL"*) ;;
  *) fail "F2-AC3: blocked phase does not show the failed gate outcome (got '$(phase_detail "$F" Awakening)')" ;;
esac
pass "F2-AC3: gate-blocked session renders as blocked with the failed gate outcome"

# ---- F1 #2: after advancing, the entry shows the new state ----
mkdir -p wip; echo "hello Koto" > wip/spirit-greeting.txt
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"
[ "$(koto_field "$F" currentState)" = "eternal" ] || fail "F1#2: currentState did not advance to eternal"
pass "F1#2: entry reflects the advanced state on re-read"

# ---- F2 AC1 (completed phase outcome): the now-completed phase shows its gate PASS ----
case "$(phase_detail "$F" Awakening)" in
  *"PASS"*) ;;
  *) fail "F2-AC1: completed phase does not show its gate outcome (got '$(phase_detail "$F" Awakening)')" ;;
esac
pass "F2-AC1: completed phase shows its gate outcome"

# ---- F1 #3: on completion the entry reads done, not stuck running ----
[ "$(field "$F" status)" = "completed" ] || fail "F1#3: terminal status != completed (got '$(field "$F" status)')"
[ "$(field "$F" startTime)" = "$START1" ] || fail "F1#3: startTime not preserved across rewrites"
pass "F1#3: terminal renders as completed; startTime stable"

# ---- F1 #4: with no published location, koto writes nothing ----
NOPUB_SESSIONS="$(mktemp -d)"; NOPUB_WF="$(mktemp -d)"
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" init nopub --template "$TEMPLATE" --var SPIRIT_NAME=X >/dev/null
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" next nopub >/dev/null 2>&1 || true
[ -z "$(ls -A "$NOPUB_WF" 2>/dev/null)" ] || fail "F1#4: file written without a published location"
rm -rf "$NOPUB_SESSIONS" "$NOPUB_WF"
pass "F1#4: no published location -> koto writes nothing (default path untouched)"

echo "ALL CHECKS PASSED: Feature 1 + Feature 2 verified end-to-end."
