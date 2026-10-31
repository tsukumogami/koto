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

**Check**: Run `cargo test --test machine_name_test` and see it pass. It runs the built `koto` under a scratch `KOTO_HOME` against the in-process fake bucket (`tests/support/fake_s3.rs`, as `tests/dashboard_cross_machine_test.rs` does). It covers the order of sources across `koto init`, a child spawn and an inline init on both backends, the `config set --user` accept and refuse cases with the file compared byte for byte, the single warning for an invalid environment value, the ignored project key, the rebind, push and pull, resolve, rewind and cancel survival, the import on a second machine, and `--detail`'s `machine` (null on the checked-in v0.33.0 store) with an unchanged `origin` and no write in the request log. Then run `cargo test machine_name` for the unit tests of the resolver, its coercion and the `producing_machine` rule for imports, and see them pass. Finally run `KOTO_COMPAT_V0_33_BIN=<koto v0.33.0 binary> cargo test --test machine_compat_v0_33_test` in the new `machine-compat-v0-33` CI job (modeled on `pin-compat-v0-23`) and see the header-rewrite case pass. Without the variable that test passes having checked nothing and says so, as `tests/pin_compat_v0_23_test.rs` does.

**Dependencies**: Step 1

**Files**: `src/session/version.rs`, `src/config/mod.rs`, `src/cli/mod.rs`, `src/engine/types.rs`, `src/session/cloud.rs`, `src/session/anchor/tests.rs`, `src/run_journal/tests.rs`, `src/cli/dashboard_data.rs`, `src/session_view.rs`, `src/cli/dashboard.rs`, `tests/machine_name_test.rs`, `tests/machine_compat_v0_33_test.rs`, `tests/fixtures/koto-v0.33.0.sha256`, `tests/fixtures/sessions-v0.33.0/`, `tests/dashboard_detail_test.rs`, `.github/workflows/validate.yml`, `docs/guides/cli-usage.md`, `docs/guides/cloud-sync-setup.md`, `docs/reference/session-feed.md`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`

### Step 5: a slow, failing or read-only bucket leaves machine B's local sessions visible with one classed notice

Requests time out at 10 seconds, failures show as unreachable, denied, timeout or throttled on one status line or stderr note while local rows stay visible and the TUI keeps responding, partial failures name a count, and list-and-get keys work with nothing written (both evidence scenarios under failure; PRD R17 to R19, R23).

### Step 6: machine B sees machine A's change within 60 seconds without refetching unchanged sessions

The TUI feed rescans every 55 seconds within its listing budget and fetches only changed summaries, --once finishes within 30 seconds at 200 prefixes and 2,000 sessions, and the cloud sync guide (its listing cost and its superseded index sentence) and the koto-user skill reference describe it (interactive scenario freshness; PRD R21, R22, R24).

### Step 7: text and objects another machine wrote can't break or mislead machine B's dashboard

Control, bidirectional and zero-width characters are replaced, oversized or malformed summaries and forged sources read unknown or only recede, timestamps are clamped, and a run stops at its ceilings with a truncation notice (PRD R15 and the design's Security Considerations).

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
