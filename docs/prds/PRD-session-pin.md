---
schema: prd/v1
status: Draft
problem: |
  koto keeps a finished session only while something live in its hierarchy
  holds it, plus a retention period, then removes it at the next cleanup,
  prune, terminal tick or subtree removal; under the cloud backend that
  removal deletes it from the bucket for every host sharing it. A finished
  session that is the record of finished work, reached later from a merged
  pull request, has no way to be kept, and nobody can say "keep this one" in
  any form koto reads.
goals: |
  Anyone can pin a finished koto session with a reason, and from then on no
  removal koto makes, on any host sharing the store, takes it; every removal
  that passes it over names the pin and its reason. Only an explicit unpin
  releases it, and an agent session releases one only when it was handed
  koto's release authority. Sessions nobody pinned keep exactly today's
  behaviour, and all of it works with koto and no niwa.
upstream: docs/briefs/BRIEF-session-pin.md
source_issue: 441
---

# PRD: Pin a finished session so no removal rule takes it

## Status

Draft

## Problem Statement

Some koto sessions matter after their work is over. A session that drove a
piece of work to a merged pull request holds what was done, in what order
and with what results, and a reviewer, an auditor or a later agent reaches
it from that pull request weeks afterwards.

koto has no way to keep one. Session retention, which koto now applies at
every removal by rule or subtree, keeps a finished session only while a
member of its hierarchy is alive: an open anchor (the koto session that
stands for a running agent session) holding it through a held-by link, a
running koto session joined to it by a dispatched-by link or lineage, and
then for the retention period after the hierarchy's release instant. Once
the agent sessions involved are gone and the retention passes, the next
`koto session prune`, `koto workspace prune`, terminal tick or same-name
replace removes the session, and `koto session cleanup` removes it at any
time whatever retention says. Under the cloud backend each of these deletes
the session from the bucket, so it's gone for every host that shares the
bucket, including hosts that never ran the work.

The person or agent who knows the record matters can't say so in any form
koto reads. Keeping an agent session open forever so its anchor holds the
record, never pruning, or copying the session out by hand are the only
options, none of them is a statement that this session should stay, and
none survives an operator's routine prune on a second host. The cloud case
is worse than it looks: a second host decides from its own local copy,
which holds only the keys it happened to read, and `koto session cleanup`
there deletes a bucket session the host never pulled.

### Terms

These terms are used from here on with exactly these meanings. Terms from
session retention keep their meaning: anchor, held-by and dispatched-by
links, hierarchy, alive, release instant, retention, removal by rule or
subtree.

- **Pin**: a koto-owned record in a session's own reserved context area that
  says the session is to be kept, with a reason. A session has at most one.
- **Pinned session**: a session whose pin is present, readable or not.
- **Unreadable pin**: a pin koto finds but can't parse or bound-check, or
  one the store lists but whose content is missing.
- **Setter** and **releaser**: the caller that pinned or unpinned, recorded
  as the anchor its environment named (or none) and the time.
- **Release authority**: koto's authority named `release-pin`, which permits
  an agent session to unpin.
- **Handed authority**: the koto authorities an agent session was given at
  its start or dispatch, through the variable `KOTO_AUTHORITY`.
- **By hand**: a call whose environment names no anchor, treated as the
  person at the shell.
- **Removal**: any koto command that deletes a session's log and keys:
  the terminal tick, the parent sweep, the same-name replace,
  `koto workspace prune`, `koto session prune`, `koto session cleanup` and
  `koto cancel --cleanup`.
- **Bucket check**: a fresh read of the bucket, made by the command about to
  delete, that answers whether the session holds a pin there.

## Goals

- A person or agent who finishes work worth keeping pins its session in one
  step, with a reason, and doesn't have to think about it again.
- No removal koto makes takes a pinned session, on this host or any other
  host sharing the cloud bucket, and each one that passes it over names the
  pin and its reason, apart from sessions kept because something is alive.
- A pin comes off only through an explicit unpin. Idle time, staleness and
  the retention period never take it off, and the next removal after an
  unpin treats the session like any other.
- An agent session releases a pin only when it was handed koto's release
  authority; a refused release says which authority it lacked.
- A pin koto can't read keeps its session and is reported.
- Sessions nobody pinned keep today's behaviour exactly.
- It all works on a host with koto and no niwa installed.

