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
  rest can ship before it.
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
session it keeps carries the reason, naming the successor or pin that keeps
it. A test author points `KOTO_HOME` at a scratch directory, confirms before
the first command that koto resolved a local store under scratch, and knows
the run can't reach the real store or bucket. A successor reads its
predecessor by name from its own directory and gets the current value, and
the predecessor is byte-identical afterwards. An agent that hits a mismatched
origin is told how to go on without deleting anything. Everything except the
name rule's replacement can ship before the continues relation does.

## User Stories

1. As an operator on a host whose koto runs on the cloud backend, I want a
   whole-store prune with a retention under one hour to stop and tell me
   which store it would hit, so that a mistyped command can't empty the
   store, and I want a confirmed run to remove only what retention releases.
2. As a test or QA author, I want to point `KOTO_HOME` at a scratch directory
   and check, without writing anything, that koto resolved a local backend
   under it, so that a run which opens, ticks, prunes and cleans up sessions
   can't touch the real store, request store, config or bucket.
3. As a coordinator's successor, I want a prune to keep my finished
   predecessor because I continue it, and to name me as the reason, while a
   finished `coordinate-` session nothing keeps is removed like any other, so
   that what keeps a session is a fact, not a name.
4. As an operator upgrading a host that has finished `coordinate-` sessions,
   I want to know before upgrading which of them the new rules would release
   and how to keep the ones a lineage still needs, so that the upgrade
   doesn't remove them by surprise.
5. As the same successor running in my own instance directory, I want to read
   a key my predecessor just changed by naming the predecessor and get the
   new value, so that I don't have to run every koto call from its directory.
6. As a lane that reads finished sessions, I want a read to leave the
   session's log, keys and bucket copy byte-identical, so that finished
   sessions can be read through koto again instead of from archives.
7. As an agent whose `koto init --attach-live` hits a session started in
   another worktree, I want the refusal to tell me a way forward that doesn't
   delete it, so that I don't destroy another workspace's run.

## Requirements

Terms used below. A session's **own copy** is the bucket copy under the
prefix koto recorded when the session was created (`origin.store.prefix`).
The **real store** is the `.koto` directory under the invoking user's home,
with its sessions, request store (`requests/`) and `config.toml`. A **read**
is `koto context get`, `koto context exists`, `koto context list` or
`koto status` naming a session. A **keeper** is what koto's retention rules
keep a session for: a pin, a live member of its hierarchy, its retention not
having passed, or, once the continues relation exists, a later session that
continues it.

### Removal guard (delivered by koto#470)

- **R1.** On the cloud backend, `koto session prune --apply` without
  `--plan` refuses when `retention.keep` or `retention.idle_period` is under
  one hour, the selection is `--all`, `--execution-dir` alone, or
  `--older-than` under one hour, and the judgment would remove at least one
  session. It exits 2 with code `prune_short_retention`, removes nothing
  locally or in the bucket, and its error names each short setting with its
  value and where it came from, the backend, the bucket, the store path and
  how many sessions would go. A retention of exactly one hour, an
  `--older-than` of an hour or more, `--apply --plan`, the local backend and
  the preview don't refuse.
- **R2.** The same command with `--confirm-short-retention` removes exactly
  the sessions no keeper holds, locally and in the bucket, and keeps every
  session a keeper holds.
- **R3.** Every prune apply that goes past the guard prints
  `koto session prune: removing from <store>` to stderr before its first
  removal.

### Isolation under `KOTO_HOME`

- **R4.** With `KOTO_HOME` set, koto selects the cloud backend only when the
  user config under that root (`$KOTO_HOME/config.toml`) sets
  `session.backend = "cloud"`. Neither `KOTO_SESSION_BACKEND` nor a project
  config selects it. Without that setting koto runs on the local backend
  under the root, and when an environment variable or a project config asked
  for the cloud backend it prints one warning to stderr naming the setting it
  ignored. Once the root's config selects cloud, the bucket, endpoint and
  credentials resolve from every layer as they do today.
- **R5.** `koto session dir --json`, with no session name, prints a JSON
  object with `root` (absolute path), `backend` (`local` or `cloud`),
  `backend_source` (`default`, `user-config`, `project-config` or `env`)
  and, for the cloud backend, `bucket`. It writes nothing to disk or to any
  bucket. The CLI guide documents the fields.
