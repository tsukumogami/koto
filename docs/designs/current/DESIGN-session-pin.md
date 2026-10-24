---
schema: design/v1
status: Current
problem: |
  koto's retention engine keeps a session only while a member of its
  hierarchy is alive, and every removal point either asks that engine or
  removes regardless; under the cloud backend a second host decides from a
  local copy that may lack keys another host wrote, and several removals
  delete the bucket prefix with no remote check. There is no per-session
  record that says "keep this one", no way for an older build to honour one,
  and no authority koto can read for releasing it.
decision: |
  A pin is one peerless reserved key, `koto/links/pin`, in the pinned
  session, read by the engine as a per-session fact rather than a link or a
  component keeper, so it keeps exactly its own session and older builds read
  it as an unreadable link key and keep. Every removal point goes through one
  removal guard, called under the per-session lock immediately before the
  delete, that reads the local pin and, under the cloud backend, lists the
  session's pin object in the bucket. `koto session pin` and `unpin`, the
  one named exception to the one-writer rule, write only the pin and release
  records as standalone objects, checked, with no log event and no other
  object pushed. Release authority is `release-pin`, handed in
  `KOTO_AUTHORITY`, recorded in the anchor by the start hook through
  `koto session anchor open --record-authority`, and read from the caller's
  anchor.
rationale: |
  A peerless key is the only layout that both avoids joining hierarchies and
  makes v0.23 builds keep a pinned session; a per-session overlay on the
  component verdict keeps one record without keeping everything an open
  anchor touched; one guard at the delete is the only place that sees both
  copies before either goes and can't drift between seven removal points; and
  recording authority at the start, where the hook already records niwa's
  reap key, is the only reading that can't be granted by an export
  mid-session.
upstream: docs/prds/PRD-session-pin.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: Pin a finished session so no removal rule takes it

## Status

Current

Implemented in full: pin and unpin, every removal point, the reports and release authority on the local backend, then the cloud backend (R20 to R24), import and the v0.23.0 compatibility check (R25, R26), the documentation and the cost test. koto v0.24.0 shipped the local-backend half with the commands hidden; the first release after it carries the rest.

## Context and Problem Statement

koto's session retention is built around one engine,
`src/session/hierarchy.rs` with `facts.rs`, `graph.rs`, `judge.rs` and
`render.rs` under `src/session/hierarchy/`. `Graph::scan` reads the header,
the reserved `koto/` keys and the links of every session in the local store;
`Judge::judge(name, scope)` takes the connected component of `name`, and
returns one `Verdict` for the whole component: `Keep { keepers, retention,
unreadable }` as soon as any present member is alive or anything is
unreadable, else `Release` once the retention has passed since the latest
stop instant. Links live under `koto/links/<kind>/<peer>` (`dispatched-by`,
`held-by`, `closes`), written only by koto (`check_writer_key` refuses an
agent write to `koto/`).

The PRD asks for a record that keeps exactly one session, with no release
instant, that every removal honours and reports, that survives the cloud
backend's cross-host gaps, and that only an authorized caller removes. Four
things in the current code stand in the way.

**The verdict is per component.** A pin modelled as a live `Keeper` would
keep every member of its component for as long as it exists. The PRD's R9
requires the opposite: a pin keeps the pinned session and nothing else, and
every other member is judged by the existing rule with the pinned session
counted as a finished member with its own stop instant. Likewise an
unreadable pin keeps only its own session (R18), while today any member's
`problems` taint the whole component.

**Removal points are scattered and two of them don't ask.** The terminal
tick (`terminal_retention` in `src/cli/mod.rs`) asks only `edge_of`; the
parent sweep (`sweep_terminal_descendants` in `src/cli/workspace.rs`) asks
nothing; the same-name replace (`predecessor_disposition` in
`src/cli/init_entry.rs`) and both prunes ask the engine; `koto session
cleanup` and `koto cancel --cleanup` always remove (R11 now requires them to
refuse a pinned session with exit 2). `koto workspace prune`'s `reclaim`
re-judges with the graph scanned before its confirmation prompt, so a pin
set while it waits is missed (R12 requires reading the pin after any prompt
and immediately before the first delete, under the same per-session lock pin
and unpin take).

**The cloud backend decides from local copies.** `Graph::scan` reads only
the local sessions directory; a context key written on host 1 reaches host 2
only when host 2 reads that key; `CloudBackend::cleanup` removes the local
copy and then best-effort deletes the whole bucket prefix; `koto session
cleanup` on host 2 deletes a bucket session host 2 never pulled; and a host
2 write to a stale copy pushes its whole manifest and log, so the bucket's
index can lose a pin entry while the pin object stays. Context writes whose
push fails only warn. The PRD requires: pin and unpin succeed only once the
bucket holds (or no longer holds) the pin (R20); every removal makes a bucket
check before deleting either copy and keeps on a pin found locally or in the
bucket (R21); the check finds the pin by its own object, never only from the
index, and no context write removes a pin object (R22); a failed check keeps
and is reported as `bucket_unchecked` (R23); and keeping depends on the pin
alone, not on anchors held in another host's local store (R24).

**There is no authority to read.** koto has no notion of authority. The PRD
fixes the shape (R7, R8): a caller with no `KOTO_ANCHOR` releases by hand; a
caller under an anchor releases only when that anchor, open or closed,
records `release-pin` among the authorities handed at its agent session's
start through `KOTO_AUTHORITY`; the variable is removed from the session's
shells; a value exported at unpin time is never read; koto never reads niwa.

Two more constraints come from compatibility and import. koto v0.23.0 reads
`koto/links/*` with today's `read_link` rules and must keep a pinned session
rather than release it (R26), and `koto session import` must carry the pin
and its release record (R25); today's import drops every `koto/links/*` key
of an unknown kind.

