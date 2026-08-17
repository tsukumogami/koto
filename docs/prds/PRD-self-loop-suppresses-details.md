---
schema: prd/v1
status: In Progress
upstream: docs/briefs/BRIEF-self-loop-suppresses-details.md
source_issue: 90
problem: |
  koto includes a phase's long-form instructions in the first response after
  each entry into that phase, and it counts a phase transitioning to itself as
  an entry. An agent looping inside a phase is therefore re-sent a procedure it
  already holds on every lap, which is the cost the suppression exists to avoid.
  koto#90's acceptance criterion 3 says a self-loop must omit the instructions;
  the shipped rule was settled the other way during a later scoping pass, and
  the issue's author has ruled that the criterion governs.
goals: |
  The delivery decision keys on whether the agent arrived from somewhere else,
  not on whether an entry event fired. A lap around a loop costs the directive
  alone; every genuine arrival, and every deliberate send-back, still carries the
  instructions; and the read-only retrieval still returns them on demand. Every
  surface that describes the rule — agent-facing skills, the CLI guide, and the
  durable PRD and DESIGN for the shipped mechanism — describes the rule that
  ships.
motivating_context: |
  Five of koto#90's six acceptance criteria are already met on merged `main`.
  This PRD covers the sixth, and the reversal it requires falsifies a normative
  definition in an accepted PRD and an argued passage in a current DESIGN. Both
  have to move with the code, or the next reader re-derives the discarded answer.
---

# PRD: a lap around a loop is not a new arrival

## Status

In Progress

## Problem Statement

A koto phase can carry long-form instructions after a `<!-- details -->` marker,
and koto decides per response whether to include them. The rule is: deliver on
the first response after the workflow enters the phase, omit afterwards. The
window that "afterwards" covers is bounded by state-entry events — it opens when
one names the phase and closes at the next one naming any phase, including the
same one.

A phase that transitions to itself appends a state-entry event naming itself. So
under the shipped rule a self-transition closes one window and opens another,
and the instructions go out again. The agent has not arrived anywhere. It is on
the next lap of a loop it has been executing continuously and already holds the
procedure it was handed on lap one. On the workflow that prompted koto#90's
audit — a fourteen-week sweep cycling three phases once per week — that is a
seven-thousand-character block re-sent thirteen times to an agent that never
left the phase.

Three groups are affected. An agent driving any long loop pays for the procedure
once per lap instead of once. A template author cannot predict what a
self-transition emits without reading the engine, because the rule is stated in
terms of an internal boundary rather than in terms of arrivals. And a maintainer
reading the durable record — `docs/prds/PRD-inline-phase-details.md`, whose
Definitions section states the boundary normatively, and
`docs/designs/current/DESIGN-inline-phase-details.md`, which carries a passage
headed "A contradiction in the PRD was corrected" resolving koto#90's criterion
against delivery — gets a confident, argued answer that the issue's author has
now ruled against.

It matters now because the rest of koto#90 is met. The gate-blocked retry stopped
re-sending, the rewind case was fixed, and a read-only retrieval shipped. The
self-loop is the one criterion left, and it is the one the issue's own reproduction
script measures.

## Goals

- An agent looping inside one phase receives that phase's instructions once, and
  the per-lap cost of the loop stops scaling with the number of laps.
- Every other way of arriving at a phase still delivers, so nothing an agent
  needs becomes harder to get than it is today.
- A deliberate send-back still delivers, because an agent told to redo a phase is
  being asked to start the procedure over.
- The rule is stated once, in terms a template author can apply to their own
  template, and every surface that states it agrees with the code.
- The change costs no new session-log content and no schema version bump.

## User Stories

- **As an agent driving a long-running loop**, I want the phase's procedure once
  rather than once per lap, so that a fourteen-iteration sweep costs one delivery
  instead of fourteen.
- **As an agent that has just arrived somewhere new**, I want the destination
  phase's procedure, so that I can execute it without a separate read.
- **As an agent that was rewound into a phase**, I want its procedure again, so
  that I can redo the work with the instructions in front of me rather than from
  memory of a pass that was judged wrong.
