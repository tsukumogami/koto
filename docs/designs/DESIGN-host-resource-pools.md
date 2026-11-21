---
schema: design/v1
status: Planned
problem: |
  koto runs every action inside one tick under a hard-coded 30-second limit
  and has no notion of a resource shared across sessions, so heavy steps from
  many sessions on one machine all start at once, and long steps can only run
  through hand-written detached supervisors that no lock taken in the action
  can cover.
decision: |
  Pools are `[pools.<name>]` tables in the user's koto config, parsed only
  where they are used. Each pool is a set of flock slot files plus one queue
  file under `<koto root>/pools/<name>/`. A pool guard runs in the advance
  loop before a pooled action: it waits up to `wait_secs` for a slot in queue
  order, else returns a temporal `__pool__` condition. Background actions run
  through a detached `koto __run-action` runner that holds the slot and a
  liveness lock for the command's life and writes a result the next tick
  ingests.
rationale: |
  Kernel-released flocks are the only portable lock that frees on `kill -9`
  without stale-file rules, and koto already has the code for them. Reusing
  `gate_blocked` with reserved conditions keeps every agent's routing working.
  A runner that lives exactly as long as the command is the simplest holder
  for a slot that must outlive its tick, and keeping the session log
  single-writer leaves rebind fences and cloud batching untouched.
upstream: docs/prds/PRD-host-resource-pools.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: host resource pools

## Status

Planned

## Context and Problem Statement

The PRD (docs/prds/PRD-host-resource-pools.md) sets 28 requirements. This
design answers how koto meets them. Four facts about today's engine shape the
answer:

- Actions and gates run inside one `koto next` (`src/engine/advance.rs`, the
  action step near line 1441), and every action command runs through
  `run_shell_command` (`src/action.rs`) with `DEFAULT_TIMEOUT_SECS = 30`; the
  action call sites pass no other value. The function returns only once the
  command's process group is gone.
- A state's action re-runs on every tick in the state until evidence is
  submitted, so anything that starts long work must be idempotent per visit.
- The only state carried between ticks is the session log, written by one
  writer at a time; cloud sessions batch those writes and fence rebinds on
  them (`src/session/cloud.rs`). Every command a tick runs carries
  `KOTO_TICK_SESSION`, and a nested `koto next` refuses with
  `NestedInvocation` (`src/engine/reentrancy.rs`).
- Everything koto keeps for itself lives under the koto root (`$KOTO_HOME`,
  else `~/.koto`, resolved in `src/koto_home.rs`), including the user config
  `config.toml`; a trusted project config may set only keys on
  `PROJECT_ALLOWLIST` (`src/config/validate.rs`).

So a pool needs three parts: a host-wide slot mechanism with crash release, a
check in the tick that queues without failing, and a way for long work to hold
a slot after its tick has returned.

## Decision Drivers

- Slots must free on any holder death, including `kill -9`, on Linux and macOS
  (R10, R27).
- Zero config: an undefined pool, or no `[pools]` table, must leave every tick
  byte-for-byte as today (R6, R3).
- One bad config value must not break unrelated koto commands (R7).
- Agents must see the existing `koto next` response shapes, with no new
  `action` variant (R15, R20).
- The session log keeps one writer; nothing outside a tick of that session
  appends to it (cloud batching, rebind fences).
- koto must never signal a process it can't prove belongs to the run, and a
  slot must not free while any process of its run still runs.
- Older koto must ignore the new template fields (R4).
- A tick on a state with no pool, or an undefined one, costs under 5 ms more
  (R28).

## Considered Options

All four questions were resolved inline under `/scope`, from the research
done during exploration and the PRD's discovery, then revised after the
design's security and architecture review.

### Decision 1: how slots, the queue and holders are kept

Resolved inline. **Chosen: flock slot files and one queue file per pool,
under the koto root.** Each `slot-<i>.lock` is held with
`flock(LOCK_EX|LOCK_NB)`, and its holder writes its description into the same
file. One `queue.json`, replaced by atomic rename under a short `queue.lock`,
holds the waiters in order and the current claim. The kernel drops a flock
when its last descriptor closes, so no stale-lock rule exists, and koto
already has bounded `flock` waits and symlink-safe opens in
`src/engine/jsonl_append.rs` and the request store.

Alternatives considered:
- **POSIX named semaphores.** Rejected: a holder that dies never returns its
  count.
