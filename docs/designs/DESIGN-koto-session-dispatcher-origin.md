---
schema: design/v1
status: Accepted
problem: |
  The koto-skills start hook learns its own anchor's name only from the output
  of `koto session anchor open`, which now talks to the configured cloud store
  and often runs past the hook's five-second limit. When it does, the hook
  unsets the dispatcher variable in the agent's shells, so every session that
  agent launches starts with no dispatcher to name. Beside that, an anchor
  records a dispatcher or nothing, so a session with no parent reads the same
  as one whose link was lost, and koto has no verb that reports either.
decision: |
  The hook derives the dispatcher name from the payload's session id
  (`anchor-<session id>`) and exports it whatever the open returns. koto
  records each anchor's origin in the `anchor_started` event that creates it,
  in one of three states: dispatched by a named anchor, no dispatcher, or named
  but not recorded with a reason. A malformed dispatcher value no longer
  refuses the open, and a failed link write is recorded rather than failing
  it. A new read-only verb, `koto session origin`, reports the state for an
  anchor or for a workflow session through the anchor that opened it. Once a
  koto-skills release with the hook change is installed, shirabe's
  coordinator brief drops its manual link step. niwa doesn't change.
rationale: |
  The anchor name is a pure function of the session id, so taking the store
  out of the export path fixes the break without raising the hook's limit or
  reading any launcher variable. Writing the origin into the creating event
  ties the record to the anchor's existence: there's no second write that can
  fail on its own. A separate verb keeps every output an agent reads unchanged,
  so the agent's calls cost what they did. The alternatives either raise the
  start-up budget, make koto read a launcher's variables, or grow output that
  agents already pay for.
upstream: docs/prds/PRD-koto-session-dispatcher-origin.md
decision_provenance: inline-resolved
---

# DESIGN: Where a session came from

## Status

Accepted

## Context and Problem Statement

Every agent session running the koto-skills plugin gets an anchor, a koto
session named `anchor-<harness session id>` that the plugin's start hook
(`plugins/koto-skills/hooks/session-start.sh`) opens through
`koto session anchor open`. The hook then writes a block into the harness's
per-session env file, which the harness sources before every shell command.
That block exports `KOTO_ANCHOR` and `KOTO_DISPATCHER_ANCHOR`, both naming the
anchor the open printed. When the agent launches another agent session from
one of those shells, directly or through a workspace launcher that passes the
environment through, the new session's hook reads the inherited
`KOTO_DISPATCHER_ANCHOR` (falling back to `KOTO_ANCHOR`) and passes it to its
own open as `--dispatched-by`. koto writes that name into the new anchor's
`anchor_started` event and a dispatched-by link to the dispatcher's anchor.
From any workflow session, a reader can walk its held-by link to its anchor,
then the anchor's dispatched-by link to the dispatcher's anchor.

On the reference host that chain broke on 2026-11-01. The hook runs every koto
call under a five-second watchdog. Since koto stopped stripping the
`KOTO_SESSION_*` variables from the hook's calls, the open runs against the
cloud store the shells are configured for, and it now often outlasts the
watchdog. The anchor still gets created, but the killed process prints no
`anchor=` line, so the hook writes `unset KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR`
and every session launched from that agent starts with no dispatcher. Counted
from the anchors' own start events, the share naming a dispatcher fell from
every anchor through 2026-10-31 to none of 211 on 2026-11-10. Joined against
the launcher's own launch records, every launch that carried the variable
produced an anchor with a dispatcher and every launch without it produced
none, so the launcher's side works. That the store stall causes the lost
output is inferred from timing: the hook finishes two to five seconds after
the anchor's creation time in the sessions that lost it. The stall itself
wasn't observed.

So the technical problem has three parts. The dispatcher's name has to reach
its shells without depending on a store call finishing inside the hook's
budget. The anchor record needs a shape that tells a session with no
dispatcher apart from one whose dispatcher was named but not recorded. And a
reader needs one koto call that answers the question for any session, without
anything moving onto the agent: today's workaround, a step in each
coordinator brief that has the agent link its own workflow session, costs the
agent turns, puts dispatcher text in its transcript, and records nothing when
the agent skips it.

The code involved is the start hook, `src/session/anchor.rs` (anchor naming,
`open`, the creating event), the `AnchorStarted` payload in
`src/engine/types.rs`, the dispatched-by link writer in `src/session/links.rs`,
the CLI under `src/cli/session_anchor.rs`, and, in shirabe, the coordinator's
brief renderer and its golden test.

