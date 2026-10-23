---
schema: prd/v1
status: In Progress
problem: |
  koto keeps a finished session only while something live in its hierarchy
  holds it, plus a retention period, then removes it at the next cleanup,
  prune, terminal tick or subtree removal; under the cloud backend that
  removal deletes it from the bucket for every host sharing it. A finished
  session that is the record of finished work, reached later from a merged
  pull request, has no way to be kept, and nobody can say "keep this one" in
  any form koto reads.
goals: |
  Anyone can pin a koto session with a reason, and from then on no removal
  koto makes, on any host sharing the store, takes it; every removal that
  passes it over names the pin and its reason. Only an explicit unpin
  releases it, and an agent session releases one only when it was handed
  koto's release authority. Sessions nobody pinned keep exactly today's
  behaviour, and all of it works with koto and no niwa.
source_issue: 441
absorbed:
  - docs/briefs/BRIEF-session-pin.md
---

# PRD: Pin a finished session so no removal rule takes it

## Status

In Progress

Absorbed [BRIEF-session-pin](docs/briefs/BRIEF-session-pin.md); carried in Absorbed Brief.

## Absorbed Brief

The brief named in Status is folded into this section and its file was
removed when this PRD absorbed it; this section is the brief's framing, and
there is no separate file to look for.

