---
schema: prd/v1
status: Accepted
problem: |
  A developer who runs koto sessions on several machines, all syncing to the
  S3-compatible bucket they configured, can't see another machine's sessions
  from the dashboard in front of them. A session found only in the bucket is
  a state-less placeholder hidden among receded rows, and no session names
  the machine that produced it, because the session's recorded origin holds
  a directory and a store, not a machine.
goals: |
  On any machine pointed at the bucket, `koto dashboard` and
  `koto dashboard --once` list the developer's sessions from every machine,
  each with its current state and the name of the machine that produced it,
  read from the developer's own bucket with the credentials koto already
  uses, and with no change for a developer on one machine.
absorbed:
  - docs/briefs/BRIEF-cross-machine-dashboard.md
---

# PRD: Sessions from every machine in one dashboard

## Status

Accepted

Absorbed [BRIEF-cross-machine-dashboard](docs/briefs/BRIEF-cross-machine-dashboard.md); carried in Absorbed Brief.

## Absorbed Brief

The brief named in Status is folded into this section and its file was
removed when this PRD absorbed it; this section is the brief's framing, and
there is no separate file to look for.

The feature was framed for a developer who runs koto sessions on several
machines and syncs them all to an S3-compatible bucket they configure. The
bucket mechanics it rests on already ship (sync, pins and successors kept
across hosts, reads of a named session from any directory, one log push per
tick), but the dashboard on any one machine shows another machine's session
as a state-less placeholder and names no machine, so the developer logs into
each machine to find the run waiting on them.

The intended outcome: from whichever machine is in front of them, the
developer sees every session they run, with its current state and the
machine it runs on, in the interactive dashboard and as text a script can
watch, read from their own bucket with no managed service, and with no
change for a developer on one machine. The framing's journeys were checking
on a blocked run from another machine, a shell loop reading
`koto dashboard --once`, and a single-machine user who sees nothing new;
they're carried as the User Stories below. The boundary it drew (no hosted
service, no acting on remote sessions, no requests or reviews in the
dashboard, no changes to bucket continuity, no project-level storage
settings, no shared buckets) is carried in Out of Scope. The questions it
deferred (credentials, discovery, how the machine is stamped, the text
columns) are settled in Requirements and Decisions and Trade-offs.

## Problem Statement

Some developers run koto sessions on more than one machine at a time: a
laptop for interactive work, a workstation or a cloud VM for long runs.
koto already syncs each machine's sessions to an S3-compatible bucket the
developer configures in their user-level settings, and the bucket has
become a record the machines share. A pin set on one host keeps the session
on every host, `koto session needed` follows successors and imported copies
through the bucket, and a named session is read and written at its own copy
from any directory.

The dashboard doesn't use any of that. On machine B, `koto dashboard` builds
its list from this machine's session store. A session machine A wrote shows
up only as a placeholder row: no state, no template, idle time counted from
the Unix epoch, so it lands in the receded band and is hidden unless the
developer asks for every session. `koto dashboard --once` prints the same
placeholder with an `unknown` status and a `pending` liveness. And no row,
local or remote, says which machine a session came from. The `origin` a
session records names its anchor directory and the store it writes to,
which locates a copy but doesn't tell a developer "this one is on the build
VM". koto's only notion of a machine today is an eight-digit hash of the
hostname, kept for conflict detection and never shown.

So to learn whether a run on another machine is blocked at a gate, the
developer logs into that machine and runs the dashboard there. The more
machines they use, the likelier it is that the session waiting on them is
the one they can't see.

## Goals

- A developer on any of their machines sees the sessions they run on all of
  them, ordered by the same attention rules the dashboard already applies to
  local sessions, with each session's current state.
- Every session names the machine that produced it, in a form a person
  recognizes, recorded when the session is created rather than inferred when
  it's read.
- The same sessions, states and machines are available as text, so a script
  can watch for a blocked session on any machine.
- None of it needs a service, a new account or new credentials, and a
  developer on one machine with no bucket sees no change.

## User Stories

- As a developer who started a long session on my workstation and moved to my
  laptop, I want the laptop's dashboard to show that session blocked at its
  gate and name the workstation, so that I know it needs me and where to go
  to answer it.
