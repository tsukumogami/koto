---
schema: design/v1
status: Proposed
problem: |
  koto's retention engine joins sessions into undirected hierarchies and
  judges each one alone, so a session that carries on another's work has no
  way to keep its predecessor, and import retires its source on purpose.
  Under the cloud backend the judgment reads only the local store, and a
  successor usually lives under another workspace's bucket prefix, so a host
  that never pulled it can't see the relationship at all.
decision: |
  The successor holds a fourth link kind, `koto/links/continues/<PRED>`,
  written by `--continues` on `koto init` or `koto session link` and by import
  in its copy. Retention keeps it out of the hierarchy graph and reads it as a
  directed per-member overlay: a session is kept while a later member is
  pinned, alive or within its own retention. Under the cloud backend the
  successor also writes a back-reference under its predecessor's `ctx/koto/`,
  which the pin guard's listing finds, and each removal walks and verifies
  those within a counted request budget. Import keeps its marker and write
  refusals, serves the source's reads through the read-only bucket reader,
  and `prune --migrated-from` waits on the same walk.
rationale: |
  An undirected edge would let an earlier session keep a later one and leave
  pins per member; a per-member overlay beside the pin keeps every hierarchy
  unchanged and gives both directions the PRD asks for. The back-reference is
  the only placement that finds successors under any prefix at no extra
  request, and verifying it against the holder's own relation means a forged
  pointer can only over-keep. Reusing the link layout, the pin guard's
  listing, the marker check and the workspace reader keeps the change out of
  most of the push and pull code that koto#477 is working on.
upstream: docs/prds/PRD-koto-session-continues.md
user_visible_surface: true
decision_provenance: inline-resolved
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

## Considered Options

### Decision 1: The verb a successor uses

Resolved inline.

koto already records a dispatch in two ways: `koto init <name>
--dispatched-by <NAME>` when the session is created, and `koto session link
<SESSION> --dispatched-by <NAME>` on an open one. Both write one link into the
named session through `links::write_link`, read no anchor and no authority,
and are idempotent. The relation needs the same two moments (R1), a
predecessor that may live in another workspace (R2), and refusals with exit
codes 2 and 3 (R3, R19).

Key assumptions:
- The successor's own workspace is the current directory's prefix, as for
  every cloud command since #463.
- Looking the predecessor up in another workspace uses the bucket reader
  `koto --workspace` already has (`WorkspaceReader::locate`).

#### Chosen: `--continues` on `koto init` and on `koto session link`

`koto init <name> --continues <PRED> [--continues-from <WORKSPACE>]` records
the relation in the command that creates the session, after the header is
written and before the first push, so the successor never exists without it.
`koto session link <SESSION> --continues <PRED> [--continues-from
<WORKSPACE>]` records it on an open session; `--dispatched-by` and
`--continues` become an exclusive pair on `link`, one of them required.
`--continues-from` takes an absolute workspace path or a 16-hex workspace id,
parsed by `WorkspaceId::parse` as `koto --workspace` parses it, and defaults to
the successor's own workspace. Nothing in either form reads `KOTO_ANCHOR`,
`KOTO_AUTHORITY` or a request leg, so a coordinator's successor, an operator
by hand and an unrelated agent session all run the same command. It mirrors a
pair users already know, and every check `handle_link` and the init path make
(name grammar, not itself, session exists) carries over.

#### Alternatives Considered

- **A new subcommand, `koto session continue <SESSION> <PRED>`.** Just as
  launcher independent, but it can't run in the creating command, so a
  successor would exist for a moment without its relation and a prune in that
  window could take the predecessor. It also adds a second verb shape beside
  the dispatch pair for the same kind of record.
- **An environment variable read at init (`KOTO_CONTINUES`).** It depends on
  the launcher setting the environment, which R1 rules out, and every child
  process the shell starts inherits it.
- **Reusing `--dispatched-by`.** A dispatched-by link joins hierarchies
  undirected and is read by the anchor family and by request dispatch; R7 and
  R20 forbid both.

### Decision 2: Storing the relation and counting it in retention

Resolved inline, and re-resolved once after cross-validation with Decision 4.

`Graph::build_edges` turns every link of every kind, known or not, into an
undirected edge, and `Judge::judge` gives one verdict per component. A pin is
a per-member overlay read through `Judgment::keeps(member)`. Seven sites
decide removals from the verdict or from `keeps`: `first_rule` and
`apply_component` in `src/cli/session_prune.rs`, `judge_subtree` and `reclaim`
in `src/cli/workspace.rs`, `predecessor_disposition` in
`src/cli/init_entry.rs`, the terminal tick's `edge_of` in
`src/session/hierarchy.rs`, and the advisory `live_members` in
`src/cli/session.rs`.

Key assumptions:
- Chains stay short, and member state is read once per command through the
  judge's existing `state_of` cache.
- Decision 4 supplies relations and standing for later members this host
  doesn't hold.

#### Chosen: a fourth link kind, kept out of the components, read as a directed per-member overlay

