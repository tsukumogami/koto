---
schema: prd/v1
status: Draft
problem: |
  When a session's work passes to another session (a coordinator's successor,
  a takeover by an unrelated session, or a `koto session import` copy), koto
  keeps no record that the new session carries on the old one. Retention
  judges the predecessor on its own, so a terminal tick, `koto session prune`
  or `koto workspace prune` removes it while the successor still reads it,
  and import retires its source on purpose. Under the cloud backend a host
  that never pulled the successor can't see the relationship at all.
goals: |
  A successor records, once and at open, the session it continues, whoever
  launched it, and `koto session import` records the same relation in its
  copy. A predecessor stays readable while anything later in its lineage is
  kept, goes no sooner than the later members' own retention allows, and a
  pin on the newest member reaches back through the chain; nothing earlier
  keeps anything later. An import's source stays as a read-only predecessor.
  Every removal on every host sharing the bucket judges the same lineage and
  names what keeps a session, and the relation grants no authority.
upstream: docs/briefs/BRIEF-koto-session-continues.md
---

# PRD: A session names the session it continues

## Status

Draft

## Problem Statement

Work in koto often outlives the session that started it. A coordinator that
crashes or runs out of context hands its job to a successor. A session takes
over work another session left unfinished, sometimes started by someone with
no tie to the original. An operator moves a session to another workspace with
`koto session import`. In each case the new session carries on the old one
and reads its decisions, context keys and history to know where things stand.

koto has no record of that. A hierarchy (the set of sessions koto keeps or
releases together) is joined only through dispatched-by, held-by and closes
links, a child's `parent_workflow` and the `chain/parent` key, and a
successor usually has none of those to its predecessor. So the predecessor is
judged alone: once it finished and `retention.keep` passed, its own terminal
tick, `koto session prune` or `koto workspace prune` removes it while a live
successor still needs it. A pin on each predecessor is the only defence, and a
pin keeps one session, never a chain.

Import discards its source by design. It leaves a `migrated.json` marker
beside the source; every command on the source then refuses with
`session_migrated`, reads included; and `koto session prune --migrated-from`
deletes it once the marker is `session.cloud.marker_retain_days` old. The
record the copy came from is gone.

Under the cloud backend the retention judgment reads only the host's local
store; the pin guard added in #460 reads the bucket for pins alone. A prune
on a host that never pulled the successor, or where the successor lives under
another workspace's bucket prefix, sees nothing joining the predecessor to
anything alive and deletes it from the bucket for every host.

### Terms

- **Relation.** A directed record held by a session S naming one session P
  that S continues. S is P's **successor**; P is S's **predecessor**.
- **Later members** of a session are its successors, their successors, and so
  on. **Earlier members** are its predecessor, that one's predecessor, and so
  on.
- **Standing.** Whether today's rules keep a session, the relation aside. A
  session has standing while it is alive (running, its log unreadable, or an
  open anchor), while its hierarchy's verdict keeps it (as
  `koto session hierarchy` reports it today: a live member, `retention.keep`
  not yet passed since the hierarchy's release instant, or a member or record
  that can't be read), or while it holds a pin, readable or not.
- **Kept by the relation.** A session is kept by the relation while at least
  one later member has standing. Each later member with standing is a
  **keeper**, and the members between the kept session and a keeper are its
  **path**.

### Output fields

Every report this PRD asks for uses these fields. A JSON entry for a session
the relation keeps carries `kept_by_relation: true` and `relation_keepers`, a
list with one object per keeper, ordered by the keeper's name:

- `keeper`: the keeper's name.
- `standing`: `alive`, `hierarchy` or `pin`.
- `retention_passes_at`: for `hierarchy` standing whose retention is
  counting down, the RFC 3339 instant it passes; absent otherwise.
- `pin`: for `pin` standing, the keeper's pin as the existing `pin` field
  shows it (`reason`, `set_by`, `set_at`, `source`); absent otherwise.
