# Decision 12: Concurrency Model Hardening

**Prefix:** `design_batch-child-spawning_decision_12`
**Complexity:** critical
**Mode:** --auto (confirmed=false, assumptions recorded)
**Scope:** koto (public, tactical)
**Round:** 1 follow-up (hardens the Concurrency Model section, extends
Decision 2 atomic init, adds ordering for CD9 retry)

<!-- decision:start id="concurrency-model-hardening" status="assumed" -->

### Decision: Concurrency Model Hardening (combined, 8 sub-questions)

## Context

Round 1 walkthrough pair 2c (`wip/walkthrough/simulation_round1_pair2c.md`)
drove the concurrency corner cases the design doc warned about but did
not close. The walkthrough produced ten findings that collectively say:
the design correctly identifies *where* the races live, then hands the
responsibility to the caller without giving the caller the instruments
needed to satisfy the contract. The symptoms (double-dispatch of
workers, silent overwrite of child state files, orphan children after
cloud-sync conflict resolution, split-brain observers, leaked tempfiles)
each have a concrete single-machine or multi-machine reproduction in the
transcript.

Round 1 synthesis clusters E and G are the scope of this decision. They
were originally nine independent gaps; they are decided as one combined
commitment because each fix on its own leaves a hole the others would
have closed. Examples:

- Renaming `scheduler.spawned` to `spawned_this_tick` without adding a
  ledger still double-dispatches workers that diff against "what did
  this tick spawn" instead of "what exists now."
- `RENAME_NOREPLACE` at the backend layer without an advisory lock on
  the parent still lets two ticks race the parent log append even when
  the child creation is atomic.
- An advisory lock without `RENAME_NOREPLACE` protects one parent's
  own ticks but does nothing for a second coordinator on another
  machine (cloud sync) or for a child-level init race from two
  parents sharing a child name on a shared filesystem.
- A `sync_status` field without reconciling child state files lets the
  observer know they are on the losing side but leaves them staring
  at diverged truth with no remediation path.

The eight sub-questions are therefore decided together. The envelope
carrying any errors this decision introduces is CD11's `NextError`
struct with `action: "error"`; every rejection below uses that shape.

### Prior commitments this decision honors

- **D2** — atomic init via `tempfile::NamedTempFile::persist`. This
  decision extends D2 (adds a kernel-level fail-if-exists check on
  Linux and a portable fallback) but does not replace the
  tempfile+rename bundle.
- **CD9** — retry CLI-interception + template-transition split. This
  decision reorders the cloud-sync-specific push within CD9's step
  sequence but does not re-litigate "intercept at CLI vs advance
  loop."
- **CD11** — `NextError { code, message, details, batch? }` envelope
  with `action: "error"`. Every concurrency-guardrail error below uses
  this envelope.

## Assumptions

- **A1.** Agents will diff `scheduler.materialized_children` between
  ticks (or use it as a ledger) to dispatch workers idempotently.
  Agents that continue to consume `scheduler.spawned` (renamed to
  `spawned_this_tick`) are signing up for per-tick observation
  semantics and accept double-dispatch risk if they fan out
  concurrent parent ticks.
- **A2.** `libc::renameat2` is available on Linux 3.15+. Koto's
  minimum supported Linux is new enough. (Release notes / CI will
  need to pin this expectation; v0.7.0 ran on 4.x distros without
  trouble.)
- **A3.** Advisory `flock` exists on all Unix targets koto supports
  (Linux, macOS, BSD) and is already used in `LocalBackend` for
  context/manifest writes (`src/session/local.rs:216-245`). Reusing
  it for a parent-lock adds no new platform dependency.
- **A4.** `CloudBackend`'s `sync_pull_state` can compute a three-way
  status ("fresh," "stale," "local_only") from the existing
  `check_sync` primitive referenced at design L2343. Surfacing it on
  responses is a projection of data `CloudBackend` already computes,
  not a new subsystem.
- **A5.** The reference template carrying CD9's `retry_failed` flow
  can tolerate one extra `Ok`-return from `handle_retry_failed` for
  the "push-parent-first failed" early-exit. CD9's step sequence
  permits inserting a guard before child Rewound writes without
  rewriting its mechanism.
- **A6.** Walkthrough prose and the koto-user skill are the two
  canonical references agents read. Softening the walkthrough
  language in one spot, plus the matching update in both skills per
  `CLAUDE.md` skill-maintenance rule, covers the surface. Agents that
  read only the source code see the invariant reflected in the
  advisory lock's error message.
- **A7.** The tempfile leak sweep is a best-effort, bounded-age
  janitor. It does not need to be crash-safe on the sweeper itself;
  worst case, a leaked tempfile survives one more tick. Since tempfiles
  are ignored by `backend.list` and `backend.exists`, they are
  functionally harmless; the sweep exists for disk-hygiene only.

## Chosen: The eight-part hardening package

The decision is one coherent commitment across eight sub-questions.
Each part cites which round-1 findings it addresses and why the
chosen option is strictly better than the alternatives in context.

---

### Q1. Ledger vs per-tick observation — Option (a): rename `spawned` → `spawned_this_tick`, add `materialized_children` as the ledger

**Chosen.** The `scheduler` output block is extended as follows. Both
fields are emitted on every non-null `scheduler` value:

```json
"scheduler": {
  "spawned_this_tick": ["coord.D"],
  "already": ["coord.A", "coord.B", "coord.C"],
  "blocked": ["coord.E"],
  "skipped": [],
  "errored": [],
  "materialized_children": [
    {"name": "coord.A", "outcome": "success",  "state": "done"},
    {"name": "coord.B", "outcome": "success",  "state": "done"},
    {"name": "coord.C", "outcome": "pending",  "state": "working"},
    {"name": "coord.D", "outcome": "pending",  "state": "working"},
    {"name": "coord.E", "outcome": "blocked",  "state": null}
  ]
}
```

**Semantics.**

- `spawned_this_tick` is the list of children whose state file this
  specific tick created via `init_state_file`. It is a per-tick
  *observation*. Two concurrent ticks can each return the same child
  in this field; callers using it for dispatch must understand that.
- `materialized_children` is the *ledger*: the complete set of
  children that exist on disk right now, with outcome and current
  state. It is identical across concurrent ticks (modulo the tick's
  own writes) because it is a pure function of
  `backend.list(parent_prefix)` + per-child state-file headers at
  tick time.
- Consumers doing idempotent worker dispatch MUST key on
  `materialized_children`, taking the set difference against their
  last-known-dispatched set. The skill and the walkthrough both say
  so explicitly.