The relation is `LinkKind::Continues`, key `koto/links/continues/<PRED>`, a
`LinkBody` with two new optional fields: `prefix` (the predecessor's bucket
prefix under the cloud backend) and `holder_session_id` (the successor's own
session id, which lets a bucket reader match the relation without reading the
successor's state), beside the existing `session_id`, `workspace` and `at`.
`v` stays 1; older readers ignore unknown fields. A session holds one: the
verb refuses when any other `koto/links/continues/*` key is present.

`Graph::build_edges` keeps continues links out of the undirected loop and the
session-id-hint loop and puts them in a separate `directed` list (holder is
the successor, target the predecessor), resolved by recorded session id first
and then by name, so the predecessor a same-name replace moved aside to
`<name>_r<N>` stays the one kept, and a new session that took the old name
reads as `mismatch`. A migrated name isn't redirected to its copies for this
list: the predecessor is the source itself. A continues link whose peer is the
holder's own name (a same-host import's copy naming its source in another
workspace) names nothing in this store and adds no edge.

`Judge::relation(name)` walks the reverse of `directed` breadth-first from
`name` with a visited set, collecting each later member and its path. Each
member's standing comes from the member alone: its pin fact (reconciled with
the bucket as pins already are), alive from `state_of`, else its own stop plus
`retention.keep` against the judge's `now`. A revisited member is `cycle`, an
unparseable or oversized body `unparseable` or `too_large`, a recorded session
id that differs from the session found `mismatch`. `Judgment` gains
`relation_of(member)`, and `keeps(member)` becomes `pin || verdict.is_kept()
|| relation keeps member`. The component verdict doesn't change, so every
hierarchy's members, keepers and release instant are the same with and
without the relation (R7).

#### Alternatives Considered

- **An undirected edge.** Forward keeping comes free, but a live predecessor
  or its anchor keeps the successor, a late-finishing predecessor delays the
  successor's release instant, and a pin on the newest member still keeps only
  that member, so both direction criteria fail.
- **Extending `koto/imported-from`.** It exists only on import copies, has no
  workspace or prefix, is stripped on import and isn't a link, so the
  coordinator and stranger cases would need a second record anyway.
- **A peerless reserved key like the pin.** A link keyed by its peer reuses
  the link layout, its validation, hint resolution and the moved-aside
  handling; a peerless record would need all of it built again.
- **Keeping the predecessor's whole component.** A pin keeps its own session
  only. Doing the same keeps the existing removal order intact and stops a
  successor from keeping unrelated sessions its predecessor dispatched.

### Decision 3: Import keeps its source as a read-only predecessor

Resolved inline.

`session_migrated` comes from one check, `CloudBackend::check_migration`,
which `read_events`, `read_header`, the `ContextStore` getters, the CLI
context gate and every write share; `koto next` and `koto status` both reach
it through `read_events`. A same-host import puts the copy into the shared
local store under the source's name, so a read of the source from the
source's workspace can't use the local store. The `--workspace` reader in
`src/session/cloud_workspace_read.rs` already reads a session in place from a
prefix into a private scratch directory, read-only, but stops at a marker.
`prune --migrated-from` removes a verified source once the marker is old
enough or with `--source-retired`.

Key assumptions:
- The source's last pushed copy is complete; import already requires a
  stopped source whose last write reached the bucket.
- Reads of a migrated source are rare, so a fresh scratch pull per command is
  acceptable.

#### Chosen: keep the backend's refusal, serve the listed reads through the bucket reader, judge the marker by the relation

The backend doesn't change: every write path and `read_events` keep refusing,
so `koto next` and each write command keep today's message. The CLI handlers
of `koto status`, `koto context get`, `exists`, `list`, `koto decisions list`
and `koto overrides list` catch `SessionMigrated` on their first read and
answer through a `WorkspaceReader` on the session's own prefix opened with a
new `serve_migrated` flag, under which `locate` fetches the state file even
when the probe shows a marker. The reader writes nothing to the bucket or the
shared local store and appends no read record, so neither the source, its
marker nor the copy that holds the name after a same-host import changes. The
`--workspace` forms use the same flag, and `decisions list` and
`overrides list` join the reader's verbs.

`import_links::dropped_on_import` drops any `koto/links/continues/*` the
source held, and `Settlement::apply` writes `koto/links/continues/<source>`
with the source's session id, workspace path and prefix and the copy's own id
before the copy is pushed. The re-run paths add it when missing and refuse
with `relation_conflict`, exit 2, when the copy holds another.
`prune --migrated-from` keeps its `Present` check, then runs Decision 4's walk
for the source with the marker's target as a later member; while that keeps,
the entry is kept with reason `relation` and `relation_keepers`.
`marker_retain_days` and `--source-retired` are read only to report them as
`ignored`. `guard_removal` keeps treating the source's pin as inert.

#### Alternatives Considered

- **A read mode in the backend's migration check.** It would let `read_events`
  pull a migrated source into the shared local store, which after a same-host
  import is the copy's directory: the overwrite `refuse_cross_import_pull`
  exists to stop.