- `path`: the names from the kept session's successor to the member just
  before the keeper, nearest first; empty when the keeper is the successor.

The text form prints one line per keeper:
`kept: continued by <keeper> (<standing>)` with `via <path joined by ", ">`
appended when the path isn't empty. A relation that keeps because it can't be
judged is reported as `relation_unreadable`, a list of objects with `holder`
(the session holding the relation, or the name the record carries when no
holder can be named) and `reason`, one of `unparseable`, `too_large`,
`mismatch`, `cycle` or `unlistable`. A field this PRD adds is omitted from an
entry where it would be empty or false.

## Goals

- A session that takes over another's work says so once, at open, through a
  koto verb that works the same whoever launched it.
- A predecessor stays readable while anything later in its lineage is kept,
  and goes once no later member keeps it; a pin on the newest member keeps
  the chain behind it; nothing earlier keeps anything later.
- An import keeps its source as a read-only predecessor instead of throwing
  it away.
- Every removal, on every host sharing the bucket, judges the same lineage and
  names what keeps each session.
- The relation adds no authority, and all of it works with koto alone.

## User Stories

- **Coordinator's successor.** As the agent running a successor coordinator,
  I want to open my session naming the coordinator session I continue, so
  that the old coordinator and its own predecessor stay readable while I work
  and, unless one of them is pinned, go in the same prune run once I finish
  and my retention passes.
- **Takeover by a stranger.** As an operator starting an unrelated agent
  session to take over a stalled session that nothing dispatched to me, I
  want to record the takeover on a session I already opened, so that a prune
  afterwards keeps the stalled session and names my session as the reason,
  while my session gains no authority the stalled session's dispatcher held.
- **Successor in another workspace.** As a coordinator's successor started
  in a fresh checkout, I want to name a predecessor that lives in another
  workspace, so that the relation holds across workspaces sharing one bucket.
- **Import into another workspace.** As an operator importing a session into
  a second workspace with the cloud backend configured through the
  environment, I want the copy to name its source, and the source to refuse
  ticks and writes while still serving reads, so that I can continue the work
  in the new workspace without losing the record it came from.
- **Prune on a second host.** As an operator running a prune on a second host
  that shares the bucket and never pulled the successor, I want the prune to
  keep the predecessor and name the same keeper the successor's own host
  names, so that no host deletes what another host still needs.
- **Keeping a finished chain.** As an operator keeping the record of a
  finished chain of sessions, I want a pin on the newest session to keep the
  whole chain, and a pin on the oldest to keep only the oldest, so that one
  pin protects exactly what I mean.

## Requirements

### Writing the relation

- **R1. A successor records the relation at open.** A koto verb records, in
  the successor's own context and nowhere else, that it continues a named
  predecessor. It is usable in the command that creates the successor's
  session, and also on an already open session that holds no relation yet.
  It needs no request leg, no `KOTO_ANCHOR`, no `KOTO_AUTHORITY` and no
  dispatcher: a session launched by neither the predecessor nor anything the
  predecessor dispatched records it the same way. The verb's name and shape
  are a design choice; the behaviour below is not.
- **R2. What the relation records.** The relation records the predecessor's
  name, its session id, and the absolute path of the workspace it lives in;
  under the cloud backend it also records the predecessor's bucket prefix. The
  predecessor may live in the successor's workspace or another one; the verb
  takes the other workspace's path when it does. Under the cloud backend the
  successor's bucket copy carries the relation once the verb's command exits
  0.
- **R3. Refusals.** The verb exits 2 and writes nothing when: the predecessor
  name fails the session-name rule or names the successor itself; no session
  with that name is in the local store and, under the cloud backend, none is
  under the named workspace's bucket prefix either; the predecessor is an
  anchor or an anchor's close record; or the successor already holds a
  relation naming a different predecessor. It exits 3 and writes nothing when
  the bucket can't be listed to look for the predecessor. Recording a relation
  the successor already holds changes nothing and exits 0. A predecessor that
  is running, finished, pinned or a migrated source may be named.