## User Stories

- **Keeping the record of finished work.** As an agent driving a piece of
  work to a merged pull request, I want to pin my koto session with a reason
  that names what it records before my run ends, so that a reviewer who
  follows the pull request back weeks later reads its log and keys as I left
  them, after my agent session and its anchor are long gone.
- **Routine prune on a second host.** As an operator on a second host that
  shares the cloud bucket, I want `koto session prune --apply --all` to
  remove the finished sessions nobody pinned and keep the pinned one, listing
  its pin and reason, so that my cleanup never deletes another host's record.
- **Cleaning up by hand.** As an operator, I want `koto session cleanup` on a
  pinned session to refuse and tell me it's pinned and why, so that I don't
  delete a record by accident and know that an unpin comes first.
- **Releasing a record nobody needs.** As a maintainer who owns the work a
  pinned session records, I want to unpin it from my shell when the record is
  no longer needed, so that the next prune reclaims it like any other
  finished session.
- **A worker without the authority.** As a coordinator that dispatched a
  worker without handing it the release authority, I want the worker's
  attempt to unpin a record to be refused and to name the authority it
  lacks, so that only sessions I trusted with it release records.
- **A damaged pin.** As an operator running prune after a pin was truncated
  by a failed write, I want the session kept and reported as kept because its
  pin can't be read, so that I repair or release it rather than lose it.
- **Finding every pin.** As an operator, I want to list every pinned session
  in the store with its reason, setter and age, so that I can find a runaway
  skill that pins everything.

## Requirements

### Setting and releasing a pin

- **R1. Pin a session.** `koto session pin <name> --reason <text>` records a
  pin in the named session. The reason is required, non-empty after trimming,
  at most 500 characters, and has control characters refused. The pin holds
  the reason, the setter's anchor (or none, by hand), the setter's koto
  session id hint where one exists, and the time. The command prints the pin
  as JSON and the store it was written to. It refuses, with exit 2 and
  nothing written, a name with no session, an anchor or an anchor's close
  record (pins are for koto workflow sessions), and a migrated source.
- **R2. A session may be pinned while it runs.** A running or finished
  session can be pinned; the pin takes effect whenever a removal would
  otherwise take the session, including at its own terminal tick.
- **R3. Pinning is idempotent.** Pinning a session that already holds a
  readable pin changes nothing, exits 0 and prints the existing pin. To
  change the reason, unpin and pin again.
- **R4. Setting a pin needs no authority.** Any caller may pin, by hand or
  under an anchor, with or without handed authority.
- **R5. Unpin a session.** `koto session unpin <name>` removes the pin when
  the caller may release it (R7), whatever the pin's content, so an
  unreadable pin can always be cleared. An optional `--reason <text>` is
  recorded with the release. Unpinning a session that holds no pin changes
  nothing and exits 0.
- **R6. A release is recorded.** Each unpin records, in the unpinned session,
  the releaser's anchor (or none, by hand), the authority it presented and
  where koto read it, the release reason when given, the time, and the
  released pin's reason and setter. The hierarchy query for the session shows
  the latest release until the session is removed.
- **R7. Who may release.** A caller whose environment names no anchor
  releases by hand and may unpin. A caller whose environment names an anchor
  may unpin only when its agent session holds the release authority among its
  handed authorities (R8). Otherwise the unpin is refused with exit 2,
  nothing changes, and the refusal names the authority it lacks
  (`release-pin`), the anchor koto judged as the caller, and that authority is
  handed to an agent session at its start or dispatch. The pin's setter gets
  no special standing: setting a pin never grants its release.
- **R8. How authority reaches koto.** koto reads an agent session's handed
  authorities only from what was handed to that session, never from niwa's
  records, niwa's commands or any other tool's state. They arrive in the
  variable `KOTO_AUTHORITY`, a comma-separated list of koto authority names,
  present in the agent session's environment when it starts. koto's
  Claude Code plugin records the handed list in the agent session's anchor
  when it opens the anchor, and removes the variable from the shells the
  session starts, so a worker or `claude -p` child launched from those
  shells inherits nothing; koto then reads the authorities an agent session
  holds from its anchor. Each start of the same agent session records what
  that start was handed. A successor anchor holds only what its own start
  was handed. Unknown names in the list are recorded, ignored for every
  decision, and reported by the hierarchy query. koto's vocabulary is a
  closed set per koto version and holds one authority in this version,
  `release-pin`.

