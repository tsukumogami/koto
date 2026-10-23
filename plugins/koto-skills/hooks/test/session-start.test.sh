#!/usr/bin/env bash
# Tests for hooks/session-start.sh, on synthetic SessionStart payloads.
#
#   session-start.test.sh [test-name ...]
#
# koto must be on the PATH (CI puts target/release first). Each test runs in
# a temp directory of its own, with its own KOTO_SESSIONS_BASE, HOME and
# `session-env` tree, and with no niwa variable, so the whole suite is also
# the no-niwa case.
#
# The hook runs under a stand-in agent process: a perl process that runs the
# hook once per line it reads, the way Claude Code runs it once per start.
# The hook walks up to that process for its token, so starts it runs share a
# token, and its watchers exit once it does.
#
# The timing test enforces the 300 ms bound on the hook's synchronous part
# when CI is set (GitHub Actions sets it) or KOTO_HOOK_TIMING=enforce, and
# only reports it otherwise: a loaded workstation says little about the
# hook.
#
# shellcheck disable=SC1090,SC2016
# (env files are sourced by path, and many checks pass shell or jq text to
# `sh -c` in single quotes on purpose.)

set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/../session-start.sh"
export HOOK

for v in $(compgen -v KOTO_SESSION_) $(compgen -v NIWA_) $(compgen -v CLAUDE_); do
  unset "$v"
done
unset KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR KOTO_WORKFLOWS_DIR XDG_CONFIG_HOME
unset KOTO_RETENTION_KEEP KOTO_RETENTION_IDLE_PERIOD KOTO_RETENTION_STALE_AFTER

if ! command -v koto >/dev/null; then
  echo "session-start.test.sh: koto is not on the PATH" >&2
  exit 2
fi

# The stand-in agent process. Each line on stdin is `<payload>\t<env file>`
# (`-` for no CLAUDE_ENV_FILE); it runs the hook with that payload on stdin
# and writes the hook's stdout, stderr, exit code and wall time in
# milliseconds beside the payload.
AGENT_PL='
use Time::HiRes qw(time);
while (my $line = <STDIN>) {
  chomp $line;
  my ($payload, $envfile) = split /\t/, $line;
  if ($envfile eq "-") { delete $ENV{CLAUDE_ENV_FILE} } else { $ENV{CLAUDE_ENV_FILE} = $envfile }
  my $t0 = time;
  my $pid = fork();
  if ($pid == 0) {
    open(STDIN, "<", $payload) or exit 1;
    open(STDOUT, ">", "$payload.out") or exit 1;
    open(STDERR, ">", "$payload.err") or exit 1;
    exec("bash", $ENV{HOOK}) or exit 1;
  }
  waitpid($pid, 0);
  my $rc = $? >> 8;
  my $ms = int((time - $t0) * 1000);
  open(my $fh, ">", "$payload.ms"); print $fh "$ms\n"; close $fh;
  open($fh, ">", "$payload.rc.tmp"); print $fh "$rc\n"; close $fh;
  rename("$payload.rc.tmp", "$payload.rc");
}
'

GUARDED_CLOSE='[ -n "$NIWA_REAP_KEY" ] || exit 0; command -v koto >/dev/null 2>&1 || exit 0; koto session anchor close --help >/dev/null 2>&1 || exit 0; exec koto session anchor close --key="$NIWA_REAP_KEY" --harness-session="$NIWA_SESSION_ID"'

# `start` waits for `go`; `yes` reaches the terminal `done`.
TEMPLATE='---
name: hook-test
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Start.

## done

Done.
'

# `work` loops on `more` and ends on `stop`.
LOOP_TEMPLATE='---
name: hook-loop
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: ["more", "stop"]
    transitions:
      - target: work
        when:
          go: "more"
      - target: done
        when:
          go: "stop"
  done:
    terminal: true
---

## work

Work.

## done

Done.
'

FAILED=0
CURRENT=""

fail() {
  echo "  FAIL [$CURRENT]: $*"
  FAILED=1
}

check() {
  local what="$1"
  shift
  if ! "$@"; then
    fail "$what"
  fi
}