- **As an agent that lost the procedure to context compaction mid-loop**, I want
  a call that returns it without advancing the workflow, because under this rule
  no further lap will bring it back.
- **As a template author writing a retry loop**, I want to know from the
  koto-author reference exactly what a self-transition repeats, so that I can
  write directive text that does not assume the procedure comes with it.
- **As a maintainer auditing the delivery rule**, I want the durable design record
  to describe the behavior that ships and to record where it was reversed, so
  that I neither trust a stale definition nor re-derive a discarded answer.

## Requirements

### Definitions

Three terms carry weight below. They are defined here because the shipped
documents use one word, "occupancy", for two things this change separates.

**State-entry event.** An event that records the workflow entering a phase: a
transition (conditional, unconditional, or `skip_if`-driven), a directed
transition, or a rewind. Each records the phase entered and, except at workflow
initialization, the phase left.

**Self-entry.** A state-entry event that is a transition or a directed
transition and whose recorded source phase equals its target phase. A rewind is
never a self-entry, whatever phases it records — see R6. Initialization is never
a self-entry, because it records no source.

**Delivery window.** The stretch of a session log in which a phase's
instructions count as already delivered. It opens at the most recent state-entry
event naming the phase that is *not* a self-entry, and runs to the end of the
log. When no such event exists, the window is the whole log. This is the only
boundary this PRD moves.

**Epoch.** The stretch used for the gate-blocked classification the dashboard and
the `/workflows` projection read, and, separately, for deriving which evidence is
in scope for the current phase. An epoch opens at the most recent state-entry
event naming the phase, self-entry included. This boundary does not move — see
R15.

### Functional — the delivery rule

- **R1.** A response includes the current phase's instructions when no delivery
  of that phase's instructions has been recorded inside the phase's current
  delivery window, and omits them when one has. A self-entry does not open a new
  delivery window, so a delivery recorded before it still counts after it.
- **R2.** The first response for the initial phase of a freshly initialized
  workflow carries that phase's instructions.
- **R3.** The first response inside a delivery window opened by an entry from a
  different phase carries that phase's instructions. This holds for a conditional
  transition, an unconditional transition, a `skip_if`-driven transition, a
  directed transition into a phase the workflow was not already in, a loop-back
  into a phase occupied earlier in the session, and the final hop of a multi-hop
  advance — including a tick that leaves phase P, passes through another phase,
  and returns to P within the same tick, whose last entry event records a
  different source.
- **R4.** A response whose phase was entered by a transition from that same phase
  omits the instructions when the enclosing delivery window already carries a
  recorded delivery. This holds however the self-transition was reached and
  however many consecutive self-transitions precede it.
- **R5.** A response whose phase was entered by an explicitly targeted transition
  into the phase the workflow already occupied behaves exactly as R4 does. This
  arrival is reachable only for a template that declares the phase as its own
  transition target.
- **R6.** A rewind opens a delivery window, whether or not the phase it records
  as the source equals the phase it records as the target, so the response after
  a rewind carries the instructions. A rewind is an instruction to redo rather
  than to continue, and the answer must not depend on which phases the rewind
  happens to record.
- **R7.** A response that appends no state-entry event omits the instructions
  when the current delivery window already carries a recorded delivery. This
  covers the gate-blocked re-tick, the evidence-required tick, the cycle-detected
  stop, the confirm tick, and any other non-advancing response, and is unchanged
  from the shipped behavior.
- **R8.** An explicit override remains available that includes the instructions
  regardless of R1, preserving the behavior of the existing `--full` flag. An
  override that delivers records the delivery, so a later response inside the
  same delivery window omits them under R1. The recording is observable: without
  it, a delivery window whose only delivery came from an override would deliver
  again on its next response.
- **R9.** The read-only retrieval on `koto status` returns the current phase's
  substituted directive, instructions and evidence schema regardless of delivery
  history, appends nothing to the session log, and takes no lock. Unchanged.
- **R10.** R1 through R8 hold identically on both response-construction paths —
  the natural-advancement path and the directed-transition path. There is one
  rule, not two.