- **Leaving reads refused.** Fails R14 and the import criterion.
- **Dropping the marker and relying on the relation alone.** The marker is
  what stops another host from ticking the source, and what lets anyone check
  the copy against the source's log.

### Decision 4: Finding the relations the bucket holds

Delegated to /decision.

A successor's relation lives under its own prefix,
`<Sp>/<S>/ctx/koto/links/continues/<P>`, and nothing in the bucket indexes a
relation by the session it names, so a host that holds P but never pulled S
can't learn S exists, and S usually sits under another workspace's prefix. No
push deletes an object that no manifest lists, which is how pins survive stale
hosts. The pin guard already lists `<Pp>/<P>/ctx/koto/` before every guarded
delete (`bucket_pin_of`, `src/session/cloud.rs`). A GET that answers 404 costs
two requests and a retry delay, so presence checks have to be listings.
"Terminal" comes from the compiled template, so a member whose template this
host lacks needs one GET of `template.json` per template hash per run.

Key assumptions:
- The relation stays under `ctx/koto/` and records the holder's own session
  id (Decision 2 does both).
- R22's budget is a total per session about to be deleted, 2 plus 3 per
  member reached, with a template read counted inside it.
- Writing into another workspace's prefix needs write access there, as
  import's marker already does; without it the verb exits 3.
- Buckets shared with older releases stay unsupported, as the PRD's Known
  Limitations say.

#### Chosen: a back-reference object under the predecessor's `ctx/koto/`, verified against the holder's relation, walked within a counted budget

The successor writes one standalone object,
`<Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>`, whose body is only
`{"v":1,"holder_session_id":"<id>"}`: the guard never decides anything from a
pointer's body, and no workspace path is copied into another workspace's
prefix. It is written by a direct, checked cross-prefix PUT, never
through the context store, so it is never in a manifest and no push deletes
it; it sits outside `koto/links/`, so no build's scan reads it as a link. The
pin guard's existing `ctx/koto/` listing therefore finds every successor of P,
under any prefix, at no extra request.

