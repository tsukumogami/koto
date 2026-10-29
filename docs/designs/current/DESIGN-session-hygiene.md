---
schema: design/v1
status: Current
problem: |
  koto's session store holds about a thousand sessions on one workstation
  and offers no way to find them by where they ran, no safe way to remove
  the finished ones as a set, no answer for a session removed at its
  terminal tick, and no bound on the run journal or on the migration
  markers imports leave in the bucket. Each gap needs a mechanism that
  reads only state koto already writes, and none may let the session store
  depend on the journal.
decision: |
  One inspection helper (header, log, cached template) backs both
  `koto session list --execution-dir/--state` and a new `koto session
  prune`, which previews finished sessions, keeps those a stated rule
  protects, and applies either a stored preview exactly or, unattended, a
  recomputed set, each removal under the session's lock after a re-check.
  koto writes a finished record before removing a session itself, and
  `koto status` answers from it. The run journal rotates on append by a
  no-replace rename to a sequence-numbered name, writers re-check identity
  after locking, and retention keeps any file a reader's recorded position
  needs. `--migrated-from` removes migration markers whose target is
  present with a log that begins with the old copy's, by age or with a
  stated retired source, marker last.
rationale: |
  Each mechanism reads what koto already records (headers, logs, context
  keys, leg pointers, file metadata) and adds one small file kind where
  nothing existed (the finished record, the reader position). Keeping
  finished sessions or reading the journal back would have answered the
  status gap more cheaply, but the first changes what every terminal tick
  does and the second makes the store depend on a derived file. Rename-based
  rotation with a published naming rule and optional reader positions is the
  only scheme in which an external reader can keep every line without koto
  ever parsing a record; it ships off by default until consumers list
  rotated files.
upstream: docs/prds/PRD-session-hygiene.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: session-hygiene

## Status

Current

## Context and Problem Statement

A koto session is a directory under the store's base (`$HOME/.koto/sessions`, or
the base `KOTO_SESSIONS_BASE` names) holding `koto-<name>.state.jsonl`: a
header line, then the event log. The header records `execution_dir`, the
canonical directory the session's ticks run in, along with `session_id`,
`parent_workflow` and the request-store fields. Whether a session is finished
isn't in the header. It takes the log (the current state and any
`workflow_cancelled`) and the compiled template (whether that state is
`terminal`, and whether it is a `failure` terminal).

Four parts of koto touch this store today, and none of them answers the
questions in `docs/prds/PRD-session-hygiene.md`:

- `koto session list` (`src/cli/session.rs`, `handle_list`) prints one row
  per session from the header alone: name, `created_at`, `template_hash`,
  `parent_workflow`, `template_source_status`. It doesn't print the execution
  directory, can't filter on it, and doesn't say whether a session is finished
  (R1 to R4).
- `koto session cleanup <name>` removes one directory, plus the session's
  remote objects under the cloud backend. `koto workspace prune --root <r>`
  removes one finished root's descendants whatever their state. Neither
  removes finished sessions as a set, and neither applies the protections in
  R8. The only code that already applies some of them is the parent's sweep
  (`src/cli/workspace.rs`, `sweep_terminal_descendants` and
  `awaits_leg_promotion`), which keeps running descendants, unreadable ones and
  ones whose request leg waits for its result.
- `finish_terminal_tick` (`src/cli/mod.rs`) removes a session that reaches a
  terminal koto doesn't keep (not a failure terminal, no `--no-cleanup`),
  after writing the run journal's `terminal` record. From then on
  `handle_status` finds no directory and exits 2 with `not found` (R12 to
  R17).
- The run journal is appended through
  `engine::jsonl_append::append_bounded_line_no_follow`: open with `O_APPEND`
  and `O_NOFOLLOW`, take `flock(LOCK_EX)` on that descriptor, check the last
  byte, write one line, `fsync`. Nothing checks the file's size, and nothing
  would stop a writer that opened the file before a rename from appending to
  the renamed file (R18 to R24). The journal is derived and host-local: koto
  never reads its records back, and nothing in the store may come to depend on
  it.

In the bucket, `koto session import` leaves `<prefix>/<name>/migrated.json`
beside the source's objects, where `<prefix>` is the first 16 hex characters
of the SHA-256 of the source workspace's path. Cloud cleanup and relocation
skip the marker on purpose, and nothing deletes one. The source's own objects
stay beside it. A marker body is
`{schema, target: {session, session_id, workspace, prefix}, machine_id,
migrated_at}` (R25 to R29).

## Decision Drivers

- **D1. Nothing new to keep consistent.** Every decision reads state koto
  already writes. Where a new file is unavoidable, it is derived, host-local,
  and safe to delete.
- **D2. The store never depends on the journal.** koto reads no journal
  record, and retention reads only file names, metadata and the positions
  readers record (R21).
- **D3. A wrong removal costs more than a missed one.** Any input the prune
  can't judge keeps its session (the PRD's Decisions).
- **D4. Scale.** Listing and a full preview over 1,000 sessions finish in
  under 2 seconds (R30), so per-session cost has to stay at one header, one
  log, and one template read shared across sessions.
- **D5. The journal's external readers.** A reader may follow the file by
  byte offset, or copy the live file by its exact name on an interval and
  keep a shrunk or changed file as a new version. Rotation can't truncate,
  rewrite or drop a line either might still read (R18 to R22), and since an
  interval copier that doesn't yet list rotated files would miss lines,
  rotation can't be on by default (R23).
- **D6. Coexistence with the gate-and-decider journal work.** That work adds
  record kinds and may flush a per-visit aggregate before the terminal record.
  This design adds no kind, treats every kind alike, keeps the terminal tick's
  order (R17), and builds nothing on the shape of the `run-journal.json`
  sidecar.
