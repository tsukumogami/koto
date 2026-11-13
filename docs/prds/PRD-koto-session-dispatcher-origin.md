---
schema: prd/v1
status: Draft
upstream: docs/briefs/BRIEF-koto-session-dispatcher-origin.md
problem: |
  koto can't reliably say which agent session dispatched a session on the same
  host. The start hook's automatic link has stopped being written on the
  reference host since 2026-11-01, the working substitute is a step the agent
  runs by hand, and a session with no dispatcher has no record that says so.
goals: |
  Every agent session started on a host is recorded by koto with the session
  that dispatched it, or with an explicit record that it has none, and a failed
  link reads as failed. The agent does nothing and pays nothing for it, and
  neither koto nor shirabe needs a workspace launcher to be installed.
---

# PRD: Where a session came from

## Status

Draft

Requirements for recording each session's dispatcher in koto, derived from the
Accepted `docs/briefs/BRIEF-koto-session-dispatcher-origin.md`. This document
fixes what must be true and how it is checked; the mechanism is the design's.

## Problem Statement

Every agent session that runs with the koto-skills plugin gets an anchor, a koto
session named after the coding-assistant session's id, opened by the plugin's
start hook. The workflow sessions the agent opens are held by that anchor. When
one agent session launches another from its own shell, directly or through a
workspace launcher that passes the environment through, the start hook in the
new session names the launching session's anchor as its dispatcher and koto
writes a dispatched-by link. With that one link, a reader can walk from any
workflow session to its anchor, from the anchor to the dispatcher's anchor,
and from there to the dispatcher's workflow sessions.

On the reference host that link is no longer written for most dispatched
sessions. Measured from the anchors' own start events, the share of new anchors
that name a dispatcher fell from all of them through 2026-10-31 to 9 of 19 on
2026-11-02, 14 of 123 on 2026-11-05, and none of 211 on 2026-11-10. The break
lines up, to the hour, with the start hook no longer exporting the anchor's
name to the dispatcher's own shells: the hook exports the name only when
`koto session anchor open` prints it, the hook stops waiting after five
seconds, and since the hook began opening the anchor in the configured session
store (a cloud bucket on this host) rather than the local one, the open often
runs past that limit. The anchors still get created; only the printed name is
lost, so every session launched from that shell starts with no dispatcher to
name. The launcher passes the variable through faithfully: every launch that
carried it produced an anchor with a dispatcher, and every launch without it
produced none. That causal chain is inferred from the measurements; the stall
itself was not observed.

Two more gaps sit beside the broken link. A session nobody dispatched gets an
anchor with nothing in the dispatcher field, which a reader can't tell apart from
a dispatch whose link failed. And the substitute in use today, a step written
into each dispatched worker's brief that has the agent run a link command once
its workflow session is open, costs the agent turns and tokens, puts dispatcher
text in its transcript, and records nothing when the agent skips it.

## Goals

- Restore the automatic dispatcher link so it is written on every dispatch,
  whatever the latency of the session store.
- Give a session with no dispatcher an explicit record, and give a link that
  failed a record that says it failed.
- Keep the agent out of it: no new writes by the agent, no change in the cost
  of its koto calls, no dispatcher text in its transcript.
- Keep koto and shirabe free of any dependency on a workspace launcher.

## User Stories

- As an operator on the reference host, I want to ask koto about a worker a
  coordinator dispatched and see the coordinator's session as its dispatcher,
  so I can tell how the work is organized without asking an agent.
- As a person starting a shirabe session by hand on a machine without a
  launcher, I want it to start with no error, warning or install prompt about
  dispatchers, and be recorded as having no parent.
- As a reader of koto's records, I want to tell a session that has no
  dispatcher from one whose dispatcher link failed, so a gap in the record is
  never mistaken for a root.
- As the owner of the fleet's costs, I want this to add nothing to what an
  agent spends, so recording origin is free.

## Requirements

### Functional

- **R1. Automatic link on every dispatch.** A session launched from another
  agent session's shell, directly or through a launcher that passes the
  environment through, is recorded in koto as dispatched by the launching
  agent session, with no action by either agent. (Known Limitations names the
  one case this rule gets wrong.)
- **R2. Independent of store latency.** R1 holds when the session store is
  slow or the anchor open in the dispatcher's start hook exceeds the hook's
  time limit. The dispatcher's identity reaches its shells without waiting on
  the store.
- **R3. Explicit no-parent record.** A session started with no dispatcher in
  its environment is recorded with an explicit statement that it has none.
- **R4. A failed link reads as failed.** When a dispatcher was named but koto
  could not record the link, koto records that it was named and that the link
  failed, distinct from R3's record. A named value koto refuses as malformed
  gets this record too, with the refusal as its reason.
- **R5. One handoff, owned by koto.** The dispatcher reaches koto through a
  variable koto and the koto-skills plugin define. Any launcher conveys it by
  passing the environment through. koto, the plugin and shirabe read no
  variable that a particular launcher defines to learn the dispatcher.