- `blocking_conditions[0].output.children` (CD11 / D5.3 / CD9 Part 1)
  is semantically equivalent to `materialized_children` when a
  `children-complete` gate is configured; it includes gate-specific
  aggregates (`total`, `completed`, `any_failed`, etc.).
  `materialized_children` is the scheduler-side projection, exposed
  even on responses where no gate evaluated (e.g., an `initialized`
  response on the first tick after `koto init`).

**Options rejected.**

- **(b) Keep `spawned`, document per-tick-ness.** Rejected: every
  agent that reads the existing walkthrough reasonably concludes
  `spawned` is "new this moment, dispatch now." The word
  "spawned" does not intuitively mean "this tick's observation of
  spawning, possibly duplicated across ticks." The cost of renaming
  to `spawned_this_tick` is one find-and-replace + a walkthrough
  pass; the cost of preserving the misleading name is
  perpetual-documentation-warning-about-a-footgun.
- **(c) Remove `spawned` entirely; require diffing
  `materialized_children` between ticks.** Rejected: observably-new
  spawns are information, not noise. Per-tick loggers, audit
  streams, and interactive agents all benefit from knowing "this
  tick I spawned D." Forcing every consumer to cache the previous
  tick's children set to derive this is hostile. The rename plus
  parallel ledger gives both audiences what they need.

**Rationale.** Findings 2 and 7 of pair 2c directly flag the
double-dispatch hazard. CD11 already renamed `spawned` to
`spawned_this_tick` in its Q4 JSON samples (see CD11 lines 311-391);
this decision confirms the rename and adds the sibling ledger. The
invariant: the ledger is the source of truth for "what exists," the
per-tick field is the source of truth for "what this tick chose to
do."

---

### Q2. `RENAME_NOREPLACE` on Linux, `link()+unlink()` fallback elsewhere

**Chosen.** `LocalBackend::init_state_file` uses a compile-time
cfg-gated approach to close the TOCTOU at the kernel level:

```rust
// pseudocode
fn init_state_file(&self, name: &str, header: &Header, events: &[Event])
    -> Result<(), InitStateError>
{
    let final_path = self.state_path(name);
    let ctx_dir = self.state_dir();

    // Write tempfile and flush
    let mut tmp = tempfile::Builder::new()
        .suffix(".tmp")
        .tempfile_in(&ctx_dir)?;
    write_state_file_contents(&mut tmp, header, events)?;
    tmp.as_file().sync_all()?;
    let tmp_path = tmp.into_temp_path();

    // Atomic rename with fail-if-exists
    atomic_rename_noreplace(&tmp_path, &final_path)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn atomic_rename_noreplace(src: &Path, dst: &Path) -> Result<(), InitStateError> {
    use libc::{renameat2, AT_FDCWD, RENAME_NOREPLACE};
    // ... call renameat2 with RENAME_NOREPLACE
    // EEXIST -> InitStateError::Collision
}

#[cfg(not(target_os = "linux"))]
fn atomic_rename_noreplace(src: &Path, dst: &Path) -> Result<(), InitStateError> {
    // Portable fallback via link()+unlink().
    //
    // link(src, dst) fails with EEXIST if dst already exists — this is
    // the atomic fail-if-exists primitive we need. It is defined by
    // POSIX and works on macOS, *BSD, illumos.
    match std::fs::hard_link(src, dst) {
        Ok(()) => {
            // dst now has a second link; drop the tempfile.
            std::fs::remove_file(src)?;
            Ok(())
        }
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
            std::fs::remove_file(src).ok();
            Err(InitStateError::Collision)
        }
        Err(e) => Err(e.into()),
    }
}
```

On `Collision`, `init_state_file` returns an error that the scheduler
surfaces through CD11's per-task `SchedulerOutcome.errored` with
`SpawnErrorKind::Collision`. See Q3 below for how this interacts with
the advisory lock.

**Portability trade-offs (explicit).**

| Platform | Primitive | Atomicity guarantee |
|----------|-----------|---------------------|
| Linux 3.15+ | `renameat2(... RENAME_NOREPLACE)` | Single syscall, kernel-atomic |
| macOS / BSD / illumos | `link()` + `unlink(src)` | `link()` is atomic per POSIX (EEXIST on conflict); the subsequent `unlink(src)` is not atomic *with* the link, but that's only tempfile cleanup — the TOCTOU window for the child state file is already closed by `link()`. |
| Windows | Not supported (koto is Unix-only today) | N/A |

**Why not keep the race and document.** The design-as-written
(`docs/designs/DESIGN-batch-child-spawning.md` L1982-1992) argues that
adding kernel-level atomicity "papers over a caller bug rather than
surfacing it." Round-1 pair 2c Probe 1 and Probe 4 demonstrate the
counter-argument: the caller bug is surfaced *either way* (via
`expected_seq` mismatch on the parent log append), but the
`init_state_file` race silently destroys child data in a narrow
window where a worker dispatches against the overwritten child. That
silent-data-loss case is unrecoverable. `RENAME_NOREPLACE` converts
the race from "silently overwrite" to "one process wins, the other
gets EEXIST and reports cleanly through CD11." The caller bug is
still surfaced (via the advisory lock's error in Q3 or via the
per-task `Collision` spawn error here); the data corruption is
eliminated.

**Options rejected.**

- **Keep tempfile+rename, document the race.** Rejected on
  data-preservation grounds. The delta to fix it is ~50 lines of
  cfg-gated code; the cost of leaving it is silent child-state
  corruption on every concurrent tick.
- **`open(O_CREAT|O_EXCL)` + rename.** This gives atomic creation of
  the tempfile destination but not of the final rename — you still
  need to copy content and rename, reintroducing the race. It also
  loses the tempfile pattern's crash-safety (partial writes visible
  under the real name). Worse on both axes than link+unlink.
- **Lockfile only, no kernel atomicity.** Discussed in Q3: useful for
  single-machine concurrency but does not survive NFS, cloud-sync
  race windows, or a second process that bypasses the lock by
  mistake. Kernel atomicity is cheap insurance.

**Rationale.** Finding 2 of pair 2c cites this exact choice as the
robust fix. The design's rejection at L1982-1992 does not account for
per-child data loss, only for parent-log racing (which `expected_seq`
already catches). This decision disagrees with the design-as-written
on that specific cost/benefit and fixes the silent-corruption case.

---

### Q3. Advisory lockfile per parent during `handle_next` — Option (a): add lockfile, document as local mutex, not persistent state