Writes go pointer first. The verb looks P up (exit 3 when unlistable), PUTs
the pointer (exit 3 on failure, nothing written), writes the relation and
pushes it strictly; if the push fails it removes the local key and the
pointer and exits 3. Import PUTs the pointer beside the source, pushes the
copy with its relation, then writes the marker. A relation recorded under the
local backend leaves a host-local "pointer owed" mark; before any cloud write
or push of that session, a command-layer `ensure_backref` beside
`refuse_if_bound_elsewhere` PUTs the owed pointer, and until then the relation
key is kept out of pushes at the three places pins are kept out. A rename of
the successor (the move-aside, the batch rewind's relocate) PUTs the new
pointer before moving and deletes the old one after.

Pointers outlive P's deletion, as `migrated.json` does: the guarded delete
paths skip `ctx/koto/continued-by/`, so a reused name reads as `mismatch` on
every host. The move-aside carries P's pointers to the moved name. The holder
deletes its own pointer when it is removed, building the key only from its own
workspace id and name and the predecessor its own relation names, never from
anything read from the bucket; the guard never deletes one.

The guard trusts only the holder's own relation. Before a pointer key becomes
a request it is validated: exactly `continued-by/<Sp>/<S>` under P's own
`ctx/koto/`, `<Sp>` 16 lowercase hex digits, `<S>` a valid session name. A key
that fails is reported as `relation_unreadable` (`unparseable`) and keeps P.
For each valid pointer the guard lists `<Sp>/<S>/ctx/koto/`; a listing that
fails for any reason, a permission error included, keeps P as
`bucket_unchecked` naming S, so a credentials gap can't turn into a deletion.
No continues key in a successful listing means a stale pointer, ignored; a key over 4 KiB is
`too_large`; otherwise it GETs the relation, and a recorded session id equal to
P's makes S a later member, the same name with another id is `mismatch`, and
anything else is ignored. A forged pointer can only send the guard to read a
relation the holder really holds, so forgery can only over-keep.

Standing costs three requests per member: the `ctx/koto/` listing (pin, the
member's relation and its own pointers), the relation GET, and a GET of the pin
when listed (standing `pin`, which takes precedence) or else of the state file
(alive, cancelled, stop instant, through a new standing-from-bytes function
that applies `inventory.rs`'s rules, with the template from the local cache or
one `template.json` GET per hash per run, checked against the hash the state
header records and parsed as data only; nothing in it runs). Every GET the
walk makes is bounded in size (4 KiB for relations and pins, the existing
limits for state and template files), and anything that doesn't parse keeps.
A reached member counts as migrated
(no standing of its own) when one of its successors holds `koto/imported-from`
and a relation naming its id written no earlier than its last event, which is
the local graph's `find_migrated` rule on reads the walk already makes. One
delimiter listing of `<Pp>/<P>/` shows a `migrated.json` beside P (R6); the
marker's target is then verified with `marker_target_status` and walked like
any other later member.

The walk runs at each removal point's judgment, after the local judgment says
delete and the pin guard is clear, so a session kept for its own standing
costs nothing; it shares the pin listing through an additive
`bucket_removal_facts` beside `bucket_pin`. The final guard before the delete
compares its fresh listing with the judgment's pointers and walks only new
ones. It counts its requests against 2 for P plus 3 per member reached, and
every pointer it examines, valid or not, stale or not, draws from that
allowance; a listing with more than 64 pointers is not walked at all. A read
that would pass the allowance, or a pointer flood, stops the walk and keeps P
as `bucket_unchecked` naming what it couldn't afford. Counts: nothing names
P, 1 request (2 where no pin listing is shared); fifty deletions naming
nothing, at most 100; the three-member chain judged on a host holding only v1,
7, or 8 with one uncached template, against an allowance of 8. When P's header
`origin.store.prefix` differs from this host's prefix, the guard also lists the
origin's `ctx/koto/`.

#### Alternatives Considered

- **A back-reference at the predecessor's top level**
  (`<Pp>/<P>/continued-by.<Sp>.<S>`, found by a delimiter listing beside the
  marker). Costs 4 requests per member unless ETag equality replaces the
  relation read, which fails on SSE-KMS and SSE-C buckets and skips checking
  the session id. Its one advantage, seeing markers that older releases left
  beside later members, needs a mixed-version bucket the PRD rules out.
- **A bucket-level index keyed by the predecessor's session id**
  (`koto-relations/<P id>/...`). It writes and lists outside every workspace
  prefix, so under repo-scoped credentials every guarded removal would keep
  and cleanup would exit 3 even with no relations, breaking R23; nothing
  deletes an entry when P goes; and it needs a second listing per member.
- **Discovering by scanning** P's prefix or the whole bucket. S3 filters a
  listing only by the start of a key, and the relation's key puts P's name
  after S's, so finding successors means listing every session in scope. The
  cost grows with the bucket (fails R22), the prefix-only form misses
  successors in other workspaces (fails R17), and a whole-bucket listing may
  need root-level permission. It stays useful as a test oracle.
- **Recording the successor in the predecessor's own context.** It writes an
  event into another session's log and manifest, against R1 and the
  cross-session writer limits #445 tracks; it can't write into a migrated
  source without breaking R14 and the import's log-prefix check; and a stale
  host's manifest can drop it.

### Decision 5: What the terminal tick reads

Resolved inline, from the architecture review of Phase 4.

The terminal tick keeps a session whenever `edge_of` finds any edge, without
judging anything, which is what keeps the tick cheap. A continues link counts
as an edge for both the holder and the session it names, so the tick already
keeps the chain; what it lacks is the keeper's name the PRD asks every removal
to print (R12).

#### Chosen: judge only when a continues link names the session

A new `relation_probe`, sharing `edge_of`'s pass over the store, says whether
any continues link names the session. Only then does the tick build a graph
and run `Judge::relation` to fill `retention.relation_keepers`. A session no
relation names costs exactly what it costs today.

#### Alternatives Considered

- **Judging at every terminal tick.** Correct and simpler, but it puts a full
  store scan on every tick of every session, which `edge_of` exists to avoid.
- **Reporting only `reason: hierarchy`.** Cheapest, but the tick would be the
  one removal point that can't say which later session keeps the predecessor,
  and the chain criterion checks it there.

## Decision Outcome

The relation is a link the successor holds, `koto/links/continues/<PRED>`,
written by `--continues` on `koto init` or `koto session link`, and by
`koto session import` in its copy. Locally, the retention engine keeps it out
of the hierarchy graph and reads it as a directed, per-member overlay: a
session is kept while any later member has standing of its own, and the
component verdict, pins and every hierarchy stay as they are. Under the cloud
backend, the successor also writes a back-reference beside its predecessor
that the pin guard's existing listing finds, and every removal walks those
back-references, verifies each against the holder's own relation, and reads
each later member's standing from the bucket within a counted budget. Import
keeps its marker and its write refusals, serves reads of the source from the
bucket through the read-only workspace reader, writes the relation into the
copy, and `prune --migrated-from` waits on the same walk.

The pieces fit through two shared shapes. Decision 2's relation body carries
the holder's session id and the predecessor's prefix, which is what lets
Decision 4's guard verify a pointer with one read and what lets the local and
bucket judgments resolve a moved-aside predecessor the same way. Decision 4's
walk is the one bucket answer every removal point asks, including Decision 3's
`prune --migrated-from` and the second-host prune, so the same chain produces
the same `relation_keepers` wherever it is judged.

## Solution Architecture

### Overview

Three layers change. The link layer gains a kind and a verb. The retention
engine gains a directed overlay that every removal point consults through
`Judgment::keeps`. The cloud backend gains a back-reference object, a bounded
walk over it, and a read path for migrated sources. Nothing changes for a
store with no relation and no marker (R23).

### Components

- `src/session/links.rs`: `LinkKind::Continues` (`as_str` is `continues`,
  listed in `ALL`); `LinkBody` gains optional `prefix` and
  `holder_session_id`; `write_continues(store, session, pred, hints)` wraps
  `write_link` with the one-relation check and lets a same-name peer through
  only when its prefix differs from the holder's (import).
- `src/cli/mod.rs` and `src/cli/session.rs`: `Init` gains `--continues` and
  `--continues-from`; `SessionCommand::Link` takes `--continues` as the
  alternative to `--dispatched-by`; a shared `record_continues` resolves the
  predecessor (local store, then the named workspace's prefix through
  `WorkspaceReader`), refuses anchors and close records, and maps errors to
  exit 2 or 3. `ensure_backref` runs beside `refuse_if_bound_elsewhere`.
- `src/session/hierarchy/facts.rs`, `graph.rs`, `judge.rs`, `render.rs`: the
  scan reads unlisted `continues` files; `Graph` keeps a `directed` list
  outside `components()` and `joined_by`; `Judge::relation` and
  `Judgment::relation_of`; `relation_json` and `relation_text` render
  `kept_by_relation`, `relation_keepers` and `relation_unreadable`;
  `query_json` and `query_text` add `predecessor` and `successors`.
- `src/session/hierarchy.rs`: `edge_of` is unchanged, since it already counts a
  link of any kind in both directions; a new `relation_probe` tells the
  terminal tick whether a continues link names the session, so it judges the
  session only then.
- Removal sites: `first_rule` (a new rule `r`, plus the relation fields on any
  kept entry) and `apply_component` (per-entry `keeps`, no group gate) in
  `src/cli/session_prune.rs`; `judge_subtree` and `reclaim` in
  `src/cli/workspace.rs`; `predecessor_disposition` in
  `src/cli/init_entry.rs`; `terminal_retention` in `src/cli/mod.rs`
  (`retention.relation_keepers`); `live_members` in `src/cli/session.rs` for
  cleanup's and cancel's report.
- `src/session/continues_bucket.rs` (new): the walk, its request counter, a
  per-process memo keyed by (prefix, name), and `standing_from_bytes`.
- `src/session/cloud.rs`: `put_backref`, `delete_backref` and
  `is_backref_key`; `bucket_removal_facts` beside `bucket_pin`, with its
  `SessionBackend` default in `src/session/mod.rs`;
  `delete_remote_session_checked`, `sync_delete_session` and
  `delete_migrated_source` skip `ctx/koto/continued-by/`; the filters that keep
  pin records out of a push (`src/session/sync.rs`, `force_push_session`,
  `flush_batch`) also keep out a relation whose pointer is owed.
- `src/session/import_links.rs` and the import in `cloud.rs`:
  `dropped_on_import` drops continues links; `Settlement::apply` writes the
  copy's relation; `import_session` PUTs the pointer before pushing the copy
  and checks the re-run paths for `relation_conflict`.
- `src/session/cloud_workspace_read.rs` and `src/cli/workspace_read.rs`:
  `serve_migrated` on `WorkspaceReader`; `decisions list` and
  `overrides list` join the reader's verbs; the read handlers in
  `src/cli/mod.rs` and `src/cli/context.rs` fall back to the reader on
  `SessionMigrated`.
- `src/cli/session_prune/markers.rs`: `evaluate` runs the walk after
  `Present`, and reports the age setting and `--source-retired` as `ignored`.

### Key Interfaces

```text
koto init <name> --continues <PRED> [--continues-from <WORKSPACE>] ...
koto session link <SESSION> (--dispatched-by <NAME> | --continues <PRED>) [--continues-from <WORKSPACE>]

context key  koto/links/continues/<PRED>
body         {"v":1,"session":"<PRED>","session_id":"<id>","workspace":"<path>",
              "prefix":"<16 hex>|null","holder_session_id":"<id>","at":"<rfc3339>"}
bucket       <Pp>/<PRED>/ctx/koto/continued-by/<Sp>/<SUCC>   (body: {"v":1,"holder_session_id":"<id>"})
```

```rust
pub enum Standing { Pin, Alive, Retention }
pub struct RelationKeeper {
    keeper: String, standing: Standing, retention_passes_at: Option<String>,
    pin: Option<Pin>, path: Vec<String>,
}
pub struct RelationVerdict { keepers: Vec<RelationKeeper>, unreadable: Vec<Unreadable> }
impl Judge<'_> { pub fn relation(&mut self, name: &str) -> RelationVerdict }
impl Judgment {
    pub fn relation_of(&self, member: &str) -> &RelationVerdict;
    pub fn keeps(&self, member: &str) -> bool; // pin || verdict || relation
}
pub fn walk(backend: &CloudBackend, pred: &str, facts: &BucketRemovalFacts) -> RelationVerdict;
```

Kept entries carry the PRD's fields: `kept_by_relation`, `relation_keepers`
(`keeper`, `standing`, `retention_passes_at`, `pin`, `path`),
`relation_unreadable` (`holder`, `reason`), and `ignored` on
`prune --migrated-from`; the text line is `kept: continued by <keeper>
(<standing>) via <path>`. Empty fields are omitted.

### Data Flow

Writing: the verb resolves the predecessor, PUTs the pointer under the
predecessor's prefix (cloud only), writes the relation key into the successor
through the context store with writer `koto`, and pushes the successor
strictly. Import does the same for the copy, then writes the marker.

Judging locally: `Graph::scan` reads every session's links once; the
`directed` list feeds `Judge::relation`, which reads each later member's state
through the judge's cache. A removal point asks `keeps(member)`, and an entry
kept by the relation carries the rendered fields.

Judging against the bucket: at a removal point that would delete P, after the
pin guard is clear, `bucket_removal_facts` returns P's `ctx/koto/` listing
(shared with the pin check) and a delimiter listing of `<Pp>/<P>/`. The walk
verifies each pointer and marker target, reads each member's standing,
follows each member's own pointers, and returns a `RelationVerdict` merged with
the local one. Anything unaffordable or unreadable keeps P.

Reading a migrated source: the read command's first backend read refuses with
`SessionMigrated`; the handler opens a `WorkspaceReader` on the session's own
prefix with `serve_migrated`, pulls the state and keys into a scratch
directory, answers, and drops the directory.

### Seams, limits and edge cases

- **Where the bucket answer meets `keeps`.** `Judgment::keeps` stays
  synchronous and local. A removal site that gets `false` from it, under the
  cloud backend, then calls `continues_bucket::bucket_keepers(backend, name,
  &facts) -> RelationVerdict` before it deletes, and keeps the session when
  that verdict has a keeper or anything unreadable, merging it into the
  entry's report. Every site that can delete from the bucket makes that call:
  `apply_component` and the preview in `session_prune.rs`, `judge_subtree` and
  `reclaim` in `workspace.rs`, `predecessor_disposition` in `init_entry.rs`,
  the terminal tick's removal in `cli/mod.rs` (when `edge_of` finds no local
  edge, so a predecessor whose successor lives only in the bucket isn't
  deleted by its own tick), and `koto session cleanup` and
  `koto cancel --cleanup` in `cli/session.rs` for their report only.
- **`BucketRemovalFacts`.** The result of one fresh listing of
  `<Pp>/<P>/ctx/koto/` (already made for the pin) plus one delimiter listing
  of `<Pp>/<P>/`: the pin and release keys and sizes, the validated pointer
  keys, whether `migrated.json` is present, the state object's ETag, and,
  when P's header names another `origin.store.prefix`, the same `ctx/koto/`
  listing for the origin.
- **The memo** lives for one judgment pass. `--apply` starts a fresh one, so
  its re-judgment reads the bucket again. It is keyed by session id, falling
  back to (prefix, name) only before the id is known.
- **Ceilings.** Besides the allowance (2 plus 3 per member), a walk stops at
  16 members or 50 requests, whichever comes first, and keeps P as
  `bucket_unchecked`; a listing with more than 64 pointers isn't walked.
- **The confirmed-pointer mark.** A relation's pointer counts as written only
  when the session directory holds `.continues-pointer`, a bare file the sync
  doesn't track, recording the pointer key. Its absence means a pointer is
  owed, which also covers a relation an earlier build wrote under the local
  backend. `ensure_backref` writes the owed pointer before the session's next
  cloud write or push; a failed PUT warns, keeps the relation out of that
  push, and leaves the session's other writes alone.
- **The terminal tick and holders.** `edge_of` counts a continues link on the
  holder's side too, so a successor's own terminal tick leaves it for a prune
  to judge, as any session holding a link is left today. That is intended:
  the tick never removes something a prune would keep.
- **Markers locally.** Markers live only in the bucket, so R6 is answered by
  the bucket walk. A same-host import with `--as` keeps both sessions local,
  and the copy's own relation keeps the source there.
- **The local backend.** Import needs the cloud backend, so a migrated source
  never exists on a local store and the read fallback never runs there.
- **The move-aside.** Carrying P's pointers to `<name>_r<N>` copies only keys
  that pass validation, at most 64, and never deletes an invalid key; anything
  not carried is reported, and the moved predecessor is still found by the
  recorded session id in its successors' relations.
- **A rollback that fails.** If the verb's push fails and the pointer's DELETE
  also fails, the pointer is left with no relation behind it. The guard reads
  that as stale and ignores it, so it is accepted.
- **Shipping.** The three implementation phases ship in one release. Phase 1
  alone refuses a cloud relation rather than writing one without a pointer.

### Overlap with other work

koto#477 (bucket copies that fork when one session is written from several
directories) is open work on the prefix and push/pull code. This design
touches that area in three places: `ensure_backref` beside
`refuse_if_bound_elsewhere` in `src/cli/mod.rs`; one predicate in each of the
three places pin records are kept out of a push (`src/session/sync.rs`, and
`force_push_session` and `flush_batch` in `src/session/cloud.rs`); and the new
cross-prefix `put_backref` and `delete_backref` in `cloud.rs`. No push
function is reordered and nothing here depends on #477 being fixed. A later
#477 fix that sweeps stray copies must carry pointers to the session's origin
rather than delete them. `handle_rebind`, `sync_pull_state` and `put_state`
are untouched.