- **R6.** A run with `KOTO_HOME` at a scratch directory that opens, ticks,
  prunes and cleans up sessions leaves the real store byte-identical and
  makes no request to a bucket that only environment variables or a project
  config select.
- **R7.** Commands a tick runs receive the absolute `KOTO_HOME` root, so a
  koto they start resolves the same root and, through the same config file,
  the same backend as the ticking koto.

### What keeps a session

- **R8.** `koto session prune` and `koto workspace prune` no longer keep a
  session because its name starts with `coordinate-`. A finished session of
  any name with no keeper is removed once its retention passes.
- **R9.** A finished session that a later session continues is kept while the
  continues relation's retention rules keep it, on both backends, and its
  entry names the successor: in JSON through the relation's
  `relation_keepers` field and in text as `kept: continued by <successor>`,
  as the relation's feature defines them. A pinned session's entry still
  names its pin.
- **R10.** The name rule's removal merges only after the continues relation
  works on the cloud backend, so no release carries the removal without the
  relation. The release notes and the session retention guide say that
  finished `coordinate-` sessions are no longer kept by name, name
  `koto session pin` and `koto session link --continues` as the ways to keep
  one, and say that `koto session prune` without `--apply` previews which a
  prune would release.

### Reads by name

- **R11.** Under the cloud backend, a read of a session uses that session's
  own copy, wherever the command runs from, and returns the value the own
  copy holds at the time of the read, even when this host read the same key
  earlier and still holds a cached copy of it.
- **R12.** A session with no `origin.store.prefix` is located first from its
  execution anchor and then from the host's sync record. A location counts
  only after the bucket confirms that the copy there carries the same
  `session_id` as the host's local header. When no location is confirmed,
  when two are, or when the recorded location holds a copy with another
  `session_id` or none, the read exits non-zero with `session_unlocated`,
  names the session and the reason (and both locations when there are two),
  and prints no value. A host with no local copy of the session refuses the
  same way.
