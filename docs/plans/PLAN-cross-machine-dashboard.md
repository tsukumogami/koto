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

Removed sessions drop, an import's migrated source is hidden and counted, sessions an older release wrote or last advanced and stale summaries read unknown, and two machines' same-named sessions show as two rows (both evidence scenarios; PRD R9 to R13).

### Step 4: the machine name follows KOTO_MACHINE_NAME and machine.name and survives every rewrite

The name follows the environment, then user-level machine.name, then the short hostname, with validation and `unknown` reserved; it survives rebind, push and pull, resolve and an older release's header rewrite, an import records the importing machine, --detail reports it, and the guides document it (the origin both evidence scenarios name; PRD R1 to R5, R16).

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