# A fresh temp tree for one test, entered as the working directory.
setup() {
  T="$(mktemp -d)"
  T="$(cd "$T" && pwd -P)"
  mkdir -p "$T/base" "$T/home" "$T/cache" "$T/work" "$T/session-env" "$T/payloads"
  printf '%s' "$TEMPLATE" >"$T/hook-test.md"
  printf '%s' "$LOOP_TEMPLATE" >"$T/hook-loop.md"
  export KOTO_SESSIONS_BASE="$T/base" HOME="$T/home" XDG_CACHE_HOME="$T/cache"
  cd "$T/work" || exit 2
  AGENT_FD_OPEN=0
}

teardown() {
  agent_stop
  pkill -f "$T/" >/dev/null 2>&1 || true
  cd / || true
  if [ -n "${KOTO_HOOK_TEST_KEEP:-}" ]; then
    echo "  kept $T"
  else
    rm -rf "$T"
  fi
}

# payload <session id> <source> [transcript path] [cwd]: writes a payload
# file and prints its path.
payload() {
  local f
  f="$(mktemp "$T/payloads/p.XXXXXX")"
  jq -n --arg id "$1" --arg src "$2" --arg tp "${3:-}" --arg cwd "${4:-$T/work}" \
    '{session_id: $id, source: $src, hook_event_name: "SessionStart", cwd: $cwd}
     + (if $tp == "" then {} else {transcript_path: $tp} end)' >"$f"
  printf '%s\n' "$f"
}

# A raw payload from a JSON string.
raw_payload() {
  local f
  f="$(mktemp "$T/payloads/p.XXXXXX")"
  printf '%s' "$1" >"$f"
  printf '%s\n' "$f"
}

# envfile <session id>: the session's hook env file, created empty.
envfile() {
  mkdir -p "$T/session-env/$1"
  local f="$T/session-env/$1/sessionstart-hook-1.sh"
  [ -e "$f" ] || : >"$f"
  printf '%s\n' "$f"
}

# hook <payload> <env file|->: one start under a stand-in agent process of
# its own, which exits as soon as the hook does.
hook() {
  printf '%s\t%s\n' "$1" "$2" | perl -e "$AGENT_PL"
}

# agent_start [VAR=value ...]: a long-lived stand-in agent process, with
# those variables in its environment. Starts run through agent_hook share its
# token until agent_stop.
agent_start() {
  rm -f "$T/agent.fifo"
  mkfifo "$T/agent.fifo"
  env "$@" perl -e "$AGENT_PL" <"$T/agent.fifo" &
  AGENT_PID=$!
  exec 7>"$T/agent.fifo"
  AGENT_FD_OPEN=1
}

agent_hook() {
  printf '%s\t%s\n' "$1" "$2" >&7
  local i=0
  while [ ! -f "$1.rc" ] && [ "$i" -lt 600 ]; do
    sleep 0.1
    i=$((i + 1))
  done
}

agent_stop() {
  if [ "${AGENT_FD_OPEN:-0}" -eq 1 ]; then
    exec 7>&-
    wait "$AGENT_PID" 2>/dev/null
    AGENT_FD_OPEN=0
  fi
}

rc_of() { cat "$1.rc"; }

# The anchor an env file exports, or nothing.
anchor_in() {
  sed -n "s/^export KOTO_ANCHOR='\(.*\)'\$/\1/p" "$1"
}

# The koto-owned keys of a session.
keys() {
  koto context list "$1" --prefix koto/ | jq -r '.[]'
}

has_key() {
  keys "$1" | grep -qx -- "$2"
}

observed() {
  has_key "$1" koto/anchor/observed
}

anchors() {
  local d
  for d in "$T/base"/anchor-*; do
    [ -d "$d" ] && printf '%s\n' "${d##*/}"
  done
}

init() {
  # init <name> [extra args...], under whatever KOTO_ANCHOR the caller set.
  local name="$1"
  shift
  koto init "$name" --template "$T/hook-test.md" "$@" >/dev/null
}

tick_done() {
  koto next "$1" --with-data '{"go": "yes"}' >/dev/null
}

prune_json() {
  koto session prune "$@"
}

kept() {
  # kept <prune json> <name>
  printf '%s' "$1" | jq -e --arg n "$2" '.kept | map(.name) | index($n) != null' >/dev/null
}

