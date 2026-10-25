---
schema: prd/v1
status: Draft
problem: |
  On a host where many workflows share one koto store and one bucket, koto's
  removals and reads can't be trusted enough to use: a short-retention bulk
  prune emptied a real store on 2026-10-24, a test confined to a scratch root
  can still reach the bucket an inherited environment configures, prune keeps
  sessions by a name prefix rather than by what still needs them, a read by
  name depends on the directory it runs from and can write into the session
  it reads, and the refusal for a session from another worktree advises
  deleting it. The host runs under a freeze until these hold.
goals: |
  A store-wide removal can't take a whole store or a session a live lineage
  needs, and every session it keeps names its keeper; a run confined by
  KOTO_HOME can't touch the real store or bucket; a session is read by name
  from any directory and returns its current value or refuses, never a stale
  copy, and the read leaves it unchanged; no refusal advises deleting a
  session. The part that needs the continues relation waits for it, and the
  rest can ship first.
upstream: docs/briefs/BRIEF-koto-trusted-removal.md
---

# PRD: Removal a lineage can trust

## Status

Draft

## Problem Statement

koto keeps every workflow session on a host in one local store, and under the
cloud backend in one bucket that several hosts can share. Sessions form
lineages: a coordinator's run continued by a successor, an imported copy and
its source, a parent and its children. People, maintenance scripts, test
harnesses and other agents remove and read sessions on that host, and each
needs to know the command can't destroy what other work still depends on and
that a read returns the truth without changing anything.

That broke on 2026-10-24. A test script meant for a scratch store ran
`koto session prune --apply --all` with a one-second retention against a
host's real store. The cloud backend came from inherited environment
variables, so a scratch home didn't isolate it, and koto removed every
finished session no keeper held, bucket copies included: about 700 sessions.
The host has run under a freeze on prune, cleanup, import and delete since,
and finished sessions are read from archives rather than through koto.

