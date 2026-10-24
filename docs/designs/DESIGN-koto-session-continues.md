---
status: Proposed
upstream: docs/prds/PRD-koto-session-continues.md
problem: |
  Placeholder, completed in Phase 6.
decision: |
  Placeholder, completed in Phase 6.
rationale: |
  Placeholder, completed in Phase 6.
---

# DESIGN: A session names the session it continues

## Status

Proposed

## Context and Problem Statement

koto's retention engine decides, at every removal point, whether a finished
session may go. It works in two layers. `Graph::scan`
(`src/session/hierarchy/graph.rs`) reads every session in the local store and
builds an undirected graph whose edges are the links a session keeps under
`koto/links/<kind>/<peer>` (dispatched-by, held-by, closes), `parent_workflow`
lineage and `chain/parent`. `Judge::judge` (`hierarchy/judge.rs`) takes the
connected component of a session and returns one verdict for all of it: kept
while any member is alive, then until `retention.keep` passes since the
latest stop. A pin (`src/session/pin.rs`) is an overlay asked about one member
at a time through `Judgment::keeps(member)`; it keeps its own session and
nothing else. The terminal tick doesn't judge at all: `edge_of`
(`src/session/hierarchy.rs`) asks only whether the session has any edge.

A successor session today has no edge to the session it continues, so the
predecessor is judged alone and removed. This feature adds a directed
relation, held by the successor, and the requirements fix how it must behave:

- **Writing it.** A successor records, in its own context, that it continues
  one named predecessor, either in the command that creates it or later on an
  open session that holds no relation, with no request leg, anchor, authority
  or dispatcher involved (PRD R1). The record carries the predecessor's name,
  session id, workspace path and, under the cloud backend, its bucket prefix
  (R2). The verb refuses with exit 2, writing nothing, for an invalid or self
  name, a name found nowhere, an anchor or close record, or a second, different
  predecessor; exits 3 when the bucket can't be listed; and repeats as a no-op
  (R3). A session holds at most one relation; a predecessor may have many
  successors (R4). Under the cloud backend a write the bucket doesn't confirm
  exits 3 and leaves nothing behind (R19).
- **Keeping.** A session is kept while any later member has standing, where
  standing is judged from that member alone: a pin (readable or not), alive
  (running or unreadable log), or within `retention.keep` of its own stop
  (R7, Terms). Nothing earlier keeps anything later, and no hierarchy's
  members, keepers or release instant change (R7). With no later member
  standing, today's rules decide alone (R8). Every removal by rule honours
  this: the terminal tick with or without `--no-cleanup`, the parent sweep,
  the same-name replace (moving aside), `koto workspace prune` in every form,
  `koto session prune` with `--all`, `--execution-dir`, `--older-than` and
  `--apply --plan`, and `--migrated-from`, each judging at its own moment
  including the apply-time re-judgment (R9). Explicit `koto session cleanup`
  and `koto cancel --cleanup` remove anyway, exit 0, and report what they
  passed over; a pin still refuses them (R10).
- **Failing toward keeping.** An unparseable or oversized relation body, a
  session id or prefix that doesn't match, and a cycle each keep the session
  and are reported in `relation_unreadable` with reasons `unparseable`,
  `too_large`, `mismatch` and `cycle`; a session whose relations can't be
  listed (`unlistable`) keeps everything the removal would delete (R11).
- **Reporting.** A kept entry carries `kept_by_relation: true` and
  `relation_keepers`, one object per keeper with `keeper`, `standing`
  (`pin` over `alive` over `retention`), `retention_passes_at`, `pin` and
  `path`; the text form is `kept: continued by <keeper> (<standing>) via
  <path>`. `kept_by_pin`, `pin` and a hierarchy's `keepers` keep their
  meanings (R12). `koto session hierarchy` shows `predecessor`, `successors`
  and the keepers (R13). New fields are omitted when empty (R23).
- **Import.** `koto session import` writes the relation in the copy naming
  the source, never carries the source's own relation, adds a missing one on
  a re-run and refuses a conflicting one with `relation_conflict` (R5). A
  verified `migrated.json` marker counts as a relation from its target to the
  session beside it, which covers imports made by older releases (R6). The
  source becomes a read-only predecessor: ticks and writes keep refusing with
  `session_migrated` naming the copy, while `koto status`, `koto context get`,
  `exists`, `list`, `koto decisions list`, `koto overrides list` and their
  `--workspace` forms answer from the source's last pushed copy, writing
  nothing to the source, the marker or the copy, including on a same-host
  import where the copy holds the source's local name (R14). `prune
  --migrated-from` removes a marker and source only once the relation stops
  keeping the source; the age setting and `--source-retired` are accepted and
  reported as `ignored` (R15). The source's own pin keeps nothing (R16).
- **The bucket.** Before a removal deletes from the bucket, it reads the
  relations naming the session that the bucket holds, including markers, and
  judges each later member's standing from the bucket when this host has no
  copy, so a second host that never pulled the successor names the same
  keepers; a later member under another workspace's prefix counts (R17). A
  check that can't be made keeps and reports `bucket_unchecked`; cleanup and
  cancel exit 3 when the bucket can't be listed (R18).
- **Limits.** No authority check reads the relation (R20); it all works with
  koto alone (R21); on the local backend it costs no bucket request, and under
  the cloud backend a session about to be deleted costs at most 2 requests
  for its relations plus at most 3 per later member reached, beyond the pin
  guard's own (R22); the changelog and retention guide name the release and
  what older releases do (R24).

The hard parts are three. First, the relation must keep in one direction
only, which the undirected component can't express. Second, a successor
usually lives under a different bucket prefix from its predecessor (a fresh
checkout, an import's destination), and nothing in the bucket today indexes a
link by the session it names, so a host that never pulled the successor has
no way to find it. Third, import's read refusals and its removal by age are
built into the shared migration check and the marker prune, which must split
reads from writes without letting a read write into either session.

## Decision Drivers

- **One direction.** Later keeps earlier, never the reverse, and no
  hierarchy changes (R7). This rules out treating the relation as one more
  undirected edge.
- **Same answer on every host.** A host that holds only the bucket must keep
  the same sessions and name the same keepers as the host that holds the
  successor (R17), which is why standing is judged from the keeper alone.
- **Fail toward keeping.** Anything unread, unmatched or cyclic keeps (R11,
  R18), as the rest of the retention engine does.
- **Bounded bucket cost.** No request for a session kept before the relation
  is asked; a constant number per session about to be deleted plus a constant
  per later member, independent of bucket size (R22). Whole-bucket scans are
  out.
- **Launcher independence and no authority.** The verb reads no anchor,
  authority or dispatcher (R1, R20).
- **Stale hosts can't erase it.** A host pushing an old manifest must not hide
  a relation, which is the property #460 built for pins.
- **Reuse what exists.** The link layout and its validation, the pin guard's
  pre-delete listing, the marker and its target check, and the `--workspace`
  bucket reader are the pieces the design should extend, not replace.
- **Contained overlap with #477.** The prefix and push/pull code is under
  open work; the design should touch it as little as it can and say where it
  does.