### Keeping a pinned session

- **R9. A pin keeps its session and nothing else.** The retention rule treats
  a pin as a keeper of the pinned session with no release instant: the
  pinned session is kept by every removal for as long as the pin is present.
  A pin keeps only the session it's in. Other members of the pinned
  session's hierarchy are kept or released by the existing rule as if the
  pinned session were a finished member, and a pin adds no link to its setter
  or to anyone else, so it never joins hierarchies.
- **R10. Every removal keeps a pinned session.** The terminal tick (with or
  without `--no-cleanup`, and for a failure terminal), the parent sweep, the
  same-name replace, `koto workspace prune` (with or without `--force`) and
  `koto session prune` (whatever its filters) keep a pinned session. A
  same-name replace that meets a pinned finished predecessor keeps it
  readable under a new name, as the replace already does for a held
  predecessor, and the pin moves with it.
- **R11. Explicit removal refuses a pinned session.** `koto session cleanup`
  of a pinned session refuses with exit 2 and removes nothing, naming the pin,
  its reason and that unpinning comes first. `koto cancel --cleanup` of a
  pinned session refuses the same way before writing anything, so the session
  is neither cancelled nor removed; `koto cancel` without `--cleanup` still
  cancels it.
- **R12. Check the pin just before deleting.** Each removal reads the pin
  again from the session itself immediately before it deletes the session,
  so a pin set after the command started (including while
  `koto workspace prune` waits for its confirmation) keeps the session, which
  the output lists as kept with the pin.
- **R13. Unpinned sessions are unchanged.** A session that holds no pin is
  kept and removed exactly as today by every removal, with today's output,
  including when another member of its hierarchy is pinned (R9).
- **R14. An unpinned session is eligible again.** After an unpin the session
  is judged by the existing rule alone, and the next removal that would take
  an unpinned session in its position takes it.

### Saying why

- **R15. Every removal that keeps a pinned session names the pin.** The
  terminal tick's answer, the parent sweep's kept-children listing, the
  replace's answer, `koto workspace prune`'s kept list, `koto session prune`'s
  preview and apply, and the refusals of R11 each name the pinned session,
  the pin's reason, its setter and when it was set, and mark the session as
  kept by a pin, apart from sessions kept because a member is alive. A pinned
  session that is also failure-terminal or also held by a live member names
  the pin as well as the other reasons. No surface describes a pinned
  session as alive or as unreadable when its pin is readable.
- **R16. The hierarchy query shows pins.** `koto session hierarchy <name>`
  (text and JSON) shows, for a pinned session, the pin with its reason,
  setter and time, the latest release (R6), and that the pin keeps that
  session only; for another member of a pinned session's hierarchy, which
  members are pinned. It writes nothing.
- **R17. Pins can be listed.** `koto session list --pinned` lists every
  pinned session in the store with its reason, setter and when it was set,
  and every unreadable pin with why; its JSON form carries the same fields.

### Failing toward keeping

- **R18. An unreadable pin keeps its session.** When koto finds a pin it
  can't read, the session is kept by every removal and the removal reports
  the session as kept because its pin can't be read, with why. An unreadable
  pin keeps only its session, like a readable one.
- **R19. A pin a store lists but can't produce keeps its session**, and so
  does a pin present without the store's listing of it, so a write
  interrupted between the two still keeps.

### Cloud backend

- **R20. A pin reaches the bucket or the command fails.** Under the cloud
  backend, `koto session pin` succeeds only once the bucket holds the pin; if
  it can't confirm that, it exits non-zero (3), says the pin is held in the
  local store only, and a later koto command on the session retries the push.
  `koto session unpin` likewise succeeds only once the bucket no longer holds
  the pin.
- **R21. Every removal checks the bucket before deleting.** Under the cloud
  backend, before any removal deletes a session's local or bucket copy, it
  makes a bucket check for that session. A pin found there keeps the session
  on this host and in the bucket, and the removal reports it (R15) from the
  bucket's pin, even when this host's local copy holds no pin or the host
  never pulled the session. This holds for `koto session cleanup` of a name
  the host has no local copy of.
