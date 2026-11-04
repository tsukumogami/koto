---
schema: plan/v2
status: Active
execution_mode: multi-pr
split_mode_source: flag
split_rationale: |
  Hard Constraint, with Incremental Value. This scoping PR merges before any
  of the build, and the build waits on two answers it can't start without:
  how the session-continuity work settles the bucket layout around
  summary.json, and step 1's delimiter probe against R2, which picks the
  layout steps 3 to 7 build on. Each step then lands in its own pull request,
  and each lands a usable increment: step 1 alone gives the text evidence
  scenario, step 2 the interactive one, and each later step one observable
  behaviour on top.
tracking_level: none
upstream: docs/designs/DESIGN-cross-machine-dashboard.md
milestone: "Sessions from every machine in one dashboard"
step_count: 7
---

# PLAN: Sessions from every machine in one dashboard

## Status

Active

## Scope Summary

A developer reads the sessions they run on several machines from any one of
them. Each session records its producing machine on its first event, every
push publishes a small `summary.json` beside the state file, and on another
machine `koto dashboard` and `koto dashboard --once` list every workspace
prefix in the bucket and show remote sessions with their current state, the
machine that produced them and their workspace. The build delivers the
PRD's two evidence scenarios: on machine B, the interactive dashboard and
its text output each show a session machine A ran to a gate, with its
current state and A named as its origin.

## Decomposition Strategy

Walking skeleton. Step 1 is the thinnest path through the whole design: a
session machine A runs to a gate on the fixture template records A's
machine name, A's tick publishes the summary, and machine B's `--once`
finds A's prefix with a `/ctx/`-delimited listing and prints the row with
its `machine` and `workspace` columns, against the in-process fake bucket.
It also carries the delimiter probe against R2 that confirms the layout
before anything else builds on it. Step 2 brings the same rows into the
TUI. Each later step adds one behaviour on top: correct handling of
removed, imported, older-release and same-named sessions; the machine-name
settings and their survival across rewrites; failure handling; freshness
and cost bounds; and the rules that keep another machine's objects from
breaking or misleading the dashboard. Tests and documentation travel with
the step whose behaviour they cover.

## Steps

### Step 1: koto dashboard --once on machine B prints machine A's blocked session with its state, machine and workspace

**Goal**: A session created on machine A records machine-a on its workflow_initialized event, A's tick publishes a summary.json beside the session's state file, and machine B's `koto dashboard --once` lists A's workspace prefix with a `/ctx/`-delimited listing and prints the session's row with its machine and workspace columns. It runs on the fixture template (two states, the first gated until context key `go` exists) against the in-process fake bucket, and crosses the machine stamp, the summary writer, the remote reader and the `--once` columns end to end, with the TUI, pacing, failure classes, legacy and migrated handling and name validation left out or hard-coded. It also carries the delimiter probe in the cloud integration suite and the `--once` column documentation. This step advances the PRD's text evidence scenario.

**Acceptance Criteria**:

- [ ] After machine A runs `koto init` on the fixture template with `KOTO_MACHINE_NAME=machine-a` and ticks once, the session stops at the gate and the fake bucket holds a `summary.json` in the session's own directory that reports the gate's state, `is_blocked` true and machine `machine-a`.
- [ ] `koto dashboard --once` on machine B (same bucket, a different directory and session store) prints a line for A's session whose `current_state`, `status`, `intent` and `template_name` columns equal the ones A's own `--once` prints for it.
- [ ] That line on B has exactly ten tab-separated fields, with `machine-a` in the ninth (`machine`) and A's 16-hex workspace prefix in the tenth (`workspace`); the same session on A's `--once` shows `machine-a` in its ninth field.
- [ ] B's run makes only listing and GET requests against the fake bucket (no PUT, DELETE or POST), creates no directory in B's session store, and fetches no object whose key is a state file.
- [ ] A `/ctx/`-delimited listing of a session-shaped directory against Cloudflare R2 returns the session's top-level objects (state file, `summary.json`, `migrated.json`) and exactly one common prefix for its `ctx/` tree, and reports a clear failure naming the endpoint when it does not.
- [ ] A session B created itself, whose copy is also in the bucket, appears exactly once in B's `--once` output, as B's own row with B's machine.
- [ ] On B, `--needs-you` keeps A's blocked session and `--status running` drops it; `koto dashboard --once <name>` with A's session name prints its line; and the eight existing columns keep their positions and meanings.
- [ ] With the local backend selected, even with a bucket configured, every `--once` line over the existing `--once` test fixtures equals the line those tests expect today followed by two empty tab-separated fields, and the run makes no network request.
- [ ] `docs/reference/session-feed.md` describes the `machine` and `workspace` columns as columns nine and ten, empty when no machine is recorded and empty for `workspace` on the local backend.