- As a developer with a shell loop that warns me when anything is waiting at
  a gate, I want `koto dashboard --once` on my laptop to list a cloud VM's
  blocked session with its state and machine in parseable columns, so that
  the warning fires and names the VM without my opening the TUI or logging
  into the VM.
- As a developer with two machines that both have a session called `review`,
  I want to see two rows, each naming its machine, so that one never hides or
  overwrites the other.
- As a developer who keeps an old laptop as a read-only window onto my runs,
  I want its dashboard to work with keys that can only list and read the
  bucket, so that the machine watching can't change anything the machines
  working depend on.
- As a developer on a train with a flaky connection, I want the dashboard to
  keep showing my local sessions and tell me the remote ones are unavailable,
  so that a network problem never hides the work in front of me.
- As a developer who uses koto on one machine with the local backend, I want
  the dashboard to look and behave exactly as it does today after I upgrade,
  so that cross-machine reading costs me nothing I didn't ask for.

## Requirements

Terms used below. A **local row** is a session this machine's session store
holds a copy of. A **remote row** is a session read from the bucket that this
machine's store holds no copy of; a local copy and a bucket copy are the same
session when their headers carry the same `session_id`. A **workspace id** is
the 16 lowercase hex digits naming the bucket prefix a session's copy lives
under.

### Producing machine

**R1. A session records the machine that produced it, when it's created.**
Creating a session (`koto init`, a child spawn, an inline init) records a
machine name in the session's own durable record, on every backend. The
name is read when the session is written, not reconstructed when it's
read.

**R2. The machine name defaults to the hostname and the developer can set
it.** The recorded name is, in order: the `KOTO_MACHINE_NAME` environment
variable, else the `machine.name` key in the user-level configuration
(`koto config set --user machine.name <name>`), else the hostname as the
operating system reports it with any domain part (from the first `.`)
removed. An empty value counts as unset. A name is
1 to 64 characters from letters, digits, `.`, `-` and `_`. `koto config set`
refuses any other value; an environment value that breaks the rule is
ignored with one warning on stderr and the next source is used; a hostname
that breaks it has each other character replaced by `-` and is cut to 64
characters.

**R3. The recorded machine survives the operations that rewrite a session.**
A rebind, a cloud push or pull, a resolve, and a header rewrite by any koto
release, including one that predates this feature, keep the recorded
machine unchanged. An import records the importing machine on the new copy,
since that's where the copy was produced and where it runs, and leaves the
source's record as it was.

**R4. The machine is distinct from the existing origin.** The session's
existing `origin` (anchor directory and store identity) keeps its meaning
and shape. The machine is a separate fact; nothing that reads `origin`
today changes behaviour.

**R5. Sessions with no recorded machine read as unknown.** A session created
by a release that predates this feature shows an empty `machine` column in
`--once`, `"machine": null` in `--detail`, and `unknown` in the TUI marker and
Session tab. The dashboard doesn't substitute
the hostname of whichever machine last wrote the session.

### Reading other machines' sessions

**R6. The dashboard lists sessions from every prefix in the bucket.** When
the cloud backend is the selected backend, `koto dashboard` and
`koto dashboard --once` list the sessions under every workspace prefix in
the configured bucket, following every page of a listing. Sessions koto owns
internally (execution anchors, batch internals, and ids containing `~`) stay
filtered out exactly as they are for local rows.

**R7. A remote row shows its current state.** A remote row carries the
same facts a local row does: current state, whether it's blocked, failed or
terminal, its intent, its template name, when it last changed, and the
liveness and attention band derived from those by the rules local rows
already use. A remote session blocked at a gate is in the needs-you band; a
terminal one recedes and is shown only with `--all` or the `a` key, as a
local one is.

**R8. Remote rows are marked in the TUI.** In the interactive list, every
remote row carries a marker after its name holding `@` and the recorded
machine name, or `@unknown` when none is recorded; at widths where the name
cell is truncated, the marker is truncated from its end, never dropped.
Local rows carry no marker. For a remote row, the Session tab shows the
facts the row carries, including Machine and the workspace id, and each of
its other tabs shows one line saying the session's detail lives on its own
machine. For a local row, the Session tab shows a Machine row holding the
recorded name, or `unknown`.