- **R11.** Every response for a non-terminal phase that declares instructions
  carries the pointer naming the retrieval, whether or not that response carries
  the instructions. This includes the responses R4 and R5 suppress.
- **R12.** A terminal phase carries no instructions and no pointer on any path.
  Unchanged, and stated because R2, R3 and R11 would otherwise read as covering
  it.
- **R13.** Events that are not state entries — gate evaluations and overrides,
  evidence submissions, context and decision records, scheduler and batch
  bookkeeping, respawn and wake — neither open a delivery window nor close one.
  Recording a gate override does not re-deliver, because the workflow has not
  moved.

### Functional — what must not move

- **R14.** For a template whose phases declare no instructions, the full response
  body is byte-identical to what the pre-change binary produces, for the same
  template and the same sequence of calls, on every path above.
- **R15.** The epoch boundary defined above is unchanged: a self-entry still
  closes the epoch used for the gate-blocked classification and the epoch used to
  derive the evidence in scope. Only the delivery window's boundary moves.
- **R16.** No new event type, no new field on an existing event, and no change to
  the session schema version. A session log written by the current release is
  indistinguishable, to the changed binary, from one the changed binary wrote
  itself; and a log written by the changed binary is read by the current release
  with no error. The delivery answers a changed binary gives on an old log are
  the answers this PRD specifies, not the answers the old binary gave — see
  Known Limitations.
- **R17.** The change adds no file read and no file write to any path, relative
  to the shipped binary.

### Non-functional

- **R18.** After this lands, no committed file outside this PRD and its own
  BRIEF *asserts* that a self-transition re-delivers a phase's instructions, or
  defines the delivery boundary as closing at a self-entry. The surfaces that do
  so today are enumerated in the acceptance criteria. Prose that reports the old
  rule as history — which R19 requires — is not an assertion of it, and is
  exempt; the distinction is whether a reader would take the sentence as
  describing what koto does now.
- **R19.** The durable record says that the rule was reversed, by what, and why.
  Deleting the passage that argued the other way is not sufficient: the reversal
  is the fact a future reader needs.
- **R20.** The koto-user eval suite keeps its existing coverage. The eval
  asserting that a second non-advancing tick omits the instructions, and the one
  asserting that a rewind arrival re-delivers, both keep their scenarios and their
  verdicts. Rewording an explanation that appeals to the boundary this PRD moves
  is in scope; removing an assertion is not. One new eval asserts the self-loop
  case.
- **R21.** `CHANGELOG.md` records the change under `[Unreleased]`.

## Acceptance Criteria

Each criterion names what decides it. "Delivery test" is
`tests/instructions_delivery_test.rs`; "unit" is the test module in
`src/engine/persistence.rs`, whose cases are synthetic event lists rather than
processes; "status test" is `tests/status_phase_retrieval_test.rs`; "baseline" is
`tests/next_response_baseline.rs` against
`tests/fixtures/next-response-baseline/instruction-free.json`. Criteria name
behavior, not function names: which function decides the rule is the DESIGN's
call.

### The rule

- [ ] Delivery test: on a template whose `implement` phase declares instructions
      and a self-transition, arriving at `implement` from `gather` returns a body
      carrying the instructions, and the next tick — a self-transition — returns a
      body with no `details` key. (R4)
- [ ] Delivery test: two successive self-transition ticks each return no `details`
      key. Both responses are asserted, not only the second. (R4)
- [ ] Delivery test: a directed transition into `implement` carries the
      instructions; a second directed transition into `implement`, issued while
      the workflow is already at `implement`, returns no `details` key. (R5)
- [ ] Delivery test: a tick that leaves `implement`, passes through a phase with
      an unconditional transition, and arrives back at `implement` within the same
      tick carries the instructions. `DELIVERY_TEMPLATE` cannot express this, so
      this criterion is satisfied with an added template constant. (R3)
- [ ] Unit: given an entry into a phase from a different phase, a recorded
      delivery for it, and then a transition from that phase to itself, the
      delivery decision reports the instructions as already delivered. The same
      holds with a directed transition from the phase to itself in place of the
      self-transition. (R1, R4, R5)
