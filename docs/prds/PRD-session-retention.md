---
schema: prd/v1
status: In Progress
problem: |
  Agent sessions dispatch each other, and the sessions above and below a
  finished worker need its koto log and keys afterwards. koto removes a
  finished session at six points without asking whether any related session
  is still alive. Its one relation-aware rule is a one-hop prune rule over
  lineage that import drops, and it has no notion of an agent session that
  sits idle and resumable after its koto run is done.
goals: |
  A koto session stays readable for at least as long as any session in its
  dispatch hierarchy is alive, where an agent session is alive until its
  deletion is observed or, where nothing observes it, until it has been idle
  for a configured period. Every removal koto makes by rule or subtree obeys
  that and says why it keeps what it keeps. It works on one machine with koto
  alone, survives `koto session import`, and the small workspace-manager and
  skill pieces stay inert where their tool is absent.
absorbed:
  - docs/briefs/BRIEF-session-retention.md
---

# PRD: Session retention across the dispatch hierarchy

## Status

In Progress

Absorbed [BRIEF-session-retention](docs/briefs/BRIEF-session-retention.md); carried in Absorbed Brief.

## Absorbed Brief

The feature was framed before these requirements existed. Agent sessions
hand work to each other, and whoever sits above or below a finished worker
needs its koto log and keys afterwards: a coordinator to check or undo what
it decided, a person to see why, a worker to re-read its dispatcher's
record. koto removed finished sessions at several points without asking who
still needed them, the relations it could follow did not survive an import,
and an idle but resumable agent session, or a person's own session with no
koto run, looked to koto as already gone. The framed outcome: a session
stays readable for as long as any session in its dispatch hierarchy is
alive, an idle resumable agent session counting as alive; koto says why it
keeps what it keeps and what a person's explicit removal broke; and it all
works on one machine with koto alone and after an import. The framing set
the boundary these requirements and Out of Scope operationalize: koto-owned
links each session keeps about itself, an anchor per agent session closed
by an observed deletion or an idle period, one rule at every removal by rule
or subtree, reads that leave the read session unchanged, and only the small
generic workspace-manager and skill pieces outside koto; no decision
records, no authority, no cloud turned on, and no other harnesses.

## Problem Statement

Agent sessions hand work to each other. A person starts a coordinator, the
coordinator dispatches workers, and a worker may run a chain of skills or
dispatch workers of its own. Each keeps its working state in koto sessions,
an event log plus context keys. After a worker finishes, the sessions above
it still need that record: a coordinator reads what the worker decided and
may undo it, and a person reads it to see why something happened. The worker
may in turn need the record of the session that dispatched it.

koto removes finished sessions without asking any of them:

- A tick that reaches a success terminal without `--no-cleanup` removes the
  session's log and keys on the spot.
- The parent sweep removes a removed parent's terminal koto children.
- A same-name replace (`koto init --replace-terminal`), which every re-run of
  a skill topic goes through, removes the finished predecessor.
- `koto workspace prune --root` removes a root and its whole subtree,
  whatever their state.
- `koto session prune` removes finished sessions as a set.
- `koto session cleanup` and `koto cancel --cleanup` remove a named session.

Only `session prune` keeps a finished session because a related one is
running, and only one hop away, over koto's parent-child lineage or a single
shirabe-owned key, in the local store, when someone runs it by hand. A
finished session in the middle of a chain breaks the protection.

The relations koto could follow don't survive what happens to sessions.
Lineage doesn't survive `koto session import`, which refuses a child and
carries no children. Request legs live in a host-local store the import
drops. The workspace manager's record of who dispatched whom holds harness
ids koto never sees, and it's deleted when the instance is reaped.

And koto's only notion of "alive" is a workflow that hasn't finished. The
sessions that dispatch are agent sessions, which often sit idle and
resumable after their koto run, or hold no koto session at all, as a
person's own session usually does. To koto those are already gone, so the
workers they dispatched lose protection while the person can still resume
the session and ask about them.

The repository owner ruled on the shape of the fix: think of sessions as a
hierarchy; a session is accessible at least for as long as any session that
dispatched it or was dispatched by it is still alive. The rule binds agent
sessions, each carried into koto as an anchor that stays open while the
agent session exists and closes on an observed deletion or, where nothing
observes deletion, after a configured idle period; then a configured
retention runs. koto and the workspace manager (niwa) never require each
other, and only shirabe may require koto.

### Terms

These terms are used from here on with exactly these meanings.

- **Agent session**: one harness conversation that a person, a coordinator
  or niwa starts, for this feature a Claude Code session. Resuming it keeps
  it the same agent session. A `claude -p` child launched through a
  session's shell is a separate agent session with its own start. A subagent
  the Agent tool runs inside a session belongs to that session.
- **koto session**: a koto workflow's log and context keys in the session
  store.
- **Anchor**: the one koto session that stands for an agent session. It is
  opened when the agent session starts, its workflow never finishes, and it
  is either *open* or *closed*.
- **Observed**: an anchor is observed when its agent session will be reaped
  by niwa, which then sees the agent session's deletion. The anchor's
  **observed marker** is the record, in the anchor's own context, that it is
  observed.
- **Link**: a koto-owned record in a session's own context naming another
  session. A **dispatched-by** link names a session that dispatched this
  one. A **held-by** link names the anchor of an agent session that opened
  this session or ticked it. A session may hold several of each.
- **Hierarchy**: the connected component of sessions joined by edges, taken
  in both directions and through finished members. Edges are links, koto's
  parent-child lineage, and shirabe's `chain/parent` key.
- **Alive**: a koto session other than an anchor is alive while its log is
  neither terminal nor cancelled; an unreadable session counts as alive. An
  anchor is alive while it is open.
- **Stop instant**: when a member stopped being alive. For a koto session,
  its terminal or cancel event's timestamp. For an anchor, its close
  instant: the recorded time of an observed close, the last activity plus
  the idle period for an idle close, or the last activity plus the observed
  backstop for a backstop close. When more than one applies, the earliest
  wins. An observed close is final; an idle or backstop close is a reading,
  so new activity in the anchor or a session it holds makes it read open
  again.
- **Last activity** of an anchor: the latest event timestamp in the anchor
  or in any session that holds a held-by link to it. Reads recorded in other
  sessions are not activity of the read session.
