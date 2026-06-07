---
schema: brief/v1
status: Accepted
problem: |
  koto can fan a workflow out to child workflows and learn that each
  child finished, but not what it produced. Completion records only a
  terminal-state name, so a coordinator that needs each child's outcome
  must read the child's session log -- reintroducing the context load
  the fan-out was meant to avoid.
outcome: |
  A coordinator converges a fan-out by reading its children's closed
  results inline at a converge point, learning each outcome without
  opening any child's log. Completion carries a result; convergence is
  a read, not a re-derivation.
motivating_context: |
  koto v0.10.0 already ships the fan-out half -- child workflows are
  created and linked to a parent, the parent discovers which need an
  agent, agents claim and run them, and terminal sessions are indexed.
  The converge half is missing: the terminal index records a state
  name, not a result. This brief frames closing that gap.
---

# BRIEF: request-store-converge

## Status

Accepted

Phase 4 jury returned all-PASS (content-quality and structural-format).
The downstream PRD owns the result-payload schema, the
auto-promote-versus-explicit-post decision, and where the result lives
and how the converge point reads it; this brief stops at the framing.

## Problem Statement

koto's coordinator-and-delegates model lets one workflow fan work out
to child workflows: the children are created and linked to the parent,
the parent learns which children need an agent, agents claim and drive
them, and a child that reaches a terminal state is recorded in koto's
terminal index. That machinery answers *which children are done*.

It does not answer *what each child produced*. The terminal-index entry
carries the child's terminal-state name and a timestamp -- not the
child's result. So a coordinator that fanned out three evaluations and
now wants to converge them has no way to read the three outcomes from
the parent's own directive. To learn what a child decided, the
coordinator must open that child's session and read its log.

That is the exact cost the fan-out exists to avoid. The whole point of
delegating to a child is that the coordinator does not carry the
child's working detail; making the coordinator read each child's log to
converge defeats the separation. The gap is structural, not cosmetic: a
coordinator can dispatch cleanly but cannot *converge* cleanly, because
completion is a bare done-signal rather than a closed result the parent
can read.

## User Outcome

A coordinator finishes a fan-out by reading each child's closed result
at a single converge point in its own workflow -- it sees what every
child concluded without opening a single child log. Completion grows a
result: when a child workflow ends, the outcome it reached travels with
the completion signal, and the parent reads those outcomes inline the
next time it asks koto what to do.

The coordinator's working context stays clean through convergence, not
just through dispatch. The agent that ran the fan-out never re-derives
or re-reads the delegated work; it reads results and decides. The same
holds at every level of a nested fan-out -- a workflow that is itself a
child converges its own children the same way and carries its own
closed result up to its parent.

## User Journeys

### A solo coordinator converges a fan-out with no extra tooling

An agent driving a koto workflow on its own -- no companion plugins, no
multi-repo workspace -- reaches a step that fans three evaluations out
to child workflows. It dispatches them through the existing flow. When
it next asks koto for its directive, the converge step reports each
child's closed result inline: three outcomes, read directly, no child
log opened. The coordinator scores them and advances. The clean-context
benefit it got from dispatch now extends through convergence.

### A delegate child records its outcome as it completes

An agent assigned one child workflow does the delegated work and
finishes. As part of completing the child, the outcome it reached is
recorded with the completion -- not buried in the transcript. The agent
does nothing special beyond finishing the way koto already expects;
the result rides the same completion it was already going to signal.
The parent picks it up on its next poll.

### A nested coordinator converges and then completes upward

A workflow that is itself a child of a larger fan-out runs its own
sub-fan-out, converges its sub-children by reading their results, and
then completes -- carrying its own closed result up to its parent
through the identical mechanism. Convergence and completion are uniform
at every depth of the tree; the same primitive serves a leaf, a
mid-level coordinator, and the root.

### A workflow completes and releases its resources

A workflow reaches its terminal state and signals completion. Because
completion now carries a closed result rather than only a state name,
the surrounding tooling has a definite, readable end-of-work signal it
can act on -- recording the outcome and reclaiming the workflow's
working space -- instead of inferring completion from an opaque
terminal marker.

## Scope Boundary

**IN:**

- A closed result that travels with a workflow's completion, rather
  than completion recording only a terminal-state name.
- Surfacing children's results to the parent at a converge point in the
  parent's own directive, so convergence is a read.
- Reuse of the machinery koto already ships for fan-out: child-workflow
  creation and parent linkage, the parent's discovery of children
  needing an agent, the claim-and-run path, and the terminal index.
- Uniform behavior at every tree depth -- a leaf child, a mid-level
  coordinator, and the root all complete and converge the same way.

**OUT:**

- The fan-out and dispatch half. Child-workflow creation, parent
  linkage, discovery of children needing an agent, claiming, and
  terminal indexing already exist and are not rebuilt here.
- A new top-level command noun for requests. Convergence rides the
  existing workflow / children / gate model rather than introducing a
  parallel request-object surface an agent has to juggle.
- The exact result-payload schema and the choice between auto-promoting
  a child's terminal evidence into its result versus an explicit result
  step. These are the framing's open design decisions, owned by the
  downstream PRD and design.
- Companion teaching and provisioning layers. A coordinator-bridge
  teaching skill and multi-repo worktree/role provisioning enrich this
  capability when present but are separate work; this feature delivers
  standalone value without them.
- Cleanup-and-resource-reclaim policy. The completion signal makes a
  definite end-of-work event available; what tooling does with it
  (retention, teardown) is gestured at here and decided downstream.