## Decision Drivers

These carry the requirements the design answers, in its own terms.

- **Every dispatch links itself.** A session launched from another agent
  session's shell, directly or through a launcher that passes the environment
  through, is recorded as dispatched by the launching agent session, with no
  action by either agent (R1).
- **Store latency can't break it.** That holds however slow the session store
  is, and when the dispatcher's own anchor open outruns the hook's limit (R2).
- **No parent is a record, not a gap.** A session started with no dispatcher in
  its environment, or with an empty one, is recorded as having none (R3). A
  dispatcher that was named but couldn't be recorded, including a value koto
  refuses as malformed, is recorded as named-but-not-recorded with the reason,
  and never as none (R4).
- **koto owns the handoff.** The dispatcher travels in a variable koto and the
  plugin define. A launcher conveys it only by passing the environment
  through, and nothing in koto, the plugin or shirabe reads a variable a
  particular launcher defines (R5, R11).
- **No launcher required, and no noise.** On a machine with koto and shirabe
  and no launcher, a shirabe session started from a plain directory reads as
  having no dispatcher, and nothing prints an error, warning or install prompt
  about it (R6).
- **One call answers it.** Every session's origin is exactly one of the three
  states, and one koto read verb reports it as JSON for an anchor or for a
  workflow session the anchor holds, following the link to the anchor itself
  (R7).
- **The manual step goes.** Once automatic linking holds, shirabe's
  coordinator brief stops telling the agent to record its dispatcher, and a
  worker whose brief has no such step is still linked (R8).
- **The agent pays nothing.** No new koto call or write by the agent, and the
  cost of its koto calls is unchanged beyond run-to-run variation (R9). This
  rules out adding anything to output agents already read.
- **No bigger start-up budget.** The hook's five-second limit and the agent's
  start-up time stay where they are (R10).
- **Implementation drivers.** The hook stays plain bash with no new
  dependency, exits 0 and prints nothing on stderr; anchors written by older
  koto versions must still read sensibly; and the change must be testable
  without a cloud store, with a test double for a slow or failing koto.

## Considered Options

All four questions were resolved inline under the scoping run; each one's
reasoning is recorded here.

### Decision 1: How the dispatcher's name reaches its shells

Resolved inline. The hook needs a name to export before, or without, the
store answering.

**Chosen: derive it from the session id.** The anchor family's first name is
`anchor-<session id>` (`anchor_name` in `src/session/anchor.rs`), and the hook
already holds the id from its payload and checks it against the plain-name
pattern. The hook exports `KOTO_DISPATCHER_ANCHOR` as the name the open
printed when it printed one, and otherwise as the derived family name. A
successor anchor (`-s<n>`) only appears when the open says so, and the family
name still identifies the same agent session, which is what the dispatcher
field names. `KOTO_ANCHOR` follows the same rule. It has to: the worker's
own open is as likely to time out as the dispatcher's, and a worker whose
shells have no `KOTO_ANCHOR` opens workflow sessions with no held-by link,
so the reader would find no anchor and report `none` for a dispatched
worker. A held-by link naming the family's first anchor where a successor
is current is no worse for retention than today's timeout case, which writes
no held-by link at all.

**Alternatives.**

- *Raise or split the hook's limit.* Ruled out by R10, and it would only move
  the threshold for the next slow store.
- *Make `anchor open` return before the cloud write, creating locally first.*
  It changes the store's consistency rules for every anchor, a much larger
  change than the problem asks for, and the hook would still depend on koto
  answering in time.
- *Have the launcher pass the parent's session id, which it already carries.*
  It makes koto or the plugin read a launcher-defined variable, which R5 and
  R11 forbid, and does nothing for launches without that launcher.
- *Write the env file later, from the detached watcher, once the open
  finishes.* The agent's first shell commands can run before the watcher
  writes, so early launches would still lose the dispatcher.

### Decision 2: The shape and place of the origin record

Resolved inline.

**Chosen: an `origin` field on the creating `anchor_started` event.** When
`open` creates an anchor (a new family or a successor), the event it appends
carries one of:

- `{"state": "dispatched", "dispatcher": "<anchor name>"}`
- `{"state": "none"}`
- `{"state": "unrecorded", "named": "<value preview>", "reason": "<text>"}`