- **Release instant** of a hierarchy: the latest stop instant among its
  members, once none is alive. Absent and migrated members have none; the
  session being judged is present and always has one.
- **Retention**, **idle period**, **observed backstop**: the three
  configured durations (R24).
- **Migrated**: a session that `koto session import` copied elsewhere. Its
  source carries koto's migrated marker under the cloud backend, or, on the
  local store, is the copy an import left behind.
- **koto-level reading**: how a session with no held-by link is treated
  (R13).
- **Removal by rule or subtree**: the terminal tick, the parent sweep, the
  same-name replace, `koto workspace prune --root`, and `koto session prune`.
  **Explicit removal**: `koto session cleanup` and `koto cancel --cleanup`.
- **Prune preview** and **prune apply**, in acceptance criteria:
  `koto session prune --all` and `koto session prune --apply --all`. "The
  prune keeps X and names Y" means the preview lists X as kept with Y in its
  reason, and the hierarchy query for X gives the same reason.

## Goals

- A coordinator, or the person behind it, reads everything a finished
  worker recorded, and a worker reads the record of the session that
  dispatched it, for as long as any session in that hierarchy is alive. This
  widens the owner's one-hop wording to the whole hierarchy on purpose: a
  coordinator has to reach a grandchild after the middle session finished.
- An idle, resumable agent session counts as alive: until its deletion is
  observed where niwa observes it, and until the idle period passes where
  nothing does.
- Every removal koto makes by rule or subtree keeps a session whose
  hierarchy is alive or inside its retention, and says which members keep
  it. A person's explicit removal still removes, and says which live
  hierarchy it broke.
- Reading a session from another agent session leaves it byte-identical.
- It all works on one machine with koto and its plugin alone, with no
  workspace manager and no cloud backend, and it holds after a session moves
  by `koto session import`.
- A niwa user who never installs koto sees nothing change.

## User Stories

These carry the brief's journeys and add the two dispatch shapes the brief
names only in passing.

- **Coordinator reviewing a finished worker.** As a coordinator that
  dispatched a worker, I want to read the worker's log and keys after its
  terminal step, the parent sweep and a prune have all run, so that I can
  check and, if needed, undo what it decided. The prune names me as why the
  worker was kept.
- **Worker reading its finished dispatcher.** As a worker whose dispatcher
  has finished its own run, I want the dispatcher's record to stay readable
  while I'm working, so that I can re-read a decision it recorded.
- **Developer with koto alone.** As a developer running koto and its Claude
  Code plugin on a laptop with no niwa, I want the helper my session
  dispatched to stay readable while I'm working, and to be reclaimable once
  my session has been idle for the idle period and the retention has passed,
  so that I neither lose records nor collect them forever.
- **A person's own session with no koto run.** As a person whose own Claude
  Code session dispatches workers but runs no koto workflow itself, I want my
  session to count as alive while I use it, so that my workers' records stay
  readable to me.
- **Idle coordinator in a reaped instance.** As an operator whose
  coordinator was launched into an instance niwa reaps, I want the
  coordinator's workers to stay readable for as long as the coordinator can
  be resumed, however long it idles, and released only after its session is
  deleted and the retention has passed.
- **Skill on the message return path.** As a skill author whose worker is
  dispatched with no request leg, I want the worker's own opener to record
  its dispatcher, so that the worker is kept with its coordinator's
  hierarchy like a leg-bound one.
- **Moving a hierarchy.** As an operator moving a session to another
  workspace by import, I want it to keep its place in the hierarchy by
  session name, with the old copy pinning nothing, and a session imported
  with no agent session behind it to fall back to the koto-level reading
  rather than being kept forever.
- **Cleaning up by hand.** As an operator, I want `koto session cleanup` to
  remove a session I name even if a live coordinator still reads it, and to
  tell me which live hierarchy lost a member.
- **Auditing what is kept.** As an operator running prune, I want each kept
  session to name every live member keeping it, or the release instant still
  inside its retention.

## Requirements

### Links

- **R1. Links are koto-owned keys, written only by the session they're in.**
  A session's links live in a reserved area of its own context that only
  koto writes; `koto context add` refuses a key there. Each link names its
  peer by session name, with the peer's workspace path and session id kept as
  hints. One writer per session: every koto command this feature adds or
  changes writes only into the session the command opens, ticks, links or
  imports, or into the anchor of the agent session running the command (a
  start event each time the session starts, R6, its observed marker, R9,
  and its read records, R21). It never writes into a
  dispatcher, a peer, or another agent session's anchor.
- **R2. A leg-bound dispatch writes its link.** When a session is opened
  with `koto init --koto-leg`, or attaches itself to a leg with `koto request
  attach` naming its own session, koto writes a dispatched-by link in that
  session naming the request's `requested_by`, when that value passes koto's
  session-name rule; otherwise it writes no link. An attach naming a session
  other than the caller's own writes nothing.
- **R3. A dispatch with no request writes its link through a verb.** koto
  offers a command that records a dispatched-by link in a named session,
  meant to be run by that session's own driver, and an equivalent flag on
  `koto init`. Recording a link that already exists changes nothing and
  exits 0. Naming a dispatcher that fails koto's session-name rule is
  refused.
- **R4. Sessions an agent session opens are held by its anchor.** When the
  environment names an anchor, `koto init` records a held-by link to it in
  the new session, and a tick of an existing session under an anchor it holds
  no link to adds one; a tick under an anchor it already holds adds nothing.
  An anchor is opened with no anchor named and never holds a held-by link.
- **R5. Links alone are enough.** Every retention outcome in the acceptance
  criteria holds with no koto lineage and no request store present. Lineage
  and `chain/parent` are additional edges: they can join sessions into a
  hierarchy, and nothing depends on them. A link whose peer is absent from
  the local store makes the peer a member that is not alive, with no stop
  instant.

- **R5a. Close records and released anchors.** Each observed close is
  recorded in its own session, one per closed anchor, never shared. That
  session is finished from the moment it is written, so it is never alive,
  and its stop instant is the close it records. It holds a link naming the
  anchor it closes, which makes it a member of that anchor's hierarchy, so
  it is removed only together with that
  anchor, never before it. An anchor, open or closed, is a member like any
  other: once its hierarchy is released and the retention has passed, prune
  removes it with the rest.

### Anchors