- **R22. A failed bucket check keeps.** When the bucket check can't be made
  or can't be read, the removal deletes neither copy and reports that the
  bucket couldn't be checked for a pin; an explicit removal then exits
  non-zero. A prune keeps that session, reports it and carries on with the
  rest.
- **R23. Keeping doesn't depend on another host's store.** A pin keeps its
  session from the pin alone, so a removal on a second host keeps and
  reports a pinned session even when the anchors and other members that hold
  it live only in the first host's local store, as happens when the cloud
  backend is chosen by environment and koto's start hook opens anchors in
  the local store.

### Import and compatibility

- **R24. A pin travels with import.** `koto session import` carries a
  session's pin to the moved copy unchanged, including its setter and time.
  The migrated source keeps nothing, as today.
- **R25. Older koto builds keep a pinned session.** A koto release that
  reads session-retention links but predates this feature reads a pin as an
  unreadable record, never as an ordinary link, so its prunes, terminal tick
  and replace keep a pinned session (with a reason naming the unreadable
  record) rather than remove it. koto's guide and changelog state which
  releases honour pins in full, that older releases' `koto session cleanup`
  and `koto cancel --cleanup` and imports don't, and that every host sharing
  a bucket needs a release that does.

### Non-functional

- **R26. No niwa.** Every requirement holds on a host with koto and its
  plugin and no niwa installed. A dispatcher other than niwa, or a person
  starting a session, hands authority with the same variable. koto's guide
  says how a niwa workspace passes `KOTO_AUTHORITY` through to workers with
  its generic pass-through list, without niwa naming koto.
- **R27. Bounded cost.** Locally, pins keep the existing bounds: on the
  session-retention cost fixture (2,000 sessions in 200 hierarchies) with 200
  sessions pinned, the hierarchy query for one session answers within 1
  second and a full prune preview within 10 seconds on the CI runner,
  enforced by a test. Under the cloud backend, the bucket check adds at most
  one bucket request per session a removal would delete, plus one read of a
  pin it finds, and no request for a session the removal keeps for another
  reason.
- **R28. Writes stay with their session.** Pin and unpin write only into the
  named session, plus the release record of R6 in that session; recording
  handed authority writes only into the agent session's own anchor. Nothing
  writes into another session, and `koto context add` and
  `koto context remove` refuse the pin's key area as they refuse the rest of
  koto's reserved keys.

## Acceptance Criteria

### Setup and conventions

Unless a criterion says otherwise: retention durations are set through the
environment to short values (for example retention 20s, idle period 10s,
stale threshold 60s); every agent session involved has been deleted and its
anchor closed; "after the retention" means after waiting past it from the
hierarchy's release instant; and P is a finished koto session pinned by hand
with reason `record of the merged change`, while U is a finished session in
the same store with no pin and no live member, past its retention.

**Removal set S**, run against P and U in this order: S1, P's and U's own
terminal steps without `--no-cleanup`; S2, a koto parent X of P reaching its
terminal step without `--no-cleanup`, so the parent sweep runs; S3,
`koto session prune --apply --all`; S4, in a separate pass,
`koto workspace prune --root X --yes`; S5, `koto session cleanup P` and
`koto session cleanup U`; S6, a same-name replace of P's topic
(`koto init --replace-terminal`).

### Keeping and reporting

- [ ] **Pinned survives every removal.** After each step of S, P's log and
  keys are readable and byte-identical to before the step, and U is removed by
  the first step that applies to it, as on a build without this feature.
- [ ] **Each removal names the pin.** S1's tick answer, S2's kept-children
  listing, S3's preview and apply, S4's kept list, S5's refusal for P and S6's
  replace answer each carry P's pin with reason `record of the merged change`,
  its setter (none, by hand) and its time, and mark it as kept by a pin.
  None of them describes P as alive or unreadable.
- [ ] **Cleanup refuses.** `koto session cleanup P` exits 2, P is unchanged,
  and the message names the pin, its reason and that an unpin comes first.
  `koto cancel --cleanup` on a running pinned session exits 2 and writes no
  event into it; `koto cancel` on it then cancels it and leaves it pinned.
- [ ] **Replace moves a pinned predecessor aside.** S6 leaves P readable under
  the new name its answer reports, still pinned with the same reason and
  setter, and the new run holds the original name with no pin.
- [ ] **Pinned while running.** A session pinned before its terminal step is
  kept at that step, both with and without `--no-cleanup`, and a
  failure-terminal pinned session's answer names the pin as well as the
  failure terminal.
