---
schema: prd/v1
status: Accepted
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

Accepted

## Problem Statement

Template authors who want koto's decider to grade a document can't grade it
where its rules hold. A `decider-check` gate runs one extraction command and
sends its whole output to the decider as one slice, once per criterion,
under a per-slice budget of 2,560 bytes by default. A state holds at most
four decider criteria and a `koto next` makes at most four consultations,
shared with the routing decider. The rules a document is held to are per
unit: is this acceptance criterion binary, does this scope bullet stay at
its altitude, does this milestone's evidence name a query. In the PRDs and
BRIEFs this work was sized against, a single acceptance criterion is about
200 bytes at the median, while the section holding them is 3 to 17 KB.
Today the author can send the whole section and get `over_budget` with no
verdict, or write one gate per unit, which can't be done for a section
whose length isn't known when the template is written and wouldn't fit the
four-consultation cap anyway.

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
  it is graded on its own, in the same state visit, under the per-slice
  budget the decider checks already use, without knowing in advance how many
  units there will be.
- An agent whose unit fails sees which unit failed and the broken rule's
  summary; if the same rule fails again in the visit, it sees the rule's
  full text. One failing unit is enough to fail the check, as one failing
  criterion is today.
- A unit the decider couldn't grade goes back to the agent with its reason,
  through the existing fallback, while the decider's verdicts on the other
  units stand.
- A maintainer on any host that sends koto telemetry can read, from exported
  records alone, each unit's answer and confidence, every fallback with its
  reason, and each rule delivery with its level, and line them up against
  reviewers' verdicts on the same units.

## User Stories

- As a template author adding a check state to a document-drafting
  workflow, I want one decider check to grade each acceptance criterion in
  a PRD on its own, so that a long section is graded under the per-slice
  budget without one gate per criterion.
- As a template author, I want to attach a rule's summary and full text to a
  criterion, so that an agent that breaks it gets the rule back without
  fetching anything.
- As an agent drafting a PRD whose draft fails two of ten acceptance
  criteria, I want the `koto next` response to name those two by key and
  give the rule's summary, and to give the rule's full text if the rule
  fails again, so that I fix exactly what's wrong.
- As an agent whose draft has one acceptance criterion longer than the
  per-slice budget, I want that one handed back to me to judge while the
  other nine get the decider's verdict, so that one oversized unit neither
  holds up the rest nor passes unjudged.
- As a maintainer on a host that ran none of the sessions, I want to read
  per-unit answers, confidence, fallbacks and rule deliveries from
  telemetry, so that I can judge whether a criterion agrees with reviewers
  well enough to leave shadow.

## Requirements

### Terms

- A **visit** is koto's existing state-visit window: it opens when the
  workflow arrives in a state from a different state, or is rewound into
  it, and a self-transition doesn't open a new one. Per-rule counts and
  verdict reuse already use this window.
- A criterion's **effective mode** is `veto` only when the template
  declares `mode: veto` and the user's (or project's) decider mode is
  `auto`; otherwise it is `shadow`. A veto criterion can fail a check; a
  shadow one never does. A user who hasn't opted in to the decider gets a
  template with decider checks removed, as today, so nothing here applies.
- A **unit** is one item the extraction prints in unit mode; its **slice**
  is its text.

### Declaring units

- **R1. Unit mode is opt-in per check.** A `decider-check` gate declares
  unit mode with `units: jsonl`. A gate without it behaves exactly as today:
  same consultations, findings, cap use and records. A template that uses
  none of this feature's fields compiles to the same JSON and template hash
  as before.
- **R2. One extraction prints every unit.** In unit mode the extraction
  command runs once per evaluation and prints one JSON object per line:
  `key` and `text` (strings, required), and `document` and `section`
  (strings, optional). Blank lines are ignored, other fields are ignored,
  and a `null` `document` or `section` counts as absent. koto grades each
  unit's `text` as that unit's slice, in the order printed.
- **R3. Unit keys are names.** `key`, `document` and `section` each match
  koto's name shape (letters, digits and `._:/@-`, 1 to 128 bytes, not
  starting with `/` or `~`). Keys are compared byte for byte and must be
  unique within one extraction. A line that isn't a JSON object with string
  `key` and `text`, a field outside the name shape, or a duplicate key makes
  the evaluation an extraction failure: every criterion is `unanswered` with
  `extraction_failed`, with no unit, as a failed extraction is today.