- **R4. One predecessor per session.** A session holds at most one relation.
  A predecessor may have any number of successors.
- **R5. Import writes the relation.** `koto session import` records the
  relation in the copy, naming the source with R2's fields, before the copy is
  pushed. A relation the source held, naming its own predecessor, is not
  carried into the copy: the copy's only relation names the source. A re-run
  of an import that finds the copy already built adds the relation when the
  copy holds none, and exits 2 with code `relation_conflict`, changing
  nothing, when the copy holds one naming another session.
- **R6. A migration marker counts as a relation.** A `migrated.json` marker
  beside a session, whose target is present by the existing check (the
  target's header carries the session id the marker names and its log begins
  with every event of the source's log), counts as a relation from that target
  to the session, whether or not the target holds one. This covers sources
  imported by releases without this feature. A marker whose target is absent,
  replaced, unverified or unknown keeps the session it sits beside, as today.

### Keeping

- **R7. The relation keeps earlier members only.** A session is kept by the
  relation while a later member has standing. A pin on a later member is
  standing, so a pin on the newest member keeps every earlier member. Nothing
  about an earlier member (its being alive, pinned, or within its retention)
  keeps a later one, and the relation never joins two hierarchies: every
  hierarchy's members, keepers and release instant are the same with and
  without the relation.
- **R8. Release follows the later members.** Once no later member has
  standing, the relation keeps nothing, so each earlier member goes when its
  own standing ends. A predecessor whose own retention passed long ago is
  removed by the first removal of R9 that judges it after the last of its
  later members loses standing, and by none before.
- **R9. Every removal by rule counts it.** These keep a session the relation
  keeps, with standing judged at the moment of each removal's own judgment
  (including the re-judgment `--apply` makes before it deletes): the terminal
  tick, with and without `--no-cleanup`; the parent sweep; the same-name
  replace (which moves the kept session aside, as it does a kept hierarchy);
  `koto workspace prune`, with and without `--root`, `--force` and `--yes`;
  `koto session prune` with `--all`, `--execution-dir`, `--older-than` and
  `--apply --plan`; and `koto session prune --migrated-from` (R15).
- **R10. Explicit removals warn.** `koto session cleanup` and
  `koto cancel --cleanup` remove a session the relation keeps, as they remove a
  session its hierarchy keeps today, exit 0, and report what they passed over:
  JSON output carries `relation_keepers` (and `relation_unreadable` when
  present) as the Output fields define, and stderr prints the text lines. A
  pin still refuses them, as today.
- **R11. Failing toward keeping.** Each of these keeps the session named and
  is reported in `relation_unreadable`: a relation body that doesn't parse
  (`unparseable`) or exceeds 4 KiB (`too_large`); a relation whose recorded
  session id or bucket prefix doesn't match the session found under its name
  (`mismatch`); and a cycle of relations, which keeps every session on it
  (`cycle`). A session whose own relations can't be listed (`unlistable`)
  keeps every session the removal would delete, as an unreadable links
  directory keeps every hierarchy today.

### Saying why

- **R12. Each kept session names its keepers.** Every removal of R9 that keeps
  a session because of the relation reports it with the Output fields, in
  JSON and text, naming every keeper. The existing fields keep their meanings:
  `kept_by_pin` and `pin` describe the session's own pin only, and a
  hierarchy's `keepers` list only its own members. A session that is kept for
  its own standing and also by the relation carries both reports.
- **R13. Reading the relation.** `koto session hierarchy <name>` reports, in
  JSON (`predecessor`, `successors`) and text, the session's predecessor when
  it holds a relation and the sessions holding a relation that names it, and,
  when the relation keeps it, its `relation_keepers` as R12 does.

### Import contract

- **R14. The source is a read-only predecessor.** Under the cloud backend
  (import needs it), after an import these commands on the source keep
  today's refusal, `session_migrated`, exit 2, naming the copy and its
  workspace: `koto next`, `koto context add` and `remove`, `koto rewind`,
  `koto cancel`, `koto decisions record`, `koto overrides record`,
  `koto session link`, `koto session rebind`, `koto session resolve`,
  `koto session update`, `koto session pin` and `unpin`, and the verb of R1
  used on the source. The copy stays the only session that advances. These
  read commands now answer from the source's last pushed copy instead of
  refusing: `koto status`, `koto context get`, `exists` and `list`,
  `koto decisions list`, `koto overrides list`, and the `--workspace` forms of
  those. A read leaves the source's bucket objects, its marker and the copy
  byte-identical, and writes no read record into the source. Run from the
  source's workspace, these read the source; run from the copy's workspace,
  they read the copy. When a same-host import gave the copy the source's local
  name, a read from the source's workspace answers from the source's bucket
  objects and never writes them into the local store the copy holds. Every
  other command keeps today's behaviour on a migrated source.
- **R15. `prune --migrated-from` waits for the lineage.** A marker and the
  source beside it are removable only when the marker's target is present
  (R6) and the relation no longer keeps the source. While the copy or any
  member later than it has standing, the marker is kept and the entry carries
  R12's fields. `session.cloud.marker_retain_days` and `--source-retired` no
  longer decide removal; both are still accepted, and a run that sets either
  adds `ignored`, a list holding `marker_retain_days` and/or `source_retired`,
  to its JSON output, prints `ignored: <name> no longer decides removal` on
  stderr, and exits as it would without them. A target that is absent,
  replaced, unverified or unknown keeps the marker, as today.
- **R16. The source's pin.** A pin the source held travels to the copy as
  today, and the source's own pin keeps nothing; what keeps the source is the
  relation.

### Cloud backend

- **R17. The bucket decides, on every host.** Under the cloud backend, before
  any removal of R9 deletes a session from the bucket, its judgment reads from
  the bucket the relations naming that session, including R6's markers, and
  judges each later member's standing from the bucket where this host holds no
  local copy of it: alive from its state log (not terminal, or its log
  unreadable), hierarchy retention from the stop instant its log records plus
  `retention.keep`, and its pin from its pin object. A later member under
  another workspace's prefix in the same bucket counts. A second host that
  never pulled the later members keeps the same sessions and names the same
  keepers as the host holding them.
- **R18. A check that can't be made keeps.** A listing or read of a relation,
  or of a later member's standing, that fails or doesn't parse keeps the
  session, which the removal reports with `bucket_unchecked` naming what
  couldn't be checked, as the pin guard does. `koto session cleanup` and
  `koto cancel --cleanup` exit 3 and remove nothing when the bucket can't be
  listed, as they do for the pin today.
- **R19. A write the bucket doesn't confirm.** When the verb of R1 runs under
  the cloud backend and the bucket doesn't confirm a write the relation needs,
  it exits 3 and leaves no relation locally or in the bucket, so running it
  again works.

### Authority and scope

- **R20. No authority.** These answer the same with and without the relation:
  a pin's release (`koto session unpin` under an anchor, which needs
  `release-pin`); an anchor's recorded authority and its display in
  `koto session hierarchy`; and a request leg's dispatch and its epoch fence.
  The relation is not a `dispatched-by` link, and no code that reads
  `dispatched-by` links (the anchor family, request dispatch) reads it.