**R9. Rows are distinct per session copy, not per name.** Two sessions with
the same name under different workspace prefixes appear as two rows. A
remote session never replaces, merges into, or nests under a local session
that shares its name; a remote session nests only under its recorded parent
within its own prefix.

**R10. A session isn't listed twice.** A session with a local copy appears
once, as a local row, even though its copy is also in the bucket.

**R11. A migrated source isn't shown as a separate session.** When an import
left a source copy as a read-only predecessor of a copy present in the
bucket or locally, the dashboard lists the copy and doesn't list the source.

**R12. A session whose state isn't published is listed as unknown.** A
remote session whose current state can't be read within R21's bound (for
example one last written by a release that predates this feature, or one
whose published state doesn't parse) is listed with status `unknown`,
counted in the unreadable tally, and never shown with a state it doesn't
have. The unreadable tally is the count the dashboard already reports for
sessions it couldn't read: the tally line in the TUI and the stderr note
from `--once`.

**R13. Removed sessions disappear.** A session removed from the bucket (by
cleanup or prune on its own machine) disappears from a running TUI within
R22's bound and from the next `--once` run.

### Text output

**R14. `koto dashboard --once` appends two columns.** After the eight
existing tab-separated columns (`id`, `current_state`, `elapsed`, `status`,
`intent`, `template_name`, `idle`, `liveness`), each line carries a ninth
column, `machine`, holding the recorded machine name or an empty string,
and a tenth, `workspace`, holding the workspace id of the session's copy, or
an empty string on the local backend. The eight existing columns keep their
positions and meanings. `--status`, `--needs-you` and `--all` apply to
remote rows as to local ones; a session-name argument prints every row with
that name, local and remote.

**R15. Text from the bucket is sanitized.** Every field that came from the
bucket (machine name, session id, state, intent, template name) has each
control character (U+0000 to U+001F, U+007F, U+0080 to U+009F, U+2028 and
U+2029) replaced by a single space, in `--once` and in the TUI.

**R16. `--detail` reports the machine.** `koto dashboard --detail <name>`
includes a `machine` field holding the recorded name, or `null` when none is
recorded, and stays a read that writes nothing. `--detail` resolves local
sessions only, as today.

### Credentials and failure

**R17. No new credentials.** The reading machine uses the bucket, endpoint,
region and credentials it already resolves from user-level configuration
and the environment. No new credential setting, file or service is added.

**R18. The dashboard's remote read writes nothing.** Listing and reading
remote sessions writes no object to the bucket and creates no directory in
this machine's session store. It works with static keys that grant only
`s3:ListBucket` and `s3:GetObject` on the bucket.

**R19. Bucket failures leave local sessions visible.** Every bucket request
the dashboard makes times out after at most 10 seconds. A failure falls in
one of four classes: `unreachable` (the connection is refused or the host
doesn't resolve), `denied` (HTTP 401 or 403), `timeout` (no answer within
the 10 seconds), and `throttled` (HTTP 429 or 503 after koto's usual
retries). When the listing of the bucket's top level fails, the run makes no
further bucket requests: the TUI shows its local rows and one status line
naming the failure class and the endpoint's host, and `--once` prints its
local rows, writes one stderr line with the same two facts, and exits 0.
When some prefixes fail and others don't, rows from the readable ones show,
and the status line (or stderr line) says how many workspaces couldn't be
read and names the class of the first failure. A remote session whose
listing succeeded but whose state read failed is counted in the unreadable
tally. The TUI's status line clears on the first refresh that reads the
bucket without failure. A cloud backend with no bucket or no credentials
configured fails as it does today.

**R20. The local backend is unchanged.** When the local backend is selected,
whatever bucket settings exist, the dashboard makes no network request, the
TUI shows no markers, and `--once` prints the two new columns empty.

### Non-functional

**R21. A refresh's cost doesn't depend on history.** The dashboard never
downloads a remote session's event log. To show one remote row it reads at
most 64 KiB of object data per refresh, and a TUI refresh in which no remote
session changed reads no object data (listing requests aside).

**R22. Remote state is fresh enough to act on.** In the TUI, a remote
session's row reflects the bucket within 60 seconds of the last-modified
time of the object that changed, with up to 200 workspace prefixes in the
bucket. `koto dashboard --once` reads the bucket's current state each time
it runs.

**R23. The TUI never waits on the bucket.** No bucket request runs on the
thread that handles input and rendering. With an endpoint that accepts
connections and never answers, the frame drawn after a key press reflects it
within 200 ms.

**R24. The dashboard holds up at a working developer's scale.** With 200
workspace prefixes and 2,000 sessions in the bucket and every request
answered in 50 ms, `koto dashboard --once` completes within 30 seconds.

## Acceptance Criteria

The first two criteria are the feature's evidence scenarios; the rest pin
the requirements around them. "The fixture template" is a two-state template
whose first state has a gate that fails until a context key `go` exists.
Bucket behaviour is checked against koto's in-process fake bucket, which
records every request.

- [ ] **Evidence scenario, interactive.** Machines A and B configure the
  same bucket with the cloud backend. On A, a session on the fixture template
  is ticked once, stops at the gate, and the tick exits. On B, `koto
  dashboard` shows that session in the needs-you band as blocked in the
  gate's state, and the row's marker reads `@` and A's machine name. (R1,
  R6, R7, R8)
