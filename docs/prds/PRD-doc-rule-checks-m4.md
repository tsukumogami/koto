---
schema: prd/v1
status: Draft
problem: |
  A decider check grades one slice per evaluation, at most four criteria a
  state and four consultations a `koto next`, under a 2.5 KB budget. The
  rules worth grading in documents hold per unit (one acceptance criterion,
  one bullet, one milestone) while the sections holding them run 3 to 17 KB,
  so template authors can't grade units at all. A decider fail hands the
  agent a one-line finding with no rule text, and the records don't say which
  unit was graded, how confident the decider was, or what rule went out.
goals: |
  A template author grades every unit of a section in one state visit with
  one check. An agent whose unit fails is told which unit and gets the rule,
  summary first and full text on a repeat; a unit the decider can't answer
  goes to the agent while the rest are graded. Every per-unit answer,
  fallback and rule delivery is in telemetry that any host can query.
upstream: docs/briefs/BRIEF-doc-rule-checks-m4.md
---

# PRD: per-unit decider checks that hand back the broken rule

## Status

Draft

## Problem Statement

Template authors who want koto's decider to grade a document can't grade it
where its rules hold. A `decider-check` gate runs one extraction command and
sends its whole output to the decider as one slice, once per criterion,
under a per-slice budget of 2,560 bytes by default. A state holds at most
four decider criteria and a `koto next` makes at most four consultations,
shared with the routing decider. The rules a document is held to are per
unit: is this acceptance criterion binary, does this scope bullet stay at
its altitude, does this milestone's evidence name a query. A single
acceptance criterion is about 200 bytes; the section holding ten of them is
several kilobytes. Today the author can send the whole section and get
`over_budget` with no verdict, or write one gate per unit, which can't be
done for a section whose length isn't known when the template is written
and wouldn't fit the four-consultation cap anyway.

When the decider does fail a criterion, the agent gets one finding naming
the criterion and its fail description. It isn't told which part of its work
failed, and it gets no rule to fix against. Command gates in the same
workflows return the broken rule's summary on a first failure and its full
text on a repeat; a decider check can return neither, because it has no
output streams and its findings carry only a short message.

The records can't support the next decision either. Every decider criterion
ships in shadow until its verdicts are shown to agree with human reviewers
on the same units. That comparison needs to know, from telemetry on every
host, which document, section and unit each verdict was about, how
confident the decider was, why a unit went to the agent, and what rule text
the agent was then shown. Today's records carry the criterion and its
outcome only, so no criterion can be compared unit by unit and none can
leave shadow.

## Goals

- A template author points one decider check at a section and every unit in
  it is graded on its own, in the same state visit, under the budget the
  decider was validated at, without knowing in advance how many units there
  will be.
- An agent whose unit fails sees which unit failed and the broken rule's
  summary; if the same rule fails again in the visit, it sees the rule's
  full text. One failing unit is enough to fail the check, as one failing
  criterion is today.
- A unit the decider couldn't grade goes back to the agent with its reason,
  through the existing fallback, while the decider's verdicts on the other
  units stand.
- A maintainer on any host can read, from exported telemetry alone, each
  unit's answer and confidence, every fallback with its reason, and each
  rule delivery with its level, and line them up against reviewers' verdicts
  on the same units.

## User Stories

- As a template author adding a check state to a document-drafting
  workflow, I want one decider check to grade each acceptance criterion in
  a PRD on its own, so that a long section is graded under the validated
  budget without one gate per criterion.
- As an agent drafting a PRD whose draft fails two of ten acceptance
  criteria, I want the `koto next` response to name those two by key and
  give the rule's summary, and to give the rule's full text if one of them
  fails again, so that I fix exactly what's wrong.
- As an agent whose draft has one acceptance criterion too long for the
  budget, I want that one handed back to me to judge while the other nine
  get the decider's verdict, so that one oversized unit doesn't hold up the
  rest or pass unjudged.
