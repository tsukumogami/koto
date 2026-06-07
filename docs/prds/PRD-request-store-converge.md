---
status: Draft
upstream: docs/briefs/BRIEF-request-store-converge.md
problem: |
  koto can fan a workflow out to child workflows and learn which children
  finished, but not what they produced. A child's completion records only
  a terminal-state name and an outcome classification, not a closed
  result. A coordinator that needs each child's outcome must therefore
  open and read the child's session log, reintroducing the working-context
  load the fan-out was meant to remove.
goals: |
  Make workflow completion carry a typed closed result, and surface
  children's results to their parent at a converge point in the parent's
  own koto next directive. A coordinator converges a fan-out by reading
  results inline, without opening any child log. The same primitive serves
  a leaf, a mid-level coordinator, and the root, so convergence is uniform
  and recursive at every depth of the tree.
motivating_context: |
  koto v0.10.0 already ships the fan-out half: parent-linked child
  workflows, parent discovery of children needing an agent, an epoch-fenced
  claim-and-dispatch path, and a workspace-wide terminal index. That
  machinery answers which children are done. It does not answer what each
  child produced. This PRD specifies the converge half so that a clean
  dispatch is matched by a clean convergence.
---

# PRD: request-store-converge

## Status

Draft

## Problem Statement

koto's coordinator-and-delegates model lets one workflow fan work out to
child workflows. As of v0.10.0 the children are created and linked to the
parent, the parent learns which children still need an agent
(`unassigned_children` in `koto next`), agents claim and drive them through
an epoch-fenced claim path, and a child that reaches a terminal state is
recorded in the workspace-wide terminal index (`_terminal_index.jsonl`).
That machinery answers *which children are done*.

It does not answer *what each child produced*. A child's completion is
recorded as a terminal-state name plus an outcome classification
(`success` / `failure` / `skipped`) — not the result the child reached.
So a coordinator that fanned out three evaluations and now wants to
converge them cannot read the three outcomes from its own directive. To
learn what a child decided, the coordinator must open that child's session
and read its log.

That is the exact cost the fan-out exists to avoid. The point of delegating
to a child is that the coordinator does not carry the child's working
detail. Making the coordinator read each child's log to converge defeats
that separation. The gap is structural, not cosmetic: a coordinator can
dispatch cleanly but cannot *converge* cleanly, because completion is a
bare done-signal rather than a closed result the parent can read. It
affects any agent driving a koto workflow that fans out and then needs the
outcomes back — including a workflow that is itself a child and must hand
its own result upward.

## Goals

- Workflow completion carries a closed result, not only a terminal-state
  name. When a child ends, the outcome it reached travels with the
  completion signal.
- A parent reads its children's results inline at a single converge point
  in its own `koto next` directive — convergence is a read, not a
  re-derivation. No child log is opened to converge.
- The behavior is uniform at every tree depth: a leaf, a mid-level
  coordinator, and the root all complete and converge through the same
  primitive. A workflow that is itself a child converges its own children
  and then carries its own closed result up to its parent identically.
- Convergence stands on the machinery koto already ships for fan-out
  (child creation and linkage, discovery of children needing an agent, the
  claim path, and the terminal index). No parallel request-object surface
  is introduced.
- Completion produces a definite, readable end-of-work signal that
  surrounding tooling can act on, rather than an opaque terminal marker
  that has to be interpreted.

## User Stories

These are coordinator-and-delegate scenarios for agents driving koto
workflows; case descriptions are used where a strict user-story phrasing
would feel forced for an engine feature.

### A solo coordinator converges a fan-out with no extra tooling

An agent driving a koto workflow on its own — no companion plugins, no
multi-repo workspace — reaches a step that fans three evaluations out to
child workflows and dispatches them through the existing flow. When it
next asks koto for its directive, the converge step reports each child's
closed result inline: three outcomes, read directly, no child log opened.
The coordinator scores them and advances. The clean-context benefit it got
from dispatch now extends through convergence.

### A delegate child records its outcome as it completes

An agent assigned one child workflow does the delegated work and finishes.
As part of completing the child, the outcome it reached is recorded with
the completion rather than left buried in the transcript. The agent does
nothing beyond finishing the way koto already expects; the result rides the
same completion it was already going to signal. The parent picks it up on
its next poll.

