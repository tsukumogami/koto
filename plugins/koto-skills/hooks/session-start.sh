#!/usr/bin/env bash
# SessionStart hook: open this agent session's koto anchor.
#
# Every agent session with the koto-skills plugin gets one anchor,
# `anchor-<session id>`, that keeps the koto sessions it starts, dispatches
# or reads from being removed while the agent session lives. The hook runs on
# startup, resume, compact and /clear, and:
#
#   1. reads `session_id`, `source`, `transcript_path` and `cwd` from the
#      payload with jq, and gives up unless the id is a plain name;
#   2. computes the agent process's token, `<host>:<pid>:<epoch-start>`, by
#      walking up from its parent past shells, so koto can link a /clear to
#      the anchor the same process ran before;
#   3. keeps the dispatcher it inherited (`KOTO_DISPATCHER_ANCHOR`, else
#      `KOTO_ANCHOR`) and, only when no `KOTO_ANCHOR` was inherited, niwa's
#      reap key (`NIWA_REAP_KEY`): a process launched from another agent
#      session's shell carries that session's `KOTO_ANCHOR`, and its parent's
#      key must not mark it;
#   4. runs `koto session anchor open` with those, under an environment that
#      names no anchor;
#   5. writes `export KOTO_ANCHOR` and `export KOTO_DISPATCHER_ANCHOR`, both
#      naming its own anchor, into its block of `CLAUDE_ENV_FILE`, or emits
#      the `KOTO_ANCHOR` export line as additionalContext when that file
#      can't be written safely;
#   6. detaches a watcher when niwa's key may still arrive (startup, or a
#      resume that opened a successor) or when a /clear's predecessor may
#      still gain its marker.
#
# Steps 1 to 5 are the synchronous part, bounded by
# `hooks/test/session-start.test.sh`. The hook always exits 0 and prints
# nothing on stderr: without koto or jq it does nothing.
#
# The watchers run this same script with a `--watch-key` or
# `--watch-inherit` argument.

trap 'exit 0' EXIT
exec 2>/dev/null

NAME_RE='^[A-Za-z0-9_-]{1,128}$'
ANCHOR_RE='^anchor-[A-Za-z0-9_-]+(-s[0-9]+)?$'
BLOCK_BEGIN='# >>> koto-skills'
BLOCK_END='# <<< koto-skills'
WATCH_INTERVAL=2
WATCH_LIMIT=180

# Runs koto with no anchor variable and no KOTO_SESSION_* variable, so a
# watcher never acts under an inherited anchor.
koto_clean() {
  local unsets=() v
  for v in $(compgen -v KOTO_SESSION_); do
    unsets+=(-u "$v")
  done
  env -u KOTO_ANCHOR -u KOTO_DISPATCHER_ANCHOR "${unsets[@]}" koto "$@"
}

# Whether the process in the token is still running; true with no pid.
agent_alive() {
  [ -z "$1" ] || kill -0 "$1"
}

# --watch-key <anchor> <env dir> <pid>: read, never source, the sibling env
# files for niwa's `export NIWA_REAP_KEY='<value>'` line and record the value
# as the anchor's observed marker.
watch_key() {
  local anchor="$1" dir="$2" pid="$3" waited=0 f line v out
  while [ "$waited" -lt "$WATCH_LIMIT" ]; do
    agent_alive "$pid" || return 0
    for f in "$dir"/sessionstart-hook-*.sh; do
      [ -f "$f" ] && [ ! -L "$f" ] || continue
      while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
          "export NIWA_REAP_KEY="*) ;;
          *) continue ;;
        esac
        v="${line#export NIWA_REAP_KEY=}"
        v="${v#\'}"
        v="${v%\'}"
        [[ "$v" =~ $NAME_RE ]] || continue
        out="$(koto_clean session anchor observe --anchor "$anchor" --key="$v")"
        case "$out" in
          *observed=yes*) return 0 ;;
        esac
      done <"$f"
    done
    sleep "$WATCH_INTERVAL"
    waited=$((waited + WATCH_INTERVAL))
  done
}

# Seconds since the epoch for an RFC 3339 UTC time, or nothing.
epoch_of() {
  local t="${1%%.*}"
  t="${t%Z}"
  date -u -j -f '%Y-%m-%dT%H:%M:%S' "$t" +%s || date -u -d "${t}Z" +%s
}