- [ ] Unit: given an entry into a phase from a different phase and then a
      self-entry, with **no** recorded delivery anywhere in the log, the delivery
      decision reports the instructions as not yet delivered. This is the case
      that discriminates the two candidate implementations: a rule that suppresses
      on the shape of the entry event alone would strand an agent whose delivery
      record was lost to a crash between printing a response and recording it.
      (R1)
- [ ] Unit: given a recorded delivery, then a rewind whose source and target are
      both the phase in question, the delivery decision reports the instructions
      as not yet delivered. (R6)
- [ ] Delivery test: a `koto rewind` issued immediately after a self-transition —
      which records the same phase as both source and target — is followed by a
      `koto next` carrying that phase's instructions. The predicate-level
      criterion above does not cover this: a change confined to the shared
      decision, with no wiring at the call sites, passes it and fails here. (R6)

### The pointer and the terminal carve-out

- [ ] Delivery test: the self-transition response that omits the instructions
      still carries the recovery pointer at the head of its `directive`. Likewise
      the directed self-transition response. (R11)
- [ ] R12 holds by construction rather than by wiring: the terminal response
      variant has no instructions field and the combinators pass it through
      untouched. It is pinned by the three existing unit tests in
      `src/cli/next_types.rs` covering pointer-prefix, suppression and
      carries-details against the terminal variant, all of which pass with their
      assertions unmodified. No new test is required. (R12)

### The override and the retrieval

- [ ] Delivery test: `full_override_returns_details_on_a_response_that_would_otherwise_be_suppressed`
      and `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`
      pass with their assertions unmodified. (R8)
- [ ] Delivery test: `--full` on a self-transition tick returns the instructions,
      and a following non-advancing tick omits them. This is a plain regression
      check, not a test of the recording clause: inside a self-entry window the
      arrival that opened the window already recorded a delivery, so the record
      the override writes can never be the load-bearing one there. The recording
      clause is checked by
      `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`,
      which applies the override to the first response of a window, and by
      nothing else. (R8)
- [ ] Status test: `status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected`
      is updated rather than left alone. Its setup reaches `implement` and then
      issues `--to implement` while already there, which R5 turns from a
      delivering arrival into a suppressing one; the criterion is met by routing
      the setup through a genuine arrival, and the assertion that the first
      response at `implement` carries the instructions survives unchanged. This is
      the third file whose existing assertions this change disturbs. (R5, R9)
- [ ] Status test: after a self-transition tick that omitted the instructions,
      `koto status` returns them, the session state file is byte-identical before
      and after that call, and the following plain tick still omits. (R9)

### Initial entry and non-entry events

- [ ] Delivery test: `init_then_first_tick_carries_details_for_initial_phase` and
      `batch_spawned_child_first_tick_carries_details_for_its_own_initial_phase`
      pass with their assertions unmodified. (R2)
- [ ] Delivery test: on a phase that has already delivered and whose repeat tick
      omits, recording a decision — an event that is not a state entry and does
      not unblock anything — leaves the next response omitting, and still
      carrying the pointer. A gate override is the wrong instrument here: on the
      gated template in that file an override unblocks the phase and the next
      response is terminal, which has no instructions field to assert against.
      (R13)

### What must not have moved

- [ ] Delivery test: the conditional-arrival, unconditional-arrival, loop-back,
      rewind-arrival, gate-blocked-repeat and directed-then-non-advancing cases
      pass with their assertions unmodified. (R3, R6, R7, R10) The PR modifies no existing test in
      this file other than the two that assert self-transition and repeated
      directed-transition re-delivery. Added tests and added template constants
      are not modifications. An edit to an existing shared template constant is
      permitted only if every other test in the file passes unchanged; a new
      constant is preferred.