**Chosen.** `handle_next` acquires an advisory `flock` on a
per-workflow lockfile for the duration of the `handle_next` call, but
only when the current workflow is a batch parent (one whose current
state declares `materialize_children`, or whose log contains at least
one `BatchFinalized`/`SchedulerRan` event). Non-batch workflows
retain the pure-stateless-CLI model.

**Lockfile location and naming.**

- Path: `<session_dir>/<workflow>.lock` (e.g., for workflow `coord`:
  `<session>/coord.lock`).
- Created on first `handle_next` call that detects the workflow is a
  batch parent; never deleted (the file itself is durable, zero-byte).
- The `flock` held against it is not. `flock` is released on
  process exit even if the process crashes (kernel-level advisory
  locks do not survive `close(fd)`), so the lock is not persistent
  state — it is strictly a process-lifetime mutex.

**Acquisition discipline.**

- Acquired at the start of `handle_next` for parent workflows, before
  reading the parent state file.
- Held through: read, advance-loop validation, scheduler tick, event
  appends, `sync_push_state` (cloud mode).
- Released on function exit (implicit via file-handle drop, as the
  existing `release_flock` at `src/session/local.rs:241` shows).
- Acquisition mode: **`LOCK_EX | LOCK_NB`** (non-blocking). On
  contention, return immediately with a CD11-shaped error:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "error": {
    "code": "integration_unavailable",
    "message": "another process is running a scheduler tick against 'coord'; koto next <parent> must be serialized. Retry in a moment.",
    "details": [{"field": "workflow", "reason": "concurrent_tick"}],
    "batch": {
      "kind": "concurrent_tick",
      "workflow": "coord",
      "lockfile": "<session_dir>/coord.lock"
    }
  }
}
```

The error code is `integration_unavailable` (exit 1, retryable) per
CD11's exit-code table because the condition is transient: the other
caller will release the lock when its tick finishes.

**Why non-blocking, not blocking.** Blocking would mask the caller's
coordination bug by silently serializing. Non-blocking surfaces the
bug immediately with a clear message, which is what Finding 7
("invariant held by caller with zero diagnostic") asks for.

**Is this stateless-CLI?**

The stateless-CLI principle (koto's design driver) reads:
"no persistent cursors, no daemons" — state lives in the state file
and context, not in between-invocation memory. `flock`-on-a-lockfile
is compatible on all counts:

- **No persistent cursor.** The lockfile's contents are empty; it
  does not remember anything between invocations.
- **No daemon.** The lock is held only while `handle_next` is
  running. There is no background process.
- **No between-invocation state.** If the process exits (clean or
  crash), the lock is released by the kernel. The next invocation
  starts from a clean slate.

The lockfile is a **local mutex**, not persistent state. The skill
update and the design doc both call it that explicitly. The analogy
to draw for reviewers: `flock` here is the same primitive koto
*already* uses in `LocalBackend` for `ContextStore` writes
(`src/session/local.rs:216-245`) — context writes are already
serialized via per-key and per-manifest flocks, and that has never
been considered a stateless-CLI violation.

**Scoping to batch parents only.**

- Non-batch workflows (no `materialize_children` hook ever declared,
  no multi-tick scheduler activity) are single-writer-by-convention
  and always have been. Adding a lock there is unnecessary mutual
  exclusion and adds test surface for no correctness benefit.
- Batch parents are *exactly* the workflows where Finding 5 / 7 /
  GAP-10 apply: multiple shells, multiple coordinators, worker
  sub-agents that might stray into the parent's lane.
- Detection: `handle_next` computes "is batch parent" once, during
  initial state-file read, by checking (a) current state's
  `materialize_children` presence or (b) any `BatchFinalized` or
  `SchedulerRan` event in the log. The check is O(1) after the read
  (both signals are cheap).

**Options rejected.**

- **(b) Keep caller-serializes model, add clear diagnostic on
  conflict.** The diagnostic-only option requires a detection
  mechanism that is itself racy. Without a mutex, two ticks that see
  the same `expected_seq` both proceed; the diagnostic fires only
  after one of them appends and the other's append fails. At that
  point the loser has already run the scheduler, spawned tempfiles,
  possibly overwritten a child state file (if Q2 is not also
  adopted), and is reporting misleading `scheduler.spawned_this_tick`
  output. The diagnostic is a post-hoc "sorry, please retry" rather
  than a preventative guard. Inferior to (a) on every axis.
- **(c) Add lockfile only for parent workflows with
  `materialize_children`, keep stateless for others.** This is
  essentially (a) with a narrower scope. Adopted as part of (a): the
  lock is scoped to batch parents, not every workflow. The framing
  difference between (a) and (c) is whether the "this is a local
  mutex" documentation primary-binds to the design or to the
  specific hook. The decision binds it to the parent-workflow
  predicate (batch parents lock; everything else does not).

**Interaction with Q2.** `RENAME_NOREPLACE` and the advisory lock
close overlapping but non-identical race windows:

| Hazard | Lock alone | RENAME_NOREPLACE alone | Both |
|--------|------------|-----------------------|------|
| Two `handle_next` calls on same host, same session | Prevented | Detected after child creation (EEXIST) — tick still runs the loser halfway | Prevented cleanly |
| Two hosts writing to shared NFS | `flock` over NFS is unreliable (and not by design) — may pass | Catches the duplicate init | Catches duplicate init; lock unreliable but RENAME_NOREPLACE covers |
| Two machines under cloud sync, each with local state | Each has own lock (local only) | Each creates; cloud sync will conflict and surface via Q5 `sync_status` | Each machine is consistent locally; cloud resolves per Q4 |
| Benign (rare) race between `handle_next` and a `koto status` read | Lock blocks read | No interaction | Lock must not block read — see exclusion below |

**Read path exclusion.** `koto status`, `koto query`, `koto
workflows --children` do not take the lock. They are pure reads; the
state file's append-only design and atomic-rename-per-append make
concurrent reads safe without coordination. The lock is specifically
for the write-bearing `handle_next` path.

**Rationale.** Findings 5, 7, and GAP-10 of pair 2c all converge on
this answer. CD9's "Concurrent `retry_failed` from two callers" edge
(D9 Part 4 table) cites D12 as the mechanism that makes serialization
deterministic; this decision delivers that. The stateless-CLI
argument against lockfiles reads as a hard rule but was never written
to preclude process-lifetime mutexes — it precludes *persistent*
state across invocations, which `flock`-on-a-zero-byte-file is not.

---

### Q4. `koto session resolve` reconciling child state files — Option (a): extend `session resolve` to scan and reconcile children

**Chosen.** `koto session resolve <parent>` (a) resolves the parent
log as today, (b) then iterates all children of the parent (via
`materialized_children` projection against the resolved parent state)
and checks each child state file's divergence status via
`CloudBackend::check_sync` or equivalent local/remote comparison, and
(c) surfaces per-child divergence in its report output.

**Report shape.** Non-JSON CLI output for `koto session resolve`
grows a children section:

```
Resolving coord...