removed() {
  printf '%s' "$1" | jq -e --arg n "$2" '.removed | map(if type == "string" then . else .name end) | index($n) != null' >/dev/null
}

kept_detail() {
  printf '%s' "$1" | jq -r --arg n "$2" '.kept[] | select(.name == $n) | (.detail // .reason // "")'
}

# Seconds from now until a hierarchy's retention passes.
until_passes() {
  koto session hierarchy "$1" --json |
    jq -r '.retention.passes_at | sub("\\.[0-9]+"; "") | fromdateiso8601 - now | ceil'
}

hash_dir() {
  find "$1" -type f ! -name context-reads.jsonl -print0 | sort -z |
    xargs -0 shasum -a 256 | shasum -a 256
}

# Whether a watcher for an anchor is running. pgrep runs in this shell, not
# under `sh -c`, so the pattern can't match the command that searches for it.
watchers_for() {
  pgrep -f "session-start.sh --watch-[a-z]* $1( |\$)" >/dev/null
}

no_watchers_for() {
  ! watchers_for "$1"
}

# ===== The env file block =====

test_block() {
  local e p before
  e="$(envfile S1)"
  printf '# written before the hook\nexport KEEP_ME=1\n' >"$e"
  before="$(head -2 "$e")"
  p="$(payload S1 startup)"
  hook "$p" "$e"
  check "exit 0" [ "$(rc_of "$p")" = 0 ]
  check "nothing on stderr" [ ! -s "$p.err" ]
  check "nothing on stdout with an env file" [ ! -s "$p.out" ]
  local want
  want="$(printf '%s\n' "$before" '# >>> koto-skills' \
    "export KOTO_ANCHOR='anchor-S1'" "export KOTO_DISPATCHER_ANCHOR='anchor-S1'" \
    'unset NIWA_REAP_KEY' ':' '# <<< koto-skills')"
  check "env file holds the lines before it and one block" [ "$(cat "$e")" = "$want" ]
  p="$(payload S1 compact)"
  hook "$p" "$e"
  check "a second start replaces the block in place" [ "$(cat "$e")" = "$want" ]
  check "no temporary file is left" [ "$(ls -A "$T/session-env/S1")" = sessionstart-hook-1.sh ]
  local seen
  seen="$(NIWA_REAP_KEY=zz KOTO_ANCHOR=anchor-x sh -c '. "$1" && echo "$KOTO_ANCHOR|$KOTO_DISPATCHER_ANCHOR|${NIWA_REAP_KEY-unset}"' sh "$e")"
  check "a shell sourcing it sees the own anchor and no niwa key ($seen)" \
    [ "$seen" = "anchor-S1|anchor-S1|unset" ]
}

test_env_file_cases() {
  local p ctx
  p="$(payload E1 startup)"
  hook "$p" -
  ctx="$(jq -r '.hookSpecificOutput.additionalContext' "$p.out")"
  check "unset env file: one export line as additionalContext ($ctx)" \
    [ "$ctx" = "export KOTO_ANCHOR='anchor-E1'" ]
  check "the event name is SessionStart" \
    [ "$(jq -r '.hookSpecificOutput.hookEventName' "$p.out")" = SessionStart ]

  mkdir -p "$T/session-env/E2"
  printf 'echo target\n' >"$T/target.sh"
  ln -s "$T/target.sh" "$T/session-env/E2/sessionstart-hook-1.sh"
  p="$(payload E2 startup)"
  hook "$p" "$T/session-env/E2/sessionstart-hook-1.sh"
  check "a symlinked env file's target is untouched" [ "$(cat "$T/target.sh")" = "echo target" ]
  check "a symlinked env file gets the additionalContext instead" \
    [ "$(jq -r '.hookSpecificOutput.additionalContext' "$p.out")" = "export KOTO_ANCHOR='anchor-E2'" ]

  local other
  other="$(envfile E9)"
  printf 'export NOT_MINE=1\n' >"$other"
  p="$(payload E3 startup)"
  hook "$p" "$other"
  check "another session's env file is untouched" [ "$(cat "$other")" = "export NOT_MINE=1" ]
  check "another session's env file gets the additionalContext instead" \
    [ "$(jq -r '.hookSpecificOutput.additionalContext' "$p.out")" = "export KOTO_ANCHOR='anchor-E3'" ]
}