- [ ] **Evidence scenario, text.** In the same setup, `koto dashboard --once`
  on B prints a line for that session whose `current_state`, `status`,
  `intent` and `template_name` columns equal those A's own `--once` prints
  for it, whose `machine` column
  holds A's machine name and whose `workspace` column holds the session's
  prefix. (R6, R7, R14)
- [ ] With `KOTO_MACHINE_NAME=env-name` and `machine.name = "cfg-name"` set
  at user level, a new session records `env-name`; with only the config key,
  `cfg-name`; with neither, the hostname without its domain part; with
  `KOTO_MACHINE_NAME=""`, the config value. A session created by a child
  spawn and one created by an inline init record the same name as one created
  by `koto init`, on the local backend as on the cloud backend. (R1, R2)
- [ ] A 64-character `machine.name` is accepted and a 65-character one is
  refused; a hostname `my host_01.example.com` is recorded as
  `my-host_01`. (R2)
- [ ] `koto config set --user machine.name "bad name!"` exits non-zero and
  changes nothing; `KOTO_MACHINE_NAME="bad name!"` produces one stderr
  warning and the session records the next source's value. (R2)
- [ ] After a rebind, a push and pull round trip, a `koto session resolve
  --keep remote`, and a header rewrite by the release before this feature,
  the session's recorded machine is unchanged. (R3)
- [ ] After `koto session import` of A's session on B, the new copy records
  B and the source still records A. (R3)
- [ ] For a session created by the release before this feature, `--once`
  prints an empty `machine` column, `--detail` prints `"machine": null`, and
  the TUI marker on B reads `@unknown`. The fixture is a session store
  written by a release that predates this feature, kept in the test suite.
  (R5, R8, R16)
- [ ] `--detail` output's `origin` object is byte-identical before and after
  this feature for the same session. (R4)
- [ ] With A's sessions under three different prefixes, B lists all three
  sessions; an anchor, a batch internal and an id containing `~` under those
  prefixes are not listed. A listing of more than 1,000 prefixes is followed
  to its last page. (R6)
- [ ] A remote session that has failed is in the needs-you band; a remote
  terminal session is absent without `--all` and present with it. (R7)
- [ ] On B, a local session's row has no marker, and its Session tab shows a
  Machine row naming B (or `unknown` for a session from before this
  feature); a remote row's Session tab shows Machine and the workspace id,
  and its other tabs each show the one-line notice. At a terminal width of 80
  columns a remote row with a 40-character machine name still shows `@` and
  the name's first 8 characters. (R8)
- [ ] With sessions named `review` on A and on B under different prefixes,
  B's TUI and `--once review` both show two `review` rows with different
  `machine` and `workspace` values; a child of A's `review` nests under A's
  row, not B's. (R9, R14)
