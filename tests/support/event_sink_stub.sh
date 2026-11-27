# The body of the stub event sink the tests install. The installer writes
# two lines above this text: a `#!/bin/sh` line and `CTL='<directory>'`,
# the stub's control directory. Nothing here reads koto's environment for
# its settings: a test changes the stub's behaviour through files in CTL.
#
# Each call gets a directory, CTL/calls/<n>, numbered in the order the
# calls started, holding:
#   at          the time the call started, seconds since the epoch
#   argv        the arguments, one per line
#   env         the environment the sink was given
#   stdin       everything read from stdin
#   read        present once stdin reached its end
#   done        present once the call is about to exit
#   dir_exists  yes or no, when CTL/require_dir names a directory to check
#   child_pid   the pid of a background child, when CTL/fork exists
#
# Settings, each a file in CTL with one value per line. Call n uses line
# n+1, and a call past the end uses the last line:
#   exit        the exit code (default 0)
#   sleep       seconds to sleep after reading stdin (default 0)

n=0
while ! mkdir "$CTL/calls/$n" 2>/dev/null; do
    n=$((n + 1))
    if [ "$n" -gt 100000 ]; then
        exit 70
    fi
done
call="$CTL/calls/$n"

if command -v perl >/dev/null 2>&1; then
    perl -MTime::HiRes=time -e 'printf "%.6f\n", time' >"$call/at"
else
    date +%s >"$call/at"
fi
for a in "$@"; do
    printf '%s\n' "$a"
done >"$call/argv"
env >"$call/env"

if [ -f "$CTL/require_dir" ]; then
    if [ -d "$(cat "$CTL/require_dir")" ]; then
        echo yes >"$call/dir_exists"
    else
        echo no >"$call/dir_exists"
    fi
fi
if [ -f "$CTL/fork" ]; then
    sleep 300 &
    echo $! >"$call/child_pid"
fi

cat >"$call/stdin"
: >"$call/read"

# The value of setting $1 for this call, or $2 when the file isn't there.
pick() {
    if [ ! -f "$CTL/$1" ]; then
        echo "$2"
        return
    fi
    v=$(sed -n "$((n + 1))p" "$CTL/$1")
    if [ -z "$v" ]; then
        v=$(tail -n 1 "$CTL/$1")
    fi
    echo "${v:-$2}"
}

s=$(pick sleep 0)
if [ "$s" != 0 ]; then
    sleep "$s"
fi
: >"$call/done"
exit "$(pick exit 0)"