# ===== One anchor per agent session =====

test_one_anchor_per_session() {
  local e p
  agent_start
  e="$(envfile S1)"
  for src in startup resume compact; do
    p="$(payload S1 "$src")"
    agent_hook "$p" "$e"
    check "$src names anchor-S1" [ "$(anchor_in "$e")" = anchor-S1 ]
  done
  check "start, resume and compaction open one anchor" [ "$(anchors)" = anchor-S1 ]

  # A session dispatched before the /clear.
  KOTO_ANCHOR=anchor-S1 init b
  tick_done b
  local e2
  e2="$(envfile S2)"
  p="$(payload S2 clear)"
  agent_hook "$p" "$e2"
  check "/clear's env file names the new id's anchor" [ "$(anchor_in "$e2")" = anchor-S2 ]
  check "the post-clear anchor links to the one before it" \
    has_key anchor-S2 koto/links/dispatched-by/anchor-S1
  local out
  out="$(prune_json --apply --all)"
  check "the session dispatched before the clear is kept" kept "$out" b
  check "the hierarchy of b reaches the anchor the environment names" \
    sh -c 'koto session hierarchy b --json | grep -q "\"anchor-S2\""'
  agent_stop
}

test_resume_cases() {
  local e p
  # Idle-closed: reads open again after the resume.
  e="$(envfile R1)"
  hook "$(payload R1 startup)" "$e"
  KOTO_ANCHOR=anchor-R1 init b
  tick_done b
  sleep 3
  check "an idle anchor holds nothing" \
    sh -c '[ "$(KOTO_RETENTION_IDLE_PERIOD=2s koto session hierarchy b --json | jq ".keepers | length")" = 0 ]'
  hook "$(payload R1 resume)" "$e"
  check "the resumed anchor holds b again" \
    sh -c 'KOTO_RETENTION_IDLE_PERIOD=2s koto session hierarchy b --json | jq -e ".keepers | map(.name) | index(\"anchor-R1\") != null" >/dev/null'
  check "resume of an idle anchor opens no successor" [ ! -d "$T/base/anchor-R1-s2" ]

  # Removed: reopens under the same name.
  e="$(envfile R2)"
  hook "$(payload R2 startup)" "$e"
  rm -rf "$T/base/anchor-R2"
  hook "$(payload R2 resume)" "$e"
  check "a removed anchor reopens under its name" [ -d "$T/base/anchor-R2" ]
  check "the env file names it" [ "$(anchor_in "$e")" = anchor-R2 ]

  # Observed close: a successor linked to the closed one.
  e="$(envfile R3)"
  NIWA_REAP_KEY=kr3 hook "$(payload R3 startup)" "$e"
  check "the close records the observed anchor" \
    [ "$(koto session anchor close --key=kr3 --harness-session=R3)" = "closed=1" ]
  hook "$(payload R3 resume)" "$e"
  check "a resume after an observed close names the successor" [ "$(anchor_in "$e")" = anchor-R3-s2 ]
  check "the successor links to the closed anchor" has_key anchor-R3-s2 koto/links/dispatched-by/anchor-R3

  # Migrated: a successor linked to the copy.
  e="$(envfile R4)"
  hook "$(payload R4 startup)" "$e"
  local log="$T/base/anchor-R4/koto-anchor-R4.state.jsonl" seq
  seq="$(tail -n1 "$log" | jq '.seq + 1')"
  jq -cn --argjson seq "$seq" '{seq: $seq, timestamp: (now | todateiso8601), type: "session_imported",
    payload: {from_workspace: "/elsewhere", from_session: "anchor-R4", from_session_id: "old", machine_id: "m"}}' >>"$log"
  hook "$(payload R4 resume)" "$e"
  check "a resume of a migrated anchor names the successor" [ "$(anchor_in "$e")" = anchor-R4-s2 ]
  check "the successor links to the migrated anchor" has_key anchor-R4-s2 koto/links/dispatched-by/anchor-R4

  # A resume under a new id finds its predecessor through the transcript.
  mkdir -p "$T/projects/one" "$T/projects/two"
  hook "$(payload R5 startup "$T/projects/one/R5.jsonl")" "$(envfile R5)"
  e="$(envfile R6)"
  hook "$(payload R6 resume "$T/projects/one/R5.jsonl")" "$e"
  check "a resume under a new id opens its own anchor" [ "$(anchor_in "$e")" = anchor-R6 ]
  check "it links to the anchor with the same transcript path" has_key anchor-R6 koto/links/dispatched-by/anchor-R5
  e="$(envfile R7)"
  hook "$(payload R7 resume "$T/projects/two/R5.jsonl")" "$e"
  check "it links to the anchor the transcript's name names" has_key anchor-R7 koto/links/dispatched-by/anchor-R5
}

