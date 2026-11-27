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
The Clause 3 rerun section at the end of this record repeats the hundred reads on the live, cloud-configured store and counts the bucket.

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

## Clause 3 rerun: a hundred reads on the live cloud store

> The same reviewer compares the session store before and after a hundred reads by that program, on a host using koto's cloud storage, and finds it unchanged, with nothing pushed.

The walk above ran its hundred unsandboxed reads on a copy without the cloud
variables, and its live-store runs under a sandbox, so "nothing pushed" wasn't
counted. This rerun reads the live `~/.koto` in this session's environment,
cloud variables included, with writes and network allowed, and lists the
bucket's objects before and after. It ran on 2026-11-27 between 13:47Z and
14:03Z on the same macOS arm64 host. The environment sets six `KOTO_SESSION_*`
variables, the backend and five `KOTO_SESSION_CLOUD_*` ones, checked by name
only.

### Binary

There was one release build of `koto-session-list` from koto main at 3112838c,
in a fresh tree exported with `git archive` into a `mktemp -d` scratch directory
`S`. It exited 0 in 18.5 s:

```bash
mkdir -p $S/build; git archive 3112838c | tar -x -C $S/build
cd $S/build && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$PWD/target cargo build --release --locked -p koto-session-list
```

### Picking a root nothing else writes

Other live sessions write this host's bucket and `~/.koto` all the time, so a
diff of the whole store or the whole bucket would differ for reasons that
aren't the program's. Both comparisons are scoped to one finished tree instead.
Each session's id and parent come from the first line of its state file (the
header's `workflow` and `parent_workflow` fields), and `_terminal_index.jsonl`
names the terminal ones. A tree is grouped by following each parent up to its
top:

```bash
cd ~/.koto/sessions
for f in */*.state.jsonl; do head -1 "$f"; done 2>/dev/null |
  jq -r 'select(.workflow) | [.workflow, (.parent_workflow // "")] | @tsv' > $S/headers.tsv
jq -r '.session_id' ~/.koto/_terminal_index.jsonl | sort -u > $S/terminal.ids
python3 -I - "$S" <<'PY'
import sys,collections
S=sys.argv[1]
t=set(open(S+'/terminal.ids').read().split())
par={}
for l in open(S+'/headers.tsv'):
    p=l.rstrip('\n').split('\t'); par[p[0]]=p[1]
def top(x):
    seen=set()
    while par.get(x) and par[x] in par and x not in seen: seen.add(x); x=par[x]
    return x
g=collections.defaultdict(list)
for s in par: g[top(s)].append(s)
out=[(len(m),r) for r,m in g.items() if len(m)>=4 and all(x in t for x in m)]
for n,r in sorted(out,reverse=True)[:15]: print(n,r)
print("trees>=4:", sum(1 for m in g.values() if len(m)>=4), "all-terminal:", len(out))
PY
```

Of 9,489 readable headers, 46 trees have four or more sessions, and 32 of them
are terminal throughout. The root is `execute-mirror-repair`, a plan run with
nine children (ten sessions), and the key is `impl_base`. Clause 2's root wasn't
reused. It was cancelled at `plan_completion`, so it lists as needs-you-stalled,
not done. Quiet time came from each session's last event and its newest file,
in bash from `~/.koto/sessions`:

```bash
ids=$(awk -F'\t' -v r=execute-mirror-repair '$1==r || index($1, r".")==1' $S/headers.tsv | cut -f1)
for d in $ids; do tail -1 $d/*.state.jsonl | jq -r .timestamp; done | sort | tail -1
for d in $ids; do find $d -type f -exec stat -f '%m' {} +; done | sort -n | tail -1
echo "$ids" > $S/root.ids
```

Across the ten, the newest event is from 2026-11-19T18:25Z and the newest file
from 2026-11-20T12:25Z, a week before the runs. `list` ran once on a scratch
copy of the ten directories (`cp -cR` into `$S/copy/sessions`), so the live
store saw no run before the hundred. It printed ten rows, all with liveness
`done`, and nine carry a value for the key:

```bash
env -i PATH=/usr/bin:/bin HOME=$S/copy/home KOTO_SESSIONS_BASE=$S/copy/sessions \
  "$S/build/target/release/koto-session-list" list --root execute-mirror-repair --key impl_base
```

### The two listings