Parent log:
  resolved: accept=remote (3 events behind local)
  local branch preserved at <session>/.conflict/coord.local-<ts>.log

Children:
  coord.A  in_sync        (no action)
  coord.B  in_sync        (no action)
  coord.C  diverged       local_state=done remote_state=working
                          -> accept=remote (matches parent log consensus)
  coord.D  local_only     remote has no state file for this child
                          -> retained local (see --children flag for override)
  coord.E  diverged       local_state=working remote_state=failed
                          -> requires manual resolution, use `koto session resolve coord.E`
```

**Resolution rules by divergence type.**

| Child state (local vs remote) | Action |
|-------------------------------|--------|
| In-sync | No action. |
| Local-only (remote has no file) | Prompt: accept local (child belongs to winning parent) or discard (child belongs to losing parent, now orphan). Default if `--auto` or `--accept remote`: discard local, log the orphan to `<session>/.conflict/coord.orphans-<ts>.log`. |
| Remote-only (local missing) | Always pull remote; child was spawned on the other machine and is now ours. |
| Diverged, trivially reconcilable (one side is a strict prefix of the other's event log) | Accept the longer log; it is strictly more work. Log the shorter-side events to `.conflict/coord.<child>.local-<ts>.log` for audit. |
| Diverged, non-trivially | Require explicit user action per child (`koto session resolve coord.<child>`). The parent `resolve` does not silently pick a winner for children. |

**Flag control.** `koto session resolve <parent>` accepts:

- `--children=auto` (default): apply the rules above; non-trivial
  divergence requires per-child resolve.
- `--children=skip`: reconcile only the parent log, report child
  divergence without acting (preserves today's behavior for callers
  who want to do their own child reconciliation).
- `--children=accept-remote` / `--children=accept-local`: apply the
  named side to all divergent children. Dangerous; prints a summary
  and requires `--yes` in interactive mode.

**Options rejected.**

- **(b) Add a `--children` flag; default to skip.** Rejected:
  defaulting to skip preserves the silent-divergence bug Finding 4
  flagged. Users who ran `koto session resolve coord` and returned to
  their shell reasonably assume the workflow is now consistent. It
  is not, under (b). The correct default is "reconcile what you can,
  flag what you can't."
- **(c) Leave to user, document explicitly.** Rejected: silent
  data-divergence across machines is the nastiest bug class (per
  Finding 3's severity rating). Documentation does not help; the
  user does not know to look for it until their next read shows
  impossible state.

**Why parent `resolve` pulling divergent children to match parent is
safe.** The parent log is the DAG-of-record. If the parent's
resolved log says coord.C reached `outcome=success`, and the local
child state file says `working`, the parent's view wins by
construction — any work represented by `working` was discarded when
the parent's log branch was discarded. This is the same
accept-the-resolution semantic as the parent log itself.

**Why this is not a sync daemon.** `koto session resolve` is a
user-invoked, one-shot command. It runs, reports, exits. No
persistent state between invocations beyond the conflict archive
files (which are audit logs, not cursors).

**Rationale.** Finding 4 of pair 2c explicitly asks for this.
Finding 3 is partly addressed here (divergence is now actionable)
and partly by Q5 (observers know they are divergent even before
running resolve). The default-reconcile behavior matches user
expectation: `resolve` means resolve, not "resolve parent and
silently leave children in limbo."

---

### Q5. `sync_status` / `machine_id` response fields — Option (b): add only when cloud-sync is configured (no-op when not)

**Chosen.** Top-level response fields are added conditionally:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "sync_status": "fresh",
  "machine_id": "m1-hostname-or-configured-id",
  ...
}
```

- **`sync_status`**: one of `"fresh"`, `"stale"`, `"local_only"`,
  `"diverged"`, emitted only when `CloudBackend` is active for this
  workflow. Values:
  - `fresh`: local state matches remote (most recent push was
    successful; no remote changes since last pull).
  - `stale`: remote has advanced since last local pull; local view
    may be behind. The response body reflects the *local* view.
    Caller should run `koto session pull` or will see a conflict on
    next write.
  - `local_only`: remote has no record of this workflow (or this
    machine has never pushed). Local is authoritative.
  - `diverged`: local and remote have conflicting updates; next write
    will trip `check_sync`. Caller should run `koto session resolve`.
- **`machine_id`**: the canonical machine identifier used by
  `CloudBackend` for conflict resolution (already computed today per
  design L2337-2357). Emitted whenever `sync_status` is emitted.

When `CloudBackend` is not configured (pure-local mode), neither
field appears in the response. Consumers that read by field name
with `field_or_none()` see `null`/absence on local mode and a
populated string on cloud mode. No existing consumer breaks.

**Options rejected.**

- **(a) Add `sync_status` and `machine_id` on all responses.**
  Rejected: under local mode, these values are constant
  (`"fresh"` / hostname) and give no information. Emitting them
  unconditionally is noise that every local-mode response carries
  forever. The "uniform shape" argument for unconditional emission
  is weak because the shape is already conditional on `action`
  (error responses have `error`, non-error have `null`, etc.).
  Conditional emission for conditionally-present subsystems is
  idiomatic in the CLI.
- **(c) Defer.** Rejected: Finding 3 cites silent split-brain as
  medium-severity but with nastiest-bug-class consequences.
  Surfacing status is cheap insurance. Waiting for a user to report
  a silent data-divergence bug in prod before adding the field is a
  poor quality choice on a design still in `Proposed`.

**Rationale.** Finding 3 of pair 2c is directly resolved here;
combined with Q4's `resolve --children`, the complete split-brain
story is "observer sees `sync_status: diverged` in the response →
runs `koto session resolve` → children reconcile → observer sees
`sync_status: fresh` next tick." The three parts compose.

---

### Q6. `retry_failed` ordering under cloud sync — Option (a): push-parent-first (append clearing event + sync_push) → then rewind children