- As a maintainer on a host that ran none of the sessions, I want to read
  per-unit answers, confidence, fallbacks and rule deliveries from
  telemetry, so that I can judge whether a criterion agrees with reviewers
  well enough to leave shadow.

## Requirements

### Declaring units

- **R1. Unit mode is opt-in per check.** A `decider-check` gate may declare
  that its extraction prints units rather than one slice. A check that
  doesn't declare it behaves exactly as today, and a template that uses
  none of this feature compiles to the same JSON and template hash as
  before.
- **R2. One extraction prints every unit.** In unit mode the extraction
  command runs once per evaluation and prints each unit with its key, its
  text, and optionally the document and section it belongs to. koto grades
  each unit's text as that unit's slice.
- **R3. Unit keys are names.** A unit key, document and section each match
  koto's name shape (letters, digits and `._:/@-`, 1 to 128 bytes), so they
  can be quoted in a command line and exported as written. Keys are unique
  within one extraction. Output that breaks this (bad shape, duplicate key,
  unparseable line) makes the whole check `unanswered` with
  `extraction_failed`, as a failed extraction does today.
- **R4. The number of units is bounded.** A unit-mode check declares, or
  defaults, the most units it grades in one evaluation, at most 32, and the
  compiler refuses a check whose units times criteria could exceed 32
  consultations. Units past the bound are not graded by the decider: each
  falls back to the agent with its own reason.
- **R5. The per-unit budget is the per-slice budget.** Each unit is held to
  the check's `max_bytes`, unchanged in range and default. An over-budget
  unit is `unanswered` with `over_budget` on its own; the other units are
  still graded. An empty unit is `not_graded`.

### Grading and combining

- **R6. One consultation per unit and criterion.** Each unit is graded
  against each criterion with one request, the configuration the decider
  was measured at. A verdict is reused within the visit only for the same
  unit, criterion, declaration and slice.
- **R7. Unit consultations don't spend the shared cap.** A unit-mode check's
  consultations are counted against its own bound (R4), not against the
  four-per-`koto next` cap the routing decider and single-slice checks
  share. The bounded wait that stops a concurrent `koto next` from passing a
  check another tick is still grading covers a unit-mode check's longest
  evaluation, or a tick that can't wait that long falls the waiting units
  back to the agent; it never passes them.
- **R8. Combining is fixed, not declared.** A unit-mode check fails when any
  veto unit fails on a verdict or on the agent's answer; otherwise it is
  pending while any unit waits on the agent; otherwise it passes. Shadow
  units never fail the check, as shadow criteria never do today.
- **R9. The response names the unit.** Each failing unit gets its own error
  finding carrying the criterion's rule id and the unit's key, document and
  section. Each waiting unit gets its own warning finding naming the unit
  and the exact command to answer it. The gate's output says which units
  failed, waited or were answered by the agent, unit by unit.

### Fallback

- **R10. Fallback is per unit.** A unit with no verdict, for any reason,
  falls back to the agent through the existing decider fallback, and the
  other units keep the decider's verdict. The fallback record names the
  unit.
- **R11. The agent answers one unit at a time.** `koto decider answer`
  accepts the unit's key for a unit-mode check, requires it there, and
  refuses it on a check that isn't in unit mode. An unknown unit, or a unit
  with no pending fallback, is refused with its own error code. The answer
  binds to that unit's slice, so an edited unit is graded afresh.

### Rule delivery

- **R12. A criterion can carry its rule.** A criterion may declare a rule
  summary (at most 300 bytes) and the rule's full text (at most 4,096
  bytes), inline in the template. Neither is sent to the decider, and
  neither changes the criterion's declaration hash.
- **R13. Summary first, full text on a repeat.** When a criterion with a
  declared rule fails and blocks, in unit mode or not, the `koto next`
  response carries the rule once for that criterion: its summary on the
  first failing evaluation of that check and rule in the visit, its full
  text on any later failing evaluation in the same visit. A criterion with
  only a summary repeats the summary; one with only full text sends it from
  the first failure.
