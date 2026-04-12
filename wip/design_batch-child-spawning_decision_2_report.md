<!-- decision:start id="init-atomicity" status="confirmed" -->
### Decision: Fix the atomic child-spawn window in `handle_init` without breaking append-only state file semantics

**Context**

Koto's `handle_init` (`src/cli/mod.rs:1029-1160`) builds a session in three separable filesystem steps: `backend.create(name)` makes the session directory, `backend.append_header(name, header)` creates the state file and writes the header line, and two subsequent `backend.append_event` calls write the `WorkflowInitialized` event (seq 1) and the initial `Transitioned` event (seq 2). The integration lead's resume-flow analysis (section 5 of `wip/research/explore_batch-child-spawning_r1_lead-koto-integration.md`) flagged that any crash after `append_header` but before both events land leaves a state file whose first line is a valid header and whose event log is empty or truncated. `backend.exists()` returns true because it checks for the state file's presence, so the scheduler skips re-spawning the child. The next `handle_next` on that child reads the empty event log and errors out at `src/cli/mod.rs:1337` with `PersistenceError`, or errors at `derive_machine_state` (line 1376) if only the `WorkflowInitialized` event is present. Any downstream task waiting on the child is then wedged until manual cleanup.

This matters disproportionately for the batch scheduler: crashes during batch materialization become unrecoverable without operator intervention, which defeats the "crashes are recoverable" property that the rest of the engine carefully preserves. It is therefore correctness-critical for batch spawning.

The constraints are non-negotiable: the event log is strictly append-only after the header (cloud sync, rewind, and the `expected_seq` monotonicity check at `src/engine/persistence.rs:180` all depend on this), no new event types may be introduced to mark "init complete", the `backend.exists(name)` check at `src/cli/mod.rs:1059` must still work, `read_events` must not change shape, and both `LocalBackend` (`src/session/local.rs`) and `CloudBackend` (`src/session/cloud.rs`) must be fixed. `CloudBackend` delegates state-file writes to `LocalBackend` and then uploads the finished file with `sync_push_state` (`src/session/cloud.rs:81-97`, called after every `append_header` and `append_event`), so fixing the local path automatically fixes the cloud path as long as the cloud sync still sees a complete file.

**Assumptions**
- `rename(2)` on the same filesystem as the session directory is atomic (POSIX guarantee). The session directory and a sibling tmp file always live on the same filesystem because the tmp file will be created inside the session directory, which already exists at the point this code runs. If this assumption fails (e.g., exotic FS without atomic rename), the fix degrades to the current behavior and no worse.
- No production or dev user currently has a header-only state file sitting on disk or in S3. This is safe to assume because (1) v0.7.0 is the first release to expose session backends with this code path, (2) the crash window is narrow, and (3) the bug is only reachable via process kill or hard-crash during a very small window. Any stragglers can be cleaned up with `koto session cleanup <name>` just as they can today.
- `CloudBackend.sync_push_state` uploads the entire state file as one S3 PUT, not a streaming append. Verified at `src/session/cloud.rs:81-97`: it reads the full file, then calls `put_object`. S3 PUT is atomic from the reader's perspective, so the remote file is either the old version (missing) or the new complete version — never a partial state.
- The `Transitioned` initial event has no dependencies on gate evaluation, integration calls, or anything that could fail mid-way. Its construction (`src/cli/mod.rs:1140-1144`) uses only the compiled template's `initial_state` field, which is already in memory at that point. Both init events can be built before any write occurs.

**Chosen: Atomic init bundle via `init_state_file` method that writes header + `WorkflowInitialized` + initial `Transitioned` to a temp file and renames into place**

Add a new method on `SessionBackend`:

```rust
fn init_state_file(
    &self,
    id: &str,
    header: &StateFileHeader,
    events: &[(EventPayload, String)], // (payload, timestamp) pairs, seqs auto-assigned
) -> anyhow::Result<()>;
```

For `LocalBackend`, the implementation lives in `src/engine/persistence.rs` as a new free function `write_init_bundle(path: &Path, header: &StateFileHeader, events: &[(EventPayload, String)])` that:

1. Computes the final state file path via `state_file_name(id)` in the session directory (directory already exists — `backend.create` ran first).
2. Creates a sibling temp file in the same directory using `tempfile::Builder::new().prefix(".koto-").suffix(".tmp").tempfile_in(session_dir)`. Same-directory tempfile guarantees same-filesystem rename.
3. Writes the header line (JSON + newline), then each event line with monotonic seq starting at 1 (the events list is supplied by `handle_init` in order: `WorkflowInitialized` then `Transitioned`). All lines are buffered and written in one pass; no partial-seq state is possible because this function constructs the events in memory before any bytes hit disk.
4. Calls `sync_data()` on the temp file to flush content to disk.
5. Calls `persist(final_path)` (the `tempfile::NamedTempFile::persist` call, which uses `rename(2)` under the hood). This is the atomic commit point.
6. Optionally `fsync`s the parent directory to ensure the rename is durable. This is standard atomic-file practice and matches what `write_manifest` already does at `src/session/local.rs:189-209`.

`LocalBackend::init_state_file` is thin: it computes `self.base_dir.join(id).join(state_file_name(id))`, calls the new persistence helper, and returns.

`CloudBackend::init_state_file` wraps `self.local.init_state_file(id, header, events)?` and then calls `self.sync_push_state(id)` once, after the local commit. Because the local file is either the complete bundle or nothing (the rename is atomic), the S3 upload either ships a complete file or ships nothing. No intermediate state can ever reach S3.

`handle_init` changes:

```rust
// Replace the three separate backend calls (append_header + two append_event)
// with one atomic bundle call.
let init_payload = EventPayload::WorkflowInitialized {
    template_path: cache_path_str,
    variables,
};
let transition_payload = EventPayload::Transitioned {
    from: None,
    to: initial_state.clone(),
    condition_type: "auto".to_string(),
};
let events = vec![
    (init_payload, ts.clone()),
    (transition_payload, ts.clone()),
];
if let Err(e) = backend.init_state_file(name, &header, &events) {
    exit_with_error(serde_json::json!({
        "error": e.to_string(),
        "command": "init"
    }));
}
```

The old `append_header` and `append_event` methods stay on the trait unchanged. They are still used everywhere else (every state transition after init, every evidence submission, every integration result) and they must remain append-only. Only `handle_init`'s call site changes. Existing unit tests for `append_header` and `append_event` keep passing because those functions are untouched.

**Rationale**

This choice is safer than all four alternatives because it makes init genuinely atomic at the filesystem level, and it does so without violating any of the constraints listed above:

1. **Append-only semantics preserved.** After `init_state_file` returns, every subsequent write still goes through `append_event`, which opens the file with `O_APPEND` and only appends. The file is written once atomically at init and then strictly appended thereafter. Rewind's expected-seq check, cloud sync's whole-file upload, and the monotonic seq validation in `read_events` all continue to work exactly as before, because the bundle writes seqs 1 and 2 in the correct order and the file on disk has the same shape as a file built by sequential `append_header` + `append_event` calls that never crashed. A reader cannot tell which code path produced the file.

2. **No new event types.** The only events written are the existing `WorkflowInitialized` and `Transitioned` events. The fix is purely about how the file is materialized, not what goes in it.

3. **`backend.exists()` still works.** It still checks for the state file's presence at its final path. Before the rename, only a dotfile-prefixed temp file exists (named like `.koto-<random>.tmp`), and `exists()` checks only for `koto-<id>.state.jsonl`. So during the brief window where the temp file exists but the rename hasn't happened, `exists()` correctly returns false, and a retry of `handle_init` will correctly see the session as uninitialized and re-run init. The only leftover is a stale dotfile, which `list()` already skips (`src/session/local.rs:89-103` iterates directory entries and only considers those with valid state file names). A follow-up sweep can clean orphaned `.tmp` files at next `cleanup`.

4. **`read_events` is untouched.** The function keeps its current behavior: parse header, then parse events with seq validation.

5. **Cloud backend compatibility.** `CloudBackend::sync_push_state` uploads the whole file via S3 PUT after the atomic rename. S3 PUT is itself atomic: either the old object version is visible or the new complete version is visible, never a partial write. So the remote state file mirrors the local guarantee. Because `init_state_file` only calls `sync_push_state` once (after the complete file exists on disk), there is no window during which S3 sees a header-only file either.

6. **Low code complexity.** The change touches four files: `src/engine/persistence.rs` (add `write_init_bundle`), `src/session/mod.rs` (add `init_state_file` to the trait with a default implementation that falls back to header + event append for compatibility), `src/session/local.rs` (delegate to persistence helper), `src/session/cloud.rs` (delegate to local + one `sync_push_state`), `src/cli/mod.rs` (replace three calls with one in `handle_init`). No new types, no new events, no new subcommands.

