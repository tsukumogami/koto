---
status: Draft
problem: |
  koto decides whether a phase's long-form instructions ride in a `koto next`
  response by counting entries into a state rather than deliveries of that
  state's instructions. The rule therefore re-sends to an agent that is standing
  still on a blocked gate, withholds from one arriving by rewind, and is not
  applied at all on the directed-transition path. A third class of failure --
  an agent whose context was compacted, or a fresh process resuming an existing
  session -- cannot be fixed by any counting rule, because the session log
  records nothing about who is attached to it, and koto offers no read-only way
  to retrieve the instructions back.
goals: |
  Instruction delivery becomes something an agent can rely on. The instructions
  arrive on first delivery, stop arriving once koto has demonstrably delivered
  them, arrive again when the agent is sent back to redo a phase, and follow one
  rule across every path a workflow moves by. Where no rule can know whether the
  caller still holds them, a read-only call returns them without moving the
  workflow, and an agent that has lost everything else still learns that the call
  exists.
upstream: docs/briefs/BRIEF-inline-phase-details.md
source_issue: 90
motivating_context: |
  Issue #90 requested this behavior as a new feature. It shipped underneath the
  issue in PR #109 four days after filing, closing #102 and never citing #90,
  and was codified as R9 of PRD-koto-next-output-contract. The issue author
  audited the shipped code twice; a two-round exploration then reproduced every
  suspected defect against a binary built from current source. This PRD amends
  that requirement rather than introducing a new one.
---

# PRD: phase instructions an agent can rely on

## Status

Draft

Authored under `/scope`'s tactical chain from `BRIEF-inline-phase-details`. The
requirements here are deliberately silent on where the read-only retrieval lives
and what it is called; that is the DESIGN's decision, and the exploration left
three costed candidates for it.

## Problem Statement

A koto template can attach long-form instructions to a phase by splitting the
phase body on a marker, and `koto next` decides per response whether to include
them. It decides by counting how many times the workflow has *entered* the
state. What it needs to know is how many times it has *delivered* that state's
instructions. Those two quantities come apart, and they come apart in both
directions, so the behavior inverts the intent on the cases that matter.

A tick that evaluates gates, fails them, and does not transition enters nothing,
so the count never moves and the instructions are re-sent on every tick for as
long as the agent stays blocked — precisely the repeat case the suppression
exists to prevent. A rewind is an entry, so a state the workflow is being sent
back to already sits above the threshold, and the instructions are withheld from
an agent that has just been told to redo that phase. The directed-transition
path never applies the check at all, so today's contract is not "first delivery
only" but "first delivery only, except under a directed transition, where it is
always on."

Beneath the counting sits a class of failure no counting rule reaches. koto is
being asked whether the caller still holds the instructions, using a log that
records only where the workflow has been. Those are the same question only while
one uninterrupted process drives a session end to end. A cold-restart respawn
continues on the predecessor's log with its counts intact, so a fresh
zero-context agent inherits a delivery it never received. Context compaction is
worse, because it leaves no event at all, and the payload in question is a tool
result — content the platform documents as compaction-eligible and not
guaranteed to survive a turn.

That is why the retrieval half is not a convenience. koto has no read-only way
back to a phase's instructions. `koto status` is genuinely side-effect-free and
returns neither the directive nor the instructions. `koto next --full` returns
them and also evaluates gates, appends events, re-executes any `default_action`
shell command, can auto-advance a routing state, and can clean up a terminal
session — so an agent recovering from context loss cannot know whether its own
recovery call will move the workflow underneath it. The remaining options are to
pass `--full` on every tick, which discards the entire saving, or to read the
template file by hand, which is the file read the mechanism exists to remove.

The cost is measured, not projected. In a recorded run of a fourteen-iteration
sweep, a phase carrying a 7,140-character procedure emitted it once on the first
iteration; the following thirteen received a 101-character directive and nothing
else, and one of them sat through fourteen consecutive gate-blocked ticks with
the procedure suppressed throughout.

## Goals

- One rule governs whether a phase's instructions ride in a response, and it
  holds across every path a workflow moves by.
- An agent that already received a phase's instructions does not receive them
  again while it stays on that phase.
- An agent sent back to redo a phase receives that phase's instructions.
- An agent that no longer holds the instructions can retrieve them with a call
  that provably changes nothing about the workflow.
- An agent that has lost its context still learns that the retrieval exists,
  through a channel that reaches it on every response.