- **R21. koto alone.** Every requirement holds on a host where koto is the
  only tool installed: no niwa, no shirabe, no GitHub CLI.

### Non-functional

- **R22. Bounded cost.** On the local backend the relation makes no bucket
  request. Under the cloud backend, a session kept for its own standing before
  the relation is consulted costs no request for the relation; a session about
  to be deleted costs at most 2 bucket requests for its relations when nothing
  names it, plus at most 3 for each later member reached, whatever the number
  of sessions in the bucket. These are in addition to what the pin guard
  already spends.
- **R23. Stores without the relation behave as before.** A store where no
  session holds a relation and no migration marker exists gives the same keeps,
  removals and output as a release without this feature; the new fields are
  omitted.
- **R24. Releases.** The changelog and the session retention guide name the
  first release that honours the relation, and say that an older release's
  local prunes keep a chain, that under the cloud backend an older release can
  delete a predecessor whose successor it never pulled, and that an older
  release's import still retires its source.

## Acceptance Criteria

### Setup and conventions

Tests run against koto's own test harness with the in-process stand-in
bucket, each host its own HOME, the cloud backend configured through the
environment only, and a PATH holding koto and no niwa, shirabe or GitHub CLI;
this satisfies R21 for every criterion. Unless a criterion says otherwise:
`KOTO_RETENTION_KEEP=1s`; every agent session involved has been deleted and
its anchor closed; each removal runs after the retention has passed since
every finished session finished, so only the relation under test can keep a
session; calls are by hand. "X hashes the same" means X's log and every key
hash the same immediately before and immediately after the step named.

