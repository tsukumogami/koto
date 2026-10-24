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
  kept, goes no sooner than the newest member's own retention allows, and a
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

koto has no record of that. A hierarchy is joined only through dispatched-by,
held-by and closes links, `parent_workflow` lineage and `chain/parent`, and a
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
store, and #460 added a bucket read for pins alone. A prune on a host that
never pulled the successor, or where the successor lives under another
workspace's bucket prefix, sees nothing joining the predecessor to anything
alive and deletes it from the bucket for every host.

### Terms

- **Relation.** A directed record held by a session S naming one session P
  that S continues. S is P's **successor**; P is S's **predecessor**.
- **Lineage.** The sessions reachable from a session by following relations
  in either direction. **Later** members are reached by following successors;
  **earlier** ones by following predecessors.
- **Standing.** Whether today's rules keep a session, the relation aside: it
  is alive (running, unreadable, or an open anchor), its hierarchy's verdict
  keeps it (a live member, the retention not yet passed since the release
  instant, or something unreadable), or it holds a pin (readable or not).
- **Kept by the relation.** A session is kept by the relation while some later
  member of its lineage has standing. The later member with standing is its
  **keeper**.

## Goals

- A session that takes over another's work says so once, at open, through a
  koto verb that works the same whoever launched it.
- A predecessor stays readable while anything later in its lineage is kept,
  and goes once the newest member's own retention lets it go; a pin on the
  newest member keeps the chain behind it; nothing earlier keeps anything
  later.
- An import keeps its source as a read-only predecessor instead of throwing
  it away.
- Every removal, on every host sharing the bucket, judges the same lineage and
  names what keeps each session.
- The relation adds no authority, and all of it works with koto alone.

## User Stories

- **Coordinator's successor.** As the agent running a successor coordinator,
  I want to open my session naming the coordinator session I continue, so
  that the old coordinator and its own predecessor stay readable while I work
  and go away together once I finish and my retention passes.
- **Takeover by a stranger.** As an operator starting an unrelated agent
  session to take over a stalled session that nothing dispatched to me, I
  want to record the takeover, so that a prune afterwards keeps the stalled
  session and names my session as the reason, while my session gains no
  authority the stalled session's dispatcher held.
- **Import into another workspace.** As an operator importing a session into
  a second workspace with the cloud backend configured through the
  environment, I want the copy to name its source, and the source to refuse
  ticks and writes while still serving reads, so that I can continue the work
  in the new workspace without losing the record it came from, and so that
  `prune --migrated-from` and prunes on other hosts keep the source while the
  copy lives.
- **Keeping a finished chain.** As an operator keeping the record of a
  finished chain of sessions, I want a pin on the newest session to keep the
  whole chain, and a pin on the oldest to keep only the oldest, so that one
  pin protects exactly what I mean.

## Requirements

### Writing the relation

- **R1. A successor records the relation at open.** A koto verb records, in
  the successor's own context and nowhere else, that it continues a named
  predecessor. It is usable when the successor's session is created, in the
  same command, and also on an already open session that holds no relation
  yet. It needs no request leg, no `KOTO_ANCHOR`, no `KOTO_AUTHORITY` and no
  dispatcher: a session launched by neither the predecessor nor anything the
  predecessor dispatched records it the same way. The verb's name and shape
  are a design choice; the behaviour below is not.
- **R2. What the relation identifies.** The relation names the predecessor
  and records enough to find it again on any host sharing the store: its
  session id and, under the cloud backend, the bucket prefix it lives under.
  The predecessor may live in the successor's workspace or another workspace;
  the verb takes the other workspace's path when it does. The successor's
  bucket copy carries the relation once the successor's first push lands.