- **R13.** Writes to a named session (`koto context add` and `remove`, the
  events a tick appends, and a header rewrite such as a rebind's) go to the
  same own copy reads use, from any directory, and create no copy under the
  caller's prefix when it differs. This covers `koto next` run from a
  subdirectory of the session's anchor.
- **R14.** When a bucket request a read needs fails (a connection error, a
  timeout, an authorization error or a 5xx answer, on the listing or on any
  later fetch), a read run from a directory whose prefix isn't the session's
  own exits non-zero and prints no value. A read from a directory whose
  prefix is the session's own keeps answering from the local copy with a
  warning, as it does today.
- **R15.** `koto next` still runs only at or under the session's execution
  anchor, and refuses elsewhere with `execution_anchor_mismatch`.
- **R16.** A rebind moves the execution anchor and doesn't change the own
  copy: the bucket's object keys for the session are the same before and
  after, and a write from the new anchor creates no object under the new
  anchor's prefix.

### Reads write nothing

- **R17.** Outside the session's own tick, a read writes nothing into the
  session it reads: its state log, context key files and bucket objects are
  byte-identical before and after, whether the session is running or
  finished and whatever `KOTO_ANCHOR` the reader has. A read inside the
  session's own tick (a gate or action command of that tick) may still log a
  `context_read` event in the session, as today. Host-local files koto keeps
  beside a session and never pushes (the sync stamp, the listing and
  manifest caches, the read spool) don't count as the session.
- **R18.** A read after a transition's assigned-key store write was lost
  still returns the value the transition assigned, served from the session's
  log without storing it. The session's own next tick restores the key in
  its store.

### Refusal wording

- **R19.** The two `origin_mismatch` messages (a session from another
  worktree or store, and a session with no origin record) and the
  `execution_anchor_mismatch` message contain none of `session cleanup`,
  `--cleanup`, `session prune` or `workspace prune`. Both `origin_mismatch`
  messages name using another session name; the first also names the
  directory the session belongs to, and the second names finishing it with
  the koto version that started it. The `execution_anchor_mismatch` message
  keeps naming the bound directory and `koto session rebind`.
- **R20.** `docs/guides/cli-usage.md`, `docs/guides/custom-skill-authoring.md`,
  `docs/reference/error-codes.md` and the koto-user skill's references give
  the same remedies for both codes and name no removal for either.

### Non-functional

- **R21.** A read from a directory other than the session's own makes no more
  bucket requests than the same read from the session's own directory with
  the same cache state, plus at most one listing when the location came from
  R12's fallback.
- **R22.** Sessions created before koto recorded `origin.store.prefix` keep
  working from their own directory as they do today: the existing tests for
  those sessions pass unchanged.
- **R23.** R1 to R7 and R11 to R22 don't depend on the continues relation and
  may ship in a release before it. R8 to R10 wait for it.

## Acceptance Criteria

Tests drive the koto binary against the in-process bucket stand-in, which
logs every request and can be made to fail.

Guard:
- [ ] On the cloud backend, `koto session prune --apply` with
  `KOTO_RETENTION_KEEP=1s` and each of `--all`, `--execution-dir <dir>` and
  `--older-than 1s`, with no plan, exits 2 with `prune_short_retention`; the
  error names `retention.keep`, `1s`, its source, the backend, the bucket,
  the store path and the would-remove count; the local session directories
  and the bucket's object listing are identical before and after.
- [ ] `KOTO_RETENTION_KEEP=1h` with `--all`, and `KOTO_RETENTION_KEEP=1s`
  with `--older-than 1h`, don't refuse; neither do `--apply --plan <token>`
  nor the same command on the local backend.
- [ ] With `--confirm-short-retention`, on a store holding finished unpinned
  sessions past retention, a pinned finished session, and a finished session
  whose hierarchy has a running member, exactly the first group is removed,
  locally and in the bucket.
- [ ] In a confirmed run, stderr carries `koto session prune: removing from`
  before any removal takes place.

Isolation:
- [ ] With `HOME` at a sentinel directory holding a populated `.koto`
  (sessions, `requests/`, `config.toml`), `KOTO_HOME` at a scratch directory,
  and `KOTO_SESSION_BACKEND=cloud` plus the cloud variables pointing at the
  stand-in, a run of `koto init`, a tick, `koto session prune --apply --all
  --confirm-short-retention` and `koto session cleanup` makes no request to
  the stand-in, leaves the sentinel `.koto` hashing the same before and
  after, and prints one warning naming `KOTO_SESSION_BACKEND`.
- [ ] The same run with `session.backend = "cloud"` in
  `$KOTO_HOME/config.toml` reaches the stand-in; with that setting in a
  project config instead, it doesn't.
- [ ] Under that setup `koto session dir --json` prints `root` under scratch,
  `backend` `local` and `backend_source` `default`; with the root's config
  selecting cloud it prints `backend` `cloud`, `backend_source`
  `user-config` and the bucket. Neither run adds a file under scratch or an
  entry to the stand-in's request log.
- [ ] A gate command in a tick under a scratch `KOTO_HOME` runs
  `koto session dir --json` and prints the same `root`, `backend` and
  `bucket` as the ticking koto does.

What keeps a session:
- [ ] A finished `coordinate-x` with no keeper is removed by
  `koto session prune --apply` once retention passes, on both backends; a
  finished `coordinate-y` that is pinned, and one with a running hierarchy
  member, are kept and their entries name the pin or the member.
- [ ] A finished session whose successor holds a continues relation to it and
  is running is kept by `koto session prune --apply --all` and by
  `koto workspace prune`, on the local and the cloud backend, with
  `relation_keepers` naming the successor in JSON and
  `kept: continued by <successor>` in text; once the successor finishes and
  its retention passes, the next prune removes both.
- [ ] The release notes and `docs/guides/session-retention.md` name
  `koto session pin` and `koto session link --continues` where they describe
  the removed name rule, and no guide or skill reference describes a
  `coordinate-` name rule.
- [ ] The change that removes the name rule merges after the change that
  makes the continues relation work on the cloud backend.

Reads by name:
- [ ] Session S is created on the cloud backend in directory A. From an
  unrelated directory B, `koto context get S k` prints `v1`; a process in A
  then sets k to `v2`; the same read from B prints `v2`.
- [ ] `koto status S`, `koto context list S` and `koto context exists S k`
  from B print the same as from A.
- [ ] A session with no `origin.store.prefix`, whose copy under its anchor's
  prefix carries the matching `session_id`, reads from B. With that copy
  missing, or carrying another `session_id`, the read from B exits non-zero
  with `session_unlocated`, names S and the reason, and prints no value.
  With matching copies under two prefixes it refuses and names both.
- [ ] On a second store with no local copy of S, a read of S refuses with
  `session_unlocated`.
- [ ] From B, `koto context add S k2 v`, `koto context remove S k2` and a
  rebind's header rewrite go to S's own copy: A reads the results, and the
  stand-in holds no object for S under B's prefix.
- [ ] `koto next S` from a subdirectory of S's anchor leaves no object for S
  under the subdirectory's prefix, and a read from A sees the tick's event.
- [ ] With the stand-in failing the listing, and separately failing a GET
  after a good listing, `koto context get S k` from B exits non-zero and
  prints no value; from A it prints the local value and a warning.
- [ ] `koto next S` from a directory outside its anchor exits 2 with
  `execution_anchor_mismatch`.
- [ ] After `koto session rebind S --to C`, the stand-in's keys for S are the
  same as before, and a write from C adds no object under C's prefix.

Reads write nothing:
- [ ] For a finished session and for a running one, the state log, every
  context key file and every bucket object hash the same before and after
  `koto context get`, `exists`, `list` and `koto status` from A and from B,
  run with `KOTO_ANCHOR` unset and with it set to an anchor that holds the
  session.
- [ ] A gate in S's own tick that runs `koto context get S k` still logs a
  `context_read` event in S.
- [ ] With the store write of a transition's assigned key made to fail, the
  way koto#204's tests inject it, `koto context get S <key>` from another
  process prints the assigned value and S's files hash the same before and
  after; S's next tick leaves the key in its store.

Refusal wording:
- [ ] `koto init --attach-live` against a running session started in another
  worktree exits 2 with `origin_mismatch`, the session's files hash the same
  before and after, and the message names another session name and the
  directory the session belongs to.
- [ ] A test asserts that both `origin_mismatch` messages and the
  `execution_anchor_mismatch` message contain none of `session cleanup`,
  `--cleanup`, `session prune` and `workspace prune`.
- [ ] The four documents R20 names contain none of those strings in their
  entries for either code.

Non-functional:
- [ ] With the same cache state on both sides (cold, then warm), the
  stand-in's request log for `koto context get S k` from B has no more
  entries than from A, plus at most one listing for a session located
  through R12's fallback.
- [ ] The existing tests for sessions without `origin.store.prefix` pass
  unchanged.

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
- A caller that sets `KOTO_HOME` and wants the cloud backend has to write
  `session.backend = "cloud"` into the root's config, which is new friction
  for anyone who sets both on purpose.
- A session that exists only on another host, with no local copy here, can't
  be read by name; `--workspace` remains the way to read it.
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
the remaining work is tests the shipped ones don't cover (the other
selections at the floor's edge, a confirmed run over a mixed store, and the
local listing).

**A scratch `KOTO_HOME` stops environment-only cloud configuration.**
Alternatives: only report the resolved backend and let harnesses refuse
(cheapest, but every harness has to remember to check, and the incident was
a script that forgot a setup step); clear the variables in koto's own test
harness (already done for spawned commands, and no help to anyone else's
harness). Making koto fail closed under `KOTO_HOME` puts the protection where
it can't be forgotten. `KOTO_HOME` hasn't been in a release yet, so nobody
depends on combining it with an environment-only bucket. The report command
(R5) stays, so a harness can also assert it.

**The opt-in is the root's own config file.** Alternatives: a new environment
variable such as an allow-cloud flag (a host-wide setup could export it too,
and the incident's script inherited everything the host exported); a
command-line flag on every verb (every nested koto a tick starts would need
it). A `session.backend = "cloud"` line in `$KOTO_HOME/config.toml` can only be
written into the scratch root on purpose, and a koto that a tick starts reads
the same file because it receives the same root. Credentials keep coming from
the environment, so no secret has to be written into the root.

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