# ===== No koto =====

test_no_koto() {
  local e p
  if env PATH=/usr/bin:/bin sh -c 'command -v koto' >/dev/null; then
    echo "  skip [$CURRENT]: koto is in /usr/bin or /bin"
    return
  fi
  e="$(envfile K1)"
  printf 'export BEFORE=1\n' >"$e"
  p="$(payload K1 startup)"
  PATH=/usr/bin:/bin hook "$p" "$e"
  check "exit 0 with no koto" [ "$(rc_of "$p")" = 0 ]
  check "nothing on stderr with no koto" [ ! -s "$p.err" ]
  check "nothing on stdout with no koto" [ ! -s "$p.out" ]
  check "the env file is untouched with no koto" [ "$(cat "$e")" = "export BEFORE=1" ]
  check "the guarded close form exits 0 with no koto" \
    env PATH=/usr/bin:/bin NIWA_REAP_KEY=k NIWA_SESSION_ID=K1 sh -c "$GUARDED_CLOSE"

  # A koto that fails every call.
  mkdir -p "$T/stub"
  printf '#!/bin/sh\nexit 1\n' >"$T/stub/koto"
  chmod +x "$T/stub/koto"
  e="$(envfile K2)"
  p="$(payload K2 startup)"
  PATH="$T/stub:$PATH" KOTO_ANCHOR=anchor-dispatcher KOTO_DISPATCHER_ANCHOR=anchor-dispatcher hook "$p" "$e"
  check "exit 0 when koto fails" [ "$(rc_of "$p")" = 0 ]
  check "a failed open unsets both variables" grep -qx 'unset KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR' "$e"
  check "a failed open never leaves the dispatcher's anchor" sh -c '! grep -q anchor-dispatcher "$1"' sh "$e"

  # A koto that stalls is killed after the hook's limit.
  mkdir -p "$T/hang"
  printf '#!/bin/sh\necho $$ >"%s/hang.pid"\nexec sleep 60\n' "$T" >"$T/hang/koto"
  chmod +x "$T/hang/koto"
  e="$(envfile K3)"
  p="$(payload K3 startup)"
  PATH="$T/hang:$PATH" KOTO_ANCHOR=anchor-dispatcher KOTO_DISPATCHER_ANCHOR=anchor-dispatcher hook "$p" "$e"
  local ms
  ms="$(cat "$p.ms")"
  check "a stalled koto is cut off within the limit (${ms} ms)" [ "$ms" -lt 10000 ]
  check "exit 0 when koto stalls" [ "$(rc_of "$p")" = 0 ]
  check "a stalled open unsets both variables" grep -qx 'unset KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR' "$e"
  check "the stalled koto is gone" sh -c '! kill -0 "$(cat "$1")" 2>/dev/null' sh "$T/hang.pid"
}

# ===== The observed marker =====

test_late_marker() {
  local e p dir
  agent_start
  e="$(envfile L1)"
  dir="$T/session-env/L1"
  local t0
  t0="$(date +%s)"
  p="$(payload L1 startup)"
  agent_hook "$p" "$e"
  check "no marker before niwa's line" sh -c '! koto context list anchor-L1 --prefix koto/anchor/ | grep -q observed'
  while [ $(($(date +%s) - t0)) -lt 30 ]; do sleep 1; done
  printf "export NIWA_REAP_KEY='key-late'\n" >"$dir/sessionstart-hook-0.sh"
  while ! observed anchor-L1 && [ $(($(date +%s) - t0)) -lt 180 ]; do sleep 2; done
  check "the anchor carries the marker within three minutes" observed anchor-L1
  check "the marker holds the unquoted value" \
    [ "$(koto session anchor close --key=key-late --harness-session=L1)" = "closed=1" ]

  # Never written.
  e="$(envfile L2)"
  p="$(payload L2 startup)"
  agent_hook "$p" "$e"
  sleep 6
  check "with no line the anchor carries no marker" sh -c '! koto context list anchor-L2 --prefix koto/anchor/ | grep -q observed'
  check "the watcher is running while the agent process lives" watchers_for anchor-L2
  local seen
  seen="$(NIWA_REAP_KEY=leaked sh -c '. "$1" && echo "${NIWA_REAP_KEY-unset}"' sh "$e")"
  check "a child's shell sees no niwa variable ($seen)" [ "$seen" = unset ]
  agent_stop
  sleep 5
  check "the watcher exits once the agent process is gone" no_watchers_for anchor-L2
}