The feature was framed before these requirements existed, from an issue
asking for a way to keep a finished session. Session retention keeps a
finished session only while a member of its hierarchy is alive, and then for
the retention period. That covers the sessions a live coordinator still
reads; it doesn't cover a session that is the record of finished work and
matters after the whole hierarchy is gone, such as one a reviewer reaches
from a merged pull request weeks later, and under the cloud backend its
removal reaches every host sharing the bucket. The framed outcome: whoever
knows a finished session matters marks it to be kept with a reason and stops
worrying about it; no removal koto applies takes it on any host sharing the
store; each removal that passes it over says why; the mark comes off only
when someone allowed to release it does so, never by idle time, staleness or
another rule; a mark koto can't read keeps its session; sessions nobody
marked are untouched; and it works with koto and no niwa. The framing's four
journeys (keeping the record of finished work, an operator's prune on a
second host, releasing a record nobody needs, and a mark that can't be read)
are the User Stories below. Its boundary put in: the mark, its release, every
removal by rule or subtree plus `koto session cleanup`, reasons where keepers
are named today, the cloud backend including anchors held on one host's local
store, failing toward keeping, and release authority handed to the session
the way its anchor name is. It put out: release by any rule, changes for
unmarked sessions, an automatic pruner, host-level stores, defining the
authority itself, enforcement stronger than today's, any dependency between
koto and niwa, and removals inside a run koto is still driving. It left three
questions to these requirements: who may release a mark and how that
authority reaches koto, whether a mark covers one session or a hierarchy, and
whether setting one needs authority; Decisions and Trade-offs settles each.

## Problem Statement

Some koto sessions matter after their work is over. A session that drove a
piece of work to a merged pull request holds what was done, in what order
and with what results, and a reviewer, an auditor or a later agent reaches
it from that pull request weeks afterwards.

koto has no way to keep one. Session retention, which koto now applies at
every removal by rule or subtree, keeps a finished session only while a
member of its hierarchy is alive: an open anchor (the koto session that
stands for a running agent session) holding it through a held-by link, or a
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
which holds only the keys it happened to read, one host's push can replace
the bucket's index of a session's keys, and `koto session cleanup` on a
second host deletes a bucket session that host never pulled.

### Terms

These terms are used from here on with exactly these meanings. Terms from
session retention keep their meaning: anchor, open and closed anchors,
observed close, held-by and dispatched-by links, hierarchy, alive, stop
instant, release instant, retention, migrated source.

- **Pin**: a koto-owned record in a session's own reserved context area that
  says the session is to be kept. It holds a reason, the setter and the time
  it was set. A session holds at most one.
- **Pinned session**: a session in which koto finds a pin, readable or not,
  in the local store or, under the cloud backend, in the bucket.
- **Unreadable pin**: a pin koto finds but can't use, for exactly one of
  these reasons, each reported by its name: `unparseable` (not the pin's
  format), `too_large` (over 4 KiB), `not_a_file` (a symlink, directory or
  special file), `missing_content` (the store's index lists the pin and its
  content is absent).
- **Anchor variable**: `KOTO_ANCHOR`, the environment variable that names the
  caller's anchor, as session retention defines it.
- **By hand**: a call whose environment has no anchor variable, treated as
  the person at the shell.
- **Setter** and **releaser**: the anchor the anchor variable named when the
  pin was set or released, or none for a call by hand.
- **Start**: one session-start event of an agent session as koto's
  Claude Code plugin sees it: the first start, or a start again on resume,
  compaction or `/clear`. A **successor anchor** is the new anchor a start
  opens when the agent session's previous anchor reads closed by an observed
  close or migrated, as session retention defines it.
- **Release authority**: the koto authority named `release-pin`, which permits
  an agent session to unpin.
- **Handed authority**: the koto authority names given to an agent session
  through the variable `KOTO_AUTHORITY` at a start.
- **Removal**: any koto command that deletes a session's log and keys: the
  terminal tick, the parent sweep, the same-name replace,
  `koto workspace prune`, `koto session prune`, `koto session cleanup` and
  `koto cancel --cleanup`.
- **Bucket check**: a read of the bucket made by the command about to delete
  a session, after any confirmation and before its first delete, that lists
  the session's objects in the bucket and answers whether a pin is among
  them.
- **Output surface**: any text or JSON a command named in this PRD prints to
  stdout or stderr.

## Goals

- A person or agent who finishes work worth keeping pins its session with
  one command and a reason, and no later removal needs anything more from
  them.
- No removal koto makes takes a pinned session, on this host or any other
  host sharing the cloud bucket, and each one that passes it over names the
  pin and its reason, apart from sessions kept because something is alive.
- A pin comes off only through an explicit unpin. Idle time, staleness and
  the retention period never take it off, and the next removal after an
  unpin treats the session like any other.
- An agent session releases a pin only when it was handed koto's release
  authority, and a refused release says which authority it lacked.
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
- **Handing the authority on.** As a coordinator that dispatches a worker to
  tidy finished records, I want to start the worker with the release
  authority so its unpins succeed and are recorded as made under that
  authority, so that the release is traceable to what I handed it.
- **A worker without the authority.** As a coordinator that dispatched a
  worker without the release authority, I want the worker's attempt to unpin
  a record refused, naming the authority it lacks, so that only sessions I
  trusted with it release records.
- **A damaged pin.** As an operator running prune after a pin was truncated
  by a failed write, I want the session kept and reported as kept because its
  pin can't be read, and why, so that I repair or release it rather than lose
  it.
- **Finding every pin.** As an operator, I want to list every pinned session
  my host can see with its reason, setter and when it was set, so that I can
  find a runaway skill that pins everything.

## Requirements

### Setting and releasing a pin

- **R1. Pin a session.** `koto session pin <name> --reason <text>` records a
  pin in the named session holding the reason, the setter and the time (RFC
  3339, UTC). The reason is required, at most 500 Unicode scalar values, not
  empty after trimming whitespace, and contains no character in Unicode
  category Cc (tab and newline included). On success it exits 0 and prints
  JSON with `name`, `pin` (`reason`, `set_by` as the anchor name or `null`,
  `set_at`) and `store` (`local`, or `cloud` with the bucket name). It
  refuses with exit 2, writing nothing, when: the reason breaks a rule above;
  no session has the name (in the local store, or under the cloud backend in
  the bucket either); the session is an anchor or an anchor's close
  record (kinds koto already records in the session's header); or the
  session is a migrated source.
- **R2. A running session may be pinned.** The pin takes effect whenever a
  removal would otherwise take the session, including at the session's own
  terminal tick. While the session runs it's alive by the existing rule, and
  that's what keeps the rest of its hierarchy (R9).
- **R3. Pinning is idempotent; an unreadable pin blocks pinning.** Pinning a
  session that holds a readable pin changes nothing, exits 0 and prints the
  existing pin, whatever reason was given. Pinning a session that holds an
  unreadable pin refuses with exit 2, naming the unreadable pin and that an
  unpin clears it.
- **R4. Setting a pin needs no authority.** Any caller may pin, by hand or
  under an anchor, with or without handed authority.
- **R5. Unpin a session.** `koto session unpin <name> [--reason <text>]`
  refuses with exit 2, writing nothing, when no session has the name (as R1
  checks it) or the reason breaks R1's rules for a reason. Otherwise it reads
  the session's pin. With no pin, it changes nothing and exits 0, whoever the
  caller is. With a pin, readable or not, it removes the pin when the caller
  may release it (R7) and exits 0, printing the release record. Under the
  cloud backend, unpin acts on the bucket's pin for a session this host holds
  no local copy of. When pin or unpin can't read the bucket to find the
  session or its pin, it exits 3 and changes nothing.
- **R6. A release is recorded.** Each unpin writes a release record into the
  unpinned session with: `released_by` (the releaser's anchor, or `null`),
  `authority` (`release-pin`, or `null` for a release by hand), `reason` (or
  `null`), `released_at`, and the released pin's `reason`, `set_by` and
  `set_at` (each `null` when the pin was unreadable). A later release replaces
  the earlier record. The release record is not a pin, a link or a keeper,
  never makes its session pinned or unreadable, moves with the session at a
  replace and an import, and is removed with the session. The record is
  written before the pin is removed, so an unpin interrupted between the two
  leaves the session pinned.
- **R7. Who may release.** A caller by hand may unpin. A caller whose anchor
  variable names an anchor may unpin only when that anchor, open or closed,
  records `release-pin` among its handed authorities (R8). Otherwise the unpin exits
  2, changes nothing and writes no release record, and its message names:
  the authority it lacks, `release-pin`; the anchor it read authority from;
  and that the authority is handed to an agent session at its start. When
  the named anchor doesn't exist or can't be read, the unpin is refused the
  same way and says so. Setting a pin never grants its release: the setter
  gets no standing of its own.
- **R8. How authority reaches koto.** koto reads an agent session's handed
  authority only from what was handed to that agent session, never from
  niwa's records, niwa's commands or any other tool's state. The decision
  source is the caller's anchor: at each start, koto's Claude Code plugin
  reads `KOTO_AUTHORITY` (a comma-separated list; each name trimmed, names
  matched exactly in lowercase) from the agent session's environment,
  records the list in the agent session's anchor, replacing what an earlier
  start recorded, and removes `KOTO_AUTHORITY` from the shells the session
  starts, so a worker or `claude -p` child launched from them inherits none.
  A successor anchor records only what its own start was handed. A value of
  `KOTO_AUTHORITY` in the caller's own environment at unpin time is never
  read. Unknown names are recorded and ignored for every decision, and the
  hierarchy query for the anchor lists them as `unknown_authorities`. koto's
  authority names are a closed set per koto version; this version has one,
  `release-pin`.

### Keeping a pinned session

- **R9. A pin keeps its session and nothing else.** A pin keeps the pinned
  session with no release instant, for as long as the pin is present. It
  keeps only that session: every other member of its hierarchy is kept or
  released by the existing rule, with the pinned session counted as alive
  while it runs and, once finished, as a finished member with its own stop
  instant. A pin adds no link to its setter or anyone else, so the links,
  lineage and hierarchy membership of every session are the same before and
  after a pin is set or removed.
- **R10. Every removal keeps a pinned session.** These keep a pinned session:
  the terminal tick, with or without `--no-cleanup` and for a failure
  terminal; the parent sweep; the same-name replace; `koto workspace prune`,
  with or without `--force` or `--yes`; and `koto session prune` with every
  flag it accepts (`--all`, `--apply`, `--plan`, `--execution-dir`,
  `--older-than`, `--migrated-from`). A same-name replace that meets a pinned
  finished predecessor moves it aside under a new name by the replace's
  existing naming, keeping its log, keys, pin and release record; the new run
  takes the original name with no pin. Under the cloud backend the moved
  predecessor's objects, its pin included, are in the bucket under the new
  name, and the original name's objects are only the new run's.
- **R11. Explicit removal refuses a pinned session.** `koto session cleanup`
  of a pinned session exits 2 and removes nothing, and its message names the
  pin's reason, setter and time (or that the pin is unreadable, and why) and
  that an unpin comes first. `koto cancel --cleanup` of a pinned session,
  running or finished, refuses the same way before writing anything, so the
  session is neither cancelled nor removed; `koto cancel` without `--cleanup`
  still cancels it and leaves it pinned. For a name with no session in the
  local store or the bucket, both commands behave exactly as today.
- **R12. Read the pin again just before deleting.** Each removal reads the
  session's pin after any confirmation prompt and immediately before its
  first delete of that session, and keeps a session pinned since the command
  started. This narrows the window to the time between that read and the
  delete; it isn't atomic. Pin and unpin take the same per-session lock that
  removals take before deleting, so a pin or unpin and a removal of the same
  session on one host never interleave.
- **R13. Unpinned sessions are unchanged.** A session that holds no pin is
  kept and removed exactly as today by every removal, and every output surface
  prints for it exactly what it prints today, also when another member of its
  hierarchy is pinned.
- **R14. An unpinned session is eligible again at once.** An unpin doesn't
  restart any clock. After it, the session is judged by the existing rule
  alone, using its hierarchy's existing release instant, so a session whose
  hierarchy is past its retention is taken by the next removal that applies.

### Saying why

- **R15. Every removal that keeps a pinned session names the pin.** In JSON,
  each kept-session entry for a pinned session carries `kept_by_pin: true` and
  a `pin` object (`reason`, `set_by`, `set_at`, `source`: `local` or `bucket`),
  or, for an unreadable pin, `kept_by_pin: true` and `pin_unreadable` with its
  reason name (Terms). The text form prints `pinned: <reason> (set by
  <anchor or "hand"> at <time>)` or `pin unreadable: <reason name>`. This holds
  for the terminal tick's `retention`, the parent sweep's kept-children
  listing, the replace's answer, `koto workspace prune`'s kept and skipped
  lists, `koto session prune`'s preview, plan and apply, and the refusals of
  R11. A pinned session also failure-terminal, or also held by a live member,
  carries the pin as well as its other reasons. No output surface describes a
  session with a readable pin as alive or as unreadable on the pin's account.
- **R16. The hierarchy query shows pins.** `koto session hierarchy <name>`, in
  text and JSON, shows for a pinned session its pin (as R15) and its latest
  release record, and states that the pin keeps that session only. For every
  other session in a hierarchy that has pinned members, it lists the name of
  each pinned member. For an anchor it lists the handed authorities it
  records and any `unknown_authorities`. It writes nothing.
- **R17. Pins can be listed.** `koto session list --pinned` lists every
  pinned session in the local store, sorted by name, each with its pin (as
  R15) and whether it's running or finished, and prints an empty list when
  there are none. Under the cloud backend it also lists, marked
  `source: bucket`, pinned sessions in the bucket this host holds no local
  copy of; when the bucket can't be read it prints the local results with
  `bucket_unchecked` (R23) and exits 3. Its JSON form carries the same
  fields.

### Failing toward keeping

- **R18. An unreadable pin keeps its session.** A session with an unreadable
  pin is kept by every removal, and the removal reports it as R15 says, with
  the reason name. An unreadable pin keeps only its session, like a readable
  one.
- **R19. The store's index never decides alone.** A pin file present in the
  session without the store's index listing it is a readable pin; a pin the
  index lists without its content is `missing_content`. Both keep.

### Cloud backend

- **R20. Pin and unpin reach the bucket or fail.** Under the cloud backend,
  `koto session pin` exits 0 only once the bucket holds the pin; if it can't
  confirm that, it exits 3, says the pin is held in the local store only, and
  the next `koto session pin`, `koto session unpin`, `koto next` or
  `koto context add` on that session on this host retries the push, printing
  the same message if it fails again.
  `koto session unpin` exits 0 only once the bucket holds the release record
  and no longer holds the pin; otherwise it exits 3 and the bucket's pin
  stays in force.
- **R21. Every removal checks the bucket before deleting.** Under the cloud
  backend, before any removal deletes either copy of a session, it makes a
  bucket check for that session. A pin found in the local copy or in the
  bucket keeps the session on this host and in the bucket, and the removal
  reports it (R15) with `source` naming where it was found. This holds when
  this host's local copy holds no pin, when the host never pulled the session,
  and for `koto session cleanup` of a name the host holds no local copy of.
- **R22. The bucket check finds a pin by its object.** The bucket check lists
  the session's objects directly and finds a pin from its own object, never
  only from the bucket's index of the session's keys, so a pin is found after
  another host pushed an index without it. No context write, index push or
  state push removes or overwrites a pin object; only unpin removes one.
- **R23. A failed bucket check keeps.** When the bucket check can't be made
  (an error or timeout talking to the bucket) or returns a listing koto can't
  parse, the removal deletes neither copy and reports the session as kept
  because the bucket couldn't be checked for a pin: in JSON, the session's
  entry carries `bucket_unchecked` with `error` (`unreachable` or
  `unparseable`), and the text form prints
  `kept: bucket not checked for a pin (<error>)`. A listing that succeeds
  and holds no pin object is not a failure. The outcome per removal: the
  terminal tick keeps the session and says so in its `retention`; the parent
  sweep skips the child and lists it; the same-name replace moves the
  predecessor aside as for a pinned one; `koto workspace prune` and
  `koto session prune` list the session as kept and carry on with the rest,
  exiting as they do today when they skip a session; `koto session cleanup`
  and `koto cancel --cleanup` exit 3 and change nothing.
- **R24. Keeping doesn't depend on another host's store.** A pin keeps its
  session from the pin alone, so a removal on a second host keeps and reports
  a pinned session even when the anchors and other members that hold it live
  only in the first host's local store, as happens when the cloud backend is
  chosen by environment and koto's start hook opens anchors in the local
  store.

### Import and compatibility

- **R25. A pin travels with import.** `koto session import` carries a
  session's pin and release record to the moved copy unchanged, an
  unreadable pin included. The migrated source keeps nothing, as today.
- **R26. Older builds keep a pinned session.** koto v0.23.0 and later
  releases without this feature, which read session-retention links, read a
  pin as an unreadable record and never as an ordinary link, so their
  `koto session prune`, `koto workspace prune`, terminal tick and same-name
  replace keep a pinned session. koto's changelog for the first release with
  this feature, and its session-retention guide, name that release as the
  first that honours pins in full, and say that older releases' `koto session
  cleanup`, `koto cancel --cleanup` and `koto session import` don't honour
  pins and that every host sharing a bucket needs the new release. Each koto
  guide that tells a reader to run `koto session cleanup` as a recovery step
  says to unpin a pinned session first.

### Non-functional

- **R27. No niwa.** Every requirement holds on a host with koto and its
  plugin and no niwa installed. Any launcher, or a person starting a session
  by hand, hands authority by setting `KOTO_AUTHORITY` in the environment a
  Claude Code session with koto's plugin starts in. An agent session without
  the plugin has no anchor variable, so its calls are by hand (R7). koto's
  guide says how a workspace that dispatches with niwa passes
  `KOTO_AUTHORITY` through to workers with niwa's generic variable
  pass-through, which needs no change to niwa.
- **R28. Bounded cost.** On the local backend, on the session-retention cost fixture (2,000
  sessions in 200 hierarchies of at most 10 sessions) with 200 sessions
  pinned, the hierarchy query for one session answers within 1 second and a
  full prune preview within 10 seconds of wall-clock time on a GitHub-hosted
  `ubuntu-latest` runner, measured by the same test harness and method as the
  existing retention cost test. Under the cloud backend, the bucket check
  makes one listing request per session the removal would otherwise delete,
  plus one read per pin it finds, and none for a session the removal keeps for
  another reason.
- **R29. Writes stay with their session.** Pin and unpin write only into the
  named session. Recording handed authority writes only into the starting
  agent session's own anchor. Nothing in this feature writes into any other
  session. `koto context add` and `koto context remove` refuse the pin, the
  release record and the recorded authority with exit 2, as they refuse koto's
  other reserved keys.

## Acceptance Criteria

### Setup and conventions

Unless a criterion says otherwise: retention 20s, idle period 10s and stale
threshold 60s are set through the environment; every agent session involved
has been deleted and its anchor closed; "after the retention" means 25
seconds after the hierarchy's release instant; calls are by hand. The fixture
creates P and U as running koto sessions in one store, with X a koto parent of
P, and pins P by hand with reason `record of the merged change`. U has no pin
and no live member. Each criterion's checks are on JSON output unless it says
text.

**Removal set S**, run in this order: S1, P and U each ticked to their success
terminal without `--no-cleanup`; S2, X ticked to its terminal without
`--no-cleanup`, so the parent sweep runs; then, after the retention, S3,
`koto session prune --apply --all`; S4, in a fresh fixture where X finished
with `--no-cleanup`, `koto workspace prune --root X --yes`; S5,
`koto session cleanup P`; S6, a same-name replace of P's name
(`koto init --replace-terminal`). U is removed at S1 (it has no edge), as on
a build without this feature, and S2 to S6 don't apply to it.

### Keeping and reporting

- [ ] **Pinned survives every removal.** After each of S1 to S5, P's log and
  keys hash the same as before the step; after S6, P's log, keys, pin and
  release record hash the same under the new name the replace reports.
- [ ] **Each removal names the pin.** S1's `retention`, S2's kept-children
  entry, S3's preview and apply entries, S4's kept entry, S5's refusal and
  S6's answer each carry `kept_by_pin: true` and `pin` with reason
  `record of the merged change`, `set_by: null`, P's `set_at` and
  `source: local`; none carries an alive keeper or an unreadable item for P.
  The text forms print `pinned: record of the merged change (set by hand at
  <time>)`.
- [ ] **Flags don't bypass.** With P finished and past the retention,
  `koto workspace prune --root X --yes --force`, `koto session prune --apply`
  with each of `--all`, `--execution-dir <P's directory>`, `--older-than 1s`,
  and `--apply --plan` on a plan listing P, each keep P and name the pin.
- [ ] **Cleanup and cancel refuse.** `koto session cleanup P` exits 2, P
  hashes the same, and the message names the pin's reason, setter and time
  and says to unpin first. On a running pinned session R, and on a finished
  pinned session F, `koto cancel R --cleanup` and `koto cancel F --cleanup`
  exit 2 with the same message and write no event; `koto cancel R` then exits
  0, R is cancelled and still pinned. `koto session cleanup nosuch` behaves as
  on a build without this feature.
- [ ] **Replace moves a pinned predecessor aside.** After S6 the new run holds
  P's original name with no pin, and `koto session hierarchy` for the moved
  name shows the same pin.
- [ ] **Pinned while running and failure terminal.** A session pinned while
  running is kept at its terminal tick with and without `--no-cleanup`. A
  pinned session ticked to a failure terminal has a `retention` carrying the
  pin and `failure_terminal`. A pinned session held by a live anchor is
  reported with the pin and the anchor.
- [ ] **Pin keeps its session only.** A dispatches B and A is pinned; both
  finish and nothing else is alive. After the retention, the prune keeps A
  with the pin and removes B. Before that prune, the hierarchy query for B
  lists A as a pinned member, and for A states the pin keeps A only.
- [ ] **Pin adds no edge.** The hierarchy query JSON for every session in a
  fixture of two hierarchies, one containing the pinning agent session's open
  anchor, lists the same members and edges before and after the pin is set
  under that anchor.
- [ ] **Unpinned members unchanged.** With A pinned and C, a finished session
  in A's hierarchy, held by a live anchor, the prune's entry for C equals,
  byte for byte, its entry from the same fixture without the pin.
- [ ] **Late pin.** In a fixture where P starts unpinned, finished and past
  the retention, with the test holding stdin open, `koto workspace prune
  --root X` (no `--yes`) lists P for removal and waits; P is pinned from
  another process; the test confirms; P is kept and its entry carries the
  pin. The same holds for `koto session prune --apply --plan` when P is pinned
  after the plan was written, and, under the cloud backend, when host 1 pins
  P after host 2's prune printed its preview.
- [ ] **Hierarchy query.** For a pinned session, `koto session hierarchy` text
  and JSON show the pin as R15 and that it keeps that session only, and the
  session's files hash the same before and after the query.
- [ ] **Listing pins.** With three pinned sessions (one running, one with an
  unreadable pin) and ten unpinned, `koto session list --pinned` lists exactly
  the three, sorted by name, each with its pin or `pin_unreadable` and its
  state. With none pinned it prints an empty list and exits 0.

### Setting, releasing and authority

- [ ] **Pin command.** By hand, `koto session pin P --reason <r>` exits 0 and
  prints `name`, `pin.reason`, `pin.set_by: null`, `pin.set_at` and
  `store: local`; under an anchor, `pin.set_by` names it. A reason of exactly
  500 scalar values (including multibyte characters) is accepted; 501, an
  empty or whitespace-only reason, and reasons containing a tab, a newline or
  U+0007 each exit 2 and write nothing. A missing session, an anchor, a close
  record and a migrated source each exit 2 and write nothing.
- [ ] **Pinning again.** Pinning P again with a different reason exits 0,
  prints the existing pin and leaves P hashing the same. Pinning a session
  with an unreadable pin exits 2 naming the unreadable pin.
- [ ] **Unpin by hand makes it eligible at once.** P is pinned, finished and
  past the retention. From a bare shell `koto session unpin P` exits 0 and the
  very next `koto session prune --apply --all` removes P. Unpinning a session
  with no pin exits 0 and leaves it hashing the same, also under an anchor
  holding no authority.
- [ ] **Unpin clears an unreadable pin.** With P's pin truncated, an unpin by
  hand exits 0, P holds no pin, and the release record's pin fields are
  `null`.
- [ ] **Unpin of a missing session.** `koto session unpin nosuch` exits 2 and
  writes nothing.
- [ ] **Interrupted unpin.** With the pin's removal made to fail after the
  release record is written, P is still pinned and every step of S keeps it.
- [ ] **Release record.** After `koto session unpin P --reason done` by hand,
  the hierarchy query for P shows `released_by: null`, `authority: null`,
  `reason: done`, `released_at`, and the released pin's reason, setter and
  time. After a second pin and unpin it shows only the second release. An
  unpinned P with a release record is removed by the next prune after the
  retention, as an unpinned session.
- [ ] **Refused without authority.** Under an anchor whose start had no
  `KOTO_AUTHORITY`, `koto session unpin P` exits 2, P hashes the same, no
  release record is written, and the message names `release-pin`, that
  anchor, and that the authority is handed at the agent session's start. The
  same holds when the start was handed `bogus`, and when the anchor variable
  names an anchor that doesn't exist (the message says it couldn't be read).
- [ ] **Allowed with authority.** Under an anchor whose start was handed
  `release-pin`, and under one handed ` release-pin , bogus`, the unpin exits
  0 and the release record has `released_by` naming the anchor and
  `authority: release-pin`; the hierarchy query for the second anchor lists
  `bogus` under `unknown_authorities`.
- [ ] **Authority isn't inherited or exported.** Run with synthetic hook
  inputs: a session started with `KOTO_AUTHORITY=release-pin` has its anchor
  record `release-pin` and its shells see no `KOTO_AUTHORITY`. A `claude -p`
  child launched from those shells, a successor anchor whose own start was
  handed nothing, and a start again of the same agent session handed nothing
  are each refused an unpin. Under an anchor whose start was handed nothing,
  `KOTO_AUTHORITY=release-pin koto session unpin P` is refused.
- [ ] **Setter gets no release.** The agent session that pinned P, holding no
  release authority, is refused an unpin of P.
- [ ] **Pinning needs no authority.** Under an anchor with no handed
  authority, `koto session pin` exits 0 and `pin.set_by` names that anchor.
- [ ] **Single writer.** During pin, unpin and the authority recording at a
  start, every session other than the named session (for pin and unpin) and
  the starting agent session's anchor (for the start) hashes the same before
  and after. `koto context add` and `koto context remove` on the pin, the
  release record and the recorded authority each exit 2.
- [ ] **Concurrent pin, unpin and removal.** Twenty runs each of a pin racing
  an unpin, and of a pin racing `koto session prune --apply --all` on a
  finished P past the retention, end with P either pinned (and present) or
  unpinned with a release record, never with a pin and no session, a partial
  pin, or a removed session that was pinned when the prune read it.

### Failing toward keeping

- [ ] **Unreadable pin keeps.** For each unreadable case (P's pin replaced by
  invalid content, by 5 KiB of valid content, by a symlink, by a directory,
  and listed by the store's index with its content deleted), every step of S
  keeps P and its entry carries `pin_unreadable` with `unparseable`,
  `too_large`, `not_a_file`, `not_a_file` and `missing_content` respectively.
  A session dispatched by P and finished is removed by the prune after the
  retention.
- [ ] **Unindexed pin keeps.** A pin present in P with the store's index entry
  for it removed keeps P through S and is reported as a readable pin.

### Cloud backend

All cloud criteria run on two hosts sharing a stand-in bucket (two working
copies at the same canonical path with separate local stores, or two
machines), with the cloud backend set by environment on both. Host 1 pins P
by hand and finishes it; X is P's koto parent.

- [ ] **Second host keeps, every removal.** Host 2 has never pulled P:
  `koto session cleanup P` exits 2 naming the pin with `source: bucket` and
  makes no delete request to the bucket. With host 2 holding a local copy of
  P taken before the pin, each of host 2's `koto session prune --apply --all`,
  `koto workspace prune --root X --yes`, terminal tick of X (parent sweep),
  same-name replace of P, terminal tick of a session in P's hierarchy, and
  `koto cancel P --cleanup` keeps P in the bucket (its objects listed the same
  before and after) and reports the pin with `source: bucket`. U, unpinned in
  the same bucket, is removed from the bucket by the same commands.
- [ ] **Stale index push doesn't unpin.** Host 2, holding a copy of P taken
  before the pin, writes a context key into P (pushing an index without the
  pin) and then runs the removals above; each keeps P and reports the pin, and
  the pin object is still in the bucket.
- [ ] **Anchors elsewhere.** P is held by an anchor that lives only in host
  1's local store (opened by the start hook with `KOTO_SESSION_*` dropped);
  host 2's prune, workspace prune and terminal tick keep P and report the pin,
  not the anchor.
- [ ] **Failed bucket check keeps.** With the stand-in failing every listing
  request, and again with it returning an unparseable listing: on host 2,
  `koto session cleanup P` and `koto session cleanup U` exit 3 and delete
  neither copy; `koto session prune --apply --all` lists P and U as kept
  because the bucket couldn't be checked and exits 0; the terminal tick of X
  keeps its children; `koto workspace prune --root X --yes` deletes nothing
  and lists each member as kept; a same-name replace of U moves it aside;
  `koto cancel U --cleanup` exits 3 and writes nothing. Each kept entry
  carries `bucket_unchecked` with the matching `error`.
- [ ] **Pin and unpin reach the bucket or fail.** With the stand-in refusing
  writes, `koto session pin P` exits 3 and says the pin is held locally only;
  `koto session pin P` again exits 3 with the same message; once writes are
  accepted, the next `koto session pin P` exits 0 and host 2's
  `koto session list --pinned` lists P with `source: bucket`. In a second run,
  once writes are accepted, a `koto context add P <key>` on host 1 pushes the
  pin instead. With deletes
  refused, `koto session unpin P` exits 3 and host 2 still keeps P.
- [ ] **Unpin on one host frees on another.** Host 1 unpins P by hand; after
  the retention, host 2's prune removes P from the bucket. Host 2 unpinning a
  pinned session it holds no local copy of exits 0 and host 1 sees no pin.
- [ ] **Bucket request count.** For `koto session prune --apply --all` over 20
  finished sessions past the retention, 5 of them pinned in the bucket only,
  the stand-in counts at most 20 listing requests for the bucket checks and 5
  reads of pin objects beyond what the same prune makes without this feature.

### Import and compatibility

- [ ] **Import carries the pin.** With the per-command cloud configuration in
  the environment, importing P (pinned, with a release record from an earlier
  pin) into another workspace leaves the moved copy with the same pin and
  release record; S there keeps it; the migrated source keeps nothing.
  Importing a session with an unreadable pin carries it unchanged.
- [ ] **Older koto keeps.** With a pin written by this build, the koto
  v0.23.0 release binary (fetched by the test from the project's release page
  and checked against its published checksum) runs `koto session prune
  --apply --all`, `koto workspace prune --root X --yes`, a terminal tick of X
  and a same-name replace of P on the same store; each keeps P, and the
  prune's entry for P lists an unreadable item naming the pin's record.
- [ ] **Changelog and guides.** The changelog entry and the session-retention
  guide each name the first release that honours pins in full and each of
  `koto session cleanup`, `koto cancel --cleanup` and `koto session import`
  as not honouring pins in older releases; every guide that runs
  `koto session cleanup` as a recovery step has an unpin-first line, checked
  by a doc test.

### Cross-cutting

- [ ] **No niwa.** On a host with no niwa installed, every criterion in the
  Keeping and reporting, Setting, releasing and authority, and Failing toward
  keeping sections, and Second host keeps, every removal, pass with koto and
  koto-skills alone.
- [ ] **Unchanged for unpinned.** koto's existing retention and removal tests
  pass unchanged, and for a fixture of unpinned sessions every output surface
  in R15 matches, byte for byte, a recorded golden output from a build
  without this feature.
- [ ] **Cost.** The R28 local test passes on the CI runner.

## Out of Scope

- **Releasing a pin by idle time, staleness, age or any rule.** Only an
  explicit unpin takes a pin off.
- **Changing how unpinned sessions are kept or removed.** The retention rule,
  its settings and its reasons stay as they are for them.
- **Pinning a whole hierarchy, or many sessions by pattern, in one call.** A
  pin keeps its own session; to keep more, pin each session that matters. A
  hierarchy-wide pin can be added later without changing the pin's format.
- **Changing a pin's reason in place.** Unpin and pin again.
- **Several pins on one session.** A second party sees the existing pin.
- **Defining or granting authority beyond koto's own name.** What
  authorities niwa or any other tool defines, who grants them and how a
  dispatcher decides to hand `release-pin` on are outside koto.
- **Enforcement stronger than today's.** A caller can drop its anchor variable
  to act by hand, and anyone with file access can delete the store; the
  release record shows the first, and nothing here prevents the second.
- **An automatic pruner, or any change to when removals run.**
- **Host-level stores**: the request store, the decider ledger, the terminal
  index, finished records.
- **Removals inside a run koto is still driving.** Batch and retry respawns
  that delete a child koto just created or is re-running don't consult pins.
- **koto requiring niwa, or niwa naming koto.** niwa needs no change.

## Known Limitations

- **Older builds on a shared bucket.** A host running a koto release from
  before this feature can still remove a pinned session with
  `koto session cleanup` or `koto cancel --cleanup`, or drop the pin on
  import, and releases before v0.23.0 ignore pins entirely (v0.22 prunes keep
  a pinned session; its terminal tick and replace don't). Every host sharing
  a bucket needs a release that honours pins (R26).
- **A bypass shows rather than stops.** An agent that drops its anchor
  variable is treated as acting by hand and may unpin; the release record
  then shows a by-hand release where no person was present.
- **A host with no bucket access can't remove under cloud.** Because a failed
  bucket check keeps (R23), a host whose bucket is unreachable removes nothing
  until it can check.
- **A pin keeps storage until released.** A skill that pins every run grows
  the store and the bucket without bound; listing pins (R17) and the prune
  reports make that visible, and nothing limits it.
- **Races aren't closed.** On one host, the per-session lock (R12) orders pins
  and removals. Across hosts nothing orders them: a pin written on one host
  between another host's bucket check and its deletes can leave a pin with no
  session behind it.
- **Older Claude Code sessions.** An agent session started before the plugin
  records authority holds none, so its unpins are refused until it starts
  again with authority handed.

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
  the variable are koto's own and any launcher hands them, the way a
  dispatcher's anchor is handed today. A release by hand keeps the issue's
  interim rule (release stays with whoever runs the command by hand) and the
  escape hatch for a mistaken pin.
- **Authority is what a start was handed, recorded in the anchor, and the
  variable is removed from the shells.** Reading the variable at unpin time
  was rejected: an agent could export it mid-session, and any child launched
  from the shell would inherit it. This mirrors how the start hook already
  records niwa's reap key once and removes it from the shells.
- **Setting a pin needs no authority.** A pin errs toward keeping and a
  person can always undo it; requiring authority would break the first user
  story for every agent nobody handed anything to, which on a host without
  niwa is every agent. A required reason, a recorded setter, a pin listing and
  the prune reports make pins visible instead. The authority set could later
  carry a pinning authority as an additive change.
- **Explicit removal refuses a pinned session.** Session retention lets
  `koto session cleanup` remove whatever retention says; a pin is a stronger
  statement than a live member, and leaving cleanup able to delete it would
  make an unpin pointless as a gate. Refusing, rather than keeping silently,
  tells the operator what to do. `koto cancel --cleanup` refuses before it
  cancels, so a refused command changes nothing.
- **An unpin restarts no clock.** Restarting the retention at unpin was
  considered; the brief and issue ask that the next removal after an unpin
  take the session, and a released record nobody needs has no reason to stay
  another retention period.
- **Under the cloud backend every removal checks the bucket, by object, and a
  failed check keeps.** A second host decides from a local copy that may lack
  the pin, and one host's push can replace the bucket's index of a session's
  keys; trusting the local copy, the index, or the prune's existing check
  that the bucket copy isn't ahead would delete records. The cost is one
  listing per session a removal deletes.
- **Pin writes are strict under the cloud backend.** Today a context write
  whose push fails only warns; a pin that silently stays local gives false
  confidence on every other host.
- **Older builds must keep, not release.** The pin's stored form is chosen so
  that a release with session-retention links but no pin support reads it as
  unreadable, which keeps, rather than as an ordinary link, which releases.
  The stored form itself is the design's choice under that constraint.
- **One pin per session; re-pinning is idempotent.** Several pins with their
  own setters and reasons were considered; one pin keeps the release rule and
  every report simple.
- **Pins are for koto workflow sessions only.** Pinning an anchor or a close
  record would keep only a bookkeeping session under the session-only rule,
  which is never what's wanted.