The pins and anchors code shares more files. `src/session/pin.rs` is where the
walk sits beside, not inside, `guard_removal`; `SessionBackend` in
`src/session/mod.rs` gains `bucket_removal_facts` beside `bucket_pin`;
`src/session/hierarchy/*` and `src/session/links.rs` change as above;
`src/session/import_links.rs` gains the continues rules; and the removal sites
in `src/cli/session_prune.rs`, `src/cli/workspace.rs` and
`src/cli/init_entry.rs` move from the verdict to `keeps`.
`src/session/anchor.rs` is untouched: `current_of_family` reads only
dispatched-by links, and the anchor authority path reads no links.

## Implementation Approach

### Phase 1: The relation and local retention

The link kind, the body fields, both verb forms, the graph's directed list,
`Judge::relation`, `Judgment::keeps`, every removal site, the report fields,
`koto session hierarchy`'s new fields, and the failure cases (`unparseable`,
`too_large`, `mismatch`, `cycle`, `unlistable`). Under the cloud backend this
phase's verb refuses with exit 2 when it would need a pointer it can't yet
write (a cloud session or `--continues-from` another workspace), so no
relation reaches a bucket without one. Property tests: `reference()` in
`hierarchy/tests.rs` skips continues links in its adjacency, a second,
directed reference checks the overlay, and the hint test runs for continues
links.