- [ ] Unit: every existing case in the module that does not concern a self-entry
      passes with its assertions unmodified. Exactly two invert:
      `instructions_delivered_resets_on_a_self_transition` and the directed
      half of `instructions_delivered_resets_on_arrival_by_directed_transition`.
      Checkable as: `git diff -U0 origin/main...HEAD -- src/engine/persistence.rs`
      shows changed `assert` lines only inside those two cases.
- [ ] Baseline: `cargo test --test next_response_baseline` passes, and
      `git diff origin/main...HEAD -- tests/fixtures/next-response-baseline/instruction-free.json`
      contains no changed line inside a `"stdout"` value. The fixture's `notes`
      and `description` strings may change, in lockstep with the matching strings
      in `tests/next_response_baseline.rs`; nothing else in the fixture may.
      Regenerating the fixture to absorb a real output change is what this
      criterion forbids. (R14)
- [ ] Unit: one synthetic log — entry into a phase from elsewhere, a recorded
      delivery for it, a failed gate evaluation, then a transition from that phase
      to itself and nothing after — yields two different answers. The gate-blocked
      classification reports not blocked, because the self-entry closed the epoch
      and the new epoch holds no gate evaluation. The delivery decision reports
      already delivered, because the self-entry did not open a new delivery
      window. One log, two boundaries, opposite answers. This is the criterion
      that decides R15: the gate epoch is computed from a helper the delivery
      decision reads today, so an implementation that widens that helper in place
      violates R15 while leaving every file named below unchanged. (R15)
- [ ] Supplementary, not sufficient on their own:
      `git diff --exit-code origin/main...HEAD -- src/cli/dashboard_data.rs
      src/workflows_surface/project.rs` reports no change, and
      `git diff origin/main...HEAD -- src/engine/persistence.rs` shows no change
      inside `derive_evidence`, which re-implements the evidence epoch inline
      rather than sharing it. (R15)

### Measured against a built binary

- [ ] Every case above is demonstrated by running a built `koto` binary against a
      real template and reading the responses, not by reading the diff. The
      demonstration covers, at minimum: first arrival; self-loop; second
      consecutive self-loop; loop-back to an earlier phase; re-entry after
      leaving; same-tick round trip; rewind from a later phase; rewind whose
      source and target are the same phase; directed transition to a different
      phase; directed transition to the occupied phase; first and second
      non-advancing tick; `--full`; and `koto status`. The measured output is
      recorded in the pull request body so a reviewer can check it without
      re-running it.

### Surfaces

- [ ] Each of these states the shipped rule:
      `plugins/koto-skills/skills/koto-user/references/response-shapes.md`,
      `plugins/koto-skills/skills/koto-user/references/command-reference.md`,
      `plugins/koto-skills/skills/koto-author/SKILL.md`,
      `plugins/koto-skills/skills/koto-author/references/template-format.md`,
      `plugins/koto-skills/.cursor/rules/koto.mdc`,
      `docs/guides/cli-usage.md`,
      `docs/reference/session-feed.md`,
      the doc comments in `src/engine/persistence.rs`, `src/cli/next_types.rs`
      and `src/cli/mod.rs` that state the rule, and
      `tests/next_response_baseline.rs`'s notes and sequence descriptions.
- [ ] `git grep -nE 'self.transition|occupancy' -- ':!wip' ':!docs/prds/PRD-self-loop-suppresses-details.md' ':!docs/briefs/BRIEF-self-loop-suppresses-details.md'`
      returns no line asserting that a self-transition re-delivers instructions or
      opens a delivery window. `occupancy` is in the pattern because the two
      durable documents state the boundary without using the phrase
      "self-transition" in the sentence that states it. (R18)
- [ ] `docs/prds/PRD-inline-phase-details.md` and
      `docs/designs/current/DESIGN-inline-phase-details.md` describe the shipped
      rule and record that it was reversed, by what, and why. (R18, R19)
- [ ] `docs/reference/session-feed.md` and the `InstructionsDelivered` doc comment
      in `src/engine/types.rs` no longer claim the event is not emitted yet. Both
      are wrong on merged `main`, describe this feature, and are inside R18's
      surface. (R18)
