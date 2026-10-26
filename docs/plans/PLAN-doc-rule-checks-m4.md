---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-doc-rule-checks-m4.md
milestone: "Per-unit decider checks and rule delivery"
step_count: 7
---

# PLAN: per-unit decider checks that hand back the broken rule

## Status

Active

Written under `/scope` with tracking level `none`: the steps live here and no
issue or milestone is filed.

## Scope Summary

koto decider checks gain unit mode: a check declaring `units: jsonl` grades
each unit its extraction prints against each criterion, under a unit budget
separate from the shared four-consultation cap, and names the failing units in
`koto next`. A unit the decider can't grade goes to the agent on its own, and
the agent answers it with `koto decider answer --unit`. A failing veto
criterion hands back its rule, summary first and full text on a repeat, and
the run journal carries unit digests, the decider's confidence and each rule
delivery. Steps 1 to 6 land in one pull request here.

The telemetry definitions, their test cases, the exporter's field lists and
the readout's text are maintained with the telemetry pipeline outside this
repository. Step 7 is that companion pull request; it is named here so the
run that carries this plan carries it too, and its repository is named only
in that run's own instructions.

## Decomposition Strategy

Walking skeleton. Step 1 runs a template whose decider check declares
`units: jsonl` over a ten-unit acceptance-criteria fixture against the stub
decider, crossing template compile, the evaluator's unit-mode parse and
per-unit consultation, the gate output and findings, and the `koto next`
response, and shows the two failing units named. Each later step adds one
behaviour on top of that path: the rule coming back (step 2), per-unit
fallback and answers (step 3), compile-time bounds (step 4), the run
journal's records (step 5), older readers, docs and skills (step 6), and the
readout that reads it all from exported records (step 7).

## Steps

### Step 1: a unit-mode decider check names the failing units in koto next

**Goal**: A template declaring `units: jsonl` on a decider check compiles, and `koto next` grades each printed unit on its own. The path crosses template compile, the check evaluator's unit-mode parse and per-unit consultation, the gate output and findings, and the `koto next` response. It runs on a ten-unit acceptance-criteria fixture against the stub decider, where the response names exactly the units that failed.

**Acceptance Criteria**:

- [ ] With ten units printed and the stub failing two of them, the `koto next` response holds two error findings, each carrying one of those two unit keys in its `unit` field, and no finding for the other eight; the gate status is `failed`.
- [ ] The gate's `output.units` has ten entries, one per printed unit in print order, each with its `key`, and the two failing keys list the criterion id under `failed` while the other eight list nothing.
- [ ] The stub receives exactly ten requests (one per unit for the one criterion), and each request's only input is that unit's own text, with no other unit's text in it.
- [ ] A routing decider consultation in the same `koto next` still runs and draws from its own four-slot cap, which the ten unit requests did not spend.
- [ ] A second `koto next` in the same visit, with the units unchanged, sends the stub no new requests and returns the same two failing unit keys.
- [ ] A template without `units` behaves as before: the same findings (none carrying a `unit` field), the same records, and the same shared-cap use as on the previous release for a single-slice check, and its gate output has no `units` key.
- [ ] When the extraction prints a unit key that starts with `-`, or prints two lines with the same key, every criterion is `unanswered` with `extraction_failed`, the stub receives no request, and the response names no unit.