- **R3. Refusals.** The verb exits 2 and writes nothing when: the predecessor
  name fails the session-name rule or names the successor itself; no session
  with that name can be found where the verb was told to look (the local
  store, or under the cloud backend the named workspace's bucket prefix); the
  predecessor is an anchor or an anchor's close record; or the successor
  already holds a relation naming a different predecessor. Recording a
  relation the successor already holds changes nothing and exits 0. A
  predecessor that is running, finished, pinned or a migrated source may be
  named.
- **R4. One predecessor per session.** A session holds at most one relation.
  A predecessor may have any number of successors.
- **R5. Import writes the relation.** `koto session import` records the
  relation in the copy, naming the source with its session id and its
  workspace prefix, as part of building the copy, before the copy is pushed.
  A relation the source itself held (naming the source's own predecessor) is
  not carried into the copy: the copy's only relation names the source. A
  re-run of an import that finds the copy already built adds the relation
  when the copy lacks it.
- **R6. A relation written by an earlier release's import.** A copy made by a
  release without this feature holds no relation. Where the copy's
  `koto/imported-from` names the source by name and session id and the
  source's `migrated.json` marker names the copy by name and session id, the
  pair is read as a relation from the copy to the source.

### Keeping

- **R7. The relation keeps earlier members only.** A session is kept by the
  relation while a later member of its lineage has standing. A pin on a later
  member is standing, so a pin on the newest member keeps every earlier
  member. Nothing about an earlier member (its being alive, pinned, or within
  its retention) keeps a later one, and the relation never joins two
  sessions' hierarchies: every hierarchy, its members, its keepers and its
  release instant are the same with and without the relation.
- **R8. Release follows the newest member.** Once no later member has
  standing, the relation keeps nothing, so each earlier member goes when its
  own standing ends. A predecessor whose own retention passed long ago is
  therefore removed by the first removal that runs after its newest later
  member's standing ends, and no sooner.
- **R9. Every removal by rule counts it.** These keep a session the relation
  keeps: the terminal tick with and without `--no-cleanup`; the parent sweep;
  the same-name replace (which moves the kept session aside, as it does for a
  kept hierarchy); `koto workspace prune`, with `--root`, `--force` and
  `--yes`; `koto session prune` with every selection flag and with `--plan`;
  and `koto session prune --migrated-from` (R15). A keeper's being kept
  counts at the moment of each removal's own judgment, including the
  re-judgment `--apply` makes before it deletes.
- **R10. Explicit removals warn.** `koto session cleanup` and
  `koto cancel --cleanup` remove a session the relation keeps, as they remove
  a session its hierarchy keeps today, and report each keeper they passed
  over in their output (JSON) and on stderr (text). A pin still refuses them
  as today.
- **R11. Failing toward keeping.** A relation that can't be read keeps the
  session it names, or every session the holder could have named when its
  name can't be read. A relation whose recorded session id or prefix doesn't
  match the session found under its name keeps that session. A cycle of
  relations keeps every session on it. Each such case is reported by the
  removal that passed over the session, naming the holder and why.

### Saying why

- **R12. Each kept session names its keeper.** Every removal of R9 that keeps
  a session because of the relation names, in JSON and text: the keeper (the
  later member with standing), what gives the keeper standing (alive, its
  hierarchy's retention with the instant it passes, or its pin with reason,
  setter and time), and the successors between the kept session and the
  keeper, in order. Where several later members have standing, it names each.
  The existing fields keep their meanings: `kept_by_pin` and `pin` still
  describe the session's own pin only, and a hierarchy's `keepers` still list
  only its own members.
- **R13. Reading the relation.** `koto session hierarchy <name>` shows the
  session's predecessor (when it holds a relation) and its successors, and
  whether the relation keeps it, with the keeper as R12 names it.

### Import contract

- **R14. The source is a read-only predecessor.** After an import, every
  command that ticks or writes the source (`koto next`, `koto context add`
  and `remove`, `koto rewind`, `koto cancel`, `koto decisions record`,
  `koto overrides record`, `koto session link`, `koto session rebind`,
  `koto session resolve`, `koto session pin` and `unpin`, and any other
  command that appends to its log or changes its keys) refuses with
  `session_migrated`, exit 2, naming the copy and its workspace, as today. The
  copy stays the only session that advances. Commands that only read the
  source (`koto status`, `koto context get`, `exists` and `list`,
  `koto decisions list`, `koto overrides list`, and the `--workspace` reads of
  another workspace) answer from the source's last pushed copy, leave the
  source's log, keys and marker byte-identical in the bucket, and leave the
  copy byte-identical too. A same-host import whose copy took the source's
  local name still serves the source's reads from the source workspace from
  the bucket copy, never by writing it over the copy.
- **R15. `prune --migrated-from` waits for the lineage.** A marker and the
  source beside it are removable only when the marker's target is present
  (the existing check that the copy's log begins with every event of the
  source's) and the relation no longer keeps the source. While the copy, or
  anything later than the copy, has standing, the marker is kept and the entry
  names the keeper as R12 does. The age bound
  (`session.cloud.marker_retain_days`) and `--source-retired` no longer decide
  removal; both are still accepted, and a run that passes either says it had
  no effect. A target that is absent, replaced, unverified or unknown keeps
  the marker as today.
- **R16. The source's pin.** A pin the source held travels to the copy as
  today. The source's own pin stays inert; what keeps the source is the
  relation.

### Cloud backend

- **R17. The bucket decides, on every host.** Under the cloud backend, before
  any removal deletes a session from the bucket, its judgment reads the
  relations naming that session that the bucket holds, as the pin guard reads
  a pin object, and judges each successor's standing from the bucket where
  this host holds no copy. A successor under another workspace's prefix that
  shares the bucket counts. A second host that never pulled the successor
  keeps the predecessor exactly as the successor's own host does, and names
  the same keeper.
- **R18. A check that can't be made keeps.** A listing or read of a relation,
  or of a successor's standing, that fails or doesn't parse keeps the session
  and is reported as `bucket_unchecked` with what couldn't be checked. Under
  the conditions where `koto session cleanup` exits 3 for an unchecked pin
  today, R10's report says the relation couldn't be checked.
- **R19. A write the bucket doesn't confirm.** When the verb of R1 runs under
  the cloud backend and the bucket doesn't confirm what it must write there,
  it exits 3 and leaves no relation anywhere, so it can be run again.

### Authority and scope

- **R20. No authority.** No authority check reads the relation. A pin's
  release, a request leg's dispatch, an anchor's recorded authority and every
  other decision that asks whether a caller may act answer the same with and
  without it. The relation is not `dispatched-by`, and nothing that reads
  `dispatched-by` links reads it.
- **R21. koto alone.** Every requirement holds on a host with koto installed
  and no other tool.

### Non-functional

- **R22. Bounded cost.** A session kept for any reason before the relation is
  consulted costs no bucket request for the relation. A session about to be
  deleted costs a fixed number of bucket requests for its relations plus a
  bounded number per relation found, independent of how many sessions the
  bucket holds. On the local backend the relation adds no bucket request.
- **R23. Old sessions unchanged.** A store with no relation behaves exactly as
  it does on a release without this feature: same keeps, same removals, same
  output apart from new fields that are absent or empty.
- **R24. Releases.** The changelog and the session retention guide name the
  first release that honours the relation and say what an older release
  sharing the store does with it.

## Acceptance Criteria

### Setup and conventions

Unless a criterion says otherwise: `KOTO_RETENTION_KEEP=1s` and short idle and
stale periods are set through the environment; every agent session involved
has been deleted and its anchor closed; each removal runs after the configured
retention has passed since every finished session finished, so only the
relation under test can keep a session; calls are by hand. "Reads leave X
byte-identical" means the session's log and every key hash the same before and
after. The **chain** is v1, v2 and v3, where v2 continues v1 and v3 continues
v2, each relation written with R1's verb when the successor is created.

**Removal set S:** S1, v1's and v2's terminal steps run without
`--no-cleanup` after v3 exists; S2, `koto session prune --apply --all`; S3,
`koto workspace prune --root v1 --yes`.

### Keeping and releasing

- [ ] **Chain.** v1 finishes; v2 continues v1 and finishes; v3 continues v2
  and is live. After each of S1, S2 and S3, v1 and v2 are present and hash
  the same; each removal's output for v1 and for v2 names v3 as the keeper,
  alive, and lists the successors between (v2 for v1). v3 reads v1's and v2's
  keys with `koto context get`, and v1 and v2 hash the same afterwards.
- [ ] **Release.** In the chain above, v3 then finishes. Before v3's
  retention passes, S1 to S3 keep all three, and each names v3 with its
  retention and the instant it passes. After it passes, the next
  `koto session prune --apply --all` removes v1, v2 and v3.
- [ ] **Direction, forward.** v3 is pinned after it finishes, and the
  retention passes. `koto session prune --apply --all` keeps v1, v2 and v3;
  the entries for v1 and v2 name the pin on v3 (reason, setter, time) as what
  gives the keeper standing, v3's entry carries `kept_by_pin: true`, and v1's
  and v2's entries don't. A build where the relation joins hierarchies
  without letting a pin reach back fails this criterion.
- [ ] **Direction, backward.** A fresh chain; v1 is pinned; v2 and v3 have
  finished and the retention passed. `koto session prune --apply --all` keeps
  v1 naming its own pin and removes v2 and v3 naming no keeper. A build where
  the relation lets an earlier member keep a later one fails this criterion.
- [ ] **No join.** For each of v1, v2 and v3, `koto session hierarchy --json`
  lists the same members, keepers and release instant as on the same store
  with the relations removed, and additionally shows the predecessor and
  successors.
- [ ] **Same-name replace.** With the chain live, `koto init v1 ...
  --replace-terminal` moves v1 aside rather than removing it, and the moved
  session is still kept by v3 and names it.
- [ ] **Explicit removal warns.** With the chain live, `koto session cleanup v1`
  removes v1, exits 0, and names v3 as a keeper it passed over.

### Writing the relation

- [ ] **Takeover by a stranger.** v1 is dispatched by anchor A and finishes;
  A is closed. An operator, with no `KOTO_ANCHOR`, no `KOTO_AUTHORITY` and no
  request leg, creates v2 continuing v1 with R1's verb, and v2 stays live. The
  relation is recorded, and `koto session prune --apply --all` keeps v1 and
  names v2. The same holds when v2 is created under an unrelated anchor B
  that never dispatched or held v1 or anything v1 dispatched.
- [ ] **Refusals.** R1's verb exits 2 and writes nothing for: a predecessor
  name with a `/`; the successor's own name; a name with no session; an
  anchor's name; and a second, different predecessor. Repeating the same
  relation exits 0 and changes nothing.
- [ ] **No authority.** With v1 pinned and v3 continuing v2 continuing v1, a
  `koto session unpin v1` under v3's anchor (which holds no `release-pin`)
  refuses exactly as it does with the relations removed, and an unpin of v3
  under v1's anchor refuses the same way. An authority check for v3 answers
  the same with and without the relations.

### Import

- [ ] **Import keeps its source.** With the cloud backend configured through
  the environment only and two workspaces W1 and W2 sharing a bucket, v2 is
  finished in W1 and imported into W2, where it continues and stays live.
  The copy's relation names the source with its session id and W1's prefix.
  In W1, `koto next v2` and `koto context add v2 k` exit 2 with
  `session_migrated` naming the copy and W2; `koto status v2` and
  `koto context get v2 <key>` answer from the source and leave the source's
  bucket objects and the copy byte-identical.
- [ ] **Migrated-from waits.** In the import setup, with the copy live,
  `koto session prune --migrated-from W1` and `--migrated-from W1
  --source-retired` keep the marker and source and name the copy as keeper; a
  `koto session prune --apply --all` run on a second host (its own HOME) in
  W1 that never pulled the copy keeps the source and names the copy. After the
  copy finishes and its retention passes, `prune --migrated-from W1 --apply`
  removes the marker and the source.
- [ ] **Earlier imports.** A source imported by a release without this
  feature, whose copy holds `koto/imported-from` and whose marker names the
  copy, is kept by `prune --migrated-from` while the copy is live and names
  the copy.

### Cloud backend

- [ ] **Second host sees the lineage.** The chain is built on host A in
  workspace W1, and v3 is opened in workspace W2 on host A. Host B (its own
  HOME, same bucket, only v1 pulled) runs `koto session prune --apply --all`
  in W1: v1 is kept and names v3.
- [ ] **Unreadable keeps.** A relation whose body doesn't parse, and a
  successor whose bucket listing fails, each keep the predecessor; the prune
  entry reports the holder and why, and with a failed listing reports
  `bucket_unchecked`.
- [ ] **Unconfirmed write.** With the bucket refusing writes, R1's verb exits
  3 and no relation is present locally or in the bucket.
- [ ] **Bounded cost.** On the retention cost fixture, a prune where no
  session is about to be deleted makes no bucket request for relations, and a
  prune deleting N sessions with no relations makes at most a fixed multiple
  of N requests for them, whatever the number of sessions in the bucket.

### Cross-cutting

- [ ] **koto alone.** Every criterion above passes on a host with koto
  installed and no other tool.
- [ ] **No relation, no change.** The existing retention and pin test suites
  pass unchanged.
- [ ] **Docs.** The changelog and the session retention guide name the first
  release that honours the relation and describe what an older release does
  with it; the CLI guide documents R1's verb.

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
- **Retiring caller workarounds** built for the missing relation.

## Known Limitations

- An older koto sharing a store reads an unknown link kind as an ordinary
  edge, so its local prunes over-keep a chain rather than remove it, but under
  the cloud backend it never reads relations from the bucket and can delete a
  predecessor whose successor it never pulled. Its import still retires the
  source. Every host sharing a bucket needs this release.
- Explicit cleanup removes a session the relation keeps (R10). A successor
  that needs a predecessor kept against explicit removal pins it.
- Nothing orders two hosts: a successor created on one host after another
  host's removal has read the predecessor's relations, and before it deletes,
  doesn't stop that removal. The window is the time between that read and the
  delete, as for pins.

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
  live member of its own hierarchy keeps its predecessors too. The
  alternative, counting only the successor's own state, would let a chain go
  while its newest member is still kept, which is what "no sooner than that
  member's own retention allows" rules out.
- **One predecessor per session.** A session continues one session. Several
  would make "the successors between" ambiguous and add nothing a feature here
  needs; a predecessor may still have many successors.
- **Explicit cleanup removes and warns.** `koto session cleanup` doesn't
  refuse for a hierarchy keeper today, only for a pin, because the pin is the
  one keeper an operator set on purpose. The relation follows the hierarchy,
  and a pin stays the way to make a session survive explicit removal.
- **Import's age rule goes.** The source was removed by age because it was
  retired; once it is a kept predecessor, its lineage decides. The age setting
  and `--source-retired` are kept as accepted no-ops so scripts don't break.
- **Earlier imports count.** Reading the copy's `koto/imported-from` together
  with the source's marker as a relation keeps sources imported before this
  release, which is the safe direction; both records must agree, so a forged
  record on one side keeps or frees nothing.
- **The verb's surface is the design's.** The requirement is behaviour at
  open, independent of the launcher (R1); whether it is a flag on `koto init`,
  a `koto session` subcommand, or both is settled in the design.
