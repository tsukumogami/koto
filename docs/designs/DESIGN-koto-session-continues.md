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
`<Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>`, whose body is the exact bytes of
its relation. It is written by a direct, checked cross-prefix PUT, never
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
deletes its own pointer when it is removed; the guard never deletes one.

The guard trusts only the holder's own relation. For each pointer under P it
lists `<Sp>/<S>/ctx/koto/` (a failed listing keeps P as `bucket_unchecked`
naming S); no continues key means a stale pointer, ignored; a key over 4 KiB is
`too_large`; otherwise it GETs the relation, and a recorded session id equal to
P's makes S a later member, the same name with another id is `mismatch`, and
anything else is ignored. A forged pointer can only send the guard to read a
relation the holder really holds, so forgery can only over-keep.

Standing costs three requests per member: the `ctx/koto/` listing (pin, the
member's relation and its own pointers), the relation GET, and a GET of the pin
when listed (standing `pin`, which takes precedence) or else of the state file
(alive, cancelled, stop instant, through a new standing-from-bytes function
that applies `inventory.rs`'s rules, with the template from the local cache or
one `template.json` GET per hash per run). A reached member counts as migrated
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
ones. It counts its requests against 2 for P plus 3 per member reached; a read
that would pass the allowance stops the walk and keeps P as
`bucket_unchecked` naming the member it couldn't afford. Counts: nothing names
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