- **R6. No launcher required.** On a machine with koto and shirabe and no
  launcher installed, a shirabe session started from a plain directory gets
  R3's record, and nothing in koto, the plugin or shirabe prints an error, a
  warning or a prompt about a missing launcher or dispatcher.
- **R7. Readable from koto.** Every session's origin is in exactly one of
  three states: dispatched by a named session (R1), no dispatcher (R3), or
  named but not recorded (R4). One koto read verb, invoked on an anchor or on
  a workflow session the anchor holds, reports that state in its JSON output;
  for a workflow session it follows the session's own link to its anchor, so
  the reader makes one call and walks nothing by hand.
- **R8. The manual link step retires.** Once R1 holds, the brief that
  shirabe's coordinator renders for a dispatched worker no longer instructs the
  agent to record its dispatcher, and a worker whose brief carried no such step
  is still recorded per R1. This is a change to shirabe, not to the launcher.

### Non-functional

- **R9. No agent cost.** The agent issues no new koto call and writes nothing
  new for this feature; the work happens in the start hook and in koto.
  Measured before and after, the cost of a worker's koto calls is unchanged
  beyond run-to-run variation.
- **R10. Start-up budget.** The start hook's time limit and the agent's
  start-up time are not raised to achieve R1.
- **R11. No new launcher dependency.** No change is required in the launcher
  for R1 to R8 to hold, and none of them holds only when it is installed.

## Acceptance Criteria

- [ ] **AC1 (R1, R2).** On the reference host, with the cloud session store in
  use, a reviewer dispatches ten workers in a row from one coordinator through
  the launcher, with briefs that carry no link step. For each worker, the R7
  read verb invoked on the worker's workflow session, and again on its
  anchor, reports the coordinator's agent session as dispatcher. Ten of ten.
- [ ] **AC2 (R2, R10).** In an automated test, the anchor open in a start
  hook is made to take five seconds longer than the hook's time limit (a test
  double for koto). The hook still returns within its limit, and a session
  launched from that agent session's shell records it as dispatcher.
- [ ] **AC3 (R3, R6, R7).** On a machine with koto and shirabe and no
  launcher, a reviewer starts a shirabe session from a plain directory and
  opens one workflow. The R7 read verb, on the anchor and on the workflow
  session, reports the no-dispatcher state. The start hook's output, every
  koto and shirabe command's stdout and stderr in the session, and the
  session's transcript hold no error, warning or install prompt.
- [ ] **AC4 (R4, R7).** In an automated test, a dispatcher is named and the
  link write is made to fail; in a second test the named value is malformed.
  In both, the R7 read verb, on the anchor and on a workflow session it holds,
  reports the named-but-not-recorded state with a reason, and never the
  no-dispatcher state.
- [ ] **AC5 (R5, R11).** A search of koto, the koto-skills plugin and shirabe
  finds no read of a launcher-defined variable used to learn the dispatcher.
- [ ] **AC6 (R7).** The R7 read verb's JSON output carries the origin state
  under a field its reference documents, with the dispatcher's name in the
  dispatched state and the reason in the named-but-not-recorded state.
- [ ] **AC7 (R9).** A reviewer takes measured reads of the same worker task,
  at least three runs before the change and three after. The count of the
  agent's koto calls is the same, and the median cost of those calls after
  the change falls within the range of the before runs.
- [ ] **AC8 (R8).** A search of the agent's transcript in each AC7 after run,
  case-insensitive, for `dispatcher`, `dispatched-by` and `link-dispatcher`
  finds nothing, and the brief shirabe's coordinator renders for a worker, in
  its own golden test, carries no link step.
- [ ] **AC9 (R10).** The start hook's time limit is the value it was before
  the change, and in AC2's slow case the hook returns within it.

## Out of Scope

- Seats, roles, holders, and links between a session and its successor.
- Any reader, listing or view built on these records.
- A coordinator adopting a session that has no recorded parent.
- Sessions started on another machine, and links across machines.
- Any workspace field in the record: a session's origin here is its
  dispatcher and nothing else.
- Changes to how the launcher starts workers.

## Known Limitations

- A session launched from a plain terminal that happens to inherit an agent
  session's environment is recorded as dispatched by that agent session. That
  is the same rule the hook applies today.

## Decisions and Trade-offs

- **Anchor link over workflow-session link.** The requirement is met at the
  agent-session level, where the start hook already runs for every session,
  rather than by asking each workflow entry point to link its own session. A
  reader reaches a workflow session's dispatcher through its anchor. This is
  what lets the manual step retire without moving work onto the agent.
- **The launcher stays as it is.** It already passes the environment through
  and already carries the variable when the dispatcher's shell has it. Its own
  copy of the parent session's id is not used, so nothing comes to depend on
  it.
