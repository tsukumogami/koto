---
schema: brief/v1
status: Draft
problem: |
  Agents a person dispatches make real decisions while nobody watches, and
  afterwards no one can tell who made each decision, what entitled them to
  make it, or how to overrule it so the rest of the toolkit honours the
  overrule. Several partial records each answer "who decided" differently.
outcome: |
  The person or agent that dispatched a session can read every decision it
  made or received, who made each one and under what authority, and can
  overrule any of them with a decision of their own that every tool reading
  the record respects.
motivating_context: |
  Unattended runs now accept documents, merge, pass checks and settle their
  own judgment calls. Work being planned on top of them (crediting accepted
  plan steps, production and review verdicts, release-gate passes, an overrule
  rate per kind of decision) assumes a record of those decisions exists, and in
  the meantime koto and the shirabe skills plugin have each started recording
  pieces of it in their own way.
---

# BRIEF: The decision record

## Status

Draft

## Problem Statement

When a person hands work to an agent, that agent, and the agents it starts,
go on to make decisions without the person watching. They accept a plan or a
design, accept one step of a plan, merge a pull request, let work past a
check, decide whether a question is worth taking to whoever dispatched them,
and settle small judgment calls on their own. Often the session that wrote
something is the same one that accepts it, and that's legitimate when it was
given the authority to.

Afterwards three questions have no reliable answer for any of those
decisions:

1. **Who made it?** A session runs one main agent and possibly several agents
   under it, and a session was itself dispatched by a person or by an agent in
   another session. Today nothing distinguishes these. Where a person decided,
   nothing names them.
2. **What entitled them to?** Authority narrows as it is handed down: no
   agent holds more than its session's main agent, and no session holds more
   than whoever dispatched it. Nothing records what authority a decision was
   made under, so nobody can tell a decision that was within bounds from one
   that wasn't.
3. **How is it overruled?** The person or agent above can see results but not
   the decisions behind them. When they disagree with one, there is no way to
   overturn it that the rest of the toolkit will respect, and no way to see
   later that it was overturned and by whom.

What already exists makes the gap worse. koto records when its automated
grader was consulted or overridden, but never says who overrode it. It
records an agent's own answer when the grader can't give one, with the agent
only implied. A change in progress records disputed findings as coming from
"agent" or "person", and nothing reads them. The shirabe plugin's review
ledger names its writers as reviewer, check, grader or agent, with no person
and no time. Its coordinator keeps a decision table in a GitHub issue with a
free-text "decided by" column, and that's the only place a reversal keeps the
decision it overturned. The word "decider" means the automated grader in some
of these and a person or coordinator in others. And because a koto session
deletes itself when it finishes successfully, a worker's decisions are
usually gone by the time anyone above it wants to question them.

## User Outcome

The person or agent that dispatched a session can open the record after the
session is gone and read every decision it made or received: what kind of
decision it was, which session and agent made it (or which person, or which
automated check), and the authority it held at the time. When they disagree,
they overrule it, and the overrule is itself a decision with its own decider
and authority. The original visibly no longer stands, and every tool that
reads the record treats it that way. Work built on top of these decisions,
from crediting an accepted plan step to counting how often each kind of
decision is overruled, reads one record instead of each keeping its own.

## User Journeys

### A coordinator audits an unattended run

A coordinator dispatches a session that writes a design, accepts it under the
authority it was dispatched with, gets its change past a check, and merges.
After the session has finished and its workspace is gone, the coordinator
reads each of those three decisions with the session and agent that made it
and the authority it acted under.

### A person decides at an acceptance point

The same unit runs in a workspace where a person accepts the design instead.
The person answers in their terminal or approves a review on the forge. The
record names the person as the decider, says which surface the decision came
through, and links to it, without koto holding any user account for them.

### The session above overrules three decisions

The coordinator disagrees with the acceptance, the check pass and the merge.
It reverses each one. Each reversal is recorded with the coordinator as its
decider and its own authority, and points at the decision it overturns. The
three originals read as no longer in effect. koto doesn't undo the merge on
the forge; the reversal is recorded alongside the revert that undoes it.

### A worker escalates, declines, and settles

A dispatched session takes one question to its coordinator, decides another
isn't worth escalating, and settles a small judgment call itself. The
coordinator reads all three with their deciders, then overrules the judgment
call, and the overrule is recorded the same way.

### A check without a decider is refused

A session tries to get past a check without saying who is passing it, and
koto refuses, naming what's missing.

### A consumer counts overrules

A tool reading the record over a month counts how many decisions of each
kind were overruled and lists them, telling decisions made by a session apart
from those made by a person or an automated check.

## Scope Boundary

### In

- The kinds of decision the record covers: accepting a document, plan or plan
  step; merging; getting past a check; escalating a question or declining to;
  settling a judgment call; overruling an earlier decision.
- Naming the decider at two levels (the session and the agent within it), or
  a person without a user account, or an automated check.
- Recording the authority a decision was made under, and the chain of
  dispatchers above it that an overrule is checked against.
- A record that outlives the session that made the decision, readable and
  writable from other sessions on the same machine.
- What overruling each kind of decision means, and how the record shows a
  decision no longer stands.
- Bringing koto's existing grader, override, agent-answer and dispute records
  and the shirabe plugin's review ledger and coordinator decision table onto
  one vocabulary, each as a writer of the record or a view over it.
- Refusing to get past a check when no decider is named.

### Out

- **On whose behalf a decision was made.** Recording the person a session was
  acting for at the top of the chain is a later extension that builds on this record.
- **Rules about who may decide.** Requiring a person at a point, forbidding a
  session from accepting its own work, and the settings that narrow authority
  are policy layers that read this record later. This record says who decided
  and under what authority; it doesn't decide who is allowed to.
- **Record-health instruments.** Detecting a workspace that stopped recording,
  or auditing whether escalations were right, reads this record later.
- **Undoing effects outside koto.** koto doesn't revert merges, reopen pull
  requests or edit documents when a decision is overruled; the overrule is
  recorded and whoever acts on it undoes the effect.
- **Cross-machine durability.** The record lives with koto's local store; a
  copy that survives across machines isn't part of this feature.
- **Duplicating the agent harness's own records.** The harness's tool
  permission decisions and transcripts stay where they are; the record may
  point at them as evidence.

## Open Questions

- How the dispatcher chain reaches koto when the workspace manager never reads
  or writes workflow state. The PRD states what must be recorded; how it gets
  there is the design's.
- Whether planning elsewhere that expects the accepter to be someone other
  than the author needs anything from this record beyond the decider and the
  authority. This brief follows the framing that the authoring session may
  accept its own work when it holds the authority.

None of these questions block the brief.

## References

- `docs/prds/PRD-koto-decider-checks.md`: the automated grader whose
  consultations and overrides are one of the records to converge.
- `docs/prds/PRD-decider-fallback.md`: the agent's answer when the grader
  can't give one, another record to converge.
- `docs/reference/run-journal.md`: where koto records grader and override
  events today.