**Check**: Run `cargo test --test dashboard_cross_machine_test once_prints_other_machines_blocked_session` and see it pass. The test runs two koto processes against one in-process fake bucket (as `tests/cloud_bound_dir_test.rs` does, using `tests/support/fake_s3.rs`): machine A inits and ticks the fixture template under `KOTO_MACHINE_NAME=machine-a`, machine B runs `koto dashboard --once`, and the assertions are the line's ten fields, its `current_state`, `status`, `intent` and `template_name` equal to A's own `--once` output, `machine-a` in the ninth, A's prefix in the tenth, and a request log with no write and no state-file GET. Then run `cargo test --features cloud-integration-tests --test cloud_integration_test ctx_delimiter -- --test-threads=1` in CI's existing R2 job and see the delimiter probe pass; without the R2 environment variables that test skips, as the other tests in that file do.

**Dependencies**: None

**Files**: `src/engine/types.rs`, `src/session/version.rs`, `src/session/row.rs`, `src/session/summary.rs`, `src/session/remote_feed.rs`, `src/session/cloud.rs`, `src/cli/dashboard_data.rs`, `tests/dashboard_cross_machine_test.rs`, `tests/cloud_integration_test.rs`, `docs/reference/session-feed.md`

### Step 2: koto dashboard on machine B shows machine A's blocked session in the needs-you band marked @machine-a

**Goal**: The interactive dashboard on machine B now shows the sessions other machines pushed to the bucket. A background feed thread owns every bucket request and hands complete snapshots of remote rows to the UI thread, so key presses and redraws never wait on the bucket. Rows are keyed by session and location, so a remote session never merges with or nests under a same-named local one, and a session with a local copy appears once, as a local row. Remote rows carry an `@<machine>` marker (`@unknown` when no machine is recorded), the Session tab shows Machine and the workspace id for a remote row, and shows a Machine row for a local one. This advances the PRD's interactive evidence scenario.

**Acceptance Criteria**:

- [ ] With machine A's session stopped at the fixture template's gate and pushed to the in-process fake bucket, the TUI on machine B (a separate `KOTO_HOME`, same bucket) lists that session in the needs-you band as blocked in the gate's state, and the row's name cell reads `@machine-a` after the name.
- [ ] Local rows carry no marker. A remote row whose machine isn't recorded reads `@unknown`. At 80 columns, a remote row with a 40-character machine name still shows `@` and the first 8 characters of the name.
- [ ] A session named `review` on A and another named `review` on B appear as two separate rows. The remote one is marked and never replaces the local one, and a child of A's `review` nests under A's row, not B's. A session B created itself appears exactly once, as an unmarked local row, even though its copy is also in the bucket.
- [ ] Selecting a remote row shows Machine (`machine-a`) and the workspace id in its Session tab, and each other tab shows one line saying the session's detail lives on its own machine. Selecting a local row shows a Machine row holding the recorded name, or `unknown` for a session from before the machine stamp.
- [ ] With a fake-bucket endpoint that accepts connections and never answers, the frame drawn after a key press reflects it within 200 ms. The local rows stay visible meanwhile, and no bucket request is made from the input and render thread.

**Check**: Run `cargo test dashboard` (the render tests in `src/cli/dashboard_render.rs` and the dashboard integration tests) and expect the new cases to pass. They draw the TUI into a ratatui `TestBackend` buffer on machine B's `KOTO_HOME`, with machine A's blocked session on the in-process fake bucket (the Step 1 setup), and check the buffer for the needs-you band row ending in `@machine-a`, the two `review` rows, the single local copy, and the Session tab text. A further test points B at a never-answering endpoint and asserts the frame after a key press arrives within 200 ms with local rows intact.

**Dependencies**: Step 1

**Files**: `src/cli/dashboard.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard_state.rs`, `src/cli/dashboard_render.rs`, `src/session/remote_feed.rs`

### Step 3: removed, imported, older-release and same-named sessions show correctly on machine B

**Goal**: Machine B's dashboard now decides every session directory a `/ctx/`-delimited listing shows by the design's classifier, in order: no state object drops the row (a removed session), an internal name drops it, a session B holds a copy of drops it (matched by `session_id`, or for a row with no summary by its home prefix and name), a `migrated.json` in the session's own directory whose named copy is in the same scan, or a local copy's `session_imported` event naming it, hides it and counts it, and a state object with no summary, a summary that's oversized, doesn't parse, has another schema or names another key, or a summary whose `state_etag` no longer matches the listed state file once that file is more than 60 seconds old, lists it with status `unknown` and counts it in the unreadable tally. Step 2 skipped a session with no summary or an unreadable one; this step lists both as `unknown` rows. The listing now keeps each top-level object's ETag and LastModified beside its size, and an `unknown` row's idle time comes from the state file's LastModified. Two machines' same-named sessions stay two rows in `--once` as in the TUI. A failed listing is never read as a removal: the TUI keeps the remote rows it last had for a workspace whose listing failed, or for every workspace when the top-level listing failed, and shows the snapshot's failure on one status line; step 5 classes that failure and bounds its timing. This keeps both evidence scenarios true when sessions are removed, imported, written by an older release, or share a name. Every row fact comes from `summary.json` and the listing's own facts about the state file; the reader fetches nothing under `ctx/`, adds no owner field, and doesn't change project config loading or how koto resolves the bucket and credentials.

