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
  is 0.41.0 on the cloud session backend. No koto release carries Steps 5 or
  6, so every binary below was built from source with
  `CARGO_INCREMENTAL=0 cargo build --release --locked`, one build at a time,
  and each run names its binary by commit:

  | Name | Commit | `koto version` | Role |
  |------|--------|----------------|------|
  | `PRE` | 22bc3a7e (v0.39.0) | `koto 0.39.0 (22bc3a7e ...)` | dashboard before the split |
  | `OLD` | 865e5ce4 | `koto 0.41.0-dev+865e5ce4 (865e5ce4 ...)` | main before koto#778 |
  | `MAIN` | 90aeaaae | `koto 0.41.0-dev+90aeaaae (90aeaaae ...)` | current main |
  | `LIST` | 90aeaaae | (no version command) | `koto-session-list` from current main |

  v0.39.0 is the newest tag that doesn't contain the split's first commit,
  cc9479be (Step 2, koto#742), and its `koto dashboard` already has `--once`
  and `--all`, so no other pre-split commit was needed.

- The store is one APFS clone of the host's `~/.koto`, made with
  `cp -cR ~/.koto "$C/root"` into a fresh `mktemp -d` directory: 1,279,399
  entries and 9,535 session directories, 4.2G apparent size, and 0.27 GiB of
  actual disk. The host's `config.toml` names no session backend, so a run with
  no `KOTO_SESSION_*` variable reads the clone as a local store. Every run on
  the clone goes through koto#778's runner, which starts the binary with an
  empty environment:

  ```bash
  run() { env -i PATH=/usr/bin:/bin HOME="$C/home" KOTO_HOME="$C/root" XDG_CACHE_HOME="$C/home/cache" "$@"; }
  ```

  The only runs against the live store are clause 3's hundred sandboxed runs.
  The clone was deleted when the runs ended.

## Clause 1: the dashboard before and after

> A reviewer runs `koto dashboard` against the same session store before and after the change and sees the same sessions, states and liveness classes.

Each view ran three times back to back on the clone, `PRE`, then `MAIN`, then
`PRE` again, so a difference that comes from the clock shows up between the two
`PRE` runs as well:

```bash
for v in "" --all; do
  run "$PRE"  dashboard --once $v > pre1$v 2> pre1$v.err
  run "$MAIN" dashboard --once $v > main$v 2> main$v.err
  run "$PRE"  dashboard --once $v > pre2$v 2> pre2$v.err
done
# each exit code was recorded; then, for the three outputs of a view:
# cmp on cut -f1,2,4,5,6,8 (the columns that don't read the clock),
# cmp on cut -f9,10, cmp on the .err files, and diff on the full rows
```

| View | Rows (PRE, MAIN, PRE again) | Exit | Columns 1, 2, 4, 5, 6, 8 | Columns 9, 10 | stderr | Full rows that differ |
|------|------|------|------|------|------|------|
| default | 177, 177, 177 | 0, 0, 0 | identical | identical | empty, identical | 26 of 177 |
| `--all` | 2979, 2979, 2979 | 0, 0, 0 | identical | identical | empty, identical | 260 of 2979 |

Both binaries print ten fields per row. Every full-row difference is in column
3 (elapsed: 12 rows default, 137 `--all`) or column 7 (idle: 15 rows default,
139 `--all`). Both columns are a duration since a recorded time, printed when
the binary runs; `MAIN` against the second `PRE` run differs on 23 and 248 rows
the same way. No row differs in id, state, status, intent, template, liveness,
machine or workspace. `MAIN`'s liveness classes on the clone were idle 6,
needs-you-blocked 15 and needs-you-stalled 156 in the default view, and done
1666, idle 6, needs-you-blocked 283, needs-you-failed 13 and needs-you-stalled
1011 with `--all`; no row's class changed between the runs, though they span
about 14 seconds.

**Reading.** Shown (measured). On one copy of a real store, the dashboard
before the split and the dashboard on current main list the same sessions in
the same order, with the same states and liveness classes, in both views. The
only differences are the two clock columns, and the same binary run twice
differs in the same way.

## Clause 2: the minimal program

> A reviewer builds a minimal program that lists the sessions under one root with their current states and one recorded context value each, using the shared reader, and sees that it builds without koto's cloud storage, TLS or HTTP dependencies and reports the same rows `koto dashboard` shows.

### Three-way diff

koto#778's run 1, with main before koto#778 as `OLD`:

```bash
for v in "" --all; do
  run "$OLD"  dashboard --once $v > old$v  2> old$v.err;  echo "exit $?"
  run "$MAIN" dashboard --once $v > main$v 2> main$v.err; echo "exit $?"
  run "$LIST" once $v             > list$v 2> list$v.err; echo "exit $?"
  for f in old main list; do cut -f1,2,4,5,6,8 $f$v > $f$v.cut; done
  cmp old$v.cut main$v.cut && cmp main$v.cut list$v.cut && echo identical
  wc -l old$v.err main$v.err list$v.err
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
run "$LIST" list --root "$ROOT" --key "$KEY" > listroot
# list's fields are id, parent, state, liveness, value; fields 3 and 4 of each
# row against columns 2 and 8 of MAIN's --once --all row for the same id;
# then, last, each row's value against: run "$MAIN" context get <id> "$KEY",
# with tabs, carriage returns and newlines read as spaces
```

- `list` exited 0 with empty stderr and printed 10 rows of 5 fields, the root
  first. 9 rows carry a value.
- State matches the dashboard on 10 of 10 rows, and liveness on 10 of 10.
- `koto context get`: on the 9 rows with a value it exits 0 and prints the
  row's value, 9 of 9. On the row without one it fails and the row's value
  is empty. No row mismatches.

`koto context get` records the read, so it ran on the clone after every other
run, clause 3's included.

### Dependencies and CI

```bash
cargo tree --locked -p koto-session-list -e normal,build --prefix none \
  | grep -cE '^(rust-s3|rustls[^ ]*|attohttpc|http|url|ratatui|crossterm) '
nm -u "$LIST" | grep -cwE '_(connect|socket|getaddrinfo)'
```

The first prints 0 (the tree has 51 lines), where the same pattern on `-p koto`
names attohttpc, crossterm, http, ratatui, rust-s3, rustls, rustls-pki-types,
rustls-webpki and url. The second prints 0, where `nm -u "$MAIN"` lists
`_connect`, `_getaddrinfo` and `_socket`.

In CI, validate.yml's Session List Checks job runs both checks on the release
build, then the ignored parity and hundred-read tests. At current main's head,
90aeaaae, validate run 45943973811 (job 140324341367) hadn't started when this
record was written, so it gives no result here. At koto#778's merge, f6c8e275,
the job passed in validate run 45878295232 (job 140110880850).

**Reading.** Shown (measured). The program built from current main reports the
dashboard's rows on a real store in both views, and on main before koto#778 and
current main the dashboard's rows are identical, so the program and the
dashboard agree across the step that added it. On a real root it lists each
session with the state and liveness the dashboard shows. Its build graph holds
none of koto's cloud storage, TLS, HTTP or terminal crates, and its binary
imports no socket call.

## Clause 3: a hundred reads change nothing

> The same reviewer compares the session store before and after a hundred reads by that program, on a host using koto's cloud storage, and finds it unchanged, with nothing pushed.

koto#778's runs 3a and 3b, as given there, with `$O` a scratch directory beside
the clone. Each loop cycles `once`, `once --all` and `list --root "$ROOT" --key
"$KEY"`.

```bash
SB='(version 1)(allow default)(deny file-write*)(deny network*)'
fails=0
for i in $(seq 100); do
  case $((i % 3)) in 1) a="once";; 2) a="once --all";; 0) a="list --root $ROOT --key $KEY";; esac
  sandbox-exec -p "$SB" "$LIST" $a > /dev/null 2>> "$O/sb.err" || fails=$((fails+1))
done
echo "nonzero exits $fails"; wc -lc < "$O/sb.err"
manifest() { (cd "$C/root" && find . -exec stat -f '%N|%HT|%z|%Fm' {} + | LC_ALL=C sort); }
manifest > before
fails=0
for i in $(seq 100); do
  case $((i % 3)) in 1) a="once";; 2) a="once --all";; 0) a="list --root $ROOT --key $KEY";; esac
  run "$LIST" $a > /dev/null 2>> "$O/un.err" || fails=$((fails+1))
done
manifest > after
echo "nonzero exits $fails"; wc -l < "$O/un.err"; cmp before after
```

The sandboxed runs read the live store with this session's environment, cloud
variables included, which is how the program would meet a cloud-configured
host.

| Runs | Store | Nonzero exits | stderr | Store after |
|------|-------|---------------|--------|-------------|
| 100 sandboxed | live `~/.koto` | 0 (every run exited 0) | 0 lines, 0 bytes | not compared: other sessions write the live store; the sandbox denied this program every write |
| 100 unsandboxed | the clone | 0 (every run exited 0) | 0 lines | manifest of 1,279,399 entries (path, type, size, modification time to the nanosecond) identical before and after |

The profile was checked against controls. Under it, `touch` on a scratch path
was denied. For network, a listener was started on 127.0.0.1 outside the
sandbox, and the same `nc -z 127.0.0.1 <port>` connect was run twice: without
the sandbox it exited 0, and under the profile it exited 1. The control stays
on loopback, so no request left the host; it shows the profile itself refuses
a connect, separately from anything on the host that filters outbound traffic.

**How "nothing pushed" was checked.** The program has no way to push: its
dependency tree holds no storage, TLS or HTTP crate and its binary imports no
`connect`, `socket` or `getaddrinfo` (clause 2). The sandboxed runs also ran
with network access denied; a push attempt there would have failed the run or
written to stderr (inferred), and none did. The unsandboxed runs had no `KOTO_SESSION_*` variable,
so no bucket was named to them. The bucket itself wasn't read: this walk
makes no request to it, and CI's hundred-read test, which counts FakeS3
requests, is the check that measures zero requests directly.

**Reading.** Shown on a copy (measured); "nothing pushed" verified by reading
and by the sandbox, not by a bucket count. A hundred runs on a copy of a real
cloud host's store left every entry unchanged. A hundred runs on the live store,
with the cloud environment set, all exited 0 with empty stderr while writes and
network were denied, so none of them failed for lack of either. A denied write
the program swallowed silently would look the same; the copy's unchanged
manifest is the check that covers writes.

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
$E claude plugin marketplace add "tsukumogami/shirabe" --scope user
$E claude plugin install shirabe@shirabe --scope project
$E ~/.tsuku/bin/tsuku install tsukumogami/shirabe
```

- `marketplace add` exited 0, cloning the marketplace into the scratch home.
  `plugin install` exited 0 and installed shirabe 0.56.4-dev: 2,898 files,
  43M, with no `target` directory and no `.rlib` anywhere in the scratch
  home. The only files named `koto` are checked-in test stubs under
  `skills/*/scripts/testdata/` and `skills/*/evals/fixtures/bin/`.
- `tsuku install` exited 0 and installed shirabe 0.56.3 as a downloaded
  arm64 Mach-O binary; `shirabe --version` prints `shirabe v0.56.3`. Its
  log has no line naming cargo, rustc, a compile or koto.
- `install.sh` wasn't run. The documented form pipes `curl` into `bash`, and
  this host's network gate refuses `curl` commands, so the README's tsuku
  route stood in for it. The script, read from main, downloads a released
  binary and its checksums and builds nothing (verified by reading).
- koto itself isn't part of either install. The README lists koto 0.33.0 or
  later as a requirement of the koto-backed skills, which `tsuku install`
  provides as a released binary (verified by reading); it wasn't installed in
  the scratch home.

**Does shirabe take koto-session-reader?** No. Nothing in the installed plugin
tree names `koto-session-reader` or `koto_session_reader` (0 files), and the
`shirabe` crate's dependencies (`crates/shirabe/Cargo.toml` on main: clap,
saphyr, shirabe-validate, serde_json, sha2, regex, rustix, getrandom) don't
include it. The roadmap leaves this open: "how shirabe takes the dependency and
how the two release schedules are tied". The design puts it out of scope too
("Any change in shirabe, its install or its release"), and no plan step makes
shirabe take the reader.

**Reading.** Not shown (measured and verified by reading). shirabe installs
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
| 2. The minimal program | Shown: rows equal the dashboard's in both views, `list` agrees with the dashboard and with `koto context get`, no cloud, TLS or HTTP crate and no socket import |
| 3. A hundred reads change nothing | Shown on a copy: a hundred runs left a copy of the store identical, and a hundred sandboxed runs on the live store exited 0 with writes and network denied; "nothing pushed" rests on reading and the sandbox, not on a bucket count |
| 4. shirabe installs without a koto build step | Not shown: the install needs no koto build step, but shirabe doesn't take koto-session-reader yet |