The **chain** is v1, v2 and v3 in one workspace W1: v2 is created continuing
v1 with R1's verb in its creating command, and v3 is created continuing v2 the
same way. v1 and v2 are finished with `--no-cleanup`; v3 is running.

**Removal set S** (each step checked on its own): S1, a fresh chain where v1's
and v2's terminal steps run without `--no-cleanup` after v3 exists; S2,
`koto session prune --apply --all`; S3, `koto workspace prune --root v1 --yes`;
S4, `koto workspace prune --yes`.

### Keeping and releasing

- [ ] **Chain.** After each of S1 to S4, v1 and v2 are present and hash the
  same. Each step's output for v1 carries `kept_by_relation: true` and one
  `relation_keepers` entry `{keeper: v3, standing: alive, path: [v2]}`, and for
  v2 `{keeper: v3, standing: alive, path: []}`; the text form prints
  `kept: continued by v3 (alive) via v2` for v1. v3 reads v1's and v2's keys
  with `koto context get`, and v1 and v2 hash the same afterwards.
- [ ] **Other removal points.** With the chain, v1 and v2 also survive and
  carry the same entries under: the terminal tick with `--no-cleanup`; a
  parent sweep (v1 created as a koto child of a parent X, X ticked to its
  terminal without `--no-cleanup`); `koto workspace prune --root v1 --yes
  --force`; `koto session prune --apply` with `--execution-dir <W1>` and with
  `--older-than 1s`; and `--apply --plan` on a preview token.
- [ ] **Re-judgment at apply.** A preview with v3 running lists v1 and v2 as
  kept. v3 is then stopped and its retention passes; `--apply --plan` on that
  token refuses to remove sessions the preview kept (exit 4, as today), and a
  fresh `--apply --all` removes all three.
- [ ] **Release.** With `KOTO_RETENTION_KEEP=20s`, v3 then finishes. Within
  20 seconds of v3's stop, S2 keeps all three, and v1's and v2's entries name
  v3 with `standing: hierarchy` and `retention_passes_at` equal to v3's stop
  plus 20 seconds. After that instant, the next `koto session prune --apply
  --all` removes v1, v2 and v3.
- [ ] **Direction, forward.** v3 is pinned after it finishes, and the
  retention passes. `koto session prune --apply --all` keeps v1, v2 and v3. v1's
  and v2's entries carry `relation_keepers` with `{keeper: v3, standing: pin}`
  and v3's pin's reason, setter and time, and no `kept_by_pin`; v3's entry
  carries `kept_by_pin: true`. A build where the relation joins hierarchies
  without letting a pin reach back fails this criterion.
- [ ] **Direction, backward.** A fresh chain where v1 is pinned and v2 and v3
  finished with the retention passed: `koto session prune --apply --all`
  keeps v1 with `kept_by_pin: true` and no `relation_keepers`, and removes v2
  and v3, whose entries name no keeper. In a second fresh chain where v1 is
  still running and v2 and v3 finished with the retention passed, the same
  prune removes v2 and v3. A build where an earlier member keeps a later one
  fails this criterion.