### A nested coordinator converges, then completes upward

A workflow that is itself a child of a larger fan-out runs its own
sub-fan-out, converges its sub-children by reading their results, and then
completes — carrying its own closed result up to its parent through the
identical mechanism. Convergence and completion are uniform at every depth;
the same primitive serves a leaf, a mid-level coordinator, and the root.

### A coordinator polls a converge point that is not yet satisfied

A coordinator reaches its converge point while one child is still running.
Its `koto next` directive reports the converge point as still blocked,
naming which children's results are outstanding, and does not advance the
parent past convergence. When the last child completes and its result is
recorded, the next poll surfaces all results inline and the converge point
clears. The coordinator never has to guess whether more results are coming.

### Tooling acts on a definite end-of-work signal

A workflow reaches its terminal state and signals completion carrying a
closed result. Surrounding tooling reads that definite end-of-work signal —
the recorded outcome — and can act on it (for example, recording the
outcome or reclaiming the workflow's working space) instead of inferring
completion from an opaque terminal marker. What the tooling does with the
signal is out of scope here; this feature guarantees the signal exists and
is readable.

## Requirements

### Functional

- **R1** — A workflow's completion records a closed result, not only a
  terminal-state name. The result is associated with the completing
  workflow's session and is available after the workflow reaches a terminal
  state.
- **R2** — The result is a typed, minimal envelope: at minimum an outcome
  status and a human-readable summary, with room for an optional structured
  payload. The shape is uniform across all workflows so a parent can read
  any child's result the same way. (Exact field set is design-altitude; see
  Decisions and Trade-offs D2.)
- **R3** — Recording a result does not require an extra mandatory agent
  action beyond the completion the agent already performs. The result is
  carried by, or designated from, the evidence the completion path already
  produces. (The precise promotion mechanism is design-altitude; see D1.)
- **R4** — A parent workflow exposes a converge point. At that point the
  parent's `koto next` directive surfaces the closed results of the
  parent's children inline, so the coordinator reads every child's outcome
  from its own directive without opening any child session.
- **R5** — The converge point stays blocked until the results it waits on
  are recorded. While blocked, the directive identifies which children's
  results are still outstanding. The parent does not advance past the
  converge point until its required children's results are in. The converge
  point reuses koto's existing gate-blocked directive surface rather than
  introducing a new top-level response shape.
- **R6** — Convergence is uniform and recursive. A workflow that is itself
  a child converges its own children through R4–R5 and produces its own
  closed result through R1–R3 for its parent to read. A leaf (no children),
  a mid-level coordinator, and the root all complete and converge through
  the same primitive.
- **R7** — The feature reuses koto's existing fan-out machinery
  (child-workflow creation and parent linkage, discovery of children
  needing an agent, the claim path, and the terminal index). It does not
  introduce a new top-level command noun for requests, and does not rebuild
  any dispatch-side machinery.
- **R8** — Reading and recording results does not require parsing or
  replaying a child's session log. The result is the legible end-of-work
  artifact; convergence consumes results, never transcripts.

### Non-functional

- **R9** — The terminal-index scan path stays lean. The terminal index is
  the hot, append-only structure the parent's discovery scan walks on every
  poll, and its lines are bounded for atomic concurrent appends. A result
  payload of arbitrary size must not be embedded in an index line; if the
  index participates at all, it carries at most a bounded pointer or flag,
  and the converge directive dereferences the full result. (The concrete
  storage-and-pointer mechanism is design-altitude; see D3.)
- **R10** — The result envelope and any new completion/converge events are
  additive to koto's existing event and wire formats. A koto version that
  predates this feature reading a newer log degrades gracefully rather than
  failing — consistent with koto's existing forward-compatible event
  handling.