- Templates that attach no instructions behave exactly as they do today.
- The change lands without a new state file, without a state-schema version
  bump, and without touching the frozen wire-format surface.

## User Stories

**As an agent driving a gated phase**, I want the phase's instructions to stop
arriving once I have them, so that a long blocked loop does not re-send the same
several-thousand-character block on every tick.

**As an agent sent back by a rewind**, I want the instructions for the phase I
am being told to redo, so that "go back and do this again" is not the one moment
koto decides I do not need to know how.

**As an agent whose context was compacted**, I want to retrieve the current
phase's instructions with a call that does not evaluate a gate, run a command,
or advance the workflow, so that recovering from my own context loss cannot
change the state of the work.

**As a freshly respawned agent resuming an existing session**, I want the same
retrieval, because the log will tell koto I already received instructions that in
fact went to a process that no longer exists.

**As a template author**, I want to put a long procedure in the phase that needs
it rather than in a separate file the directive tells the agent to read, so that
I stop maintaining a parallel file to work around a delivery rule I cannot rely
on over a loop.

**As a koto maintainer**, I want the change covered by tests at the level the
behavior actually lives, because the suppression logic has no direct test
coverage today beyond its counting primitive.

## Requirements

### Functional — the delivery rule

- **R1.** The decision to include a phase's instructions in a response is keyed
  on whether koto has already delivered that phase's instructions to a caller
  during the current occupancy of that phase, not on how many times the workflow
  has entered it.
- **R2.** A response that does not advance the workflow, and that follows a
  response which already carried the current phase's instructions, omits them.
  This covers the gate-blocked re-tick and any other non-advancing repeat.
- **R3.** The first response after arriving at a phase carries that phase's
  instructions, for every way of arriving: a conditional transition, an
  unconditional transition, a directed transition, a rewind, and workflow
  initialization at the initial state.
- **R4.** R1 through R3 hold identically on the directed-transition path and on
  the advance path. There is one rule, not two.
- **R5.** An explicit override remains available that includes the instructions
  regardless of the rule, preserving the behavior of the existing `--full` flag
  for callers that already depend on it.
- **R6.** A phase that declares no instructions produces responses byte-identical
  to those koto produces today.

### Functional — the read-only retrieval

- **R7.** koto offers a way to retrieve the current phase's instructions that is
  keyed by the workflow name alone and requires no argument the caller would have
  had to memorize from an earlier response.
- **R8.** The retrieval returns, at minimum: the current phase's identifier, its
  directive, and its instructions, with all runtime and template variables
  substituted exactly as `koto next` substitutes them. Un-substituted placeholder
  text does not satisfy this requirement.
- **R9.** The retrieval returns the evidence schema the current phase expects,
  when the phase declares one, so a recovering agent knows what it must submit.
- **R10.** The retrieval returns the instructions whether or not the delivery rule
  would have suppressed them, and retrieving them does not itself count as a
  delivery for the purposes of R1.
- **R11.** The retrieval performs none of the following: appending any event to
  the session log, evaluating any gate, executing a phase's default action or any
  other shell command, transitioning the workflow, cleaning up a session at a
  terminal phase, writing to the request store, or advancing a discovery cursor.
  The list is exhaustive and each item is separately checkable.
- **R12.** The retrieval does not block on a lock held by another process. A
  respawned agent must be able to retrieve instructions while its predecessor is
  still running.
- **R13.** The retrieval's error behavior — for an unknown workflow, an
  unreadable or corrupt session, and a phase that declares no instructions —
  follows koto's existing structured error conventions, and a phase with no
  instructions is not an error.

### Functional — discoverability

- **R14.** A pointer to the retrieval reaches the agent on a channel that is
  present in every non-terminal response, so an agent that has lost its context
  learns the retrieval exists without having retained anything.
- **R15.** The pointer does not displace or truncate the phase's own directive
  text.

### Non-functional

- **R16.** No new state file is introduced, and the state-file schema version is
  not bumped. This is the constraint R9 of `PRD-koto-next-output-contract`
  already imposes, carried forward unchanged.
- **R17.** The frozen wire-format surface that `koto-stability-tests` pins is
  unchanged.
- **R18.** Per-call cost stays within the same order as today's derivation: one
  additional pass over an already-read event list, with no new file reads on the
  `koto next` path.
- **R19.** The behavior is covered by tests at the level it lives. The response
  construction and the delivery rule are exercised directly, not only through
  the counting primitive underneath them, which is where today's coverage stops.

### Downstream obligations