The event is written in the same call that creates the anchor, so what the
starting environment named can't be lost separately from the anchor. The
dispatched-by link is still a second write, and its failure is what the
`unrecorded` state records. Three changes go with it. A malformed
`--dispatched-by` value no longer refuses the whole open: the anchor is
created and records `unrecorded` with the validation message as its reason.
An empty value records `none`. When the dispatched-by link write fails after
the anchor exists, `open` appends an `origin_unrecorded` event with the
error and still prints its `anchor=` line, instead of failing. The hook stops
discarding inherited values that miss its own anchor pattern and passes any
non-empty value through, so koto, not the hook, decides what is malformed.

**Alternatives.**

- *A separate context key holding the origin.* A second write that can fail
  independently of the anchor's creation, which is exactly the gap the record
  exists to close.
- *Infer the state from whether the link exists.* A missing link can't be told
  apart from a session with no dispatcher.
- *Record the origin on each workflow session at `koto init`.* It puts the work
  on the agent's own koto calls, against R9, and repeats one fact on every
  session an agent opens.

### Decision 3: The read verb

Resolved inline.

**Chosen: a new read-only verb, `koto session origin <name>`,** printing JSON
with the state and, by state, the dispatcher or the reason, plus `via` naming
the anchor the answer came from. For an anchor it reads the record directly.
For a workflow session it first looks for a dispatched-by link on the session
itself (written by `koto init --dispatched-by` or `koto session link`), and
otherwise follows the held-by link written when the session was opened to that
anchor. A held-by link to an anchor that doesn't exist reads as
`unrecorded` with the reason `anchor not found`. A workflow session with
neither link reads as `none`: it was opened with no anchor in its
environment, so nothing named a dispatcher for it. An anchor written
before this change has no `origin` field: one whose event names a dispatcher
reads as `dispatched`, and one that names nothing reads as `unrecorded` with
the reason `recorded before koto tracked origin`, so an old gap is never shown
as a root.

**Alternatives.**

- *Add the field to `koto status`.* Agents call `koto status`, so every call
  would carry more output, against R9.
- *Extend `koto session hierarchy --json`.* It reads only the local sessions
  directory, so on a host using the cloud store it answers the wrong question,
  and its output is far larger than one state.

### Decision 4: Retiring the manual link step

Resolved inline.

**Chosen: remove the link row from shirabe's coordinator brief, and from its
golden test, once a koto-skills release with the hook change is installed on
the reference host.** The link script stays callable, so briefs already in
flight keep working. Removing the step first would lose links in the meantime.

**Alternatives.**

- *Keep the step and make the script do nothing.* The instruction would still
  land in the agent's transcript, which AC8 checks for.
- *Delete the script with the step.* Workers running from older briefs would
  hit a missing command.

## Decision Outcome

The fix is one hook change and one koto change, with a shirabe change after
both have shipped. The hook exports the dispatcher's name from the session id
it already holds, so a slow store no longer erases it from the agent's
shells; that is also the repair for the link that stopped being written, so
the repair and the new behavior are the same change. koto writes the origin
into the event that creates the anchor and reports it through a verb no agent
calls. The launcher already passes the environment through and is left alone;
its own copy of the parent's id stays unused.

This settles the four questions the framing left open. The
launcher hands the dispatcher to koto by passing `KOTO_DISPATCHER_ANCHOR`
through, a variable koto defines. koto records it as the `origin` of the
anchor's creating event, in three states, read by `koto session origin`. The
link failed because the hook exported its anchor's name only from the output
of a store-bound call that now outlasts the hook's limit, and deriving the
name is the repair. niwa needs no change.

## Solution Architecture

### Overview

```
dispatcher's start hook
  open anchor (may time out)  ->  env file: KOTO_DISPATCHER_ANCHOR=anchor-<id>
                                   (printed name, else derived)
dispatcher's shell -> launcher (passes env through) -> worker agent session
worker's start hook
  reads KOTO_DISPATCHER_ANCHOR -> koto session anchor open --dispatched-by=<value>
  koto creates the anchor; anchor_started carries origin
    valid value, link written  -> dispatched
    empty or unset             -> none
    malformed, or link failed  -> unrecorded + reason
reader: koto session origin <workflow session | anchor>  -> JSON
```

### Components