**Check**: Run `cargo test --test decider_units_next` (the integration test added with this step, run through the repo's usual test setup). It compiles a template with `units: jsonl`, feeds the ten-unit fixture through an extraction command, points the decider at the stub with two units set to fail, and drives `koto next`. The test passes only when the response shows two error findings with exactly those unit keys, a failed gate, ten entries in `output.units`, ten stub requests, and a routing consultation that still ran in the same call. It also runs the second `koto next`, the no-`units` template, and the `-` key and duplicate key cases against the same assertions listed above.

**Dependencies**: None

**Files**: `src/template/decider_check.rs`, `src/template/types.rs`, `src/template/compile.rs`, `src/decider/check.rs`, `src/cli/check_evaluator.rs`, `src/findings.rs`, `src/cli/mod.rs`

**Complexity**: testable

### Step 2: the failed criterion's rule comes back, summary first and full text on a repeat

**Goal**: A criterion can declare a `summary` and a `rule_text`. When a veto criterion fails, the blocking condition in `koto next` carries a `rules` entry (`rule_id`, `level`, `text`, `units`) at level `summary` on the first failing evaluation of that check and rule in the visit, and at level `full` on every one after it. A `rule_delivered` event is appended once the response has printed, and it never holds the text.

**Acceptance Criteria**:

- [ ] With a veto criterion that declares both `summary` and `rule_text`, the first failing `koto next` in a visit prints a `rules` entry with `level` "summary", the summary text, and the keys of the failing units; the next failing `koto next` in the same visit, even one with nothing changed, prints `level` "full" with the `rule_text`.
- [ ] After the workflow leaves the state and comes back into it, the first failing `koto next` prints `level` "summary" again.
- [ ] A criterion that declares only `summary` prints `level` "summary" with the summary text on every failing evaluation; one that declares only `rule_text` prints `level` "full" with that text from its first failure.
- [ ] A failing criterion that declares neither field adds no `rules` entry to the blocking condition.
- [ ] A single-slice veto check (no `units`) prints the same ladder: "summary" on the first failure in the visit, "full" afterwards, with no `units` in the entry.
- [ ] A shadow criterion that fails produces no `rules` entry and no `rule_delivered` event.
- [ ] The first failure of a second rule in the same visit prints `level` "summary" even though the first rule is already at "full".
- [ ] Twenty failing units of one rule print one `rules` entry listing all twenty keys and append one `rule_delivered` event.
- [ ] A criterion with an empty `summary`, a `summary` over 300 bytes, or a `rule_text` over 4,096 bytes is refused when the template compiles, with an `E-DECIDER-CHECK-RULE` error.

**Check**: Run the integration test against the stub decider (for example `cargo test --test decider_rules`). It drives a template with one veto criterion that declares both texts and checks four results. The first failing `koto next` has `rules[0].level` "summary", the summary text and the failing unit keys. A second failing `koto next` with nothing changed has `rules[0].level` "full" and the rule text. After the workflow leaves the state and re-enters it, the next failing `koto next` is "summary" again. Then the session log holds exactly one `rule_delivered` event per delivery, and a sentinel string placed in the `rule_text` and `summary` appears in no event in the log.

**Dependencies**: Step 1

**Files**: `src/cli/next_types.rs`, `src/cli/mod.rs`, `src/engine/types.rs`, `src/template/`, `src/run_journal/`, `docs/reference/session-feed.md`, `tests/`

**Complexity**: testable

### Step 3: units the decider can't grade go to the agent one by one, and the agent answers with --unit

Over-budget, past-bound, empty, busy and malformed-output units fall back or grade per unit, and `koto decider answer --unit` records per-unit answers with `unit_required`, `unit_not_expected` and `unknown_unit` refusals.

### Step 4: templates that would overrun the unit bound are refused at compile time

`E-DECIDER-CHECK-UNITS` and `E-DECIDER-CHECK-RULE` refuse bad bounds, labels and text lengths, `max_units` defaults divide the 32-consultation state bound, and templates without the new fields compile byte-identically.

### Step 5: the run journal carries unit digests, confidence and rule deliveries

Unit-mode `decider_checked`, `decider_fallback` and `decider_check_answered` records carry the key digest and declared or digested document and section, `decider_checked` carries `koto.decider.confidence_bp`, and `rule_delivered` records carry `koto.rule.extent` and `koto.rule.unit_count`, with the no-probability rule amended where koto states it.

### Step 6: older readers and the docs and skills cover unit mode and rule delivery

The koto v0.14.1 compatibility job reads logs and ledgers holding the new records, the session feed declares them, and the reference docs and koto-author and koto-user skills name every term the PRD's docs table lists.

### Step 7: the telemetry readout returns this feature's per-unit rows from exported records

A companion pull request in the telemetry definitions repository admits every new name and the confidence exception, adds a test case per new record shape, lists the new kind and fields for export, and holds the readout's query text, reviewed by the telemetry lane before it is reported ready and merged after this pull request.

## Implementation Sequence

Steps 1 to 6 run in order in this repository's one pull request. Steps 3
and 4 need only step 1, so either can run beside step 2; step 5 needs step
2's `rule_delivered` event, and step 6 comes last because the compatibility
job and the docs cover everything before it. Step 7 is the companion pull
request outside this repository: it can be written once step 5 fixes the
record shapes, and it merges after this one, so the exporter never lists a
field koto doesn't write yet.
