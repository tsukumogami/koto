# Verification: a second program reads koto sessions through koto's reader

This records a walk of the four Evidence clauses of the shared session reader
milestone against what shipped on koto main. All six steps of
`PLAN-koto-shared-session-reader` are merged: Step 5 as 6530ae76 (koto#768)
and Step 6 as f6c8e275 (koto#778). koto#778's description gives five
real-store runs for the reviewer with their result lines blank; this record
runs them, adapted to the merged code.

Each section quotes its clause, gives the commands as they were run, and
reports counts and diff results only. No session content or context value
appears here. Each reading says what the runs show; it isn't a verdict on the
milestone.

## Host, binaries and store

- macOS on arm64, run on 2026-11-27 between 05:40Z and 06:30Z. The host's koto
  is 0.41.0 on the cloud session backend, which this session's environment
  selects through `KOTO_SESSION_*` variables. No koto release carries Steps 5 or
  6, so every binary below was built from source, one build at a time, with
  `CARGO_INCREMENTAL=0 cargo build --release --locked -p koto` at 22bc3a7e and
  at 865e5ce4, and `CARGO_INCREMENTAL=0 cargo build --release --locked -p koto
  -p koto-session-list` at 90aeaaae (the workspace has no default members, so
  `koto-session-list` builds only when named, as validate.yml names it). Each
  run names its binary by commit:

  | Name | Commit | `koto version` | Role |
  |------|--------|----------------|------|
  | `PRE` | 22bc3a7e (v0.39.0) | `koto 0.39.0 (22bc3a7e ...)` | dashboard before the split |
  | `OLD` | 865e5ce4 | `koto 0.41.0-dev+865e5ce4 (865e5ce4 ...)` | main before koto#778 |
  | `MAIN` | 90aeaaae | `koto 0.41.0-dev+90aeaaae (90aeaaae ...)` | current main |
  | `LIST` | 90aeaaae | (no version command) | `koto-session-list` from current main |

  v0.39.0 is the newest tag that doesn't contain the split's first commit,
  cc9479be (Step 2, koto#742), and its `koto dashboard` already has `--once`
  and `--all`, so no other pre-split commit was needed. The tag and the
  version strings come from this, run in the koto checkout after the setup
  below, with a `cd "$O"` back before clause 1:

  ```bash
  git tag --no-contains cc9479be --sort=-v:refname | head -1   # v0.39.0
  for b in "$PRE" "$OLD" "$MAIN"; do "$b" version; done
  ```

- The store is one APFS clone of the host's `~/.koto`, made with
  `cp -cR ~/.koto "$C/root"` into a fresh `mktemp -d` directory: 1,279,399
  entries and 9,534 session directories. The host's `config.toml` names no session backend, so a run with
  no `KOTO_SESSION_*` variable reads the clone as a local store. Every run on
  the clone goes through koto#778's runner, which starts the binary with an
  empty environment:

  ```bash
  run() { env -i PATH=/usr/bin:/bin HOME="$C/home" KOTO_HOME="$C/root" XDG_CACHE_HOME="$C/home/cache" "$@"; }
  ```

  The only runs against the live store are clause 3's hundred sandboxed runs.
  The clone was left in place when the runs ended, for a later cleanup.

Every block ran in bash, not zsh: several rely on bash splitting an unquoted
variable into words (`set -- $p`, `"$LIST" $a`, `$E claude ...`), which zsh
doesn't do. The setup below ran once, in one bash shell, before clause 1.
Every later block runs
in that shell, on the same clone, in the order the record gives them, and reads
the earlier blocks' outputs and variables. `B` is the directory the release binaries were copied to:

```bash
B=<directory holding the release binaries>
C=$(mktemp -d); cp -cR ~/.koto "$C/root"; O=$C/out; mkdir -p "$O"; cd "$O"
find "$C/root" | wc -l                                          # entries
find "$C/root/sessions" -mindepth 1 -maxdepth 1 -type d | wc -l  # session directories
# names only, never values: the cloud variables this session sets (6), and
# the sections and keys in the clone's config.toml ([decider] mode, [machine] id)
env | cut -d= -f1 | grep -c '^KOTO_SESSION_'
grep -oE '^\[[^]]+\]|^[A-Za-z_]+' "$C/root/config.toml"
PRE=$B/koto-22bc3a7e OLD=$B/koto-865e5ce4 MAIN=$B/koto-90aeaaae
LIST=$B/koto-session-list-90aeaaae
run() { env -i PATH=/usr/bin:/bin HOME="$C/home" KOTO_HOME="$C/root" XDG_CACHE_HOME="$C/home/cache" "$@"; }
```

`koto dashboard --once` prints ten tab-separated columns (verified by reading
`src/cli/`): 1 id, 2 state,
3 elapsed, 4 status, 5 intent, 6 template, 7 idle, 8 liveness, 9 machine and
10 workspace. Columns 3 and 7 are durations measured from the clock when the
binary runs. `koto-session-list once` prints columns 1 to 8 of the same rows
and leaves out machine and workspace (verified by reading its source).

## Clause 1: the dashboard before and after

> A reviewer runs `koto dashboard` against the same session store before and after the change and sees the same sessions, states and liveness classes.

Each view ran three times back to back on the clone, `PRE`, then `MAIN`, then
`PRE` again, so a difference that comes from the clock shows up between the two
`PRE` runs as well:

```bash
for v in "" --all; do
  run "$PRE"  dashboard --once $v > pre1$v 2> pre1$v.err; echo "exit $?"
  run "$MAIN" dashboard --once $v > main$v 2> main$v.err; echo "exit $?"
  run "$PRE"  dashboard --once $v > pre2$v 2> pre2$v.err; echo "exit $?"
  for f in pre1 main pre2; do
    wc -l < $f$v; awk -F'\t' '{print NF}' $f$v | sort -u; wc -l < $f$v.err
    cut -f1,2,4,5,6,8 $f$v > $f$v.cut; cut -f9,10 $f$v > $f$v.cut910
  done
  cmp pre1$v.cut main$v.cut && cmp main$v.cut pre2$v.cut && echo "1,2,4,5,6,8 identical"
  cmp pre1$v.cut910 main$v.cut910 && echo "9,10 identical"
  cmp pre1$v.err main$v.err && echo "stderr identical"
  # full rows that differ, by position, and which columns differ on them:
  # PRE against MAIN, MAIN against PRE again, and PRE against PRE again
  for p in "pre1 main" "main pre2" "pre1 pre2"; do set -- $p
    paste -d'\n' $1$v $2$v | awk 'NR%2{a=$0;next} a!=$0{c++} END{print c+0}'
    paste $1$v $2$v | awk -F'\t' '{for(i=1;i<=10;i++) if($i!=$(i+10)) n[i]++} END{for(i in n) print i, n[i]}'
  done
  cut -f8 main$v | sort | uniq -c   # MAIN's liveness classes
done
```

| View | Rows (PRE, MAIN, PRE again) | Exit | Columns 1, 2, 4, 5, 6, 8 | Columns 9, 10 | stderr | Full rows that differ |
|------|------|------|------|------|------|------|
| default | 177, 177, 177 | 0, 0, 0 | identical | identical | empty, identical | 26 of 177 |
| `--all` | 2979, 2979, 2979 | 0, 0, 0 | identical | identical | empty, identical | 260 of 2979 |

Both binaries print ten fields per row. Every full-row difference is in column
3 (elapsed: 12 rows default, 137 `--all`) or column 7 (idle: 15 rows default,
139 `--all`). Both columns are a duration since a recorded time, printed when
the binary runs. `MAIN` against the second `PRE` run differs on 23 and 248 rows
the same way, and the control, `PRE` against `PRE` again, differs on 44 and 492
rows, again only in column 3 (20 and 267 rows) and column 7 (26 and 272 rows).
No row differs in id, state, status, intent, template, liveness,
machine or workspace. `MAIN`'s liveness classes on the clone were idle 6,
needs-you-blocked 15 and needs-you-stalled 156 in the default view, and done
1666, idle 6, needs-you-blocked 283, needs-you-failed 13 and needs-you-stalled
1011 with `--all`; no row's class changed between the runs.

**Reading.** Shown (measured). On one copy of a real store, the dashboard
before the split and the dashboard on current main list the same sessions in
the same order, with the same states and liveness classes, in both views. The
only differences are the two clock columns, and the same binary run twice
differs in the same two columns, on more rows.

## Clause 2: the minimal program

> A reviewer builds a minimal program that lists the sessions under one root with their current states and one recorded context value each, using the shared reader, and sees that it builds without koto's cloud storage, TLS or HTTP dependencies and reports the same rows `koto dashboard` shows.

### Three-way diff

koto#778's run 1, with main before koto#778 as `OLD`:

```bash
for v in "" --all; do
  run "$OLD"  dashboard --once $v > c2old$v  2> c2old$v.err;  echo "exit $?"
  run "$MAIN" dashboard --once $v > c2main$v 2> c2main$v.err; echo "exit $?"
  run "$LIST" once $v             > c2list$v 2> c2list$v.err; echo "exit $?"
  for f in c2old c2main c2list; do
    wc -l < $f$v; awk -F'\t' '{print NF}' $f$v | sort -u; wc -l < $f$v.err
    cut -f1,2,4,5,6,8 $f$v > $f$v.cut
  done
  cmp c2old$v.cut c2main$v.cut && cmp c2main$v.cut c2list$v.cut && echo identical
done
```

| View | Rows (OLD, MAIN, LIST) | Fields | OLD vs MAIN | MAIN vs LIST | stderr |
|------|------|------|------|------|------|
| default | 177, 177, 177 | 10, 10, 8 | identical | identical | empty for all three |
| `--all` | 2979, 2979, 2979 | 10, 10, 8 | identical | identical | empty for all three |

### `list` on a real root

The root is `execute-skill-regression-detection`, a plan run with nine children
(ten sessions), and the key is `impl_base`, which nine of the ten hold. Both
were picked from `koto workflows` and `koto context list` on the clone.

```bash
ROOT=execute-skill-regression-detection KEY=impl_base
run "$LIST" list --root "$ROOT" --key "$KEY" > listroot 2> listroot.err; echo "exit $?"
wc -l < listroot; awk -F'\t' '{print NF}' listroot | sort -u; wc -l < listroot.err
head -1 listroot | cut -f1                 # the root's id first
awk -F'\t' '$5!=""' listroot | wc -l     # rows that carry a value
# list's fields: 1 id, 2 parent, 3 state, 4 liveness, 5 value.
# State and liveness against columns 2 and 8 of MAIN's --all row for the same id
# (c2main--all is the three-way diff's MAIN --all output):
st=0 lv=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | cut -f1)
  d=$(awk -F'\t' -v id="$id" '$1==id' c2main--all)
  [ "$(printf '%s' "$d" | cut -f2)" = "$(printf '%s' "$line" | cut -f3)" ] && st=$((st+1))
  [ "$(printf '%s' "$d" | cut -f8)" = "$(printf '%s' "$line" | cut -f4)" ] && lv=$((lv+1))
done < listroot
echo "state $st, liveness $lv"
```

Each row's value was then checked against `koto context get`, in the block
that ends clause 3.

- `list` exited 0 with empty stderr and printed 10 rows of 5 fields, the root
  first. 9 rows carry a value.
- State matches the dashboard on 10 of 10 rows, and liveness on 10 of 10.
- `koto context get` (the block at the end of clause 3): on the 9 rows with a
  value it exits 0 and prints the row's value, 9 of 9. On the row without one
  it fails and the row's value is empty. No row mismatches.

`koto context get` records the read (verified by reading `src/cli/context.rs`),
so it ran on the clone after every other run, clause 3's included.

### Dependencies and CI

In the setup shell, from the koto checkout at 90aeaaae:

```bash
cd <koto checkout at 90aeaaae>
PAT='^(rust-s3|rustls[^ ]*|attohttpc|http|url|ratatui|crossterm) '
tree() { cargo tree --offline --locked -p "$1" -e normal,build --prefix none; }
tree koto-session-list | wc -l
tree koto-session-list | grep -cE "$PAT"
tree koto | grep -oE "$PAT" | sort -u
nm -u "$LIST" | grep -cwE '_(connect|socket|getaddrinfo)'
nm -u "$MAIN" | grep -owE '_(connect|socket|getaddrinfo)' | sort -u
cd "$O"
```

The tree of `koto-session-list` has 67 lines and the pattern matches none of
them, where on `koto` it matches nine crates: attohttpc, crossterm, http,
ratatui, rust-s3, rustls, rustls-pki-types, rustls-webpki and url. `nm -u` on
`LIST` matches none of the three socket calls, where on `MAIN` it lists
`_connect`, `_getaddrinfo` and `_socket`.

In CI, validate.yml's Session List Checks job runs both checks on the release
build, then the ignored parity and hundred-read tests (verified by reading the
workflow). At current main's head, 90aeaaae, validate run 45943973811 (job
140324341367) was still queued when this record was written, so it gives no
result here. At koto#778's merge, f6c8e275, the job passed in validate run
45878295232 (job 140110880850):

```bash
gh run view 45943973811 --repo tsukumogami/koto --json status,conclusion
gh run view 45878295232 --repo tsukumogami/koto --json headSha,jobs \
  --jq '.headSha, (.jobs[] | select(.databaseId == 140110880850) | .name + " " + .conclusion)'
```

**Reading.** Shown (measured). The program built from current main reports the
dashboard's rows on a real store in both views, equal on every column that
doesn't read the clock (the two it leaves out aside), and on main before
koto#778 and current main the dashboard's rows are equal on those columns, so the program and the
dashboard agree across the step that added it. On a real root it lists each
session with the state and liveness the dashboard shows. Its build graph holds
none of the nine cloud storage, TLS, HTTP and terminal crates the same pattern
finds in `koto`'s, and its binary imports none of `connect`, `socket` and
`getaddrinfo`.

## Clause 3: a hundred reads change nothing

> The same reviewer compares the session store before and after a hundred reads by that program, on a host using koto's cloud storage, and finds it unchanged, with nothing pushed.

koto#778's runs 3a and 3b, as given there, with `$O` a scratch directory beside
the clone and `ROOT` and `KEY` as set in clause 2. Each loop cycles `once`,
`once --all` and `list --root "$ROOT" --key "$KEY"`.

```bash
SB='(version 1)(allow default)(deny file-write*)(deny network*)'
fails=0
for i in $(seq 100); do
  case $((i % 3)) in 1) a="once";; 2) a="once --all";; 0) a="list --root $ROOT --key $KEY";; esac
  sandbox-exec -p "$SB" "$LIST" $a > /dev/null 2>> "$O/sb.err" || fails=$((fails+1))
done
echo "nonzero exits $fails"; wc -lc < "$O/sb.err"
manifest() { (cd "$C/root" && find . -exec stat -f '%N|%HT|%z|%Fm' {} + | LC_ALL=C sort); }
manifest > before; wc -l < before
fails=0
for i in $(seq 100); do
  case $((i % 3)) in 1) a="once";; 2) a="once --all";; 0) a="list --root $ROOT --key $KEY";; esac
  run "$LIST" $a > /dev/null 2>> "$O/un.err" || fails=$((fails+1))
done
manifest > after
echo "nonzero exits $fails"; wc -l < "$O/un.err"; cmp before after && echo identical
```

The sandboxed runs read the live store with this session's environment, cloud
variables included, which is how the program would meet a cloud-configured
host.

| Runs | Store | Nonzero exits | stderr | Store after |
|------|-------|---------------|--------|-------------|
| 100 sandboxed | live `~/.koto` | 0 (every run exited 0) | 0 lines, 0 bytes | not compared: other sessions write the live store; the sandbox denied this program every write |
| 100 unsandboxed | the clone | 0 (every run exited 0) | 0 lines | manifest of 1,279,399 entries (path, type, size, modification time to the nanosecond) identical before and after |

The profile was checked against controls. A write to a scratch path, then the
same loopback connect without and with the profile, each time to a listener
started outside the sandbox:

```bash
sandbox-exec -p "$SB" /usr/bin/touch "$O/sb-probe"; echo "sandboxed touch: exit $?"
test -e "$O/sb-probe" && echo "probe file present" || echo "probe file absent"
P=$((20000 + RANDOM % 20000))
/usr/bin/nc -l 127.0.0.1 "$P" > /dev/null 2>&1 & L=$!; sleep 1
/usr/bin/nc -z -G 3 127.0.0.1 "$P"; echo "unsandboxed: exit $?"
kill $L; wait $L
/usr/bin/nc -l 127.0.0.1 "$P" > /dev/null 2>&1 & L=$!; sleep 1
sandbox-exec -p "$SB" /usr/bin/nc -z -G 3 127.0.0.1 "$P"; echo "sandboxed: exit $?"
kill $L; wait $L
```

The sandboxed `touch` failed and left no file. Without the profile the connect
exited 0; under it, it exited 1 with empty stderr. The control stays on loopback, so no request left the host; it shows
the profile itself refuses a connect, separately from anything on the host
that filters outbound traffic.

**How "nothing pushed" was checked.** The program shows no way to push
(inferred from what follows): its
dependency tree holds none of the nine cloud storage, TLS, HTTP and terminal
crates the pattern finds in `koto`'s, and its binary imports no `connect`, `socket` or `getaddrinfo`
(clause 2). The sandboxed runs also ran
with network access denied; a push attempt there would have failed the run or
written to stderr (inferred), and none did. The unsandboxed runs had no
`KOTO_SESSION_*` variable, so no bucket was named to them. The bucket itself
wasn't read: this walk makes no request to it, and CI's hundred-read test,
which counts FakeS3 requests (verified by reading the test), is the check that
measures zero requests directly.

**Reading.** Partly shown (measured): shown on a copy, while the
cloud-configured case is covered only by the sandboxed runs; "nothing pushed"
verified by reading and by the sandbox, not by a bucket count. A hundred runs on a copy of a real
cloud host's store left every entry unchanged in type, size and modification
time. A hundred runs on the live store, with the cloud environment set, all
exited 0 with empty stderr while writes and network were denied, so none of
them failed for lack of either (inferred from the exit codes and stderr). A denied write the program swallowed silently
would look the same. The copy's manifest covers writes only for runs without
the cloud variables and only under the store root, so a write made only under
cloud configuration, and swallowed when denied, is covered by neither run.

### Last: clause 2's values against `koto context get`

`koto context get` records the read in the clone, so this ran after every
other run on it, clause 3's manifest included. Each `list` row's value is
compared with what `koto context get` prints, with tabs, carriage returns and
newlines read as spaces:

```bash
vm=0 em=0 other=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | cut -f1); val=$(printf '%s' "$line" | cut -f5)
  got=$(run "$MAIN" context get "$id" "$KEY" 2>/dev/null; echo "rc=$?")
  rc=${got##*rc=}; flat=$(printf '%s' "${got%rc=*}" | tr '\t\r\n' '   ')
  if [ "$rc" = 0 ] && [ "$flat" = "$val" ]; then vm=$((vm+1))
  elif [ "$rc" != 0 ] && [ -z "$val" ]; then em=$((em+1))
  else other=$((other+1)); fi
done < listroot
echo "value matches $vm, absent and empty $em, mismatches $other"
```

It printed `value matches 9, absent and empty 1, mismatches 0`; clause 2
gives the reading.

## Clause 4: shirabe installs without a koto build step

> A user installs shirabe the documented way on a clean machine and it installs without a separate koto build step.

shirabe's README (tsukumogami/shirabe main, 74ce25e9) documents two installs:
the plugin, through Claude Code's marketplace, and the `shirabe` binary, through
tsuku or `install.sh`. Both were followed in a scratch home under `$TMPDIR`,
with a `PATH` holding no cargo, rustc or koto (`/usr/bin` and `/bin` hold
none, and neither does `$HOME/.local/bin`, checked by name):

```bash
S4=$(mktemp -d); mkdir -p "$S4/home" "$S4/proj"; cd "$S4/proj"
E="env -i HOME=$S4/home PATH=/usr/bin:/bin:$HOME/.local/bin SHELL=/bin/zsh TMPDIR=$TMPDIR"
ls /usr/bin /bin "$HOME/.local/bin" | grep -cxE 'cargo|rustc|rustup|koto'   # 0
$E claude plugin marketplace add "tsukumogami/shirabe" --scope user; echo "exit $?"
$E claude plugin install shirabe@shirabe --scope project; echo "exit $?"
$E ~/.tsuku/bin/tsuku install tsukumogami/shirabe; echo "exit $?"
# what the installs left in the scratch home
P=$S4/home/.claude/plugins/cache/shirabe/shirabe/0.56.4-dev
find "$P" -type f | wc -l; du -sh "$P"
find "$S4" -type d -name target | wc -l; find "$S4" -name '*.rlib' | wc -l
find "$S4" -name koto -type f
grep -rlE 'koto-session-reader|koto_session_reader' "$S4" | wc -l
X=$S4/home/.tsuku/tools/shirabe-0.56.3/bin/shirabe
file -b "$X"; $E "$X" --version
```

- `marketplace add` exited 0, cloning the marketplace into the scratch home.
  `plugin install` exited 0 and installed shirabe 0.56.4-dev: 2,898 files,
  43M, with no `target` directory and no `.rlib` anywhere in the scratch
  home. The only files named `koto` are ten checked-in test stubs under
  `skills/*/scripts/testdata/` and `skills/*/evals/fixtures/`.
- `tsuku install` exited 0 and installed shirabe 0.56.3; `file` reports an
  arm64 Mach-O executable, and `shirabe --version` prints `shirabe v0.56.3`.
- `install.sh` wasn't run. The documented form pipes `curl` into `bash`, and
  this host's network gate refuses `curl` commands, so the README's tsuku
  route stood in for it. The script, read from main, downloads a released
  binary and its checksums and builds nothing (verified by reading).
- koto itself isn't part of either install. The README lists koto 0.33.0 or
  later as a separate requirement of the koto-backed skills, installed through
  tsuku as a released binary (verified by reading the README); koto wasn't
  installed in the scratch home, so that install didn't run here.

**Does shirabe take koto-session-reader?** No. Nothing in the installed plugin
tree names `koto-session-reader` or `koto_session_reader` (0 files), and the
`shirabe` crate's direct dependencies (verified by reading
`crates/shirabe/Cargo.toml` at shirabe's 74ce25e9: clap,
saphyr, shirabe-validate, serde_json, sha2, regex, rustix, getrandom) don't
include it. The roadmap leaves this open: "how shirabe takes the dependency and
how the two release schedules are tied". The design puts it out of scope too
("Any change in shirabe, its install or its release"), and no plan step makes
shirabe take the reader.

**Reading.** Not shown; the parts that were checked were measured or verified
by reading. shirabe installs
the documented way with no koto build step today, but only because it doesn't
depend on koto's reader at all. The clause asks what happens once a second
program in shirabe reads through the reader, and nothing shipped puts that
dependency in shirabe. Under the design's chosen form, a git dependency pinned
to a koto release tag, the dependency is compiled into the released `shirabe`
binary, so an install that downloads that binary would still need no koto build
step; that is inferred, not run.

## Summary

| Clause | What the runs show |
|--------|--------------------|
| 1. Dashboard before and after | Shown: same sessions, states and liveness classes in both views; only the two clock columns differ |
| 2. The minimal program | Shown: rows equal the dashboard's in both views, `list` agrees with the dashboard and with `koto context get`, none of the nine crates `koto`'s tree matches and no `connect`, `socket` or `getaddrinfo` import |
| 3. A hundred reads change nothing | Partly shown: a hundred runs left a copy of the store's manifest identical, and a hundred sandboxed runs on the live store exited 0 with writes and network denied; "nothing pushed" rests on reading and the sandbox, not on a bucket count |
| 4. shirabe installs without a koto build step | Not shown: the install needs no koto build step, but shirabe doesn't take koto-session-reader yet |