- **R20.** `plugins/koto-skills/skills/koto-user/references/response-shapes.md`
  and `command-reference.md` describe the delivery rule and the retrieval as
  shipped. These document the current contract and would otherwise be wrong.
- **R21.** `plugins/koto-skills/skills/koto-author/SKILL.md` and
  `references/template-format.md` describe what an author can now rely on.
- **R22.** `docs/guides/cli-usage.md` and
  `plugins/koto-skills/.cursor/rules/koto.mdc` reflect the changed behavior and
  any new surface.
- **R23.** Both skills retain at least one eval, and the evals covering the
  delivery contract assert the changed behavior rather than the old one.
- **R24.** `CHANGELOG.md` records the fix and any added surface under the
  repository's existing convention.

## Acceptance Criteria

### The delivery rule

- [ ] A workflow reaches a gated phase whose gate fails. The first response
      carries the instructions; a second `koto next` that evaluates the same
      failing gate and does not transition returns a response with no
      instructions field.
- [ ] The same workflow, after the gate passes and the workflow later loops back
      to that phase, carries the instructions again on the arrival response.
- [ ] A workflow is advanced past a phase and then rewound into it. The next
      response carries that phase's instructions.
- [ ] Two consecutive directed transitions into the same phase: the first
      carries the instructions, the second does not.
- [ ] A directed transition into a phase the workflow has never occupied carries
      the instructions.
- [ ] `koto init` followed by a first `koto next` carries the initial phase's
      instructions.
- [ ] A batch-spawned child's first `koto next` carries its initial phase's
      instructions.
- [ ] The existing override flag returns the instructions on a response where the
      rule would otherwise have omitted them.
- [ ] A template whose phases declare no instructions produces responses with no
      instructions field, on every path above.

### The read-only retrieval

- [ ] The retrieval, invoked with only the workflow name, returns the current
      phase's identifier, directive, and instructions.
- [ ] A directive or instructions block containing a runtime variable comes back
      with the variable substituted, matching what `koto next` would have
      returned for the same phase.
- [ ] The retrieval returns the instructions on a phase where the delivery rule
      is currently suppressing them.
- [ ] Retrieving does not change what the next `koto next` returns: a retrieval
      between two `koto next` calls leaves the second call's response identical
      to what it would have been without the retrieval.
- [ ] The session state file is byte-identical before and after a retrieval.
- [ ] A retrieval against a phase whose gate is a shell command does not execute
      that command, verified by a gate command with an observable side effect.
- [ ] A retrieval against a phase carrying a default action does not execute it,
      verified the same way.
- [ ] A retrieval against a workflow at a terminal phase does not clean up the
      session; the session directory still exists afterwards.
- [ ] A retrieval succeeds while a second process holds the session, without
      blocking.
- [ ] A retrieval against an unknown workflow name returns a structured error and
      a non-zero exit code consistent with koto's existing conventions.
- [ ] A retrieval against a phase that declares no instructions succeeds and
      reports their absence rather than erroring.
- [ ] The retrieval returns the phase's expected-evidence schema when the phase
      declares one.

### Discoverability

- [ ] Every non-terminal `koto next` response carries a pointer naming the
      retrieval, on a field present in all such responses.
- [ ] The phase's own directive text is present and unaltered in a response that
      also carries the pointer.

### Constraints and downstream

- [ ] No file is added under the session directory beyond those koto writes
      today, and the state-file schema version is unchanged.
- [ ] `koto-stability-tests` passes unmodified.
- [ ] Tests exercise the response construction and the delivery rule directly,
      covering at minimum: the non-advancing repeat, the rewind arrival, and both
      directed-transition cases.
- [ ] `cargo fmt --check`, `cargo clippy -D warnings`, and the full test suite
      pass.
- [ ] `koto-user`'s response-shapes and command-reference documents describe the
      shipped rule and the retrieval.
- [ ] `koto-author`'s SKILL.md and template-format reference describe what an
      author can rely on.
- [ ] `docs/guides/cli-usage.md` and the Cursor rules file match the shipped
      behavior.
- [ ] Every skill under `plugins/*/skills/*/` still has at least one eval, and
      any eval asserting the old delivery behavior is updated.
- [ ] `CHANGELOG.md` records the change.
- [ ] `wip/` is empty and no committed file references a `wip/` path.

## Decisions and Trade-offs