**Acceptance Criteria**:

- [ ] After `koto session cleanup` of a session on machine A, the next `koto dashboard --once --all` on machine B doesn't print it, and B's running TUI drops its row on the feed's next snapshot. A session directory that holds a `summary.json` but no state file prints no row.
- [ ] With B's TUI showing A's session and the listing of A's prefix then failing, the next snapshot leaves A's row in place and the TUI shows one status line carrying the snapshot's failure; with the top-level listing failing, every remote row stays. Once the listing answers again and A's session has been cleaned up, the row drops.
- [ ] After `koto session import` of A's session on B, B's `--once --all` prints the copy once, as B's local row, and doesn't print the source under A's prefix. The run's stderr note and the TUI's tally line each count 1 hidden source.
- [ ] A source whose `migrated.json` names a copy that isn't in the scan, and that no local `session_imported` event names, is listed from its own summary, not hidden.
- [ ] A session in A's prefix whose state file has no `summary.json` beside it (as an older release pushes it), and one whose `summary.json` is truncated mid-record, each print in B's `--once --all` with status `unknown`, an empty `current_state`, `intent`, `template` and `machine`, and A's prefix in `workspace`. Each is counted in the stderr note's unreadable tally, and in the TUI each row reads `@unknown` and is counted on the tally line.
- [ ] A summary whose `state_etag` differs from the listed state file's ETag lists its session as `unknown` and counted when the state file's LastModified is 2 minutes old, and shows it from the summary when that LastModified is 10 seconds old.
- [ ] A session B holds a local copy of whose bucket copy has no summary appears once in B's `--once`, as its local row. A no-summary session named `review` under A's prefix still prints beside B's own local `review`.
- [ ] With sessions named `review` on A and on B, B's `--once review` prints two lines, each with its own machine in the ninth field and its own workspace in the tenth.
- [ ] B's run records no write request, no GET of a state file and no request for a key under any `ctx/` in the fake bucket's request log; the only GETs it makes are of `summary.json` and `migrated.json` objects.