- [ ] A session B created itself appears exactly once in B's `--once`
  output. (R10)
- [ ] After `koto session import` of A's session on B, B's `--once --all`
  lists the copy and not the source. (R11)
- [ ] A remote session last written by a release that predates this feature,
  and one whose published state is truncated mid-record, are each listed with
  status `unknown` and counted in the unreadable tally. (R12)
- [ ] With `--needs-you`, B's `--once` includes A's blocked session; with
  `--status running` it doesn't. (R14)
- [ ] After `koto session cleanup` of a session on A, B's running TUI drops
  its row within 60 seconds and the next `--once` on B doesn't print it.
  (R13, R22)
- [ ] With the local backend, every line of `koto dashboard --once` over the
  existing `--once` test fixtures equals the line those tests expect today
  followed by two empty tab-separated fields. (R14, R20)
- [ ] A remote session whose machine name, id, state, intent and template
  name each contain a tab, a newline, an ESC, a U+0085 and a U+2028 prints in
  `--once` with each of those replaced by a space and exactly ten fields per
  line, and the TUI's rendered buffer for its row contains none of those
  characters. (R15)
- [ ] `--detail` for a session with a recorded machine prints its name in
  `machine`, and the bucket's request log shows no write during the run.
  (R16, R18)
- [ ] With keys granting only `s3:ListBucket` and `s3:GetObject`, B's
  dashboard lists A's sessions; the fake bucket records no write request and
  B's session store gains no directory. (R17, R18)
- [ ] With an endpoint that refuses connections, B's `--once` prints its
  local rows, one stderr line containing `unreachable` and the endpoint host,
  and exits 0; B's TUI shows its local rows and one status line with the
  same two facts. With the endpoint answering 403, the class is `denied`;
  answering 429 to every retry, `throttled`. Once the endpoint answers
  normally again, the TUI's status line is gone after the next refresh.
  (R19)
- [ ] With one of three prefixes returning errors on listing, B shows the
  other two prefixes' sessions and reports that 1 workspace couldn't be
  read and names that prefix's failure class. A session whose listing
  succeeds but whose state read returns 500 is counted in the unreadable
  tally. (R19)
- [ ] With an endpoint that accepts connections and never answers, B's TUI
  reflects a key press within 200 ms, and `--once` exits 0 within 15
  seconds. (R19, R23)
- [ ] With an empty bucket, B shows its local rows and no status line; with
  1,500 sessions under one prefix, all 1,500 are listed. (R6, R19)
- [ ] With the local backend selected and a bucket configured, a
  `koto dashboard` run makes no network request and shows no markers. (R20)
- [ ] For a remote session whose event log is 5 MB, B reads at most 64 KiB
  of object data to show its row; a second TUI refresh with no change in the
  bucket issues no GET request. (R21)
- [ ] After A's tick changes a session's state, B's running TUI shows the
  new state within 60 seconds of the tick's exit, measured with the
  dashboard's refresh clock under test control, and a `--once` run on B
  started after the tick exits shows it. (R22)
- [ ] With 200 prefixes, 2,000 sessions and 50 ms per request in the fake
  bucket, `--once` completes within 30 seconds. (R24)

## Out of Scope

- A hosted or managed service of any kind, including a shared web dashboard.
- Acting on another machine's session from the dashboard: advancing it,
  answering its gate, cancelling or cleaning it up. Those stay on the machine
  that runs it, or go through an import.
- Requests, review threads, messages or external signals in the dashboard.
  The dashboard renders sessions.
- Changes to session continuity in the bucket: pins, successors, imports,
  prune rules and the shared-bucket reads that already ship. They're built
  on as they are.
- Project-level session-storage settings. This feature reads the bucket
  configuration koto already resolves at user level.
- Several developers sharing one bucket, and an owner field naming the
  person who ran a session.
- Temporary (session-token) credentials. koto signs requests with static
  keys today, and this feature doesn't add token support.
- Changing `koto session list`. Its remote-only rows keep their current
  null-state treatment.

## Known Limitations

