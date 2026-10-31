#!/usr/bin/env bash
# Tests for hooks/session-start-workflows.sh, on a fixed payload matrix.
#
#   session-start-workflows.test.sh
#
# Every case runs in two passes. The `jq` pass uses the normal PATH, which
# must have jq on it. The `no-jq` pass uses a PATH of symlinks to only the
# commands the hook needs (bash, cat, dirname, grep, sed, head), where jq must
# not be found, so it runs the hook's fallback reader and writer. Either
# precondition failing stops the suite before any case runs.
#
# A case either names a directory or expects silence. For a directory, the
# hook must exit 0 and print one JSON object whose hookEventName is
# SessionStart and whose additionalContext names the directory byte for byte
# in its plain sentence and ends with the export line, the directory single
# quoted for the shell. The export line is evaluated in a subshell: it must
# set KOTO_WORKFLOWS_DIR to exactly the directory and create no sentinel file.
# When both passes name a directory, their additionalContext values must be
# identical. For silence, the hook must exit 0 and print nothing.
#
# jq from the normal PATH reads the hook's output in both passes. Each failing
# case is reported with its pass and payload; the suite exits 1 if any fails.
#
# shellcheck disable=SC2016
# (payloads hold `$(` and backticks that must reach the hook unexpanded.)

set -u
export LC_ALL=C

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/../session-start-workflows.sh"
NAME="session-start-workflows.test.sh"

if ! command -v jq >/dev/null 2>&1; then
  echo "$NAME: jq is not on the PATH; the jq pass needs it" >&2
  exit 2
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

RBIN="$TMP/bin"
mkdir "$RBIN" "$TMP/eval"
for c in bash cat dirname grep sed head; do
  p="$(command -v "$c")" || {
    echo "$NAME: $c is not on the PATH" >&2
    exit 2
  }
  ln -s "$p" "$RBIN/$c"
done
if PATH="$RBIN" "$RBIN/bash" -c 'command -v jq' >/dev/null 2>&1; then
  echo "$NAME: jq is on the restricted PATH, so the no-jq pass would not test the fallback" >&2
  exit 2
fi

SENT_DOLLAR="$TMP/sentinel-dollar"
SENT_BACKTICK="$TMP/sentinel-backtick"

failures=0
jq_ok=0
jq_total=0
nojq_ok=0
nojq_total=0

# run_hook PASS PAYLOAD: sets OUT to the hook's exact stdout and RC to its
# exit status.
run_hook() {
  local pass="$1" payload="$2"
  if [ "$pass" = jq ]; then
    OUT="$(printf '%s' "$payload" | bash "$HOOK" 2>/dev/null; r=$?; printf x; exit "$r")"
  else
    OUT="$(printf '%s' "$payload" | PATH="$RBIN" "$RBIN/bash" "$HOOK" 2>/dev/null; r=$?; printf x; exit "$r")"
  fi
  RC=$?
  OUT="${OUT%x}"
}

# shell_quote DIR: sets QUOTED to DIR in single quotes, each ' written '\''.
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

fail() {
  # fail CASE PASS PAYLOAD REASON
  local q
  q="$(printf '%q' "$3")"
  echo "FAIL [$2] $1: $4"
  echo "     payload: $q"
  failures=$((failures + 1))
}

count() {
  # count PASS OK(0|1)
  if [ "$1" = jq ]; then
    jq_total=$((jq_total + 1))
    [ "$2" = 1 ] && jq_ok=$((jq_ok + 1))
  else
    nojq_total=$((nojq_total + 1))
    [ "$2" = 1 ] && nojq_ok=$((nojq_ok + 1))
  fi
}

# check CASE PASS PAYLOAD EXPECT: EXPECT is the directory the hook must name,
# or `-` for silence. Sets CTX to the additionalContext on a pass that named a
# directory, and returns 0 when the case passed.
check() {
  local name="$1" pass="$2" payload="$3" expect="$4" n et line got
  CTX=""
  rm -f "$SENT_DOLLAR" "$SENT_BACKTICK"
  run_hook "$pass" "$payload"
  if [ "$RC" -ne 0 ]; then
    fail "$name" "$pass" "$payload" "exit $RC, want 0"
    return 1
  fi
  if [ "$expect" = - ]; then
    if [ -n "$OUT" ]; then
      fail "$name" "$pass" "$payload" "printed ${#OUT} bytes, want nothing"
      return 1
    fi
    return 0
  fi
  if ! n="$(printf '%s' "$OUT" | jq -s length 2>/dev/null)"; then
    fail "$name" "$pass" "$payload" "output is not JSON: $(printf '%q' "$OUT")"
    return 1
  fi
  if [ "$n" != 1 ]; then
    fail "$name" "$pass" "$payload" "printed $n JSON values, want 1"
    return 1
  fi
  et="$(printf '%s' "$OUT" | jq -r '.hookSpecificOutput.hookEventName')"
  if [ "$et" != SessionStart ]; then
    fail "$name" "$pass" "$payload" "hookEventName is $(printf '%q' "$et"), want SessionStart"
    return 1
  fi
  CTX="$(printf '%s' "$OUT" | jq -j '.hookSpecificOutput.additionalContext | strings'; printf x)"
  CTX="${CTX%x}"
  case "$CTX" in
    *"render into $expect when"*) ;;
    *)
      fail "$name" "$pass" "$payload" "additionalContext does not name $(printf '%q' "$expect") in its sentence: $(printf '%q' "$CTX")"
      return 1
      ;;
  esac
  shell_quote "$expect"
  case "$CTX" in
    *"for koto processes: export KOTO_WORKFLOWS_DIR=$QUOTED") ;;
    *)
      fail "$name" "$pass" "$payload" "additionalContext does not end with export KOTO_WORKFLOWS_DIR=$QUOTED: $(printf '%q' "$CTX")"
      return 1
      ;;
  esac
  line="${CTX#*for koto processes: }"
  got="$(
    cd "$TMP/eval" || exit 1
    unset KOTO_WORKFLOWS_DIR
    eval "$line" >/dev/null 2>&1
    printf '%sx' "${KOTO_WORKFLOWS_DIR-}"
  )"
  got="${got%x}"
  if [ -e "$SENT_DOLLAR" ] || [ -e "$SENT_BACKTICK" ]; then
    fail "$name" "$pass" "$payload" "evaluating the export line ran a command (a sentinel file exists)"
    return 1
  fi
  if [ "$got" != "$expect" ]; then
    fail "$name" "$pass" "$payload" "the export line set KOTO_WORKFLOWS_DIR to $(printf '%q' "$got"), want $(printf '%q' "$expect")"
    return 1
  fi
  return 0
}