7. **Minimal test impact.** Existing tests for `append_header` and `append_event` continue to test those functions as standalone operations — they are still used after init. A new test in `src/session/local.rs` verifies that `init_state_file` produces a file whose shape is byte-identical to the old three-call path, plus a crash-simulation test that aborts between the temp write and the rename (e.g., by calling the helper up to the rename, then inspecting that no state file is visible to `exists()`). One functional-test update in `test/` may be needed if any scenario relied on the intermediate state between header write and first event — none should, because that window was never documented as observable.

8. **No migration concern.** v0.7.0 is recent and the crash window is narrow enough that no header-only files are expected to exist in the wild. If one does exist (e.g., a developer's local dev machine), `koto session cleanup <name>` already removes it. No migration script is required, no on-disk schema change is made, and header-only files produced by pre-fix binaries remain broken but visible the same way they are today — users see the same `PersistenceError` they always did. We are not regressing any case.

**Alternatives Considered**

- **Option 2: Combine header and first event into one `append_header` call.** Modify `append_header` to accept the first event and write both lines in one `open+write+close`. Rejected because the single-write approach is *not* atomic without a rename — a crash mid-write leaves a file with a valid header and a truncated event line. `read_events` would then use its truncated-final-line recovery path and return zero events, which puts us right back in the original crash window. Making it atomic requires tmp+rename anyway, at which point Option 1 (init bundle) is strictly more correct because it also covers the Transitioned event and the single `sync_push_state` optimization on cloud. Option 2 also mutates a method (`append_header`) whose signature and behavior are depended on elsewhere, whereas Option 1 leaves existing methods untouched.

- **Option 3: Repair subcommand.** Add `koto session repair <name>` that detects header-only state files and either deletes or "completes" them. Rejected because it pushes a correctness problem onto operators. Any user-facing surface area that requires manual intervention after crashes is a regression for an engine whose core promise is recoverability. It also doesn't help unattended batch spawning, which is exactly the scenario this decision is meant to protect: if the scheduler crashes while materializing 20 children in a loop, we cannot expect a human to notice and run `repair`. Automation could run it, but then the "fix" is a cron job that papers over an atomicity bug. The right place to fix atomicity is at the site of the write, not in a separate cleanup pass.

- **Option 4: Idempotent `handle_init`.** Make `handle_init` detect header-only state files and complete them instead of erroring at the `exists()` check. Rejected for three reasons. First, it requires reading and classifying every "already exists" case to distinguish "fully initialized" from "header-only" from "header + partial events", which adds substantial logic to an init path that should be simple. Second, the completion path would have to re-run template compilation and variable resolution, and there's no guarantee the original `--var` flags are available — `handle_init` is typically called once, not retried, and batch spawning calls it with specific variable values that would need to be preserved somehow. Third, this doesn't remove the underlying crash window; it just adds a second codepath that tries to patch around it, increasing the chance of divergence between the "happy path" and the "repair path". The atomicity bug should be killed at the source.

- **Option 5: Accept the window and rely on the existing recovery path in `read_events`.** `read_events` already recovers from a truncated final event line by warning and returning what it has. One might argue the crash window is narrow enough to ignore. Rejected because the window's narrowness does not bound its consequences: for batch spawning, a single unlucky crash wedges an entire child workflow indefinitely. "Rare but unrecoverable" is worse than "never" for a feature whose purpose is to make scheduled work reliable.

**Consequences**

*What gets easier:*
- Batch scheduler can safely create many children in a tight loop. A crash at any point during the batch leaves every created child either completely initialized (visible to `exists`, runnable by `handle_next`) or not created at all (invisible to `exists`, retryable on next scheduler tick). No partial children, no manual cleanup, no wedged workflows.
- The resume story for `handle_init` collapses to a single invariant: "if the state file exists, it has header + `WorkflowInitialized` + initial `Transitioned`, and `handle_next` can immediately run". This is much easier to reason about than the current "three-step crash window".
- Cloud sync gets slightly more efficient during init: one `sync_push_state` call (after the bundle is committed) instead of three (one after `append_header`, one after each `append_event`). That's two fewer S3 PUTs per child spawn, which multiplies in batch scenarios.
- Future features that need atomic initialization of other state files (e.g., checkpoint files, snapshot files) now have a pattern to copy.

*What gets harder:*
- The `SessionBackend` trait grows one method. New backend implementations must either implement `init_state_file` or inherit a default that calls the old three-step path (still broken, but no worse than today and explicitly opt-out). We should document the atomicity guarantee on the trait method.
- Testing the crash window requires simulating a crash between tmp-write and rename. The test helper has to avoid calling `persist()` and then assert that `exists()` still returns false and the session is retryable. This is a one-time test-infra cost.
- Any future feature that wants to do work between "header written" and "first event written" no longer has that seam. No known feature wants this; the seam existed only because the code was written as sequential steps, not because anything depended on the intermediate state.

---

## Crash-Failure Mode Walkthrough (Critical Decision Requirement)

The remainder of this section enumerates every point in the new `handle_init` flow where the process could die, and demonstrates that each point produces a recoverable state. "Recoverable" means: the next invocation of `handle_init` (or a scheduler retry) can complete successfully without manual cleanup, *or* a pre-existing operator-facing error is raised that is no worse than the current behavior.

Let `T` denote the ordered steps in the fixed `handle_init`:

1. `T1`: validate workflow name
2. `T2`: validate parent exists (if any)
3. `T3`: check `backend.exists(name)` — expect false
4. `T4`: `backend.create(name)` creates the session directory
5. `T5`: compile template, load compiled template
6. `T6`: resolve `--var` flags
7. `T7`: build `StateFileHeader`, `WorkflowInitialized` payload, `Transitioned` payload — all in memory, no I/O
8. `T8`: `backend.init_state_file(name, &header, &events)` invokes the persistence helper
9. `T9`: inside the helper: create temp file in session dir, write header line, write event lines, `sync_data`, `rename` to final path, `fsync` parent dir
10. `T10`: (cloud only) `sync_push_state(name)` uploads the complete file to S3
11. `T11`: print JSON response, exit

**Crash at T1-T3:** No filesystem state has been touched. Session directory does not exist, state file does not exist. `exists()` returns false. Retry is trivial — just run `handle_init` again. Recoverable.

**Crash at T4 (during `create`, or immediately after):** Session directory exists, no state file inside. `exists()` returns false because it checks for the state file, not the directory (`src/session/local.rs:61-63`). Retry: `handle_init` sees `exists() == false`, proceeds, `create` is idempotent (`fs::create_dir_all`), and the flow continues normally. Recoverable.

**Crash at T5-T7:** Session directory exists, no state file. Same as T4. The compile-cache lookup is a read-only operation that writes to `~/.koto/cache/` but produces no stable side effects for this session. Variable resolution is pure. Recoverable.

**Crash at T8 before the helper opens the temp file:** Same state as T4. Recoverable.

**Crash inside T9 *before* the `rename`:** The session directory contains a `.koto-<random>.tmp` file with partial or complete content. The final state file path does not exist. `exists()` returns false. Retry: `handle_init` proceeds, creates a new temp file (new random name so no collision), completes, renames. The old temp file is leaked. It will be cleaned up by `backend.cleanup(name)` if the user ever runs that, or it can be cleaned up eagerly by having `init_state_file` scan for `.koto-*.tmp` in the session dir and remove stale ones before starting. Recoverable. **The leaked temp file is not an operator-visible problem** — it doesn't appear in `list()` (which filters by state file name), it doesn't block retries, and it consumes trivial disk space.

**Crash at the exact moment of `rename`:** POSIX `rename(2)` is atomic. Either the rename completed (state file exists at its final path with full content) or it did not (only the temp file exists). There is no intermediate state. If completed: recoverable by proceeding. If not completed: same as "before rename", recoverable by retry.

**Crash after `rename` but before `sync_data` on parent dir:** The rename is visible to the current kernel's page cache, and `exists()` returns true. If the machine then loses power before the parent dir entry hits disk, the state file could theoretically "disappear" on reboot. This is a weak spot on filesystems that don't journal directory updates, but it is no worse than the current code, which also doesn't fsync the parent dir after `append_header`. And on ext4/xfs/btrfs with journaling (the realistic deployment targets), the rename is durable as soon as the journal commits. The optional `fsync` on the parent directory closes the hole entirely on paranoid configurations. Recoverable.

**Crash after `rename` succeeds but before `T10` (`sync_push_state`):** On local-only deployments (`LocalBackend`), T10 doesn't exist, so this case is vacuous. On cloud deployments, the local file is complete but S3 has not yet received it. The next operation that reads state will read locally (`CloudBackend.read_events` pulls from S3 only as a fallback — see `src/session/cloud.rs:478-480`), so local processing continues correctly. The next operation that writes state will trigger a fresh `sync_push_state` that uploads the full file (including the init bundle plus whatever was just appended). The remote copy converges to the correct state on the next sync. Recoverable.

**Crash during T10 (S3 upload in progress):** S3 PUT is atomic from the client's perspective — either the old object version is visible (missing, or an older session) or the new complete version is visible. There is no partial S3 object. The local file is complete, so all future reads from local work. The next write-and-sync on this session will re-upload and catch up. Recoverable.

**Crash after T10, before T11:** Everything is durable. The process failed to print its response JSON, but the session is fully initialized. Retry of `handle_init` will error with "workflow already exists" at T3. This matches the pre-fix behavior for double-init and is the correct signal to the caller: the session is ready, there's no need to init it again. Callers that want to distinguish "first-init" from "already-initialized" can use `koto status` or check `exists()` before calling `init`. Recoverable.

**Summary of the crash matrix:**

| Crash point | State on disk | `exists()` | Retry action | Operator action |
|---|---|---|---|---|
| T1-T3 | Nothing | false | Re-run init | None |
| T4 | Empty session dir | false | Re-run init | None |
| T5-T7 | Empty session dir | false | Re-run init | None |
| T9 pre-rename | Session dir + stale `.tmp` | false | Re-run init | None (temp cleaned later) |
| T9 rename | Either pre or post state | respective | Re-run or proceed | None |
| T9 post-rename | Complete state file | true | `handle_next` runs | None |
| T10 (cloud) | Local complete, S3 partial (none) | true | Next sync catches up | None |
| T11 | Complete state file | true | Re-run errors with "exists" | None |

Every row is recoverable. No row requires operator intervention. No row leaves a workflow wedged. The critical property holds: **after any crash during `handle_init`, the next scheduler tick can make forward progress without human help.**

Contrast this with the pre-fix flow, where a crash between `append_header` and the first `append_event` leaves `exists() == true` but the event log empty, and `handle_next` errors indefinitely until an operator runs `koto session cleanup`. That wedged state is exactly what the atomic init bundle eliminates.

<!-- decision:end -->

---

## Output Contract

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Atomic init bundle via init_state_file method (tmp + rename)"
  confidence: "high"
  rationale: |
    Writing header + WorkflowInitialized + initial Transitioned to a temp file
    inside the session directory and atomically renaming it into place closes
    the entire crash window without violating append-only semantics, without
    introducing new event types, and without changing how existing backends
    handle subsequent appends. Both LocalBackend and CloudBackend are fixed
    by a single change because cloud delegates state-file writes to local and
    uploads whole files via S3 PUT. Every crash point produces a recoverable
    state with no operator intervention required.
  assumptions:
    - "rename(2) is atomic within the session directory (POSIX guarantee on the same filesystem)"
    - "No header-only state files currently exist in the wild; v0.7.0 is recent and the crash window is narrow"
    - "CloudBackend.sync_push_state uploads the whole state file as one S3 PUT, which is atomic from the reader's perspective"
    - "The initial Transitioned event can be fully constructed in memory before any write, with no I/O dependencies"
  rejected:
    - name: "Combine header + first event in one append_header call"
      reason: |
        A single write is not atomic — a crash mid-write leaves a truncated
        file that triggers read_events's recovery path and returns zero events,
        reproducing the original bug. Making it atomic requires tmp+rename
        anyway, at which point the full init-bundle approach is strictly
        better because it also covers the Transitioned event.
    - name: "koto session repair subcommand"
      reason: |
        Pushes a correctness problem onto operators. Batch spawning cannot
        depend on humans noticing wedged children. Atomicity should be fixed
        at the write site, not papered over with a cleanup pass.
    - name: "Idempotent handle_init"
      reason: |
        Requires classifying every 'already exists' case into sub-states
        (header-only, header+partial-events, complete) and adds substantial
        logic to the init path. Does not eliminate the crash window, just
        adds a second codepath that patches around it. Also cannot recover
        the original --var flags on retry.
    - name: "Accept the window and rely on read_events recovery"
      reason: |
        The window is narrow but its consequence — a wedged child workflow —
        is unbounded in severity. 'Rare but unrecoverable' is worse than
        'never' for an engine whose core promise is crash recoverability.
  report_file: "wip/design_batch-child-spawning_decision_2_report.md"
```
