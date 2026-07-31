# Workspace Layout

This document catalogs what koto writes under `~/.koto/` and the
derivability / safe-deletion semantics of each entry. Most of it is
derived: operators can prune any file in the "Derived files" section
below when troubleshooting without risk of data loss, because every
one of those entries is rebuildable from the authoritative session
state (headers + event logs).

Two trees are authoritative and rebuild from nothing:
`~/.koto/sessions/` and `~/.koto/requests/`. Deleting either destroys
history.

Cross-references: `docs/STABILITY.md` for the public crate stability
contract; `docs/designs/DESIGN-koto-request-store.md` for the full
request-store design (Consequences > Mitigations, line 2223, is the source
of authority for this document).

## Directory tree

```
~/.koto/
├── sessions/                                  # AUTHORITATIVE state
│   └── <session-id>/
│       ├── koto-<session-id>.state.jsonl      # header + event log
│       └── claim.lock                         # derived (request-store sidecar)
├── requests/                                  # AUTHORITATIVE state
│   └── <request_id>/
│       ├── request.jsonl                      # header + event log
│       └── request.lock                       # flock target for the write path
├── coordinators/                              # derived (request-store cursor state)
│   └── <coord_id>/
│       └── scan_cursor.toml
├── _terminal_index.jsonl                      # derived (request-store skip-list)
└── _terminal_index.compact.lock               # derived (request-store compaction lease)
```

Sessions under `~/.koto/sessions/` ARE the authoritative state and
must not be deleted manually except via `koto session cleanup`.
Request records under `~/.koto/requests/` are authoritative too and
have no cleanup verb at all. The four derived files below are safe to
delete.

## Authoritative state: `~/.koto/requests/`

`koto request create` mints an opaque lowercase identifier and writes
one directory per request:

```
~/.koto/requests/<request_id>/
├── request.jsonl     header line + every event for this request
└── request.lock      flock target for the validate-and-append critical section
```

`request.jsonl` is the whole record. The header line carries the
request's own `schema_version`, its id, creation timestamp,
`requested_by`, and `coordinator_of_record`; every line after it is an
event in the `request.` family — creation, leg binds, progress
appends, results, abandonments, and the close. A request's current
shape is a projection of that log, not a stored snapshot, and the
`revision` on every CLI response is the sequence number of the last
line.

Creation is a single atomic write. The header and the creation event
are buffered together, fsynced into a tempfile in the target
directory, then renamed into place with no-replace semantics — so a
crash can never leave a request whose header parses and whose log is
empty, and a colliding identifier is refused by the rename rather than
by a check that could race.

`request.lock` is a separate file from the log so acquiring the lock
never truncates or extends it. Every write takes it. That is not
politeness: the log's sequence numbers are derived from a read of the
last one, and the reader hard-errors on a gap, so two unlocked
concurrent appends computing the same next sequence would make the
request permanently unreadable. Acquisition is non-blocking plus retry
against a five-second deadline; a writer that loses the race that long
gets `lock_contention`, which is transient and retryable. Reads take
no lock at all and write nothing, so `get`, `list`, and `wait` never
contend with a writer.

**Permissions.** `requests/` and each `<request_id>/` directory are
created mode 0700, and both `request.jsonl` and `request.lock` are
opened mode 0600, rather than relying on the home directory's mode
having been set correctly once. Both paths refuse to follow a symlink
— the log through an explicit check before it is opened, the lock
through `O_NOFOLLOW` — so a planted link can't redirect an append into
a file the operator never meant to write.

### Why this outlives the sessions it references

A request names the child sessions bound to its legs, and those
sessions are deleted on their terminal tick. The request record
survives that by construction, not by every deletion site remembering
to spare it: nothing in koto walks `~/.koto/` outside `sessions/` and
`coordinators/`, so neither `koto session cleanup` nor `koto workspace
prune` touches `requests/`. A coordinator that restarts after its
children are gone still reads the full history — which legs resolved,
with what result, which were abandoned and why.

The flip side is that nothing deletes a request either. The store
grows monotonically for the life of the workspace, and `koto request
list` parses every record's header, so listing gets slower as records
accumulate. There is no prune verb for it yet. Operators who need the
space back can remove a `<request_id>/` directory by hand, accepting
that the history goes with it; do it only when no writer holds the
lock, since unlinking a lock file out from under a writer would leave
two writers locking different inodes.

### Two limitations

**Request records do not replicate under the cloud backend.** They
live on the local filesystem only. A workspace whose sessions
replicate will still have request records visible on one host.

**The store requires a local filesystem.** The write path's mutual
exclusion is flock, which is host-local. Two hosts appending to the
same record over a network filesystem would not see each other's lock,
would compute the same next sequence number, and would leave a log
whose sequence gap the reader refuses — the record becomes unreadable
rather than merely stale. Point `~/.koto/` at local storage.

Both are documented limitations rather than silent gaps.

## Derived files introduced by the request-store

### 1. `~/.koto/_terminal_index.jsonl`