- **Start hook** (`plugins/koto-skills/hooks/session-start.sh`). Computes
  `derived="anchor-$session_id"` after the id check, so the name holds only
  plain-name characters and is safe inside the single quotes the env-file
  block already uses. It keeps two values apart: `anchor`, the name the open
  printed, and `exported`, the name the env file carries. `exported` is
  `anchor` when the open printed one, `derived` when the open was cut off by
  the watchdog or failed without a refusal, and empty when koto refused the
  open (exit 2 with no `anchor=` line, such as a session id koto won't name
  an anchor after); only an empty `exported` writes the `unset` line. The
  watchers and the additionalContext fallback keep reading `anchor`, so
  nothing runs against an anchor that wasn't confirmed. The hook passes any
  non-empty inherited dispatcher value, cut to 256 bytes, to the open in
  `--flag=value` form. An older koto refuses a malformed value outright, so
  when the open exits 2 with no `anchor=` line and the value missed the
  anchor-name pattern, the hook opens again without it, the same fallback
  it already uses for `--record-authority`. The watchdog and the limit are
  unchanged.
- **Anchor open** (`src/session/anchor.rs`). `OpenRequest` carries the raw
  dispatcher value. `open` validates it without refusing, computes the
  origin, and puts it on every `AnchorStarted` that creates a session (a new
  family, a reopen, a successor). A link-write error after creation becomes
  one `origin_unrecorded` event, at most one per anchor, and an
  `origin=unrecorded` output line, and the open still succeeds. Continuing a
  live anchor writes no new origin; the creating event's stands.
- **Event payload** (`src/engine/types.rs`). `AnchorStarted` gains an optional
  `origin` (absent in older logs). A new `OriginUnrecorded` payload holds the
  named preview and the reason.
- **Read verb** (`src/cli/session.rs` and a small `src/session/origin.rs`).
  `koto session origin <name>` resolves the session, finds the anchor by the
  rules in Decision 3, folds the creating event, the link and any
  `origin_unrecorded` event into one state, and prints it. A creating event
  that names a dispatcher with no dispatched-by link beside it reads as
  `unrecorded` whether or not the event recording the failure got written.
  It reads through the configured store backend, the path `koto status`
  uses, not the local-directory helpers in `src/session/anchor.rs`, writes
  nothing, and exits 0 for any of the three states.
- **Mixed versions.** Older koto readers ignore the new field and event, as
  serde already does for unknown optional fields. Anchors created by an older
  koto have no `origin` and read by the legacy rule in Decision 3.
- **koto-skills skills.** The `koto-user` skill gains the new verb and the
  `origin=` output line, per the repository's rule for source changes under
  `src/cli/`.
- **shirabe coordinator brief** (in the shirabe repository). The brief
  template and its renderer drop the link row; the golden test's expected
  briefs drop it too.

### Key Interfaces

`koto session origin <name>` prints one JSON object:

```json
{"session": "<name>", "via": "<anchor name or null>",
 "origin": {"state": "dispatched", "dispatcher": "anchor-<id>"}}
```

with `origin` being `{"state": "none"}` or
`{"state": "unrecorded", "named": "<preview>", "reason": "<text>"}` in the
other states. In the dispatched state `dispatcher_kind` says what
`dispatcher` names: `anchor` when the answer came through an anchor, or
`workflow` when it came from a link on the workflow session itself, as the
older manual step wrote. The verb's reference documents the `origin` field (AC6). The
anchor open's output gains `origin=<state>`; the hook doesn't need to read it.

### Data Flow

The value moves only through the environment and the anchor's own event log.
The dispatcher's hook writes the env file without a store round trip, the
launcher copies the environment, the worker's hook passes one flag, and koto
writes one event in the call it already makes. The reader makes one call.

## Implementation Approach

### Phase 1: The record and the read verb (koto)

The `origin` field, the `OriginUnrecorded` event, the non-refusing
validation, the recorded link failure, and `koto session origin` with its
reference. Unit tests for each state, for an anchor and for a workflow
session through its anchor, and for an anchor from before the change.
Covers AC4 and AC6.

### Phase 2: The hook (koto-skills plugin)

The derived export and the raw pass-through, with the hook test extended by
a koto test double whose open sleeps past the limit: the hook returns within
it, the env file names the derived anchor, and a session launched from that
environment records it (AC2, AC9). Ships in the same koto release as Phase 1.

### Phase 3: Retire the manual step (shirabe)

After the koto release from Phases 1 and 2 is installed on the reference host,
drop the link row from the coordinator brief and its golden test (AC8's
golden-test half), and confirm by search that nothing in koto, the plugin or
shirabe reads a launcher's variable to learn the dispatcher (AC5).

### Phase 4: Verify on the reference host

No code. The three acceptance checks from the roadmap, run by a reviewer:

- **Ten dispatches.** With the cloud store in use, dispatch ten workers in a
  row from one coordinator with briefs that carry no link step, and run
  `koto session origin` on each worker's workflow session and on its anchor.
  Ten of ten report the coordinator's anchor (AC1).