# --watch-inherit <anchor> <inherit_until> <pid>: copy the /clear
# predecessor's marker once it has one, until inherit_until and never longer
# than the watch limit.
watch_inherit() {
  local anchor="$1" until_s now limit out
  local pid="$3"
  now="$(date +%s)"
  limit=$((now + WATCH_LIMIT))
  until_s="$(epoch_of "$2")"
  if [ -n "$until_s" ] && [ "$until_s" -lt "$limit" ]; then
    limit="$until_s"
  fi
  while [ "$(date +%s)" -lt "$limit" ]; do
    agent_alive "$pid" || return 0
    out="$(koto_clean session anchor observe --anchor "$anchor" --inherit)"
    case "$out" in
      *observed=yes*) return 0 ;;
    esac
    sleep "$WATCH_INTERVAL"
  done
}

case "${1:-}" in
  --watch-key)
    watch_key "$2" "$3" "$4"
    exit 0
    ;;
  --watch-inherit)
    watch_inherit "$2" "$3" "$4"
    exit 0
    ;;
esac

command -v koto >/dev/null || exit 0
command -v jq >/dev/null || exit 0

payload="$(cat)"
{
  IFS= read -r -d '' session_id
  IFS= read -r -d '' source
  IFS= read -r -d '' transcript_path
  IFS= read -r -d '' payload_cwd
} < <(printf '%s' "$payload" | jq -j '
  (.session_id, .source, .transcript_path, .cwd)
  | (if type == "string" then . else "" end) + "\u0000"')

[[ "$session_id" =~ $NAME_RE ]] || exit 0
case "$source" in
  startup | resume | compact | clear) ;;
  *) source=other ;;
esac

# Prints "<ppid> <epoch-start> <command name>" for a pid.
proc_info() {
  local pid="$1" stat rest btime hz line ppid lstart start
  if [ -r "/proc/$pid/stat" ]; then
    read -r stat <"/proc/$pid/stat" || return 1
    local comm="${stat#*(}"
    comm="${comm%)*}"
    rest="${stat##*) }"
    # shellcheck disable=SC2086
    set -- $rest
    # After the command name, field 4 (ppid) is $2 and field 22 (the start
    # time in clock ticks since boot) is ${20}.
    btime="$(sed -n 's/^btime //p' /proc/stat)"
    hz="$(getconf CLK_TCK)"
    [ -n "$btime" ] && [ -n "$hz" ] || return 1
    printf '%s %s %s\n' "$2" "$((btime + ${20} / hz))" "$comm"
    return 0
  fi
  line="$(LC_ALL=C ps -o ppid=,lstart=,comm= -p "$pid")" || return 1
  # shellcheck disable=SC2086
  set -- $line
  [ "$#" -ge 7 ] || return 1
  ppid="$1"
  lstart="$2 $3 $4 $5 $6"
  shift 6
  start="$(LC_ALL=C date -j -f '%a %b %d %T %Y' "$lstart" +%s)" ||
    start="$(LC_ALL=C date -d "$lstart" +%s)" || return 1
  printf '%s %s %s\n' "$ppid" "$start" "$*"
}

# The agent process: the first ancestor that isn't a shell.
agent_pid=""
token=""
pid="$PPID"
for _ in 1 2 3 4 5 6 7 8; do
  info="$(proc_info "$pid")" || break
  ppid="${info%% *}"
  rest="${info#* }"
  start="${rest%% *}"
  comm="${rest#* }"
  comm="${comm##*/}"
  comm="${comm#-}"
  case "$comm" in
    sh | bash | zsh | dash | ksh | mksh | fish | tcsh | csh)
      [ -n "$ppid" ] && [ "$ppid" -gt 1 ] || break
      pid="$ppid"
      continue
      ;;
  esac
  agent_pid="$pid"
  token="${HOSTNAME:-$(uname -n)}:$pid:$start"
  break
done

dispatcher="${KOTO_DISPATCHER_ANCHOR:-${KOTO_ANCHOR:-}}"
[[ "$dispatcher" =~ $ANCHOR_RE ]] || dispatcher=""
inherited_anchor="${KOTO_ANCHOR:-}"
key=""
if [ -z "$inherited_anchor" ]; then
  key="${NIWA_REAP_KEY:-}"
  [[ "$key" =~ $NAME_RE ]] || key=""