- [ ] **Pin keeps its session only.** A dispatches B and pins A; both finish,
  nothing is alive. After the retention, `koto session prune --apply --all`
  keeps A, naming its pin, and removes B. The hierarchy query for A says the
  pin keeps A only, and for B (before the prune) names A as a pinned member.
- [ ] **Unpinned members unchanged.** With A pinned and C, a finished session
  in A's hierarchy, held by a live anchor, the prune keeps C naming only the
  anchor, exactly as without the pin.
- [ ] **Late pin.** `koto workspace prune --root X` (without `--yes`) shows P
  unpinned for removal; P is pinned while the prompt waits; after confirming,
  P is kept and listed as kept with the pin. The same holds for
  `koto session prune --apply` when P is pinned between preview and apply.
- [ ] **Hierarchy query.** For a pinned session, `koto session hierarchy`
  text and JSON show the pin's reason, setter and time and that it keeps that
  session only, and the session's files hash the same before and after.
- [ ] **Listing pins.** With three pinned sessions, one with an unreadable
  pin, and ten unpinned, `koto session list --pinned` lists exactly the three,
  each with reason, setter and time, the unreadable one with why; the JSON
  form carries the same.

### Setting, releasing and authority

- [ ] **Pin command.** `koto session pin P --reason <r>` exits 0 and prints
  the pin and the store. Pinning P again with any reason exits 0, changes
  nothing and prints the existing pin. An empty or whitespace reason, one over
  500 characters, one with a control character, a missing session, an anchor,
  a close record and a migrated source each exit 2 and write nothing.
- [ ] **Unpin by hand makes it eligible.** From a bare shell, unpinning P
  exits 0; the next `koto session prune --apply --all` after the retention
  removes P. Unpinning an unpinned session exits 0 and changes nothing.
- [ ] **Unpin clears an unreadable pin.** With P's pin truncated, an unpin by
  hand exits 0 and leaves P with no pin.
- [ ] **Release record.** After an unpin with `--reason done`, the hierarchy
  query for P shows the release: releaser (none, by hand), authority presented
  (none), reason `done`, time, and the released pin's reason and setter.
- [ ] **Refused without authority.** Under an anchor whose agent session was
  started with no `KOTO_AUTHORITY`, `koto session unpin P` exits 2, P keeps
  its pin, and the refusal names `release-pin`, the caller's anchor, and that
  the authority is handed at session start or dispatch. The same holds when
  `KOTO_AUTHORITY` lists only unknown names.
- [ ] **Allowed with authority.** Under an anchor whose agent session was
  started with `KOTO_AUTHORITY=release-pin`, the unpin exits 0 and the release
  record names that anchor and `release-pin`.
- [ ] **Authority isn't inherited.** Run with synthetic hook inputs, a session
  started with `KOTO_AUTHORITY=release-pin` has its anchor record
  `release-pin`, its shells see no `KOTO_AUTHORITY`, and a `claude -p` child
  launched from them, and a successor anchor opened after an observed close
  whose start was handed nothing, are each refused an unpin.
- [ ] **Exporting mid-session doesn't grant.** Under an anchor whose start was
  handed nothing, running `KOTO_AUTHORITY=release-pin koto session unpin P`
  is refused with the same message.
- [ ] **Setter gets no release.** The agent session that pinned P, holding no
  release authority, is refused an unpin of P.
- [ ] **Pinning needs no authority.** Under an anchor with no handed
  authority, `koto session pin` succeeds and records that anchor as setter.
- [ ] **Single writer.** During pin, unpin and the authority recording, every
  session other than the named session and the caller's own anchor is
  byte-identical before and after, and `koto context add` and
  `koto context remove` on the pin's key exit 2.

### Failing toward keeping

- [ ] **Unreadable pin keeps.** With P's pin made unparseable, over its size
  bound, a symlink, a directory, or listed by the store with its content
  missing, every step of S keeps P and reports it as kept because its pin
  can't be read, naming why; P's hierarchy siblings are judged as if P were
  an ordinary finished member.
- [ ] **Unlisted pin keeps.** A pin present in P's store with the store's
  listing of it removed keeps P through S.

### Cloud backend