test_inherited_key() {
  local e p
  hook "$(payload P1 startup)" "$(envfile P1)"
  # A process launched from P1's shell: it inherits P1's variables, and niwa's
  # key reached it too.
  agent_start KOTO_ANCHOR=anchor-P1 KOTO_DISPATCHER_ANCHOR=anchor-P1 NIWA_REAP_KEY=kc
  e="$(envfile C1)"
  p="$(payload C1 startup)"
  agent_hook "$p" "$e"
  check "a child of an agent session gets no marker" sh -c '! koto context list anchor-C1 --prefix koto/anchor/ | grep -q observed'
  check "and starts no watcher" no_watchers_for anchor-C1
  check "it records its parent as dispatcher" has_key anchor-C1 koto/links/dispatched-by/anchor-P1
  check "its shells name its own anchor" [ "$(anchor_in "$e")" = anchor-C1 ]
  agent_stop

  e="$(envfile C2)"
  p="$(payload C2 startup)"
  KOTO_DISPATCHER_ANCHOR=anchor-P1 NIWA_REAP_KEY=kw hook "$p" "$e"
  check "a niwa worker records the marker" observed anchor-C2
  check "a niwa worker records its dispatcher" has_key anchor-C2 koto/links/dispatched-by/anchor-P1
  check "its close records it" [ "$(koto session anchor close --key=kw --harness-session=C2)" = "closed=1" ]
}

# ===== Retention with the hook's anchors =====

test_first_dispatch() {
  local ea eb p
  ea="$(envfile A1)"
  hook "$(payload A1 startup)" "$ea"
  # B starts from A's shell, before A ran any koto command of its own.
  eb="$(envfile B1)"
  p="$(payload B1 startup)"
  (. "$ea" && hook "$p" "$eb")
  check "B's anchor links to the dispatcher's" has_key anchor-B1 koto/links/dispatched-by/anchor-A1
  check "B's shells name B's own anchor" [ "$(anchor_in "$eb")" = anchor-B1 ]
  (. "$eb" && init b && printf 'done\n' | koto context add b result && tick_done b)
  check "b is held by B's anchor" has_key b koto/links/held-by/anchor-B1
  local out detail
  out="$(prune_json --apply --all)"
  check "a prune keeps b" kept "$out" b
  detail="$(kept_detail "$out" b)"
  case "$detail" in
    *anchor-A1*) ;;
    *) fail "the prune names the dispatcher's anchor ($detail)" ;;
  esac
  local before after
  before="$(hash_dir "$T/base/b")"
  (. "$ea" && koto context get b result >/dev/null && koto context exists b result)
  after="$(hash_dir "$T/base/b")"
  check "reads from A leave b byte-identical" [ "$before" = "$after" ]
}

