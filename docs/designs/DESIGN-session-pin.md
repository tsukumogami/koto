---
schema: design/v1
status: Proposed
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
  session's pin object in the bucket. `koto session pin` and `unpin` write
  strictly to the bucket. Release authority is `release-pin`, handed in
  `KOTO_AUTHORITY`, recorded in the anchor by the start hook through
  `koto session anchor open --authority`, and read from the caller's anchor.
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
decision_provenance: inline-resolved
user_visible_surface: true
---

# DESIGN: Pin a finished session so no removal rule takes it

## Status

Proposed

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
pins (R28); writes only into the named session or the starting agent
session's own anchor, and `koto context add`/`remove` refusing every new key
(R29); and everything with no niwa (R27).

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
- **Authority can't be self-granted mid-session.** It's read from what a
  start recorded, never from the caller's environment at unpin (R8).
- **Unpinned output unchanged.** Every existing surface prints the same bytes
  for a session with no pin (R13), so new fields appear only on pinned
  entries.
- **Single writer.** Pin, unpin and the release record write only into the
  named session; recording authority writes only into the starting agent
  session's own anchor (R29), as session retention's one-writer rule already
  requires.
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
"authority","reason","released_at","pin":{"reason","set_by","set_at"}}`,
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
judged session's own `PinFact`, and `pinned_members`, the names of members
whose `PinFact` isn't `None`. A new method, `Judgment::keeps_session()`,
answers for the one session: true when `pin` isn't `None` or when the
component verdict is `Keep`. Every caller that decides whether to delete a
particular session uses that method, so a pin keeps its session while the
rest of the component follows the verdict. A pinned session that is running
is alive and keeps its component as any running session does; once finished
it contributes its stop instant like any finished member (R2, R9). Nothing
about the pin enters `keepers`, `unreadable` or the release instant, so an
unpin restarts no clock (R14).

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
`BucketUnchecked { error }`. It is called with the session's lock held,
after any confirmation prompt, immediately before the first delete of that
session. It reads the local `koto/links/pin` from disk (not from the scan),
and, when the backend is the cloud backend, calls a new backend method,
`SessionBackend::bucket_pin(name) -> Result<BucketPin>`, which lists
`<repo_id>/<name>/ctx/koto/links/pin` directly (one request) and, when an
object is there, reads it (one more). The local backend's default answers
`NotApplicable`. A pin in either copy keeps; a listing error or an
unparseable listing is `BucketUnchecked` with `error` `unreachable` or
`unparseable` (R21 to R23). Only `Clear` lets the delete run.

Each removal point calls the guard and maps the result to its own output:

| Removal | Where the guard runs | On a pin or a failed check |
|---|---|---|
| Terminal tick | `terminal_retention`, before `edge_of` | keeps; `retention.reason` `pinned` (or the existing reason plus `pin`) |
| Parent sweep | `Sweep::enter`, per child | skips the child; `kept_children` entry with `kept_by_pin` |
| Same-name replace | `predecessor_disposition`, before Release | treats as Keep: move aside |
| `koto workspace prune` | `reclaim`, per member under its lock | lists the member as kept or skipped |
| `koto session prune` | `remove_member`, beside the `chain/parent` re-read | skips, lists, exit as for a skip |
| `koto session cleanup` | before `cleanup` | exit 2 `pinned` (or 3 `bucket_unchecked`) |
| `koto cancel --cleanup` | before the cancel event | exit 2 or 3, writes nothing |

The preview paths (prune preview, workspace prune's plan, the hierarchy
query) read the pin from the scan and, under cloud, call `bucket_pin` for
each session they would list for removal, so a preview already shows a pin
another host set; the guard at the delete is the one that decides.

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

#### Chosen: strict writes for the pin and its release, objects as truth

`koto session pin` writes `koto/links/pin` with writer `koto` through a new
`write_reserved_strict(name, key, value)` in `src/session/links.rs`, which
journals the key as `add_as` does and then calls `ensure_pushed`; any failure
exits 3 with "pin held in the local store only", and the open batch makes the
next `koto session pin`, `unpin`, `koto next` or `koto context add` on that
session retry (R20). `koto session unpin` writes `koto/pin-release`
strictly first and then removes `koto/links/pin` strictly (the object delete
is checked), so an unpin interrupted between the two leaves the pin in force
(R6). Removal of `koto/links/pin` is allowed only from `pin.rs`; the generic
context remove path refuses every `koto/` key already. Because the bucket
check reads the object, a host 2 push of an index that lacks the pin can't
hide it (R22).

`koto session import` adds `koto/links/pin` and `koto/pin-release` to the
keys `import_links.rs` carries verbatim, an unreadable pin included, beside
its rule that rewrites held-by links (R25).

#### Alternatives Considered

- **Merging manifests on push.** Fixes the index race for every key, but it
  changes every context write under cloud and its cost profile (koto#439);
  the pin needs only its own object to be authoritative.
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

#### Chosen: `koto session anchor open --authority`, recorded in the anchor

The hook reads `KOTO_AUTHORITY` and passes it as
`--authority=<value>` to `koto session anchor open`, which validates it
(comma-separated, each name trimmed, lowercase, at most 16 names of at most
64 characters) and writes `koto/anchor/authority` in the anchor it opens or
reuses: `{"v":1,"names":[...],"unknown":[...],"at":"<rfc3339>",
"source":"<start source>"}`, replacing what an earlier start recorded (R8).
With no `--authority`, it writes an empty list, so a start handed nothing
clears what an earlier start was handed. A successor anchor gets only its
own start's list. The hook adds `unset KOTO_AUTHORITY` to its block, so the
session's shells, and any worker or `claude -p` child launched from them,
carry none.

`koto session unpin` resolves the caller: no `KOTO_ANCHOR` (or an empty one)
is by hand; otherwise it reads `koto/anchor/authority` from the named
anchor, open or closed, and allows the release only when `names` holds
`release-pin`. A missing or unreadable anchor, or a missing key, is a refusal
that says the anchor's authority couldn't be read (R7). The process's own
`KOTO_AUTHORITY` is never read by `unpin`. `koto session hierarchy` shows an
anchor's `names` and `unknown` (R16).

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
every delete; the bucket read is meaningful because pin and unpin are strict
and the object, not the index, is the truth (Decision 3); and the only
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
strict writer and remover, the release record, the authority check and the
removal guard. The engine (`src/session/hierarchy/`) reads pins into
`SessionFacts` and exposes them on `Judgment`. Each removal point calls the
guard before its delete and renders the result through `render.rs`. Two new
CLI verbs, `koto session pin` and `koto session unpin`, and a flag,
`koto session list --pinned`, sit in `src/cli/session.rs`. The start hook
passes `KOTO_AUTHORITY` to `koto session anchor open --authority` and unsets
it in the session's shells.

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
- **`src/session/links.rs`**: `write_reserved_strict` and
  `remove_reserved_strict`, used only by `pin.rs`; `check_writer_key` keeps
  refusing agent writes to all of `koto/`, which already covers the new keys
  (R29).
- **`src/session/mod.rs` / `cloud.rs`**: `SessionBackend::bucket_pin(name)
  -> Result<BucketPin>` (default `NotApplicable`); the cloud implementation
  lists the pin key's object and reads it when present, outside the probe
  cache. A `remove_reserved_strict` path deletes the pin object and checks the
  delete.
- **`src/session/hierarchy/facts.rs`**: `SessionFacts.pin`; the pin key is
  read before the link walk and excluded from it.
- **`src/session/hierarchy/judge.rs`**: `Judgment.pin`,
  `Judgment.pinned_members`, `Judgment::keeps_session()`. `MemberKind`,
  `Keeper` and `Verdict` are unchanged.
- **`src/session/hierarchy/render.rs`**: `pin_json(&PinFact, source)` and
  `pin_text`, producing R15's fields and text; `bucket_unchecked_json`; the
  hierarchy query's `pin`, `release`, `pinned_members`, `authorities` and
  `unknown_authorities` blocks.
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
- **`src/cli/session_anchor.rs` / `src/session/anchor.rs`**: `--authority` on
  `open`, writing `koto/anchor/authority`.
- **`src/session/import_links.rs`**: carries `koto/links/pin` and
  `koto/pin-release` verbatim.
- **`plugins/koto-skills/hooks/session-start.sh`**: passes
  `--authority="$KOTO_AUTHORITY"` when set and writes `unset KOTO_AUTHORITY`
  in its block.

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

koto session anchor open ... --authority=<comma list>
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
koto/links/pin          {"v":1,"reason","set_by","set_at"}
koto/pin-release        {"v":1,"released_by","authority","reason","released_at",
                         "pin":{"reason","set_by","set_at"}}
koto/anchor/authority   {"v":1,"names":[...],"unknown":[...],"at","source"}
```