**Check**: Run `cargo test --test dashboard_cross_machine_test` and see the new cases pass beside step 1's and step 2's. They run two `koto` processes against the in-process fake bucket (`tests/support/fake_s3.rs`, now recording each object's LastModified and letting a test set it): A cleans up a session, B imports one of A's sessions, the test removes a summary, truncates one, and replaces a state file without its summary at a chosen LastModified, and each case asserts B's `--once --all` lines and stderr note, the TUI's rendered buffer and tally line (drawn into a ratatui `TestBackend` with the feed on a short test interval), and the request log. A further case makes the fake bucket fail the listing of A's prefix and asserts A's row stays with the failure on the status line. Then run `cargo test remote_feed` and see the unit tests of the pure classifier pass, one per rule in order, including the marker whose named copy isn't in the scan and the 60-second window.

**Dependencies**: Step 2

**Files**: `src/session/remote_feed.rs`, `src/session/cloud.rs`, `src/session/row.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard.rs`, `src/cli/dashboard_render.rs`, `tests/dashboard_cross_machine_test.rs`, `tests/support/fake_s3.rs`, `docs/reference/session-feed.md`

### Step 4: the machine name follows KOTO_MACHINE_NAME and machine.name and survives every rewrite

**Goal**: A new session records the machine name the developer chose: `KOTO_MACHINE_NAME` when it's set, non-empty and valid, else `machine.name` from the user-level config file, else the short hostname, coerced. A name is 1 to 64 characters of `[A-Za-z0-9._-]` and never `unknown`; `koto config set --user machine.name` refuses anything else, and an invalid environment value is skipped with one stderr warning. Step 1 stamped `workflow_initialized` from the environment or the hostname only, and recorded the environment value unchecked; this step adds the config key and the validation it left out, and shares that rule with the dashboard's reader of bucket names. The recorded name is checked to survive a rebind, a push and pull, a resolve, a rewind, a cancel and a header rewrite by koto v0.33.0 (the last release before the machine stamp). An import records the importing machine on its new `session_imported` event, and a session's producing machine follows its latest import. `koto dashboard --detail` reports `machine`, and the guides document the name, where it comes from and that it's published to the bucket. This keeps the origin both evidence scenarios name correct under the developer's own settings and across the operations that rewrite a session. The name stays on the session's own events: nothing is read from `ctx/`, no owner field is added, and project config loading and how koto resolves the bucket and credentials don't change.

**Acceptance Criteria**:

- [ ] With `KOTO_MACHINE_NAME=env-name` and `machine.name = "cfg-name"` in the user-level config file, a new session records `env-name`. With only the config key it records `cfg-name`. With neither it records the hostname cut at its first `.`. With `KOTO_MACHINE_NAME=""` it records `cfg-name`. Sessions created by `koto init`, a child spawn and an inline init record the same name, on the local backend and on the cloud backend.
- [ ] `koto config set --user machine.name` accepts a 64-character name and writes it to the user-level file. It refuses a 65-character name, `unknown`, `""` and `"bad name!"`, exiting non-zero and leaving the file byte-identical.
- [ ] `KOTO_MACHINE_NAME="bad name!"` (and likewise `KOTO_MACHINE_NAME=unknown`) produces exactly one stderr line naming `KOTO_MACHINE_NAME`, the command still succeeds, and the session records the config value, or the coerced hostname when no config value is set. A batch that creates several sessions in one process prints that warning once.
- [ ] A hostname `my host_01.example.com` resolves to `my-host_01`, a hostname `unknown` to `unknown-host`, and an 80-character hostname to its first 64 characters.
- [ ] `koto config set machine.name x` without `--user` exits non-zero and writes no project config file. A `[machine] name` in the project's `.koto/config.toml` is never read: with only that set, the session records the hostname.
- [ ] After a `koto session rebind`, a push and pull round trip through the in-process fake bucket, a `koto session resolve --keep remote`, a `koto rewind` and a `koto cancel`, the session's recorded machine is unchanged. After koto v0.33.0 rewrites the header of a session this build created, this build's `--detail` still reports the same machine.
- [ ] After `koto session import` of machine A's session on machine B, the new copy's last event is a `session_imported` that carries `machine_name: "machine-b"`. B's `--detail` and `--once` for the copy show `machine-b`, and A's `--detail` for the source still shows `machine-a`. A copy whose latest `session_imported` event has no `machine_name` (an import made by a release before this step) reads as having no recorded machine, even though its copied `workflow_initialized` names the source's machine.
- [ ] `koto dashboard --detail <name>` prints a `machine` field holding the recorded name, and `"machine": null` for a session from the checked-in session store koto v0.33.0 wrote, whose `--once` line under the cloud backend has an empty ninth (`machine`) field. A cloud-backed `--detail` run records no write request in the fake bucket's request log.
- [ ] For the same session, every `--detail` field this build printed before (including `anchor` and `store`, the origin facts it shows) is byte-identical apart from the added `machine` field, and the state file header's `origin` object is byte-identical.
- [ ] `docs/guides/cli-usage.md` lists `machine.name` in the config keys reference as user-only and describes `KOTO_MACHINE_NAME`, the order of the sources, the name rule and the warning. `docs/guides/cloud-sync-setup.md` says the default name is the short hostname, that it's published to the bucket, and how to set another. `docs/reference/session-feed.md` describes `session_imported.machine_name` (in the field table and the frontmatter), names `machine.name` among the sources in the `workflow_initialized.machine` field and `--once` column 9, and adds `machine` to the Session Detail table as the one fact printed as `null` rather than left out. The koto-user skill's command reference mentions `machine` in `--detail` and the `machine.name` key.

**Check**: Run `cargo test --test machine_name_test` and see it pass. It runs the built `koto` under a scratch `KOTO_HOME` against the in-process fake bucket (`tests/support/fake_s3.rs`, as `tests/dashboard_cross_machine_test.rs` does). It covers the order of sources across `koto init`, a child spawn and an inline init on both backends, the `config set --user` accept and refuse cases with the file compared byte for byte, the single warning for an invalid environment value, the ignored project key, the rebind, push and pull, resolve, rewind and cancel survival, the import on a second machine, and `--detail`'s `machine` (null on the checked-in v0.33.0 store) with an unchanged `origin` and no write in the request log. Then run `cargo test machine_name` for the unit tests of the resolver, its coercion and the `producing_machine` rule for imports, and see them pass. Finally run `cargo test --test machine_compat_v0_33_test` and see its fixture case pass: it reads the checked-in session a koto v0.33.0 header rewrite produced. Its real-binary case runs only when `KOTO_COMPAT_V0_33_BIN` names a koto v0.33.0 binary, and without it passes having checked nothing and says so, as `tests/pin_compat_v0_23_test.rs` does. This step adds no CI job for the real-binary run and doesn't edit `.github/workflows/validate.yml`; a follow-up issue tracks an opt-in job.

**Dependencies**: Step 1

**Files**: `src/session/version.rs`, `src/config/mod.rs`, `src/cli/mod.rs`, `src/engine/types.rs`, `src/session/cloud.rs`, `src/session/anchor/tests.rs`, `src/run_journal/tests.rs`, `src/cli/dashboard_data.rs`, `src/session_view.rs`, `src/cli/dashboard.rs`, `tests/machine_name_test.rs`, `tests/machine_compat_v0_33_test.rs`, `tests/fixtures/koto-v0.33.0.sha256`, `tests/fixtures/sessions-v0.33.0/`, `tests/dashboard_detail_test.rs`, `docs/guides/cli-usage.md`, `docs/guides/cloud-sync-setup.md`, `docs/reference/session-feed.md`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`

### Step 5: a slow, failing or read-only bucket leaves machine B's local sessions visible with one classed notice

**Goal**: Every bucket request the dashboard's reader makes times out after 10 seconds, a timed-out request isn't retried, and every failure the reader meets falls in one of four classes: `unreachable` (connection refused, host not resolved, or any other answer that isn't a usable reply), `denied` (HTTP 401 or 403), `timeout` (no answer within 10 seconds), and `throttled` (HTTP 429 or 503 after koto's usual retries). `RemoteSnapshot.failure` becomes that classed value (class, endpoint host, failed workspaces) instead of free text. When the top-level listing fails, the run makes no further bucket request, the TUI keeps its local rows and the remote rows it last had and shows one status line naming the class and the endpoint's host, and `--once` prints its local rows, writes one stderr line with the same two facts and exits 0. When some prefixes fail, rows from the readable ones show and the one line says how many workspaces couldn't be read and names the first failure's class; the rows step 3 kept for the failed workspaces stay. A summary GET that fails is counted in the unreadable tally. The status line clears on the first snapshot read without failure. The reader works with keys that grant only list and get: it writes nothing to the bucket and creates nothing in the session store. On the writing side, a failed `summary.json` PUT after a state push warns as before and marks the session, so the next flush writes the summary even when the state didn't change, and clears the mark once a summary PUT succeeds. Step 3 kept prior rows and showed a failed listing as unclassed text; this step classes it and bounds its timing. No row fact comes from `ctx/`, no owner field is added, and project config loading and how koto resolves the bucket and credentials don't change.

**Acceptance Criteria**:

- [ ] With an endpoint that refuses connections, B's `koto dashboard --once --all` prints its local rows, writes exactly one stderr line containing `unreachable` and the endpoint's host, and exits 0; B's TUI shows its local rows and one status line with the same two facts. With every request answered 403 the class is `denied`, and with every request answered 429 through koto's retries it is `throttled`.
- [ ] With the top-level listing failing, the fake bucket's request log shows no request after that listing.
- [ ] With an endpoint that accepts connections and never answers, B's TUI frame after a key press reflects it within 200 ms, and B's `--once` exits 0 within 15 seconds with one stderr line containing `timeout` and the host.
- [ ] With one of three workspace prefixes answering 500 on its listing, B's `--once` prints the other two prefixes' sessions and one stderr line saying 1 workspace couldn't be read and naming the class; the TUI's status line says the same and keeps the rows it last had for that workspace.
- [ ] A session whose listing succeeds but whose `summary.json` GET answers 500 prints with status `unknown` and is counted in the unreadable tally.
- [ ] With B's TUI showing the failure status line, once the endpoint answers normally again the line is gone after the feed's next snapshot.
- [ ] With an empty bucket, B shows its local rows and no status line.
- [ ] With the fake bucket refusing every PUT and DELETE with 403 (keys that grant only list and get), B's TUI and `--once` list A's sessions, the request log shows no write request, and B's session store gains no directory.
- [ ] With the fake bucket failing the `summary.json` PUT once, a tick on A prints the existing warning and succeeds; the next flush on A that changes nothing in the state PUTs `summary.json` (and no state file), after which B's `--once` shows the session from its summary. A further unchanged flush PUTs nothing.
- [ ] A tick on A that pushes state also PUTs `summary.json` within the 30-second action limit; the test prints the summary PUT's latency after the state push.
- [ ] Every class name and the host in the status line and stderr note pass through the dashboard's sanitizer.

**Check**: Run `cargo test --test dashboard_cross_machine_test` and see the new failure cases pass beside steps 1 to 3's. They run `koto` processes against the in-process fake bucket (`tests/support/fake_s3.rs`), a listener that refuses connections, and the file's never-answering endpoint, and assert `--once`'s stdout, its one stderr line and exit code, the TUI's rendered buffer and status line (ratatui `TestBackend`, feed on a short test interval) and the request log. Then run `cargo test --test cloud_summary_retry_test` for the summary retry on the next flush, and `cargo test remote_feed` for the unit tests of the failure classifier, one per class.

**Dependencies**: Step 2

**Files**: `src/session/remote_feed.rs`, `src/session/cloud.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard.rs`, `src/cli/dashboard_render.rs`, `tests/dashboard_cross_machine_test.rs`, `tests/cloud_summary_retry_test.rs`, `tests/support/fake_s3.rs`, `docs/reference/session-feed.md`

### Step 6: machine B sees machine A's change within 60 seconds without refetching unchanged sessions

**Goal**: The TUI's feed keeps a per-session ETag cache in memory, owned by its one fetcher thread and keyed by workspace and session: an entry holds the `summary.json` ETag the workspace listing gave and the row read from that object. A scan GETs a session's summary only when its listed ETag differs from the cached one or the session has no entry, and reuses the cached row otherwise, so a refresh in which no remote session changed reads no object data (listing requests aside). Only a summary that was read and parsed is cached; a failed or unreadable GET is asked again on the next scan. A session gone from a workspace listing that answered loses its entry; a workspace whose listing failed keeps its entries and the rows step 5 keeps. The feed rescans 55 seconds after each scan ends. Each refresh lists at most 200 workspace prefixes: at 200 or fewer, every prefix every refresh; above 200, every new prefix and every prefix whose newest listed object changed in the last 30 minutes, then idle prefixes in rotation, oldest-listed first, up to the budget, keeping their last rows meanwhile. `--once` keeps no cache and reads the bucket's current state on every run, and with 200 prefixes, 2,000 sessions and every request answered in 50 ms it finishes within 30 seconds. The cloud sync guide replaces its sentence that bounding the scan would need an index of session names with the dashboard's listing cost (one top-level listing plus one listing per prefix, at most 200 per TUI refresh, and GETs only for changed summaries), and the koto-user skill's command reference describes `koto dashboard --once` and its remote rows. No row fact comes from `ctx/`, no owner field is added, and project config loading and how koto resolves the bucket and credentials don't change.

**Acceptance Criteria**:

- [ ] With B's TUI feed on a test interval and A's sessions unchanged, the fake bucket's request log for the second and every later scan holds listings only, no GET.
- [ ] After a tick on A changes one session's state, the next scan GETs that session's `summary.json` and no other summary, and B's TUI shows the new state.
- [ ] With the feed's interval under test control, B's TUI shows A's new state at most one interval after the tick's summary PUT; the production interval is 55 seconds, so the bound is within 60 seconds of the object's last-modified time.
- [ ] A session whose summary GET failed or didn't parse on one scan is asked again on the next, and shows its state once the GET succeeds.
- [ ] After `koto session cleanup` on A, B's next scan drops the row and its cache entry; while A's workspace listing fails, B keeps the row and asks no GET for it.
- [ ] With 250 workspace prefixes in the bucket, no TUI scan lists more than 200 prefixes; a prefix created after the first scan is listed on the next one, and every idle prefix is listed within two scans' rotation.
- [ ] Two `--once` runs in a row against an unchanged bucket each GET every session's summary.
- [ ] With 200 prefixes holding 2,000 sessions and the fake bucket answering every request after 50 ms, B's `koto dashboard --once --all` prints all 2,000 rows and exits 0 within 30 seconds; the test prints the elapsed time.
- [ ] `docs/guides/cloud-sync-setup.md` no longer says bounding the scan would need an index of session names, and states the listing cost above; the koto-user command reference describes `--once` and remote rows.

**Check**: Run `cargo test --test dashboard_cross_machine_test` and see the cache, freshness and budget cases pass beside steps 1 to 5's, reading the fake bucket's request log (`tests/support/fake_s3.rs`) and the TUI's rendered buffer with the feed on a short test interval. Run the scale case (200 prefixes, 2,000 sessions, `set_latency` of 50 ms) and read its printed elapsed time. Run `cargo test remote_feed` for the unit tests of the cache and of the budget's prefix choice, and `cargo test --test doc_names` for the guide and skill text.

**Dependencies**: Step 3

**Files**: `src/session/remote_feed.rs`, `src/cli/dashboard.rs`, `tests/dashboard_cross_machine_test.rs`, `tests/support/fake_s3.rs`, `docs/guides/cloud-sync-setup.md`, `docs/reference/session-feed.md`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`

### Step 7: text and objects another machine wrote can't break or mislead machine B's dashboard

**Goal**: Machine B treats everything it reads from the bucket as untrusted display text. One sanitizer handles every bucket string before it reaches a remote row, replacing each C0 and C1 control character, DEL, U+2028, U+2029, the bidirectional override and isolate characters, the zero-width and directional marks and the tag characters with a single space, in `--once` and in the TUI. Today `--once` folds only tab, newline and carriage return, the list's cells take the text as it came, and the Session tab writes a visible `\u{...}` escape instead of a space. A summary is fetched only when its listed Size is at most 64 KiB, and the read stops at 64 KiB plus one byte. The reader checks each field's cap again (name, template name, state and salient variable 256 bytes each, intent 1 KiB, machine 64) and reads a summary with more than 16 `sources` as unreadable. A `sources` entry in another remote row's summary only recedes the row it names, which stays reachable with `--all` and the `a` key; hiding still needs a `migrated.json` in the hidden session's own directory whose named copy is in the same scan, or a local record, and is counted. A timestamp later than the reader's clock plus five minutes is clamped to the reader's clock, and no timestamp can hang or panic the reader. Each run stops at 5,000 workspace prefixes, 100 listing pages per prefix and 20,000 sessions and shows a `listing truncated` notice, never dropping a local row. The cloud sync guide states the risk the design accepts, and the dashboard guide says what gathering every workspace on one screen exposes. This keeps both evidence scenarios true against objects written to break or mislead the dashboard. No row fact comes from `ctx/`, no owner field is added, and project config loading and how koto resolves the bucket and credentials don't change.

**Acceptance Criteria**:

- [ ] A remote session whose name (its key and its summary's `name`), `current_state`, `intent` and `template_name` each contain a tab, a newline, an ESC, a U+0085 and a U+2028 prints in B's `--once --all` on one line of exactly ten tab-separated fields, with each of those characters replaced by one space (`a<ESC>b` prints as `a b`, never as `\u{1B}`), and still shows its state: the name is compared with its key before it's sanitized. The TUI's rendered buffer for its list row and Session tab, which also shows `session_id` holding the same characters, contains none of them. A session with such a name and no `summary.json` prints the same way, with status `unknown`.
- [ ] Each character of U+0000 to U+001F, U+007F, U+0080 to U+009F, U+2028, U+2029, U+202A to U+202E, U+2066 to U+2069, U+200B to U+200F, U+061C, U+FEFF and U+E0000 to U+E007F, placed between two letters in an `intent` and a `current_state`, comes out of `--once` as those letters with one space between them, and appears nowhere in the TUI's buffer. Text around them, such as `café`, is unchanged.
- [ ] An `intent` holding `<ESC>[2J` and `<ESC>]0;owned<BEL>` leaves no ESC or BEL byte in `--once`'s stdout or the TUI's buffer. The same characters in `created_at`, `kind`, `parent` and `salient_var` reach no row either: B's Session tab shows `created_at` with each one replaced by a space.
- [ ] A `machine` of `bad name!`, `unknown`, a 65-character name, or a name holding a tab or an ESC reads as no recorded machine: an empty ninth field in `--once`, `@unknown` on the TUI's row and `unknown` in its Session tab, with the row's other facts shown from the summary.
- [ ] A session whose `summary.json` is listed at 64 KiB plus one byte prints with status `unknown`, an empty `current_state`, `intent`, `template` and `machine`, is counted in the unreadable tally, and the request log holds no GET of that object. A valid summary padded to exactly 64 KiB shows from its fields.
- [ ] A summary replaced between the listing and the GET by a 64 MiB body prints as `unknown` and is counted, and B's read stops at 64 KiB plus one byte: the fake bucket records at most that many body bytes for a Range request, or a closed connection before the whole body was sent. A `migrated.json` swapped the same way is read no further and hides nothing.
- [ ] A summary with a `current_state`, `template_name`, `salient_var` or `name` of 257 bytes, or an `intent` of 1,025 bytes, prints as `unknown` and counted, and none of that text reaches the row. The same fields at 256 bytes and 1,024 bytes show from the summary. The caps count bytes: a `current_state` of 200 two-byte characters (400 bytes) prints as `unknown`.
- [ ] Each of these prints as `unknown`, counted, with none of its text in the row: a body that isn't JSON, a body that isn't UTF-8, a summary missing `is_blocked`, one with an unknown field, one with `schema` 2, one whose `name` or `home` isn't its key's, and one with an empty `session_id`.
- [ ] A summary from session X with a `sources` entry naming session Y by `session_id` takes Y out of B's default `--once`, `--needs-you` and the TUI's needs-you band. Y still prints with its own state in `--once --all` and shows after the `a` key, and X is listed as before. No `hidden source(s)` count appears for it, and Y's status isn't `unknown`.
- [ ] A `sources` entry naming a session B holds locally leaves that local row in B's default `--once`, and an entry naming its own summary's session recedes nothing, nor does one carrying Y's prefix and name with another `session_id`. A session with no `migrated.json` of its own stays listed however many summaries name it. With a forged `migrated.json` in its own directory and the named copy in the same scan it is hidden and counted on the tally line and in the stderr note, never silently.
- [ ] A summary with 17 `sources` entries prints as `unknown` and counted, and recedes nothing; one with 16 entries shows from the summary.
- [ ] A summary whose `parent` names itself, and two summaries whose `parent` fields name each other, still show each of those rows in B's TUI (today they aren't roots and vanish with no count), as they do in `--once`.
- [ ] A `migrated.json` and a `parent` holding `../../x`, read by B, cause no GET outside the session's own `summary.json` and `migrated.json`, no file or directory in B's session store and no write in the request log. Pressing Enter, Tab, `r`, `a`, `j` and `k` on a remote row adds no write to the log and nothing to the store, and shows only the Session tab's facts and the other tabs' one line.
- [ ] A summary with `last_event_at` one hour ahead of B's clock prints idle `0s` in `--once`, and in B's TUI, across feed scans that find its summary's ETag unchanged, its idle keeps growing, reading at least 2 seconds on a frame drawn 2 seconds after the first with a scan between them; one 4 minutes ahead isn't clamped and still reads `0s` on both frames. An `unknown` row whose state file's LastModified is one hour ahead ages the same way. A `created_at` one hour ahead prints elapsed `0s`, and the Session tab shows no time later than B's clock.
- [ ] A summary whose `last_event_at` or `created_at` is `18446744073709551615-01-01T00:00:00Z`, `2026-01-01T18446744073709551615:00:00Z`, `2026-01-01T00:00:00.ééZ` or `yesterday` prints in B's `--once --all` on ten fields, and the run exits 0 within 30 seconds with no panic. B's TUI draws its frame after a key press within 200 ms with that row present.
- [ ] With 5,001 workspace prefixes in the bucket, B's `--once` lists no more than 5,000 of them, writes one stderr line containing `listing truncated`, and prints B's local rows. B's TUI shows the same notice on its status line and keeps its local rows.
- [ ] With one prefix holding more than 100 listing pages, the request log shows at most 100 listing requests for it, the rows from those pages and from the other prefixes print, and the notice appears.
- [ ] With more than 20,000 sessions in the bucket and one prefix, last in key order, holding the state files changed most recently, B's `--once --all` prints exactly 20,000 remote rows, those of that prefix among them, plus every local row, writes the notice, and the request log holds at most 20,000 summary GETs. B's TUI, over enough scans to rotate through every prefix, never holds more than 20,000 remote rows and shows the notice.
- [ ] A bucket below every ceiling shows no notice, including one with 250 prefixes, which only passes the TUI's 200-prefix budget. Once a bucket that was over a ceiling drops below it, the notice is gone after the feed's next snapshot.
- [ ] `docs/guides/cloud-sync-setup.md` states that any machine with write credentials can make a row show the wrong state or machine, hide a row by planting a marker and a copy (counted), or pad the bucket past a ceiling (shown by the notice). `docs/guides/dashboard.md` says the dashboard gathers every workspace's intents and states on one screen, which matters on a shared or recorded screen. `docs/reference/session-feed.md` replaces its sentence on sanitized fields with the sanitizer's rule for bucket text, and describes the ceilings and the notice.

**Check**: Run `cargo test --test dashboard_cross_machine_test` and see the untrusted-object cases pass beside steps 1 to 6's. They plant objects directly in the in-process fake bucket (`tests/support/fake_s3.rs`, with `put`, `put_after` for the swap, `set_last_modified` for the ceilings' order and LastModified cases, and now answering a Range request with 206 and recording the body bytes each GET was sent) and assert `--once`'s stdout field by field, its stderr lines and exit code, the TUI's rendered buffer (ratatui `TestBackend`, feed on a test interval) and the request log. The fake's 1,000 keys a page gives the 100-page case with padding objects in one session directory. Run `cargo test remote_feed` for the unit tests of the sanitizer (one per character range), the `sources` rules, the clamp on each side of five minutes and the ceilings' choice of sessions; `cargo test summary` for the field caps and the `sources` cap at their boundaries; `cargo test dashboard_data` for the timestamp parser on each out-of-range input and the parent cycles; and `cargo test --test doc_names` for the guide text.