- **R4. Units are bounded.** A state's unit-mode checks share a bound of 32
  consultations per evaluation, counted as the sum over those checks of
  `max_units` times the check's criteria (shadow and veto alike). A check
  may declare `max_units`; when it doesn't, the default is 32 divided by
  the number of criteria across all unit-mode checks in the state, rounded
  down, so defaults never exceed the bound. The compiler refuses a
  template, with `E-DECIDER-CHECK-UNITS`, when declared values exceed the
  bound or `max_units` is below 1. The first `max_units` units printed are
  graded; each later unit falls back to the agent for every criterion with
  reason `unit_limit` and is not sent to the decider, whatever its size.
- **R5. The per-unit budget is the per-slice budget.** Each graded unit's
  text is held to the check's `max_bytes`, unchanged in range and default.
  A unit whose text is empty or only whitespace is `not_graded`; otherwise
  a unit over `max_bytes` is `unanswered` with `over_budget`, and a unit at
  exactly `max_bytes` is graded. An extraction that prints no units grades
  nothing: every criterion is `not_graded` and the check passes.

### Grading and combining

- **R6. One consultation per unit and criterion.** Each graded unit is
  graded against each criterion with one request carrying only that unit's
  text, one question per request as single-slice checks do. Within a visit,
  a recorded verdict is reused, with no new request, for the same check,
  criterion, declaration, unit key and text; a change to any of them asks
  again.
- **R7. Unit consultations don't spend the shared cap.** A unit-mode check's
  consultations count against the bound in R4, not against the four-per-
  `koto next` cap that the routing decider and single-slice checks share. A
  `koto next` that finds another process grading the same session waits as
  today; when its wait runs out, every unit it would have consulted is
  `unanswered` with `busy` and falls back. No unit passes for lack of a
  verdict.
- **R8. Combining is fixed, not declared.** For a unit-mode check:
  - it fails when any veto criterion fails on any unit, on the decider's
    verdict or on the agent's recorded answer;
  - otherwise it is pending, as decider fallbacks already are, while any
    criterion on any unit waits on the agent, shadow criteria included
    (the agent's answer to a shadow fallback is recorded and never fails
    the check);
  - otherwise it passes.
  A shadow criterion's fail never fails the check.
- **R9. The response names the unit.** Each (unit, veto criterion) that
  fails gets its own error finding carrying the criterion's rule id and the
  unit's key, document and section. Each (unit, criterion) that waits on
  the agent gets its own warning finding naming the unit and the exact
  `koto decider answer` command line for it. The gate's output gains a
  `units` array with one object per unit printed: `key` (string), and
  `failed`, `awaiting` and `agent_answered` (arrays of criterion ids, each
  possibly empty). The existing `failed`, `unanswered` and `awaiting` lists
  keep listing criterion ids, each once.

### Fallback

- **R10. Fallback is per unit.** A (unit, criterion) with no verdict, for
  any reason (`over_budget`, `unit_limit`, `busy`, a provider error, an
  unreadable answer), falls back to the agent through the existing decider
  fallback, and the other units keep the decider's verdicts. In shadow it
  falls back too, as shadow criteria do today. The fallback record names
  the unit.
- **R11. The agent answers one unit at a time.** `koto decider answer`
  takes `--unit <key>`. It is required when the criterion's pending
  fallbacks name units (refused `unit_required`), and refused when they
  name none (`unit_not_expected`). A key that the latest evaluation didn't
  print, including a unit that has left the extraction, is refused
  `unknown_unit`; a printed unit with no pending fallback for that criterion
  is refused with the existing `no_fallback`. Each code is listed in the
  error-code reference. An answer binds to that
  unit's text, so editing the unit's text makes the next evaluation grade it
  afresh.

### Rule delivery

- **R12. A criterion can carry its rule.** A criterion may declare `summary`
  (1 to 300 bytes) and `rule_text` (1 to 4,096 bytes), inline in the
  template. The compiler refuses either outside its range with
  `E-DECIDER-CHECK-RULE`. Neither is sent to the decider, and neither
  changes the criterion's declaration hash.
