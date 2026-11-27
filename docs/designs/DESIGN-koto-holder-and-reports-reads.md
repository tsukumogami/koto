---
schema: design/v1
status: Proposed
problem: |
  A reader that wants a seat's past and present holders, or the seats that
  report to it, can only get them from koto today by reading every session in
  the store, because koto records each succession and each dispatch only on
  the newer or lower session's side. The cost grows with the store, not with
  the seat, and the holder of a session is recorded as an assistant session
  id with no name.
decision: |
  koto writes a back-reference beside each record it already writes on the
  newer side: a local copy of the cloud's continued-by pointer at succession,
  and a dispatched pointer under the dispatching workflow session at dispatch,
  on both backends, each written by koto inside a call the agent already
  makes and trusted only after the reader checks the successor's or child's
  own link. A rebind records its actor, koto next appends driver_changed on a
  real driver change, and the koto-skills SessionStart hook passes the
  assistant's session title for koto to keep as harness_name on
  anchor_started. Since-stamps tell an old holder from one that dispatched
  nothing. The walk lives in koto_session_reader::chain behind a read-only
  ChainSource, and three verbs, koto session names, chain and reports, serve
  it with one 1024-hop cap and an exit 4 for an unknown name.
rationale: |
  Only a record on the older or dispatching side makes a read follow the seat
  instead of the store, and mirroring the pointer the cloud already verifies
  reuses its key, body and removal rules instead of inventing a new index. An
  index in the dispatcher's context store, a store-wide index, the host-local
  request logs, and keying reports by anchor were rejected: the first takes
  the dispatcher's manifest lock on every child's write, the second exposes
  every workspace's dispatch graph and is never cleaned, the third never
  leaves the host, and the fourth over-attributes reports when one assistant
  session holds several workflow sessions.
  Taking the name from the hook keeps niwa out of the read and leaves the agent
  writing nothing; one walk shared by the verbs and the library stops the two
  from drifting apart.
upstream: docs/briefs/BRIEF-koto-holder-and-reports-reads.md
---

# DESIGN: Holder and Direct-Report Reads Without a Store Scan

## Status

Proposed

## Context and Problem Statement

A seat is the position a dispatcher or a person creates, filled in turn by
koto sessions called its holders. The seat definition decision, built on the
coordinator seat design in tsukumogami/shirabe#1762, treats a seat as a read
model that shirabe's seat reader assembles from facts koto already holds: a
coordinator seat is named `coordinate-<SLUG>`, that name is the lineage, and
koto records facts, never a seat record.

Two of the reads that model needs can't be answered by koto today without
reading every session on the host:

- **A seat's holder history.** Given a seat, the current holder and every
  past holder in order, each named by the coding-assistant session that held
  it and when. A successor records `koto/links/continues/<PRED>` in its own
  context, and a coordinator takeover writes `session_superseded` into the
  predecessor's log. Walking backward from the newest member is a read per
  hop. Walking forward, or finding the members of a seat from its name, isn't:
  `koto session hierarchy` computes `successors` by listing every session and
  reading each one's links, and tsukumogami/koto#662 asks for a command that
  prints a whole chain in either direction without that scan. Apart from that fence and
  its `koto/superseded-by` marker, the only forward pointer koto writes is
  the cloud back-reference under the predecessor's `ctx/koto/continued-by/`,
  which no read verb uses for a walk.