test_idle_close() {
  export KOTO_RETENTION_IDLE_PERIOD=6s KOTO_RETENTION_KEEP=6s KOTO_RETENTION_STALE_AFTER=1h
  local ea out
  ea="$(envfile I1)"
  hook "$(payload I1 startup)" "$ea"
  (. "$ea" && init b && tick_done b)
  (. "$ea" && koto init s --template "$T/hook-loop.md" >/dev/null)
  local i
  for i in 1 2 3 4; do
    sleep 3
    (. "$ea" && koto next s --with-data '{"go": "more"}' >/dev/null)
    out="$(prune_json --apply --all)"
    check "b is kept while the agent session ticks ($i)" kept "$out" b
  done
  (. "$ea" && koto next s --with-data '{"go": "stop"}' >/dev/null)
  out="$(prune_json)"
  check "inside the idle period b is kept" kept "$out" b
  case "$(kept_detail "$out" b)" in
    *anchor-I1*) ;;
    *) fail "the prune names the anchor ($(kept_detail "$out" b))" ;;
  esac
  sleep 7
  local release
  release="$(koto session hierarchy b --json | jq -c '.retention.release')"
  check "after the idle period the anchor's idle close releases b ($release)" \
    sh -c 'printf "%s" "$1" | jq -e ".member == \"anchor-I1\" and .close_kind == \"idle\"" >/dev/null' sh "$release"
  out="$(prune_json --apply --all)"
  check "inside the retention b is kept" kept "$out" b
  local wait_s
  wait_s="$(until_passes b)"
  [ "$wait_s" -gt 0 ] && sleep $((wait_s + 1))
  out="$(prune_json --apply --all)"
  check "after the retention b is removed" removed "$out" b
  # The removal lists the anchor with its idle-close instant as its stop.
  check "the removal names the idle close" \
    sh -c 'printf "%s" "$1" | jq -e --argjson rel "$2" ".removed | map(select(.name == \"anchor-I1\" and .stopped_at == \$rel.at)) | length == 1" >/dev/null' \
    sh "$out" "$release"
  unset KOTO_RETENTION_IDLE_PERIOD KOTO_RETENTION_KEEP KOTO_RETENTION_STALE_AFTER
}

test_bare_shell() {
  export KOTO_RETENTION_KEEP=4s
  local out
  init d
  init b --dispatched-by d
  tick_done b
  out="$(prune_json --apply --all)"
  check "b is kept while its dispatcher is open" kept "$out" b
  tick_done d
  out="$(prune_json --apply --all)"
  check "b is kept for the retention after its dispatcher finishes" kept "$out" b
  local wait_s
  wait_s="$(until_passes b)"
  [ "$wait_s" -gt 0 ] && sleep $((wait_s + 1))
  out="$(prune_json --apply --all)"
  check "then b is removed" removed "$out" b
  unset KOTO_RETENTION_KEEP
}

test_no_niwa_hierarchy() {
  local ea er out before after
  check "no niwa variable is set" [ -z "$(compgen -v NIWA_)" ]
  ea="$(envfile N1)"
  hook "$(payload N1 startup)" "$ea"
  er="$(envfile N2)"
  hook "$(payload N2 startup)" "$er"
  (. "$ea" && init p && init c --parent p && init g --parent c &&
    printf 'x\n' | koto context add c result && printf 'y\n' | koto context add g result &&
    tick_done g && tick_done c)
  out="$(prune_json --apply --all)"
  check "a finished child is kept" kept "$out" c
  check "a finished grandchild is kept" kept "$out" g
  before="$(hash_dir "$T/base/c")$(hash_dir "$T/base/g")"
  (. "$er" && koto context get c result >/dev/null && koto context exists g result)
  after="$(hash_dir "$T/base/c")$(hash_dir "$T/base/g")"
  check "reads from another agent session leave them byte-identical" [ "$before" = "$after" ]
  (. "$ea" && tick_done p)
  out="$(prune_json --apply --all)"
  check "a finished parent is kept" kept "$out" p
  check "its children stay with it" sh -c 'printf "%s" "$1" | jq -e ".kept | map(.name) | (index(\"c\") != null and index(\"g\") != null)" >/dev/null' sh "$out"
}

# ===== Injection and cross-anchor cases =====