Where a session's bytes live in the bucket was verified by reading
`src/session/cloud.rs` and `src/session/name_index.rs`. Every key of a
session is under its home, the 16-hex-digit workspace prefix its local header
records as `origin.store.prefix`, followed by the session's id. The name index
holds one key per prefix that has the session, under `koto-names/` and the
id. Locally, a session is its own directory under `~/.koto/sessions`.

The bucket listing is read-only. It uses the AWS CLI already on the host
(`aws-cli/2.31.18`), and calls `list-objects-v2` and nothing else. The
credentials, endpoint, region and bucket are passed from the
`KOTO_SESSION_CLOUD_*` variables by name under `env -i`, with a scratch `HOME`
and no config or credentials file. No value was printed or written anywhere.
The listing's stderr went to a scratch file, which stayed empty throughout.

```bash
awsls() {
  env -i PATH=/usr/bin:/bin:/opt/homebrew/bin HOME="$S/awshome" \
    AWS_CONFIG_FILE=/dev/null AWS_SHARED_CREDENTIALS_FILE=/dev/null AWS_PAGER= \
    AWS_ACCESS_KEY_ID="$KOTO_SESSION_CLOUD_ACCESS_KEY" \
    AWS_SECRET_ACCESS_KEY="$KOTO_SESSION_CLOUD_SECRET_KEY" \
    AWS_DEFAULT_REGION="$KOTO_SESSION_CLOUD_REGION" \
    aws --endpoint-url "$KOTO_SESSION_CLOUD_ENDPOINT" s3api list-objects-v2 \
      --bucket "$KOTO_SESSION_CLOUD_BUCKET" --prefix "$1" \
      --query 'Contents[].[Key,Size,ETag,LastModified]' --output text \
      2>> "$S/aws.err" | grep -v '^None$' | LC_ALL=C sort
  return "${PIPESTATUS[0]}"
}
# every object under koto-names/<id>/, <home>/<id>/, and <p>/<id>/ for each
# prefix <p> the id's koto-names entries name: key, size, ETag, last modified
bucketscope() {
  local out=$1 id home p bad=0 n=0
  : > "$out"
  : > "$S/scope.prefixes"
  while IFS= read -r id; do
    home=$(head -1 ~/.koto/sessions/"$id"/*.state.jsonl | jq -r '.origin.store.prefix // empty')
    awsls "koto-names/$id/" > "$S/names.tmp" || bad=$((bad+1))
    cat "$S/names.tmp" >> "$out"
    { [ -n "$home" ] && echo "$home"; cut -f1 "$S/names.tmp" | awk -F/ '{print $3}'; } | LC_ALL=C sort -u |
      while IFS= read -r p; do echo "$p/$id/"; done >> "$S/scope.prefixes"
    echo "koto-names/$id/" >> "$S/scope.prefixes"
  done < "$S/root.ids"
  while IFS= read -r p; do
    case $p in koto-names/*) continue;; esac
    awsls "$p" >> "$out" || bad=$((bad+1))
  done < "$S/scope.prefixes"
  rm -f "$S/names.tmp"
  LC_ALL=C sort -o "$out" "$out"
  echo "prefixes $(wc -l < "$S/scope.prefixes" | tr -d ' '), failed listings $bad, objects $(wc -l < "$out" | tr -d ' ')"
}
# path, type, size and modification time (ns) of every entry under
# ~/.koto/sessions/<id>/ for each id
localscope() {
  local out=$1
  (cd ~/.koto/sessions && while IFS= read -r id; do find "$id" -exec stat -f '%N|%HT|%z|%Fm' {} +; done < "$S/root.ids") | LC_ALL=C sort > "$out"
  echo "entries $(wc -l < "$out" | tr -d ' ')"
}
```

The scope covers 20 prefixes: each session's name-index prefix and its home.
No session's name index named any prefix but its home. A first listing at
13:51:21Z and a control 26 s later were identical on both sides. The newest
object's last-modified time was 2026-11-19T18:25:47Z.

### The hundred runs

The loop ran in bash, in this session's environment, with no sandbox. Each run
starts in the background, and `lsof` polls its pid for internet sockets until
it exits. Every tenth run, both scopes are listed again. The loop stops at once
on an internet socket or on any listing difference:

```bash
LIST=$S/build/target/release/koto-session-list
ROOT=execute-mirror-repair KEY=impl_base
O=$S/out; : > "$O/live.err"; : > "$O/lsof.out"
fails=0 samples=0 hits=0 stop=""
for i in $(seq 100); do
  case $((i % 3)) in 1) a="once";; 2) a="once --all";; 0) a="list --root $ROOT --key $KEY";; esac
  "$LIST" $a > /dev/null 2>> "$O/live.err" & pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    samples=$((samples+1))
    lsof -nP -a -p "$pid" -i >> "$O/lsof.out" 2>/dev/null && hits=$((hits+1))
  done
  wait "$pid" || fails=$((fails+1))
  [ "$hits" -gt 0 ] && { stop="network at run $i"; break; }
  if [ $((i % 10)) -eq 0 ]; then
    localscope "$O/local.$i" > /dev/null; bucketscope "$O/bucket.$i" > /dev/null
    cmp -s "$S/local.before" "$O/local.$i" || { stop="local change by run $i"; break; }
    cmp -s "$S/bucket.before" "$O/bucket.$i" || { stop="bucket change by run $i"; break; }
  fi
done
echo "runs $i, nonzero exits $fails, stderr lines $(wc -l < "$O/live.err"), lsof samples $samples, samples with an inet file $hits, stop: ${stop:-none}"
```

The loop was saved as `$S/loop.sh` and driven like this, with fresh before
listings taken right before it:

```bash
bucketscope $S/bucket.before; localscope $S/local.before
source $S/loop.sh
localscope $S/local.after; bucketscope $S/bucket.after
cmp $S/local.before $S/local.after && echo "local identical"
cmp $S/bucket.before $S/bucket.after && echo "bucket identical"
wc -l < $S/aws.err; wc -c < $O/lsof.out
```

| Measure | Result |
|---------|--------|
| Runs | 100 (34 `once`, 33 `once --all`, 33 `list`), 13:55:06Z to 14:02:19Z |
| Nonzero exits | 0 |
| stderr | 0 lines |
| Bucket scope, before (13:54:57Z) and after (14:02:28Z) | 278 objects over 20 prefixes, 0 failed listings; identical by `cmp` (key, size, ETag, last modified) |
| Local scope, before and after | 614 entries (45 directories, 569 files); identical by `cmp` (path, type, size, modification time to the nanosecond) |
| The ten checks, one every tenth run | each identical to the before listings on both sides; the loop never stopped |
| `lsof` samples of the program's pid | 15,348, none showing an internet socket; `lsof.out` 0 bytes |

### What the scope sees and what it doesn't

The scope covers every key koto itself writes for these ten sessions. A push of
a session goes under its home prefix, which the local header names, and an
index entry goes under its name-index prefix (verified by reading the two files
above). Locally, a session's state, context and cloud bookkeeping all sit in
its own directory. Every run reads these sessions: `once --all` and `list`
print them, and `once` scans the store and leaves them out as receded
(verified by reading `koto-session-list/src/main.rs`). A write or push by any
run to the sessions it read would change a key, size, ETag or modification
time in the scope, and none did (measured). The sessions had been quiet for a
week, so a change in the scope during those seven minutes would have pointed at
the program, not another session (inferred from that quiet time, and from the
before, control and ten interim listings agreeing).

The scope doesn't see:

- the store's other sessions, which the `once` runs also read. A write to one
  of them would be lost among the live sessions' own writes.
- `~/.koto` outside these ten directories (`config.toml`, the top-level
  indexes and journals), which other sessions write.
- the bucket's other workspace prefixes (5,545 in all at 13:51Z) and other
  name-index entries. An object put under a prefix no index names for these
  sessions wouldn't show.
- a request that reads the bucket without writing to it, which no object
  listing records.

The network observation covers part of that. `lsof -i` sees the internet
sockets the program's own process holds when a sample is taken. It can miss a
socket opened and closed between two samples, or one held by another process,
and it doesn't list Unix sockets. The sampler wasn't tested against a process
holding a known socket, so its sensitivity is inferred, not measured. Clause
2's checks still stand beside it: the program's dependency tree has no HTTP,
TLS or cloud storage crate, and its binary imports no `connect`, `socket` or
`getaddrinfo`.

**Reading.** Shown on the scope (measured); nothing pushed outside it is
inferred. A hundred unsandboxed reads by `koto-session-list` from current main
ran on the live store of a host using koto's cloud storage, with the cloud
variables set and writes and network allowed. Every run exited 0 with empty
stderr. The runs left the ten sessions' 278 bucket objects and 614 local
entries unchanged, by count and by content: the same keys, sizes, ETags and
modification times. None of the 15,348 samples of the program's process showed
an internet socket. That nothing was pushed anywhere else in the bucket is
inferred from the sampling, within the limits above, and from clause 2's
dependency and symbol checks. The whole bucket wasn't diffed.