Deliverables:
- `src/session/links.rs`, `src/session/hierarchy/{facts,graph,judge,render}.rs`
- `src/cli/mod.rs` (init flags, terminal tick), `src/cli/session.rs` (link,
  hierarchy, live members), `src/cli/session_prune.rs`,
  `src/cli/workspace.rs`, `src/cli/init_entry.rs`
- tests for the chain, release, both directions, two successors, the replace,
  cleanup and cancel, refusals and authority

### Phase 2: The bucket walk

Back-reference writes (the verb, the local-backend carry, renames), delete
paths that skip pointers, `bucket_removal_facts`, the bounded walk with its
counter and memo, and its use at every removal point; the cloud verb opens up.
Tests use the in-process stand-in with paging at 1000 keys and a request
counter, and a whole-bucket oracle on fixtures with a rename, a second host
and a moved-aside predecessor.

Deliverables:
- `src/session/continues_bucket.rs`, `src/session/cloud.rs`,
  `src/session/sync.rs`, `src/session/mod.rs`, `src/cli/mod.rs`
- the `bucket_keepers` call at each site: `src/cli/session_prune.rs`,
  `src/cli/workspace.rs`, `src/cli/init_entry.rs`, `src/cli/session.rs`
- the pointer re-point at the rename sites: the same-name replace's
  move-aside (`src/cli/move_aside.rs`) and the batch rewind's relocate
  (`src/cli/batch.rs`)