- [ ] The koto-user eval suite still contains the non-advancing-tick eval and the
      rewind eval with their verdicts intact, and contains one new eval asserting
      that a self-loop tick omits the instructions and that this is expected.
      Structural check only: the eval suite is not executed by CI. (R20)
- [ ] `CHANGELOG.md` has an entry under `[Unreleased]`. (R21)

### Gates

- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`,
      `cargo test -- --test-threads=1`,
      `cargo test -p koto-stability-tests -- --test-threads=1`, and `cargo audit`
      all exit 0. These are the commands CI runs; clippy is invoked without
      `--all-targets`, so no criterion assumes lints on test targets are gated.
- [ ] The documentation workflow's per-file schema validation passes. This PR
      touches `docs/`, so it triggers.
- [ ] The plugin workflow's template compilation step passes over the
      `koto-templates/` directories under the plugin tree.
- [ ] `git diff origin/main...HEAD -- src/engine/types.rs` adds no `EventPayload`
      variant, adds no field to the delivery event, and does not change the schema
      version constant. (R16)
- [ ] The two response-construction sites keep their current read behavior: the
      directed path still builds its event list in memory from the payload it just
      appended, and the natural path still performs exactly the one post-advance
      read it performs today — no more, no fewer. Checkable as a diff review at
      each site. (R17)
- [ ] The staging directory for non-durable workflow artifacts is empty, and no
      file this change adds or modifies names a path inside it. The check greps
      that directory's prefix across the branch's own diff against main, not
      across the whole repo: several archived designs and the contributor guide
      mention the directory for unrelated reasons and predate this work. This
      criterion deliberately does not spell the prefix, so that it does not match
      its own grep.
- [ ] The pull request is out of draft. Several jobs, including the artifact
      check, are skipped while a PR is a draft, so a green draft is not a green
      PR.

## Out of Scope

The upstream BRIEF's Scope Boundary is the boundary; the entries below are the
ones that constrain a requirement or that a reader would otherwise expect here.

- A dedicated command for re-reading a phase's instructions. `koto status`
  already returns the same text the tick would, through the same substitution
  pipeline, without moving the workflow. koto#90 asks for
  `koto phase-info <workflow>` "(or similar)", and this is the similar.
- Changing what a session log contains. R16 states this as a requirement because
  it constrains the solution space in a way a downstream design must respect.
- The other open koto defects in this area, each filed and independent. The
  rewind defect is adjacent: R6 is written so that the delivery answer does not
  depend on which phases a rewind records, which is what keeps the two apart.
- Whether a phase the engine auto-advances through should surface a directive or
  instructions at all.
- Removing the visit-count helper the shipped rule stopped using for this
  decision. It still has a consumer in the `/workflows` projection, so removing
  it is a separate change with its own blast radius.

## Decisions and Trade-offs

### A directed transition into the occupied phase suppresses; a rewind into it delivers

This is the question the upstream BRIEF deferred here. Both are explicit operator
instructions issued from outside the workflow, and they land on opposite sides of
the rule, so the asymmetry needs an argument rather than an assertion.

The two commands mean different things. `koto next --to P` says "route to P" —
it is the transition the template would have taken, taken by hand, and on a phase
the workflow already occupies it is a lap of the same loop with the condition
supplied manually. `koto rewind` says "discard the forward progress and do that
again": it is a corrective signal about work already performed, and the agent it
addresses is being asked to start a procedure over, not to continue one. The
instructions are exactly what that agent needs and exactly what the looping agent
already has.

The alternative considered was making both explicit commands deliver, on the
ground that an operator reaching for either has stepped outside the workflow and
probably wants the full picture. It was rejected because `--to P` at P is
reachable only when the template declares `P -> P`, which is to say it is
reachable only as a hand-driven lap of a declared loop — the very case koto#90
names. Delivering there would leave the criterion half-met and the rule stated
with an exception a template author would have to memorize.

The cost of the choice is that the two commands now differ in a way neither one's
name suggests. That is paid down in the documentation requirement: the
agent-facing surfaces state the rule in terms of "did you arrive from somewhere
else", under which a rewind is a redo and a directed self-transition is not an
arrival at all.

### The rule stays keyed on the recorded delivery, not on the shape of the entry event

Two rules would satisfy koto#90's criterion. One tests the entry event: if the
event that put the workflow here records the same phase as source and target,
omit. The other keeps the shipped question — has a delivery been recorded since
this phase was entered? — and widens the window backwards across self-entries.

They agree on every case anyone has asked about and disagree on one nobody has:
a phase entered from elsewhere whose delivery record is missing, followed by a
self-transition. The record can be missing — the shipped design accepts a crash
between printing a response and appending its record, and biases toward
re-delivering when that happens, because an agent that receives instructions
twice is inconvenienced and an agent that never receives them is stuck.

Under the first rule that bias inverts: the self-entry suppresses on its own
shape, no lap re-delivers, and the agent never gets the procedure. Under the
second the widened window finds no record and delivers. R1 is written as the
second, which is why it speaks of a recorded delivery rather than of the entry
event's shape, and why the acceptance criteria carry a case with no record in the
log at all.

### The requirement is scoped to the delivery decision, not to a global redefinition

The shipped implementation derives three things from one notion of "the events
since this phase was last entered": which instructions have been delivered, which
gate evaluation decides the blocked badge, and which evidence is in scope. Only
the first is meant to move, which is why the Definitions section gives the moving
one its own name — delivery window — and keeps "epoch" for the other two.

Writing this as "an entry into a phase from itself no longer starts anything"
would have been shorter and would have silently retargeted a badge nothing tests
and an evidence window a retry loop depends on. Whether the implementation forks
the shared helper, parameterizes it, or writes a second scan is the DESIGN's
call; R15 and its criteria make every answer checkable.

### The failure direction for a completed loop is accepted

Once a delivery is recorded inside a window, no lap re-delivers, so an agent that
loses the procedure mid-loop cannot recover it by ticking. This is the point of
the change rather than a side effect of it, and it is safe only because of R9 and
R11 together: the retrieval returns the instructions without moving the workflow,
and every response for an instruction-carrying phase names the retrieval in its
directive. Those two are load-bearing here in a way they were not before, which
is why they carry acceptance criteria of their own.

### The reversal is recorded rather than quietly corrected

R19 exists because the failure this PRD fixes was not a coding error. The shipped
definition was reasoned, written down, and defended in a design document, and the
thing that went wrong is that it overrode an acceptance criterion and an older
accepted requirement without citing either. Deleting the passage that made the
argument would reproduce that failure in the opposite direction. The durable
record has to carry both rulings and say which governs.

## Known Limitations

- **An in-flight session changes answer on upgrade.** A session sitting on a
  self-entry when the new binary lands will suppress where the old one delivered,
  on its very next tick. No migration can avoid this without keeping the old rule
  for old logs, which would mean two rules; the recovery route is the retrieval,
  and the pointer in the directive names it.
- An agent that loses a phase's instructions to compaction while inside a loop
  has exactly one recovery route: the read-only retrieval, or the forcing flag if
  it is willing to spend a tick. No lap will hand them back.
- The eval suite is not executed by CI — the plugin workflow only asserts that
  each skill has at least one eval. The new eval's presence is checkable; its
  verdict is not gated, and confirming it requires running the eval script by
  hand.
- The rule is stated in terms of the entry that opened the current delivery
  window, which is precise but is not the same as "the tick started and ended in
  the same phase". A tick that leaves a phase and returns to it delivers. Any
  documentation that paraphrases the rule as "did the state change" will be wrong
  on that case.
- R16's cross-version read clause has no harness. Nothing in the repo runs the
  previous release against a log the changed binary wrote. It is checkable by
  inspection — no new event type and no new field means an older build has
  nothing new to fail on — and by the additive-events rule the session schema
  already documents, but it is not gated.
- The baseline fixture embeds prose from its generating test file, so correcting
  the one sentence in it that states the old boundary requires editing two files
  in lockstep and produces a diff in a fixture whose whole purpose is to have no
  diff. The acceptance criterion is written to permit exactly that one change and
  no other.