The workspace-wide skip-list of terminal sessions (Issue 8). Each
line is one JSONL entry: `{session_id, terminal_at, header_mtime_ns,
terminal_state}`. The discovery scan consults the file to avoid
re-walking terminal sessions on every tick.

- **Derivability:** every entry is recoverable by walking session
  headers under `~/.koto/sessions/` and reading their `terminal_state`
  fields (or replaying the event log to derive `current_state` and
  the template's `terminal: true` flag).
- **Safe to delete:** yes. The next discovery scan rebuilds the
  in-memory dedup map; the writer re-populates the file as new
  sessions reach terminal.
- **Recovery cost:** at year-2 workspace scale (~26k sessions, ~25.9k
  terminal), the first scan after deletion walks every header on
  disk. This is the cold-cursor / full-rescan path measured by
  `benches/discovery_scan.rs` — typically ~150 ms at 26k on the
  reference hardware.

### 2. `~/.koto/coordinators/<coord_id>/scan_cursor.toml`

Per-coordinator scan cursor (Issue 7). Records the last
`(scan_time, max_header_mtime, seen_at_boundary)` triple so the next
tick can resume incremental walks instead of redoing the full
workspace.

- **Derivability:** the cursor IS derived state. A fresh-rescan
  produces a new cursor on the next tick that captures the current
  scan boundary.
- **Safe to delete:** yes. Deleting the cursor (or letting the 7-day
  TTL fire automatically) triggers a full-rescan fallback on the
  next tick.
- **Recovery cost:** ~150 ms one-time at 26k sessions on the
  reference hardware. The discipline is the same as the
  `_terminal_index.jsonl` cold-cursor path; subsequent ticks are
  back to ~30 ms steady-state.

### 3. `~/.koto/_terminal_index.compact.lock`

Single-writer lease for the terminal-index compaction routine
(Issue 9). Created via `O_CREAT | O_EXCL` so two coordinators
racing on compaction never both run the rewrite. The lock body is
TOML: `{coord_id, started_at, started_at_unix_seconds}`.

- **Derivability:** the lock IS derived state. It encodes "a
  compaction is in progress"; the underlying `_terminal_index.jsonl`
  carries the authoritative skip-list, and the `.jsonl.tmp` (if
  present alongside) is a partial rewrite.
- **Safe to delete:** yes, but with caveats. Deleting an active
  lock while a coordinator is mid-compaction lets a second
  coordinator race the rewrite; both will then overwrite each other
  via `rename(2)`. Use `koto workspace prune` to remove stale
  locks safely (the prune verb checks the `started_at` timestamp
  against the configured timeout).
- **Recovery cost:** the stale-lock recovery walk inside
  `recover_stale_compact_lock` cleans up automatically on the next
  compaction tick when the lock's `started_at` exceeds
  `request_store.compact_lock_timeout_seconds` (default 3600 s) AND the
  recorded `coord_id` is foreign. No operator action required for
  typical crashed-coordinator cases.

### 4. `~/.koto/sessions/<session-id>/claim.lock` (per-session)

Per-session O_EXCL claim sidecar (Issue 11). Created when a
coordinator picks up a request-store dispatched child; carries the
claiming `coord_id` + `claimed_at` timestamp. Unlinked when the
dispatched agent completes the child's terminal write.

- **Derivability:** the sidecar IS derived state. The header's
  `assignment_claim` field carries the authoritative claim record;
  the sidecar is the O_EXCL semaphore that prevents two coordinators
  from claiming the same child.
- **Safe to delete:** yes, but only after the dispatched agent has
  reached terminal. Deleting a sidecar while a dispatch is in flight
  allows a second coordinator to re-claim the same child. Use
  `koto workspace prune` to remove sidecars whose owning coordinator
  is older than the configured `request_store.stale_claim_timeout_seconds`
  (default 600 s).
- **Recovery cost:** the stale-claim recovery walk inside Issue 11's
  `recover_orphaned_sidecar` cleans up dead-coord sidecars on the
  next coordinator's tick. No operator action required for typical
  crashed-coordinator cases.

## Recommended prune cadence

Per the design's Consequences > Mitigations (line 2223), operators
should run `koto workspace prune` on a **weekly to monthly**
cadence. The verb is idempotent and tolerates a missing workspace.

```bash
# Manual prune (dry-run first if you want a preview)
koto workspace prune --root <session-id> --dry-run
koto workspace prune --root <session-id>

# Cron the prune to fire every Sunday at 02:00
0 2 * * 0 /usr/local/bin/koto workspace prune --root <session-id> --yes >/dev/null 2>&1
```

`koto workspace prune` reclaims:

- Stale scan cursors whose `last_scan_at` exceeds the 7-day TTL.
- Stale compaction locks whose `started_at` exceeds
  `request_store.compact_lock_timeout_seconds`.
- Stale claim sidecars whose `claimed_at` exceeds
  `request_store.stale_claim_timeout_seconds`.

The verb does NOT delete session directories under
`~/.koto/sessions/`. Session cleanup is the
operator-driven `koto session cleanup <session-id>` path.

### Sizing your prune cadence

Concrete per-session on-disk costs (measurements taken on the
reference Linux/ext4 setup, average-case workloads):

- **State-file header line:** ~500 bytes (varies with `requested_by`,
  `coordinator_of_record`, `assignment_claim` populated).
- **Event log:** typically 5-50 KB per session depending on workflow
  depth. A 5-state linear workflow with one `evidence_submitted` per
  state lands around 5 KB; a /shirabe:design workflow with parallel
  dispatch and per-state confirmations lands around 30-50 KB.
- **Claim sidecar (`claim.lock`):** ~150 bytes (request-store dispatched
  children only; plain children carry no sidecar).
- **Per-coordinator scan cursor (`scan_cursor.toml`):** ~200 bytes.

We use a 10 KB per-session estimate below as the typical median when
mixing /work-on, /design, and /decision workflows.

**Worked example — 100 workflows/day at typical depth:**

- 100 × 10 KB = ~1 MB/day = ~7 MB/week = ~30 MB/month.
- Weekly prune cadence keeps the workspace under ~7 MB steady-state.
- Monthly prune cadence keeps it under ~30 MB.

**Worked example — 1000 workflows/day at typical depth:**

- 1000 × 10 KB = ~10 MB/day = ~70 MB/week = ~300 MB/month.
- Weekly prune cadence keeps the workspace under ~70 MB steady-state.
- Daily prune cadence keeps it under ~10 MB.

**Recommended cadence by workload tier:**

| Workload | Typical workflows/day | Recommended cadence |
|----------|----------------------|---------------------|
| Low (solo, occasional) | 1-10 | Monthly |
| Medium (active solo / small team) | 10-200 | Weekly |
| High (team or CI-driven) | 200-2000 | Daily |
| Very high (LLM agents at scale) | > 2000 | Twice-daily or hourly |

The cron snippet above runs once a week. For higher cadences, switch
the cron expression to `0 2 * * *` (daily) or `0 */6 * * *`
(every 6 hours).

### Sizing the stale-claim timeout

The `request_store.stale_claim_timeout_seconds` config dimension
(default 600s / 10 min) controls when Issue 11's recovery walk
treats a still-held claim as stale and unlinks the sidecar so a
fresh coordinator can re-claim. Set this too low and you'll
false-positive on legitimate long-running work; set it too high and
a crashed coordinator's sidecar lingers, blocking the affected
children indefinitely.

**Typical claim-to-terminal durations by workload type:**

- **Short tool runs (lints, formatters, status checks):** 30-60s.
- **Medium reviews (/work-on, /design subroutines):** 2-5 min.
- **Long /decision phases (multi-agent bake-offs, peer review):**
  5-30 min.
- **Respawn-heavy workloads (LLM-driven loops with retries):**
  can legitimately exceed 30 min.

**Cost of a false-redelegation** (timeout fires while the original
agent is still working): both the original AND the re-dispatched
agent burn LLM tokens; the audit log records spurious
`ChildRedelegated` events; idempotency-hash collisions can surface
on the next event-write attempt; in extreme cases the original's
terminal write races the re-dispatch's claim. None corrupt state —
the request-store contract handles the race — but the operational
noise is high.

**Recommendation:** set the timeout to your **typical claim duration
plus a 3-5x safety multiplier**.

- Medium-depth review workloads → keep the default 600s (10 min).
- Long-decision workloads → bump to **3600s (60 min) or higher**.
- Respawn-heavy LLM workloads → bump to **7200s+ (2 hours)** and
  rely on the substrate's own crash-detection rather than the
  request-store timeout.

```bash
# Set the timeout to 1 hour for long-decision workloads
koto config set request_store.stale_claim_timeout_seconds 3600 --user
```

## When to delete manually

The supported flow is `koto workspace prune`. Manual deletion is a
diagnostic shortcut for an operator investigating an unusual state —
e.g., a coordinator stuck behind a stale lock that the prune verb
should but hasn't cleared. The four derivability rules above keep
manual deletion safe: every file rebuilds on the next tick.

The exceptions are `~/.koto/sessions/<session-id>/` and
`~/.koto/requests/<request_id>/`: those directories are NOT derived
and contain the authoritative state. Deleting either permanently
destroys the history it holds.

## Cross-references

- `docs/STABILITY.md` — public crate surface lockdown (Issue 19,
  Decision 5).
- `docs/designs/DESIGN-koto-request-store.md` — full request-store design.
  Consequences > Mitigations (line 2223) is the source of
  authority for this document.
- `docs/designs/current/DESIGN-request-lifecycle.md` — Decision 1, the source
  of authority for the `~/.koto/requests/` layout.
- `docs/reference/error-codes.md` — the `koto request` code set,
  including the bounds and their config keys.
- `koto workspace prune --help` — the operator-driven cleanup verb.
