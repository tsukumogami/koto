# Lead: Should koto recover headerless state logs already on disk, and how?

## Findings

### What `koto session recover` does today

`koto session recover` only handles the old-layout migration quarantine. The module
doc says so directly (src/session/recover.rs:1-25): it walks
`base/.migration-conflicts/<repo-id>/<name>/` and moves each directory back into the
flat namespace as `r<repo-id>-<name>`. It is wired into the CLI at src/cli/mod.rs:1391.
The handler is src/cli/session.rs:654-765. It reports by default and needs `--apply`
to move anything, and it notes that it works only on the local store
(session.rs:748-753).

Headerless logs show up only as a side case. `recover_one` moves the directory,
renames the state file, and then tries `rewrite_header_identity`. If the header
won't parse, it records `header_error` and still counts the session as recovered
(recover.rs:228-236). The unit test (recover.rs:503-530) and the CLI test
(tests/session_recover_cli.rs:237-267) both feed it a `{"seq":1,"type":"context_added"}`
line and assert `header_rewritten: false`. The CLI test also says outright that
"`session list` still skips it -- an unreadable header is a separate defect"
(session_recover_cli.rs:259-261). So the command makes a headerless session
addressable again but leaves it just as unreadable. It never looks at a headerless
log sitting in the normal flat namespace, which is exactly where the #236 and #200
logs live.

### Where the corruption error comes from and what it tells the user

- `parse_header` wraps the serde error as
  `EngineError::StateFileCorrupted("failed to parse header: ...")`
  (src/engine/persistence.rs:570-572). The Display prefix is `state file corrupted:`
  (src/engine/errors.rs:19-20), and it exits with code 3 (errors.rs:184).
- The message suggests no remedy and doesn't separate "never had a header" from
  any other parse failure. The reference docs send the reader to "Inspect the
  file directly" (docs/reference/error-codes.md:172-178). The koto-skills guidance
  for `persistence_error` / exit 3 is "Report to user; this is an infrastructure
  problem" (plugins/koto-skills/skills/koto-user/references/error-handling.md:14,78).
- No code path points at `koto session recover` for this error. That's right,
  because recover can't fix it.

### How unreadable logs are treated elsewhere

- `koto session list`: `LocalBackend::list` silently drops any session whose header
  won't parse (src/session/local.rs:125-144). The only place they're counted is
  `count_unreadable` (local.rs:152-181), and only the dashboard surfaces that
  count (src/cli/dashboard_data.rs:640). `CloudBackend::list` builds on the local
  list (src/session/cloud.rs:700-706).
- The discovery scan prints `warning: skipping <path> during discovery: state file
  corrupted: ...` for each unreadable header on every tick
  (src/engine/discovery.rs:406-414). That's the "skipping ... state file corrupted"
  noise on unrelated sessions that #236 describes. shirabe's docs had to tell
  agents not to act on it (shirabe skills/scope/SKILL.md:303,
  skills/scope/references/phases/phase-0-setup.md:392).
- `find_wake_candidates` prints `warning: skipping wake candidate ... header read
  failed` (src/engine/wake.rs:274-282).