All cloud criteria run against a stand-in bucket shared by two hosts (two
working copies at the same canonical path with separate local stores, or two
machines), with the cloud backend set by environment on both.

- [ ] **Second host keeps.** Host 1 pins P and finishes; host 2 has never
  pulled P. On host 2, `koto session cleanup P` exits 2 naming the bucket's
  pin and reason and deletes nothing in the bucket; host 2's
  `koto session prune --apply --all`, its terminal tick of a session it shares
  with P's hierarchy, and `koto workspace prune --root X --yes` over a stale
  local copy of P without the pin each keep P in the bucket and name the pin
  and reason. U, unpinned in the same bucket, is removed by the same commands.
- [ ] **Stale write doesn't unpin.** Host 2 holds a local copy of P taken
  before the pin, writes a context key into it (pushing its manifest), and
  then runs the removals above; P is still kept and its pin still reported.
- [ ] **Anchors elsewhere.** P is held by an anchor that lives only in host
  1's local store (the start hook dropped `KOTO_SESSION_*`); host 2's removals
  keep P from its pin alone and report the pin, not the anchor.
- [ ] **Failed bucket check keeps.** With the bucket unreachable for the
  check, `koto session cleanup P` and `koto session cleanup U` each exit
  non-zero and delete neither copy, and `koto session prune --apply --all`
  keeps both, reporting that the bucket couldn't be checked, and exits as it
  does when it skips a session.
- [ ] **Pin reaches the bucket or fails.** With the bucket refusing writes,
  `koto session pin P` exits 3 and says the pin is held locally only; once
  the bucket accepts writes, the next koto command on P pushes it and host 2
  sees it. `koto session unpin` with the bucket refusing deletes exits 3 and
  P stays pinned in the bucket.
- [ ] **Unpin on one host frees on another.** Host 1 unpins P by hand; after
  the retention, host 2's prune removes P from the bucket.
- [ ] **Cost.** On the stand-in bucket with a fixed per-request latency, a
  `koto session prune --apply --all` over 20 finished sessions, 5 of them
  pinned, makes at most one bucket check per deleted session plus one pin
  read per pin found, counted by the stand-in.

### Import and compatibility

- [ ] **Import carries the pin.** With the per-command cloud configuration in
  the environment, importing P into another workspace leaves the moved copy
  pinned with the same reason, setter and time; S there keeps it; the
  migrated source keeps nothing.
- [ ] **Older koto keeps.** With a pin written by this build, the published
  koto 0.23.0 release binary's `koto session prune --apply --all`,
  `koto workspace prune --root X --yes`, terminal tick and same-name replace,
  run on the same store, each keep P, with a reason naming an unreadable
  record.
- [ ] **Changelog and guide.** koto's changelog for the release and its
  session-retention guide state which releases honour pins in full and what
  older releases do, and the guides that tell a reader to run
  `koto session cleanup` as a recovery step say to unpin a pinned session
  first.

### Cross-cutting

- [ ] **No niwa.** On a host with no niwa installed, Pinned survives every
  removal, Cleanup refuses, Refused without authority, Allowed with
  authority, Unreadable pin keeps and Second host keeps pass with koto and
  koto-skills alone.
- [ ] **Unchanged for unpinned.** koto's existing retention and removal tests
  pass unchanged.
- [ ] **Cost.** The R27 local test passes on the CI runner.

## Out of Scope

- **Releasing a pin by idle time, staleness, age or any rule.** Only an
  explicit unpin takes a pin off.
- **Changing how unpinned sessions are kept or removed.** The retention rule,
  its settings and its reasons stay as they are for them.
- **Pinning a whole hierarchy in one call.** A pin keeps its own session; to
  keep more, pin each session that matters. A hierarchy-wide pin can be added
  later without changing the pin format.
- **Defining or granting authority beyond koto's own name.** What
  authorities niwa or any other tool defines, who grants them and how a
  dispatcher decides to hand `release-pin` on are outside koto. koto reads
  only what's handed to a session.
- **Enforcement stronger than today's.** A caller can unset its anchor
  variable to act by hand, and anyone with file access can delete the store;
  the release record shows the first, and nothing here prevents the second.
- **An automatic pruner, or any change to when removals run.**
- **Host-level stores**: the request store, the decider ledger, the terminal
  index, finished records.
- **Removals inside a run koto is still driving.** Batch and retry respawns
  that delete a child koto just created or is re-running don't consult pins.