Part of the fix is on main. A short-retention bulk removal on the cloud
backend now refuses unless confirmed and prints the store first (koto#470).
Every koto path resolves from one root that `KOTO_HOME` moves, and
`koto session dir` prints it (koto#471). A read from another directory refuses
instead of returning a stale copy (koto#463), and a pull no longer drops
events (koto#480). What's left:

- A scratch `KOTO_HOME` doesn't stop a cloud backend that the environment
  alone configures, and nothing tells a harness which backend koto resolved.
- Prune keeps every session whose name starts with `coordinate-`, forever,
  whether or not anything needs it, while a session a live successor depends
  on has no protection unless its name happens to match.
- A successor can't read its predecessor from its own directory: the read
  refuses. A tick from a subdirectory of its anchor still writes a stray copy
  under that subdirectory's bucket prefix (koto#465, koto#466).
- A read can still write into the session it reads: after a lost store write
  it restores the assigned value into that session (koto#436), and a read the
  session counts as its own appends a `context_read` event and pushes it.
- The `origin_mismatch` refusal tells the caller to remove the other
  workspace's session with `koto session cleanup`.

## Goals

An operator who runs a store-wide removal on a shared host can't empty the
store by accident or take a session a live lineage still needs, and every
session it keeps carries the reason, naming the successor that keeps it. A
test author points `KOTO_HOME` at a scratch directory, confirms before the
first command that koto resolved a local store under scratch, and knows the
run can't reach the real store or bucket. A successor reads its predecessor
by name from its own directory and gets the current value, and the
predecessor is byte-identical afterwards. An agent that hits a mismatched
origin is told how to go on without deleting anything. Everything except the
name rule's replacement ships before the continues relation does.

## User Stories

1. As an operator on a host whose koto runs on the cloud backend, I want a
   whole-store prune with a test-sized retention to stop and tell me which
   store it would hit, so that a mistyped command can't empty the store, and
   I want a confirmed run to remove only what retention releases.
2. As a test or QA author, I want to point `KOTO_HOME` at a scratch directory
   and check, without writing anything, that koto resolved a local backend
   under it, so that a run which opens, ticks, prunes and cleans up sessions
   can't touch the real store, request store, config or bucket.
3. As a coordinator's successor, I want a prune to keep my finished
   predecessor because I continue it, and to name me as the reason, while a
   finished `coordinate-` session nothing continues is removed like any
   other, so that what keeps a session is a fact, not a name.
4. As the same successor running in my own instance directory, I want to read
   a key my predecessor just changed by naming the predecessor and get the
   new value, so that I don't have to run every koto call from its directory.
5. As a lane that reads finished sessions, I want a read to leave the
   session's log, keys and bucket copy byte-identical, so that finished
   sessions can be read through koto again instead of from archives.
6. As an agent whose `koto init --attach-live` hits a session started in
   another worktree, I want the refusal to tell me a way forward that doesn't
   delete it, so that I don't destroy another workspace's run.

## Requirements

### Removal guard (delivered by koto#470)

- **R1.** On the cloud backend, `koto session prune --apply` without
  `--plan`, with a whole-store selection and a retention under the floor,
  exits non-zero, removes nothing locally or in the bucket, and names the
  short retention setting and its value, the backend, the bucket and the
  store path. The floor, the selections it counts as whole-store and the
  confirmation flag are those koto#470 shipped.
- **R2.** The same command with the explicit confirmation removes exactly
  the sessions the retention rules release and keeps every session a keeper
  holds.
- **R3.** Every prune apply prints the store it acts on before it removes
  anything.

### Isolation under `KOTO_HOME`

- **R4.** With `KOTO_HOME` set, koto reaches no bucket that only inherited
  environment variables configure. The cloud backend is used under a
  `KOTO_HOME` root only when the caller opts in explicitly; the opt-in is the
  design's choice, and it must not be any variable that a host-wide cloud
  setup already exports.
- **R5.** A read-only command reports the resolved root, the backend kind,
  and for the cloud backend the bucket and the source of its configuration,
  in a machine-readable form, and writes nothing. A harness can check it
  before running anything else.
- **R6.** A run with `KOTO_HOME` at a scratch directory that opens, ticks,
  prunes and cleans up sessions leaves the real store, the request store, the
  config and the bucket unchanged.
- **R7.** Commands a tick runs keep receiving the root and the backend the
  ticking koto resolved, so a koto they start stays in the same root and on
  the same backend.

### What keeps a session

- **R8.** Prune no longer keeps a session because its name starts with
  `coordinate-`. A finished session of any name, with no continues relation
  naming it, no pin and no live relative, is removed once retention passes.
- **R9.** A finished session that a later session continues is kept while the
  relation's rules keep it, on both backends, and the prune names the
  successor that keeps it. This uses the continues relation and its retention
  rules as the relation's own feature defines them, unchanged.
- **R10.** The name rule's removal ships in a koto release that also carries
  the continues relation on the cloud backend, and the release notes and the
  session retention guide say what changes for existing `coordinate-`
  sessions and how to keep one (a pin, or a `koto session link --continues`
  from its successor) before upgrading.

### Reads by name

- **R11.** Under the cloud backend, a command that names a session reads that
  session's own bucket copy, wherever the command runs from. A read run from
  any directory returns the session's current value: a key another process
  just changed reads as the new value.
- **R12.** The session's own copy is found from the record koto keeps of where
  the session was created. When that record is missing, koto may use other
  evidence of the session's location only after confirming in the bucket that
  the copy found is the same session. When the location can't be established,
  or two locations both hold it, the command refuses and says which session
  and why, and never answers from a guess.
- **R13.** Writes to a named session (an event, a context key, a header
  rewrite) go to the same copy reads use, from any directory. A tick from a
  subdirectory of the session's anchor no longer creates a copy under another
  prefix.
- **R14.** When the bucket can't be reached, a read from outside the session's
  own workspace refuses rather than answering from the host's local copy. A
  read from inside its own workspace keeps today's behaviour.
- **R15.** The tick rule is unchanged: `koto next` still runs only at or under
  the session's execution anchor.
- **R16.** A rebind moves the session's anchor and doesn't move the copy reads
  and writes use.

### Reads write nothing

- **R17.** A read (`koto context get`, `exists`, `list`, `koto status`) of a
  session leaves that session's state log, context keys and bucket copy
  byte-identical, unless the read runs inside that session's own tick.
  Host-local caches koto keeps beside a session (the sync stamp, the listing
  and manifest caches, the read spool) don't count as the session.
- **R18.** A read after a transition's lost store write still returns the
  value the transition assigned, served from the session's log, and the lost
  write is restored only by the session's own next command.

### Refusal wording

- **R19.** Neither `origin_mismatch` nor `execution_anchor_mismatch` names
  `koto session cleanup`, `koto cancel --cleanup` or any other removal as a
  remedy. Each names a way forward that removes nothing.
- **R20.** The docs and the koto-user skill describe the same remedies and
  don't name a removal for either code.

### Non-functional

- **R21.** A read by name from another directory makes no more bucket requests
  than the same read from the session's own directory, plus at most one
  listing to confirm a location found without the record.
- **R22.** Sessions created before koto recorded where they were created keep
  working from their own directory exactly as today.
- **R23.** Every requirement above that doesn't depend on the continues
  relation (R1 to R7 and R11 to R22) can ship in a release before that
  relation does. R8 to R10 wait for it.

## Acceptance Criteria

- [ ] On the cloud backend (tests use the in-process bucket stand-in),
  `koto session prune --apply --all` with `KOTO_RETENTION_KEEP=1s` and no
  plan exits non-zero; its error names the retention value and source, the
  backend, the bucket and the store path; the local store listing and the
  bucket listing are identical before and after.
- [ ] The same command with the confirmation flag, on a store holding both
  sessions the retention releases and sessions a pin or a live relative
  keeps, removes exactly the released ones, locally and in the bucket.
- [ ] Each prune apply's stderr carries the store line before any removal
  line.
- [ ] With `KOTO_HOME` at a scratch directory and the cloud backend configured
  only through `KOTO_SESSION_*` variables pointing at a stand-in bucket, a run
  that opens, ticks, prunes and runs `koto session cleanup` makes no request
  to that bucket, and the scratch `HOME`'s `.koto`, its request store and
  config are byte-identical before and after.
- [ ] The same run with the design's explicit opt-in reaches the stand-in
  bucket.
- [ ] The read-only report command, run with `KOTO_HOME` at scratch and cloud
  variables in the environment, prints a local backend and a root under
  scratch, and the scratch directory holds no new file afterwards.
- [ ] A gate command in a tick under a scratch `KOTO_HOME` sees the same root
  and resolves the same backend as the ticking koto.
- [ ] A finished session named `coordinate-x`, with no relation, no pin and no
  live relative, is removed by `koto session prune --apply` once retention
  passes; its entry names no keeper.
- [ ] A finished session whose successor holds a continues relation to it and
  is running is kept by `koto session prune --apply --all`, on the local
  backend and on the cloud backend, and the entry names the successor.
- [ ] The session retention guide and the CLI guide no longer describe a
  `coordinate-` name rule, and describe how to keep an existing coordinator
  session before upgrading.
- [ ] On the cloud backend, session S is created in directory A; another
  process run from A changes key k; `koto context get S k` run from an
  unrelated directory B prints the new value.
- [ ] `koto status S` and `koto context list S` from B match the same commands
  from A.
- [ ] A session with no recorded creation location, whose copy can be located
  and confirmed by session id, reads from B; one whose location can't be
  confirmed refuses from B with a message naming the session and the reason,
  and prints no value.
- [ ] A session whose name exists under two locations that both carry its
  session id refuses from B and names both.
- [ ] `koto context add S k2 v` from B is read back as `v` from A, and the
  bucket holds no copy of S under B's prefix.
- [ ] `koto next S` run from a subdirectory of S's anchor leaves no copy of S
  under the subdirectory's prefix, and the next read from A sees the tick's
  event.
- [ ] With the bucket unreachable, `koto context get S k` from B exits
  non-zero and prints no value; from A it behaves as before this change.
- [ ] `koto next S` from a directory outside its anchor still refuses with
  `execution_anchor_mismatch`.
- [ ] After `koto session rebind S --to C`, reads from B and writes from C use
  the same bucket copy as before the rebind.
- [ ] A finished session's state log, context keys and bucket objects hash the
  same before and after `koto context get`, `koto context exists`,
  `koto context list` and `koto status` run from its own directory and from
  another one, with and without `KOTO_ANCHOR` set to an anchor that holds it.
- [ ] After a transition's assigned-key store write is made to fail, a read of
  that key from another process returns the assigned value and the session's
  files hash the same before and after the read; the session's next tick
  restores the key in its store.
- [ ] A test asserts that the `origin_mismatch` messages (no origin record,
  and another worktree or store) and the `execution_anchor_mismatch` message
  contain neither `session cleanup` nor `--cleanup`, and that each names a
  remedy that removes nothing.
- [ ] `docs/guides/cli-usage.md`, `docs/guides/custom-skill-authoring.md`,
  `docs/reference/error-codes.md` and the koto-user skill name no removal as
  the remedy for either code.
- [ ] A read by name from B makes no more bucket requests than the same read
  from A, plus at most one listing when the location was found without the
  record, counted with the stand-in's request log.
- [ ] A session created by an older koto, read from its own directory,
  behaves exactly as before.

## Out of Scope

- The continues relation itself: the verb that writes it, how retention counts
  it, the bucket walk and import keeping its source belong to the feature that
  adds the relation (koto#479, koto#481). This PRD consumes it unchanged.
- A coordinator's successor writing the relation at takeover, and lineage
  scripts reading it. That's the workflow side of succession, outside koto's
  removals.
- Lifting the host-wide freeze. That's the host owner's decision, made after
  this ships and a prune preview on the real store names a keeper for every
  live lineage.
- A guard on the local backend or on marker-mode prune (`--migrated-from`).
  koto#470 scoped the guard to removals that also delete bucket copies.
- Explicit single-session removal (`koto session cleanup <name>`,
  `koto cancel --cleanup`); a pin already refuses it.
- Finding a session by name on a host that has no local copy of it. Such a
  read refuses (R12) and `--workspace` already reads another workspace's
  session without importing it.
- Removing or merging stray copies earlier per-directory pushes left under
  other prefixes. Reads and writes stop using them; cleaning them up is
  separate work.
- Other cross-session writers outside the read path (a context add into
  another session, a child's completion event in its parent's log).
- A prune in an old workspace deleting a same-host import's moved copy
  (koto#473), whose fix is the import's local guard.
- Restoring the sessions the 2026-10-24 prune removed; that recovery is done.

## Known Limitations

- Copies left under other prefixes by older per-directory pushes stay in the
  bucket. They may hold events the session's own copy lacks; nothing reports
  them after this change.
- A session created before koto recorded its creation location and never
  confirmed in the bucket can still only be read from its own directory.
- Under the cloud backend with `KOTO_HOME` set, a caller that wants the cloud
  backend has to opt in explicitly, which is new friction for anyone who sets
  both on purpose.
- Removing the name rule makes finished `coordinate-` sessions with no
  relation removable after retention. A lineage whose successor hasn't
  written the relation loses its predecessor unless an operator pins or links
  it first.

## Decisions and Trade-offs

**The guard is delivered, not re-specified.** koto#470's floor (one hour),
its whole-store selections (`--all`, `--execution-dir` alone, a short
`--older-than`), cloud-only scope and `--confirm-short-retention` already meet
the first user story. Alternatives: re-open the floor's value, extend the
guard to the local backend or to marker mode. The guard exists because the
bucket deletes can't be undone and local removals on a host have archives;
the remaining work is two tests the shipped ones don't cover.

**A scratch `KOTO_HOME` stops environment-only cloud configuration.**
Alternatives: only report the resolved backend and let harnesses refuse
(cheapest, but every harness has to remember to check, and the incident was
a script that forgot a setup step); clear the variables in koto's own test
harness (already done for spawned commands, and no help to anyone else's
harness). Making koto fail closed under `KOTO_HOME` puts the protection where
it can't be forgotten. `KOTO_HOME` hasn't been in a release yet, so nobody
depends on combining it with an environment-only bucket. The report command
(R5) stays, so a harness can also assert it.

**The name rule goes only with a cloud-capable relation.** Alternatives:
remove it as soon as the relation lands on the local backend; keep it as a
fallback beside the relation. The local-only relation can't keep a
coordinator on a cloud host, and every coordinator on the affected host runs
on the cloud backend, so removing the rule first would release them with
nothing in its place. Keeping the rule beside the relation keeps the defect
the feature exists to remove: a session kept because of its name.

**Reads resolve the session's own copy rather than refusing.** koto#463 chose
to refuse because sessions didn't yet record where they were created, so
resolving by name would have rested on a guess. They do now. Alternatives:
keep refusing (the successor can't read its predecessor, the workaround
stays); resolve by guessing from the anchor without confirmation (the silent
stale read koto#463 removed). The chosen rule resolves from the record, falls
back only to locations confirmed by session id in the bucket, and refuses
otherwise.

**Writes follow reads to the same copy.** Resolving reads without moving
writes would let a successor's write into its predecessor land under the
successor's prefix, a second copy that a read never sees. Resolving both from
one location also closes koto#465 and koto#466.

**Host-local caches don't count toward "byte-identical".** The sync stamp,
listing and manifest caches and the read spool live beside a session on one
host, are never pushed and record what this host has seen. Counting them
would force every read through a scratch store and give up the host cache.
What a lineage relies on is the session's log, keys and bucket copy.

**The anchor-mismatch wording already qualifies.** `execution_anchor_mismatch`
names running from the anchor and `koto session rebind`, neither a removal.
It gets a test that keeps it that way, not a rewrite.
