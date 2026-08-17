# Explore Scope: self-loop-suppresses-details

## Visibility

Public

## Scope

Tactical

## Entry Assessment

Result: needs investigation
Confidence: high
Dissent: none recorded. The dispatch brief itself names an open design question
(what to do with the Current DESIGN doc whose occupancy definition the change
falsifies), which is the defining signal of "needs investigation".
Signals cited: the issue's AC 3 conflicts with a shipped, documented definition
in a Done PRD and a Current DESIGN; the change touches a predicate, its unit
tests, an integration test that asserts the opposite, six documentation
surfaces, and an eval suite.

## Core Question

koto#90 AC 3 says a self-loop must omit a phase's `details`. Merged `main`
(b7b0799) re-delivers them, because #197 defined an *occupancy* as beginning at
any state-entry event and ending at the next one -- which makes `P -> P` an
occupancy boundary. The user has ruled that AC 3 wins. What is the smallest
correct way to make entry-from-the-same-phase stop being an occupancy boundary,
across the predicate, every delivery call site, the tests, the evals, the skills,
and the durable design record that currently documents the opposite?

## Context

- The `details` field and first-visit inlining shipped in PR #109 (closed #102,
  never cited #90). PR #197 (`b7b0799`) replaced the *when* rule because the
  shipped one counted state entries rather than instruction deliveries.
- #197's durable artifacts on `main`: `docs/briefs/BRIEF-inline-phase-details.md`
  (Done), `docs/prds/PRD-inline-phase-details.md` (Done),
  `docs/designs/current/DESIGN-inline-phase-details.md` (Current). The PLAN was
  deleted by the finalization cascade.
- Measured on merged `main`: first visit PRESENT; gate-blocked re-tick omitted;
  self-loop `work -> work` PRESENT (wrong); `koto status` PRESENT.
- The decision to make self-loops re-deliver was taken unilaterally during #197's
  scoping and written into the PRD as a definition, without flagging that it
  overrode an explicit AC of the issue being fixed. The user has now ruled the
  other way.

## In Scope

- The delivery predicate and its occupancy helper in `src/engine/persistence.rs`.
- Every call site that decides whether `details` rides along on a response.
- Tests, fixtures and evals that encode the old rule.
- koto-user and koto-author skills, `.cursor/rules/koto.mdc`, `docs/guides/cli-usage.md`.
- The durable-artifact question: amend, supersede, or otherwise record the change
  to the occupancy definition in the Done PRD and Current DESIGN.
- `CHANGELOG.md` under `[Unreleased]`.

## Out of Scope

- koto#198 (required `accepts` does not gate advancement), koto#199 (`koto rewind`
  oscillates), koto#200 (session log truncation under concurrent writers),
  koto#193 (migration never converges / stderr flood), shirabe#328.
- Adding a `koto phase-info` command. `koto status` already satisfies AC 4 and a
  second surface would be redundant.
- Auto-advanced intermediate phases surfacing neither directive nor details.
- Any change to `CURRENT_SCHEMA_VERSION`.

## Research Leads

1. **How does the delivery predicate actually decide today, and where in that
   evaluation is the previous phase knowable?**
   The whole change hinges on whether `occupancy_slice` can distinguish "entered
   from elsewhere" from "entered from myself" using events already in the log.

2. **Which call sites decide `details` delivery, and what does each one expect?**
   `koto next`, `koto next --to`, `koto next --full`, `koto status`, batch mode
   and the auto-advance loop may each reach the predicate differently. A change to
   the shared helper lands on all of them at once.

3. **Exactly which sentences in the BRIEF, PRD and DESIGN encode the old
   occupancy definition, and what does this repo's doc lifecycle say about
   changing a Done PRD and a Current DESIGN?**
   The brief calls this out as a real open question and forbids silently leaving
   a durable artifact describing behaviour the code no longer has.

4. **What is the full blast radius in tests, fixtures, evals and documentation?**
   Every assertion that encodes redelivery has to flip or be re-justified, and the
   byte-identity baseline fixture must stay green untouched.

5. **Does a self-transition leave any distinguishable trace in the event log
   other than the transition event itself, and does #199's rewind oscillation
   change which events a rewind appends?**
   The brief warns that #199 matters indirectly: if a rewind appends different
   events than assumed, the predicate can regress on the rewind case.