- tests for the second host, another workspace, the unchecked bucket, the
  unconfirmed write and the bounded cost

### Phase 3: Import keeps its source

The copy's relation and pointer, re-run and conflict handling, the read path
through `serve_migrated`, `prune --migrated-from` waiting on the walk with the
`ignored` report, and the docs: the changelog, the session retention guide
(what keeps a session, the cross-session writer list, older releases), the CLI
guide and the koto-user skill's error tables.

Deliverables:
- `src/session/import_links.rs`, `src/session/cloud.rs` (import),
  `src/session/cloud_workspace_read.rs`, `src/cli/workspace_read.rs`,
  `src/cli/context.rs`, `src/cli/session_prune/markers.rs`
- `CHANGELOG.md`, `docs/guides/session-retention.md`,
  `docs/guides/cli-usage.md`, the koto-user skill
- tests for import across workspaces and on one host, migrated-from, earlier
  imports, the absent target, re-run and pin travel

## Security Considerations

The relation adds objects that other hosts write and this host reads, so each
of them is untrusted input.

A back-reference is only a hint. The guard never decides anything from its
body; it uses the key to find the holder's own relation and trusts that
relation only after matching its recorded session id to the predecessor's. A
forged, stale or reused-name pointer therefore finds nothing and is ignored,
finds a mismatch and keeps the session, or finds a real relation and keeps
the session. A forged pointer can add a keeper, never remove one. Every part
of a key or body that becomes a request, a scratch path or output is validated
first: workspace ids and the relation's `prefix` are 16 lowercase hex digits,
names pass the session-name grammar, session ids match the id format, `at` is
RFC 3339, `workspace` is an absolute path used only as a hint, and pointer
keys have exactly the expected segments. Anything else is reported as
unreadable and keeps. A forged pointer can make a host LIST and GET under a
prefix of the forger's choosing before the relation is checked; nothing read
from an unverified holder is printed beyond its validated or placeholder name.