**Dependencies**: Step 3

**Files**: `src/session/remote_feed.rs`, `src/session/summary.rs`, `src/session/cloud.rs`, `src/cli/dashboard_data.rs`, `src/cli/dashboard_state.rs`, `src/cli/dashboard_render.rs`, `src/cli/dashboard.rs`, `tests/dashboard_cross_machine_test.rs`, `tests/support/fake_s3.rs`, `docs/guides/cloud-sync-setup.md`, `docs/guides/dashboard.md`, `docs/reference/session-feed.md`

## Implementation Sequence

The steps run in order, each in its own pull request after this scoping
merges, through `/work-on` on this plan. Step 4 needs only step 1 and
step 5 only step 2, so either can run beside step 3 once its dependency is
done; steps 6 and 7 both build on step 3's classifier. The plan's owner
details step 3 after step 2's work reaches its terminal, and each later step
the same way. If step 1's delimiter probe fails on R2, the owner re-details
the remaining steps for the design's fallback layout (a sibling summary
object, which needs tombstones and new deletes) before step 3 starts.

Which evidence scenario each step advances:

- Step 1: the text scenario (`koto dashboard --once` on B shows A's session,
  state and origin machine); PRD R6, R7, R14 and R20, R10 in `--once`, and the parts of R1
  and R18 the skeleton needs.
- Step 2: the interactive scenario (the dashboard on B shows the same);
  PRD R8, R9 and R10 in the TUI, and R23.
- Steps 3 to 7: both scenarios, by keeping them true for removed, imported,
  older-release and same-named sessions (3), for the origin name's settings
  and its survival (4), under bucket failures and read-only keys (5), within
  the 60-second freshness bound and at scale (6), and against objects another
  machine wrote to mislead (7).