- Sessions written by koto releases from before this feature have no
  recorded machine, and may have no state the dashboard can read cheaply,
  until they're advanced by a release that writes it.
- Two machines with the same hostname and no configured machine name
  record the same name. Their rows are still marked as remote on each other
  (R8 decides by local copy, not by name), but the names don't tell them
  apart until the developer sets `machine.name` on one of them.
- The 60-second freshness bound and the 30-second `--once` bound are stated
  for up to 200 workspace prefixes. A bucket that a great many directories
  have used costs more listings per refresh.
- The machine name is whatever the producing machine recorded. A
  developer with write access to the bucket can write any name; the
  dashboard sanitizes it for display but doesn't verify it.
- Freshness is bounded by how often the producing machine pushes: a session
  that hasn't ticked since its last push shows its last pushed state.

## Decisions and Trade-offs

**The machine name defaults to the hostname, with a user-level override.**
The existing `machine.id` is an eight-digit hash of the hostname, which no
one recognizes and which collides when two machines share a hostname.
Requiring the developer to name every machine would make the single-machine
case pay for the multi-machine one. A hostname default with an override in
user-level configuration or the environment is recognizable out of the box
and fixable when two names collide. A machine name describes the machine,
not a project, so it lives with the machine's own settings.

**An import's copy records the importing machine.** The alternative, keeping
the original producer on the copy, would name a machine the session no longer
runs on, which is the wrong answer to "where do I go to act on it?". The
source copy keeps its own record, so the history of where the work started
isn't lost.

**No new credentials, and the read path writes nothing.** The existing
user-level configuration and environment variables already reach every
prefix in the bucket, and the dashboard needs only list and get. Requiring
the read path to write nothing means a developer can give a machine
read-only keys for watching, and keeps a dashboard left running on a laptop
from contending with the machines doing the work.

**Two appended `--once` columns: `machine` and `workspace`.** The dashboard's
text contract only appends columns and leaves absent values empty, so
appending keeps every existing consumer's positions. `machine` answers which
machine a session is on; `workspace` is what a script needs to read the
session further with `koto --workspace <id> status <name>`, and what tells
two same-named rows apart. A single combined `origin` column was rejected
because a script would have to split it.

**The TUI marks remote rows instead of adding a column.** The list has no
width left at 80 to 100 terminal columns, and a column that's empty for every
local row is noise for single-machine users. A marker on remote rows, plus a
Machine fact in the Session tab, shows the machine where it matters and
nowhere else. Whether a row is remote is decided by whether this machine
holds a copy, not by comparing machine names, so two machines that share a
name still mark each other's rows.

**`--once` exits 0 when the bucket fails.** A non-zero exit
on a partial view was considered so scripts notice. It was rejected because
it would turn a network blip into a failure for scripts that only care about
local sessions; the stderr note and the unreadable tally carry the signal,
following the precedent of the existing unreadable-session note.

**The dashboard never downloads an event log.** Logs on a long-lived host
run from a few kilobytes to several megabytes each, and a full scan of them
on every refresh isn't affordable. A session whose state can't be read
without its log is listed as unknown rather than fetched (R12, R21). How
current state becomes readable cheaply is the design's choice.

**`--detail` stays local.** Reading a remote session's full detail would
need the reads `koto --workspace` already offers. The dashboard's job here is
to show that a remote session exists and what state it's in; the workspace
column gives a script what it needs to read further.

**A 60-second freshness bound for the TUI.** The TUI refreshes local state
every 500 ms, which a bucket can't sustain across many prefixes. A minute is
short enough for "is my run waiting on me?" and leaves room for a slower
remote cadence. `--once` has no cadence of its own and reads the bucket's
current state every run.

**No owner field.** A bucket belongs to one developer in this feature's
scope, so every session in it has the same owner and the field would carry no
information. It belongs with shared buckets, which are out of scope.

**How sessions are discovered and how state is published are design
decisions.** The requirements bound the cost (R21, R23, R24) and the
freshness (R22) but don't pick between scanning every prefix, a small per-session
summary written at push time, or a bucket-wide index. That choice, and where
exactly the machine is recorded, belong to the design.
