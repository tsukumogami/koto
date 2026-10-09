---
schema: prd/v1
absorbed: docs/briefs/BRIEF-session-migration.md
status: Done
source_issue: 313
problem: |
  koto's S3 remote stores a session but can't move one. Each session lives
  under a prefix derived from the workspace that created it, `rebind` is
  undone under the cloud backend, context keys never leave the first
  workspace's prefix, and a pull onto a host with no session directory
  fails. Operators who need to continue a session on another host have no
  working path, and the guide tells them one exists.
goals: |
  An operator imports a session from its remote object onto another host or
  workspace with one command, state log and every context key intact, and
  advances it there. The source object records that the session moved and
  where, and every koto command that reads it refuses instead of continuing
  it. A shipped harness re-runs that move end to end, and the guide
  describes it accurately.
---

# PRD: Session migration

## Status

Done

Absorbed [BRIEF](docs/briefs/BRIEF-session-migration.md); carried in Absorbed Brief.

## Absorbed Brief

**Why this exists.** koto's S3 remote stores a session but can't move one.
Each session lives under a prefix derived from the workspace that created
it, `rebind` is undone under the cloud backend, context keys never leave
the first workspace's prefix, a host with no session directory can't take a
pull, and the guide says a second machine resumes by running `koto next`.
The maintainers' model (koto#313) is a migration: import into a fresh local
session, mark the old object.

**The outcome.** An operator continuing a session elsewhere brings it over
with one command, state log and every context key intact, and keeps working
there. Any command on the first host that reads it says where it went, so
nothing there continues it by accident.

**The journeys.** An operator moves a coordinator's session to a second
host; an agent on the first host touches the migrated session and is
refused; a maintainer re-runs the carrier harness after changing the cloud
backend; a first-time user sets up cloud sync, including a self-hosted
MinIO, from the guide.

**The boundary.** In: the import verb, the migrated marker and its refusal,
what an imported session needs to run (anchor, template, name), the
terminal-cleanup interaction, the state log's safety during an import, a
harness, and the guide and design corrections. Out: a session shared live
between hosts, stopping an offline original, requests, wakes and legs,
child sessions, listing and pruning, a context view, callers above koto,
and any remote but koto's own bucket.

## Problem Statement