- **SysV semaphores with `SEM_UNDO`.** Rejected: split by IPC namespaces in
  containers and awkward on macOS.
- **Lock directories with pid files.** Rejected: needs stale-pid guessing and
  breaks on pid reuse.
- **The make jobserver FIFO.** Rejected: tokens leak when a holder dies, the
  failure Gentoo's host-wide jobserver work also ran into.
- **A coordinator daemon on a Unix socket.** Rejected for a first version: a
  second long-lived process to start, supervise and upgrade.
- **One file per waiter plus a separate claim file.** Rejected after review:
  two structures to keep consistent where one atomic file does.

### Decision 2: how a background action outlives its tick

Resolved inline. **Chosen: a detached `koto __run-action` runner.** The tick
takes the slot and the run's locks, writes `run.json`, and spawns the runner,
which passes the slot and `run.lock` on to the command, so the slot frees only
when every process of the run has gone. The runner holds `runner.lock` for its
life, runs the command in a new process group, kills the command's descendants
at the end or the timeout, and writes `result.json`. The next tick ingests the
result into the session log.

Alternatives considered:
- **A slot leased until the session leaves the state.** Rejected: no process
  lives long enough to hold it, an idle session holds it for nothing, and an
  abandoned one leaks it.
- **A separate keeper process beside the runner.** Rejected: two processes to
  coordinate when the runner already lives exactly as long as the command.
- **Running the command inside the tick with a long timeout.** Rejected: the
  agent's tool call would block for the suite's length.
- **The runner appending its result to the session log.** Rejected: breaks the
  single-writer rule that cloud batching and rebind fences depend on.

### Decision 3: where the tick takes a slot and how it reports waiting

Resolved inline. **Chosen: a pool guard in the advance loop's action step,
before the action and before any gate.** It returns run-as-today, acquired,
inherited, queued or invalid; queued and running stop the loop with reserved
`__pool__` and `__action__` conditions on `gate_blocked`, temporal and
non-actionable, with `retry_after_secs`, the contract poll gates already use.

Alternatives considered:
- **An implicit gate.** Rejected: gates run after the action, and a gate whose
  wait times out fails the run.
- **Blocking the tick until a slot frees.** Rejected: an agent's tool call
  would hang for minutes.
- **A new `koto next` action variant.** Rejected: every consumer routes on the
  existing set.

### Decision 4: config and template surface

Resolved inline. **Chosen: `[pools.<name>]` kept as written in `KotoConfig`
and parsed only by the guard and `koto pool status`; three optional
`default_action` fields serialized only when set.** This mirrors how retention
values avoid failing the config load.

Alternatives considered:
- **Typed pool fields in `KotoConfig`.** Rejected: a bad value would fail every
  koto command on the machine.
- **A separate `pools.toml`.** Rejected: a second file to discover and document.
- **`pool` on the state or on a gate.** Rejected: older koto would refuse to
  compile such templates; unknown keys under `default_action` are already
  ignored.

## Decision Outcome

A template names a pool on a state's `default_action`. The operator sizes it in
`<koto root>/config.toml`. When a tick reaches the action, the pool guard reads
the config: no pool, or an undefined one, and the action runs as today. For a
defined pool the guard enters the queue, waits up to `wait_secs` for its turn
and a free slot, and either takes one or returns the queued condition, leaving
earlier transitions recorded and the gates unrun. The slot follows the work:
every process of the run, from the koto process or runner down to the
command's descendants, holds the slot's descriptor, so the slot frees only
when all of them have exited, and a crash needs no cleanup. A background action
hands its slot to a runner that supervises the command, kills the command's
descendants when it ends or times out, and writes a result the next tick reads.
koto signals only a runner whose pid and start time it has checked, never a
recorded process group, so no reused pid can direct a signal at an unrelated
process.

This tightens one PRD rule after the design's security review: when a runner
dies and its command survives, the slot stays held until the survivors exit,
instead of freeing at once while they run (PRD R10 and R22, amended to match).

## Solution Architecture

### Components