The PRD's other requirements set the surface: `koto session pin <name>
--reason <text>` with a 500-scalar reason free of Unicode Cc characters,
idempotent on a readable pin and refusing an unreadable one (R1, R3, R4);
pinning a running session (R2); `koto session unpin` writing a release record
before removing the pin, refusing a missing session with exit 2 and changing
nothing when there is no pin (R5, R6); `kept_by_pin`, `pin`, `pin_unreadable`
and `bucket_unchecked` on every kept entry (R15, R23); pins in `koto session
hierarchy` and `koto session list --pinned` (R16, R17); an index-less pin
file counting as readable and an indexed pin with no content as
`missing_content` (R19); no restart of any clock at unpin (R14); unpinned
sessions byte-for-byte unchanged (R13); the retention cost bounds with 200
pins (R28); pin records that move no clock and log no event (R14a); one
named, tracked exception to the one-writer rule, limited to the pin and the
release record (R29, koto#445), and `koto context add`/`remove` refusing
every new key; and everything with no niwa (R27).

## Decision Drivers

- **One session, one record.** A pin must keep only its session (R9, R18)
  and must never become an edge (it can't join hierarchies).
- **Older builds keep.** v0.23.0's link reader must treat a pin as
  unreadable, never as a plain edge or a self-link it drops (R26).
- **No gap between the check and the delete.** The pin is read at the delete,
  after any prompt, under the lock that pin and unpin also take (R12), on both
  copies under cloud (R21), and a failed bucket read keeps (R23).
- **Bucket truth by object.** A host 2 push must not be able to hide a pin
  (R22), and a pin that didn't reach the bucket must fail loudly (R20).
- **Exporting the variable grants nothing.** Authority is read from what a
  start recorded, never from the caller's environment at unpin (R8). Re-running
  the start's own command by hand can still record authority; that is within
  the owner's "format over enforcement" ruling, and it leaves a trace.
- **Unpinned output unchanged.** Every existing surface prints the same bytes
  for a session with no pin (R13), so new fields appear only on pinned
  entries.
- **One named exception, kept narrow.** Pin and unpin write the pin and the
  release record into the named session, the one exception to session
  retention's one-writer rule (R29, koto#445), as standalone records with no
  log event and, under cloud, no object but their own (R14a, R20); recording
  authority writes only into the starting agent session's own anchor, which
  the rule already allows.
- **Bounded cost.** Reading a pin adds one small file read per session to the
  scan locally and one listing per session a removal would delete under cloud
  (R28).
- **Tool independence.** Nothing reads niwa or needs it; niwa changes nothing
  (R27).

## Considered Options

### Decision 1: How a pin is stored and how the engine counts it

Resolved inline.

The pin has to live in the pinned session's own context, under koto's
reserved prefix, so that only koto writes it and import can carry it. The
engine has to read it during the same scan that reads links, and count it in
a way that keeps the pinned session and nothing else.

Key assumptions:

- v0.23.0's `read_link` records a problem (which keeps the component) for a
  key under `koto/links/` that has no `/<peer>` component, and reads an
  unknown kind with a well-formed peer as a plain edge.
- v0.23.0's `read_context` reads a `koto/links/*` key only when the manifest
  lists it, so the pin must be written through the manifest like any key.
- Keys outside `koto/links/` and outside the handful of `koto/` keys the
  engine reads (`koto/anchor/observed`, `koto/imported-from`) are ignored by
  v0.23.0's scan.

#### Chosen: one peerless key `koto/links/pin`, read as a per-session fact

The pin is the single key `koto/links/pin` in the pinned session, beside the
`dispatched-by/`, `held-by/` and `closes/` directories. Its value is one JSON
object, at most 4 KiB: `{"v":1,"reason":"<text>","set_by":"<anchor>"|null,
"set_at":"<rfc3339>"}`. It names no peer, so it can never be an edge, and
v0.23.0 reads it as "isn't a link key", a problem that keeps the session's
component: its prunes, terminal tick (`edge_of` sees an own problem) and
replace keep a pinned session, and the reason names the pin's record (R26).

The release record is a separate key, `koto/pin-release`, outside
`koto/links/` so that no build reads it as a link: `{"v":1,"released_by",
"authority","reason","released_at","pin":{"id","reason","set_by","set_at"}}`,
replaced at each release (R6).

In the new engine, `SessionFacts` gains `pin: PinFact`, one of `None`,
`Present(Pin)` or `Unreadable(PinProblem)` with `PinProblem` one of
`unparseable`, `too_large`, `not_a_file`, `missing_content` (the PRD's four
names). `read_context` reads `koto/links/pin` before the link walk and skips
it there, so a pin is never a `LinkFact` and never one of the session's
`problems`. A non-empty `pin` file the manifest doesn't list is read as a
readable pin (R19); a manifest entry with no file is `missing_content`; a
symlink, directory (`koto/links/pin/...`) or special file is `not_a_file`.

The component verdict is unchanged. `Judgment` gains two fields: `pin`, the
judged session's own `PinFact`, and `pinned_members`, a map keyed by session name from each member
whose `PinFact` isn't `None` to that fact. A new method,
`Judgment::keeps(member)`, answers for one member: true when the member is in
`pinned_members` or when the component verdict is `Keep`. The two questions
stay separate on purpose. A gate that asks "may this component be removed at
all" keeps reading `verdict.is_kept()`; a decision about deleting one
particular member asks `keeps(member)`. Mixing them is the failure to avoid:
asking `keeps` of a component's first member at a component gate would let a
pinned member keep its whole component. The callers, by question:

| Caller | Asks |
|---|---|
| `session_prune.rs` `apply_component` component re-judge | `verdict.is_kept()`, then `keeps(member)` per member |
| `session_prune.rs` planning (`first_rule`, the preview entries) | `keeps(member)` per entry, rule `p` before `a` and `h` |
| `workspace.rs` `judge_subtree`, `reclaim` re-judge | `keeps(member)` per member |
| `init_entry.rs` `predecessor_disposition` | `keeps(predecessor)` |
| `render.rs` reason for one session | `keeps(member)` and the member's pin |

A pinned session that is running is alive and keeps its component as any
running session does; once finished it contributes its stop instant like any
finished member (R2, R9). Nothing about the pin enters `keepers`,
`unreadable` or the release instant, so an unpin restarts no clock (R14).

Around a kept pinned member, the rest of the tree follows the ordinary rule,
in both prunes alike. A pinned child keeps neither its parent nor its own
children: when they're released they go, and the pinned child's
`parent_workflow` then names an absent peer, as it does today after a parent
is removed by `koto session cleanup`. `koto workspace prune`'s rule that a
member goes only after everything below it applies to members kept by the
hierarchy rule, not to a member kept only by its pin, so the two prunes
remove the same sessions. A pin found on an anchor or a close session (one
that arrived by import or the bucket, since `koto session pin` refuses those
kinds) still keeps that session, and `koto session unpin` still releases it.
A pinned child is found and kept by its own name and its own pin, never
through `parent_workflow` lineage, which import doesn't carry and which can
dangle; the sweep's `Sweep::enter` and `reclaim` read the child's pin by name.
Naming a kept child in their output is a read and writes nothing into the
child or its pin.

#### Alternatives Considered

- **A new link kind with a peer, `koto/links/pin/<setter>`.** The setter's
  anchor would become an edge, joining the pinned session to everything that
  anchor held; v0.23.0 would read it as a plain edge and release the session
  once the hierarchy was released.
- **A self-peer link, `koto/links/pin/<self>`.** `write_link` refuses a
  self-link and `build_edges` drops self edges, so v0.23.0 would ignore it and
  release.
- **A pin as a `Keeper` of the component.** Keeps the whole hierarchy with no
  release instant, against R9.
- **A key outside `koto/links/`, such as `koto/pin`.** v0.23.0 ignores keys it
  doesn't know there, so it would release a pinned session.
- **A header field.** Rewrites line 1 of the log and doesn't travel through
  import's key copy.

### Decision 2: Where the pin is checked before a delete

Resolved inline.

Seven commands delete sessions, through `backend.cleanup(name)` or the
prune's strict remote delete, and they decide at different times from
different data. Batch and retry respawns and init rollbacks also call
`cleanup` on children koto just created or is re-running; the PRD leaves
those alone.

Key assumptions:

- Every removal already holds, or can take, the per-session state-file lock
  (`SessionBackend::lock_state_file`) before deleting; the prune's
  `remove_member` already does.
- The cloud listing the delete makes is a fresh ListObjectsV2 call, not the
  per-process probe cache, which caches failures.

#### Chosen: one removal guard called at the delete

A new module, `src/session/pin.rs`, owns the guard:
`pin::guard_removal(backend, name) -> Guard`, where `Guard` is `Clear`,
`Pinned { pin, source }`, `PinUnreadable { problem, source }` or
`BucketUnchecked { error, local }`, `local` being the local copy's pin so
the entry can name it too. It is called with the session's lock held,
after any confirmation prompt, immediately before the first delete of that
session. It reads the local `koto/links/pin` from disk (not from the scan),
and, when the backend is the cloud backend, calls a new backend method,
`SessionBackend::bucket_pin(name, local_pin_id) -> BucketPin`, which lists
`<repo_id>/<name>/ctx/koto/` directly (one request) and, when the pin object
is there, reads it (one more); with no pin object and a local pin, it reads
the release record to see whether it names that pin's `id`. `BucketPin` is
`NotApplicable` (the local backend's default), `Pin(PinFact)`,
`NoPin { released }` or `Unchecked(error)`. A pin in either copy keeps,
unless the local one was released on another host; a listing error or an
unparseable listing is `BucketUnchecked` with `error` `unreachable` or
`unparseable` (R21 to R23). Only `Clear` lets the delete run.

Each removal point calls the guard and maps the result to its own output:

| Removal | Where the guard runs | On a pin or a failed check |
|---|---|---|
| Terminal tick | `terminal_retention`: local pin read always (for the report); the full guard only when removal would otherwise proceed | keeps; `retention.reason` `pinned` (or the existing reason plus `pin`) |
| Parent sweep | `Sweep::enter`, per child | skips the child; `kept_children` entry with `kept_by_pin` |
| Same-name replace | `predecessor_disposition`, before Release | treats as Keep: move aside |
| `koto workspace prune` | `reclaim`, per member under its lock | lists the member as kept or skipped |
| `koto session prune` | `remove_member`, beside the `chain/parent` re-read | skips, lists, exit as for a skip |
| `koto session cleanup` | before `cleanup` | exit 2 `pinned` (or 3 `bucket_unchecked`) |
| `koto cancel --cleanup` | before the cancel event | exit 2 or 3, writes nothing |

The preview paths (prune preview, workspace prune's plan, the hierarchy
query) read the pin from the scan and, under cloud, call `bucket_pin` for
each session they would list for removal, so a preview already shows a pin
another host set; the guard at the delete is the one that decides. A
`koto session prune --apply --plan` whose plan listed a session that has
since been unpinned refuses as it already does when the fresh judgment would
remove something the plan didn't name; a pin set since the plan only removes
less, which the plan check allows.

The guard is reached through one wrapper, `pin::remove_guarded(backend,
name) -> Result<Removed | Kept(Guard)>`, which takes the lock, calls the
guard and then `cleanup`, so a future removal point gets the check by
construction. These deletes deliberately don't go through it, each because it
removes something koto just made or is moving, not a finished session:

| Delete | Why it isn't guarded |
|---|---|
| Batch respawns, `batch.rs` (half-initialised child; reclassified running or skip-marker child) | children koto is driving inside a live run; out of the PRD's scope |
| Retry respawns, `retry.rs` (skip markers, `spawn_failed` children) | the same |
| Init rollbacks, `init_entry.rs`, `init_child.rs` | a session created moments earlier in the same command |
| `delete_remote_checked` in the same-name replace's move-aside | deletes the old prefix only after every object, the pin included, was copied to the new name |
| `koto session prune --migrated-from` | deletes a migrated source, whose pin travelled with the import |

#### Alternatives Considered

- **Inside `SessionBackend::cleanup`.** Batch respawns and init rollbacks also
  call it and the PRD leaves them alone; a refusal from `cleanup` would
  surface as an unexpected error at those sites; and `CloudBackend::cleanup`
  removes the local copy before the bucket copy, so the check would have to be
  split anyway.
- **A check at each call site, written seven times.** Seven copies of the
  local read, the bucket listing and the error mapping drift, which is how the
  parent sweep came to skip the engine today.
- **Trusting the scan and the prune's existing remote check.** The scan is
  stale across `koto workspace prune`'s prompt, and the remote check reports
  "remote copy is ahead" (or nothing, after a host 2 push), never the pin.

### Decision 3: How pin and unpin reach the bucket, and what import carries

Resolved inline.

Under the cloud backend a context write journals its key, uploads the object
and then uploads the whole local manifest; a failed upload only warns. A
strict path exists for the same-name replace: `ensure_pushed`, which
returns every PUT error and keeps the batch open for the next command.

Key assumptions:

- A manifest upload replaces the bucket's manifest but deletes no object; only
  a key removal deletes an object.
- The bucket check of Decision 2 lists objects, so it finds a pin whatever
  the manifest says.

#### Chosen: standalone records, object-only writes, objects as truth

Pin and unpin are the one named exception to session retention's one-writer
rule (PRD R29, tracked in koto#445): they write into the named session, which
can belong to another agent session or host, and only these two records. The
records are standalone. Neither goes through the context write path, so
neither appends an event to the session's log, journals a key, or triggers a
push of the session's state, log, manifest or version object (R14a, R20).

- **Local backend.** `pin.rs` writes `ctx/koto/links/pin` (or
  `ctx/koto/pin-release`) by temp file and rename, under the session's lock,
  and adds or removes the key's entry in the local manifest, also under the
  lock, with writer `koto`. The manifest entry is what lets v0.23.0's reader,
  which reads `koto/links/*` only when the manifest lists it, see the pin and
  keep the session (R26). No event is logged. There is no sync to disturb.
- **Cloud backend.** A new backend method, `put_reserved_object(name, key,
  bytes)`, PUTs exactly the object `<repo_id>/<name>/ctx/<key>` and checks the
  response; `delete_reserved_object(name, key)` deletes exactly one object and
  checks it. Neither touches any other object. The local record (when this
  host holds a copy) is written only after the bucket confirms, so a pin the
  bucket refused exists nowhere, and the command exits 3 (R20); running it
  again retries. Because nothing else is pushed, a host holding a stale copy,
  or no copy, can pin or unpin a session another host owns without replacing
  that host's log or index.
- **Local copies under cloud stay out of sync (R20a).** On a cloud host that
  holds a local copy, the local pin or release record is written as a bare
  file with no manifest entry, so `cloud-manifest.json` and `cloud-sync.json`
  see no change, nothing is marked dirty or ahead, and no push follows. The
  reader counts an unindexed pin file (R19), so this host still sees it. A
  later whole push from this host uploads a manifest without a pin entry,
  which removes no object (Decision 2's bucket check reads the object), and a
  pull leaves the bare files alone. On a cloud host, v0.23.0 can't see that
  local pin, as it can't see a pin another host set; the guide's minimum
  release covers it (R26).
- **A backend switch.** A session pinned under the local backend carries an
  indexed pin. On its first push under the cloud backend (the adopt path
  koto#437 added), `pin.rs` uploads the pin and release record with
  `put_reserved_object` and drops their local index entries, leaving bare
  files under R20a. Until that push the guard keeps the session from the
  local pin.
- **The pin's `id`.** Each pin carries a random 128-bit `id`, set when it's
  written; the release record copies it as `pin.id`. Comparing ids rather
  than `set_at` keeps a pin set, released and set again within one second
  from reading as released.
- **Unpin order.** The release record is written first and the pin removed
  second, so an unpin interrupted between the two leaves the pin in force
  (R6).
- **Bucket truth.** Under the cloud backend the guard of Decision 2 decides
  from the bucket's objects: a pin object keeps the session whether or not the
  local copy has it, and a local pin file whose `id` equals the `pin.id`
  of the bucket's release record was released on another host and no longer
  keeps (R22); the guard then deletes it with the session. A whole pull
  (`koto session resolve --keep remote`) downloads the pin object even when
  the bucket's index lacks it, and the reader counts an unindexed pin file
  (R19), so host 1 sees a pin host 2 set.

Removal of `koto/links/pin` is allowed only from `pin.rs`; the generic context
remove path refuses every `koto/` key already. Because the bucket check reads
the object, a host 2 push of an index that lacks the pin can't hide it (R22).

`koto session import` adds `koto/links/pin` and `koto/pin-release` to the
keys `import_links.rs` carries verbatim, an unreadable pin included, beside
its rule that rewrites held-by links, and nothing else of this feature: an
anchor's `koto/anchor/authority` is dropped, and the read spool, request store
and run journal stay on their host as they do today (R25). The migrated
source's pin is inert: the engine already treats a migrated source as keeping
nothing, `koto session unpin` refuses it with exit 2, and `koto session prune
--migrated-from` removes the source with its pin, which is why that delete is
in the not-guarded table of Decision 2.

#### Alternatives Considered

- **Merging manifests on push.** Fixes the index race for every key, but it
  changes every context write under cloud and its cost profile (koto#439);
  the pin needs only its own object to be authoritative.
- **Writing the pin through the context path (`ensure_pushed`).** It pushes
  the session's whole manifest and state, so host 2 pinning a session from a
  stale copy would replace host 1's index and log.
- **Recording the pin on the setter's side.** Keeps every write in the
  setter's own session, but the record wouldn't travel with the pinned
  session to another host or through import.
- **Leaving pin writes best-effort, relying on retries.** A pin that silently
  stays local is believed on host 1 and absent on host 2, which is the case
  the feature exists for.

### Decision 4: How release authority is recorded and read

Resolved inline.

The PRD fixes the policy; the design chooses the mechanism. The start hook,
`plugins/koto-skills/hooks/session-start.sh`, already runs `koto session
anchor open` with `--harness-session`, `--source`, `--dispatched-by` and
`--observed-key`, writes `export KOTO_ANCHOR=...` into its block of the
session's `CLAUDE_ENV_FILE`, and writes `unset NIWA_REAP_KEY` there so the
session's shells don't carry niwa's value.

Key assumptions:

- The hook runs before any shell command of the session, so the environment
  it reads is what the session was started with.
- A write into the agent session's own anchor by its own start is allowed
  by session retention's one-writer rule, as the start events and the
  observed marker already are.

#### Chosen: `koto session anchor open --record-authority`, recorded in the anchor

The hook passes `--record-authority` to `koto session anchor open` and leaves
`KOTO_AUTHORITY` in that one command's environment, so the value never
appears in a process listing. With the flag, `open` reads `KOTO_AUTHORITY`
from its own environment, validates it (comma-separated, each name trimmed,
lowercase, at most 16 names of at most 64 characters) and writes
`koto/anchor/authority` in the anchor it opens or reuses:
`{"v":1,"names":[...],"unknown":[...],"at":"<rfc3339>",
"source":"<start source>"}`, replacing what an earlier start recorded (R8).
With the flag and no variable it writes an empty list, so a start handed
nothing clears what an earlier start was handed; when the anchor already
records no authority (an empty list or no record), it writes nothing at all,
so a start with no `KOTO_AUTHORITY` is a no-op for the anchor. The record is a
local file write in the anchor, never a bucket call: the hook opens anchors
with `KOTO_SESSION_*` dropped, and where the anchor is in the cloud store the
record is a bare local file under the same rule as a local pin (R20a), never
uploaded, which is enough because `unpin` reads it on the anchor's own host.
Without the flag `open`
leaves the record alone, so the hook is the only caller that changes it in
normal use. A successor anchor gets only its own start's list. The hook adds
`unset KOTO_AUTHORITY` to its block, so the session's shells, and any worker
or `claude -p` child launched from them, carry none.

`koto session unpin` resolves the caller: no `KOTO_ANCHOR` (or an empty one)
is by hand; otherwise the name must pass the session-name check and resolve
to an anchor-kind session, and `unpin` reads `koto/anchor/authority` from it,
open or closed, and allows the release only when `names` holds
`release-pin`. A missing, unreadable or non-anchor session, or a missing key,
is a refusal that says the anchor's authority couldn't be read (R7). The
process's own `KOTO_AUTHORITY` is never read by `unpin`. The release record
copies the authority record's `at` and `source`, so a reader sees how the
authority arrived. `koto session hierarchy` shows an anchor's `names` and
`unknown` (R16). Import never carries `koto/anchor/authority`, as it already
drops an anchor's observed marker.

What this does and doesn't stop: exporting the variable mid-session grants
nothing, and a child launched from the session's shells inherits nothing. An
agent that runs `koto session anchor open --record-authority` itself with the
variable set, or names another anchor in `KOTO_ANCHOR`, or unsets
`KOTO_ANCHOR` to act by hand, can still release. Each leaves a release record
with its source, and none is stronger than what the owner's ruling asks for.

#### Alternatives Considered

- **Reading `KOTO_AUTHORITY` at unpin time.** An agent could export it in its
  own shell and grant itself the release; a child launched from the shell
  would inherit it.
- **A signed or capability token.** Enforcement stronger than today's, which
  the owner's ruling doesn't ask for; a session can already edit its own
  files.
- **Deciding from the setter's identity or the hierarchy.** Rejected by the
  PRD: koto guessing, and the refusal couldn't name an authority.

## Decision Outcome

The four choices meet in one place each. The pin is a key (Decision 1) that
the engine reads into a per-session fact and that the removal guard
(Decision 2) reads again, from disk and from the bucket, at the moment of
every delete; the bucket read is meaningful because pin and unpin write
their own objects, checked, and the object, not the index, is the truth
(Decision 3); and the only
command that removes a pin checks an authority the start recorded in the
caller's anchor (Decision 4). Because the pin isn't a link or a keeper, the
engine's component verdict, its release instants and every unpinned
session's output are untouched; new fields appear only on a pinned
session's entries. Because the key is peerless under `koto/links/`, a
v0.23.0 host sharing a store or bucket keeps a pinned session in every path
that already asks its engine. And because every removal goes through one
guard, the seven removal points can't disagree about what a pin means.

Cross-validation found no conflicts between the decisions' assumptions:
Decision 2's bucket listing is what makes Decision 3's "object is truth"
hold, Decision 1's peerless key is what both list and read, and Decision 4
writes only into the anchor, which Decision 1's scan reads as anchor state,
not as a link.

## Solution Architecture

### Overview

A new module, `src/session/pin.rs`, holds the pin's types, its reader, its
object-only writer and remover, the release record, the authority check and the
removal guard. The engine (`src/session/hierarchy/`) reads pins into
`SessionFacts` and exposes them on `Judgment`. Each removal point calls the
guard before its delete and renders the result through `render.rs`. Two new
CLI verbs, `koto session pin` and `koto session unpin`, and a flag,
`koto session list --pinned`, sit in `src/cli/session.rs`. The start hook
runs `koto session anchor open --record-authority` with `KOTO_AUTHORITY` in
that command's environment, and unsets the variable in the session's shells.

### Components

- **`src/session/pin.rs`** (new)
  - `Pin { reason, set_by: Option<String>, set_at }`, `PinProblem`,
    `PinFact`, `ReleaseRecord`, `PIN_KEY = "koto/links/pin"`,
    `RELEASE_KEY = "koto/pin-release"`, `AUTHORITY_KEY =
    "koto/anchor/authority"`, `MAX_PIN_BYTES = 4096`, `MAX_REASON = 500`.
  - `validate_reason(&str) -> Result<String>`: trim check, scalar count,
    Unicode Cc refusal.
  - `read_local(base, name) -> PinFact`: `lstat`, size, parse, and the
    manifest cross-check of R19.
  - `pin(backend, name, reason, caller) -> Result<PinOutcome>` and
    `unpin(backend, name, reason, caller) -> Result<UnpinOutcome>`, each under
    `lock_state_file`, each refusing anchors, close records and migrated
    sources (from the header's `kind` and `check_not_migrated`).
  - `caller() -> Caller` (`ByHand` or `Anchor(name)` from `KOTO_ANCHOR`) and
    `authority_of(base, anchor) -> Result<Vec<String>>`.
  - `guard_removal(backend, name) -> Guard`.
- **`src/session/links.rs`**: `write_reserved_record` and
  `remove_reserved_record`, used only by `pin.rs`: a temp-file write or
  removal of one `ctx/koto/` file plus its local manifest entry, under the
  session's lock, logging no event; `check_writer_key` keeps refusing agent
  writes to all of `koto/`, which already covers the new keys (R29).
- **`src/session/mod.rs` / `cloud.rs`**: `SessionBackend::bucket_pin(name,
  local_pin_id) -> BucketPin` (default `NotApplicable`), which lists the
  session's `ctx/koto/` objects and reads the pin object, or the release
  object when it must check a local pin's `id`, outside the probe cache; `put_reserved_object(name, key, bytes)` and
  `delete_reserved_object(name, key)`, each one checked request on exactly
  one object and nothing else (no manifest, log, state or version push).
- **`src/session/hierarchy/facts.rs`**: `SessionFacts.pin`; the pin key is
  read before the link walk and excluded from it.
- **`src/session/hierarchy/judge.rs`**: `Judgment.pin`,
  `Judgment.pinned_members`, `Judgment::keeps(member)`. `MemberKind`,
  `Keeper` and `Verdict` are unchanged.
- **`src/session/hierarchy/render.rs`**: `pin_json(&PinFact, source)` and
  `pin_text`, producing R15's fields and text; `bucket_unchecked_json`; a
  member-aware `reason_for(judgment, member)` beside today's
  `reason_text(&verdict)`, so a pinned member of a released component reads
  as kept by its pin and never as released; the hierarchy query's `pin`,
  `release`, `pinned_members`, `authorities` and `unknown_authorities`
  blocks. Every stored string (reason, `set_by`, `released_by`, authority
  names) reaches output through one function, `display_text`, which escapes
  control, bidirectional, zero-width and line or paragraph separator
  characters, and renders as a labelled field, never as bare prose.
- **Removal points**: `src/cli/mod.rs` (`terminal_retention`, cancel
  `--cleanup`, cleanup), `src/cli/workspace.rs` (`Sweep::enter`, `reclaim`,
  `kept_children`), `src/cli/init_entry.rs` (`predecessor_disposition`),
  `src/cli/session_prune.rs` (a new rule `p`, pinned, checked before rules a
  and h, and the guard in `remove_member`). `RetentionReason` gains `Pinned`.
- **`src/cli/session.rs`**: `handle_pin`, `handle_unpin`, `--pinned` on
  `handle_list`; refusals use the typed `{"error":{"code","message",...}}`
  shape with codes `pinned`, `pin_unreadable`, `invalid_reason`,
  `not_pinnable`, `no_session`, `authority_required`, `bucket_unchecked`,
  `pin_not_pushed`.
- **`src/cli/session_anchor.rs` / `src/session/anchor.rs`**:
  `--record-authority` on `open`, reading `KOTO_AUTHORITY` from the command's
  environment and writing `koto/anchor/authority`.
- **`src/session/import_links.rs`**: carries `koto/links/pin` and
  `koto/pin-release` verbatim, and drops `koto/anchor/authority` beside the
  observed marker it already drops.
- **`plugins/koto-skills/hooks/session-start.sh`**: adds
  `--record-authority` to its `koto session anchor open` call, keeping
  `KOTO_AUTHORITY` in that call's environment, and writes
  `unset KOTO_AUTHORITY` in its block.

### Key Interfaces

```text
koto session pin <name> --reason <text>
  exit 0  {"name", "pin": {"reason","set_by","set_at"}, "store": "local"|{"cloud": "<bucket>"}}
  exit 2  {"error": {"code": "invalid_reason"|"no_session"|"not_pinnable"|"pin_unreadable", ...}}
  exit 3  {"error": {"code": "pin_not_pushed"|"bucket_unchecked", ...}}

koto session unpin <name> [--reason <text>]
  exit 0  {"name", "released": null | {release record}}
  exit 2  {"error": {"code": "no_session"|"invalid_reason"|"authority_required",
                     "authority": "release-pin", "anchor": "<name>", "message"}}
  exit 3  {"error": {"code": "pin_not_pushed"|"bucket_unchecked", ...}}

koto session list --pinned [--json]
  [{"name", "state": "running"|"finished", "pin": {...} | "pin_unreadable": "<problem>",
    "source": "local"|"bucket"}]

koto session anchor open ... --record-authority      (reads KOTO_AUTHORITY)
```

On every kept entry for a pinned session (terminal tick `retention`,
`kept_children`, replace answer, workspace prune `kept`/`skipped`, session
prune entries, refusals): `"kept_by_pin": true` and either
`"pin": {"reason","set_by","set_at","source"}` or
`"pin_unreadable": "<problem>"`; text `pinned: <reason> (set by <anchor or
"hand"> at <time>)` or `pin unreadable: <problem>`. On a failed bucket check:
`"bucket_unchecked": {"error": "unreachable"|"unparseable"}`, text
`kept: bucket not checked for a pin (<error>)`. Entries for unpinned
sessions carry none of these fields.

Stored records (each at most 4 KiB, written with writer `koto`):

```text
koto/links/pin          {"v":1,"id","reason","set_by","set_at"}
koto/pin-release        {"v":1,"released_by","authority","reason","released_at",
                         "pin":{"id","reason","set_by","set_at"}}
koto/anchor/authority   {"v":1,"names":[...],"unknown":[...],"at","source"}
```

### Data Flow

Pinning: the caller runs `koto session pin`; `pin.rs` validates the reason,
takes the session's lock, checks the session's kind and migration, reads any
existing pin (idempotent or refusal; under cloud, from the bucket's
objects), then under cloud PUTs the pin object alone and, once the bucket
confirms, writes the local record if this host holds a copy; on the local
backend it writes the local record. A refused PUT writes nothing and exits 3.
No log event is written and no other object is pushed.

Removing: a removal point decides from its scan as today, then for each
session it would delete calls `remove_guarded`, which takes the lock, calls
`guard_removal` (the local pin, then the bucket's pin object), and deletes
only on `Clear`.
Every other result becomes a kept entry with the pin's fields.

Releasing: `koto session unpin` resolves the caller from `KOTO_ANCHOR`; under
an anchor, reads `koto/anchor/authority` from that anchor; on refusal exits 2
with nothing written; on success writes the release record, then removes the
pin, each as its own object under cloud (then locally) or as its own local
record, with no log event. Reading the anchor's authority is a direct file
read in `pin.rs`, not a `koto context get`, so it logs no read anywhere and
writes no read-spool entry (R14a).

Authority at start: the hook runs `koto session anchor open
--record-authority` with `KOTO_AUTHORITY` in its environment, `open` writes
`koto/anchor/authority` in the anchor, and the hook's env block unsets the
variable.

## Implementation Approach

### Phase 1: the pin record, its commands and the engine

The record, `koto session pin`/`unpin` on the local backend, the release
record, `koto session list --pinned`, `SessionFacts.pin`,
`Judgment.pin`/`pinned_members`/`keeps(member)`, `display_text`, the
hierarchy query's pin blocks, and the import carry (a local behaviour, so a
pin never silently drops on import). Until Phase 3, `unpin` under an anchor
refuses with `authority_required`, failing closed; a call by hand releases.
The new verbs and flag are hidden from `--help` and left out of the
changelog until Phase 4 lands, because until Phase 2 no removal honours a
pin; a release cut in between ships them hidden. A unit test asserts the pin
key fails today's link-key grammar, the rule v0.23.0's reader shares; the
test against the v0.23.0 release binary is its own CI job (Phase 5).

Deliverables:
- `src/session/pin.rs`, `links.rs` reserved writers, `facts.rs`, `judge.rs`,
  `render.rs`, `import_links.rs`, `src/cli/session.rs`
- tests for the reason rules, idempotence, unreadable cases, import and the
  R13 golden outputs

### Phase 2: the removal guard at every removal point (local)

`guard_removal` and `remove_guarded` with the local read, wired into the
terminal tick, sweep, replace, both prunes, cleanup and cancel `--cleanup`
by the caller table in Decision 1; `RetentionReason::Pinned`, prune rule `p`,
`reason_for`; R15's fields on every surface; the late-pin and concurrency
tests, and a test that walks every R15 surface with a reason holding each
escaped character class.

Deliverables:
- `src/cli/mod.rs`, `workspace.rs`, `init_entry.rs`, `session_prune.rs`,
  `next_types.rs`

### Phase 3: release authority

`koto session anchor open --record-authority`, `koto/anchor/authority`, the
hook's `--record-authority` and `unset KOTO_AUTHORITY`, the caller resolution
and authority check in `unpin`, import dropping the authority record, and the
hierarchy query's authority blocks.

Deliverables:
- `src/session/anchor.rs`, `src/cli/session_anchor.rs`, `pin.rs`,
  `plugins/koto-skills/hooks/session-start.sh` and its hook tests

### Phase 4: the cloud backend and import

`bucket_pin` on the cloud backend, the bucket half of the guard and of the
previews, object-only pin and unpin writes (`put_reserved_object`,
`delete_reserved_object`), the stale-local-pin rule, `list --pinned` over the
bucket, and the two-host stand-in tests including the request count, a pin
from a stale copy and from no copy that writes only the pin object, a whole
pull from an index lacking the pin, a list-after-write check against the
stand-in, and a test that no push or pull step deletes the pin object. With
this phase the verbs and flag stop being hidden.

Deliverables:
- `src/session/cloud.rs`, `sync.rs`, `mod.rs`, `pin.rs`, cloud tests

### Phase 5: documentation and the cost test

The changelog entry, the session-retention guide's pin section, the
unpin-first lines in each guide that uses `koto session cleanup` for
recovery (`docs/guides/cloud-sync-setup.md`, `error-handling.md`,
`cli-usage.md`, `custom-skill-authoring.md`, `batch-workflows.md`), the
koto-user skill's command reference and response shapes, how to pass
`KOTO_AUTHORITY` through a dispatcher, the koto-user skill's note that pin
and release text is untrusted data, the doc test, the R28 cost test with 200
pins, and the CI job that runs the v0.23.0 release binary against a pinned
store.

Deliverables:
- `CHANGELOG.md`, `docs/guides/*`, `plugins/koto-skills/skills/koto-user/references/*`,
  the cost test

## Security Considerations

A pin keeps data; it removes nothing and grants nothing, so the main risks
are a pin that is wrongly absent, a pin that can't be released, and stored
text that reaches an operator's terminal. The owner's ruling that this
feature records and tracks, and doesn't enforce beyond today's level, sets
the threat model. A process running as the same user can already edit
anything under `~/.koto` directly, and the design doesn't try to stop that.
It aims to stop accidents, cross-host gaps, and an agent that misreads its
own authority.

**Stored text from outside the session.** The reason is checked when
`koto session pin` runs (500 scalars, no Unicode Cc characters), but pin,
release and authority records also arrive from the bucket, from other koto
builds and from imported sessions, none of which pass through that check.
The reader applies the 4 KiB cap and strict parsing to every such record
wherever it came from, and treats an unknown record version as an
unreadable pin, which keeps the session. Every text renderer escapes control
characters, bidirectional overrides, zero-width characters and line and
paragraph separators in the reason, `set_by`, `released_by` and authority
names before printing, through one display function that every surface
uses, with a test that walks every surface; JSON output keeps the escaped
form. Command output therefore can't carry terminal escape sequences from a
stored record. Stored text also reaches agents, which read koto's output: a
forged reason can read like an instruction. Every surface prints it as a
labelled, quoted field, and the koto-user skill tells agents that pin and
release text is untrusted data, never an instruction.

**The bucket as untrusted input.** The removal guard finds a pin by listing
the exact object key `ctx/koto/links/pin` and reads it with a hard limit of
4 KiB plus one byte, so a forged oversized object can't exhaust memory. Keys
that only share the prefix (`pin-old`) don't count, while a `pin/...` entry
reads as `not_a_file` and keeps the session. A listing error, a truncated
listing or an unparseable response keeps the session and reports
`bucket_unchecked`; an unparseable object keeps it and reports
`pin_unreadable`. Someone with write access to the bucket can add a pin to
any session, delete a real one, or add `release-pin` to a synced anchor's
authority record. That party can already delete or overwrite the sessions
themselves, so most of this adds no new power; the new one is making a
session unremovable with a forged pin, which is why `koto session unpin`
works on a session that exists only in the bucket. The guard trusts a
listing to show an object written just before it; the S3-compatible stores
koto targets list with read-after-write consistency, and a list-after-write
test against the stand-in checks it. A test also checks that no push or pull
step deletes the pin object, the assumption the index race relies on.

**Release authority.** `koto session unpin` reads authority only from the
record a session start wrote into the caller's anchor, never from the
`KOTO_AUTHORITY` in its own environment, so exporting the variable
mid-session grants nothing. The name in `KOTO_ANCHOR` is validated as a
session name and must resolve to an anchor-kind session before its record is
read, so the variable can't point the check at another path or at an
ordinary session. Import carries pin and release records but never an
authority record. The value reaches `koto session anchor open` through that
command's environment, not its arguments, so it doesn't appear in an
ordinary process listing (a same-user `ps eww` still shows it), and the hook
never writes it to the session's env file. The release
record stores the authority record's `at` and `source`. Four bypasses remain
by design, and the guides name them: unsetting `KOTO_ANCHOR` and releasing by
hand, naming another anchor in `KOTO_ANCHOR` (the hierarchy output shows
which anchors hold `release-pin`), running `koto session anchor open
--record-authority` by hand with the variable set, and editing files under
`~/.koto`. Each release writes a release record, so the trail shows that one
happened, though not always who really did it. The authority is a convention
that the start hook carries, not a security boundary. The hook unsets
`KOTO_AUTHORITY` for the session's shells; processes the harness starts by
other routes may still inherit it.

**Resource use.** A pin is at most 4 KiB, with one replaced release record per
session, so storage grows with the number of pinned sessions and no faster.
Pinning needs no authority, so any agent, including one following injected
instructions, can pin sessions and keep them from every removal: a retention
bypass, not only a storage cost. `koto session list --pinned` makes that
visible, the retention guide tells operators to review it, and a person can
release any pin by hand. Under the cloud backend each removal and
each preview costs one listing per session it would delete, plus one read
when a pin exists, and the cost test with 200 pins bounds the local side.

**Races.** Pin, unpin and the removal guard share the per-session lock on one
host. Between hosts a window remains between the guard's bucket listing and
the delete, and a pin written inside it is lost with the session. The guard
runs immediately before the first delete to keep that window to one request.
An unpin interrupted after the release record and before the pin removal
leaves the pin in force.

**Data exposure.** Under the cloud backend the reason, the anchor name that
set the pin, the timestamps and, on release, the release record (including
the earlier pin's reason) are uploaded to the user's own bucket and stay
there after unpin. The guides tell users not to put secrets or customer
details in a reason. Authority names are labels, not credentials.

**Fetching the v0.23.0 binary in a test.** The compatibility test downloads a
released binary and runs it. It verifies a SHA-256 digest checked into the
repository before executing, downloads over HTTPS into its own empty
directory, runs the binary with a scrubbed environment and an isolated
sessions directory, and fails when the digest doesn't match. It runs only
when its CI job enables it, so ordinary test runs make no network fetch.

## Consequences

### Positive

- A record someone needs is kept by one command on every host, and every
  removal says why.
- The engine's verdict, its reasons and every unpinned session's output stay
  as they are.
- v0.23.0 hosts keep a pinned session in every path their engine already
  covers.
- One guard decides every delete, so a future removal point gets pin
  handling by calling it.

### Negative

- **Cost.** Under the cloud backend every removal pays one more listing per
  session it would delete, beside any remote check it already made, and the
  previews one per session they'd list.
- **No bucket, no removal.** A host that can't reach the bucket can't remove
  anything under cloud.
- **Accumulation.** Pins accumulate until someone releases them, and any
  agent can add them.
- **Bypass.** An agent can still release by hand by unsetting `KOTO_ANCHOR`,
  or record authority by running the start's command itself.
- **Cross-host race.** Two hosts racing a pin and a removal can still leave a
  pin behind a deleted session.
- **Older builds.** On a shared store or bucket, a host running koto from
  before this feature deletes a pinned session through `koto session
  cleanup` and `koto cancel --cleanup`, and v0.22 or older also through the
  terminal tick and replace; v0.23.0's prunes, terminal tick and replace keep
  it as unreadable.
- **Respawned batch children.** A pin on a batch child is lost when koto
  respawns or rolls back that child inside its run.

### Mitigations

- **Cost:** the guard's listing is one request per session a removal would
  delete, plus one read per pin found. It doesn't replace the prune's own
  remote check (rule g), which still lists the session's prefix, so a prune
  makes two listings per session it deletes; both run one session at a
  time, so a prune's wall time grows with the number of sessions it deletes.
- **No bucket, no removal:** the kept entries say `bucket_unchecked`, so the
  operator sees why nothing went.
- **Accumulation:** `koto session list --pinned` and the prune reports make
  pins visible, and a person can release any pin by hand.
- **Bypass:** the release record shows a by-hand release and the authority
  record's source, so a bypass leaves a trace.
- **Cross-host race:** the guard runs immediately before the delete, keeping
  the window to one request.
- **Older builds:** the changelog and retention guide name the first release
  that honours pins in full and say every host sharing a store or bucket
  needs it.
- **Respawned batch children:** the guide says to pin a batch's parent
  session, which koto doesn't respawn, rather than a child.