- **R6. koto's Claude Code plugin opens one anchor per agent session.** The
  koto-skills plugin's session-start hook opens the agent session's anchor
  and makes its name available to every shell command the session runs,
  through a koto-prefixed variable. Every start, first or repeated, records a
  start event in the anchor. A start fired again by resume or compaction
  reuses the anchor, so an idle-closed anchor reads open again with that
  event. If the anchor has been removed, the start opens it afresh,
  and if it reads closed by an observed close, the start opens a new anchor
  that holds a dispatched-by link to the closed one. After `/clear`, the
  anchor the session's
  commands name is the same anchor or one linked to it, so the hierarchy is
  never split. With koto not installed, the hook exits 0 and prints nothing;
  no koto failure fails a session start.
- **R7. koto-skills' hooks load.** The plugin declares its hooks at
  `plugins/koto-skills/hooks/hooks.json` in the shape Claude Code documents
  for plugin hooks (each event maps to a list of matcher groups, each with a
  `hooks` list of command entries), and the repository's tests fail if the
  file is missing, moves, or breaks that shape.
- **R8. The anchor records its dispatcher.** When an agent session starts
  with a dispatcher's anchor named in its environment, its anchor records a
  dispatched-by link to it at open, with no name typed by anyone, and the
  variable the session's commands then see names the session's own anchor.
- **R9. The anchor records whether it is observed.** When niwa's variable
  for a session it will reap is present, the anchor records an observed
  marker: at open for a session `niwa dispatch` launched, and, for a session
  `niwa instance from-hook` set up, within three minutes of the session's
  start, written by any process the agent session's own start hook began. The plugin removes
  niwa's variable from the environment of the shells the session starts, so
  a `claude -p` child launched through the shell is not observed.
- **R10. An observed anchor closes when niwa observes its deletion.** koto
  provides a close command, and the plugin documents the guarded form a
  workspace declares as niwa's on-reap command. Given niwa's facts about a
  session niwa removed, it records that each anchor carrying the matching
  observed marker closed, and when, in a koto session of the close command's
  own, never in the anchor; anchors with other markers are untouched.
  Readers honour the record. Running it twice, or for facts that match no
  anchor, changes nothing and exits 0. In its guarded form with koto not
  installed, it exits 0.
- **R11. An unobserved anchor closes after its idle period.** An anchor with
  no observed marker is closed once the idle period has passed since its last
  activity. koto computes this when it reads the anchor; nothing writes into
  the anchor to close it.
- **R12. An observed anchor whose close never lands closes after the
  backstop.** An observed anchor with no close record is closed once the
  observed backstop has passed since its last activity. This covers a destroy
  that keeps its session, a failed or skipped close command, an instance
  deleted by hand, and a host where niwa was removed.
- **R13. A session with no anchor uses the koto-level reading.** koto run
  from a bare shell, a harness without the plugin, or a failed hook opens
  sessions with no held-by link. Such a session is alive while its koto
  session is open; if it belongs to a hierarchy, the rule then keeps it until
  the hierarchy's release instant plus the retention. koto neither refuses
  nor warns when a session opens with no anchor named.

### Retention

- **R14. One retention rule.** A session may be removed by rule or subtree
  only when no member of its hierarchy is alive and the retention has passed
  since the hierarchy's release instant. Two finished members never keep each
  other: a hierarchy with nothing alive is released.
