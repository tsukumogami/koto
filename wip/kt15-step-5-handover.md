# KT15 step 5 handover

Branch `feature/kt15-bucket-failures`, from koto main dfcd233. Delete this file
before the PR merges (wip hygiene); nothing committed may reference it.

## Done

- Step 5 detailed in docs/plans/PLAN-cross-machine-dashboard.md (goal, 11
  acceptance items, check, files); `step-detail-check.sh ... 5` exits 0.
- work-on session `task_kt15-step-5` opened (free-form, `--auto`,
  `--verification=hosted-ci`), dispatcher linked, sitting at
  `post_research_validation` with no verdict submitted. Its worktree is this
  one; attaching from another worktree needs a rebind, which needs the merge
  seat's go.
- Nothing else built. No code changed.

## What main already has (verified by subagent reads, file:line at dfcd233)

- `remote_feed.rs`: `RemoteSnapshot.failure: Option<String>` (:345) plus
  `failed_workspaces: Vec<String>`, `top_level_failed: bool`.
  `REQUEST_TIMEOUT` 10 s (:41) applied by `RemoteReader::new` (:369) via
  `bucket.with_request_timeout`. Top-level listing failure returns at once
  (`scan` :494-502). Per-prefix failures collected, message built at :533-538.
  Summary GET failure already lists `unknown` + counts unreadable (`get` :424,
  `read_summary` :433). No classification anywhere: failures are strings.
- TUI: `dashboard_data::apply_snapshot` (:769-785) keeps rows for failed
  workspaces and sets `tree.remote_failure` (None clears it). Status line
  `dashboard_render.rs:327-332`: `"\u{26a0} bucket: {failure}"`.
- `--once`: `once_remote_rows` in `dashboard.rs` ~110-160, stderr
  `note: other machines' sessions weren't all read: {failure}` (:146); exits 0.
- Errors: rust-s3 0.37 sync attohttpc (`Cargo.toml:53`, `fail-on-err`).
  Timeout, refused connection and DNS failure all arrive as `S3Error::Atto`
  (inferred, not traced in attohttpc); HTTP errors as `HttpFailWithBody`.
  Status helpers near `cloud.rs:6560-6590`; `is_transient` (:6569) retries
  Atto and Io, so a timeout is retried.

## The timing problem (the main work)

Against a never-answering endpoint, `--once` today costs about 64 s:
`list_retrying` (cloud.rs:6596) makes 3 calls (`LIST_ATTEMPTS` :215), and
rust-s3's own `retry!` (RETRIES = 1, lib.rs:38) doubles each, 10 s apiece.
The acceptance needs 15 s. The reader needs its own listing path that doesn't
retry a timeout (classify first, retry only 429/503/5xx) and must avoid
rust-s3's internal retry for reads (check whether 0.37 exposes `set_retries`;
it's a process-wide setting, so scope it to the dashboard process or use the
bucket's own option if one exists).

## Summary PUT retry marker

- `put_summary` cloud.rs:1524, called from `put_state` :1514 and
  `put_state_over` :1604. Warns on failure, skips when `probes.read_only`.
- An unchanged flush returns before any network call: `flush_batch`
  :3238-3249 (`!journal.is_open()` or no pending).
- Marker: copy the `sync_stamp.rs` const + `save(&dir)` pattern (:45-58),
  e.g. a `cloud-summary-pending` file in the local session dir. Nothing
  uploads the whole dir (flush PUTs ctx keys, manifest, state, version.json,
  pins; `is_host_local_key` context.rs:62), so it can't leak. Cleanup's
  `remove_dir_all` (local.rs:121) removes it.
- Retry point: in `flush_pending`/`flush_batch`'s early returns, when the
  marker exists, read the local state file (as :3328 does) and the recorded
  ETag from `self.stamp(id).state_etag`, call `put_summary`, clear the marker
  on success. `SessionSummary::build` (summary.rs:107) also reads session-dir
  files, no network.

## Test support

- fake_s3: `fail(method, key, status)` exact key only (:204, matching
  :511-517), no wildcard; read-only keys need a new all-PUT/DELETE fault.
  No refuse-connections helper: bind a TcpListener, take its port, drop it.
  LIST pages at 1000 with continuation tokens (:612).
- `dashboard_cross_machine_test.rs`: `never_answering()` :524, `once_on`
  :778, `drive_tui` :790, `assert_read_only` :761; step 3's failed-listing
  test :883 asserts on `"\u{26a0} bucket:"` and `"workspace prefixes"`, and
  will need its expected text updated for the classed line.

## Open question for the coordinator

The PRD's four classes don't place an HTTP 500 or an unparseable reply. The
detail maps any non-usable answer to `unreachable` (recommended: the endpoint
didn't serve the request, and a fifth class would break the PRD's closed set).