### Data Flow

Pinning: the caller runs `koto session pin`; `pin.rs` validates the reason,
takes the session's lock, checks the session's kind and migration, reads any
existing pin (idempotent or refusal), and writes `koto/links/pin` locally;
under cloud it journals the key and `ensure_pushed` uploads the object and
manifest, or the command exits 3.

Removing: a removal point decides from its scan as today, then for each
session it would delete takes the lock, calls `guard_removal`, which reads
the local pin and lists the bucket's pin object, and deletes only on `Clear`.
Every other result becomes a kept entry with the pin's fields.

Releasing: `koto session unpin` resolves the caller from `KOTO_ANCHOR`; under
an anchor, reads `koto/anchor/authority` from that anchor; on refusal exits 2
with nothing written; on success writes `koto/pin-release` strictly, then
removes `koto/links/pin` strictly.

Authority at start: the hook reads `KOTO_AUTHORITY`, passes
`--authority`, `koto session anchor open` writes `koto/anchor/authority` in
the anchor, and the hook's env block unsets the variable.

## Implementation Approach

### Phase 1: the pin record, its commands and the engine

The record, `koto session pin`/`unpin` (by hand only, local backend), the
release record, `koto session list --pinned`, `SessionFacts.pin`,
`Judgment.pin`/`pinned_members`/`keeps_session()`, the hierarchy query's pin
blocks, and the v0.23.0 compatibility test. Nothing removes differently yet.