**The directed-transition path is treated as a defect, not a carve-out.** It
could be read as deliberate: an operator issuing a directed transition is making
an explicit choice and arguably always wants full context. Nothing in the code or
the design documents says so, and the cost of the ambiguity is that koto's
documented contract is false — the skills tell agents the instructions arrive on
first visit and are omitted afterwards, which is not what this path does. R4
resolves it toward one uniform rule. The alternative reading is recorded here so
the DESIGN can overturn it with a stated reason rather than rediscover it.

**The respawn case is assigned to the retrieval, not to the counting rule.**
A cold-restart respawn continues on the predecessor's session log without
resetting anything, so a fresh zero-context agent inherits a delivery it never
received. No rule derived from the log can distinguish that agent from the one
that received the instructions, because the log carries no notion of who is
attached. Silent context compaction is the same problem with no event at all.
Both are therefore requirements on R7 through R14 rather than on R1 through R6.
This is the load-bearing decision in the PRD: it is what makes the retrieval
mandatory rather than a convenience, and it is why the retrieval's side-effect
freedom (R11) is enumerated exhaustively rather than described.

**Batch child spawn and batch child retry are not in the defect set.** Both
re-initialize the child's session, which resets the count cleanly and delivers
the instructions on the child's first arrival. An earlier reading treated retry
as a break case; it is not, because the retry path deletes the child's log before
reinitializing.

**The gate-fail-with-accepts fallthrough needs no dedicated criterion.** It sits
on the directed-transition path and inherits its missing check, but it is
unreachable in production today because the directed-transition handler evaluates
zero gates, so the branch that would trigger it cannot fire. Fixing R4 covers it
structurally. Requiring the directed-transition path to evaluate gates would
reach it, and that is a materially larger behavior change than this PRD's problem
statement supports.

**Terminal-phase instructions stay out of scope, and the gap is recorded.** The
terminal response variant carries no instructions field, and has never carried a
directive either — so this is not an asymmetry this work introduces. But the
template compiler splits every declared phase at the marker, terminal ones
included, which means an author can write instructions under a terminal phase
today and have them silently reach nobody. This PRD neither closes that nor
pretends it is closed; it is named here so a future issue can decide between
rejecting it at compile time and delivering it.

**Whether the retrieval reports gate state is left to the DESIGN.** The
last-known gate outcome is derivable read-only from the event log via machinery
the dashboard already uses, so including it is possible without evaluating
anything. It would also be stale by construction, and a recovering agent could
read it as live. R9 requires the evidence schema and stops there; the DESIGN
decides whether a stale-labelled gate outcome earns its place.

**Where the retrieval lives and what it is called are DESIGN questions.** The
requirements above name a capability and its contract, not a surface. The
exploration costed three candidates and found a strong vocabulary signal against
one of the names the source issue proposed; that evidence belongs to the DESIGN's
decision, not to this document.

## Out of Scope

- **Auto-advance discarding the phases it crosses.** A `koto next` that advances
  through an intermediate phase surfaces neither that phase's instructions nor
  its directive. It predates this mechanism, is broader than it, and would
  mis-frame as a delivery regression. Filed separately.
- **Two consecutive rewinds moving a session forward.** A defect in how the
  rewind target is selected, which makes an early phase unreachable once the
  workflow has passed it. Adjacent — a rewind-aware change here touches the same
  function — but unrelated to instruction delivery. Filed separately.
- **`accepts:` not gating advancement.** A transition with no condition fires
  regardless of any `accepts` block, so a chain of phases an author believes are
  interactive can run to completion in one call. Filed separately.
- **The migration scan's output on every invocation.** It obstructed measurement
  during the exploration and is very likely the already-open issue #193.
- **Retrofitting existing templates onto the mechanism.** Adoption work in other
  repositories, downstream of anything koto changes here.
- **Requiring the directed-transition path to evaluate gates.** A larger behavior
  change than this problem statement supports; see Decisions.
- **Changing the shared visit-count derivation's own semantics.** It has a second
  consumer unrelated to instruction delivery, so it is a constraint on the
  solution rather than a target of it.

## Known Limitations

The delivery rule can still be wrong about whether a caller holds the
instructions. R1 narrows the gap by tracking deliveries instead of entries, but
no rule derived from the session log can detect that the process reading a
response is not the one that read the last, or that a context was compacted
between two ticks. The retrieval is the mitigation, and it is only as good as an
agent's willingness to reach for it — which is why R14 exists and why the pointer
rides a channel that survives everything else being lost. There is no published
evidence on how reliably agents notice a missing procedure rather than
confabulating one, so the residual risk is real and unquantified.