- `wake_candidates_pass` treats any `read_events` error on the coord's own log as
  "Fresh coord with no log yet" and prints an `info:` line before skipping
  (wake.rs:470-486). That's the `info: wake_candidates_pass found no readable coord
  log` line in #236. It swallows corruption as if it were a missing log. The real
  `persistence_error` comes a moment later from the main `koto next` read.

### The header, and what a #236-shaped log is missing

`StateFileHeader` (src/engine/types.rs:223-330) requires `workflow`, `template_hash`
and `created_at`. `schema_version` defaults to 1 and `session_id` defaults to empty.
The optional fields are `parent_workflow`, `template_source_dir`, `execution_dir`,
`intent`, `template_name`, plus the request-store and dispatch fields
(`dispatch_epoch`, `coordinator_of_record`, and so on). The only producer of a
child header is `init_child_core`, which fills it in from the compiled template
and the parent (src/cli/init_child.rs:555-582).

For a #236 log that holds only `context_added` events:

- `workflow` can be recovered: it's the directory name, which is also the
  state-file name (`state_file_name(id)`).
- `parent_workflow` can be guessed from the dotted prefix, but only guessed.
- `created_at` could be taken from the first event's timestamp, which would be a
  fabricated value.
- `template_hash`, `template_name`, `template_source_dir` and `execution_dir`
  can't be recovered from the log at all. They come from the child's template,
  which the parent's batch task entry names, and from the parent's header.
- More importantly, the log has no `workflow_initialized` event (so no
  `template_path` or variables) and no initial `transitioned` event. Even with a
  reconstructed header, `derive_state_from_log` would return `None`
  (persistence.rs:708-715), and `koto next` would fail with the existing
  `corrupt state file: cannot derive current state` error (src/cli/mod.rs:3696).
  An in-memory header turns one unreadable error into another. It never gives
  you a session that can be ticked.

A session that has context events and no init isn't meaningful to "recover" as a
session. Its only real content is the context blobs, and those live outside the
log in `ctx/` with their own `manifest.json` (local.rs:492-540). `ContextStore::add`
never checks that the session is initialized (local.rs:493-498), which is how the
content gets there before any log exists. The blobs survive anything that touches
only the state-log file.

### What the batch scheduler does with a headerless child today

This is how #236 strands the parent, traced through the code:

1. `snapshot_existing_children` builds its snapshots from `backend.list()` and
   filters on `info.parent_workflow` from the header (src/cli/batch.rs:1485-1509).
   A headerless child is already missing from `list()`, so it never gets a
   snapshot.
2. `repair_half_initialized_children` removes a child only when `read_events` is
   `Ok` with no events, meaning a header-only file. An unreadable log falls into
   the `_ =>` arm and is left alone on purpose (batch.rs:659-676).
3. With no snapshot, `classify_task` treats the child as unspawned. It stays
   `BlockedByDep` while its deps are unmet (batch.rs:554-600), which matches the
   `blocked` the reporter saw, and becomes `Ready` once they're met.
4. `spawn_ready_task` calls `init_child_from_parent` (batch.rs:1569-1577). That
   runs `backend.create`, a no-op `create_dir_all` (local.rs:65-75), then
   `init_state_file`, which writes a tempfile and does
   `renameat2(RENAME_NOREPLACE)` onto the existing state-file path
   (local.rs:242-303, src/engine/atomic_fs.rs:49-83). The existing headerless
   file makes that fail with `Collision`. Init therefore refuses. It never appends
   a header after the events, and it never overwrites them.
5. The Collision becomes `SpawnErrorKind::Collision` with the message "child
   workflow already exists: state file collision" (init_child.rs:268-277). The
   task is marked `Failure` for the rest of that tick (batch.rs:1590) and
   reported as `SpawnFailed` / `Errored { kind: "collision" }`
   (batch.rs:1027-1031, 1168-1174). Classification is rebuilt from disk on every
   tick, so the same collision repeats on every tick forever. The parent's batch
   never settles.

Deleting or moving aside only the headerless state-log file would let the next
tick's normal Ready path init the child cleanly. `init_state_file` needs the file
to be gone and nothing else: the directory and `ctx/` can stay.
`koto session cleanup <child>` also works today because it runs `remove_dir_all`
(local.rs:85-92), but it throws away the `ctx/` content the agent wrote, and
nothing tells the user it's the fix.

### #200-shaped logs

A #200 log is a single `context_added` line from a session that had gone through
about 45 events. The header, the `workflow_initialized` event (template path and
variables), all the evidence and all the transitions are gone. Nothing in the file
can rebuild the current state or the audit trail. At most the `ctx/` blobs and the
`workflow` name survive. Confirmed: nothing is recoverable beyond "start over".
Worse, by content alone a #200 log looks exactly like a #236 log: both are
headerless, both hold only `context_added` events, and both have `seq:1` or a
duplicated seq. Any automatic rule that treats "headerless plus context-only"
as "never started" would also catch a #200 session whose history was destroyed.

### The three options

- **(A) A tolerant reader that builds a header in memory.** It would touch
  `parse_header`/`read_log_inner`, which every log family goes through
  (persistence.rs:611-627), so request logs too. It would have to invent
  `template_hash` and `created_at`. It still can't produce a tickable session,
  because the init events are missing. It also hides real corruption: #200's
  destroyed history would read back as a quiet, empty session. And it changes
  what `list()`, discovery and wake see, which is agent-facing. High risk, low
  payoff. Reject.
- **(B) Quarantine a headerless log so the child can re-init.** The narrowest
  version goes in `repair_half_initialized_children` (batch.rs:647-677), which
  already exists for "a child file blocking a clean spawn". It would rename the
  child's state log to a sibling such as
  `koto-<id>.state.jsonl.headerless-<ts>` when the first line doesn't parse as a
  header but does parse as an `Event`. It would leave `ctx/` alone and emit a
  scheduler warning. The normal Ready path then inits the child on the same
  tick. It needs no new CLI verb, and the only agent-facing change is one
  warning. It doesn't delete anything, and the preserved file keeps #200-style
  evidence. Extending `koto session recover` instead is a worse fit: that command
  is scoped and documented as a migration-quarantine tool, and it would need a
  new mode and new docs. It also doesn't help a stranded batch unless somebody
  first figures out that they should run it. The risk in B is the #200 overlap:
  a live, advanced child whose history was lost gets re-inited from the initial
  state. That's also the only way forward for such a child, and the warning
  plus the kept file keep it from being silent.
- **(C) No recovery, just a clearer error.** In `read_header_only`/`read_log_inner`,
  when header parsing fails, check whether line 1 parses as an `Event` (it has
  `seq` and `type`). If it does, report something like `state log has no header:
  first line is a <type> event (seq N); the workflow was never initialized or its
  history was lost` instead of serde's `missing field workflow at column 187`. It
  costs a few lines, touches no data, keeps the same error variant and exit code,
  and just improves the text. It shouldn't name `koto session cleanup` as the
  remedy in the generic error. shirabe already had to warn agents that a cleanup
  suggestion on a live run in another worktree is destructive. The remedy belongs
  in docs/reference/error-codes.md.

## Implications

Once the root cause is fixed (refusing appends to a missing log, covered by other
leads), new headerless logs stop appearing. Recovery then only matters for logs
already on users' disks from 0.12.x, and for any leftover #200-type race. The #236
impact that actually hurts is the stranded parent batch: the headerless child
blocks its own re-init by colliding on every tick. That's a scheduler problem more
than a reader problem. The scheduler already has a repair pass for exactly this
category, "a child file blocking a clean spawn". Its only gap is that it ignores
unreadable logs.

Rebuilding a header is the wrong tool. The missing information (template hash, the
init events) can't be recovered, and the result still wouldn't be tickable. It
would also make #200's destroyed history look like a valid empty session. The
duplicate `seq:1` lines show that `read_last_seq` also mis-parses these files
(persistence.rs:513-518 skips line 1 as if it were the header), so any tolerant
path would inherit that bug too.

**Recommendation:** do C in this PR, reject A, and decide on B separately:

- **C (clearer error):** have the header reader recognize a headerless log (line 1
  parses as an event) and say so plainly, and document the manual remedy in
  error-codes.md: move the state-log file aside, or run `koto session cleanup` on
  a batch child, then tick the parent.
- **B (optional, in this PR if the scheduler owner agrees):** extend
  `repair_half_initialized_children` to rename a headerless child log aside, keep
  `ctx/`, and emit a scheduler warning, so stranded batches heal on the next
  parent tick. Leave `koto session recover` unchanged.
- **A (tolerant reader):** reject.

## Surprises

- `koto session recover` already "recovers" headerless sessions, but only in the
  sense of moving them out of the migration quarantine. They stay unreadable, and
  its own test says so (session_recover_cli.rs:259-261). Its name suggests it
  could help with #236, but it can't.
- The batch scheduler already refuses correctly: the atomic `RENAME_NOREPLACE`
  init never clobbers the headerless file. But the refusal surfaces as a
  misleading "child workflow already exists" collision, and it repeats on every
  tick instead of being diagnosed.
- A spawn Collision marks the task `Failure` for the rest of that tick
  (batch.rs:1590). Under `failure_policy: skip_dependents`, dependents classified
  later in the same tick could be spawned as terminal skip markers, and those
  persist. So a headerless child could permanently skip-mark its downstream
  tasks. This was inferred from the topological classify-and-spawn loop
  (batch.rs:857-1004) and not confirmed by a test.
- `wake_candidates_pass` labels every coord-log read error as "Fresh coord with no
  log yet" (wake.rs:470-477). Corruption gets reported at `info:` level as though
  it were normal.
- `list()` hides headerless sessions completely, so the scheduler's snapshot,
  `session list` and the dashboard tree all behave as if the child doesn't exist,
  while `backend.exists()` (a bare file-existence check, local.rs:81-83) says it
  does. The two disagree, and that disagreement is what produces the collision
  loop.

## Open Questions

- Does anything reconcile the `ctx/manifest.json` keys against the log's
  `context_added` events? If B keeps `ctx/` and re-inits, the new log has no
  `context_added` events for keys that are already in the manifest. That's
  probably harmless, but it should be checked.
- Cloud backend: if a headerless log was already pushed to S3, would a local
  rename-aside be undone by a later sync pull? `CloudBackend::exists` falls back
  to S3 (cloud.rs:686-692).
- Can the skip-marker cascade under `skip_dependents` be reproduced with a test?
  If it can, B, or at least a scheduler-side special case for Collision, becomes
  more urgent.
- Should the scheduler's repair only quarantine a headerless child whose events
  are all `context_added`, so it won't touch logs with other event types that
  might be a different kind of corruption?

## Summary

`koto session recover` only drains the migration quarantine and leaves headerless logs unreadable, and a #236 log can't be turned back into a tickable session because it never had the init events. A #200 log has lost its history entirely and can't be told apart from a #236 log by content. The batch scheduler already refuses to overwrite a headerless child (atomic `RENAME_NOREPLACE`), but that refusal surfaces as a "collision" spawn failure on every tick, which is what strands the parent. Recommendation: reject a tolerant reader, ship a clearer "log has no header" error plus documented manual remedy in this PR, and optionally let the scheduler's existing half-init repair pass rename a headerless child log aside (keeping `ctx/`) so stranded batches heal on the next tick.