- **R14. The rule text stays out of the log.** The rule text appears only
  in the response, never in finding messages, the session log or the run
  journal. Each finding message stays within koto's 1,000-byte bound.
- **R15. Shadow delivers nothing.** A shadow criterion never blocks, so it
  produces no finding and no rule delivery.

### Telemetry

- **R16. Per-unit decider records.** Each per-unit consultation, fallback
  and agent answer records the unit's key and, when given, its document and
  section, as optional fields on the existing decider-check events, and the
  run journal relays them on the `decider_checked`, `decider_fallback` and
  `decider_check_answered` records.
- **R17. Confidence.** Each `decider_checked` record whose consultation read
  an answer carries the decider's probability for the outcome it recorded,
  as an integer in hundredths of a percent (0 to 10,000). This is a
  deliberate change to the run journal's rule that it holds no probability,
  made for this one value, and the run-journal reference says so.
- **R18. Rule deliveries.** Each rule delivered in a response is recorded
  once, with the state, the check, the rule id, the level (`summary` or
  `full`) and, for a unit-mode check, how many units of that rule failed,
  and the run journal relays it. When koto already records rule deliveries
  for other checks, decider deliveries use that same record kind and
  fields; this feature adds no second shape.
- **R19. Names collide with nothing.** Every new attribute is in the `koto.`
  namespace and is neither equal to nor a dot-prefix of an existing one.
- **R20. Records fit.** A worst-case per-unit record (three 128-byte names)
  stays within the run journal's 4,096-byte line bound.
- **R21. The readout reads exported telemetry.** The feature's evidence is
  read by one named telemetry query over exported records, re-runnable from
  any host, that reads no local file, no session store and no
  `koto decider report`. It does not depend on derived per-template series.
  koto's documents refer to it as the telemetry readout.

### Non-functional

- **R22. Older readers.** koto v0.14.1 reads a session log holding per-unit
  records and rule-delivery records without error, and `koto decider report`
  on a ledger holding them exits 0.
- **R23. Latency is visible.** Each per-unit consultation records its
  latency as today, so a unit-mode check's added time per `koto next` can be
  read from the records.
- **R24. Docs and skills.** The template-format reference, decider authoring
  guide, error-code reference, run-journal and session-feed references, and
  the koto-author and koto-user skills describe unit mode, per-unit
  fallback, the answer command's unit argument and rule delivery.

## Acceptance Criteria

- [ ] A template whose unit-mode decider check (veto, effective mode auto in
  the test) grades a section of ten acceptance criteria, two of them not
  binary, returns a `koto next` response whose error findings name exactly
  those two unit keys and whose rule entry for the criterion carries the
  summary; a second failing evaluation in the same visit carries the full
  text instead. An integration test against the stub decider covers it.
- [ ] In the same template with one unit over `max_bytes`, that unit is
  `unanswered` with `over_budget` and gets a waiting finding naming it and
  the answer command, while the other nine units carry the decider's
  verdicts. A test covers it.
- [ ] A unit-mode check grading ten units makes ten consultations in one
  `koto next` and leaves the shared four-consultation cap untouched, so a
  routing decider consultation in the same call is still made. A test covers
  it.
- [ ] Units past the declared bound fall back to the agent with their own
  reason and are not sent to the decider. A test covers it.
- [ ] Extraction output with a duplicate key, a key outside the name shape
  or an unparseable line makes every criterion `unanswered` with
  `extraction_failed`. A test covers each case.
- [ ] `koto decider answer --unit <key>` records an answer that the next
  evaluation applies to that unit only; omitting `--unit` on a unit-mode
  check, naming an unknown unit, and passing `--unit` on a single-slice check
  are each refused with a distinct error code. Tests cover all four.