- [ ] **Two successors.** v2a and v2b both continue v1, and v1 is finished.
  While either has standing, prunes keep v1 and name each keeper that has
  standing, ordered by name; once neither has, the next prune removes v1.
- [ ] **No join.** For each of v1, v2 and v3, `koto session hierarchy --json`
  lists the same members, `keepers` and release instant as on the same store
  with the relations removed. v2's output carries `predecessor: v1` and
  `successors: [v3]`; v1's carries `relation_keepers` naming v3.
- [ ] **Same-name replace.** With the chain, `koto init v1 --replace-terminal`
  (with v1's template) moves v1 aside to the name its output reports; the
  moved session has v1's session id, is still kept by v3, and a prune entry for
  it carries `relation_keepers` naming v3.

### Explicit removals

- [ ] **Cleanup and cancel warn.** With the chain, `koto session cleanup v1`
  removes v1, exits 0, its JSON carries `relation_keepers` naming v3, and
  stderr prints `kept: continued by v3 (alive) via v2`. With a fresh chain
  where v1 is still running, `koto cancel v1 --cleanup` does the same. With v1
  pinned, both refuse with exit 2 and code `pinned`, as today.
- [ ] **Cleanup when the bucket can't be listed.** Under the cloud backend
  with the stand-in failing listings, `koto session cleanup v1` exits 3 and v1
  is present in the local store and the bucket.

### Writing the relation

- [ ] **Recorded fields.** After the chain is built under the cloud backend,
  v2's relation records `v1`, v1's session id, W1's absolute path and W1's
  bucket prefix, and the relation object is present under v2's bucket prefix
  when the verb's command has exited 0.
- [ ] **On an open session.** v2 is created without a relation and then given
  one naming v1 with R1's verb; prunes keep v1 and name v2.
- [ ] **Another workspace.** v3 is created in workspace W2 continuing v2 in
  W1, naming W1's path; prunes in W1 keep v1 and v2 and name v3.
- [ ] **Takeover by a stranger.** v1 is dispatched by anchor A and finishes;
  A is closed. An operator, with no `KOTO_ANCHOR`, no `KOTO_AUTHORITY` and no
  request leg, records v2 continuing v1, and v2 stays running: the relation is
  recorded and `koto session prune --apply --all` keeps v1, naming v2. The same
  holds when v2 is created under anchor B, where neither B nor v2 holds a
  dispatched-by, held-by or closes link naming v1, A or any session v1
  dispatched, and none of those holds such a link naming B or v2.
- [ ] **Refusals.** R1's verb exits 2, and v2's keys and log hash the same,
  for: a predecessor named `a/b`; v2's own name; `nosuch`; an anchor's name; an
  anchor close record's name; a name present in neither the local store nor
  W1's bucket prefix under the cloud backend; and, when v2 already continues
  v1, a different predecessor. Repeating `v2 continues v1` exits 0 and v2
  hashes the same. With the stand-in failing listings, naming a predecessor
  exits 3 and writes nothing.
- [ ] **Allowed predecessors.** Naming a running session, a finished one, a
  pinned one and a migrated source each exits 0 and records the relation.
- [ ] **Unconfirmed write.** With the stand-in refusing PUTs, R1's verb exits
  3, v2 holds no relation key locally and the bucket lists no relation object
  for v2; with PUTs allowed again, the same command exits 0 and the relation
  is present in both.

### Failing toward keeping

- [ ] **Unreadable relation.** v2's relation body is replaced, in the local
  store and the stand-in, with bytes that don't parse; a prune keeps v1 and
  reports `relation_unreadable: [{holder: v2, reason: unparseable}]`. The same
  with a 5 KiB body reports `too_large`.
- [ ] **Mismatch.** v1 is removed by hand and a new session named v1 created;
  v2's relation still records the old session id. A prune keeps the new v1 and
  reports `{holder: v2, reason: mismatch}`.
- [ ] **Cycle.** Relations written by hand make v1 continue v3 as well as the
  chain; all three finished with the retention passed. A prune keeps all three
  and reports `cycle` for each.
- [ ] **Unchecked bucket.** On a second host that never pulled v3, with the
  stand-in failing the listing of v3's prefix, a prune keeps v1 and v2 and
  reports `bucket_unchecked` naming v3.

### Import

- [ ] **Import keeps its source.** In W1 a chain v1 to v2 is built, v2
  finishes, and v2 is imported into W2, where it continues and stays running.
  The copy's relation records v2, the source's session id, W1's path and W1's
  prefix, and the copy holds no relation naming v1. In W1, each write command
  of R14 on v2 exits 2 with `session_migrated` naming the copy and W2. In W1,
  `koto status v2`, `koto context get v2 <key>`, `exists`, `list`,
  `koto decisions list v2` and `koto overrides list v2` exit 0 with the
  source's values, and the source's bucket objects, its marker and the copy
  hash the same afterwards.
- [ ] **Same-host import.** The same import run on one host without `--as`,
  so the copy takes the name v2 in the shared local store: `koto status v2`
  from W1 answers with the source's state and the copy hashes the same; from
  W2 it answers with the copy's.
- [ ] **Migrated-from waits.** With the copy running, `koto session prune
  --migrated-from W1` keeps the marker and source with `relation_keepers`
  naming the copy (`standing: alive`); the same with `--source-retired` keeps
  them too, its JSON carries `ignored: ["source_retired"]` and stderr prints
  the ignored line. A `koto session prune --apply --all` run in W1 on a second
  host that never pulled the copy keeps the source and names the copy. After
  the copy finishes and its retention passes, `prune --migrated-from W1
  --apply` removes the marker and the source.
- [ ] **Import of a chain member keeps the chain.** v1 to v2 in W1, v2
  imported into W2 and the copy running: prunes in W1 keep v1 with
  `relation_keepers` `{keeper: <copy>, path: [v2]}`.
- [ ] **Earlier imports.** A source imported by a release without this
  feature (its copy has no relation, its marker names the copy) is kept by
  `prune --migrated-from` and by a prune on a second host while the copy is
  running, naming the copy.
- [ ] **Absent target.** A marker whose target was removed from the bucket
  keeps the marker under `prune --migrated-from`, with reason `absent` as
  today.
- [ ] **Re-run and conflict.** Re-running the import on a copy built without
  a relation adds it and exits 0; re-running it on a copy whose relation names
  another session exits 2 with `relation_conflict` and the copy hashes the
  same.
- [ ] **Pin travels.** The source pinned before the import: the copy holds
  the pin afterwards; once the copy finishes and its retention passes and the
  copy is unpinned, `prune --migrated-from W1 --apply` removes the source.

### Cloud backend

- [ ] **Second host sees the lineage.** The chain's v1 and v2 are in W1 and
  v3 in W2, all on host A. Host B (its own HOME, same bucket, only v1 pulled)
  runs `koto session prune --apply --all` in W1: v1 is kept with the same
  `relation_keepers` host A reports. With v3 pinned and finished, host B names
  `standing: pin`; with v3 finished inside its retention, host B names
  `standing: hierarchy` and the same `retention_passes_at` as host A.
- [ ] **Bounded cost.** The stand-in counts requests. A prune over 100 and
  over 10,000 finished sessions with no relation, nothing about to be deleted,
  makes no request for relations. A prune deleting 50 sessions that nothing
  names makes at most 100 requests for relations, at both bucket sizes. A
  prune judging v1 of the chain makes at most 2 + 3 × 2 requests for v1's
  relations.

### Authority

- [ ] **No authority.** With the chain and v1 pinned: `koto session unpin v1`
  under v3's anchor, which records no `release-pin`, refuses exactly as it
  does on the same store with the relations removed; an unpin of v3 under
  v1's anchor refuses the same way. `koto session hierarchy` shows the same
  recorded authority for each anchor with and without the relations. A request
  leg dispatched to v3 is fenced by the same epoch with and without them.

### Cross-cutting

- [ ] **Stores without the relation.** A fixed scenario with no relation and no
  marker (the pin PRD's removal set) is run on this build and on the previous
  release: JSON and text outputs are identical, and the existing retention and
  pin test suites pass unchanged.
- [ ] **Docs.** The changelog and the session retention guide name the first
  release that honours the relation and state R24's three facts about older
  releases; the CLI guide documents R1's verb and R15's ignored settings.

## Out of Scope

- **Authority of any kind.** The relation feeds no authority check (R20).
- **Guards on trusted removals**, such as a prune floor (#458). They decide
  whether a removal may run at all, not what keeps a session.
- **The coordinator-name rule in prune and read-by-name fixes.** Both touch
  the same commands and are separate changes.
- **The takeover flow.** How a coordinator's successor is started and what it
  does first belongs to the workflow skills that run coordinators.
- **Workspace or instance lifetimes** managed by tools outside koto.
- **Bucket copies that fork when one session is written from several
  directories (#477).** The relation is read correctly for a session written
  from one directory; the fork itself is #477's to fix.
- **Removing a relation.** No verb removes a relation; it goes with the
  session that holds it.
- **Removing a marker whose target is gone.** An absent target keeps its
  marker, as today; that cleanup is unchanged by this feature.
- **Testing older releases' behaviour.** R24 is a documentation requirement;
  no criterion runs an older release.
- **Retiring caller workarounds** built for the missing relation.

## Known Limitations

- An older koto sharing a store reads an unknown link kind as an ordinary
  edge, so its local prunes over-keep a chain, but under the cloud backend it
  never reads relations from the bucket and can delete a predecessor whose
  successor it never pulled. Its import still retires the source. Every host
  sharing a bucket needs this release.
- Explicit cleanup removes a session the relation keeps (R10). A successor
  that needs a predecessor kept against explicit removal pins it.
- Nothing orders two hosts: a successor recorded on one host after another
  host's removal has read the predecessor's relations, and before it deletes,
  doesn't stop that removal. The window is the time between that read and the
  delete, as for pins.
- A relation recorded under the local backend reaches the bucket only when the
  successor is first pushed under the cloud backend; until then no other host
  sees it.

## Decisions and Trade-offs

- **Directed, per member, not a hierarchy edge.** Adding the relation as an
  ordinary link would join predecessor and successor into one hierarchy:
  forward keeping would come free, but an alive predecessor or its anchor
  would keep the successor, a late-finishing predecessor would delay the
  successor's removal, and a pin on the newest member would still keep only
  that member. Keeping the relation out of the hierarchy and checking it per
  member, like a pin, is the only reading that passes both direction
  criteria.
- **A keeper's standing includes its own hierarchy.** A successor kept by a
  live member of its own hierarchy keeps its predecessors too. Counting only
  the successor's own state would let a chain go while its newest member is
  still kept, which "no sooner than that member's own retention allows" rules
  out.
- **One predecessor per session.** A session continues one session. Several
  would make a keeper's path ambiguous and add nothing a story here needs; a
  predecessor may still have many successors.
- **Explicit cleanup removes and warns.** `koto session cleanup` doesn't
  refuse for a hierarchy keeper today, only for a pin, because the pin is the
  one keeper an operator sets on purpose. The relation follows the hierarchy,
  and a pin stays the way to make a session survive explicit removal.
- **Import's age rule goes.** The source was removed by age because it was
  retired; once it is a kept predecessor, its lineage decides. The age setting
  and `--source-retired` stay accepted with no effect so scripts don't break.
- **The marker counts as a relation.** A verified marker already names the
  copy by session id and checks the copy's log against the source's, which is
  stronger evidence than a record in the copy alone. Counting it covers
  imports made before this release and gives every host the same answer from
  the source's own prefix. A marker that fails the check keeps, as today.
- **Fixed field names.** The report fields are fixed here, not left to the
  design, because callers and tests read them; the verb's surface is left to
  the design because R1 fixes its behaviour.