- **`src/pool/config.rs`.** Parses one `[pools.<name>]` value into
  `PoolSpec { size, claim_secs, lapse_secs, wait_secs }` with the PRD's bounds
  (size 1..=1024, claim and lapse 1..=86400, wait 0..=60; defaults 120, 600,
  10), returning `PoolLookup::{Undefined, Defined(PoolSpec), Invalid{key, reason}}`.
  It also reads `[background]`: `enabled` (default true) and
  `max_timeout_secs` (default 7200, at most 86400). `KotoConfig` stores both
  tables as raw `toml::Value`s; `pools` and `background` join `KNOWN_KEYS` as
  namespaces, so `koto config list` names them among a project file's dropped
  keys (R5).
- **`src/pool/store.rs`.** The on-disk pool: safe directory setup, slot
  try-lock and probe, holder records in slot files, `queue.json` under
  `queue.lock`, the claim, lapse sweeping, and a `Slot` handle.
- **`src/pool/guard.rs`.** The tick-side guard: resolves the pool, honours the
  nesting marker, joins or refreshes the queue, waits, acquires, and builds the
  queued report. Exposed to the advance loop as a closure so engine tests can
  fake it. On a platform other than Linux and macOS it returns
  `pool_unsupported_platform` for any defined pool (R27).
- **`src/engine/background.rs`.** Run records under
  `<session dir>/runs/<run_id>/`, run identity per visit, starting the runner,
  reading a run's state, cancelling, pruning (the five most recent runs per
  session are kept; all go with the session), and the move checks.
- **`src/cli/run_action.rs`.** The hidden `koto __run-action` subcommand: the
  runner.
- **`src/cli/pool.rs`.** `koto pool status [--json]`.
- **Template types and compile (`src/template/types.rs`, `compile.rs`).**
  `ActionDecl` gains `pool`, `background` and `timeout_secs`, each with
  `skip_serializing_if`; compile checks per R2, plus `background` refused with
  `polling` or `requires_confirmation`, and `background: true` refused on a
  platform other than Linux and macOS (R27).

### On-disk layout

```
<koto root>/pools/<name>/
  slot-<i>.lock      flock target; while held, its content is the holder's
                     JSON (session, workspace, state, pid, pid_start, run_id,
                     since), at most 4 KiB
  queue.lock         short flock around queue edits, waited for at most 2 s
  queue.json         {"next_ticket", "entries": [{ticket, session, workspace,
                     joined_at, last_seen}], "claim": {ticket, deadline} | null},
                     at most 256 KiB, replaced by write-to-temp then rename
<session dir>/runs/<run_id>/
  run.json           state, visit index, the command with credentials redacted,
                     effective timeout, pool, slot, machine, boot id,
                     pid-namespace id, runner pid and start time
  runner.lock        held by the runner only, close-on-exec
  run.lock           held by every process of the run (inherited)
  result.json        exit status, failure kind, duration, truncated flag
  stdout, stderr     the command's output, at most 64 KiB each, redacted
```

Directories are created 0700 and files 0600, with `O_EXCL` for new records.
Every path below the koto root is opened component by component with `openat`,
`O_NOFOLLOW` and `O_DIRECTORY`; an existing `pools/` or `runs/` directory not
owned by the current user, or writable by group or others, or on a network
filesystem, is refused as `pool_dir_unavailable` (R14). Pool names (R1) and
run ids (`^[a-z0-9-]{1,64}$`) are validated before they reach a path, including
values read back from the session log. `runs/` is host-local: cloud sync pushes
the session log and context, never `runs/`.

### Taking a slot: the queue and the claim

All queue decisions happen under `queue.lock`, waited for at most 2 seconds (a
longer wait counts as "not yet" and is retried at the next poll). Session-log
events are appended after `queue.lock` is released.

1. Drop entries whose `last_seen` is older than `lapse_secs` (R12). The sweeper
   can't write another session's log, so a lapsed session appends its own
   `pool_released` with reason `lapsed` on its next tick, when it finds its
   ticket gone, then queues again at the back.
2. Find or create this session's entry (one per session per pool) and refresh
   `last_seen`; a new entry means `pool_queued` is appended (R26).
3. Count held slots by probing every `slot-*.lock` file, of any index, with a
   non-blocking try on a fresh descriptor, released at once. If held is at
   least the current `size`, nobody is admitted, whatever indexes are in use
   (R8). A probe that collides with another process's probe reads as held, so
   the worst case is one extra 250 ms poll.
