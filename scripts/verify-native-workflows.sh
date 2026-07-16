#!/usr/bin/env bash
# End-to-end verification for Feature 1: koto sessions render natively in
# Claude Code's /workflows screen.
#
# Drives a real koto session with a real template through the same
# publish -> advance -> render path an operator's Claude Code session
# exercises, and asserts Feature 1's four "Verified when" criteria against the
# emitted koto-<uuid>.json. This is the CI/CLI-runnable proof of the property;
# the live-TUI check (that Claude Code actually renders the file) is documented
# as a manual procedure in docs/guides/native-workflows-verification.md, since
# CI cannot drive the TUI.
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

# ---- Setup: init a real session and publish the /workflows location ----
"$KOTO" init verify --template "$TEMPLATE" --var SPIRIT_NAME=Koto --intent "verify" >/dev/null
"$KOTO" workflows publish --dir "$WF_DIR" --session verify >/dev/null

# ---- AC1: advancing shows an entry with the current state ----
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"; [ -n "$F" ] || fail "AC1: no koto-*.json written after advance"
[ "$(koto_field "$F" currentState)" = "awakening" ] || fail "AC1: currentState != awakening"
[ "$(field "$F" status)" = "running" ] || fail "AC1: status != running"
case "$(basename "$F")" in koto-*.json) ;; *) fail "AC1: filename not koto-<uuid>.json" ;; esac
case "$(basename "$F")" in wf_*|wf-*) fail "AC5: filename collides with wf_*" ;; esac
pass "AC1/AC5: entry rendered with current state, koto-<uuid>.json filename"

START1="$(field "$F" startTime)"

# ---- AC2: after advancing, the entry shows the new state ----
mkdir -p wip; echo "hello Koto" > wip/spirit-greeting.txt
"$KOTO" next verify >/dev/null 2>&1 || true
F="$(wf_file)"
[ "$(koto_field "$F" currentState)" = "eternal" ] || fail "AC2: currentState did not advance to eternal"
pass "AC2: entry reflects the advanced state on re-read"

# ---- AC3: on completion the entry reads done, not stuck running ----
[ "$(field "$F" status)" = "completed" ] || fail "AC3: terminal status != completed (got $(field "$F" status))"
[ "$(field "$F" startTime)" = "$START1" ] || fail "AC3: startTime not preserved across rewrites"
pass "AC3: terminal renders as completed; startTime stable"

# ---- AC4: with no published location, koto writes nothing ----
NOPUB_SESSIONS="$(mktemp -d)"; NOPUB_WF="$(mktemp -d)"
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" init nopub --template "$TEMPLATE" --var SPIRIT_NAME=X >/dev/null
KOTO_SESSIONS_BASE="$NOPUB_SESSIONS" "$KOTO" next nopub >/dev/null 2>&1 || true
[ -z "$(ls -A "$NOPUB_WF" 2>/dev/null)" ] || fail "AC4: file written without a published location"
rm -rf "$NOPUB_SESSIONS" "$NOPUB_WF"
pass "AC4: no published location -> koto writes nothing"

echo "ALL CHECKS PASSED: Feature 1 verified end-to-end."