- **R13. Summary first, full text on a repeat.** When a veto criterion with a
  declared rule fails, in unit mode or not, that check's entry in the `koto
  next` response's `blocking_conditions` gains a `rules` array with one
  object for that criterion: `rule_id`, `level`, `text` (the text at that
  level) and, in unit mode, `units` (the keys of the units that failed it).
  The level is `summary` on the first failing evaluation of that check and
  rule in the visit, and `full` on every later failing evaluation of that
  check and rule in the same visit, whichever units fail and whether or not
  the verdicts were reused from an earlier evaluation. A criterion with only `summary` always
  delivers the summary; one with only `rule_text` delivers it at level
  `full` from the first failure. A criterion with neither delivers nothing.
  Another rule, or the same rule on another check, starts at `summary`.
- **R14. The rule text stays out of the records.** Rule text appears only in
  the `koto next` response. It never appears in finding messages, the
  session log, the decider ledger or the run journal. Each finding message
  stays within koto's 1,000-byte bound.
- **R15. Shadow delivers nothing.** A shadow criterion never blocks, so a
  shadow fail produces no error finding and no rule delivery. Its fallbacks
  still produce waiting findings (R10).

### Telemetry

- **R16. Per-unit decider records.** Each per-unit consultation, fallback
  and agent answer records the unit's key and, when given, its document and
  section, as optional fields on the existing `decider_checked`,
  `decider_fallback` and `decider_check_answered` events, and the run
  journal relays them as `koto.unit.key`, `koto.unit.document` and
  `koto.unit.section` on the records of the same names. Records without a
  unit are written exactly as today.
- **R17. Confidence.** A `decider_checked` record whose consultation read an
  answer carries `koto.decider.confidence_bp`: the highest of the decider's
  probabilities for `pass`, `fail` and `unclear`, times 10,000 and rounded
  to the nearest integer (halves up), so 0 to 10,000. It is absent when no
  answer was read.
  The run-journal reference records this as the one exception to its rule
  that records hold no probability.
- **R18. Rule deliveries.** Each `rules` entry delivered in a response is
  recorded once, after the response is printed, as a `rule_delivered` event
  relayed to the run journal as a `rule_delivered` record with `koto.state`,
  `koto.check`, `koto.rule.id`, `koto.rule.level` (`summary` or `full`)
  and, in unit mode, `koto.rule.unit_count` (how many units failed that
  rule). koto has one rule-delivery record kind: any other check that
  delivers rule text, now or later, writes this kind with these fields.
- **R19. Names collide with nothing.** Every new attribute is in the `koto.`
  namespace and is neither equal to nor a dot-prefix of an existing one; the
  run journal's collision tests list them.
- **R20. Records fit.** A worst-case per-unit record, with three 128-byte
  names and a 128-byte rule id, stays within the run journal's 4,096-byte
  line bound.
- **R21. The readout reads exported telemetry.** This feature's evidence is
  read by one named telemetry query over exported run-journal records:
  per unit, its document, section and key, the decider's outcome and
  confidence, any fallback and reason, and each rule delivery with its
  level. Anyone can re-run it from any host; it reads no local file, no
  session store and no `koto decider report`, and doesn't depend on derived
  per-template series. The query's text lives with the telemetry
  definitions, outside this repository; koto's documents call it the
  telemetry readout.

### Non-functional

- **R22. Older readers.** koto v0.14.1 reads a session log holding per-unit
  and `rule_delivered` events without error, and `koto decider report` on a
  ledger holding per-unit `checked` lines exits 0. The new koto reads logs
  and ledgers written before this feature unchanged.
- **R23. Docs and skills.** Each document below names the terms beside it:

  | Document | Terms |
  |---|---|
  | koto-author template-format reference | `units`, `max_units`, `summary`, `rule_text` |
  | decider authoring guide | `units`, `max_units`, `summary`, `rule_text`, `rules` |
  | error-code reference | `E-DECIDER-CHECK-UNITS`, `E-DECIDER-CHECK-RULE`, `unit_required`, `unit_not_expected`, `unknown_unit` |
  | run-journal reference | `koto.unit.key`, `koto.decider.confidence_bp`, `rule_delivered` |
  | session-feed reference | `rule_delivered`, the optional `unit` fields |
  | koto-user skill and its command and response references | `--unit`, `rules`, `units` |

## Acceptance Criteria

Each criterion is checked by an automated test against the stub decider
unless it says otherwise; "veto" below means a criterion whose effective
mode is veto in the test.

Units and bounds:

- [ ] A unit-mode check (one veto criterion with `summary` and `rule_text`)
  over ten units, the stub failing two, fails the gate; the response's
  error findings name exactly those two unit keys; its `rules` entry has
  level `summary`, the summary text and those two keys. (R2, R8, R9, R13)
- [ ] A second failing evaluation of the same check in the same visit, with
  a different unit now failing, delivers level `full` with the rule text,
  and so does a repeated `koto next` with nothing changed; in the same
  visit a second rule's first failure delivers `summary`; a failing
  evaluation after the workflow leaves and re-enters the state delivers
  `summary` again. (R13)
- [ ] The gate output's `units` array has one object per printed unit with
  `key`, `failed`, `awaiting` and `agent_answered` matching the stub's
  verdicts and recorded answers. (R9)
- [ ] With one of the ten units one byte over `max_bytes`, that unit falls
  back with `over_budget` and a waiting finding carrying `--unit <key>`,
  the other nine carry the stub's verdicts, and a unit at exactly
  `max_bytes` is graded. (R5, R9, R10)
- [ ] An empty-text unit is `not_graded`; an extraction printing no units
  passes with every criterion `not_graded`. (R5)
- [ ] With `max_units: 3` and five units, units four and five fall back
  with `unit_limit` for every criterion and the stub receives exactly three
  requests per criterion. (R4)
- [ ] Ten units make ten consultations in one `koto next`, and a routing
  decider consultation in the same `koto next`, with the shared cap
  otherwise unused, is still made. (R7)
- [ ] A template whose state's unit-mode checks allow 33 consultations is
  refused with `E-DECIDER-CHECK-UNITS`; one allowing exactly 32 compiles;
  `max_units: 0` is refused; a two-criterion check without `max_units`
  defaults to 16, a three-criterion one to 10, and two one-criterion
  unit-mode checks in one state without `max_units` default to 16 each and
  compile. (R4)
- [ ] Extraction output with a duplicate key, a 129-byte key, an empty key,
  a key with a space, a `section` outside the name shape, or a line that
  isn't JSON makes every criterion `unanswered` with `extraction_failed`;
  1-byte and 128-byte keys are accepted. (R3)
- [ ] Two criteria (one veto, one shadow) over three units make six
  consultations; a shadow fail alone passes the gate with no error finding
  and no `rules` entry. (R6, R8, R15)
- [ ] Combining: all units passing passes; one unit waiting and none failing
  is pending; a unit the agent answered `fail` on a veto criterion fails;
  one failing unit and one waiting unit fails. (R8)
- [ ] An unchanged unit is not re-sent within the visit; changing its text
  sends it again. (R6)
- [ ] While another process holds the session's decider lock past the wait,
  a `koto next` on a unit-mode check falls every unit back with `busy` and
  doesn't pass the gate. (R7)

Fallback answers:

- [ ] `koto decider answer --unit <key>` records an answer the next
  evaluation applies to that unit only; after the unit's text changes, the
  answer no longer applies. (R11)
- [ ] Omitting `--unit` for a unit fallback is refused `unit_required`;
  passing `--unit` for a fallback without a unit is refused
  `unit_not_expected`; a key the latest evaluation didn't print (including
  one that left the extraction) is refused `unknown_unit`; a printed unit
  with no pending fallback is refused `no_fallback`. (R11)

Rule delivery:

- [ ] A summary-only criterion delivers `summary` on every failure; a
  `rule_text`-only criterion delivers `full` from the first; a criterion
  with neither adds no `rules` entry; a single-slice (non-unit) veto check
  with a rule delivers summary then full like a unit-mode one. (R13)
- [ ] A `summary` of 301 bytes or a `rule_text` of 4,097 bytes is refused
  with `E-DECIDER-CHECK-RULE`; changing either leaves the criterion's
  declaration hash unchanged; neither appears in the stub's received
  requests. (R12)
- [ ] With a sentinel string in `summary` and `rule_text`, after a failing
  run the sentinel is absent from the session log, the decider ledger and
  the run journal, and every finding message is at most 1,000 bytes. (R14)

Telemetry and compatibility:

- [ ] The run journal's `decider_checked`, `decider_fallback` and
  `decider_check_answered` records carry `koto.unit.key`,
  `koto.unit.document` and `koto.unit.section` for unit-mode checks and
  omit them otherwise; with the stub answering pass 0.12345, fail 0.8, unclear
  0.07655, `decider_checked` carries `koto.decider.confidence_bp` 8000, and
  omits it on `over_budget` and `extraction_failed`. (R16, R17)
- [ ] A shadow criterion whose unit falls back holds the gate pending; the
  agent's `fail` answer to it is recorded and the gate then passes. (R8,
  R10)
- [ ] Each delivered `rules` entry produces exactly one `rule_delivered`
  record with `koto.rule.level`, and in unit mode `koto.rule.unit_count`
  equal to the entry's key count; twenty failing units of one rule produce
  one record. (R18)
- [ ] The run journal's doc tests and collision tests pass listing every new
  name, and a record with three 128-byte names and a 128-byte rule id is
  written. (R19, R20)
- [ ] A template declaring none of the new fields compiles to byte-identical
  JSON and the same template hash, and an existing single-slice decider
  check evaluates with the same findings, records and cap use. (R1)
- [ ] The koto v0.14.1 compatibility job passes on a log and ledger holding
  per-unit and `rule_delivered` records, and the new koto reads a log
  written before this feature. (R22)
- [ ] The readout query's text exists with the telemetry definitions, and
  run from a host that ran none of the test sessions it returns, for a test
  session's exported records, per-unit rows with outcome, confidence,
  fallback reason and rule level. Checked by hand at review. (R21)
- [ ] Each document in R23's table contains every term beside it (a grep
  per row), and
  `cargo test` (including `doc_names`), `cargo clippy --all-targets -- -D
  warnings` and `cargo fmt --check` pass. (R23)

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
- Raising the four-consultation cap the routing decider and single-slice
  checks share.
- The exported-telemetry pipeline's limits on derived series; the readout
  reads exported records.
- A batch form of `koto decider answer` that answers several units at once.

## Decisions and Trade-offs

- **Units come from one extraction, not a per-unit loop.** One command
  prints every unit with its key. A loop would need the template to know the
  unit list in advance, or a second command to list units, and would run the
  extraction many times. One extraction keeps the slicing contract in one
  script.
- **JSON lines.** Units are often multi-line Markdown; a JSON string carries
  that without a delimiter convention, and every script language can print
  it.
- **Combining is fixed.** Any veto unit fail fails the check. A declared
  rule (all, any, a threshold) was considered; no criterion this work is for
  needs anything but "every unit must hold", and a knob nobody uses is one
  more thing to validate.
- **A separate bound of 32 consultations per state.** Raising the shared cap
  would raise the routing decider's allowance too and break the lock-wait
  sizing; keeping units under the shared cap would make every unit past the
  fourth fall back. 32 covers a section of 30 units under one criterion
  and keeps a `koto next`'s worst case bounded.
- **Rule text inline, summary and full.** koto reads nothing at run time to
  build a decider check and tells agents not to fetch `rule_ref`, so the
  text the agent gets has to be in the template. It goes out in the response
  only, once per failed rule, so twenty failing units don't repeat it and no
  record holds it.
- **Repeat means same rule, same check, same visit.** That's the window
  koto's per-rule counts already use; counting per unit would send the
  summary again for each new unit that breaks the same rule.
- **Shadow delivers nothing.** Shadow means a check logs without changing
  the run. Delivering rule text from a shadow criterion would change what
  the agent does and blur the agreement the shadow period measures.
- **Confidence as an integer.** The journal has no float writer and its
  records stay small; hundredths of a percent keep the four decimal places
  the session log already stores. The highest probability is used because
  it's defined for every answer, ties and escapes included.
- **Unit keys must be names.** A free-form key would be digested on export
  and couldn't be lined up against reviewers' verdicts without the source
  document. Extraction scripts can always slug a heading.
- **One rule-delivery record kind.** Command gates and decider checks both
  hand back rules; two record shapes would split one readout in two. This
  feature defines `rule_delivered` with fields any check can fill, so
  instrumenting command-gate deliveries later adds writers, not a second
  kind.

## Known Limitations

- A unit-mode check grading 30 units adds 30 decider round trips to one
  `koto next`, one after another. The records carry each one's latency.
- An agent answer doesn't survive an edit to its unit, by design.
- In shadow, where every criterion starts, no rule is delivered, so
  `rule_delivered` records come only from veto opt-ins and tests until a
  criterion leaves shadow.