- **No launcher.** On a machine with koto and shirabe and no launcher, start a
  shirabe session from a plain directory, open one workflow, and run the verb
  on the workflow session and, when the plugin is installed, on the anchor.
  Both report `none`, and the hook output, every command's output and the
  transcript hold no error, warning or install prompt (AC3).
- **No cost to the agent.** Take measured reads of the same worker task, at
  least three runs before the change and three after. The count of the
  agent's koto calls matches, the median cost of those calls after falls
  inside the before range, and a case-insensitive search of each after
  transcript for `dispatched-by`, `link-dispatcher` and `dispatcher_session`
  finds nothing (AC7, AC8).

## Security Considerations

The dispatcher value comes from an environment variable any process in the
agent's environment can set, so a process could name an arbitrary anchor as
its dispatcher. That's the trust model the hook has today and is recorded as
a known limitation, not a new exposure: the record says who the starting
environment claimed, and nothing grants authority from it. The value now
reaches koto even when malformed, so koto stores only a bounded preview of
it (the existing `preview` helper) and never interprets it; it goes in
`--flag=value` form, so a leading `-` isn't read as a flag. The derived name
is built only from a session id that already passed the hook's plain-name
check. No credential, token or store configuration enters the record or the
verb's output, and the verb is read-only. Dropping the link step removes
nothing a worker relied on for authority.

Four rules keep the record from carrying more than it should:

- Every `reason` is one of a closed list, never a store's raw error text,
  which can carry endpoints or fragments of credentials: `malformed: not an
  anchor name`, `self: names this anchor`, `link write failed: store error`,
  `link not found`, `anchor not found`, and `recorded before koto tracked
  origin`. A test feeds a store error carrying a marker string through the
  link writer and checks the marker never reaches the record or the verb.
- The hook cuts the value to 256 bytes and koto refuses longer. The `named`
  preview is bounded, and control characters and newlines in it are
  replaced, so a value can't forge an `anchor=` line in the open's output or
  put terminal escapes in a reader's output. A value with characters outside
  the plain-name set is kept as a SHA-256 prefix and a length, not as text.
- The derived name is built from a session id that passed the hook's
  plain-name check, so the single-quoted export can't be broken out of; the
  hook test runs a hostile id (quotes, `$(...)`, a newline) and checks the
  env file carries no export for it.
- The existing `dispatched_by` field on the event is set only for a value
  that passed validation, so older readers never see a malformed one.
- A value naming the anchor being opened records `unrecorded` with a fixed
  reason. A value naming an anchor that doesn't exist yet records
  `dispatched`, since the dispatcher's own open may still be creating it.

The verb's reference says plainly that `dispatched` means the starting
environment named that dispatcher, and that the claim isn't authenticated;
decision records and shirabe read it the same way, as a claim and never as a
grant. Two things are new surface rather than today's: malformed values now
reach koto, which the rules above bound, and the exported name no longer
waits for the store to confirm the anchor. In a store shared by several
hosts, a name derived from one session id could match an anchor another
party created under the same id; harness session ids are random UUIDs, so
that needs a collision or a forged id, and the forged case is the same
environment trust the hook already extends.

## Consequences

### Positive

- Dispatches link again however slow the store is, with no change to the
  launcher, the hook's limit or the agent.
- A reader can tell no parent from a lost link, and gets the answer in one
  call.
- The manual step and its transcript text go away.

### Negative

- Older anchors can't say whether they had no parent; they read as
  `unrecorded` with a fixed reason.
- If a worker's anchor open fails before the anchor exists, its workflow
  sessions hold a link to an anchor that isn't there. The verb reports that
  as `unrecorded` with the reason `anchor not found`, so it reads as a gap,
  but the dispatcher's name is lost for that worker. Today that open almost
  always creates the anchor before stalling, so the window is small.
- When a timed-out open would have opened a successor, the shells name the
  family's first anchor instead, and held-by links point at it rather than
  at the live successor.
- After the shirabe step retires, the dispatcher a decision record reads
  names the coordinator's anchor rather than its workflow run, until a reader
  walks from that anchor to its sessions.

### Mitigations

- The `unrecorded` reason for old anchors is fixed text, so a reader can filter
  it.
- The failed-open case reads as `unrecorded`, never as a root, and the
  reference-host check runs the verb on every one of ten workers, which would
  surface it.
- The link script stays callable for anyone who still wants a workflow-level
  link.