- **A seat's direct reports.** Given a seat, the seats dispatched from it.
  A dispatched session records `koto/links/dispatched-by/<DISPATCHER>` in its
  own context, so finding every session that names a given dispatcher means
  reading every session (tsukumogami/koto#666).

The name the milestone asks for isn't recorded at all. koto reads
`CLAUDE_CODE_SESSION_ID` and keeps the id in its anchors' names and in the
host-wide run journal, but not the session's name (tsukumogami/koto#663). A
holder that changes by rebind leaves an `execution_anchor_rebound` event with
directories and no actor, so a session's holders before the latest are only
in the host-local journal (tsukumogami/koto#664).

The work in scope is what koto records at the two moments it already writes
these links, succession and dispatch, and the reads that use those records,
so that each read costs a function of the seat's own history and reports. Out
of scope: the seat definition itself, how shirabe's paths come to open a
linked successor instead of rebinding (tsukumogami/shirabe#1738, #1739,
#1740), and any change in niwa.

## Decision Drivers

- **Cost follows the seat, not the store.** Each read is bounded by the
  number of holders in the seat's history and the number of its direct
  reports, and stays flat as unrelated sessions are added.
- **The agent writes nothing new and its koto calls cost what they did.**
  Anything recorded is written by koto inside a call the agent or launcher
  already makes, or by the launcher; no new agent call, no new agent-written
  key, and no growth in output an agent reads.
- **No niwa.** A machine with no niwa names every holder, and the reads take
  nothing from niwa's attributes or files.
- **Facts, not a seat record.** koto records facts about sessions (links,
  ids, names, times); the seat stays a read model in shirabe.
- **Both backends.** The local store and the cloud bucket answer the same
  read, and a record written under one survives a push to the other.
- **An unknown seat is an answer.** A name nothing holds gets a message, not
  an empty list and not a long wait.
- **Reuse what exists.** The link layout, the reserved `koto/` area, the cloud
  back-reference pattern, the bucket's name index and the
  koto-session-reader crate are the building blocks, before anything new.

## Considered Options

Six questions shape the design. The first three settle what koto records: at
succession, at dispatch, and when an assistant session starts. The fourth
settles how a caller reads it. The last two are choices the architecture makes
that could have gone another way. None of the first four has been confirmed by
the owner yet, so each lists the assumptions it rests on.

### Decision 1: What koto records at succession and on a rebind

A holder history needs, for each holder, who came before and after it, when it
held the seat, and which assistant session drove it, read in time that follows
the chain from any member or from the seat name, on both backends.

Half of it exists. Every successor holds one `koto/links/continues/<P>` naming
its predecessor by name and session id, with the predecessor's bucket prefix,
the holder's own id and an `at` time (`write_continues` in
`src/session/links.rs`), so walking backward is one read per hop. A
`--supersede` also fences the predecessor with
`session_superseded {by, by_session_id, actor, at}` and writes the create-only
`koto/superseded-by` marker (`src/cli/init_supersede.rs`).

Walking forward is the gap. On the cloud the successor PUTs
`<Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>`, and the bucket never holds a
relation whose pointer wasn't confirmed (`src/session/continues_bucket.rs`), so
one LIST of a member's `ctx/koto/` returns its relation and its successors'
pointers. Locally a plain `--continues` leaves nothing in the predecessor and
`ensure_backref` does nothing, so `koto session hierarchy` and the terminal
tick's `relation_probe` scan the store. Some successions record no relation at
all: `--replace-terminal` moves the old holder to `<name>_r<N>` with no link
(`src/cli/move_aside.rs`), and a coordinator carry writes none before
shirabe#1740. A rebind, one koto session driven by several assistant sessions
in turn, leaves only held-by links (one per anchor, and only when `KOTO_ANCHOR`
is set), the host-local run journal, and an
`execution_anchor_rebound {from?, to}` with no actor (koto#664).

Key assumptions:

- Every process writing to one local store runs this release or later, the rule
  the continues design already sets for shared buckets. If not, a pointer an
  old build skipped reads as "no successor" until the successor's next tick
  under a new build repairs it.
- Coordinator members are named `coordinate-<SLUG>-<YYYYMMDD>T<HHMMSS>Z` and
  root seat members `<name>` and `<name>_r<N>`; worker seats resolve through
  the parent's Holdings row, which shirabe reads.
- A local directory listing that compares names and opens no file stays near
  constant against several thousand sessions. That's estimated, not measured.
- `CLAUDE_CODE_SESSION_ID` is set in the sessions that drive koto. Where it
  isn't, the driver reads as unknown.

#### Chosen: A local mirror of the continued-by back-reference, read name-first and walked both ways, with times and end reasons derived and the driver recorded on a rebind

koto writes the cloud's verified `continued-by/` back-reference on the local store too, with a since-stamp, records who rebound a session and which assistant session drives it, and derives times and end reasons. It wins because only a forward record gives a chain-length read from any member, and it reuses a key, body and verification the cloud already has. Decision Outcome has the mechanism.

#### Alternatives Considered

**A per-chain index keyed by the chain root.** Every successor writes
`chain/<Sp>/<S>/<P>` under the root, so one listing returns the chain. Rejected
because the root becomes a shared cross-prefix write target on every
succession, with its own credential, repair, removal and rename rules, and an
index written only at succession misses the move-aside and carry joins. A
name-first read gets the same parallel latency writing nothing.

**The successor recorded through the predecessor's context store**, as a
`koto/links/continued-by/<S>` link with a manifest entry and an event. Rejected
because fenced logs refuse appends, an event races a live predecessor's ticks,
the cloud form is a conditional pull and push of another workspace's state
(about 6 requests), and every build reads a `koto/links/` key as an undirected
hierarchy edge.

**Existing facts only, read backward from the newest member found by name.**
Kept as the floor, rejected as the whole answer: it fails "from any member"
locally for chains outside a seat and leaves the chain read koto#662 asks for, and `relation_probe`, as
scans. One reviewer held that the pointer could wait for koto#662. It ships now
because the read is asked of every member, and it costs one small file write
inside a call already made.

### Decision 2: What koto records at dispatch, and which dispatcher identity keys it

koto writes `koto/links/dispatched-by/<D>` only in the child, through
`write_link_via`, so listing a dispatcher's children reads every session
(koto#666). The link is written by the child's `koto init --dispatched-by` (via
`init`), `koto session link` (via `link`), the request-leg bind naming
`requested_by` (via `leg`), and the anchor open (via `anchor`), which links
anchor to anchor under the start hook's five-second watchdog. On the cloud each
link write is already pushed at about 5-7 serial requests.

Two facts decide the identity. shirabe's seat reader seats a worker only
through a workflow-level dispatched-by naming a coordinate run whose Holdings
row matches it (shirabe's DESIGN-coordinator-seat-and-role), and a coordinator
seat's holders are workflow sessions. And the main dispatch paths already carry
that link: coordinate's `dispatch-worker.sh` opens a request leg for /scope,
/execute, /deliver and /work-on, and /explore, /decision, /coordinate and
/release workers open with `--dispatched-by <run>`. Only the planning skills,
/work-on given a PLAN, and holdings moved off their leg lack one, and shirabe
already makes those root seats. The continues design (Decision 4 there) settled
the same question for successors with an object beside the predecessor, outside
the manifest, trusted only through the holder's own relation.

Key assumptions:

- A coordinator and its workers normally share a host, so the child reads D's
  prefix (`CloudBackend::home_prefix`) and session id without a request. If
  not, the dispatch pays one `koto-names/<D>/` listing.
- Workers keep running koto commands after they link, so an owed pointer is
  retried within seconds. A child that dies right after a failed PUT stays
  missing.
- Credentials that may write `ctx/koto/continued-by/` under another prefix may
  also write `ctx/koto/dispatched/` there. If not, the PUT gets a 403, the link
  still lands, and the pointer stays owed with a warning.
- Retention releases a dispatcher only with its whole component, children
  included, and shirabe keeps its root-seat rule for unlinked workers.

#### Chosen: A back-reference pointer under the dispatching workflow session, on both backends, verified against the child's own link

The child's own koto call writes a `dispatched/` pointer under the workflow session that dispatched it, on both backends, and the reader trusts a pointer only after checking the child's link. It wins because it puts each report one listing away from its holder, costs one write per dispatch inside a call already made, and can't add a wrong report. Decision Outcome has the mechanism.

#### Alternatives Considered

**An index key appended through the dispatcher's context store.** Rejected
because every child's write takes D's manifest lock, pulls D's log, appends
`context_added` and pushes: 5-7 more serial requests on D's prefix per child,
busy-lock failures against a ticking coordinator, and races between workers.
The continues design rejected the same shape.

**Reading the host-local request logs** (`requested_by`, `RequestLegBound`).
Rejected because they never replicate, finding one requester's requests scans
every request on the host, and they cover only leg dispatch. Kept as a possible
same-host backfill source.

**A store-wide index keyed by dispatcher name**
(`koto-names/_dispatched/<D>/<Cp>/<C>`), read for a whole coordinator seat in
one prefix listing. Rejected because every pushing host must list
`koto-names/`, exposing every workspace's dispatch graph bucket-wide; entries
are never removed and have no completeness without an audit; and the local form
reads every dispatcher's entries on the host.

**Keying by the dispatcher's anchor**, written at anchor open or in `hold()`,
to cover workers with only an anchor-to-anchor link. Rejected because legs
already give the main workers a workflow-level link and the rest are root seats
by shirabe's rule; it adds a write to a hook already over budget,
over-attributes when one assistant session holds several workflow sessions,
picks up `-sN` anchor successors, and takes three or four reads to verify.
Worth reconsidering only if shirabe's seat rule moves to `koto session origin`.

### Decision 3: How the assistant session's name reaches koto

koto keeps no assistant session name (koto#663); readers have been cutting one
out of `execution_dir`, which ties the display to one workspace manager's
layout. Every holder resolves to an assistant session through its held-by link,
written by `hold()` on the agent's `koto init` and `koto next`. The anchor it
names (`anchor-<id>`, successors `-s<n>`) is opened by the koto-skills
SessionStart hook (`plugins/koto-skills/hooks/session-start.sh`), which parses
its payload with jq, hands every harness fact to koto as a flag, and appends an
`anchor_started` on every open.

Claude Code exports no name variable, but its SessionStart hook input carries
`session_title`, the session's custom title or nothing (confirmed in the
2.1.332 binary), which the hook drops today. A background job's name is also in
`$CLAUDE_JOB_DIR/state.json`, read by shirabe's `dc_claude_name`, but that file
exists only for jobs and is undocumented. No hook fires on a rename.

Key assumptions:

- A named job's title is set before its startup SessionStart: in 40 of 40 job
  transcripts checked, the title line came first. No live payload was captured;
  if wrong, the name lands at the job's first resume, compact or clear.
- `session_title` stays in the payload. If it goes, anchors record no name and
  readers show the id. Nothing fails.
- The recorded name is the title Claude Code shows, which won't always match a
  job's `state.json` `name`.

#### Chosen: The hook forwards the payload's session_title, and koto records it as an optional harness_name on every anchor_started

The koto-skills SessionStart hook forwards the payload's `session_title` as `--harness-name`, and koto stores it on `anchor_started`. It wins because the name comes from the assistant itself, with no niwa and no agent write, through a hook that already runs. Decision Outcome has the mechanism.

#### Alternatives Considered

**koto reads `$CLAUDE_JOB_DIR/state.json` in `anchor open`.** Rejected because
it names only jobs, couples koto core to an undocumented file, and reads a host
variable where every other harness fact arrives as a flag.

**The hook reads that file and passes the flag.** Rejected because it's still
job-only and undocumented, and duplicates the payload. It isn't kept as a
fallback, so a holder's name means one thing.

**The dispatcher passes the name at `koto init`.** Rejected because root seats
and interactive sessions have no dispatcher, a niwa-supplied name breaks the
no-niwa rule, resume and rebind never pass through init, and the
dispatcher-origin design rejected putting assistant-session facts on every
workflow session.

**The name in each held-by link body, exported into the agent's shells.**
Rejected because it adds writes to the agent's own calls, freezes the name at
the first hold, and carries free text through a variable any process can set.

**A separate event written only when the name changes.** Rejected because
detecting a change means reading the anchor log inside the hook's five-second
budget to save a few bytes.

### Decision 4: The read surface for a seat's holder history and direct reports

shirabe's `seat-read.sh` is a shell script that reaches koto only through
verbs, with its own exits (1 unreadable or a cycle, 3 past the hop cap). A
fleet view reads through the koto-session-reader crate, which has no network
and a CI guard against writing, so it reads the local store or the cloud
backend's local mirror, never the bucket (DESIGN-koto-shared-session-reader).
`docs/STABILITY.md` promises that crate's listing, log and context reads, not
`koto/links/*`, link bodies or pointer areas. Existing caps are mixed:
`links::MAX_SUPERSEDE_HOPS` is 64, used by `links::newest_member`,
`closes_a_loop` and the decisions chain behind `run_predecessors`, which runs
inside loops; `supersede_chain::MAX_CHAIN_HOPS` and shirabe's resolver cap are
1024. `session origin` and `session hierarchy` exit 2 for both a missing and a
malformed name.

Key assumptions:

- The fleet view is a Rust program reading its own host's store; for another
  host's latest writes it calls the verbs with `--json`.
- A per-verb exit 4 for an unknown name is acceptable (`session prune` already
  gives 4 its own meaning).
- shirabe filters prefix matches by its own name rule and orders unlinked
  stretches.

#### Chosen: Both, with one implementation: the crate owns the walk, three new koto verbs call it over a bucket-backed source, and the 64-hop cap is left alone

The walk lives once in `koto_session_reader::chain` behind a read-only `ChainSource`, and three koto verbs (`session names`, `chain`, `reports`) call it over the bucket. It wins because shirabe gets a command and a second program gets a library without two walks drifting apart. Decision Outcome has the mechanism.

#### Alternatives Considered

**koto verbs only.** Rejected because the fleet view would pay a process start
and store setup per seat on each refresh, and any later crate-side link read
would re-derive the walk with no promised layout.

**Crate calls only.** Rejected because `seat-read.sh` can't call them without
shirabe shipping a binary, and the crate can't answer from the bucket, which
fails "both backends" on any host that isn't the writer.

**Extending `session hierarchy`, `decisions chain` or `session origin`.**
Rejected because it grows output agents and scripts already read, and
`hierarchy` is a retention judge built on a scan while `decisions chain` is
scoped to decisions under the 64-hop cap.

**A generic edge walker** (`koto session links --kind --direction`). Rejected
because every caller would redo verification, ordering, times and end reasons,
and would drift on what "ended" means.

**Raising `MAX_SUPERSEDE_HOPS` to 1024 everywhere now.** Rejected because the
decision-chain callers run it inside loops, so their cost grows with the square
of chain length.

### Decision 5: How the generic walk gets parallel reads from the bucket

The cloud costs in Decision 4 assume a round of reads goes out together. The
walk lives in a crate with no async runtime and no network, behind a trait
whose calls each read one thing, and koto's S3 client is the synchronous
rust-s3 build, fanned out with `std::thread::scope` in `src/session/cloud.rs`.

#### Chosen: A prefetch hint on the trait, served by a per-call cache in koto's source

`ChainSource` gains a no-op `prefetch` hint that koto's bucket source serves from a per-call cache. It wins because it keeps the trait synchronous and the local source unchanged while a cloud read takes a few parallel rounds. Decision Outcome has the mechanism.

#### Alternatives Considered

**An async trait** would add an async runtime to the crate's allowlist for
one consumer whose client is synchronous. **Batch-only trait methods** double
every method, give the local source nothing, and force the walk into batches
throughout. **koto pre-reading every member's `ctx/koto/`** means koto knows
which reads the walk makes, and the two drift the first time the walk changes.

### Decision 6: How a reader tells "dispatched nothing" from "dispatched before the record"

A holder created before `dispatched/` existed must read `predates_record`, and
a worker that dispatched nothing must read as having no reports. On the cloud,
continued-by needs no such test because the bucket has never held a relation
without its pointer, but every dispatched-by link before this release was
pushed with none.

#### Chosen: A since-stamp per local store and per bucket workspace prefix, written by the first dispatched pointer

The first dispatched pointer writes a since-stamp, once per local store and once per bucket workspace prefix. It wins because it separates an old holder from one that dispatched nothing for the cost of one write per store or prefix. Decision Outcome has the mechanism.

#### Alternatives Considered

**A header field written at every `koto init`** adds a write to every
session the agent opens rather than to dispatches, and says nothing about
whether the children ran a build that writes pointers. **No distinction**
makes an old holder read as having no reports, a wrong answer the seat
definition decision would act on.

## Decision Outcome

koto adds two pointer areas beside sessions, a field in the dispatched-by link,
two event fields, one new event, and three read verbs over a walk in the shared
reader crate. Every new record extends a fact koto already writes at a moment
it already writes it, and every pointer is a hint checked against the
successor's or child's own link, so none can put a wrong session in an answer.

The decisions meet in the walk. Decision 1 gives the chain a forward edge on
both backends plus a name-prefix entry; Decision 2 hangs reports off each
holder the chain returns; Decision 3 names each holder through the held-by link
the chain already follows for drivers; Decision 4 puts that walk in
`koto_session_reader::chain` for both `seat-read.sh` and a fleet view; and
Decisions 5 and 6 keep the cloud read parallel and an old holder's silence from
reading as "no reports". The seat stays in shirabe: it turns a seat name into a
prefix, takes the newest member of the newest stretch as the current holder,
and maps reports to seats through Holdings.

### Decision 1: A local mirror of the continued-by back-reference, read name-first and walked both ways, with times and end reasons derived and the driver recorded on a rebind

The successor's own `koto init --continues` (with or without `--supersede`) or
`koto session link --continues` also writes a pointer on the local backend. The
local branch of `record_continues` (`src/cli/session.rs`) writes the regular
file `<sessions>/<P>/ctx/koto/continued-by/<Sp>/<S>`, with the cloud pointer's
key shape and body. It's written by temp file and rename, with no lock, no
manifest entry and no event in P, and only when P's state file and `ctx/`
exist, so it never recreates a removed predecessor. It sits outside
`koto/links/`, so no scan reads it as a hierarchy edge and no push carries it.
The first such write creates `<sessions>/.continued-by-since`; for a
predecessor created after that stamp, a missing pointer proves it has no
successor.

An owed pointer is repaired at the `ensure_backref` hook every tick already
runs, whose local branch gains the write under its own owed mark. It never
reuses `.continues-pointer`: the key is byte-identical to the cloud one, so
after a switch to the cloud `owed_pointer` would take the local file for a
confirmed bucket pointer. A removed successor drops its pointer, and a renamed
one (move-aside, relocate) rewrites it, as the cloud code does. The read trusts
only the successor's own relation, matched by session id. The cloud side gets
nothing new.

On a rebind, `execution_anchor_rebound` gains `actor` in the `SupersedeActor`
shape `{claude_session, host, user}`, inside the append `koto session rebind`
already makes. And `koto next` appends one `driver_changed {driver, anchor?}`
to the session's own log when `CLAUDE_CODE_SESSION_ID` differs from the last
driver that log records. The baseline is the synced log, not the host-local
sidecar, so a second host agrees. Held-by links stay as they are.

A rebind changes a holder's driver, not the holder: the koto session is the
holder, and its drivers are listed oldest first by each one's `from`. A held-by
link is never released, so it means "this anchor drove the session from `at`",
not "this anchor drives it now". Take a workflow session that one worker stops
driving, a second worker rebinds to its own worktree and resumes, and the
first worker's workspace is then removed. The session keeps both held-by
links. The reader shows the first anchor as an earlier driver of that same
holder, ending where the next driver begins, and the second as its current
driver; it shows the stale anchor neither as a separate holder nor as the
current one. The rebind's `actor` and `at` and the second worker's
`driver_changed` pin the moment of the change; without them, as for a rebind
before this release, the order of the two links' `at` values still puts the
stale anchor first. That the first workspace no longer exists changes nothing,
since the reader never reads a workspace or an `execution_dir`.

The read uses only facts: a name-prefix listing for members, verified hops both
ways, and order from links, not clocks. Members with no link between them
(`_r<N>` reopens, carry gaps) come back as separate stretches for shirabe to
order. Times come from relation and fence `at` values and each member's stop
instant (`inventory::status_of`), and the end reason is derived, never
recorded.

Only a forward record gives chain-length reads from any member locally; without
one, the chain read koto#662 asks for stays a scan. Mirroring the cloud's verified back-reference reuses
its key, body, verification and removal rules, and a lost pointer can make a
read slower or mark it incomplete, never wrong. The stamp replaces a backfill:
older history still reads backward from the newest member, so the existing seat
with over 80 holders reads in full. The name-first entry is the only path
across successions with no relation. The actor closes koto#664 in an existing
append, and `driver_changed` covers rebound seats until shirabe#1738 and #1739
land, where held-by alone misses drivers when `KOTO_ANCHOR` is unset and loses
one that comes back.

### Decision 2: A back-reference pointer under the dispatching workflow session, on both backends, verified against the child's own link

The record is keyed by the dispatching workflow session, the peer of the
child's workflow-level link. koto writes it wherever it writes such a link: in
`record_dispatched_by_via`, which serves every init path and the leg bind, and
in `handle_link`. Nothing is written at anchor open. The child's link body
gains `holder_session_id`, the child's own id, with `v` still 1.

Locally, the child's process writes `<sessions>/<D>/ctx/koto/dispatched/<C>`
create-only by temp file and rename, the way the supersede marker is published:
never through D's context store, no lock on D, no manifest entry. On the cloud
it writes `<Dp>/<D>/ctx/koto/dispatched/<Cp>/<C>` by a direct checked
cross-prefix PUT, the `continued-by` shape, which a pull copies into D's local
cache. The body is `{"v":1,"holder_session_id":"<C id>"}`, under the reserved
`koto/` prefix that `koto context add` and `remove` can't reach.

Pointer first, then link. If the pointer fails, the link is written and pushed
anyway (a dispatched-by link is a retention edge, so it's never held back the
way a continues relation is), an owed mark goes in C's directory, and an ensure
step beside `ensure_backref` retries on C's next command. An orphan pointer
left by a failure between the two is dropped by verification. The child drops
its pointer on removal, and the cloud delete, migrate and relocate filters and
import staging carry `dispatched/` as they do `continued-by/`.

The read accepts a candidate C only when C's own `koto/links/dispatched-by/<H>`
exists, its `session_id` equals H's (by name and `at` window when an older body
has none) and its `holder_session_id` equals the pointer's; past a per-holder
cap a holder reads `unverifiable`.

It's the only option that meets every constraint at once: it rides a call
already made, once per dispatched worker; one key shape serves both backends;
D's manifest, lock and log are untouched; the edge stays under D's access
rules; and a forged pointer can cost a read but never add a report.

### Decision 3: The hook forwards the payload's session_title, and koto records it as an optional harness_name on every anchor_started

`session-start.sh` adds `.session_title` to the jq read it already makes. When
the value is a non-empty string, it cuts it to 256 bytes on a UTF-8 boundary
with its existing `cut_bytes` and passes `--harness-name=<value>` to the
`koto session anchor open` it already runs. No new call, no new dependency, no
change to the watchdog, nothing on stderr. An older koto that refuses the flag
exits 2 with no `anchor=` line, and the hook reopens without it, the fallback
it already uses for `--record-authority` and `--dispatched-by`.

`OpenRequest` and `AnchorStarted` gain `harness_name: Option<String>`, skipped
when absent. koto never refuses an open over the value: it replaces control, format,
line-separator and private-use characters (newlines, escapes, bidi controls)
with U+FFFD, trims
whitespace and keeps at most 128 characters; an empty result records nothing.
The field goes on every `anchor_started`, so each SessionStart (startup,
resume, compact, clear) records the title current then. The hook passes no
`--harness-name` when `KOTO_SKIP_HARNESS_NAME=1` is set in its environment,
for a user who doesn't want titles in a synced log.

A reader follows the holder's held-by link to its anchor family
(`-s2, -s3, ...` by exact name) and shows the newest recorded `harness_name`
beside the assistant session id; a rename with no later SessionStart is never
seen. A family with no `harness_name` (koto predates it, or the session had no
title) shows the id with `"name": null, "name_recorded": false`. A session with
no held-by link falls back to a supersede fence's `actor.claude_session` when
it has one, and is otherwise unknown. The reader never substitutes an
`execution_dir` basename, a branch, anything from niwa, or the host-local run
journal.

It's the only option that names interactive sessions as well as jobs, because
the title arrives in the payload every SessionStart hook gets. The name comes
from the assistant alone, so a machine with no niwa names every holder that has
a title. The write rides the hook's own call, and koto core stays
harness-agnostic, the pattern the dispatcher-origin design set for keeping an
assistant-session fact once on the anchor.

### Decision 4: Both, with one implementation: the crate owns the walk, three new koto verbs call it over a bucket-backed source, and the 64-hop cap is left alone

A new module, `koto_session_reader::chain`, holds the walk, hop verification,
the derivation of times, end reasons and drivers, and the output types, generic
over a read-only `ChainSource` trait. The crate implements it for
`LocalDirSource` through its hardened capped read with no new dependency, so
the write guard still passes; koto implements it in `src/` over the bucket.
koto gains `koto session names --prefix`, `koto session chain` and
`koto session reports`, which take names, never a seat, and print sessions,
links, times and drivers, never a role. A seat read is three calls:
`names --prefix`, `chain <those names>`, `reports <newest> --chain`. An unknown
name exits 4 after one `stat` or one LIST. `koto session chain` is the command
koto#662 asks for.

One `chain::MAX_CHAIN_HOPS = 1024`, imported by `engine::supersede_chain`, caps
each stretch; at the cap the stretch says `stop: "hop_cap"` and exits 0, never
truncating silently. `links::MAX_SUPERSEDE_HOPS = 64` stays, because its
decision-chain callers run inside loops. STABILITY.md promises what the crate
reads and the verbs' JSON at `v: 1`.

A shell script can't link a crate and the crate can't reach the bucket, so
neither surface alone works. With the walk behind a source trait, "both" isn't
two copies, the reason the shared-reader design moved the dashboard's logic
instead of copying it. Holdings, roles and the current-holder choice stay in
shirabe, and new verbs leave every output an agent reads untouched.

### Decision 5: A prefetch hint on the trait, served by a per-call cache in koto's source

`ChainSource` gains `prefetch(&self, round: &[Want])` with a no-op default. The
walk proceeds in rounds and names each round's reads first. `LocalDirSource`
ignores the hint; koto's bucket source fans the round out over
`std::thread::scope` into a cache that lives for one verb call, then serves the
single-item calls from it. A source that ignores the hint gives the same
answer, sequentially.

### Decision 6: A since-stamp per local store and per bucket workspace prefix, written by the first dispatched pointer

The first local `dispatched/` write creates `<sessions>/.dispatched-since`
create-only, as `.continued-by-since` is. On the cloud, a host's first pointer
PUT under a workspace prefix also writes `koto-names/_dispatched-since/<prefix>`
with a conditional create that loses harmlessly to an earlier writer, and the
host notes it has done so. The stamp is kept per workspace prefix, not per
bucket, so one workspace's upgrade doesn't speak for another's, and it is a
lower bound: it says pointers exist from a point on, not that every host
writing that prefix runs a build that writes them. A holder created before the
stamp, or in a store with none, reads `predates_record`; one after it with an
empty area reads `recorded` with `reports: []`.

## Solution Architecture

### Overview

```
writes (koto, inside existing calls)
  successor: koto init --continues | koto session link --continues
      local -> <sessions>/<P>/ctx/koto/continued-by/<Sp>/<S>  (new)
               <sessions>/.continued-by-since                  (first write)
      cloud -> <Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>        (unchanged)
  child: koto init --dispatched-by | koto session link | leg bind
      local -> <sessions>/<D>/ctx/koto/dispatched/<C>          (new)
               <sessions>/.dispatched-since                    (first write)
      cloud -> <Dp>/<D>/ctx/koto/dispatched/<Cp>/<C>          (new)
               koto-names/_dispatched-since/<prefix>           (first per host and prefix)
      both  -> dispatched-by link body gains holder_session_id
  koto session rebind -> execution_anchor_rebound.actor
  koto next           -> driver_changed (only when the driver changed)
  SessionStart hook   -> koto session anchor open --harness-name
                      -> anchor_started.harness_name

reads
  koto session names | chain | reports --+
  fleet view (Rust)  ---------------------+--> koto_session_reader::chain
                                                 |-- LocalDirSource (crate)
                                                 +-- BucketChainSource (koto)
```

### Components

- **`koto_session_reader::chain`** (new). The walk, verification, derivation,
  output types, `ChainSource`, its `LocalDirSource` implementation over
  `fs_read::read_store_file`, and the layout names the writers import
  (`CONTINUED_BY_AREA`, `DISPATCHED_AREA`, `CONTINUED_BY_SINCE`,
  `DISPATCHED_SINCE`, `MAX_CHAIN_HOPS`).
- **`BucketChainSource`** (new, `src/session/chain_source.rs`). `ChainSource`
  over exact-name GETs, `ctx/koto/` and `koto/links/<kind>/` LISTs and
  `koto-names/<prefix>` listings, with the prefetch cache.
- **Read verbs** (new, `src/cli/session_chain.rs`). Argument checks, source
  choice, JSON and text rendering, the exit mapping.
- **Succession writer.** `record_continues` and `ensure_backref` gain local
  branches; removal, move-aside and relocate drop or rewrite the pointer.
- **Dispatch writer.** `record_dispatched_by_via` and `handle_link` write the
  pointer; `LinkBody` carries `holder_session_id`; `deletable_keys`,
  `delete_remote_session_checked`, `delete_migrated_source`, the relocate carry
  and import staging handle `dispatched/`.
- **Driver and name records.** `koto session rebind`, the `koto next` path in
  `src/cli/mod.rs`, `--harness-name` with `OpenRequest` and `AnchorStarted`,
  and the hook.

### Key Interfaces

#### The three verbs

```
koto session names --prefix <P> [--json]
koto session chain <NAME>... [--no-drivers] [--json]
koto session reports <NAME> [--chain] [--json]
```

`names` lists sessions whose names start with P, sorted, reading no header; on
the cloud `complete` reflects `koto-names/_complete`. Each entry carries the
workspace prefix it was found under (`null` locally), because a name match
spans every workspace sharing the bucket and isn't proof of membership. In
`chain`, a member reached only by name and joined to no other member by a link
reads `"linked": false`, so a consumer can filter on the seat's own workspace.

```json
{"v": 1, "prefix": "coordinate-foo-", "store": "bucket", "complete": true,
 "names": [{"name": "coordinate-foo-20261101T090000Z", "prefix": "3f9a0c12d4e5b678"},
           {"name": "coordinate-foo-20261103T141522Z", "prefix": "3f9a0c12d4e5b678"}]}
```

`chain` walks each named member both ways and places each member once.
`joined_by` is `continues`, `supersede` or null; `end.kind` is `superseded`,
`continued`, `finished`, `failed`, `cancelled`, `moved_aside` or `unknown`;
`newest` marks a stretch's last member; `stop` is null, `hop_cap`, `request_budget`,
`broken_hop` or `pointers_predate_record`; a driver's `source` is `held_by`,
`driver_changed`, `rebind` or `fence`. `--no-drivers` skips anchor reads.

```json
{"v": 1, "store": "local", "stretches": [{"complete": true, "stop": null, "members": [
  {"name": "coordinate-foo-20261101T090000Z", "session_id": "...", "joined_by": null,
   "started_at": "2026-11-01T09:00:00Z", "ended_at": "2026-11-03T14:15:22Z",
   "end": {"kind": "superseded", "actor": {"claude_session": "...", "host": "...", "user": "..."}},
   "newest": false,
   "drivers": [{"assistant_session": "8f2c...", "name": "foo coordinator",
                "name_recorded": true, "from": "2026-11-01T09:00:02Z", "source": "held_by"}]},
  {"name": "coordinate-foo-20261103T141522Z", "session_id": "...", "joined_by": "supersede",
   "started_at": "2026-11-03T14:15:22Z", "ended_at": null, "end": null, "newest": true,
   "drivers": [{"assistant_session": "c41a...", "name": null, "name_recorded": false,
                "from": "2026-11-03T14:15:30Z", "source": "held_by"}]}]}]}
```

The text form prints one line per member: name, start, end or `now`, end kind,
and the driver's name or `<id> (no name recorded)`.

`reports` lists `ctx/koto/dispatched/` under NAME, or under every member of its
chain with `--chain`, and prints only verified children. A holder's `state` is
`recorded`, `predates_record`, `unverifiable` or `removed`.

```json
{"v": 1, "session": "coordinate-foo-20261103T141522Z", "store": "bucket", "complete": true,
 "holders": [{"name": "coordinate-foo-20261103T141522Z", "session_id": "...", "state": "recorded",
   "reports": [{"name": "work-on-bar-20261103T150102Z", "session_id": "...",
                "at": "2026-11-03T15:01:02Z", "via": "leg"}]}]}
```

| Exit | Meaning, all three verbs |
|------|--------------------------|
| 0 | Printed, including a `hop_cap` stop and a known session with `reports: []` |
| 2 | Malformed name or prefix, or bad usage |
| 3 | The store, its layout marker or the bucket can't be read |
| 4 | Unknown name: any `chain` or `reports` argument with no session, or a `names` prefix nothing matches |

Exit 4 prints `{"error", "command", "unknown": [...]}` on stderr, the shape
`session origin` uses, naming every miss. On a bucket whose name index isn't
declared complete, a `names` miss adds `"index_complete": false` and says the
miss isn't proof.

#### The crate module

```rust
// koto_session_reader::chain
pub const MAX_CHAIN_HOPS: usize = 1024;
pub enum LinkKind { Continues, DispatchedBy, HeldBy }
pub enum PointerArea { ContinuedBy, Dispatched }
pub enum StoreKind { Local, LocalMirror, Bucket }

pub struct LinkRecord { pub session: String, pub session_id: Option<String>,
    pub at: Option<String>, pub prefix: Option<String>,
    pub holder_session_id: Option<String>, pub via: Option<String> }
pub struct Pointer { pub holder: String, pub prefix: Option<String>, pub holder_session_id: String }
pub struct PointerListing { pub pointers: Vec<Pointer>, pub superseded_by: Option<SupersedeMarker> }

pub trait ChainSource {
    fn store(&self) -> StoreKind;
    fn header(&self, name: &str) -> Result<Option<StateFileHeader>, ReadError>;
    fn link(&self, name: &str, kind: LinkKind, peer: &str) -> Result<Option<LinkRecord>, ReadError>;
    fn link_peers(&self, name: &str, kind: LinkKind) -> Result<Vec<String>, ReadError>;
    fn pointers(&self, name: &str, area: PointerArea) -> Result<PointerListing, ReadError>;
    fn events(&self, name: &str) -> Result<Vec<Event>, ReadError>;
    fn names_with_prefix(&self, prefix: &str) -> Result<NameListing, ReadError>;
    fn stamp(&self, area: PointerArea, prefix: Option<&str>)              // Decision 6;
        -> Result<Option<String>, ReadError>;                             // prefix: bucket only
    fn prefetch(&self, _round: &[Want]) {}                                   // Decision 5
}

pub fn names_with_prefix(src: &dyn ChainSource, prefix: &str) -> Result<NameListing, ReadError>;
pub fn holder_chain(src: &dyn ChainSource, names: &[String], opts: ChainOptions)
    -> Result<ChainRead, ReadError>;            // ChainOptions { drivers: bool, limits: Limits }
pub fn reports(src: &dyn ChainSource, name: &str, scope: ReportScope)
    -> Result<ReportsRead, ReadError>;          // ReportScope::{Session, Chain}

pub struct Limits {                             // defaults shown; tests may lower them
    pub hops: usize,                // 1024 (MAX_CHAIN_HOPS)
    pub pointers_per_holder: usize, // 1000, one LIST page; past it the holder reads `unverifiable`
    pub requests: usize,            // 4096 per verb call
    pub fan_out: usize,             // 32 requests in flight per parallel round
    pub object_bytes: usize,        // 64 KiB per link, pointer or stamp; logs keep the reader's own cap
}

pub enum ReadError {
    UnknownName(Vec<String>),                   // exit 4
    InvalidName,                                // exit 2
    Unreadable { category: String, path: String }, // exit 3
}
```

`pointers` returns one listing of a member's `ctx/koto/`, with the
`koto/superseded-by` marker when present, so "superseded by whom and when"
costs one small read. It accepts a one-segment `<C>` (the local backend's
`dispatched/`) and a two-segment `<Cp>/<C>` (the cloud shape, which a pull
copies into a cloud host's mirror). A result read from a mirror says
`"store": "local_mirror"` and can trail another host's write.

#### The pointers, stamps and link field

| Record | Local backend | Cloud bucket | Body |
|--------|---------------|--------------|------|
| Successor pointer | `<sessions>/<P>/ctx/koto/continued-by/<Sp>/<S>` (new) | `<Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>` (unchanged) | `{"v":1,"holder_session_id":"<S id>"}` |
| Dispatched pointer | `<sessions>/<D>/ctx/koto/dispatched/<C>` | `<Dp>/<D>/ctx/koto/dispatched/<Cp>/<C>` | `{"v":1,"holder_session_id":"<C id>"}` |
| Successor stamp | `<sessions>/.continued-by-since` | not needed | `{"v":1,"at":"<RFC 3339>"}` |
| Dispatched stamp | `<sessions>/.dispatched-since` | `koto-names/_dispatched-since/<prefix>` | `{"v":1,"at":"<RFC 3339>"}` |

None is in a manifest or under `koto/links/`. Local pointers are never pushed;
a store that moves to the cloud owes its cloud pointers until each session's
first cloud write. The dispatched-by link body gains one field:
`{"v":1, "session", "session_id", "workspace", "at", "via", "holder_session_id": "<child id>"}`.

#### The event changes

All are additive under STABILITY.md's `EventPayload` rules: optional fields
skipped when absent, and one new variant older readers take through the
catch-all.

```json
"execution_anchor_rebound": {"from": "...", "to": "...",
  "actor": {"claude_session": "8f2c...", "host": "build-01", "user": "dev"}}
"driver_changed":           {"driver": "c41a...", "anchor": "anchor-c41a..."}
"anchor_started":           {"harness_session": "c41a...", "harness_name": "foo coordinator", "...": "unchanged"}
```

`driver_changed.anchor` is present when `KOTO_ANCHOR` was set. `harness_name`
is stored already cleaned.

### Data Flow

**At succession.** S's `koto init --continues P` writes its own relation as
today. Locally, `record_continues` checks that P's state file and `ctx/` exist,
writes the pointer in P by rename, and creates `.continued-by-since` if
missing; on failure S gets an owed mark that its next tick's `ensure_backref`
clears by writing the pointer. A `--supersede` writes the fence and marker as
today. The cloud path is unchanged.

**At dispatch.** The child's linking call resolves D's id and prefix from D's
header on the host (or, on the cloud and only when that header isn't local, one
`koto-names/<D>/` listing), writes the pointer under D, creates the stamp if
needed, then writes the link with `holder_session_id`.

**At anchor open.** The hook reads `.session_title`, cuts it, and passes
`--harness-name`; koto cleans it onto the `anchor_started` it appends.

**At rebind and next.** `koto session rebind` adds `actor` to its append.
`koto next`, having read the log to derive state, compares
`CLAUDE_CODE_SESSION_ID` with the log's last `driver_changed` and appends one
only when they differ. An unset variable appends nothing.

**How a chain read walks.** Round 1 reads each named member's header and
`ctx/koto/` listing. Each hop backward (the member's continues link) or forward
(a pointer, marker or fence) is accepted only when the other side's own record
carries the matching session id. A member with no forward hop is its stretch's
newest, and the stretch stops with `pointers_predate_record` if that member
predates `.continued-by-since`, `hop_cap` at 1024 hops, or `broken_hop` at a
missing or mismatched peer. Drivers come from held-by peers, `driver_changed`
and rebind actors, names from each anchor family.

**How a reports read walks.** For each holder in scope, list `dispatched/`,
read each candidate's own `dispatched-by/<holder>` link, and keep it when the
id checks pass. A holder before the stamp reads `predates_record`, one past the
cap `unverifiable`, one gone from the store `removed`.

### Read cost

n is the holders in the seat's history, r its direct reports, a the anchors
(assistant sessions) across those holders, m the names matching a prefix, and D
the entries in the local sessions directory.

| Read | Local store | Cloud bucket |
|------|-------------|--------------|
| `names --prefix` | one `readdir` comparing D names, no file opened | ceil(m/1000) LISTs of `koto-names/<P>` + 1 GET of `_complete` |
| `chain`, one member named, `--no-drivers` | about 3 small file reads per holder + 1 log read: O(n) | 2-3 requests per holder, sequential: 2n-3n (160-240 at 80 holders) |
| `chain`, every member named | O(n) | the same requests in about 3 parallel rounds |
| drivers on top | 1 log read per holder + 1 anchor-family read per anchor: O(n + a) | n + a GETs in parallel |
| `reports --chain` | the chain walk + n listings + r link reads: O(n + r) | n LISTs + r GETs of at most 4 KiB, in parallel |
| unknown name | 1 `stat` | 1 LIST of `koto-names/<name>/` (cap 256) |

Neither read depends on store size. `chain` and `reports` open or request
nothing that belongs to a session outside the seat's history and its reports,
and no cloud term grows with the bucket. The only place the store shows is the
local name-first entry: one `readdir` compares D names and opens nothing,
estimated to stay near constant at several thousand entries (Phase 7 measures
it). A chain read started from a known member skips it.

#### Today's cost, measured

Read on 2026-11-27 with koto 0.42.0 on the cloud backend, read-only commands
only, on a host whose local sessions directory held 9,565 session directories.

| Read | Command | Result |
|------|---------|--------|
| Every session | `/usr/bin/time -p koto session list` | real 0.67 s; 9,496 rows (the local list merged with the bucket's delimiter listing) |
| Newest member of a 107-member coordinator seat | `/usr/bin/time -p koto session hierarchy <name> --json` | real 5.78, 6.03, 3.37 s; prints the one-hop predecessor only |
| A live 39-member manager seat | `/usr/bin/time -p koto session hierarchy <name> --json` | real 3.78, 3.19, 3.14 s; one undirected component of 193 sessions: 25 of the seat's 39 members (the rest sit in other components) and 48 sessions with a dispatched-by link to a member, mixed with anchors, closes and held-by peers |

Each `hierarchy` call reads every session's header and `ctx/koto/` entries
(`Graph::scan`). Derived, not measured: the 107-member seat's history today is
one call per hop, about 107 calls at 3-6 s, roughly 6 to 11 minutes, each call
reading all ~9.5k sessions. Its reports take one 3-4 s call whose output the
caller filters down from 193 mixed sessions.

### The owner's constraint

Status comes from koto, agents write nothing new, and their koto calls cost
what they did:

- **Local successor pointer and stamp.** Written by koto in the successor's
  `koto init --continues` or `koto session link --continues`, a call already
  made: one small file per succession, plus the stamp once per store.
- **Owed-pointer repairs**, successor and dispatched. Written by koto at the
  ensure step every tick already runs, only while a mark says one is owed.
  Otherwise the cost is one `stat` of the mark.
- **Rebind actor.** A field in the append `koto session rebind` already makes.
  No extra write.
- **`driver_changed`.** Appended by koto inside `koto next` only on a real
  driver change, the same change the run journal already detects in that call
  for its `driver_seen`. It compares against the log the call already read, so
  it adds no read: one append on a session's first tick after the release, then
  one per real change.
- **Dispatched pointer and link field.** Written by koto in the child's
  `koto init --dispatched-by`, `koto session link` or leg bind: one local file
  or one PUT per dispatched worker, never per tick. The `koto-names/<D>/`
  lookup happens only on the cloud when D's header isn't on the host; the
  bucket stamp is one conditional PUT per host and workspace prefix, once.
- **`harness_name`.** Read from the SessionStart payload and passed by the
  koto-skills hook to the `anchor open` it already runs.
- **Output.** The three verbs are new; `koto init`, `koto next`, `koto status`
  and every other output an agent reads gain no field.

Every write is by koto inside a call already made, by the hook, or by the
launcher. A machine with no niwa names every holder: the name comes from the
assistant's own SessionStart payload through the hook, and no read uses an
`execution_dir` basename or a niwa attribute. A holder whose session has no
title shows its id marked "no name recorded", with or without niwa.

### What the seat definition decision reads

| Fact | Where koto records it | Read through | Cost |
|------|-----------------------|--------------|------|
| Seat name | Nowhere; it's the members' name prefix | `names --prefix` / `names_with_prefix` | one name-only `readdir`; ceil(m/1000) LISTs + 1 GET |
| Succession links | `koto/links/continues/<P>` in each successor; `continued-by/` pointers, fence and marker in each predecessor | `chain` / `holder_chain` | O(n) |
| Dispatcher | `koto/links/dispatched-by/<D>` in the holder | `koto session origin` (unchanged) or `chain` | one listing |
| Direct reports | `dispatched/` pointers under each holder, verified against each child's link | `reports --chain` / `reports` | O(n + r) |
| Holder's assistant session id | `koto/links/held-by/<anchor>`, `driver_changed`, the rebind actor, the fence actor | `chain` drivers | O(n + a) |
| Holder's assistant session name | `anchor_started.harness_name` in the anchor family | `chain` drivers | one family read per anchor, within O(n + a) |
| Holder times, current holder | Derived from relation and fence `at` and each member's stop instant; `newest` marks a stretch's last member | `chain` | within O(n); shirabe takes the newest stretch's newest member |

None needs a scan.

### Open items, answered or deferred

| Open item | Answer or carrier |
|-----------|-------------------|
| Read verbs, library calls, or both | Both, one implementation (Decision 4) |
| What koto records at succession (koto#664) | A local `continued-by` pointer and stamp, the rebind actor, and `driver_changed` (Decision 1) |
| What koto records at dispatch (koto#666) | A `dispatched/` pointer under the dispatching workflow session, `holder_session_id` in the link, and a stamp (Decisions 2 and 6) |
| How the assistant session name reaches koto (koto#663) | The hook passes the payload's `session_title` as `--harness-name`; koto records it on `anchor_started` (Decision 3) |
| Whether koto#662's chain command is this read | Yes: `koto session chain` is it, and closes koto#662 |
| A holder from before the name was recorded | Its assistant session id, `"name_recorded": false`, text `<id> (no name recorded)` |
| Whether a reason a holder ended is recorded | No; it's derived from facts, never recorded |
| The hop limit | New walks share `MAX_CHAIN_HOPS = 1024` with an explicit `hop_cap` stop. The `links::MAX_SUPERSEDE_HOPS` (64) callers `newest_member` and `closes_a_loop`: proposed follow-on, no issue filed yet |
| Successors as new linked sessions | shirabe#1738, #1739, #1740; `driver_changed` covers rebound seats until then |
| Backfill for dispatches and successions before the release | Proposed follow-on, no issue filed yet; until then those read as `predates_record` or `pointers_predate_record` |
| A workflow-level dispatched-by for every worker | Proposed shirabe follow-on, no issue filed yet; until then those workers read as nobody's reports |

## Implementation Approach

This design changes no code itself. Each phase is one koto pull request.
Records come first, because each day they aren't written is history that later
reads as "predates the record". Phase 1 also creates the crate module that
holds the layout names, so every writer imports its key from the place
STABILITY.md will promise. Phases 2 and 4 depend on nothing else.

### Phase 1: The chain module's names and the local successor pointer

Deliverables:
- `koto_session_reader::chain` with the layout constants and
  `MAX_CHAIN_HOPS = 1024`, imported by `engine::supersede_chain`.
- Local branches of `record_continues` and `ensure_backref`, the stamp, the
  local owed mark, and drop or rewrite on removal, move-aside and relocate,
  with tests for a missing predecessor, a forced failure and a backend switch.

### Phase 2: Driver records

Deliverables:
- `execution_anchor_rebound.actor`, the `driver_changed` variant and its
  append, with tests for no change, an unset id, A to B to A, and a second host
  reading the synced log.

### Phase 3: The dispatched pointer

Needs Phase 1. Deliverables:
- `holder_session_id` on dispatched-by bodies; the pointer in
  `record_dispatched_by_via` and `handle_link` on both backends; the owed mark,
  ensure step and stamps.
- Delete, migrate, relocate and import handling of `dispatched/`, and
  `ctx/koto/dispatched/` in the bucket permission statement in
  `docs/guides/session-retention.md`.

### Phase 4: The assistant session name

Deliverables:
- `--harness-name`, `OpenRequest.harness_name`, `AnchorStarted.harness_name`
  and the cleaning rule; the hook change, with `session-start.test.sh` pinning
  `session_title` from a captured payload, covering the older-koto fallback
  and `KOTO_SKIP_HARNESS_NAME=1`;
  the flag in the koto-user command reference.

### Phase 5: The walk in the crate

Needs Phases 1-4 for fixtures. Deliverables:
- `ChainSource`, `LocalDirSource`'s implementation, `names_with_prefix`,
  `holder_chain`, `reports`, `ReadError` and the output types, tested on a
  1,100-member chain, a 107-holder seat with `_r<N>` and carry gaps, a manager
  with three workers and one forged pointer, a worker with no reports, holders
  before each stamp, and an unnamed holder.
- STABILITY.md's store read surface: link keys and fields, both pointer areas,
  both stamps and the event fields, pinned by the layout literals test.

### Phase 6: The verbs and the bucket source

Needs Phase 5. Deliverables:
- `BucketChainSource` with the prefetch cache, and the three verbs with their
  JSON, text and exits; STABILITY.md's CLI output section at `v: 1` and
  `docs/guides/cli-usage.md`. koto#662 closes.

### Phase 7: The evidence run

After a release with Phases 1-6, the evidence Milestone 4 of
ROADMAP-shirabe-status-breadcrumbs asks for, on a store of several thousand
sessions: an 80-plus-holder seat in order with names and the current one
marked; a manager with three workers returning exactly those, and a worker's
seat returning none; both about as fast after several thousand unrelated
sessions are added; a host with no niwa naming every titled holder; an unknown
seat exiting 4 at once. The run also times the local `names --prefix` listing,
and dispatches once with a credential limited to the `dispatched/` and stamp
key patterns and once with a credential lacking them, which must take the 403
warning path with the link still written.

### Follow-on work

Proposed, no issue filed yet:

- **koto: shared cap for the 64-hop O(n) walks.** `links::newest_member` and
  `closes_a_loop` take `MAX_CHAIN_HOPS`; the decision-chain callers stay at 64.
- **koto: backfill for records from before the release.** A same-host pass
  writing pointers from children's dispatched-by links, successors' relations
  and the request logs, each verified as a live write would be.
- **shirabe: a workflow-level `--dispatched-by` in every worker brief**
  (planning skills, /work-on given a PLAN, holdings moved off their leg), and
  the flag folded into the opener's `koto init` in place of a separate
  `koto session link` and its 5-7 cloud requests.
- **koto: `session hierarchy` successors and `relation_probe` read pointers**
  instead of scanning, for predecessors created after the stamp.

Only if evidence asks for them: a local mirror of `koto-names/`, a
UserPromptSubmit capture for renames between starts, shirabe's `dc_claude_name`
reading koto's record, and an anchor-keyed dispatch record if shirabe's seat
rule moves to `koto session origin`.

## Security Considerations

The reads in this design consume objects that other hosts and processes write: pointer keys and bodies, link bodies, synced event logs, the bucket's name index, the stamp objects, and the session title the assistant puts in the SessionStart payload. koto treats all of them as untrusted data. Nothing is executed, and nothing read from the bucket or a log is interpreted as code or as an instruction.

**Pointers are hints, and the owner's own record decides.** A pointer under a holder's context names a candidate and nothing more. The read accepts a successor only through that successor's own continues relation, and a report only through the child's own `dispatched-by` link, matching session ids on both sides and the child's `holder_session_id` against the pointer's. A stale pointer, a reused name with a new id, or an orphan left by a failed write finds no matching record and is dropped. A forged pointer can cost a read some requests but can't put a session in an answer without the matching record in the child's or successor's own prefix. That record is itself a claim by whoever controls that prefix. Someone who can write their own workspace prefix and can read the dispatcher's session id can forge both halves, the same trust the dispatched-by link has always carried, so a report means "this session says it was dispatched by that holder", never "that holder is authorized to act on it". Nothing in these reads grants authority, and shirabe and decision records read them as claims. Where an older link body has no `holder_session_id`, the child is accepted only as weakly verified, matched by name and time window, and is reported that way.

**Input is validated before it is used.** Every key segment and body field that becomes a request, a path or output is checked first: workspace prefixes are 16 lowercase hex digits, names pass the session-name grammar, session ids match the id format, `at` is RFC 3339, and pointer keys have exactly the expected segments (one for the local dispatched area, two for the local continued-by area and both cloud shapes). Pulled copies of pointers pass the same checks before landing in a local mirror. Anything that fails is reported as unreadable and never walked. Names and prefixes printed from bucket data, and the unknown names echoed on exit 4, go through the existing `sanitize` and `display_text` rendering. Error output uses a closed list of reasons and never raw store error text, which can carry endpoints or credential fragments.

**Reads are bounded.** The `Limits` in the crate module set every bound. A pointer listing past 1000 entries per holder isn't walked and reads `unverifiable`. Each verb call has a budget of 4096 requests and at most 32 in flight per parallel round, stops at 1024 hops with an explicit `hop_cap`, and caps every link, pointer and stamp read at 64 KiB. Running out of budget stops with `request_budget` and reports the walk as incomplete, never as empty. A listing that fails for any reason, a permission error included, is unreadable, not "no successor" or "no reports".

**Permissions.** The feature adds one cross-prefix write: the child's create-only PUT, and on removal DELETE, of `ctx/koto/dispatched/<Cp>/<C>` under the dispatcher's prefix, plus a conditional create of `koto-names/_dispatched-since/<prefix>`. This extends the existing `continued-by/` write and the import marker, and bucket policy should grant exactly those key patterns, not all of `ctx/koto/`. Without the permission the PUT gets a 403, the link is still written, the pointer stays owed, and the user sees a warning; a credentials gap never becomes a deletion or a refused dispatch. A holder deletes only the pointer built from its own workspace id, its own name and the dispatcher its own link names, never a key taken from bucket data. When the child must find the dispatcher's prefix through the name index, it uses the listed entry only if the entry's session id matches the id it already holds for the dispatcher and the entry is unambiguous; otherwise it writes nothing and leaves the pointer owed. The area sits under the reserved `koto/` prefix, so `koto context add` and `remove` can't reach it. Locally, the child writes into another session's directory only after validating every segment, opening each path component with `O_NOFOLLOW` from a directory handle so the check and the write can't race, refusing a target directory owned by another OS user, by temp file and rename with mode 0600, and only when the target session's state file and `ctx/` exist. The reader crate stays read-only, and the bucket source issues only GET and LIST.

**Known limits, accepted.**

- Anyone who can write a prefix can hold or hide a report or a successor by writing or removing pointers there, the same exposure as continues pointers and pins. Bucket policy limiting who can write `ctx/koto/` under each workspace prefix is the control.
- A writer to a holder's pointer area can add pointers past the per-holder cap, so the holder reads `unverifiable`, and a writer to `koto-names/_dispatched-since/<prefix>` can create the stamp early, so older holders read `recorded` with no reports. The same bucket write policy controls both, and a consumer reads `recorded` with no reports as "none recorded", never "none dispatched".
- Every control above that rests on bucket policy belongs to whoever operates the bucket. koto's part is to name the exact key patterns in its cloud setup guide and to prove, in Phase 7, that a credential limited to them can dispatch and that one without them takes the warning path.
- A name-prefix read returns every session in the bucket whose name matches, across workspaces. Sessions reached only by name are marked as not linked and carry their workspace prefix; a consumer that needs the seat's own workspace must filter on it and must not treat a name match as proof of membership.
- The `recorded` state of a holder depends on every writer to the store running a build that writes pointers. The stamp tells a reader that pointers exist from a point on, not that every host wrote them, so on a bucket shared with hosts on older builds, `recorded` with no reports can still miss a worker. The stamp is kept per workspace prefix so one workspace's upgrade doesn't speak for another's, and a stamp is only a lower bound on what the writer promised.
- `driver` values come from `CLAUDE_CODE_SESSION_ID`, which any process in the agent's environment can set. koto records them only when they are well-formed session ids, and they are a claim about the starting environment, not an identity.
- An older release sharing the bucket ignores both pointer areas. It can leave orphans, which verification drops, and its writes are invisible to the stamp.
- Pointers survive until their holder is removed and carry the child's name, workspace prefix and session id. koto never deletes a pointer it can't attribute.

**The session title is untrusted display text.** The hook forwards the payload's `session_title`, cut to 256 bytes on a UTF-8 boundary, as one `--harness-name=<value>` argument in `--flag=value` form, so a leading dash isn't read as a flag and nothing passes through a shell expansion. koto never refuses an open over it. It replaces control, format, line-separator and private-use characters with U+FFFD, trims whitespace and keeps at most 128 characters, so a title can't forge an `anchor=` line, carry terminal escapes or flip text direction. Cleaning can't make the words themselves safe: a title can still contain text written to steer a reader, so readers must show it as quoted data and never act on it. The hook test runs a hostile title (quotes, `$(...)`, newlines, escapes, a leading dash) and checks that the open output and the recorded value stay intact.

**What the records reveal.** The title is written to the anchor's event log, and the log is pushed to the bucket with the session, so anyone who can read that prefix can read the title for as long as the session exists. A title can name a customer, a ticket or a person, which until now stayed inside the assistant. koto records it only when the payload carries one, never reads a title from anywhere else, and the hook passes no title when `KOTO_SKIP_HARNESS_NAME=1` is set (Phase 4 builds and tests it). The rebind actor and the drivers record an assistant session id, a hostname and an OS user name in the synced log, the same shape a supersede fence already writes. The dispatched pointer's key holds the child's workspace prefix and name, so anyone who can list a holder's `ctx/koto/` sees the holder's direct reports, including ones from other workspaces; this is narrower than the store-wide index the design rejected, which would have exposed every workspace's dispatch graph. The name index, `koto-names/<name>/<prefix>`, already lists every name and its workspace prefix to anyone who can list the bucket, so the name-prefix read exposes nothing new beyond its linked marker. Pointer and stamp bodies hold times and session ids only, and no local path is copied into another workspace's prefix. Where that visibility matters, scope bucket read access per workspace.

## Consequences

### Positive

- A seat's holder history reads in O(n) and its reports in O(n + r) on both
  backends, from shell or Rust, where the 107-member seat takes an estimated 6
  to 11 minutes of store scans today.
- An unknown name exits 4 after one `stat` or LIST, and a walk at the cap says
  so.
- Every holder is named from the assistant itself, with no niwa and no
  directory parsing; an old or untitled holder is an explicit state.
- A rebound session's drivers live in its own synced log; koto#662, #663, #664
  and #666 close.
- A forged or stale pointer can cost a read but can't add a successor or a
  report.

### Negative

- STABILITY.md now promises the link layout, pointer areas and stamps, so they
  change only additively.
- Each dispatched worker's linking call on the cloud pays one more request, two
  on the cross-host path.
- Records from before the release read as "predates", and earlier driver
  changes aren't recorded.
- An owed pointer reads missing until the child's next command, and for good if
  it never runs one; a local removal of a past holder loses its pointers.
- Workers launched without a workflow-level dispatched-by read as nobody's
  reports.
- A rename is seen only at the next SessionStart, and the name rests on a
  Claude Code payload field that isn't a koto contract.
- A cloud read from one old member is sequential; a fleet view's crate reads
  trail other hosts under the cloud backend.
- `newest_member` and `closes_a_loop` stay wrong past 64 hops until their
  follow-on.

### Mitigations

- The additive rule already governs the read surface, and the layout literals
  test fails CI on a rename.
- The extra PUT rides the call that already pushes the link, never a tick.
- Both "predates" states are explicit, and the proposed backfill covers
  same-host history.
- The ensure step retries on every later command, and a removed holder reads
  `removed`, not empty.
- The proposed shirabe follow-on gives those briefs a workflow-level link.
- The hook test pins `session_title`; if it disappears, the display falls back
  to the id, never an error.
- Naming every member up front makes the cloud read about 3 parallel rounds,
  and each result says which store it came from.
- The new reads don't use the 64-hop cap, and its follow-on is named.