- **D7. Additive surfaces.** Existing output keeps its fields and meaning
  (R4). `koto session cleanup` and `koto workspace prune` don't change.

## Considered Options

### Decision 1: Deriving state for the listing

**Chosen: one inspection helper with a per-run template cache.** A new
`src/cli/session_inventory.rs` reads a session's header and log, derives the
current state the way `derive_machine_state` already does, and looks the state
up in its compiled template. Templates are read and parsed once per path per
run, since most sessions in a store share a few templates. Listing and prune
both use it.

*Alternative: read the terminal index.* `$HOME/.koto/_terminal_index.jsonl`
records `completed` or `abandoned` per session name with the header's mtime,
and discovery already trusts an entry only when its mtime is current. It would
save the log read for finished sessions. Rejected: the index lives at
`$HOME/.koto` even for a redirected store, it is keyed by name rather than
session id, and an entry is never removed on rewind or cleanup. Every answer
would need the mtime check anyway, and a stale or missing entry falls back to
the full read. It saves little at 1,000 sessions and adds a second source of
truth that can disagree with the log (D1).

*Alternative: record the terminal state in the header.* A `terminal_state`
header field, written on each terminal arrival, would make a header-only
listing complete. Rejected: it rewrites every finished session's header,
changes the state file format and its reference
(`docs/reference/session-feed.md`), and still can't describe sessions written
by an older koto (D7).

### Decision 2: The prune verb and its apply contract

**Chosen: a new `koto session prune`, preview by default, with a token mode
and an unattended mode.** Without `--apply` it prints the would-remove list,
the kept list with one rule per entry, and a plan token. `--apply --plan
<token>` recomputes the set and refuses, removing nothing, unless the token
matches. `--apply` alone removes the recomputed set and prints it. Both modes
run the same rules.

*Alternative: extend `koto workspace prune`.* It already has `--dry-run`,
`--yes` and `--force`. Rejected: its contract is one named root and everything
under it, and `--force` deliberately skips the safety gate. Making the same
verb also mean "every finished session, with protections" would give one verb
two opposite safety postures (D7).

*Alternative: `koto session cleanup --all-finished`.* This is one of the three
options koto#162 lists. Rejected: `cleanup` is idempotent, per-name and silent
about what else exists. A bulk variant needs a preview, a kept list and a
plan check that a per-name verb has no place for.

*Alternative: token mode only.* This is the exact-list guarantee with nothing
else. Rejected: a scheduled run can't hold a token from a preview it never
read. The unattended mode trades only the equality check, not the
protections.

### Decision 3: How the prune knows each protection

**Chosen: rules evaluated over a full inventory of the local store, from
header, log, one context key and the leg pointer.** The prune inspects every
local session, filtered or not, because a session outside the filters can
still protect one inside them. For each session it reads:

| Rule | Keeps | How the prune knows |
|------|-------|---------------------|
| a | a running or unreadable session | the inspection: status `running` or `unreadable` (log, template or header unreadable, or the session directory is a symlink) |
| b | a session some running or unreadable session references, or any finished session while some session's header can't be read | the reverse of the reference map below, checked against each referencer's status; a session whose header doesn't parse has no known references, so it counts as a possible referencer of every session and is named in each kept reason |
| c | a finished session that references a running or unreadable one | the reference map, checked against the referenced session's status; a reference to a name with no session protects nothing |
| d | a session with a malformed `chain/parent` | the context key exists but its content isn't exactly one name passing `validate_session_id` (no surrounding whitespace or newline) |
| e | a session bound to a waiting or unreadable request leg | `request-leg.toml` in the session directory; the request is read with `request_store::read_view`; kept when the request is open with the leg open, or when the pointer or the request can't be read or parsed; a request that doesn't exist releases it |
| f | retired by koto#532; it kept every session whose name begins `coordinate-` | nothing: the letter stays unused |
| g | under the cloud backend, a session whose remote copy is ahead of the local one, diverged from it, or can't be read | one listing of the session's remote prefix: a remote state log longer than the local one keeps it, and when a remote `version.json` is listed it is compared with the local one by the existing `check_sync`, where anything but `Safe` keeps it |