Work is bounded. Every pointer examined, valid or not, draws from the walk's
request allowance, a listing with more than 64 pointers isn't walked, and a
walk stops at 16 members or 50 requests; running out keeps the session and
reports `bucket_unchecked`. A listing that fails for any reason other than an
absent key, a permission error included, is `bucket_unchecked` and keeps, so a
credentials gap can't become a deletion. Relation, pin, state and template
reads are size-bounded. The template comes from the local cache when it holds
the hash; one fetched from the bucket is checked against the hash its
session's header records, which proves it matches that header but not who
wrote it, and is parsed as data with the compiler's existing depth limits;
nothing in it runs, and the walk reads only which states are terminal.

The permission this feature adds is the successor's write into its
predecessor's prefix. It needs PUT and DELETE only on `ctx/koto/continued-by/`
keys there, and, on the predecessor's and other successors' prefixes, LIST of
`<prefix>/<session>/` and its `ctx/koto/`, GET of relations, pins, state files
and `template.json`, and the same on a session's `origin.store.prefix`;
without write access the verb exits 3 and writes nothing. Import's marker is
already a cross-prefix write today (`mark_source` PUTs `migrated.json` under
the source's prefix from the destination workspace). A holder deletes only the
pointer built from its own workspace id, its own name and the predecessor its
own relation names, never one named by bucket data; the move-aside carries
only validated keys and deletes none it can't validate. The
`koto/continued-by/` area sits under the reserved `koto/` prefix, so
`koto context add` and `remove` can't touch it. A successor can keep its
predecessor for as long as it stands, which is the relation's purpose and
costs one small object; it can't read, change or remove anything else, and
explicit `koto session cleanup` still removes the predecessor, pointer flood
or not, while a pin still refuses. Pointers left by a flood are removed by
hand with the bucket's own tools; koto never deletes one it can't attribute.

The pointer body carries no workspace path, so no local path is copied into
another workspace's prefix. Reads of a migrated source are served only for the
caller's own workspace prefix or a prefix the caller names with
`--workspace`, under the same prefix checks `--workspace` applies today; they
write nothing and use a private scratch directory (mode 0700, keys validated
before any path is joined, as the reader does today) removed afterwards. Names
and prefixes printed in `relation_keepers`, `relation_unreadable` and
`bucket_unchecked` come from bucket data, so they go through the retention
reports' existing `sanitize`/`display_text` rendering, and a value that fails
its validator is shown as a placeholder.

The relation grants no authority. The verb reads no anchor, authority or
request leg; continues links are kept out of the hierarchy graph and out of
the anchor family's lookup, which reads dispatched-by links only; and pin
release keeps checking the caller's anchor for `release-pin`, never a link.
Residual risks accepted, and named in the retention guide:

- Anyone who can write a relation or a pointer can hold a session in place
  until it is removed by hand, and there is no unlink verb. Where
  `retention.keep` serves data minimization, bucket policy should limit who
  can write `ctx/koto/` under each workspace prefix.
- Anyone who can write a successor's own objects (its relation, state or pin)
  can make it look released and so let its predecessor go, in a workspace that
  writer may not otherwise reach. The same holds for pins today; bucket policy
  is the control.
- Between a removal's final listing and its delete, a pointer written by
  another host isn't seen, the same window the pin guard accepts.
- An older release sharing the bucket ignores relations and can delete a
  predecessor.
- Pointers left after their predecessor is gone carry holder names and ids in
  that prefix until each holder is removed.

## Consequences

### Positive

- A predecessor survives every removal by rule while its lineage needs it, on
  every host, with one reason printed wherever it is kept.
- Pins and hierarchies keep their meaning, and a store with no relation
  behaves as before.
- Import no longer throws away the record its copy came from, and the source
  stays readable without becoming writable.
- One bucket walk answers every removal point, so the local and remote answers
  can't drift apart by site.

### Negative

- A new cross-session writer: the successor writes a pointer into its
  predecessor's prefix, which needs write access to another workspace's part
  of the bucket.
- Pointers outlive their predecessor until each holder is removed, leaving
  small ghost prefixes.
- The budget cap can make one host report `bucket_unchecked` where another
  names keepers.
- An older release sharing the bucket ignores relations under the cloud
  backend and can delete a predecessor.
- Renames of a successor must re-point, and delete paths must skip one more
  object kind.

### Mitigations

- A pointer decides nothing until the holder's own relation is read and
  matched by session id, so a forged or
  stale pointer can only over-keep; the retention guide lists it with koto's
  other cross-session writers (#445).
- A ghost prefix holds one small object per holder and goes when the holder
  does.
- The cap engages only in corners (uncached templates, piles of stale
  pointers), keeps toward safety, and clears on the next run.
- The changelog and the guide name the first release that honours relations
  and say every host sharing a bucket needs it.
- The two rename sites that must re-point are named in Phase 2, and the
  whole-bucket test oracle runs on a renamed successor, so a missed re-point
  fails a test rather than a prune.