**Chosen.** Under `CloudBackend`, CD9's Part 2 step sequence (a-e)
is reordered to push the parent's clearing event before writing
`Rewound` to children. Explicit sequence:

```
a'. validate retry set (unchanged from CD9)
b'. append EvidenceSubmitted { retry_failed: <payload> } to parent log locally
c'. append EvidenceSubmitted { retry_failed: null } clearing event to parent log locally
d'. sync_push_state(parent)              -- under CloudBackend only
e'. if push fails: return CD11 error, no child writes occurred; child state untouched
f'. for each child in retry closure:
     f1. append Rewound event to child state file
     f2. sync_push_state(child)          -- under CloudBackend only
g'. run advance loop; it reads retry_failed and fires template transition
```

**Why this reorders CD9's committed sequence.** CD9's canonical
order (CD9 line 193-195) is:

```
b. append EvidenceSubmitted { retry_failed: <payload> }
c. append Rewound to each child (delete+respawn synthetic)
d. advance loop fires template transition
e. append clearing event
```

Under pure-local mode, that order is crash-safe (the advance loop
resumes idempotently, re-running the transition). Under cloud sync,
step (c) can land on M2 while step (b) has already pushed from M1,
leaving M2 with `Rewound` events on children that the parent log
(post-resolution) never references. That is Finding 8's "phantom
epoch" bug.

The chosen reorder (push parent-side both events first, then do
children) preserves CD9's guarantee on local mode (the clearing
event is still appended) while adding a cloud-sync ordering rule:
the parent must be in its final local+remote consistent state before
any child is touched. If cloud push fails at step (d), nothing has
changed on children — the retry is fully rejectable, the user sees a
CD11 error and retries.

**This is not a re-opening of CD9.** CD9's "CLI-interception-vs-
advance-loop split" (the load-bearing architectural choice) is
untouched. `handle_retry_failed` still intercepts at the CLI layer,
still stages child-side effects, still defers the parent transition
to the advance loop's template matcher. What this decision does is
reorder steps *inside* CD9's `handle_retry_failed` — explicitly
permitted by CD11's "Prior constraints that CANNOT be re-opened"
section.

**Local-mode equivalence.** When `CloudBackend` is not active,
steps (d') and (f2) are no-ops; the sequence collapses to:

```
validate → append submit → append clear → advance loop → rewind children
```

which is the CD9 sequence with the clear moved up. CD9 explicitly
calls out (at its line 181-186) that "transition-first is slightly
cleaner" and swap is permissible. This decision uses that permission
to move the clear even earlier: *before* the children touch. Local
mode's crash-resume property is preserved: on any crash after (c'),
the clearing event is already in the log, so re-running
`handle_retry_failed` sees `retry_failed: null` in merged evidence
and rejects with `no_retry_in_progress`. The user retries the whole
retry submission from scratch — idempotent at the submission level.