- **koto requiring niwa, or niwa naming koto.** niwa needs no change; a
  workspace declares the variable in its existing pass-through list.

## Known Limitations

- **Older builds on a shared bucket.** A host running a koto release from
  before this feature can still remove a pinned session with
  `koto session cleanup` or `koto cancel --cleanup`, or drop the pin on
  import, and releases before session retention ignore pins entirely. Every
  host sharing a bucket needs a release that honours pins (R25).
- **A bypass shows rather than stops.** An agent that unsets `KOTO_ANCHOR` is
  treated as acting by hand and may unpin; the release record then shows a
  by-hand release where no person was present.
- **A host with no bucket access can't remove under cloud.** Because a failed
  bucket check keeps (R22), a host whose bucket is unreachable removes
  nothing until it can check.
- **A pin keeps storage until released.** A skill that pins every run grows
  the store and the bucket without bound; listing pins (R17) and the prune
  reports make that visible, and nothing limits it.
- **A narrow race on a second host.** A pin written on one host between
  another host's bucket check and its deletes can leave a pin object with no
  session behind it; the session is gone. The check runs immediately before
  the deletes to keep the window small.

## Decisions and Trade-offs

- **A pin keeps its own session only, not its hierarchy.** Retention's verdict
  covers a whole connected component, so a pin counted literally as a live
  member would keep everything its hierarchy touched, and an agent that pins
  while its anchor is still open would keep everything that anchor touches
  afterwards, with no release instant. A session-scoped pin keeps exactly the
  record someone asked for; the alternative, hierarchy-wide, was rejected for
  that unbounded growth and can be added later as an option.
- **Release needs a koto-named authority handed to the session; a call by
  hand may release.** Deciding from the setter's identity or the hierarchy
  was rejected: it's koto guessing, the refusal couldn't name an authority,
  and the setter is usually gone by release time. niwa's named authorities
  are niwa's closed set, and koto may not read niwa's record, so the name and
  the variable are koto's own and any dispatcher hands them, the way the
  dispatcher's anchor is handed today. A by-hand release keeps the issue's
  interim rule (release stays with whoever runs the command by hand) and the
  escape hatch for a mistaken pin.
- **Authority is what the session was handed at its start, recorded in its
  anchor, and the variable is removed from its shells.** Reading the variable
  at unpin time was rejected: an agent could export it mid-session, and any
  child launched from the shell would inherit it. This mirrors how the start
  hook already records niwa's reap key once and removes it from the shells.
- **Setting a pin needs no authority.** A pin errs toward keeping and a
  person can always undo it; requiring authority would break the first user story
  for every agent nobody handed anything to, which on a host without niwa is
  every agent. A required reason, a recorded setter, a pin listing and the
  prune reports make pins visible instead. The authority list could later
  carry a pinning authority as an additive change.
- **Explicit removal refuses a pinned session.** Session retention lets
  `koto session cleanup` remove whatever retention says; a pin is a stronger
  statement than a live member, and leaving cleanup able to delete it would
  make an unpin pointless as a gate. Refusing (rather than keeping silently)
  tells the operator what to do. `koto cancel --cleanup` refuses before it
  cancels, so a refused command changes nothing.
- **Under the cloud backend every removal checks the bucket, and a failed
  check keeps.** A second host decides from a local copy that may lack the
  pin, and one host's write can replace the bucket's index of a session's
  keys; trusting the local copy, or relying on the prune's existing check
  that the bucket copy isn't ahead, would delete records. The cost is one
  bucket request per session a removal deletes, which is the listing a
  delete already makes.
- **Pin writes are strict under the cloud backend.** Today a context write
  whose push fails only warns; a pin that silently stays local gives false
  confidence on every other host.
- **Older builds must keep, not release.** The pin's stored form is chosen so
  that a release with session-retention links but no pin support reads it as
  unreadable, which keeps, rather than as an ordinary link, which releases.
- **One pin per session; re-pinning is idempotent.** Several pins with their
  own setters and reasons were considered; one pin keeps the release rule and
  every report simple, and a second party who needs the record can see the
  existing pin.
- **Pins are for koto workflow sessions only.** Pinning an anchor or a close
  record would keep only bookkeeping sessions under the session-only rule,
  which is never what's wanted.