fi

args=(session anchor open --harness-session "$session_id" --source "$source")
[ -n "$token" ] && args+=(--process "$token")
[ -n "$dispatcher" ] && args+=(--dispatched-by "$dispatcher")
[ -n "$key" ] && args+=(--observed-key "$key")
[ -n "$transcript_path" ] && args+=(--transcript-path "$transcript_path")
[ -n "$payload_cwd" ] && [ -d "$payload_cwd" ] && args+=(--workspace "$payload_cwd")

out="$(env -u KOTO_ANCHOR -u KOTO_DISPATCHER_ANCHOR -u CLAUDE_CODE_SESSION_ID koto "${args[@]}")"

anchor=""
opened=""
observed=""
previous=""
inherit_until=""
while IFS= read -r line; do
  case "$line" in
    anchor=*) anchor="${line#anchor=}" ;;
    opened=*) opened="${line#opened=}" ;;
    observed=*) observed="${line#observed=}" ;;
    previous=*) previous="${line#previous=}" ;;
    inherit_until=*) inherit_until="${line#inherit_until=}" ;;
  esac
done <<<"$out"
[[ "$anchor" =~ $ANCHOR_RE ]] || anchor=""

# The env file is written only when it is a regular file, or not there yet,
# in a directory of this session's own that isn't a symlink.
env_dir=""
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  d="${CLAUDE_ENV_FILE%/*}"
  if [ "$d" != "$CLAUDE_ENV_FILE" ] && [ "${d##*/}" = "$session_id" ] &&
    [ -d "$d" ] && [ ! -L "$d" ] &&
    [ ! -L "$CLAUDE_ENV_FILE" ] &&
    { [ -f "$CLAUDE_ENV_FILE" ] || [ ! -e "$CLAUDE_ENV_FILE" ]; }; then
    env_dir="$d"
  fi
fi

if [ -n "$env_dir" ]; then
  tmp="$(mktemp "$env_dir/.koto-skills.XXXXXX")" || exit 0
  {
    if [ -f "$CLAUDE_ENV_FILE" ]; then
      inside=0
      while IFS= read -r line || [ -n "$line" ]; do
        if [ "$inside" -eq 0 ] && [ "$line" = "$BLOCK_BEGIN" ]; then
          inside=1
        elif [ "$inside" -eq 1 ]; then
          [ "$line" = "$BLOCK_END" ] && inside=0
        else
          printf '%s\n' "$line"
        fi
      done <"$CLAUDE_ENV_FILE"
    fi
    printf '%s\n' "$BLOCK_BEGIN"
    if [ -n "$anchor" ]; then
      printf "export KOTO_ANCHOR='%s'\n" "$anchor"
      printf "export KOTO_DISPATCHER_ANCHOR='%s'\n" "$anchor"
    else
      printf 'unset KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR\n'
    fi
    printf 'unset NIWA_REAP_KEY\n'
    printf ':\n'
    printf '%s\n' "$BLOCK_END"
  } >"$tmp" || {
    rm -f "$tmp"
    exit 0
  }
  mv -f "$tmp" "$CLAUDE_ENV_FILE" || rm -f "$tmp"
elif [ -n "$anchor" ]; then
  jq -n --arg ctx "export KOTO_ANCHOR='$anchor'" '{
    hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $ctx}
  }'
fi

[ -n "$anchor" ] || exit 0

# Starts this script again as a watcher in a session of its own, with every
# descriptor redirected, so the hook returns without waiting for it.
detach() {
  if command -v setsid >/dev/null; then
    setsid bash "${BASH_SOURCE[0]}" "$@" </dev/null >/dev/null 2>&1 &
  else
    nohup bash "${BASH_SOURCE[0]}" "$@" </dev/null >/dev/null 2>&1 &
  fi
}

if [ -z "$inherited_anchor" ] && [ -z "$key" ] && [ -n "$env_dir" ]; then
  if [ "$source" = startup ] || { [ "$source" = resume ] && [ "$opened" = successor ]; }; then
    detach --watch-key "$anchor" "$env_dir" "$agent_pid"
  fi
fi
if [ "$source" = clear ] && [ "$observed" = no ] && [ -n "$previous" ]; then
  detach --watch-inherit "$anchor" "$inherit_until" "$agent_pid"
fi

exit 0