- **R11** — Convergence is correct under the existing concurrency model.
  Multiple children completing concurrently each record their result
  without corrupting the index or one another's results, and a parent
  polling its converge point observes a consistent set of results
  (no partial or interleaved reads of a single child's result).

## Acceptance Criteria

- [ ] **AC1 (R1)** — After a workflow reaches a terminal state, a closed
  result is retrievable for that workflow's session. A workflow that
  completed before this feature (no result recorded) is distinguishable
  from one that completed with a result.
- [ ] **AC2 (R2)** — A recorded result exposes a typed outcome status and a
  human-readable summary, and accepts an optional structured payload. Two
  different workflows' results are read through the identical accessor with
  no per-workflow special-casing.
- [ ] **AC3 (R3)** — Completing a workflow records its result without the
  agent performing a separate result-submission step beyond the completion
  it already performs. A test that completes a workflow the normal way
  observes a result present afterward.
- [ ] **AC4 (R4)** — A parent with completed children, when it runs
  `koto next` at its converge point, receives each child's closed result
  inline in the directive. The test asserts the results appear in the
  directive output and that no child session log was read to produce them.
- [ ] **AC5 (R5)** — A parent whose converge point has at least one
  outstanding child result receives a blocked converge directive that names
  the outstanding child(ren), and the parent is not advanced past the
  converge point. Once the last required child records its result, the next
  `koto next` clears the converge point and surfaces all results.
- [ ] **AC6 (R5)** — The converge point is surfaced through koto's existing
  gate-blocked directive surface; no new top-level `koto next` response
  variant and no new top-level command noun are introduced. A test inspects
  the directive shape and the command surface to confirm.
- [ ] **AC7 (R6)** — A three-level fan-out (root → mid-level coordinator →
  leaf children) converges end-to-end: leaves record results, the
  mid-level coordinator converges them and records its own result, and the
  root converges the mid-level result — all through the same APIs, with no
  depth-specific code path exercised.
- [ ] **AC8 (R7, R8)** — Converging a fan-out reads zero child session logs:
  a test that fails if any child transcript is opened during convergence
  still passes. Existing dispatch-side machinery (child creation, linkage,
  discovery, claim, terminal indexing) is unchanged by the feature, as
  shown by the pre-existing dispatch tests continuing to pass.
- [ ] **AC9 (R9)** — A result whose payload exceeds the terminal-index line
  bound does not enlarge any index line past its existing bound; the index
  line stays within its current size limit regardless of result payload
  size. A test records a large result and asserts every index line is still
  within bound.
- [ ] **AC10 (R10)** — A koto build without this feature reading a log that
  contains the new result/converge events does not error; the unrecognized
  events are tolerated (graceful degradation), matching koto's existing
  forward-compatible behavior.
- [ ] **AC11 (R11)** — With multiple children completing concurrently, every
  child's result is recorded intact and the parent's converge directive
  reports a consistent, complete set of results with no corruption — a
  concurrency test with N simultaneous completions passes deterministically.

## Out of Scope

- **The fan-out and dispatch half.** Child-workflow creation, parent
  linkage, discovery of children needing an agent, claiming, and terminal
  indexing already exist (koto v0.10.0) and are not rebuilt here.
- **A new top-level command noun for requests.** Convergence rides the
  existing workflow / children / gate model; no parallel request-object
  surface is added (see R7).
- **The exact result-payload schema and the exact promotion mechanism.**
  Whether and how terminal evidence is auto-promoted into a result, and the
  precise field set of the result envelope, are framed here and finalized
  in the downstream design (see Decisions D1, D2, D3).
- **The concrete result-storage location and pointer mechanism.** R9 fixes
  the constraint (the index scan stays lean); where the full result lives
  and how the converge directive dereferences it is a design decision (D3).
- **Companion teaching and provisioning layers.** A coordinator-bridge
  teaching skill and multi-repo worktree/role provisioning enrich this
  capability when present but are separate work; this feature delivers
  standalone koto value without them.
- **Cleanup-and-resource-reclaim policy.** This feature makes a definite
  end-of-work signal available; what tooling does with it (retention,
  teardown, reclaim) is gestured at here and decided downstream.

## Decisions and Trade-offs

These close the open questions the upstream BRIEF deferred to the PRD.

### D1 — Result is carried by the completion path, not an extra step

**Decision:** A workflow's result is carried by, or designated from, the
evidence the completion path already produces, rather than requiring a
separate result-submission action. (Status: assumed under --auto; the
precise mechanism is design-altitude.)

**Alternatives:** (a) auto-promote the terminal evidence the completion
path already writes into the closed result — no extra agent step;
(b) require an explicit, separate result-submission step before or at
completion.

**Why:** koto already writes terminal evidence on the completion path and
already classifies a typed outcome on completion. A separate submission
step adds an agent round-trip that the fan-out exists to avoid, and creates
a failure mode where a workflow completes without a result. Carrying the
result on the path the agent already takes keeps R3 true. The exact
designation mechanism (which evidence kind, how the result is marked) is
left to the design.

### D2 — Typed, minimal result envelope over free-form JSON

**Decision:** The result is a typed, minimal envelope — outcome status plus
a human-readable summary, with an optional structured payload — not an
opaque free-form JSON blob. (Status: assumed under --auto; exact field set
is design-altitude.)

**Alternatives:** (a) a typed minimal envelope with a small fixed core and
an optional payload; (b) a free-form JSON blob the producer fills however
it likes.

**Why:** A parent must read any child's result uniformly (R2, R6). koto's
own idiom is typed — its completion outcome is a typed enum chosen over a
stringly-typed value precisely so consumers can match it exhaustively and
the wire format stays stable. A free-form blob maximizes producer freedom
but forces every converging parent to know each child's private shape,
defeating uniform convergence. A minimal envelope keeps the common read
path typed while the optional payload preserves flexibility. The exact
fields are finalized at design.

### D3 — Index stays lean; result location deferred to design

**Decision:** The terminal-index scan path stays lean (R9): the full result
does not live in an index line. The PRD fixes this constraint and defers
the concrete storage location and pointer/dereference mechanism to the
downstream design. (Status: deferred to design.)

**Alternatives considered (for the design to resolve):** (a) result lives
with the child session, the index carries at most a bounded pointer/flag,
and the parent's converge directive dereferences and inlines the full
result; (b) result embedded directly in the index line — rejected at PRD
altitude because the index is the hot, append-only, line-bounded structure
the discovery scan walks every poll.

**Why deferred:** The constraint (lean scan path) is a requirement and is
recorded as R9. The mechanism that satisfies it (exact storage, pointer
format, dereference path) is an implementation-architecture choice and
belongs in the design, not the requirements.

### D4 — Reuse existing surfaces; no new command noun

**Decision:** Convergence reuses koto's existing children, gate, and
terminal-index surfaces. No new top-level `koto request` command noun and
no new top-level `koto next` response variant are introduced. (Status:
confirmed — directly inherited from the BRIEF scope boundary.)

**Alternatives:** (a) reuse the existing gate-blocked directive surface for
the converge point; (b) introduce a dedicated request-object command and
response surface.

**Why:** koto's `koto next` already returns a gate-blocked directive with
structured blocking conditions and a children list — a converge point that
stays blocked until results are in is a natural fit. A parallel
request-object surface is a second model an agent has to juggle, against
koto's substrate-agnostic, minimal-surface design. The reserved
`request_store` configuration namespace already anticipates wiring
convergence into existing structures rather than a new noun.

### D5 — Complexity classification: Complex

**Decision:** This PRD is classified **Complex** and routes to the design
workflow before planning.

**Why:** The feature changes koto's engine substrate — it adds a result
envelope and a converge-gate semantics to workflow completion, touches the
closed event family and the hot terminal-index scan path, and leaves three
genuine architectural decisions open (D1 promotion mechanism, D2 envelope
fields, D3 result storage/pointer). It is not a localized change with one
obvious implementation. A technical design is needed to resolve D1–D3 and
specify the event/storage shapes before the work can be decomposed into
issues.

## Known Limitations

- The PRD intentionally leaves three architectural decisions open (D1
  promotion mechanism, D2 envelope field set, D3 result storage and pointer
  mechanism). Downstream planning cannot begin until the design resolves
  them; this is by design for a Complex feature, not an omission.
- Standalone koto value is the contract: a solo coordinator converges
  without any companion tooling. Richer multi-repo and teaching-layer
  experiences are explicitly separate work and are not guaranteed by this
  PRD.