# both CASE PAYLOAD EXPECT_JQ [EXPECT_NOJQ]: runs both passes. EXPECT_NOJQ
# defaults to EXPECT_JQ; when both name the same directory, the two
# additionalContext values must be identical.
both() {
  local name="$1" payload="$2" ej="$3" en="${4-$3}" ok_j=0 ok_n=0 ctx_j
  if check "$name" jq "$payload" "$ej"; then
    ok_j=1
  fi
  ctx_j="$CTX"
  if check "$name" no-jq "$payload" "$en"; then
    ok_n=1
    if [ "$ok_j" = 1 ] && [ "$ej" != - ] && [ "$ej" = "$en" ] && [ "$ctx_j" != "$CTX" ]; then
      fail "$name" no-jq "$payload" "additionalContext differs from the jq pass's: $(printf '%q' "$CTX") vs $(printf '%q' "$ctx_j")"
      ok_n=0
    fi
  fi
  count jq "$ok_j"
  count no-jq "$ok_n"
  [ "$ok_j" = 1 ] && echo "ok   [jq] $name"
  [ "$ok_n" = 1 ] && echo "ok   [no-jq] $name"
  return 0
}

# --- Cases that name a directory ----------------------------------------------

both plain \
  '{"session_id":"abc123","transcript_path":"/work/proj/t.jsonl"}' \
  '/work/proj/abc123/workflows'

both quote-and-backslash \
  '{"session_id":"abc123","transcript_path":"/tmp/a \"b\\c d/t.jsonl"}' \
  '/tmp/a "b\c d/abc123/workflows'

both shell-metacharacters \
  $'{"session_id":"abc123","transcript_path":"/tmp/q$(x `y` \'z/t.jsonl"}' \
  $'/tmp/q$(x `y` \'z/abc123/workflows'

both empty-transcript-with-cwd \
  '{"session_id":"abc123","transcript_path":"","cwd":"/work/c \"q\\ d"}' \
  '/work/c "q\ d/abc123/workflows'

both absent-transcript-with-cwd \
  $'{"session_id":"abc_1-2","cwd":"/work/c $(d) \'e"}' \
  $'/work/c $(d) \'e/abc_1-2/workflows'

both command-substitution-sentinel \
  "{\"session_id\":\"abc123\",\"transcript_path\":\"/tmp/s\$(touch $SENT_DOLLAR)e/t.jsonl\"}" \
  "/tmp/s\$(touch $SENT_DOLLAR)e/abc123/workflows"

both backtick-sentinel \
  "{\"session_id\":\"abc123\",\"transcript_path\":\"/tmp/s\`touch $SENT_BACKTICK\`e/t.jsonl\"}" \
  "/tmp/s\`touch $SENT_BACKTICK\`e/abc123/workflows"

both escaped-slash \
  '{"session_id":"abc123","transcript_path":"\/work\/esc\/t.jsonl"}' \
  '/work/esc/abc123/workflows'

# --- Cases that print nothing --------------------------------------------------

both no-session-id \
  '{"transcript_path":"/work/proj/t.jsonl"}' \
  -

both session-id-with-slash \
  '{"session_id":"../x","transcript_path":"/tmp/p/t.jsonl"}' \
  -

both del-in-directory \
  $'{"session_id":"abc123","transcript_path":"/tmp/d\177x/t.jsonl"}' \
  -

both not-json \
  'this is not json' \
  -

# Without jq, an escape other than \" \\ \/ refuses the field, and a refused
# transcript_path never falls back to cwd. With jq, \n and \t decode to
# control characters, which the hook refuses in the directory.
both newline-escape-with-cwd \
  '{"session_id":"abc123","transcript_path":"/tmp/n\nx/t.jsonl","cwd":"/work/proj"}' \
  -

both tab-escape-with-cwd \
  '{"session_id":"abc123","transcript_path":"/tmp/t\tx/t.jsonl","cwd":"/work/proj"}' \
  -

# The \u escape is spelled in two pieces so an editor that decodes such
# escapes can't turn it into a plain `A`.
U0041='\u''0041'
both unicode-escape-with-cwd \
  '{"session_id":"abc123","transcript_path":"/tmp/u'"$U0041"'x/t.jsonl","cwd":"/work/proj"}' \
  '/tmp/uAx/abc123/workflows' \
  -

echo "jq pass: $jq_ok/$jq_total cases passed"
echo "no-jq pass: $nojq_ok/$nojq_total cases passed"
if [ "$failures" -ne 0 ]; then
  echo "$NAME: $failures failure(s)"
  exit 1
fi
exit 0