test_injection() {
  local e p before id
  e="$(envfile evil)"
  printf 'export UNTOUCHED=1\n' >"$e"
  before="$(cat "$e")"
  for id in "a'b" "\$(touch $T/pwned)" "a\"b" "$(printf 'a%.0s' $(seq 129))"; do
    p="$(payload "$id" startup)"
    hook "$p" "$e"
    check "a bad id exits 0" [ "$(rc_of "$p")" = 0 ]
    check "a bad id writes nothing to stdout" [ ! -s "$p.out" ]
  done
  p="$(raw_payload '{"session_id":"a\nb","source":"startup"}')"
  hook "$p" "$e"
  check "a newline in the id writes nothing" [ ! -s "$p.out" ]
  p="$(raw_payload '{"session_id":"`touch pwned`","source":"startup"}')"
  hook "$p" "$e"
  check "bad ids leave the env file untouched" [ "$(cat "$e")" = "$before" ]
  check "bad ids open no anchor" [ -z "$(anchors)" ]
  check "nothing in an id ran" [ ! -e "$T/pwned" ]
  check "nothing in an id ran in the working directory" [ ! -e "$T/work/pwned" ]

  # Values that start with a dash are values, not flags.
  e="$(envfile -lead)"
  p="$(payload -lead startup -transcript.jsonl)"
  NIWA_REAP_KEY=-key hook "$p" "$e"
  check "an id starting with a dash opens its anchor" [ "$(anchor_in "$e")" = anchor--lead ]
  check "a key starting with a dash is recorded" observed anchor--lead

  # A watcher reads its siblings and never sources them, and writes only
  # into the anchor it names.
  hook "$(payload O1 startup)" "$(envfile O1)"
  local other_before
  other_before="$(hash_dir "$T/base/anchor-O1")"
  agent_start
  e="$(envfile V1)"
  {
    printf 'touch %s/sourced\n' "$T"
    printf "export NIWA_REAP_KEY='\$(touch %s/pwned2)'\n" "$T"
    printf "export NIWA_REAP_KEY='bad key'\n"
    printf "export NIWA_REAP_KEY='kv'\n"
  } >"$T/session-env/V1/sessionstart-hook-0.sh"
  p="$(payload V1 startup)"
  agent_hook "$p" "$e"
  local i=0
  while ! observed anchor-V1 && [ "$i" -lt 15 ]; do sleep 1; i=$((i + 1)); done
  check "the watcher records the valid key" observed anchor-V1
  check "the watcher never sources a sibling" [ ! -e "$T/sourced" ]
  check "the watcher never runs a value" [ ! -e "$T/pwned2" ]
  check "the valid key is the one recorded" \
    [ "$(koto session anchor close --key=kv --harness-session=V1)" = "closed=1" ]
  # A watcher started with another anchor in its environment.
  hook "$(payload V2 startup)" -
  KOTO_ANCHOR=anchor-O1 bash "$HOOK" --watch-key anchor-V2 "$T/session-env/V1" "" &
  local wpid=$!
  i=0
  while ! observed anchor-V2 && [ "$i" -lt 15 ]; do sleep 1; i=$((i + 1)); done
  kill "$wpid" 2>/dev/null
  check "the watcher marks the anchor it names" observed anchor-V2
  check "and never the one in its environment" [ "$(hash_dir "$T/base/anchor-O1")" = "$other_before" ]
  agent_stop
}

# ===== The synchronous part's time =====

test_timing() {
  local i p e times=() t
  for i in $(seq 1 20); do
    e="$(envfile "T$i")"
    p="$(payload "T$i" startup)"
    hook "$p" "$e"
    times+=("$(cat "$p.ms")")
  done
  local sorted p50 p95
  sorted="$(printf '%s\n' "${times[@]}" | sort -n)"
  p50="$(printf '%s\n' "$sorted" | sed -n 10p)"
  p95="$(printf '%s\n' "$sorted" | sed -n 19p)"
  echo "  timing: 20 starts, p50 ${p50} ms, p95 ${p95} ms (each a startup with an env file and a detached watcher)"
  if [ -n "${CI:-}" ] || [ "${KOTO_HOOK_TIMING:-}" = enforce ]; then
    check "p95 of the synchronous part under 300 ms (${p95} ms)" [ "$p95" -lt 300 ]
  fi
}

ALL="test_block test_env_file_cases test_one_anchor_per_session test_resume_cases
test_no_koto test_late_marker test_inherited_key test_first_dispatch
test_idle_close test_bare_shell test_no_niwa_hierarchy test_injection test_timing"

selected="${*:-$ALL}"
for t in $selected; do
  case "$t" in test_*) ;; *) t="test_$t" ;; esac
  CURRENT="$t"
  echo "$t"
  setup
  "$t"
  teardown
done

if [ "$FAILED" -ne 0 ]; then
  echo "session-start.test.sh: FAILED"
  exit 1
fi
echo "session-start.test.sh: all passed"