Wait — that last point deserves scrutiny. If clearing-first means
re-running sees "no retry in progress," then a crash after (c') but
before (f1) leaves children in their pre-retry state, and the user
must resubmit the retry. That is acceptable: the retry submission is
the user's unit of action, not an internal thing. Re-submission is
cheap and idempotent at the child-outcome level.

CD9's crash-between-Rewound-and-clearing edge (CD9 line 272) becomes
inverted: crash between clearing and Rewound. The user retries. CD9
was lenient about crash recovery because its order put child writes
first; flipping the order flips the crash recovery mode from
"transparent re-apply" to "user re-submits." Both are acceptable; the
cloud-safety argument tips the choice.

**Options rejected.**

- **(b) Make the clearing event a pre-condition of child rewinds
  (scheduler checks presence of clearing event before rewinding).**
  Rejected: this inverts the "scheduler runs pure functions of
  disk + log" principle. Adding a guard condition on a specific
  event's presence-in-log is ad-hoc; the cleaner way is to enforce
  ordering at write time (Option a).
- **(c) Rely on resume idempotency, crash-is-fine.** Rejected for
  cloud sync specifically. Resume idempotency works on a single
  machine because the local log is authoritative. With two
  machines, "the local log" may be the losing branch after
  `resolve`, and the children's `Rewound` events survive the
  resolution. That is Finding 8's exact failure mode. Relying on
  "crash-is-fine" here is relying on "split-brain-is-fine," which
  is not fine.

**Rationale.** Finding 8 of pair 2c explicitly asks for this
reorder. The trade-off (user retries on mid-sequence crash) is
preferable to phantom child epochs that outlive their parent log's
retry record.

---

### Q7. Tempfile sweep in `repair_half_initialized_children` — yes, but scoped and bounded

**Chosen.** `repair_half_initialized_children` (design L2120-2125)
adds a tempfile-sweep sub-pass that runs at the start of every
scheduler tick on batch parents. Rules:

```rust
fn sweep_stale_tempfiles(session_dir: &Path, parent: &str, age_threshold: Duration) {
    // Age threshold: 60 seconds by default. Tempfiles younger than
    // this belong to an in-flight tick (possibly on another PID) and
    // must not be removed.
    for entry in read_dir(session_dir) {
        let name = entry.file_name();
        if !name.starts_with(&format!("{}.", parent)) { continue; }
        if !name.contains(".koto-") || !name.ends_with(".tmp") { continue; }
        let metadata = entry.metadata()?;
        let age = SystemTime::now().duration_since(metadata.modified()?)?;
        if age > age_threshold {
            // Best-effort remove; ignore errors.
            let _ = fs::remove_file(entry.path());
        }
    }
}
```

**Why not eliminate the need.** Q2's `RENAME_NOREPLACE` / `link()`
fallback does not eliminate all paths to leaked tempfiles. The
specific non-race paths:

- Process crash between tempfile creation and rename (pair 2c Probe
  6). Atomicity gives you "either renamed or not," not "either
  renamed or tempfile auto-deleted on crash." The tempfile leaks.
- OOM / disk-full / permission-denied after tempfile creation.
  Similar to above.
- Signal-based kill (SIGKILL) at any point in the tempfile phase.
  `tempfile::NamedTempFile` registers a cleanup drop-handler, but
  SIGKILL bypasses drop.

So even with Q2 closing the *race*, tempfile leaks remain possible
on crashes. The sweep is the janitor.

**Age threshold.** 60 seconds is chosen because a scheduler tick
that legitimately takes longer than 60 seconds is already an
operational problem. Under normal operation, a tick's tempfiles are
either persisted (renamed) or abandoned (crashed) within low
milliseconds. 60 seconds gives a comfortable buffer without letting
leaks accumulate.

**Interaction with concurrent ticks.** The Q3 lock prevents
concurrent ticks on the same parent in the same session, so the
sweep cannot race with another tick's in-flight tempfile of the
same parent. Across sessions (different parents), tempfiles are
namespaced by parent prefix, so sweeps scoped to one parent don't
touch another parent's tempfiles.

**Options rejected.**

- **Do not sweep; confirm rename races are impossible under Q2.**
  Rejected: race elimination does not eliminate crash leaks. Finding
  9 of pair 2c explicitly cites this.
- **Global tempfile sweep (any tempfile older than threshold in the
  session dir).** Rejected: overreach. The sweep is scoped to the
  specific parent being ticked. If other parents leak, their own
  ticks will sweep them.
- **Make the sweep part of `backend.cleanup` only.** Rejected:
  `backend.cleanup` is a user-invoked command (design L712-713). It
  runs rarely; leaks accumulate between invocations. The tick-time
  sweep bounds the leak duration to 60s + next-tick-latency.

**Rationale.** Finding 9 of pair 2c asks for this directly. The
sweep is cheap (`read_dir` on the session dir, which is already
traversed during child classification) and bounded (age threshold
protects in-flight tempfiles).

---

### Q8. Walkthrough language softening — explicit rewrite

**Chosen.** Two concrete edits to `wip/walkthrough/walkthrough.md`
(and the parallel doc that will eventually become
`docs/guides/batch-workflow.md` once the design ships):

**Edit 1 — "Protocol summary" paragraph (wherever it says "any
caller" or "the coordinator just starts driving the parent's
workflow name again").** Replace with:

> The coordinator drives the parent. Workers drive only their own
> children. The coordinator submits the initial task list, calls
> `koto next <parent>` periodically to let the scheduler tick,
> submits `retry_failed` on failures, and runs terminal cleanup.
> Workers each call `koto next <parent>.<child>` in their own loop,
> driving one child to terminal state. Workers never call `koto
> next <parent>` directly.
>
> The coordinator-owns-parent, workers-own-children partition is a
> *caller contract*, enforced at runtime by an advisory lock (see
> Concurrency Model). A worker that violates the contract gets a
> clear error, not silent data loss.

**Edit 2 — Any phrase suggesting "switch from driving the parent to
driving a child" (or vice versa) by calling `koto next` with a
different workflow name.** Replace with:

> Each koto workflow name is a separate lane. A worker agent
> dispatched to drive `coord.issue-1` does that and nothing else.
> The coordinator supervises `coord` and dispatches workers to
> newly-spawned children; it does not itself call `koto next
> coord.<child>`. The two roles are distinct; merging them creates a
> race hazard described in the Concurrency Model section.

**Options rejected.**

- **Preserve the current "agent simply calls `koto next` with a
  different workflow name" language.** Rejected: pair 2c Probe 5
  showed a worker reasonably reading that sentence and calling
  `koto next coord` from within its own workflow loop, triggering
  the Probe-1 race. The words matter.
- **Add the hard rule only in the Concurrency Model section, leave
  the walkthrough conversational.** Rejected: the walkthrough is
  where agents learn the mental model. A later-section hard rule
  does not repair a mental model set by earlier casual language.

**Also update the koto-user skill.** `plugins/koto-skills/skills/
koto-user/SKILL.md` gains a section (or a reference-file sentence)
that says, verbatim, "workers drive only their own children; the
coordinator drives the parent." Per `CLAUDE.md`'s "After completing
any source change in `src/` or `cmd/`, assess both skills before
closing the work" rule, this is mandatory.

**Rationale.** Finding 7 of pair 2c cites "walkthrough says 'any
caller can do it'" as a direct invitation to invariant violation.
Softening the language is free documentation work that prevents a
class of bugs even Q3's lock can't stop (because the lock fires an
error; prevention via clear documentation avoids the error entirely).

---

## Rationale (cross-cutting)

**Why the eight parts form one decision.** Each part on its own
leaves a hole:

- Q1 alone (rename + ledger) surfaces the double-dispatch hazard
  but does not prevent the underlying race.
- Q2 alone (RENAME_NOREPLACE) prevents data loss on the child
  state file but still lets two parent ticks race the log and
  double-report.
- Q3 alone (lock) prevents concurrent ticks on one host but does
  nothing across hosts and does not repair existing data corruption.
- Q4 alone (`resolve --children`) cleans up after divergence but
  gives observers no way to know they diverged before they tried
  to write.
- Q5 alone (`sync_status`) tells observers they are stale but leaves
  them with no remediation path.
- Q6 alone (push-parent-first) prevents one specific retry-cloud
  race but leaves the general concurrent-tick race in place.
- Q7 alone (tempfile sweep) is disk hygiene and fixes nothing
  correctness-wise without the other seven.
- Q8 alone (doc softening) depends on the other seven having a
  coherent story to describe.

Together they compose into: **the coordinator-per-parent model is
now safe by construction** on single machine (Q2 + Q3 + Q1 + Q7 + Q8)
**and observably-safe across machines** (Q4 + Q5 + Q6), **with
clearly-shaped errors on every guardrail** (CD11 envelope), **and
the walkthrough teaches the right mental model** (Q8).

**Why this does not re-open D2.** D2 committed the
tempfile+rename bundle. Q2 extends the bundle's final rename step
with a fail-if-exists check on Linux and a portable POSIX-`link()`
fallback. The tempfile part is unchanged. The append sequence
(header + initial events before rename) is unchanged. D2's
"whole-file atomic init" guarantee is strengthened (from "atomic
replace" to "atomic create-only"), not replaced.

**Why this does not re-open CD9.** CD9 committed
CLI-interception-vs-advance-loop as the mechanism split. Q6 reorders
the steps *inside* CD9's `handle_retry_failed`; it does not move
work between the CLI layer and the advance loop. CD9's "prior
constraints" clause in its sibling decisions' review explicitly
permits "reorder cloud-sync-specific steps inside it (e.g., which
push happens first)" — Q6 is exactly that.

**Why this upholds CD11.** Every error this decision introduces
(concurrent-tick conflict, child init collision, tempfile-stale,
session-resolve child divergence) uses CD11's
`NextError { code, message, details, batch? }` envelope with
`action: "error"`. The `batch.kind` values added by this decision
are: `concurrent_tick`, `init_state_collision`,
`session_resolve_child_divergence`. They are additive over CD11's
`InvalidBatchReason` enum; neither decision's envelope shape
changes.

## Alternatives Considered (summary)

### Q1 alternatives

- **(b) Keep `spawned`, document per-tick-ness.** Misleading name
  for a misleading field; cost of rename is trivial.
- **(c) Remove `spawned` entirely.** Strips information from
  audit/observer tooling without benefit.

### Q2 alternatives

- **Keep race, document.** Silent child-state data loss; the
  design's own rationale at L1982-1992 did not account for this cost.
- **`O_CREAT|O_EXCL` + rename.** Loses tempfile crash-safety.
- **Lockfile only (no kernel atomicity).** `flock` unreliable over
  NFS; kernel atomicity is cheap insurance.

### Q3 alternatives

- **(b) Caller-serializes with diagnostic.** Diagnostic fires after
  corruption, not before; preventative guard is strictly better.
- **(c) Lock for materialize_children-only workflows.** Folded into
  (a): the chosen design already scopes the lock to batch parents.

### Q4 alternatives

- **(b) Add `--children` flag, default skip.** Preserves silent
  divergence; wrong default.
- **(c) Leave to user.** Silent cross-machine divergence is the
  nastiest bug class; user can't know to investigate.

### Q5 alternatives

- **(a) Add fields unconditionally.** Constant values under local
  mode; noise.
- **(c) Defer.** Waiting for prod report of silent divergence is a
  poor quality choice on a still-`Proposed` design.

### Q6 alternatives

- **(b) Clearing event as pre-condition of rewinds.** Ad-hoc guard;
  write-ordering is cleaner.
- **(c) Resume idempotency, crash-is-fine.** "Crash-is-fine"
  collapses to "split-brain-is-fine" under cloud sync; rejected.

### Q7 alternatives

- **No sweep, rely on Q2.** Race elimination ≠ crash-leak
  elimination.
- **Global sweep.** Overreach; scoping to one parent is sufficient.
- **Sweep only in `backend.cleanup`.** Runs too rarely; leaks
  accumulate between invocations.

### Q8 alternatives

- **Preserve current language.** Invites Probe-5-style race
  interpretation.
- **Hard rule only in Concurrency section.** Too late; mental
  model is set by earlier prose.

## Consequences

### What becomes easier

- **Single-machine parallel batches are safe by construction.**
  Coordinator and N workers with no coordination primitives of their
  own (no IPC, no shared state) run correctly because koto enforces
  serialization at the lock layer and atomicity at the rename layer.
- **Worker double-dispatch is prevented.** The ledger field gives
  agents a single authoritative "what exists right now" set; the
  old per-tick `spawned` ambiguity is gone.
- **Cloud-sync divergence is observable.** `sync_status` tells
  observers they are stale before they write; `koto session resolve`
  cleans up children in one command.
- **Retry under cloud sync is safe.** Push-parent-first ordering
  eliminates phantom child epochs.
- **Tempfile disk hygiene is automatic.** No manual `backend.cleanup`
  is needed to keep the session dir tidy after crashes.
- **Agents learn the right mental model from docs.** Workers-own-
  children-coordinator-owns-parent is explicit everywhere an agent
  reads.

### What becomes harder

- **Platform matrix.** Linux uses `renameat2`; other Unixes use
  `link()+unlink()`. Tests must cover both paths. `#[cfg]`-gated
  code paths add review surface.
- **Cargo version constraint on Linux.** `renameat2` was added in
  kernel 3.15 (2014). Koto's release notes must pin Linux ≥ 3.15 as
  a supported-platform minimum. This is a no-op for modern distros
  but deserves the note.
- **Lock contention surfaces a new error.** Callers that genuinely
  want parallel parent ticks (against the invariant) see
  `concurrent_tick` errors instead of silently proceeding. This is
  the intended behavior; callers must either adopt the coordinator
  pattern or accept the error.
- **`koto session resolve` grows children-reconciliation logic.**
  Implementation and test cost. The `--children` flag variations
  add surface. Documentation must cover the resolution rules.
- **Response envelope grows two optional top-level fields.**
  Downstream consumers keying on `action`, `state`, `scheduler`,
  `blocking_conditions` are unaffected. Consumers that do
  `assert_eq!(response.keys().len(), K)` will need to update — but
  those consumers are already broken under CD11's `batch` field
  addition, so the regression is a no-op.
- **`SchedulerRan` event ordering under the lock is more
  deterministic.** Tests that relied on observed tick-order under
  contention need to adjust to the new lock-serialized behavior.
- **Skill updates are mandatory.** Both `koto-user` and `koto-
  author` need updates per `CLAUDE.md`. This is in-scope for the
  PR implementing D12; missing it is a CI-blocking gap.

### Implementation touch points

- `src/session/local.rs` — extend `init_state_file` implementation
  (follow the pattern at lines 189-209), add
  `atomic_rename_noreplace` helper with Linux/non-Linux cfg
  branches, add parent-workflow lock acquisition wrapper.
- `src/session/mod.rs` — add `InitStateError` enum with
  `Collision`, `Io`, `BackendUnavailable` variants; extend the
  `SessionBackend` trait if needed to expose the workflow-lock
  primitive.
- `src/session/cloud.rs` — extend `sync_push_state` call site in
  retry flow to push parent before children (Q6); add
  `sync_status` computation via `check_sync`.
- `src/cli/next_types.rs` — add `sync_status: Option<SyncStatus>`
  and `machine_id: Option<String>` to response envelope; add
  `materialized_children` field to scheduler output block; rename
  `spawned` → `spawned_this_tick` in Serialize impl (backward-compat
  alias via `#[serde(alias = "spawned")]` during migration window).
- `src/cli/mod.rs handle_next` — acquire workflow lock on batch
  parents at entry, release on exit; map `InitStateError::Collision`
  to per-task `SchedulerOutcome.errored` (CD11 envelope).
- `src/cli/retry.rs handle_retry_failed` — reorder step sequence
  per Q6.
- `src/engine/batch.rs` — add `sweep_stale_tempfiles` pre-pass to
  the scheduler tick, scoped to the current parent.
- `src/cli/session_resolve.rs` (new or extended) — add child
  reconciliation pass, `--children=auto|skip|accept-remote|accept-
  local` flag, conflict archive writes.
- `docs/designs/DESIGN-batch-child-spawning.md` — revise Concurrency
  Model section, add Q2's RENAME_NOREPLACE extension to Decision 2
  write-up, document the lock, document `materialized_children`,
  document Q6 ordering, document tempfile sweep.
- `wip/walkthrough/walkthrough.md` — apply Q8 edits.
- `plugins/koto-skills/skills/koto-user/SKILL.md` — add the
  coordinator/worker partition section, document
  `materialized_children` as the dispatch ledger, document
  `sync_status` interpretation.
- `plugins/koto-skills/skills/koto-author/SKILL.md` — add a note
  that templates declaring `materialize_children` opt their parent
  workflow into the advisory-lock path (for author situational
  awareness).

### Coordination with parallel decisions

- **CD9 (retry path):** Q6 reorders CD9's push sequence under cloud
  sync. CD9's single-machine crash-recovery is preserved; the
  change is cloud-sync-specific ordering.
- **CD11 (error envelope):** Q3 and Q4 introduce new error
  variants (`concurrent_tick`, `session_resolve_child_divergence`)
  that plug into CD11's `NextError { batch }` extension slot. No
  schema change to CD11.
- **CD13 (post-completion observability):** CD13's
  `BatchFinalized` event is one signal used by `handle_next` to
  detect "this is a batch parent" for lock-scoping (Q3). CD13's
  ledger (`batch_final_view`) complements Q1's
  `materialized_children`; they answer different questions
  (historical vs current).
- **CD14 (path resolution):** Independent. No interaction.
- **D2 (atomic init):** Q2 extends D2's tempfile+rename bundle
  with `RENAME_NOREPLACE`. D2's core guarantee is strengthened.
- **D10 (mutation semantics):** Q1's `materialized_children`
  ledger is the surface D10 reads to enforce spawn-time
  immutability; D10 keys on "does the child already exist" which
  is exactly what `materialized_children` answers.

<!-- decision:end -->

---

## YAML Summary

```yaml
decision_result:
  status: COMPLETE
  chosen: >
    Concurrency hardening is an eight-part package that composes into
    "coordinator-per-parent is safe by construction" on single machine
    and "observably-safe with reconciliation" across machines. Parts:
    (1) rename scheduler.spawned -> spawned_this_tick and add a parallel
    materialized_children ledger for idempotent dispatch; (2) use
    renameat2(RENAME_NOREPLACE) on Linux and link()+unlink() on other
    Unixes in init_state_file to close the init TOCTOU at the kernel
    level; (3) acquire a non-blocking advisory flock on
    <session>/<parent>.lock for the duration of handle_next on batch
    parents, returning a CD11-shaped concurrent_tick error on
    contention; this is a local mutex, not persistent state; (4) extend
    koto session resolve <parent> to reconcile child state files by
    default (--children=auto), with skip/accept-remote/accept-local
    overrides; (5) add sync_status and machine_id top-level response
    fields, emitted only under CloudBackend configuration; (6) reorder
    CD9's retry_failed sequence to push parent (append + sync_push
    both submit and clearing events) BEFORE writing Rewound to
    children, eliminating phantom child epochs under cloud sync; (7)
    add a per-tick tempfile sweep in repair_half_initialized_children
    for .koto-*.tmp files older than 60 seconds, scoped to the current
    parent; (8) rewrite walkthrough language to make
    "coordinator-owns-parent, workers-own-children" explicit, and
    mirror the guidance in koto-user and koto-author skills.
  confidence: high
  rationale: >
    Round-1 pair 2c produced ten concrete findings on the concurrency
    story. Each part of this package resolves one or more findings,
    and the parts compose: Q1+Q2 fix silent child overwrite and the
    double-dispatch observation hazard; Q3 prevents concurrent ticks
    with clear errors on contention while preserving the stateless-CLI
    principle (flock is process-lifetime, not persistent); Q4+Q5
    close the cloud-sync split-brain observability loop (observer sees
    divergence; resolve reconciles it); Q6 extends CD9 with cloud-safe
    ordering without re-opening CD9's architecture; Q7 handles crash
    tempfile leaks that Q2 cannot eliminate; Q8 fixes documentation
    that actively invites invariant violation. D2's atomicity commitment
    is strengthened, not replaced. CD9's CLI-interception split is
    honored, only step ordering is adjusted per CD9's own
    reorder-permission clause. CD11's NextError envelope shape is
    unchanged; new batch.kind values are additive.
  assumptions:
    - Agents will key on materialized_children for idempotent dispatch;
      spawned_this_tick is per-tick observation
    - Linux kernel 3.15+ for renameat2 (release notes will pin)
    - flock is available on all supported Unix targets (already used
      in LocalBackend for context writes)
    - CloudBackend's check_sync can compute three-way sync_status
      cheaply
    - The reference template tolerates inserting a push-failure early
      exit in handle_retry_failed before child writes
    - Walkthrough and koto-user/koto-author skill updates are in-scope
      for the same PR implementing D12
    - 60-second tempfile sweep age threshold bounds leak duration
      without touching in-flight ticks
  rejected:
    - name: Keep `spawned`, document per-tick-ness (Q1 option b)
      reason: Misleading field name; rename cost is trivial
    - name: Remove `spawned` entirely (Q1 option c)
      reason: Strips information from audit/observer tooling
    - name: Keep init_state_file race, document it (Q2)
      reason: Silent child-state data loss; design's rationale did not
        account for per-child corruption
    - name: Caller-serializes with diagnostic-only (Q3 option b)
      reason: Diagnostic fires after corruption; preventative guard is
        strictly better
    - name: Add `--children` flag to session resolve, default skip
      (Q4 option b)
      reason: Preserves silent cross-machine divergence; wrong default
    - name: Leave child reconciliation to user (Q4 option c)
      reason: Silent split-brain is the nastiest bug class; user can't
        know to investigate
    - name: Add sync_status on all responses unconditionally
      (Q5 option a)
      reason: Constant noise under local mode
    - name: Defer sync_status / machine_id (Q5 option c)
      reason: Known hazard; additive, low-cost insurance
    - name: Make clearing event a pre-condition of child rewinds
      (Q6 option b)
      reason: Ad-hoc guard; write-ordering is cleaner
    - name: Rely on resume idempotency for retry under cloud sync
      (Q6 option c)
      reason: Collapses to split-brain-is-fine under cloud sync
    - name: No tempfile sweep (Q7, relying on Q2 alone)
      reason: Race elimination does not eliminate crash-leak paths
    - name: Global tempfile sweep
      reason: Overreach; scoped sweep is sufficient
    - name: Preserve current walkthrough "any caller" language (Q8)
      reason: Invites invariant violation; Probe 5 of pair 2c
        demonstrates the misread
  report_file: wip/design_batch-child-spawning_decision_12_report.md
```