koto can keep its sessions in any S3-compatible bucket, and the guide
presents that as a way to resume work on another machine. It isn't one. The
remote keys each session under a prefix computed from the creating
workspace's working directory, which is the session's identity on its host
and stays that way (koto#309 was closed as intended), so a second host or a
second workspace looks under its own prefix and finds nothing.

The only command that moves a session, `koto session rebind`, appends its
event through the cloud backend and then rewrites the header outside it, so
the next read pulls the old header back and `koto next` keeps refusing on
the old anchor (koto#310). It moves the state file only: the context keys,
where a long-running coordinator keeps its workers, holdings and standing
answers, stay under the first workspace's prefix. A host with no local
session directory can't take a pull at all. Underneath, a non-failure
terminal tick deletes the session's remote prefix, session names are
machine-wide, and the first event records the compiled template's absolute
path in the origin host's cache, which another host doesn't have.

Nothing stops two hosts from advancing one session either. A state pull
overwrites the local file whole with no version check (the version counter
guards context writes only), and state-log writes take no lock (koto#239).
The guide (`docs/guides/cloud-sync-setup.md`) covers none of this, documents
a flag and a config key that don't exist, gives a MinIO example that can't
work, and lists three of the eight commands that reach the remote
(koto#312).

The maintainers' model, recorded in koto#313, is that a session is never
shared live across hosts. It is migrated: imported from its S3 object into a
fresh local session on the new host, which moves forward under its own
object, while the old object is marked migrated with a pointer to the new
one. If the original resumes on a host that never saw the marker, that is an
accepted risk; the marker is the only lock.

## Goals

- An operator continuing a session elsewhere brings it over with one
  command and keeps working; the session they get holds the original's state
  log and every context key, byte for byte, and runs where they brought it.
- Nobody on the first host continues a migrated session by mistake: every
  koto command there that reads it stops and says where it went.
- A maintainer can tell whether a session still survives a move by running
  one harness, without hand steps.
- A user setting up cloud sync from the guide gets commands that exist, an
  endpoint configuration that works for self-hosted S3, and an accurate
  account of how sessions move.

## User Stories

- As an operator retiring the host a coordinator session runs on, I want to
  import that session on a new host from the shared bucket, so that the
  coordinator's state and every key it stored arrive intact and its next
  `koto next` advances from where the old host stopped.
- As an agent on the first host that never heard about a move, I want any
  command I run against the migrated session to refuse and name the newer
  session, so that two copies of the session never advance in parallel.
- As an operator importing a session whose name is already taken on my host,
  I want the import to refuse and tell me how to pick another name, so that
  it never overwrites a session I already have.
- As a koto maintainer changing the cloud backend, I want a harness that
  moves a session between two simulated hosts and reports pass or the step
  that failed, so that a regression in the carrier shows up in CI rather
  than in someone's handover.
- As a first-time user pointing koto at a self-hosted MinIO, I want the guide
  to show flags and keys that exist and an endpoint setup that works with an
  IP address, so that setup doesn't send me down a dead end.

## Requirements

Terms used below. A session's **remote object** is everything the cloud
backend stores under `<prefix>/<name>/` in the bucket, where `<prefix>` is
derived from the creating workspace's absolute path. A session's **execution
anchor** is the directory its ticks run in (the header's `execution_dir`);
its **origin record** names that anchor and the session store that holds it;
its **command-environment record** holds the `PATH`, `HOME` and
`XDG_CONFIG_HOME` its commands run with. The **source** is the session being
imported; the **target** is the session the import creates.

### Functional

**R1. Import verb.** koto provides a session subcommand that imports one
root session from its remote object into a fresh local session on the host
where it runs. The user names the session and the source workspace by the
absolute path it had on the host that created the session. The import works
when that path doesn't exist on the importing host. The target is anchored
in the directory the import runs in.

**R2. Fresh local session.** The import creates the local session directory
itself, and succeeds on a host that has never held the session.

**R3. Complete carry.** The target holds every event of the source's state
log, in order and unchanged, and every context key listed in the source's
remote manifest, with byte-identical content and the same size, hash and
writer. A key whose content doesn't match the manifest's hash, or a manifest
entry with no content object, makes the import refuse. The import appends
one event recording the source workspace path, source session name, source
session id and the importing machine's id.

**R4. Anchored where it runs.** The target's header names the import
directory as execution anchor and origin anchor, this host's session store
in the origin record, a new session id, and a command-environment record
taken on this host the way `koto init` takes one. A `koto next` from the
import directory advances the target without a `rebind`.

**R5. Template available.** After the import, the target's compiled template
is readable on this host. If no copy whose SHA-256 matches the source's
recorded template hash can be found, the import refuses, naming the hash and
how to supply the template.

**R6. Its own remote object.** The import pushes the target's state file,
every context key, its manifest, a version record new to this host, and the
compiled template under the importing workspace's prefix, so the target is
itself importable from there.

**R7. Migrated marker.** After R6 succeeds, the import writes a marker into
the source's remote object naming the target session, the importing
workspace's path, the importing machine's id and the time. The import writes
nothing else under the source's prefix.

**R8. Refusal on a migrated source.** Every read of a session's state or
context through the cloud backend checks for the marker. When it is present,
the command exits non-zero with the error code `session_migrated` and a
message naming the target session and its workspace path, and leaves the
local copy unchanged. `koto session list`, `koto session cleanup` and
`koto session resolve` don't read a session's state or context and are
exempt. When the marker check can't reach the remote, the
command proceeds on its local copy with a warning, the way every other sync
failure does today.

**R9. Refusals.** The import refuses, with the code listed and with no local
directory and no remote object left behind, when:

| Condition | Code |
|---|---|
| The configured backend isn't the cloud backend | `import_requires_cloud` |
| The source's remote object has no state file | `import_source_not_found` |
| The source already carries a marker (the message names its target) | `import_source_migrated` |
| The source's header names a parent workflow | `import_source_is_child` |
| The source's state file has a schema version this koto doesn't read | `import_source_unreadable` |
| A context key fails R3's check | `import_source_unreadable` |
| The target name exists on this host or under this workspace's prefix | `import_name_taken` |
| R5 fails | `import_template_unavailable` |
| The push in R6 fails | `import_push_failed` |

**R10. Name collision and rename.** `import_name_taken` names an option that
imports under a different name the user picks. Under it the target's header
carries the new name, locally and in the remote object, and the marker names
the new name. Importing into the source's own workspace under a new name is
allowed.

**R11. Failure and retry.** A failure before R6 completes leaves no local
directory and removes whatever the import pushed. When the marker write in R7
fails, the import exits non-zero with `import_unmarked` and keeps the
complete target; running the same import again completes the marker without
re-importing. A target whose import event names the same source session id
counts as this retry, not as `import_name_taken`.

**R12. Stopped-source rule.** Until a state-log lock covers every writer
(koto#239), an import is defined only for a source that no process is
advancing and whose last write reached the remote. The import reads the
source's remote object and never writes the source's state file or context
keys. The verb's help text and the guide state the rule and how to make
sure the last write was pushed.

**R13. Anchor rewrites persist under the cloud backend.** `rebind` and the
first tick's anchor adoption both reach the remote object, so the next read
doesn't restore the old header (koto#310).

**R14. Markers outlive cleanup.** No koto command deletes a marker. A
terminal tick or `koto session cleanup` on a migrated source removes its
local copy and every other remote object under its prefix and leaves the
marker. A source that reached a non-failure terminal tick before import has
no state file left, and the import refuses it as `import_source_not_found`.

**R15. Measurement harness.** koto ships an integration test that, against
an S3-compatible endpoint, starts a session in workspace A, stores context
keys, imports it into workspace B with its own home, template cache and
session store (a simulated second host), advances it in B, compares every key
byte for byte, and checks that `koto next` in A refuses with
`session_migrated`. Each step fails the test with the step's name. It runs in
the CI unit-test job against an endpoint the test starts itself, and against
a real bucket when the existing cloud-integration secrets are present.

**R16. Path-style endpoints.** A config key makes the cloud backend address
the bucket path-style, so an endpoint given as `http://<ip>:<port>` (a
self-hosted MinIO) works for every remote operation. It is allowed in
project config.

**R17. Guide.** `docs/guides/cloud-sync-setup.md` documents moving a session
as an import with the stopped-source rule and the error codes in R8 and R9;
doesn't present `rebind` or a second machine running `koto next` as the way to
continue a session elsewhere; drops the `--project` flag and the
`allow_insecure` key; documents the path-style key; and lists the commands
that reach the remote: `init`, `next`, `status`, `context add`, `context get`,
`context list`, `context exists`, `context remove`, `session list`,
`session rebind`, `session cleanup`, `session resolve` and `session import`.

**R18. Designs and skills.** `DESIGN-config-and-cloud-sync.md` and
`DESIGN-backend-state-persistence.md` each carry a note dated with the day
it lands, naming `koto session import` or this PRD, where this feature
changes what they say. The koto-user skill tells agents to import a session
that moved machines instead of rebinding it, and documents the import verb,
its codes and `session_migrated`.

### Non-functional

**R19. Cost of the marker check.** A command on a non-migrated cloud session
makes at most one more remote request than the same command made before this
feature, however many times it reads the session. A command on the local
backend makes no marker check.

**R20. Bounded import.** An import makes a number of remote requests linear
in the number of context keys, and the harness's import step finishes in
under 30 seconds against its local endpoint.

**R21. Public-safe output.** Error messages, import output and the marker
contain no access key, secret key or credential value.

## Acceptance Criteria

Unless stated, each criterion runs against an S3-compatible endpoint the
test starts, with workspaces A and B on separate homes, template caches and
session stores.

- [ ] Importing a session from A into B, where A's path doesn't exist on B
  and B has never held the session, creates a local session whose state log
  holds every source event in order plus one import event carrying A's path,
  the source name, the source session id and B's machine id (R1, R2, R3).
- [ ] Every context key arrives with the source's SHA-256, size and writer;
  a source with zero keys imports too; a key altered in the bucket after its
  manifest entry was written makes the import refuse with
  `import_source_unreadable` (R3, R9).
- [ ] The target's header names B's import directory as execution and origin
  anchor, B's session store, a session id different from the source's, and
  B's `HOME` in its command-environment record; `koto next` from B advances
  it (R4).
- [ ] With the template compiled in B's cache, the import uses it and the
  target's next tick loads it although A's cache path doesn't exist on B;
  with no acceptable copy, the import refuses with
  `import_template_unavailable` naming the hash (R5).
- [ ] After the import, B's prefix holds the state file, every key, the
  manifest, a version record and the template, and importing from B into a
  third workspace C succeeds (R6).
- [ ] The marker under A's prefix names the target, B's path, B's machine id
  and a timestamp; a recording endpoint shows the import made no PUT or
  DELETE under A's prefix other than the marker (R7, R12).
- [ ] After the import, each of `koto next`, `koto status`, `koto context add`,
  `koto context get`, `koto context list`, `koto context exists`,
  `koto context remove` and `koto session rebind` against the source exits
  non-zero with `session_migrated` and B's session and path in the message,
  and A's local state file is byte-identical before and after (R8).
- [ ] With the endpoint stopped, `koto status` on a session proceeds on its
  local copy and prints a warning (R8).
- [ ] Each row of R9's table has a test asserting its code and that no local
  directory and no object under B's prefix remain afterward (R9, R11).
- [ ] An import whose target name exists locally refuses with
  `import_name_taken`; repeating it with the rename option succeeds, the
  target's local and remote headers carry the new name, and the marker names
  it (R10).
- [ ] With the endpoint made to fail the marker PUT, the import exits with
  `import_unmarked` and the target exists; re-running the same import writes
  the marker and creates nothing new (R11).
- [ ] `koto session import --help` states the stopped-source rule (R12).
- [ ] On the cloud backend, after `koto session rebind`, and after a first tick
  that adopts an anchor, the remote state file's header names the new anchor
  and `koto next` from it proceeds (R13).
- [ ] `koto session cleanup` on a migrated source, and a terminal tick of the
  target, each leave every marker in the bucket in place; importing a source
  whose terminal tick removed its state file refuses with
  `import_source_not_found` (R14).
- [ ] The harness test runs in the CI unit-test job, exercises every step R15
  names, and fails with the step's name when a step fails (R15).
- [ ] The harness runs with the endpoint set to `http://127.0.0.1:<port>` and
  the path-style key on, covering init, push, pull, context add and get,
  list, cleanup, import and the marker check; setting the key in project
  config is accepted (R16).
- [ ] `grep` finds in the guide no `--project`, no `allow_insecure`, no
  sentence saying a second machine picks up by running `koto next`, and no
  instruction to `rebind` onto another machine; it finds `session import`,
  the path-style key, every code in R8 and R9, and each of the thirteen
  commands R17 lists (R17).
- [ ] Each named design contains a dated note mentioning `koto session import`;
  the koto-user skill contains `koto session import` and `session_migrated`
  and no longer tells agents to rebind a session that moved machines (R18).
- [ ] Against a recording endpoint, `koto status` and `koto next` on a
  non-migrated session each make exactly one more request than on the
  commit before this feature; on the local backend no request is made (R19).
- [ ] Against a recording endpoint, an import of a session with N keys and
  one with 2N keys differ in request count by a constant times N, and the
  harness's import step takes under 30 seconds (R20).
- [ ] With sentinel values as the access and secret keys, no import output,
  error message or marker contains either value (R21).

## Out of Scope

- **A session shared live between hosts.** Two hosts never advance one
  session; this feature moves a session, it doesn't synchronize one.
- **Preventing an offline original from resuming.** The maintainers accepted
  that risk; the marker is a flag, not a distributed lock.
- **The state-log lock itself (koto#239).** A single-host concurrency defect
  the import doesn't depend on once the stopped-source rule holds; see
  Decisions and Trade-offs.
- **Requests, wakes, request legs and the decider ledger.** They don't
  replicate today and the import doesn't carry them. A session bound to a
  request leg arrives unbound.
- **Child sessions.** The import moves one root session; its children, and a
  batch parent's children, are not carried.
- **Finding a source by name alone across the bucket.** The user names the
  source's workspace.
- **Following a chain of migrations, or clearing a marker.** A session
  migrated A to B to C leaves A's marker naming B; koto doesn't follow it, and
  no command removes a marker.
- **Listing by execution directory, pruning, and a terminal signal**
  (koto#308, koto#162, koto#234).
- **A human-readable view of context keys and their contents.**
- **How a caller above koto decides when to import.**
- **Any remote other than koto's self-managed S3 bucket.**

## Known Limitations

- The marker is checked only when a command reaches the remote. A host that
  is offline, or whose remote is unreachable, advances its local copy with a
  warning; the session then forks, and nothing reconciles the two copies. A
  few readers never reach the remote (the dashboard, `koto session recover`,
  and the decider's local reads); they show a migrated source's local copy
  without refusing, and none of them advances it.
- Two hosts importing the same unmarked source at the same moment both
  succeed, and the later marker wins. The stopped-source rule makes this an
  operator error rather than a race koto prevents.
- Until koto#239 lands, two processes writing one session on one host can
  still corrupt its log. The stopped-source rule keeps the import from making
  that worse; it doesn't fix it.
- A session bound to a request leg loses the binding on import, so a caller
  waiting on that leg waits until it notices the move.

## Decisions and Trade-offs

**Migration, not sharing.** Considered: making the remote prefix independent
of the workspace so any host could open the same session. Chosen: the
maintainers' model in koto#313, an import into a fresh session plus a marker.
Sharing would need a cross-host lock koto doesn't have, and the prefix is the
session's identity on its host by decision (koto#309).

**The user names the source's workspace path.** Considered: scanning every
prefix in the bucket for a session by name, or asking for the raw prefix.
Chosen: the absolute path the source workspace had, which the user knows and
the source's own header records. A scan reads every prefix and two
workspaces can hold the same name; a raw prefix is a hash nobody can read.

**Anchored where the import runs.** Considered: an option naming another
directory. Rejected for now: the remote prefix is computed from the working
directory, so a target anchored elsewhere would be pushed under a prefix its
own ticks never read.

**Collision refuses; rename is opt-in.** Considered: overwriting, or silently
picking a new name. Chosen: refuse and offer a rename option, because session
names are machine-wide and an overwrite destroys a session the operator may
still need.

**Fail open when the marker check can't reach the remote.** Considered:
refusing every command while the remote is unreachable. Rejected: that turns
every network blip into an outage for every cloud session, and the offline
original is already the accepted risk. The command warns and uses its local
copy, as every sync failure does today.

**Markers are never deleted by koto.** Considered: deleting the whole prefix
on cleanup, as today. Chosen: keep the marker, because a cleanup on the first
host after a migration would otherwise remove the only lock.

**koto#239 deferred, with an interim rule.** Considered: taking the lock in
scope. Chosen: defer it. The lock fixes concurrent writers on one host; an
import reads the source's remote object, writes nothing of the source but the
marker, and creates a session no other process knows about until the import
returns. The stopped-source rule covers the rest, and a version check on
state pulls stays available as the corrective if a migrated log corrupts in
use.

**Fix koto#310 rather than only make it moot.** The import writes its header
before its first push, so it never hits the bug. `rebind` is still the right
verb for a checkout that moved on one host, and under the cloud backend it
silently fails today, so it gets fixed and tested here.

**Path-style addressing in scope.** The guide's MinIO gap could be closed by
documenting that IP endpoints don't work. Chosen: make them work, because the
harness needs a local endpoint and self-hosted S3 is the common case for a
self-managed remote.

**A harness endpoint the test starts itself.** Considered: a MinIO container
in CI, or only the existing real-bucket job. Chosen: an in-process
S3-compatible endpoint in the test, so the harness runs on every pull request
without secrets or a container, plus a run against the real bucket where the
existing secrets allow it.

**Framing carried from the BRIEF.** The BRIEF deferred four questions here:
the verb's name and how the source is named (R1 states how the source is
named; the design names the verb), the marker's form (R7, R8 and R19 bound
it; the design picks it), the collision rule (decided above), and whether the
template path needs carrying (R5 requires the template to be readable; the
design picks how).

## Downstream Artifacts

- `docs/designs/current/DESIGN-session-migration.md` (to be written)