4. Claims (R11). Any queue edit that finds a free slot, entries waiting and no
   claim creates `claim = {first entry, now + claim_secs}`, so a claim starts
   whether the previous holder exited cleanly, was killed or was a runner that
   only wrote its result. When a claim's deadline passes unused, the next edit
   moves it to the following entry with a fresh deadline; the skipped entry
   keeps its place at the front for the next free slot. A session may take a
   free slot when it holds the claim.
5. On success, lock a free slot file, write the holder record into it, remove
   the entry, clear the claim, and, after releasing `queue.lock`, append
   `pool_acquired`.

Outside the lock the guard sleeps 250 ms between attempts for up to
`wait_secs` in total (R15); time spent there counts as ticking.

### Tick flow for a pooled action

1. The advance loop reaches a state with `default_action` and no evidence.
2. If this visit's latest `background_started` event names a run, read it (see
   "Reading a run"). Running or orphaned returns the matching `__action__`
   condition without touching the pool; finished ingests the result (step 6);
   lost appends `pool_released` with `lost` and the next tick starts a new run
   from step 3.
3. The guard looks up the pool. Undefined, or no `pool` field: no slot is taken
   and the action runs as today; a `background` action still runs through the
   runner, without a slot. Invalid: stop with `pool_config_invalid`. A nesting
   marker that verifies (see "Nesting"): run on the outer slot. A pool removed
   from config while this session waited: append `pool_released` with
   `pool_removed` and run unthrottled.
4. Queue and acquire as above, or return the `__pool__` condition.
5. Ordinary action: run it with the slot descriptor and `KOTO_POOL_HELD`
   inherited by the command; on return append `pool_released` with `done` or
   `failed`, or `timed_out` when cut at the 30-second limit. Background action:
   if `[background] enabled` is false, stop with `background_disabled`.
   Otherwise lock `runner.lock` and `run.lock` in a new run directory, write
   `run.json`, spawn the runner passing the slot, `runner.lock` and `run.lock`
   descriptors, append `background_started` (carrying `run_id`, machine and
   boot id), and return the `running` condition within 2 seconds of taking the
   slot (R19).
6. A later tick finds `result.json`: append `background_finished` and the
   action record, append `pool_released` with `done`, `failed` or `timed_out`
   once `run.lock` is free, deliver `capture_stdout_as` on exit 0, and continue
   to the state's gates; a failure builds the `__action__` condition with its
   `failure_kind`.

Leaving the state by transition, rewind or completion removes the session's
queue entry and appends `pool_released` with `left_state`; a live background
run is cancelled first, as below.

### The runner

`koto __run-action <session> <run_id>` refuses to start, writing nothing and
exiting 2, unless it received the descriptors it expects and `run.json`
matches its arguments. It starts in a new session (setsid) with stdin from
`/dev/null`, and on Linux sets `PR_SET_CHILD_SUBREAPER`, so descendants that
leave the command's process group still reparent to it. It keeps
`runner.lock` close-on-exec; the slot descriptor and `run.lock` are inherited
by the command and its descendants. It builds the command environment a tick
builds, with credential redaction from the same sources, replaces
`KOTO_TICK_SESSION` with `KOTO_BACKGROUND_OF=<session>`, and sets the nesting
marker. It runs the command in a new process group and drains stdout and stderr
through the redactor, which keeps lookahead across read chunks and the 64 KiB
bound as ordinary actions do.

The effective timeout is the template's `timeout_secs`, cut to
`[background] max_timeout_secs` when larger (reported as
`timeout_capped: true` on the `running` condition). At the timeout, on
SIGTERM from `koto cancel`, or when the command exits, the runner sends TERM to
the command's group and to every remaining descendant (on Linux its subreaped
children; on macOS the group), then KILL after 2 seconds. It writes
`result.json` by temp-and-rename, exits, and so frees `runner.lock`. The slot
and `run.lock` free when the last process of the run has gone.

### Reading a run

The tick reads a run by probing its locks:

| Run directory | runner.lock | result.json | run.lock | Reads as |
|---|---|---|---|---|
| absent | | | | lost (another machine's run, or pruned) |
| present | held | any | any | running |
| present | free | present | free | finished |
| present | free | present | held | finished, slot still held by survivors |
| present | free | absent | held | orphaned |
| present | free | absent | free | lost |

A `background_started` event whose machine, boot id or pid-namespace id differ
from this host's reads as lost and is never acted on locally (R24). Orphaned
means the runner died while processes of the run survive: the tick returns an
`__action__` condition with status `orphaned`, temporal and non-actionable,
naming the runner pid it recorded; no new run starts and the slot stays held
until the survivors exit, after which the run reads as lost and the next tick
starts again. On Linux, `koto pool status` lists the pids holding `run.lock` and
the slot, read in-process from `/proc/locks`; macOS has no such table, so it
names the recorded runner and says survivors may remain.

koto itself signals only a live runner, after matching its recorded pid and
start time: `koto cancel` and session removal send it TERM, wait up to 5
seconds, and append `pool_released` with `cancelled` (R23). Survivors of an
orphaned run are shown, never signalled. Relocate, rebind and cross-host import
refuse with `background_run_in_flight` while `runner.lock` or `run.lock` is
held (R24); an orphaned run can be released for a move only with an explicit
`--abandon-run`, which records the run as lost and leaves the survivors to the
operator.

### Nesting

`KOTO_POOL_HELD` carries the pool name, the slot index and the holder's
`run_id` (or tick id for an ordinary action), separated by `/`, and
`KOTO_POOL_ROOT` carries the koto root on its own, so a root path with any
character is unambiguous. A nested pooled action verifies the marker by reading
the named slot's holder record: the slot must be held and its record must name
the same `run_id` and koto root. Then it runs on the outer slot without a
second `pool_acquired` (R13). A marker that doesn't verify (another pool or
root, a free slot, a different holder) is ignored, so a stale marker left in a
daemon's environment grants nothing once that run has ended. A nested
background action never runs on an inherited slot: it queues as its own
session, since its runner would outlive the slot it borrowed.