- **R15. The terminal tick keeps hierarchy members.** A session that has any
  edge (it holds a link, another session's link names it, or it has lineage)
  is kept at its terminal step whether or not the tick passed `--no-cleanup`,
  with no retention evaluated at that moment, and the tick's answer gives the
  reason `hierarchy`. A session with no edge keeps today's terminal behaviour.
  Delivery to a parent and to a bound leg is unchanged. koto's changelog
  entry for the release says that the end-of-run removal no longer removes a
  session that has a link or lineage, and that prune reclaims it later.
- **R16. The parent sweep, workspace prune and session prune apply the rule
  and give reasons.** The parent sweep skips a terminal child the rule keeps
  and lists each skipped child with its reason in the tick's answer.
  `koto workspace prune --root` removes only the subtree members the rule
  lets go and lists the rest with reasons. `koto session prune` applies the
  rule over whole hierarchies in preview and apply, whatever its filters
  (`--execution-dir`, `--older-than`, `--all`), which only narrow which
  sessions are candidates and never which sessions count as members; at
  apply it re-checks each session, and one the rule now keeps is skipped and
  listed with its reason, with exit 0.
- **R17. A re-run keeps a held predecessor readable.** When a same-name
  replace meets a finished session the rule keeps, the predecessor stays
  readable with its log, keys and links intact under a new name, derived from
  the original and unique across repeated re-runs, which the replace's answer
  and the hierarchy query report (the format is the design's); the new run
  takes the original name. A
  predecessor the rule lets go is replaced as today.
- **R18. Explicit removal still removes, and says what it broke.**
  `koto session cleanup` and `koto cancel --cleanup` remove the named
  session as today. When the rule would have kept it, the output names every
  other live member of its hierarchy (the removed session itself is not
  named); when it wouldn't, the output names none.
- **R19. koto says why it keeps a session.** Wherever the rule keeps a
  session (the terminal tick, the sweep, both prunes, the replace, and a
  hierarchy query for one named session), koto names the reason: every live
  member keeping it, each with its kind (koto session or anchor) and, for an
  anchor, whether it is observed; or, when nothing is alive, the release
  instant, the member and close kind it came from, and when the retention
  passes. The hierarchy query prints this human-readable and as JSON, and
  writes nothing.
- **R20. Migrated sessions pin nothing.** A migrated session is neither
  alive nor able to keep another, and a link naming it resolves to the
  moved copy by name where that copy is in the local store, and otherwise
  to an absent peer (R5). A migrated session contributes no stop instant.

### Reading from another agent session

- **R21. A read from another agent session leaves the read session
  byte-identical.** `koto context list`, `koto status`, `koto session list`,
  the hierarchy query and a prune preview write nothing anywhere. The two
  commands that log a read today, `koto context get` and `koto context
  exists`, follow this table, with "reader anchor" being the anchor the
  reader's environment names, open or closed:

  | Reader anchor | Read session | What is written |
  |---|---|---|
  | present, holds the read session | any | the read is logged in the read session, as today |
  | present, doesn't hold it | any | nothing in the read session; the read is recorded in the reader's anchor, naming the read session |
  | absent | any | the read is logged in the read session, as today: a known exception (see Known Limitations) |

  Reads a gate or decider makes inside the session's own tick are unchanged.
  The read session's run journal stops counting reads by other agent
  sessions, which were never part of its own visits; anything that reads
  `context_read` events from session logs finds those reads in the readers'
  anchors, each naming the session it read. A read record in the reader's
  anchor is the reader's own activity and extends its anchor's last
  activity; nothing a reader does extends the read session's.

### Import

- **R22. Import settles a moved session's held-by links.** For each held-by
  link a moved session holds, import writes, in the moved copy only:

  | Condition at import time | Result |
  |---|---|
  | the named anchor is already in the importing workspace, open and not migrated | the link is kept |
  | otherwise, and the importing environment names an anchor | the link is rewritten to that anchor, with the old name kept in the link as a hint |
  | otherwise | the link is dropped |

  Dispatched-by links travel verbatim. The source gets nothing beyond koto's
  existing migrated mark.
- **R23. Import clears an anchor's observed marker.** An imported anchor
  carries no observed marker, and its last activity counts from the import.

### Settings

- **R24. Retention, idle period and observed backstop are configured.**
  Each is a koto setting with a default: retention 7 days, idle period 7
  days, observed backstop 30 days. Each is read, highest precedence first,
  from an environment variable for the one command, the project
  configuration, the user configuration, then the default, as koto's
  session-backend settings are. Values are a whole number with a unit of
  seconds, minutes, hours or days. koto refuses an observed backstop that
  isn't longer than the retention, and an unparseable value, naming the
  setting and exiting non-zero.

### niwa

- **R25. niwa passes declared variables through to workers.** The workspace
  configuration may list variable names that `niwa dispatch` copies from its
  own environment into a worker's settings environment, the block that
  reaches a worker the agent's daemon starts, merged with the variables niwa
  already sets there. A name that is unset is skipped. niwa refuses at load a
  declared name beginning `NIWA_` or `CLAUDE_`, or one niwa sets itself in
  that block.
- **R26. niwa marks sessions it will reap.** niwa sets one variable of its
  own, under its own name, for every agent session it will reap: in the
  settings environment and the process environment before launch for a
  session `niwa dispatch` launches, and in the session's environment file
  for a session `niwa instance from-hook` sets up. Its value lets niwa's
  on-reap command identify the session.
- **R27. niwa runs a declared on-reap command.** The workspace configuration
  may declare a command niwa runs once for each agent session it removes, or
  finds already removed, at these points: the reap of a session niwa holds a
  mapping for (`niwa reap`, and the reap `niwa dispatch`, `niwa create` and
  `niwa watch` run first, including the idle-grace reclaim);
  `niwa destroy`, when it removes the instance's session or finds it gone;
  and `niwa watch`'s discard of a staged review whose session is gone. It
  hands the command, through the environment only, niwa's variable value,
  the session id and the instance path. The idle-grace reclaim is niwa's
  removal of a session idle past niwa's own grace period, which counts as a
  reap. It never runs the command when it
  removes an instance alone: `niwa reset`, `niwa destroy --keep-session`,
  the reap of an instance with no session mapping, and the rollback of a
  failed dispatch.
- **R28. A failing on-reap command never fails a niwa step.** Each run is
  bounded by a timeout, 10 seconds unless the configuration sets another,
  and a niwa step's runs together by 30 seconds of wall time; a run that
  fails, times out
  or can't start, and any run the step's budget skips, is not retried and
  makes niwa print one warning line on stderr naming the session. The step's
  own outcome and exit code are unchanged.
- **R29. niwa names no koto.** niwa's non-test source names no koto
  variable, command or path. Each piece above is generic and inert when koto
  isn't installed; niwa never fails, refuses or degrades because koto is
  missing.

### shirabe

- **R30. Every shirabe dispatch path that uses no request records a link.**
  The dispatched session records a dispatched-by link to its dispatcher, in
  its own session: a chain child whose chain parent `skill-session.sh adopt`
  matched, naming that parent; an `/execute` coordinated `/work-on` child,
  naming the `/execute` run (these children are root sessions opened inside
  the run's own agent session, so its anchor holds them too, and the link
  keeps them when the run starts from a bare shell); and a `/coordinate`
  worker on the message return path, naming the dispatching coordinator. For
  the last, the brief `/coordinate` renders carries the dispatcher's session
  name, including for a worker moved to the message path later by a
  re-brief, a hand-off of execution or a leg found unwatchable, and a failed
  link call warns and doesn't stop the worker's opener. The `/coordinate`
  change lands with or after the coordinator-record teardown build
  (shirabe#978 to #986), which rewrites the same scripts.
- **R31. shirabe keeps no retention rule of its own.**
  `skill-session.sh reclaimable` and the reclaimable rule in shirabe's
  session convention are removed, and the convention names koto's hierarchy
  query as the answer to whether a session may be reclaimed.
- **R32. No worker launch inside a tick carries cloud credentials.** No
  shirabe path launches a worker or dispatch from inside a koto tick, whose
  commands inherit the `KOTO_SESSION_*` cloud settings; any path that comes
  to need one unsets them first.

### Non-functional

- **R33. Local and offline.** Every requirement except R22 and R23 works
  with the local session backend and no cloud configuration. Nothing here
  turns on the cloud backend or edits koto's user configuration file.
- **R34. Bounded cost.** On a GitHub-hosted `ubuntu-latest` runner, for a
  generated store of 2,000 sessions in 200 hierarchies of 10 sessions at most
  4 deep, the hierarchy query for one session answers within 1 second and a
  full prune preview within 10 seconds, enforced by a test in koto's suite.
- **R35. Fail toward keeping.** When koto can't read a session, a link or a
  close record, every session in the hierarchy it would belong to is kept,
  and the reason names the unreadable item; when the link graph can't be
  built at all, prune removes nothing and exits non-zero.

## Acceptance Criteria

### Setup and conventions

Unless a criterion says otherwise: every agent session involved has been
deleted and its anchor closed, so a member counts as alive only while its
koto session is open; durations are set through the environment to short
values (for example retention 20s, idle period 10s, observed backstop 60s),
and "after the retention" means after waiting past it; only the relation
under test can keep a session; "reads from A" means the read runs with A's
anchor named in the environment, whether that anchor is open or closed; and
every read from another agent session leaves the read session's log and
keys byte-identical, compared by hashing its files before and after. "The
prune keeps" or "a prune removes" means a prune apply, checked against the
preview it follows.

**Removal set S**, run in this order: S1, B's own terminal step without
`--no-cleanup`; S2, X (a koto parent of B, where the criterion has one)
reaching its terminal step without `--no-cleanup`, so the parent sweep
runs; S3, `koto session prune --apply --all`; S4, in a separate pass where
X reached its terminal step with `--no-cleanup`,
`koto workspace prune --root X --yes`. A criterion without an X runs S1 and
S3.

### Retention scenarios

- [ ] **Finished child.** A dispatches B, and B is also a koto child of X. B
  finishes while A is alive. After S, the reviewer reads B's log and keys
  from A; the prune, S2's sweep listing and S4's listing all name A as why B
  is kept.
- [ ] **Finished parent.** A dispatches B and C. B finishes, then A reaches
  its terminal step without `--no-cleanup`, while C is alive. After S the
  reviewer reads A's log and keys from C, and the prune names C as why A is
  kept and as why B is kept.
- [ ] **Grandchild.** With A alive, B (dispatched by A) finishes, then C
  (dispatched by B) reaches its terminal step without `--no-cleanup`. After
  S the reviewer reads C's keys from A, and the prune names A as why B and C
  are kept.
- [ ] **Co-parent.** C is dispatched by both A and B. A finishes, B stays
  alive, and D (dispatched by A only) finishes. After S the prune keeps A,
  C and D and names B for each.
- [ ] **Release instant.** With a retention of 20 seconds, A and B (A
  dispatched B) finish 12 seconds apart and nothing else is alive. A prune
  22 seconds after the first finish keeps both and names the later member
  and the time the retention passes; a prune 40 seconds after the first
  finish removes both. The same holds when the later member was cancelled
  instead of finishing.
- [ ] **Two finished sessions.** A dispatched B, both finished, nothing else
  alive: after the retention the prune removes both.
- [ ] **Whole hierarchy.** With two hierarchies that share no edge, one
  released past its retention and one with a live member, the prune removes
  every member of the first and nothing of the second.
- [ ] **Lone session.** From a bare shell (no anchor named), a session with
  no edge, ticked to a success terminal without `--no-cleanup`, is removed at
  that tick, as today.
- [ ] **Inbound only.** From a bare shell, a session that holds no link but
  that another session's dispatched-by link names is kept at its terminal
  step, with reason `hierarchy` in the tick's answer.
- [ ] **Terminal keeps regardless.** A session with a link is kept at its
  terminal step with reason `hierarchy` both with and without
  `--no-cleanup`, and also when every other member of its hierarchy was
  released past the retention before the tick.
- [ ] **Workspace prune.** A subtree under X holds one member whose
  hierarchy is released past the retention and one whose dispatcher is
  alive; `koto workspace prune --root X --yes` removes the first, keeps the
  second, and lists the second with its reason.
- [ ] **Reasons.** For a session kept by a live anchor, the hierarchy
  query's JSON names the anchor, its kind and whether it is observed; for a
  session inside its retention, it names the member and close kind the
  release instant came from and the time the retention passes; the
  human-readable output carries the same facts.
- [ ] **Changelog.** koto's changelog entry for the release states that the
  end-of-run removal no longer removes a session with a link or lineage and
  that prune reclaims it later.
- [ ] **Delivery unchanged.** A kept child's completion still reaches its
  parent and a leg-bound session's result still reaches its leg.
- [ ] **Re-run.** The reviewer re-runs B's skill topic while A is alive. The
  replace's answer names B's new predecessor name; the reviewer reads the
  earlier run's log and keys from A under it, its links are unchanged, and
  the new run holds the original name. A second re-run gives a third,
  distinct name. With nothing alive and the retention passed, a re-run
  replaces the predecessor as today.
- [ ] **Filters narrow candidates only.** B, finished, is dispatched by A,
  alive in another execution directory; `koto session prune --apply
  --execution-dir <B's directory>` and `--older-than` filters that select B
  but not A still keep B and name A.
- [ ] **Prune race.** A session the preview listed for removal gains a live
  member before `--apply`; apply skips it, lists it with its reason, and
  exits 0.
- [ ] **Explicit removal.** `koto session cleanup B` while A is alive removes
  B and names A; `koto cancel --cleanup C` on a live C with a live dispatcher
  removes C and names the dispatcher; `koto session cleanup` of a session
  nothing keeps names no member.
- [ ] **Fail toward keeping.** With one member's log made unreadable, a
  link file truncated, or a close record malformed, the prune keeps every
  member of that hierarchy and its reason names the item; with the store
  directory unreadable, prune removes nothing and exits non-zero.

### Links and writes

- [ ] **Leg-bound link.** `koto init B --koto-leg <request>:<leg>`, on a
  request whose `requested_by` is A, leaves a dispatched-by link naming A in
  B; a self-run `koto request attach` does the same; an attach naming
  another session writes nothing into it; a request whose `requested_by`
  fails the session-name rule leaves no link.
- [ ] **Link verb.** The link command and the `koto init` flag each record
  one dispatched-by link; running the command again leaves the session
  byte-identical and exits 0; an invalid dispatcher name exits non-zero and
  writes nothing.
- [ ] **Held-by.** `koto init` under an anchor records a held-by link to it;
  a second tick under the same anchor adds nothing; a tick of that session
  under a second anchor adds a second held-by link; an anchor holds no
  held-by link.
- [ ] **Single writer.** During every scenario in this section and the one
  above, every session other than the one the command opened, ticked, linked
  or imported, and other than the running agent session's own anchor, is
  byte-identical before and after the command, including the dispatcher and
  every other anchor; and `koto context add` into the link area is refused.
- [ ] **No lineage needed.** Finished child, Grandchild and Co-parent pass
  with the request store directory removed and no koto lineage between A
  and B.
- [ ] **Absent peer.** B's dispatched-by link names A, which has been
  removed by `koto session cleanup`; after the retention from B's finish
  the prune removes B.

### Anchors

- [ ] **Hooks load.** A test in the repository fails if
  `plugins/koto-skills/hooks/hooks.json` is missing or isn't in the
  documented matcher-group shape; and in a live Claude Code session with
  koto-skills enabled, checked by hand, the session-start hook runs and an
  anchor opens for that session.
- [ ] **One anchor per agent session.** Run with synthetic hook inputs
  (start, then resume, compact and clear for the same agent session), the
  hook leaves the session's environment file naming the same anchor after
  resume and compaction and opens no second one; after clear, a session B
  dispatched before it is still kept by the anchor the environment names.
  A resume whose anchor was idle-closed leaves it reading open; one whose
  anchor was removed reopens it under the same name; one whose anchor reads
  closed by an observed close opens a new anchor holding a dispatched-by
  link to the old one. A live session confirms the same by hand.
- [ ] **Close records last.** After an observed close, a prune apply inside
  the retention and another after the release leave the anchor reading
  closed with kind `observed` until both are removed together; removing the
  close session alone with `koto session cleanup` makes the anchor read
  open again until its backstop.
- [ ] **No koto.** With koto absent from the PATH, the session-start hook
  exits 0 with nothing on stderr, and the close command in its guarded form
  exits 0. With a koto stand-in on the PATH that exits 1 for every call,
  the hook still exits 0.
- [ ] **No anchor named.** `koto init` with no anchor in the environment
  succeeds with nothing on stderr and records no held-by link.
- [ ] **Close record.** Running the close command for an anchor records its
  close in a session other than the anchor, leaves the anchor byte-identical,
  and the hierarchy query then reports the anchor closed with kind
  `observed`; running it again, or with facts that match no anchor, exits 0
  and changes nothing; an anchor whose marker differs stays open.
- [ ] **Activity extends the idle period.** An unobserved anchor holds a
  session the agent session ticks every 5 seconds for 30 seconds, with a
  10-second idle period: the anchor stays open throughout and closes 10
  seconds after the last tick.
- [ ] **What isn't activity.** With a 10-second idle period, another agent
  session reads the anchor and its held session, and ticks a session the
  anchor doesn't hold, every 3 seconds; the anchor still closes 10 seconds
  after its own last activity.
- [ ] **Backstop.** An observed anchor whose close command never runs stays
  open past the idle period, is closed once the observed backstop has passed
  since its last activity with kind `backstop`, and the hierarchy it held is
  removed only after the retention from that close.
- [ ] **Anchor kept while resumable.** Overrides the setup. An agent session
  launched by `niwa dispatch` into an ephemeral instance niwa reaps, with
  koto-skills installed and the workspace declaring koto-skills' close
  command as its on-reap command, runs no koto session besides its anchor and
  dispatches B, which finishes. The reviewer ends the agent session's process
  and leaves it resumable, not deleted, for longer than both the idle period
  and the retention. After S, B is readable and the prune names the anchor.
- [ ] **Anchor closed by the reap.** Continuing that scenario, the reviewer
  deletes the agent session and runs `niwa reap`, then no koto command
  before the prune. A prune inside the retention keeps B and names the
  anchor's observed close; a prune after it removes B.
- [ ] **Idle worker.** The same two criteria pass for a worker that finished
  its own koto run and sits idle: the sessions it dispatched are kept and
  then removed the same way.
- [ ] **from-hook session.** The same two criteria pass for a session
  `niwa instance from-hook` set up in an ephemeral instance niwa reaps.
- [ ] **Late observed marker.** Run with synthetic hook inputs, when niwa's
  variable is written to the session's environment file 30 seconds after
  the hook starts, the anchor carries the observed marker within three
  minutes of the start; when it is never written, the anchor carries none,
  and a `claude -p` child's shell sees no niwa variable.
- [ ] **Reap by create.** The Anchor closed by the reap criterion passes
  with the reap done by a later `niwa create` instead of `niwa reap`.
- [ ] **Child of a reaped worker.** Overrides the setup. A worker launched by
  `niwa dispatch` dispatches B, and B finishes and its anchor closes. The
  worker launches a `claude -p` child through its shell and keeps it active.
  The worker is deleted and reaped. The close records only the worker's
  anchor, and the child's anchor carries no observed marker. While the child
  is active the prune keeps B, names the child's anchor and doesn't name the
  worker's. Once the child idles past the idle period its anchor closes; a
  prune inside the retention keeps B; a prune after it removes B and names
  the child's idle close.
- [ ] **Idle close.** Overrides the setup. On a host with no niwa, an agent
  session with koto-skills dispatches B, and B finishes. The agent session
  ticks a koto session of its own every half idle period until it reaches its
  terminal step, more than one idle period after the anchor opened, and B is
  kept throughout. Then the agent session is left idle and never deleted.
  Inside the idle period S runs, B is kept, and the prune names the anchor.
  Once the
  idle period has passed since the last tick the anchor is closed; a prune
  inside the retention keeps B; a prune after it removes B and names the idle
  close.
- [ ] **First dispatch.** Overrides the setup. An agent session with
  koto-skills dispatches B before making any koto call of its own. B's
  anchor records a dispatched-by link to the dispatcher's anchor from the
  inherited variable, with no name typed, and B's commands name B's own
  anchor. B finishes. Inside the dispatcher's idle period the prune keeps B
  and names the dispatcher's anchor, and the reviewer reads B's keys from the
  dispatching agent session.
- [ ] **First dispatch, detached.** With the workspace configuration
  declaring the anchor variable for niwa to pass through, B is a worker
  launched by `niwa dispatch --detach`; its anchor records its dispatcher
  with no name typed, and the prune keeps B and names the dispatcher's
  anchor.
- [ ] **Bare shell.** A runs from a bare shell, with no koto-skills and so no
  anchor, and dispatches B. B is kept while A's koto session is open and for
  the retention after A finishes, and is then removed.
- [ ] **Kept session.** Overrides the setup. A worker launched by `niwa
  dispatch` dispatches B, and B finishes. `niwa destroy --keep-session` on
  the worker's instance runs no on-reap command and leaves the anchor open; a
  prune after the retention from the destroy keeps B and names that anchor.

### Reading

- [ ] **Read from another agent session.** Run from agent session A against
  session B that A's anchor doesn't hold, `context get` and `context exists`
  each leave B's files hashing the same and add one read record naming B
  and the key to A's anchor; `context list`, `status`, `session list`, the
  hierarchy query and a prune preview leave B, A's anchor and every other
  session hashing the same.
- [ ] **Own read.** A `context get` and a `context exists` from the agent
  session whose anchor holds B each add a `context_read` event to B, as
  today, and a gate read inside B's own tick still logs in B.
- [ ] **No reader anchor.** A `context get` from a bare shell adds a
  `context_read` event to the read session, as today.
- [ ] **Read consumers.** After A (whose anchor doesn't hold B) reads B
  during one of B's visits, B's run journal records no context read for that
  read, and A's anchor's log holds a `context_read` event naming B and the
  key.

### Import

- [ ] **Import.** With the owner's per-command cloud configuration in the
  environment only (`KOTO_SESSION_*`, never the user configuration file), on
  one host, and with the importing agent session kept alive, the reviewer
  moves A to another workspace by `koto session import` while B is finished.
  A's new copy then reaches its terminal step without `--no-cleanup`, and S
  runs. The prune names the importing agent session's anchor as why both A's
  new copy and B are kept. The migrated source keeps nothing and refuses
  reads. The reviewer reads B's log and keys from A's new copy.
- [ ] **Import keeps a held-by.** With the same configuration, the reviewer
  imports an anchor and then a session it holds into the same workspace; the
  moved session's held-by still names that anchor. When the anchor imported
  first is closed, or is migrated, the moved session's held-by is rewritten
  to the importing environment's anchor and keeps the old name as a hint.
  In each case the source changes only by koto's migrated mark.
- [ ] **Import with no anchor.** Overrides the setup: B's original anchor
  stays open. With the same configuration, the reviewer imports a finished B
  from a bare shell. B's held-by link is gone, its dispatched-by links are
  unchanged, and the prune removes it once the retention from its finish
  passes.
- [ ] **Imported anchor.** Overrides the setup. A source anchor opened in an
  instance launched by `niwa dispatch` keeps B. B finishes, and the anchor and
  B see no activity for longer than one idle period. With the same
  configuration on one host, the reviewer imports the anchor and then B into
  another workspace from a bare shell, and leaves the anchor idle. B's
  held-by still names the anchor, which reads there. The imported anchor has
  no observed marker. Inside the idle period counted from the import the
  prune keeps B and names the imported anchor; after it the anchor is
  closed; a prune inside the retention keeps B; a prune after it removes B,
  naming the idle close.
- [ ] **Migrated link.** A link naming a migrated session resolves to the
  moved copy in the same store, and a migrated anchor keeps nothing; with
  no moved copy in the store, the migrated peer counts as absent and
  contributes no stop instant.

### Settings

- [ ] **Settings.** With no configuration the hierarchy query reports a
  retention of 7 days, an idle period of 7 days and an observed backstop of
  30 days. Each takes a user setting, a project setting over the user
  setting, and an environment value over both. Values in seconds, minutes,
  hours and days are each accepted. An observed backstop of 7 days with a
  retention of 7 days, including when one comes from the environment and the
  other from the user configuration, and a value of `7x`, are each refused
  with a message naming the setting and a non-zero exit.
- [ ] **Configuration untouched.** Across the whole suite, koto's user
  configuration file is byte-identical before and after, and no criterion
  except the import ones runs with a cloud backend set.

### niwa

- [ ] **Pass-through.** With `FOO` declared and set, a worker launched by
  `niwa dispatch --detach` sees `FOO` in its settings environment next to the
  variables niwa already sets; with `FOO` unset it's absent and the dispatch
  succeeds; declaring `NIWA_X`, `CLAUDE_X` or a name niwa sets in that block
  is refused when the configuration loads.
- [ ] **Reap variable.** A session `niwa dispatch` launches has niwa's
  variable in both its settings environment and its launch process
  environment before its first command; a session
  `niwa instance from-hook` sets up has it in its environment file.
- [ ] **On-reap runs.** With a declared command that records its
  environment, `niwa reap`, the reap run by `niwa dispatch`, by `niwa
  create` and by `niwa watch`, the idle-grace reclaim, `niwa destroy` and
  `niwa watch`'s discard each run it once per removed session with niwa's
  variable, the session id and the instance path, and nothing else from
  niwa.
- [ ] **On-reap doesn't run.** `niwa reset`, `niwa destroy --keep-session`,
  the reap of an instance with no session mapping and a failed dispatch's
  rollback each run no on-reap command.
- [ ] **Close command failure.** A declared command that exits 1, and one
  that sleeps past the timeout, each leave the niwa step's exit code and
  outcome unchanged, end within the timeout plus one second, and print one
  warning line naming the session; checked at dispatch, `niwa create`,
  `niwa reap`, `niwa destroy` and `niwa watch`'s discard. A configured
  timeout of 2 seconds is honoured. With five sessions removed in one reap
  and a command that sleeps 10 seconds, the reap's on-reap runs end within
  31 seconds of wall time, and each session whose run timed out or was
  skipped gets one warning line.
- [ ] **No koto.** On a host with niwa and no koto installed, the reviewer
  runs `niwa dispatch` from a workspace whose configuration installs
  koto-skills, declares the anchor variable for pass-through and declares
  koto-skills' close command as the on-reap command. The dispatch succeeds,
  the worker runs, its session start shows no failed koto hook, and niwa
  prints no error or warning. Reaping that instance runs the close command,
  which finds no koto and exits 0, and niwa prints no error or warning. With
  the declared command missing from the PATH, the reap still succeeds.
- [ ] **Destroy runs the close.** `niwa destroy` without `--keep-session`, on
  an instance whose session it removes, runs the declared on-reap command
  once for that session, and the anchor then reads closed with kind
  `observed`.
- [ ] **niwa names no koto.** A case-insensitive search of niwa's non-test
  Go source and its embedded strings for `koto` finds nothing, and the
  pass-through and on-reap criteria pass with names and commands that have
  nothing to do with koto.

### shirabe

- [ ] **shirabe links.** A chain child whose chain parent `adopt` matched,
  an `/execute` coordinated `/work-on` child and a `/coordinate` worker on
  the message return path each hold a dispatched-by link naming their
  dispatcher, written in their own session; the brief `/coordinate` renders
  for a message-path worker contains the dispatcher's session name, also
  when a leg-bound worker is moved to the message path by a re-brief, a
  hand-off of execution or a leg found unwatchable.
- [ ] **Failed link warns.** With the link command made to fail, a
  `/coordinate` message-path worker's opener prints a warning naming the
  dispatcher and carries on to open the worker's session.
- [ ] **Sequencing.** The `/coordinate` change's pull request merges no
  earlier than the coordinator-record teardown build (shirabe#978 to #986);
  this is checked in the plan's dependencies, not by a test.
- [ ] **No shirabe retention rule.** `skill-session.sh reclaimable` exits
  as an unknown subcommand, no shirabe script computes whether a session
  may be reclaimed, and the session convention names koto's hierarchy query.
- [ ] **No launch in a tick.** No shirabe template's gate or default-action
  command, or a script it runs, launches `niwa dispatch` or `claude`, and a
  test in shirabe's suite fails if one is added without unsetting
  `KOTO_SESSION_*`.

### Cross-cutting

- [ ] **No niwa.** On a host with no niwa installed, Finished child,
  Finished parent, Grandchild, Idle close and First dispatch pass with koto
  and koto-skills alone.
- [ ] **No cloud.** Every criterion except Import, Import keeps a held-by,
  Import with no anchor and Imported anchor passes with no cloud backend
  configured.
- [ ] **Cost.** The R34 test passes on the CI runner.

## Out of Scope

- **Recording delegated decisions, reversing them, and who may do so.** That
  later work builds on this retention; nothing here records a decision.
- **Authority.** Membership in a hierarchy keeps a session readable and
  grants nothing.
- **Turning on the cloud backend generally, editing koto's user
  configuration file, and cross-host reads or tests.** Import is exercised on
  one host with per-command configuration in the environment. Reading a
  relative's keys on another host waits on the owner's instruction.
- **niwa reading or writing workflow state, naming koto, or depending on it,
  and koto depending on niwa.**
- **Session-start equivalents for harnesses other than Claude Code**, and the
  anchor variable reaching a Codex worker, which has no settings document.
  Sessions there have no anchor and use the koto-level reading (R13).
- **Retention of host-level stores**: the request store, the decider ledger,
  the terminal index and finished records.
- **Automatic pruning.** Removal stays the existing commands; this feature
  changes what they may remove, not when they run.
- **Removing koto's existing cross-session writes** (see Known
  Limitations). This feature builds nothing on them.

## Known Limitations

- **Known cross-session writers.** Three existing koto behaviours write
  into a session from another session's process and are left as they are:
  a child's terminal tick appending its completion to its parent's log;
  `koto context add` naming any session; and a read with no anchor on the
  reader's side logging into the read session (R21). Under the cloud backend
  a migrated source refuses all three. Nothing in this feature treats a
  child's completion record as a link or a sign of life. koto#409 tracks
  stopping the first, by having a parent read its children's results at its
  own tick.
- A coordinator left idle past the idle period with nothing observing its
  deletion loses its hold on the sessions it dispatched, even though a person
  could still resume it. The owner accepted this cost; the 7-day default
  keeps a weekend away inside it.
- A long-lived anchor joins every hierarchy it touched into one, so a
  person's interactive session that runs for days keeps everything it touched
  until it closes, and a busy held session keeps its anchor's idle clock
  fresh. The hierarchy query makes this visible.
- A link naming a session on another host counts as a member that isn't
  alive, since nothing here reads another host.
- Sessions with an edge are no longer removed at their terminal step, so the
  store grows until a prune runs. Callers that already pass `--no-cleanup`,
  which is every shirabe skill, see no difference.
- A lost observed close can release a hierarchy earlier than a correct close
  would have, when the agent session stayed resumable for longer than the
  backstop after its last activity. The backstop is long for that reason.

## Decisions and Trade-offs

- **The rule binds agent sessions through anchors** (the owner's decision).
  Binding koto sessions only was simpler but misses an idle, resumable agent
  session and a person's own session.
- **Links are written only in the session they belong to**, at the
  dispatched end, and the downward view is the inverted map over the local
  store. Writing both ends was rejected: it writes into another session,
  which a cloud-synced or migrated session can't take, and one end is enough
  for a reader that inverts the map.
- **Retention reaches the whole hierarchy, through finished members.** One
  hop drops an open coordinator's grandchildren; directed ancestor or
  descendant closure drops a co-parent's branch a reader still reaches. Two
  finished sessions never keep each other, so a bad link can only
  over-retain while something is alive.
- **Lineage and `chain/parent` count as edges.** They already join sessions
  that prune keeps today, so dropping them would regress; nothing depends on
  them, so import losing lineage costs nothing the links don't carry.
- **Unobserved anchors close after an idle period; observed ones on the
  observed deletion, with a longer backstop** (the owner's pick for the idle
  close). A lease was rejected: an idle session can't renew it. The backstop
  has to exceed the retention, or a `--keep-session` destroy releases B
  before a correct close would.
- **Defaults: retention 7 days, idle period 7 days, backstop 30 days.** An
  idle period of 24 hours would close a person's own session over a
  weekend, against the owner's ruling that idle and resumable counts as
  alive. All three stay configurable.
- **An observed close is recorded in the close command's own session.**
  Writing it into the anchor would need the owner's exception to the
  one-writer rule; the idle close and the backstop cover every case without
  it.
- **The terminal tick keeps every session with an edge** rather than relying
  on every caller's `--no-cleanup`, including koto children while their
  parent runs. Otherwise retention holds only for callers that remember the
  flag. Sessions with no edge keep today's behaviour.
- **A re-run moves a held predecessor aside instead of refusing or
  destroying it.** Refusing would break every re-run of a skill topic while a
  coordinator lives; destroying loses the record the rule protects.
- **Absent peers count as not alive.** Counting them alive would keep a
  hierarchy forever after any explicit removal or a cross-host link.
  Unreadable present sessions still count as alive, and an unreadable link
  keeps its hierarchy (R35).
- **A read from another agent session is recorded in the reader's anchor.**
  Recording nothing would cost the run journal and the session archive their
  read events; recording in the read session is the cross-session write the
  rule forbids.
- **koto doesn't warn when a session opens with no anchor.** The bare-shell
  fallback is expected, and a warning on every bare-shell `koto init` would
  be noise.
- **niwa's pass-through refuses only names niwa and Claude Code own.**
  Refusing koto's cloud variables would make niwa name koto; the workspace
  configuration declares exact names, and koto's guide says to declare only
  the anchor variable.
- **A failed on-reap command is skipped, not retried.** The observed
  backstop already closes an anchor whose close never landed, so a retry
  queue in niwa would duplicate it.
- **shirabe's reclaimable rule is retired, not re-pointed.** Nothing calls
  it, and a second answer to "may this go" is the drift the change exists to
  remove.