- [ ] A template declaring none of the new fields compiles to byte-identical
  JSON and the same template hash as before, and a criterion's declaration
  hash doesn't change when only its rule summary or full text changes. Tests
  cover both.
- [ ] A shadow unit-mode check whose units fail produces no error finding and
  no rule delivery. A test covers it.
- [ ] The run journal's `decider_checked`, `decider_fallback` and
  `decider_check_answered` records carry the unit key, document and section;
  `decider_checked` carries the confidence integer when an answer was read;
  each delivered rule produces one rule-delivery record with its level. The
  run-journal doc tests and collision tests pass with the new names.
- [ ] No rule text appears in the session log, the run journal or any
  finding message. A test asserts it.
- [ ] The koto v0.14.1 compatibility job passes on a log and ledger holding
  per-unit and rule-delivery records.
- [ ] `cargo test` (including `doc_names`), `cargo clippy --all-targets -- -D
  warnings` and `cargo fmt --check` pass, and the koto-author and koto-user
  skills document unit mode, the `--unit` argument and rule delivery.

## Out of Scope

- Moving `decider.mode`, or any criterion, out of shadow. Per-unit fails
  block only where a template says veto and a user opts in, as today.
- Writing the document criteria themselves (binary acceptance criteria,
  scope-bullet altitude, user-path shape). This feature gives them a place
  to run.
- A check state in shirabe's document skills, and how /scope or /charter
  places checks.
- A third rung naming an exit after repeated failures; a decider check
  stops at the full text.
- Batching several units or criteria into one decider request.
- Changing the per-slice budget, its range, or criterion thresholds.
- Running unit consultations in parallel.
- The exported-telemetry pipeline's limits on derived series; the readout
  reads exported records, not derived series.
- A batch form of `koto decider answer` that answers several units at once.

## Decisions and Trade-offs

- **Units come from one extraction, not a per-unit loop.** One command
  prints every unit with its key. A loop would need the template to know the
  unit list in advance, or a second command to list units, and would run the
  extraction many times. One extraction also keeps a single place where the
  slicing contract lives.
- **Combining is fixed.** Any veto unit fail fails the check. A declared
  rule (all, any, a threshold) was considered; no criterion the work is for
  needs anything but "every unit must hold", and a knob nobody uses is one
  more thing to validate. A later need can add it.
- **A separate bound for unit consultations.** Raising the shared cap would
  raise the routing decider's per-call allowance too and break the lock-wait
  sizing; keeping units under the shared cap would make every unit past the
  fourth fall back. A per-check bound of 32 consultations keeps the cost
  visible and bounded.
- **Rule text inline, summary and full.** koto does no I/O at compile time
  for decider checks and tells agents not to fetch `rule_ref`, so the text
  the agent gets has to be in the template. The text is delivered from the
  response only, once per failed rule, so twenty failing units don't repeat
  it and the log never holds it.
- **Repeat means the same rule, same check, same visit.** That's the window
  koto's per-rule counts already use; counting per unit would send the
  summary again for each new unit that breaks the same rule.
- **Shadow delivers nothing.** Shadow means a check logs without changing
  the run. Delivering rule text from a shadow criterion would change what
  the agent does and blur the agreement the shadow period measures.
- **Confidence as an integer.** The journal has no float writer and its
  records stay small and comparable; hundredths of a percent keep the four
  decimal places the session log already stores.
- **Unit keys must be names.** A free-form key would be digested on export
  and couldn't be lined up against reviewers' verdicts without the source
  document. Extraction scripts can always slug a heading.

## Known Limitations

- A unit-mode check grading 30 units adds about 30 decider round trips to
  one `koto next`, sequentially. At the measured median that's several
  seconds; the records show it.
- Binding an agent answer to the unit's slice means an answer doesn't
  survive an edit to that unit, by design.
- In shadow, which is where every criterion starts, no rule is delivered, so
  rule-delivery records come only from veto opt-ins and tests until a
  criterion leaves shadow.