Deliverables:
- `src/session/pin.rs`, `links.rs` reserved writers, `facts.rs`, `judge.rs`,
  `render.rs`, `src/cli/session.rs`
- tests for the reason rules, idempotence, unreadable cases and the R13
  golden outputs

### Phase 2: the removal guard at every removal point (local)

`guard_removal` with the local read, wired into the terminal tick, sweep,
replace, both prunes, cleanup and cancel `--cleanup`; `RetentionReason::Pinned`,
prune rule `p`; R15's fields on every surface; the late-pin and concurrency
tests.

Deliverables:
- `src/cli/mod.rs`, `workspace.rs`, `init_entry.rs`, `session_prune.rs`,
  `next_types.rs`

### Phase 3: release authority

`koto session anchor open --authority`, `koto/anchor/authority`, the hook's
`--authority` and `unset KOTO_AUTHORITY`, the caller resolution and authority
check in `unpin`, and the hierarchy query's authority blocks.

Deliverables:
- `src/session/anchor.rs`, `src/cli/session_anchor.rs`, `pin.rs`,
  `plugins/koto-skills/hooks/session-start.sh` and its hook tests

### Phase 4: the cloud backend and import

`bucket_pin` on the cloud backend, the bucket half of the guard and of the
previews, strict pin and unpin writes, `list --pinned` over the bucket, the
import carry, and the two-host stand-in tests including the request count.

Deliverables:
- `src/session/cloud.rs`, `sync.rs`, `mod.rs`, `pin.rs`, `import_links.rs`,
  cloud tests

### Phase 5: documentation and the cost test

The changelog entry, the session-retention guide's pin section, the
unpin-first lines in each guide that uses `koto session cleanup` for
recovery (`docs/guides/cloud-sync-setup.md`, `error-handling.md`,
`cli-usage.md`, `custom-skill-authoring.md`, `batch-workflows.md`), the
koto-user skill's command reference and response shapes, how to pass
`KOTO_AUTHORITY` through a dispatcher, the doc test, and the R28 cost test
with 200 pins.

Deliverables:
- `CHANGELOG.md`, `docs/guides/*`, `plugins/koto-skills/skills/koto-user/references/*`,
  the cost test

## Security Considerations

To be written after the security review.

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

- Under the cloud backend every removal pays one listing per session it
  would delete, and the previews one per session they'd list.
- A host that can't reach the bucket can't remove anything under cloud.
- Pins accumulate until someone releases them.
- An agent can still release by hand by unsetting `KOTO_ANCHOR`.
- Two hosts racing a pin and a removal can still leave a pin behind a
  deleted session.

### Mitigations

- The listing is one request and replaces the remote check the prune
  already makes; `in_parallel` keeps a prune's wall time bounded.
- `koto session list --pinned` and the prune reports make pins and failed
  checks visible.
- The release record shows a by-hand release, so a bypass leaves a trace.
- The guard runs immediately before the delete to keep the race window to
  one request.