A background command doesn't carry `KOTO_TICK_SESSION`, so it may run `koto
next` on another session, as a person in a shell could. `koto next` refuses
with `NestedInvocation` when its target is the session in `KOTO_BACKGROUND_OF`.
That marker stops accidental same-session ticks; it isn't a guarantee against
a same-user process that clears it, whose writes the session log's append lock
still serializes. Ordinary actions keep `KOTO_TICK_SESSION`, so nested ticks
from them stay refused as today.

### Interfaces

- Template: `default_action: { command, pool?: <name>, background?: bool,
  timeout_secs?: 1..86400, capture_stdout_as? }`.
- Config: `[pools.<name>] size = N` with optional `claim_secs`, `lapse_secs`,
  `wait_secs`; `[background] enabled`, `max_timeout_secs`; `koto config set
  pools.<name>.size N` (user only; `--project` refuses).
- CLI: `koto pool status [--json]`. It takes no session lock and no
  `queue.lock`; it reads `queue.json` and probes slot files with non-blocking
  tries it releases at once, and escapes control characters in every value it
  prints.
- Queued report holders: session name, workspace directory name and state,
  each with control characters removed and cut to 128 characters, because the
  report reaches other projects' agents.
- Environment: `KOTO_POOL_HELD`, `KOTO_POOL_ROOT`, `KOTO_BACKGROUND_OF`.
- Events: `pool_queued`, `pool_acquired`, `pool_released{reason}` (reasons
  `done`, `failed`, `timed_out`, `lost`, `cancelled`, `lapsed`, `left_state`,
  `pool_removed`), `background_started{run_id, machine, boot_id}`,
  `background_finished{run_id}`; older koto reads them as unknown events.
- Error codes: `pool_config_invalid`, `pool_dir_unavailable`,
  `background_run_in_flight`, `background_disabled`,
  `pool_unsupported_platform`.

## Implementation Approach

1. **Template and config surface.** The three `ActionDecl` fields with compile
   checks; `[pools]` and `[background]` raw storage and parsing, `koto config`
   support and project-config refusal. No runtime behaviour yet; byte-identical
   compile of existing templates is the test.
2. **Pool store and `koto pool status`.** `src/pool/store.rs` with unit and
   multi-process tests (crash release, the claim after clean and killed
   releases, lapse, shrink, a 20-process race for the last slot, an unsafe or
   unwritable directory), and the status command the later phases' tests read.
3. **Pool guard for ordinary actions.** The advance-loop hook, in-tick wait, the
   `__pool__` condition, the nesting marker and its verification, the
   `left_state`, `lapsed` and `pool_removed` releases, pool events, and the
   zero-config and chained-tick tests. Leaving the queue on cancel and removal
   lands here, since it needs only the queue.
4. **Background runner.** `koto __run-action`, run records and locks, the
   subreaper and descendant kill, running, finished, timed-out, orphaned and
   lost outcomes, `capture_stdout_as` delivery, re-entry and retry, output
   bounds and redaction, the timeout cap and the `enabled` switch,
   `KOTO_BACKGROUND_OF`.
5. **Cancel, removal and moves for runs.** `koto cancel` and removal stopping a
   live runner; relocate, rebind and cross-host import refusing while a run is
   in flight, with `--abandon-run` for an orphaned run; a foreign machine's run
   reading as lost; run pruning.
6. **Cost and docs.** The cost benchmark, the authoring guide for pools and
   background actions, and the config-precedence correction in
   `docs/guides/cli-usage.md`.

## Security Considerations

- **External inputs.** Nothing is downloaded. The inputs are the template (pool
  name, `background`, `timeout_secs`), the user config, files under the koto
  root and values read back from the session log, which cloud sync can carry
  from another machine. Pool names and run ids are validated before they reach
  a path, and records have size caps (4 KiB holder records, 256 KiB queue).
- **Who can affect a pool.** The pools and runs directories are 0700 and their
  files 0600 under the user's koto root, opened with `openat` and `O_NOFOLLOW`
  at every component, with owner and mode checked on existing directories. Only
  the user's own processes can take slots, read holder records or edit the
  queue. A pool is a scheduling aid between one user's sessions, not an
  isolation boundary.
- **What a template can and can't do.** A template can already run any command
  as an ordinary action. `background` lets it keep one running past its tick,
  bounded by the operator's `max_timeout_secs` and switchable off with
  `[background] enabled = false`; on Linux the runner reaps and kills
  descendants that leave the command's group. A template can name a pool, but
  only the operator sizes it; `pools.*` and `[background]` are not on the
  project allowlist.
- **Signals.** koto signals only a live runner whose pid and start time match
  its record, on the machine, boot and pid namespace the run started in. It
  never signals a recorded process group, so a reused pid or a foreign record
  can't direct a signal at an unrelated process. The runner signals only its
  own command group and its own reaped descendants.
- **Descriptors.** `runner.lock` is close-on-exec. The slot and `run.lock`
  reach every process of the run on purpose, so the slot can't free while any
  of them runs. A daemon a command leaves behind can therefore hold a slot; on
  Linux the subreaper kills such descendants when the run ends, and on both
  platforms `koto pool status` shows the slot as held by the run.
- **The session log keeps one writer.** The runner never writes it; a
  background command can't tick its parent session by accident, and the log's
  append lock serializes a deliberate same-user attempt.
- **Data exposure.** Background output and the command in `run.json` are
  redacted with the same credential sources a tick uses and stored 0600 in the
  session directory, on the host only; the five most recent runs per session
  are kept and all go with the session. The queued report shows other sessions'
  names, workspace names and states, sanitized and truncated, to any agent on
  the machine waiting for the same pool; `koto pool status` escapes control
  characters.
- **Accepted risks.** The nesting marker can be copied by a same-user process
  while its run is alive and grants a slot only for that run's lifetime. A
  same-user process can forge a `result.json`, a holder record or a queue
  entry. A stopped or hung koto process keeps its ordinary action's slot until
  it exits. All of these bend scheduling between one user's sessions and reach
  no other user's data.
- **Denial of service.** A holder that never finishes holds its slot until its
  capped timeout (background), its exit (ordinary), or an operator ends it.
  `koto pool status` names it, and `koto cancel` stops a live runner.

## Consequences

### Positive

- Heavy steps queue across every session and workspace on a machine without
  anyone serializing koto by hand.
- Long steps no longer need hand-written supervisors to get past the 30-second
  limit, and their slots cover their real length, including any stragglers.
- Machines and templates that don't use pools see no change.

### Negative

- koto gains a second long-lived process kind (the runner) and on-disk run
  records to keep consistent with the session log.
- A process that escapes the runner on macOS, or a leftover daemon, holds the
  run's slot until it exits, and koto won't kill it.
- Pools don't span koto roots, so two `KOTO_HOME` values on one machine don't
  share limits.

### Mitigations

- Run records are written once per run and read only by ticks of their own
  session, so the single-writer rule stays intact.
- The Linux subreaper and the runner's descendant kill end almost all
  stragglers; `koto pool status` shows any that remain, with pids on Linux.
- The authoring guide states the per-root scope and how to share one root.
