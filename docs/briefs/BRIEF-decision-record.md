---
schema: brief/v1
status: Accepted
problem: |
  Delegated sessions accept documents, pass gates, merge, escalate or decline to, and settle
  judgment calls with nobody watching. Nobody above can later read who made each decision and
  on what authority, or reverse one so the toolkit honours it, and the records that exist each
  name the decider their own way, some of them in a host file outside any session.
outcome: |
  The session or person above a delegated session reads every decision it made or received from
  the deciding session's own record, with the decider and the authority it acted under, and
  reverses any of them; the reversal is named and the original visibly no longer stands, even
  after the deciding session has finished.
motivating_context: |
  A first build of this record (koto#394, koto#396) kept decisions in a host-level store,
  `~/.koto/decisions/`, because koto removed a session when its workflow succeeded. The owner
  closed both unmerged as a workaround for that limit. koto now keeps a finished session readable
  while any session in its dispatch hierarchy is alive (koto#410 and the work after it), so the
  record can live where the decision was made.
---

# BRIEF: The decision record

## Status

Accepted

## Problem Statement

When a person hands work to an agent, that agent and the sessions it dispatches go on to make
decisions without the person watching. They accept a design or a plan, merge a pull request, let
a workflow past a gate, decide whether a question is worth taking to whoever dispatched them, and
settle judgment calls. Often the session that wrote something also accepts it, which is fine when
it holds the authority to.

For every one of those decisions three things are missing. Nothing says who made it: a session,
an agent inside it, a person, or an automated check. Nothing says what entitled them to. And the
session or person above has no way to overrule it that a later reader can see and that koto's own
workflows will respect.

The records that do exist make this worse. koto's gate overrides, `koto decisions record` and
`koto decider answer` keep a choice with no actor or an implied one. `koto disputes record` tags
its writer `agent` or `person` with nothing more. The decider ledger keeps gate answers in a host
file, `~/.koto/_decider_ledger.jsonl`, because sessions used to be deleted. shirabe's findings
ledger and its coordinator's decision table each have their own idea of who decided. Every one of
those is a separate record forming in parallel, which is the outcome the delegation work set out
to prevent.

The first build of a shared record solved the attribution and the reversal, then put every
decision in a new host-level store because the deciding session would be gone by the time anyone
read it. The owner rejected that store as a workaround for koto's session lifetime, not a fix for
it, and asked for decisions to live in the koto session itself. Session retention has since made
that possible: a finished session stays readable while any session that dispatched it, or that it
dispatched, is alive. What's missing now is the record built on that footing.

## User Outcome

A coordinator, or the person at the top of a dispatch chain, opens any session below it and reads
every decision that session made or received: what kind it was, what it was about, who decided,
and the authority they acted under. It reads them from the deciding session's own record, after
that session has finished, for as long as the coordinator's own session is alive, and nothing
about the decision sits in a file under koto on the host.

When the coordinator disagrees, it reverses the decision from its own session. The reversal is a
decision of its own, with its own decider, and from then on every reader sees the original as no
longer in effect. A live workflow that built on a reversed gate pass stops at its next step
instead of carrying on. A person deciding is named without koto holding an account for them, and
no gate is ever passed without someone named as having passed it.

## User Journeys

### A coordinator audits an unattended unit after it finished

A coordinator dispatched a worker that ran unattended, accepted a design, got past a review gate
and merged its pull request, then finished. While the coordinator's session is still open, it
lists the worker's decisions and reads all three, each with the worker session as decider, the
agent inside it where known, and the dispatch authority it held. Nothing was read from a host
file.

### A maintainer decides at the acceptance point

The same unit runs again, but this time a maintainer accepts the design at their terminal. The
record names the maintainer as the decider, by a handle the surface supplied, with no koto account
behind it, and says which session entered the decision for them.

### The coordinator reverses what it disagrees with

Reading the unattended run, the coordinator disagrees with the acceptance, the gate pass and the
merge. It reverses each from its own session. Each reversal is named with the coordinator as its
decider, the originals now read as reversed, and the worker, had it still been running, would have
been stopped at its next step past the reversed gate.

### A worker's quieter calls become visible

A worker takes one question to its coordinator, decides another isn't worth escalating, and
settles a judgment call itself. The coordinator reads all three with their deciders, and reverses
the judgment call. A choice that used to leave no trace now has one, with a name on it.

### A gate pass nobody named

A worker overrides a failing gate without saying who decided. The maintainer auditing the run
later finds the worker session named as the decider and marked as defaulted, so every such pass can
be found and questioned. When a CI script with no session identity tries the same override without
naming a decider, the pass is refused and the workflow stays where it was.

### Reversing two levels down

Session A dispatched B, and B dispatched C. C settles a call and keeps working; B finishes. From
A, the person reverses C's decision. The reversal is recorded in A's own session, C's next step is
blocked by it, and nothing in B's or C's session changed because A read or wrote. When two sessions
above C reverse the same decision at once, both reversals are recorded in their reversers'
sessions, every reader settles on the same one, and the other reads as refused.

### The session above moves to another workspace

With the owner's per-command cloud configuration, the person moves the coordinating session to
another workspace on the same host with `koto session import`. From there they still read the
decisions below it and reverse one.

## Scope Boundary

**In:**

- The kinds of decision recorded, accepting, merging, getting past a gate, escalating, declining
  to escalate, settling a judgment call, a pass or fail verdict, reversing, and a refused attempt,
  and what reversing each one means.
- The decider, at two levels (the session and the agent within it), or a person named without an
  account, or an automated check; and the authority it acted under.
- The dispatch chain each decision is judged against, read from what koto's session links already
  record, so standing to reverse can be decided later.
- Keeping each decision in the deciding session's own record, write-once, and reading it from
  above through session retention.
- Reversal from above, recorded in the reverser's own session, with a deterministic answer when two
  reversers race.
- Blocking a live workflow's next step after a gate pass it relied on was reversed.
- Every gate pass naming a decider, defaulted to the observed session when none is named, refused
  when none can be established.
- Retiring the decider ledger as a host file: its answers belong in the deciding session, and its
  readers move with them.
- Bringing koto's existing partial records (overrides, `koto decisions record`, decider answers,
  disputes) onto the one vocabulary.
- Reads from above that leave the read session unchanged.

**Out:**

- On whose behalf a decision was made, beyond naming the decider. That builds on this record later.
- Rules about who may decide at a point, requiring a person, or excluding a document's author. Those
  are policy layers that read this record; this record says who decided, and only decides who may
  reverse.
- Undoing effects outside koto. koto won't revert a merge, reopen a pull request or edit a
  document's status when a decision is reversed; the reversal points at the compensating change.
- Session retention itself, the dispatch links and the anchors. Those already shipped; this work
  reads them and doesn't change their rules.
- Reading another host's sessions directly. A hierarchy that moves hosts moves by import; reads
  across hosts without an import wait on the owner's instruction.
- Any change that makes niwa require koto or koto require niwa. niwa never reads or writes workflow
  state for this.
- Rebuilding shirabe's findings ledger or coordinator record. shirabe's writers move onto the
  vocabulary as their own work items, and the coordinator's interim record folds in when this lands.
- Pruning decisions. They go when their session goes, under retention's rules.