Rule f is retired. A session's name keeps nothing: a finished coordinator
session is kept like any other, by a pin, by its hierarchy, or by a later
session that continues it (rule r, whose entry names that successor). The
letter f stays unused, since the letters are the `rule` field callers read in
prune's JSON. The retention guide's
[Coordinator sessions and the retired name rule](../../guides/session-retention.md#coordinator-sessions-and-the-retired-name-rule)
covers the change.

Rule g goes beyond the PRD's list for one reason. The cloud backend's
cleanup deletes the session's remote objects too, and a host whose local copy
looks finished can be behind a bucket that another host has advanced, for
example by rewinding the session. The prune judges from the local copy, so it
removes nothing whose remote it can't show is no newer (D3).

The two signals cover different writes. koto bumps `version.json` only on a
context write, so it alone misses an advance or rewind that another host
pushed as state-log events. The remote log's size, from the same listing,
catches that: a pushed log longer than the local one holds events this host
never saw. A local log longer than the remote one only means this host's last
push didn't land, which the removal loses nothing by. A remote `version.json`
the local copy has none to compare with keeps the session too.

An unreadable session counts as possibly running for rules b and c, so it
protects what it references and what references it, as rule a keeps it. When
the unreadable part is the header, what it references is unknown: its
`parent_workflow` is in the header it lost. So while any session's header
can't be read, rule b keeps every finished session and names that session,
so a person can inspect or remove it and let the prune go on. Nothing
narrows the set, since the execution directory that could scope it is in the
header too.

The reference map has an edge A to B when A's header `parent_workflow` is B,
or A's context key `chain/parent` holds exactly B. The context key is read from
the local session directory (`ctx/` under the session's directory) directly,
never through the cloud backend's context store, so a prune makes no network
call to judge a session.

These rules carry the resume convention's close rule without knowing about
it. A chain parent that exits with an `exit:` value closes its children and
then itself. Nothing running references either, so both go. A clean cancel,
refusal or error terminal closes nothing: the children stay running, and they
reference the parent through `chain/parent`. So rule b keeps the parent, and
the running children aren't candidates at all. A koto-lineage child of a
running parent is kept by rule c until the parent finishes, so a parent never
loses a child it hasn't read. The prune never removes a running session,
which is also why it never loses another feature's open per-visit journal
aggregate: one exists only on a session that hasn't finished.

*Alternative: a generic reference area in context (`refs/<name>`).* A
session would declare each session it depends on as a key, and koto would
own the convention outright. Rejected for now: the one consumer that links
sessions without koto lineage already writes `chain/parent`, and a second
convention would need that consumer to write both until every running
session had moved over. Rule c accepts exactly one key, compared and never
interpolated, so widening it later is additive.

*Alternative: scan the request store for open legs bound to each session.*
This is the authoritative reverse lookup (`list_requests` plus `read_view` per
request). Rejected: it replays every request per prune, and the session's own
`request-leg.toml` pointer is what the existing sweep already trusts for the
same question.

*Alternative: protect by template name (skill-session templates).* Rejected:
a template name says nothing about whether a session is still needed, and it
would keep every finished session of the most common template.

### Decision 4: The terminal signal after removal

**Chosen: a finished record written before koto removes a session itself.**
One JSON file per name at `<sessions base>/.finished/<name>.json`, written by
the terminal tick and by the parent sweep, read by `koto status`, and deleted
when a session of that name is created or moved to it, when the removal it
was written for fails, when a person removes a session a crash left one
beside, or by the prune.

*Alternative: stop removing success terminals and let the prune reclaim
them.* `koto status` would answer from the session itself, with its full
result. Rejected: it changes what every terminal tick does for every caller
that relies on the removal today, and the PRD keeps that out of scope. The
`retention` field already makes the removal explicit to the caller of
`koto next`.

*Alternative: answer from the run journal.* The journal's `terminal` record
outlives the session. Rejected outright by D2: it would make `koto status`
read journal records back.

*Alternative: answer from the terminal index.* It has the name and
`completed` or `abandoned`, but no result, no terminal state name, and it
lives at `$HOME/.koto` for every store. It would need new fields and a
retention rule of its own, which makes it this record with a worse location.

### Decision 5: Run journal rotation and retention

**Chosen: rotate on append under the journal's own lock, by a no-replace
move to a sequence-numbered name; writers re-check identity after locking;
readers may record positions keyed by device and inode; retention runs at
rotation; the whole mechanism is off until a size bound is configured.**

*Alternative: rotate from a separate verb or from cron.* Rejected: the bound
would hold only if someone scheduled the verb. A writer-side size check costs
one `fstat` on a descriptor the append already holds. Retention runs at
rotation, so it needs no scheduler either.

*Alternative: copy-then-truncate (logrotate's `copytruncate`).* It keeps the
active file's inode, so an offset reader never sees a new file. Rejected by
D5: lines written between the copy and the truncate are lost, an offset
reader's saved position falls past the new end, and the archiver sees a
shrunk file whose prefix no longer matches. That is the failure the readers'
rule forbids.

*Alternative: timestamp names (`_run_journal.20261011T234436Z.jsonl`).*
Rejected: names would order by the host clock, which can step backwards. A
sequence one past the highest existing rotated file orders by construction.
Retention always keeps the newest rotated file, so the sequence never
restarts on its own; it restarts at 1 only if someone deletes every rotated
file, and then there is nothing to order against.

*Alternative: shift names (`.1` becomes `.2`, logrotate style).* Rejected: a
rotation would rename every rotated file, so a reader holding a name would
find a different file under it.

*Alternative: age-only retention, no reader positions.* Rejected: a reader
offline past the bound would lose lines with no way to prevent it. Positions
are opt-in, so a reader that doesn't record one gets exactly the age-only
behavior.

### Decision 6: Migrated-marker removal

**Chosen: `koto session prune --migrated-from <workspace-path>`, reusing the
prune's preview and token contract. A marker is removable only when its
target is present, and then either by age or with `--source-retired`.
Present means the target's header names the marker's session id and its
whole log reads with no error and begins with every event of the old
copy's log, unchanged. Removal deletes the source's whole prefix, marker
last.**

Removal deletes the old copy along with the marker, so the newer session has
to exist, hold everything the old copy holds, and not have moved on again
before anything goes. The present target is required on both branches.
Age past the bound is the default evidence that the old host is gone;
`--source-retired` lets a caller who knows sooner say so.

*Alternative: remove a marker whenever its target is present and not
migrated away, with no caller statement.* Rejected: the marker is what stops
a source host that comes back from advancing its old copy, and a present
target says nothing about that host. Tying that branch to an explicit
`--source-retired` makes the risky case a deliberate act.

*Alternative: judge the target by its header's session id alone.* Rejected:
a target whose log is truncated or corrupt still carries the right header,
and removing the old copy beside it could delete the only complete copy.
Comparing only the logs' last sequence numbers isn't enough either: an old
copy that gained one event after the import ends at the same number as the
target's `session_imported` event. The import carries the source's event
lines verbatim and the target only appends, so the target's log has to
begin with them, unchanged.

*Alternative: remove by age alone, whatever the target's status.* Rejected:
a marker whose target is gone, replaced or migrated again points at nothing
that holds the session, and removing it would also delete the only remaining
copy beside it.

*Alternative: remove only `migrated.json`, leaving the source's other
objects.* Rejected: with the marker gone, the old copy reads as a live,
unmigrated session again, which is the fork the marker exists to stop.

*Alternative: a bucket-wide listing to find markers in every workspace.*
Rejected for the same reason the import design rejected a bucket-wide name
search: one request per workspace that ever used the bucket, for an operation
that is about one old workspace.

## Decision Outcome

The six decisions fit together around one inspection helper and one prune
verb:

- `koto session list --execution-dir <dir>` and `--state` report each
  session's status from one inspection per session (R1 to R4).
- `koto session prune` turns the same inspections into a removal set. It
  keeps running and unreadable sessions, anything a running session
  references or that references a running session (through
  `parent_workflow` or `chain/parent`), malformed references, sessions
  bound to a waiting request leg, and, under the cloud backend, sessions
  whose remote copy may be newer. It no longer keeps `coordinate-` sessions
  by name: koto#532 retired that rule. A preview is
  stored under its plan token so an exact apply can diff against it, and
  each removal happens under the session's lock after a re-check
  (R5 to R11).
- The terminal tick and the parent sweep write a finished record before they
  remove a session, and `koto status` answers from it. The prune lists and
  removes records alongside sessions (R12 to R17).
- The journal rotates itself into `_run_journal.NNNNNNNNNN.jsonl` files,
  and keeps every one that a reader's recorded `(device, inode, offset)`
  position still needs (R18 to R24).
- `--migrated-from` extends the prune to markers in one old workspace's
  bucket prefix. A marker goes only when its target is present (same session
  id, not migrated again, and a log that reads cleanly and begins with every
  event of the old copy's), and then when it is at least as old as its bound
  or `--source-retired` is given (R25 to R28). koto#343, listing a source
  workspace's sessions
  before an import, isn't covered: the marker preview lists markers only, and
  a session listing is a different read with different callers (R29).

## Solution Architecture

### Session inspection (`src/cli/session_inventory.rs`, new)

```rust
pub(crate) enum SessionStatus { Running, Terminal, Cancelled, Unreadable }

pub(crate) struct Inspection {
    pub name: String,
    pub session_id: Option<String>,
    pub execution_dir: Option<PathBuf>,
    pub parent_workflow: Option<String>,
    pub status: SessionStatus,
    pub state: Option<String>,
    pub failure: bool,
    pub finished_at: Option<String>, // timestamp of the last event in the log
}

pub(crate) struct TemplateCache { /* path -> Option<CompiledTemplate> */ }

pub(crate) fn inspect(base: &Path, name: &str, cache: &mut TemplateCache) -> Inspection;
```

`inspect` reads `<base>/<name>/koto-<name>.state.jsonl` from the local store,
whatever the backend. A session is `Cancelled` when a `workflow_cancelled`
event has no `transitioned`, `directed_transition` or `rewound` after it.
This is narrower than `derive_terminal_status`'s "any cancel" rule only for a
log that moved on after a cancel. Calling such a session running is the
conservative reading for a verb that removes things (D3). Otherwise it is
`Terminal` when its current state is `terminal` in the cached template, with
`failure` from the state's `failure` flag, and `Running` when it isn't. Any
read or parse failure, or a session directory that is a symlink, gives
`Unreadable`. `finished_at` is the last event's timestamp.

The prune enumerates the local base directory itself rather than calling
`Backend::list`, which merges bucket-only placeholder rows under the cloud
backend and drops sessions whose header doesn't parse. Every directory holding
a `koto-<dir>.state.jsonl` is a session, and one whose header fails to parse
is `Unreadable`, so rule a keeps it.

### Listing (`src/cli/session.rs`, `handle_list`)

```
koto session list [--execution-dir <dir>] [--state]
```

- Every row gains `execution_dir` (string or null) and `session_id` (string or
  null), read from the header `LocalBackend::list` already parses.
  `SessionInfo` gains the two fields; the cloud backend's placeholder rows
  carry null for both.
- `--state` adds `state`, `status`, `failure` and `finished_at` from
  `inspect`.
- Under `--state`, a placeholder row keeps null for every added field.
- `--execution-dir <dir>` implies `--state`. The argument is canonicalized
  when it resolves, and trailing slashes are stripped (`/` stays `/` and
  matches every session). A row matches when its `execution_dir` equals the
  argument or starts with it followed by `/`. With
  the filter the output is an object, `{"sessions": [...],
  "excluded_remote_only": N}`, where N counts placeholder rows. Without it,
  the output stays a JSON array.
- Rows are filtered on the header first, and `inspect` runs only on matching
  rows.

### Prune (`src/cli/session_prune.rs`, new)

```
koto session prune [--execution-dir <dir>] [--older-than <n>{s,m,h,d}] [--all]
                   [--apply [--plan <token>]]
koto session prune --migrated-from <workspace-path> [--source-retired]
                   [--apply [--plan <token>]]
```

An unattended `--apply` (no `--plan`) needs at least one of
`--execution-dir`, `--older-than` or `--all`, so a bare `--apply` typed by
mistake is a usage error rather than a store-wide removal.

Session mode:

1. Enumerate every local session (above) and `inspect` each one. Read each
   session's `chain/parent` and build the reference map (Decision 3).
2. Candidates are sessions that pass the filters and are `Terminal` or
   `Cancelled`, plus finished records (below) that pass the filters. The age
   filter compares `finished_at` with now.
3. For each session that passes the filters, whatever its status, evaluate
   rules a to g in order, so a running or unreadable session shows on `kept`
   under rule a. The first rule that applies puts the session on `kept` with
   that rule's letter and a detail (the referencing or referenced name, the
   request id); a candidate no rule keeps goes on the would-remove list.
   Records have
   no rules: koto writes one only when it removes a session itself, and that
   removal already waited until the session's parent had its
   `child_completed` (the terminal tick defers removal when that append
   fails), so no parent still needs a record to learn its child's result.
4. The plan token is the first 32 hex digits of SHA-256 over the sorted lines
   `<kind>\t<name>\t<session_id>\n`, where `kind` is `session` or `record`.
   Only would-remove entries enter it.
5. The preview stores its would-remove list, with the filters it ran with,
   at `<sessions base>/.prune-plans/<token>.json` (mode 0600 under a 0700
   directory; derived and safe to delete), removing any stored plan older
   than a day, and prints:

```json
{"mode": "preview", "plan": "<token>",
 "remove": [{"kind": "session", "name": "...", "session_id": "...", "status": "terminal",
             "state": "done", "failure": false, "finished_at": "...", "execution_dir": "..."}],
 "kept": [{"name": "...", "rule": "b", "detail": "referenced by running session 'x'"}]}
```

6. Apply recomputes steps 1 to 4. With `--plan` it runs with the stored
   plan's filters, and a filter passed alongside `--plan` is a usage error
   (exit 2), so a filtered preview's token applies as it stands. A
   recomputed token that differs exits 4 (`prune_plan_stale`, a code no
   other koto verb uses),
   removes nothing, and prints `added` and `dropped` by diffing the recomputed
   list against the stored plan. With no stored plan for the token (expired,
   or previewed on another host) it still refuses with exit 4 and says the
   plan is unknown. A token that isn't 32 lowercase hex digits is a usage
   error (exit 2). Removal then walks only entries the stored plan names,
   so a session that finished after the preview is never removed by that
   plan, even if the hash were to collide. A successful apply deletes its
   stored plan.
7. For each session to remove, children first (deepest `parent_workflow`
   first): take `lock_state_file` (non-blocking), re-`inspect`, and confirm
   the same session id, still finished, and no rule applies against the
   referencers and referenced sessions re-inspected now (rule g's remote
   check included). Then remove it while holding the lock: under the cloud
   backend the session's remote objects first, except a migration marker
   (R10), and the local directory after them, so a failure part way leaves a
   local copy that a later prune can retry. The remote delete goes through a
   variant of `sync_delete_session` that
   returns the first remote failure instead of only warning, so a remote
   delete that failed reaches `failed`. A held lock, a changed id, a session
   no longer finished or newly protected each go to `skipped` with the
   reason. For each record to remove, re-read it, confirm the same session id
   and `finished_at`, then delete it.

   The state lock is taken by `koto next`; other writers (`rewind`, `cancel`,
   `context add`) append under the per-append lock instead. The re-check
   narrows a race with them to the moment between it and the removal, and
   doesn't close it.
8. Apply prints `{"mode": "apply", "plan": ..., "removed": [...], "skipped":
   [...], "kept": [...]}` and exits 0. A failed removal is reported under
   `failed`, and the run exits 3.

The prune never writes a finished record, never touches the run journal,
and removes a pruned session's `run-journal.json` sidecar with its
directory (R11).

Marker mode (`--migrated-from`; usage error, exit 2, unless the backend is
cloud):

1. `prefix_for_workspace(<workspace-path>)` gives the source prefix. One
   paginated ListObjectsV2 over `<prefix>/` collects the keys exactly
   `<prefix>/<name>/migrated.json` where `<name>` passes `validate_session_id`.
2. GET each marker body. A body that doesn't parse, or has no
   `migrated_at`, is kept with reason `unreadable`.
3. Target status: list `<target.prefix>/<target.session>/` (every listing
   prefix ends in `/`, so `foo` never matches `foo-2`). A `migrated.json`
   there makes it `migrated`. Otherwise, a state file whose header names
   another session id makes it `replaced` (a later session under the same
   name), and one whose header doesn't read makes it `unknown`. When the
   header's `session_id` equals the marker's `target.session_id`, the
   target's whole log is read with the import's own parser (no line that
   doesn't parse, the last one included, and no sequence gap) and compared
   with the old copy's, read the same way from `<prefix>/<name>/`: the
   target's event lines must begin with every one of the old copy's,
   unchanged, to make it `present`. A target log that doesn't read, ends
   early or differs is `unverified`, and so is any target when the old
   copy can't be listed or its log can't be read. An old copy whose state
   file is gone, which only a removal that stopped part way leaves, has no
   events to compare. No state file at the target makes it `absent`. A
   failed target listing or read makes it `unknown`. Only `present` can
   satisfy either branch.
4. Removable when the target is `present` and either its age (now minus
   `migrated_at`) is at least `session.cloud.marker_retain_days` (default
   30; 0 makes every such marker removable at once; read from user config
   only, since it guards a deletion and a cloned repository's project file
   must not shorten it) or `--source-retired`
   was given. All others are kept, and the preview names the target status
   or the age as the reason, with `objects`, the number of objects the
   removal deletes, when the old copy could be listed and its log read.
5. The token covers lines `marker\t<name>\t<migrated_at>\n`.
6. Apply re-reads each marker (same `migrated_at`) and re-evaluates its
   rule, so a target that changed status since the preview is skipped. It
   then lists `<prefix>/<name>/`, deletes every object except the marker, and
   deletes the marker last. A failed
   delete stops that entry before its marker is deleted, so the marker still
   refuses, reports it under `failed`, and the run exits 3. Re-running
   finishes it. Nothing locks the bucket between the re-evaluation and the
   deletes, so a target that changes in that window isn't seen.

Marker mode removes nothing from the local store, including a local copy of
the source session that this host might hold; its only local write is the
stored plan. Once the marker and the old copy in the bucket are gone,
nothing refuses a local copy still on the old host, and its next sync pushes
it back. The guides therefore say to run `koto session cleanup <name>` on
the old host, or retire it, before removing its markers.

### Finished records (`src/session/finished.rs`, new)

```json
{"v": 1, "name": "deploy-staging", "session_id": "…", "execution_dir": "/…",
 "state": "done", "status": "terminal", "failure": false,
 "result": {"status": "success", "summary": "…"},
 "finished_at": "2026-10-11T23:44:36.123Z", "removed_by": "terminal"}
```

- Location: `<sessions base>/.finished/<name>.json`. The base is the parent
  of `backend.session_dir(name)`, so a redirected store keeps its own records
  and the cloud backend keeps them in its local store. They never sync. The
  directory is created mode 0700, and every write, read or delete first
  `lstat`s it and refuses a symlink. Each record is written mode 0600 to a
  temporary file in the same directory and renamed into place. A read refuses
  a symlinked record and one whose `name` field differs from its file name.
- Size: at most 64 KiB. When the serialized result would exceed that, the
  record keeps the result's `status` and `summary` and sets `"truncated":
  true`.
- Writers: `finish_terminal_tick`, after the journal's terminal record, which
  it already writes before any removal, and immediately before `sweep_if_parent`
  and `backend.cleanup`, with `removed_by: "terminal"`. Also `sweep_children`
  in `src/cli/workspace.rs`, before each child's `cleanup`, with `removed_by:
  "parent"` and the child's result from
  `terminal_result::recorded_result_for_current_arrival`, falling back to the
  read-only `resolve_terminal_result` that `handle_status` already uses. A
  cancelled child's record has `status: "cancelled"`, the state it was
  cancelled in as `state`, and a null result; `koto status` answers it with
  `is_terminal: true` and `status: "cancelled"`, where a kept cancelled
  session's answer reads the current state from its log. `finished_at` is the
  time koto writes the record, for either writer. Only the branch that
  actually removes writes a record. A write failure
  prints one warning and the removal proceeds. The tick's order is therefore:
  whatever precedes the terminal journal record (unchanged), the terminal
  record, the finished record, then the removal (R17).
- Reader: `handle_status`. When `backend.exists(name)` is false and a record
  reads, it exits 0 with `name`, `current_state`, `is_terminal: true`,
  `result`, `removed: true`, `finished_at`, `session_id` and
  `execution_dir`, plus `result_truncated: true` when the record kept only
  the result's status and summary. A kept terminal session's answer already uses the first
  four with the same meanings (R14). With no record, it answers `not found`,
  exit 2, as today.
- Contents: the record keeps the session's whole result, including any
  payload its terminal state's result map carries, until the name is reused
  or the prune removes it. The removed answer has no `template_path`,
  `template_hash`, `batch` or `leg`, since no session is left to read them
  from.
- Deletion: `LocalBackend::init_state_file` deletes
  `.finished/<name>.json` after creating the session, and the cloud backend's
  initialization goes through it. `relocate`, which a rewind uses to move a
  parent's children to new names, deletes the record under the name it
  moves a session to; a moved session is live and has none of its own. A
  removal that fails after its record was written deletes the record again.
  When `koto session cleanup` or `koto cancel --cleanup` removes a session,
  it deletes a record a crash left beside it (written just before a removal
  that never happened); cleanup on a name koto already removed finds no
  session and keeps the record, so cleanup stays idempotent. Otherwise the
  prune is the verb that clears records. Nothing
  lists `.finished` as a session, because `list` and `count_unreadable` only
  consider directories holding a `koto-<dir>.state.jsonl`.

### Run journal rotation (`src/run_journal/rotate.rs`, new)

The journal's append path changes in one place: `run_journal::write` calls
`rotate::append(root, line, max, &settings)` instead of
`append_bounded_line_no_follow`. The new function (backed by
`jsonl_append::append_bounded_line_current`, a variant of the existing helper)
loops, up to eight times:

1. Open `<root>/_run_journal.jsonl` with `O_APPEND | O_CREAT | O_NOFOLLOW`,
   mode 0600, and take `flock(LOCK_EX)`.
2. Compare the descriptor's `(dev, ino)` from `fstat` with the path's from
   `lstat`. If they differ, or the path is gone, the file was rotated after
   the open. Close and go to 1. This is what keeps a raced writer out of a
   renamed file (R20).
3. If the size bound is non-zero and the descriptor's size is at or over it,
   rotate. If the file's last byte isn't a newline (a crash left a partial
   line), append one first, so no rotated file ends mid-line. Compute the next
   name and move the file there without replacing anything: `link(2)` the
   path to the new name, which fails rather than overwrite an existing file,
   then `unlink(2)` the old path. Then run retention, close, and go to 1. The
   move keeps the inode, so the locked descriptor now refers to the rotated
   file and every waiting writer fails step 2. Between the link and the unlink
   the file has both names for an instant, and a reader keyed by identity sees
   one file. A failed link prints one warning, skips rotation for this
   process, and falls through to 4 on the unrotated file. A failed unlink
   after a successful link removes the new name again, so no file keeps two
   names, and is handled the same way.
4. Partial-last-line check, write, `fsync`, as today.

After eight failed identity checks the record is dropped with the journal's
usual one-time warning, like any other failed journal write.

The size check runs once per locked append, before anything is written. When
the append carries several lines under one lock (one commit's records written
as a batch), the batch is one unit: rotation happens before it or after it,
never between two of its lines. `run_journal::write` keeps its signature;
rotation wraps the one call it makes to the append helper.

**Naming rule.** Rotated files are `_run_journal.NNNNNNNNNN.jsonl`, ten
decimal digits, zero-padded, in the journal's own directory. The next number
is one more than the highest existing rotated file's, or 1 when none exists.
Lexical order of the names is rotation order, and `_run_journal.jsonl` is
always the newest. Only names matching `^_run_journal\.[0-9]{10}\.jsonl$`
count.

**Reader positions.** A reader records its position in
`<root>/_run_journal.readers/<reader-id>.json`. The reader id matches
`^[A-Za-z0-9._-]{1,64}$`. The file holds:

```json
{"v": 1, "dev": 16777231, "inode": 1234567, "offset": 52311,
 "file": "_run_journal.0000000007.jsonl"}
```

`dev` and `inode` name the journal file the reader is in, and survive that
file's rotation. `offset` is the byte count it has consumed. `file` is
informational. A reader that archives whole files and wants this protection
writes the identity of the last file it archived and that file's length.
Recording a position is optional. A reader writes its position by temporary
file and rename, and refreshes it on every pass, since the stale bound goes
by the position file's modification time. koto creates the directory (mode
0700) on first rotation and never writes a position itself. A reused inode
(a removed file's number given to a new one) can only make a stale position
protect more than it needs, never less.

**The offset reader's rule.** Keep `(dev, inode, offset)` for the file you're
reading. On each pass, open `_run_journal.jsonl` and `fstat` the open
descriptor (stat-then-open could race a rotation). If its identity is yours,
read from your offset. Otherwise find the rotated file with your identity,
read it from your offset to its end, then read every newer rotated file (by
name) from byte 0, then the open active file from byte 0, and keep the new
identity and offset. If another rotation lands during the pass, the file
open as "active" turns up among the rotated files; skip it there by
identity, since it is read last. A reader that finds no file with its
inode lost its place to
retention, and only a reader that recorded no position, or let it go stale,
can be in that state.

**The interval copier's rule.** A reader may not follow the file at all and
instead copy the live journal by its exact name every so often, keeping a
shrunk or changed file as a new version. Rotation alone would cost such a
reader the lines written after its last copy and before the rotation: they
exist only in the rotated file, which it never looks at. To stay lossless it
must, on every pass, also list the rotated files by the naming rule and copy
each one newer than the last it copied, in name order, before copying the
live file. A reader like this needs no position file; age-based retention
covers it as long as it runs more often than the retention bound. This
failure mode is why rotation is off by default (below).

**Retention.** It runs after each rotation, under the rotated file's lock:

1. Read every position file in the readers directory, refusing symlinks and
   files over 4 KiB. One that doesn't parse, or whose mtime is older than
   `run_journal.reader_stale_days`, is ignored with a warning naming it.
2. Map each position to a journal file by `(dev, inode)`. A position protects its
   file and every newer one. One whose offset is at or past its rotated file's
   end protects only the newer ones. A position matching no file protects
   nothing and is reported.
3. Remove each rotated file whose mtime is older than
   `run_journal.retain_days` and that no position protects, except the
   newest rotated file, which is always kept so the sequence never restarts
   and name order stays rotation order. A retention bound therefore leaves
   at least one rotated file behind, however old.

Retention reads file names, `stat` metadata and position files, never a
journal record (R21, D2).

**Settings.** `run_journal.rotate_bytes` (default 0, which means rotation is
off; 67108864 is a reasonable value to opt in with),
`run_journal.retain_days` (default 30) and `run_journal.reader_stale_days`
(default 90) join `src/config` the way `request_store.terminal_index_compact_lines`
did. Rotation stays off by default until the journal's consumers list rotated
files; turning it on by default then is a one-line change to the default, not
part of this design. With rotation off, the size check is a comparison
against 0 and nothing else changes on the append path. All three settings
are read from user config only, like the marker bound: the journal lives
beside the user's session store, so a project file in a cloned repository
must not turn rotation on or shorten retention, and `koto config set`
refuses them without `--user`.
`run_journal::write` resolves them once per process into a `OnceLock`, with
a test override like the module's existing ones.

Nothing in `koto session list`, `koto workflows` or the layout migration
mistakes the new files for sessions: rotated files are files, and the readers
directory holds no `koto-<dir>.state.jsonl`.

### Data flow summary

```
koto next (terminal) ─ terminal journal record ─ finished record ─ cleanup
koto status <gone>   ─ .finished/<name>.json ─▶ removed answer, exit 0
koto session list    ─ header ─(filter)─ inspect ─▶ rows
koto session prune   ─ inspect all + chain/parent + leg pointer ─▶ rules ─▶ token
                     └ apply: lock ─ re-inspect ─ cleanup (remote too)
journal append       ─ open ─ lock ─ identity check ─(size ≥ bound)─ rename ─ retention
prune --migrated-from ─ list prefix ─ GET markers ─ target status ─▶ token
                      └ apply: delete prefix objects ─ delete marker last
```

## Implementation Approach

The work lands as five pull requests, in this order, and a sixth that
follows the finished records. The listing comes first because the prune is
built on its inspection helper.

1. **Listing.** `session_inventory.rs`, `SessionInfo`'s two fields,
   `--execution-dir` and `--state`, the koto-user skill's command reference,
   and the 1,000-session timing (R1 to R4, R30 for listing).
2. **Prune (sessions).** `session_prune.rs`, rules a to g, stored plans,
   both apply modes, exit 4, the checked cloud delete, the 1,000-session
   preview timing (R30 for the prune), and the `docs/workspace-layout.md` and
   skill updates (R5 to R11). Depends on 1.
3. **Finished records and status.** `finished.rs`, the writers in
   `finish_terminal_tick` and `sweep_children`, `handle_status`, deletion on
   init, records as prune entries, and `.finished` and `.prune-plans` in
   `docs/workspace-layout.md`'s derived files (R12 to R17). Depends on 2 for
   the prune half.
4. **Journal rotation.** `rotate.rs`, the identity-checked append variant,
   the three settings, the reader rule in `docs/reference/run-journal.md`, and
   the rotated files and readers directory in `docs/workspace-layout.md`
   (R18 to R24). Independent of 1 to 3. It changes the journal's append path,
   which concurrent work adding journal record kinds also touches, so the two
   pull requests are merged one after the other rather than together.
5. **Marker removal.** Marker listing and removal in `src/session/cloud.rs`,
   `--migrated-from` and `--source-retired`, `session.cloud.marker_retain_days`,
   and fake-S3 tests (R25 to R29). Depends on 2. It shares `src/config` with
   4, so it follows 4 to avoid a conflict there.
6. **Finished-record clean-up.** The record deletions by
   `koto session cleanup`, `koto cancel --cleanup`, a failed removal and
   `relocate`, and the docs on the removed answer's fields and the record's
   contents (R15). Follows 3.

Each pull request carries its verbs' end-to-end tests, run against a scratch
HOME or `KOTO_SESSIONS_BASE`, with cloud behavior on koto's fake S3 harness
(R31, R32).

## Security Considerations

- **Path construction from stored strings.** Session names come from
  directory listings and are re-validated with `validate_session_id` before
  any path is built. A `chain/parent` value is compared against the name
  set, never joined into a path or a command. Finished-record names come
  from validated session names only. Marker session names come from bucket
  keys and are validated before any key is built from them. Removal touches
  `<prefix>/<name>/` only.
- **Symlinks.** The prune treats a session directory that is a symlink as
  unreadable and keeps it, the same `lstat` check `koto workspace prune`
  uses on its root. Finished records and reader positions are opened with
  `O_NOFOLLOW`. The journal's existing no-follow open is kept, and the
  identity check compares `lstat` of the path, so a symlink planted at the
  journal's path fails the check rather than redirecting a write.
- **Untrusted input from other writers.** Position files are written by
  readers koto doesn't control. They are size-capped, parsed strictly, and
  can only add protection, never cause a deletion. A forged position can at
  most keep rotated files past their bound, and the stale bound ends that.
  Marker bodies in the bucket are untrusted: their fields are compared,
  printed JSON-escaped, and used to build only the target listing prefix,
  which is validated like any key. A forged marker can make a session refuse
  (as today) or be listed, never cause the removal of objects outside its own
  source prefix.
- **Races.** The prune removes each session under its state lock after a
  re-check. Commands that don't take that lock (`rewind`, `cancel`, a raw
  edit of the session directory) can still race it between the re-check and
  the removal, as they can race `koto session cleanup` today. A session
  created between the inventory and a removal that newly references the
  removed session is not seen. A remote advance between rule g's check and
  the remote delete is the same one-removal window; the remote delete isn't
  conditional, so it can't close without conditional deletes. These windows
  are accepted and documented.
- **Another host's newer copy.** Under the cloud backend a removal deletes
  remote objects too, so the prune keeps any session whose remote state
  log is longer than the local one, or whose remote `version.json` is ahead
  of, diverged from, or unreadable against the local copy (rule g), and
  re-checks it at apply. A remote delete that fails is
  reported, not swallowed. Without a remote `version.json`, the check
  compares only the pushed log's length, so a remote log that diverged
  without growing longer than the local one isn't caught.
- **Mistyped scope.** An unattended `--apply` needs an explicit filter or
  `--all`, and the token mode refuses a plan it didn't store. The unattended
  form removes in one step with no preview anyone read: the rules still
  apply, but only `--plan` guarantees that what goes is what someone saw.
- **Disclosure.** Finished records hold a session's result, which `koto
  status` already printed to anyone who could read the session. They stay in
  the store's base, mode 0600, under a 0700 directory, and never sync.
- **Deletion scope.** Marker mode deletes only under the computed source
  prefix, and only for markers matching the stated rule, marker last.
  `--source-retired` is the only input that widens it, and the preview
  states which markers it widens to.

## Consequences

### Positive

- A teardown removes what its worker left with two commands, and the preview
  says exactly why anything stayed.
- `koto status` tells a clean finish from a missing session, with the
  result, for every session koto itself removed.
- An operator can bound the journal's growth with one setting, and a reader
  that follows the stated rule keeps every line.
- Old workspaces' bucket prefixes can be emptied once their sessions have
  moved.

### Negative

- Listing with state and a prune preview read every session's log. Cost
  grows with the store, about 2 seconds at the stated bound.
- Finished records are a new file kind that grows until a prune removes them
  or the name is reused.
- A reader that never records a position can lose files once they pass the
  retention bound.
- The journal stays unbounded by default: rotation is opt-in until the
  journal's consumers list rotated files.
- The rules know one context key by name, `chain/parent`, so a consumer
  linking sessions any other way gets no protection from rule c.
- Marker removal by age relies on the stopped-source rule. A host revived
  after the bound could advance its old copy again. The repository owner
  accepts this risk for multi-host use, which koto treats as a migration and
  never as a shared session.
- Under the cloud backend a preview makes one remote listing per candidate
  session for rule g, plus a GET when the session has a remote
  `version.json`.

### Mitigations

- The template cache and header-first filtering keep the per-session cost to
  one log read. The PR adding the verbs records the 1,000-session timing.
- `koto session prune` removes records under the same filters, and the
  records are a few hundred bytes each.
- The rule and the position format are in `docs/reference/run-journal.md`,
  and the default retention bound (30 days) is far longer than any polling
  interval a reader would use.
- Turning rotation on by default later is a one-line change to one setting's
  default, made once consumers list rotated files.
- Rule c reads one key with a fixed name. Adding a second reference key later
  is an additive change to the reference map.
- Marker removal by age is opt-in per run and previewed, and the preview shows
  each marker's age and target before anything is removed. The bound is a
  setting, so an operator who keeps old hosts around longer can raise it.
- Only finished candidates that pass the filters get the remote check, and
  it is skipped entirely under the local backend.
