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

Step 1 left the surface this builds on: in unit mode each failing unit already produces an error finding whose `unit.key` names it (`UnitRef` in `src/decider/check.rs`), so a `rules` entry's `units` are the keys of that criterion's failing findings in print order. The `rule_delivered` journal record's exported field names (`koto.check.name`, `koto.rule.extent`) are step 5's; this step adds the session event and the response field.

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

**Goal**: Every (unit, criterion) the decider gives no verdict for falls back to the agent on its own while the other units keep their verdicts, a check can cap how many units it grades with `max_units`, and the agent answers one unit at a time with `koto decider answer --unit <key>`.

Steps 1 and 2 left most of the per-unit grading path in `src/cli/check_evaluator.rs`: an empty unit is `not_graded`, a unit over `max_bytes` is `unanswered` with `over_budget`, a lock wait that runs out is `busy`, a spent `UnitBudget` is `unit_limit`, and reuse and recorded answers already match on the unit. `DeciderFallback` and `DeciderCheckAnswered` already carry an optional `unit`, and `koto decider answer` already binds an answer to its fallback's unit, but it has no `--unit` flag and `latest_fallback` (`src/cli/decider_answer.rs`) ignores units. The waiting finding names the unit but its command has no `--unit`. No check can declare `max_units` yet. This step adds the missing pieces; compile-time bounds, the per-state sum and the default value are step 4's, so here a check without `max_units` is bounded only by the 32-slot `UnitBudget`.

**Acceptance Criteria**:

- [ ] With one of ten units one byte over `max_bytes`, that unit falls back with `over_budget` (a `decider_fallback` event naming its key) and its waiting finding's command carries `--unit <key>`; the other nine carry the stub's verdicts, and a unit at exactly `max_bytes` is graded.
- [ ] A check declaring `max_units: 3` over five units sends the stub exactly three requests per criterion; units four and five fall back with `unit_limit` for every criterion, whatever their size, and are not sent.
- [ ] An empty or whitespace-only unit is `not_graded` with no fallback, and an extraction that prints no units passes with every criterion `not_graded`.
- [ ] A stub reply the decider can't read, or a provider error, on one unit falls back that unit alone with the existing reason; the other units keep their verdicts.
- [ ] A `koto next` whose lock wait runs out marks every unit it would have consulted `unanswered` with `busy`, each with its own fallback, and none passes.
- [ ] `koto decider answer --unit <key>` records an answer for that unit only; the next `koto next` uses it for that unit and still waits on the others. Editing that unit's text makes the next evaluation grade it afresh.
- [ ] `koto decider answer` refuses with `unit_required` when the criterion's pending fallbacks in the latest evaluation name units and no `--unit` was given, `unit_not_expected` when they name none and one was given, `unknown_unit` when the key wasn't printed by the latest evaluation, and `no_fallback` when the unit was printed but has no pending fallback for that criterion. The success JSON carries `unit`. Each new code is listed in `docs/reference/error-codes.md`.
- [ ] Combining holds: all units passing passes; one unit waiting and none failing is pending; a unit the agent answered `fail` on a veto criterion fails the gate; one failing unit and one waiting unit fails.
- [ ] A check without `units` behaves as before: its waiting finding's command has no `--unit`, and `koto decider answer` without `--unit` works on it as it did.

**Check**: Run `cargo test --test decider_units_fallback` (the integration test added with this step, through the repo's usual test setup, against the stub decider). It drives a ten-unit fixture with one unit over budget, a five-unit `max_units: 3` fixture, an empty unit, an unreadable stub reply on one unit, and a held lock, and asserts the per-unit fallbacks and verdicts above. It then answers one waiting unit with `--unit`, ticks again, and checks that unit's answer applies while the others still wait, and drives each of the four refusals.

**Dependencies**: Step 1

**Files**: `src/cli/check_evaluator.rs`, `src/cli/decider_answer.rs`, `src/template/decider_check.rs`, `src/template/compile.rs`, `src/decider/check.rs`, `docs/reference/error-codes.md`, `tests/`

**Complexity**: testable

### Step 4: templates that would overrun the unit bound are refused at compile time

**Goal**: A state's unit-mode checks can't be declared past the 32-consultation bound: the compiler sums `max_units` times criteria over them and refuses more than 32, gives a check without `max_units` its share of the bound, and accepts the `unit_document` and `unit_section` labels the run journal will use. Templates without the new fields compile exactly as before.

Steps 2 and 3 left part of this in place. `E-DECIDER-CHECK-RULE` already refuses an empty, whitespace-only or over-long `summary` or `rule_text` (step 2). `max_units` already parses in `src/template/compile.rs` and is refused below 1 with `E-DECIDER-CHECK-UNITS`, and the evaluator reads `spec.max_units` as a per-check cap, treating no value as no cap, so today such a check is bounded only by the 32-slot `UnitBudget` (step 3). `max_units` on a check without `units: jsonl` is accepted and ignored. `validate_state_decider_checks` in `src/template/types.rs` checks the per-state criteria limit, duplicates and routing, but no unit bound. Nothing parses `unit_document` or `unit_section`.

**Acceptance Criteria**:

- [ ] A state whose unit-mode checks declare `max_units` times criteria summing to 33 is refused with `E-DECIDER-CHECK-UNITS`, naming the state and the sum; one summing to exactly 32 compiles.
- [ ] A check without `max_units` gets 32 divided by the number of criteria across the state's unit-mode checks, rounded down, written into the compiled spec: a two-criterion check defaults to 16, a three-criterion one to 10, and two one-criterion unit-mode checks in one state default to 16 each and compile. When some checks in the state declare `max_units` and others don't, the default divides what the declared ones leave, and a state where nothing is left is refused with `E-DECIDER-CHECK-UNITS`.
- [ ] At run time a check whose `max_units` came from the default caps its units at that value, as a declared one does: with the default 16 and twenty units, units seventeen to twenty fall back with `unit_limit`.
- [ ] `max_units`, `unit_document` or `unit_section` on a check without `units: jsonl` is refused with `E-DECIDER-CHECK-UNITS` naming the field.
- [ ] `unit_document` and `unit_section` accept a value in koto's name shape (letters, digits and `._:/@-`, 1 to 128 bytes, not starting with `/`, `~` or `-`) and refuse anything else with `E-DECIDER-CHECK-UNITS`; accepted values are carried in the compiled spec and in the declaration hash only when set.
- [ ] A template with no decider check, and one whose decider checks use none of `units`, `max_units`, `unit_document`, `unit_section`, `summary` and `rule_text`, compiles to the same bytes and the same declaration hash as on the previous release; a compile-compatibility snapshot pins a decider-check fixture to show it.
- [ ] `docs/reference/error-codes.md` lists each new `E-DECIDER-CHECK-UNITS` case.

**Check**: Run `cargo test --test decider_check_compile` (extended with this step, through the repo's usual test setup). It compiles the 33 and 32 sums, the defaults for one, two and three criteria and for two checks in a state, a mixed declared-and-default state, each refused field on a non-unit check, label values inside and outside the name shape, and the no-new-fields fixture against its pinned compiled bytes and hash. A stub-driven case in `tests/decider_units_fallback.rs` runs twenty units under a defaulted 16 and asserts four `unit_limit` fallbacks.

**Dependencies**: Step 3

**Files**: `src/template/compile.rs`, `src/template/types.rs`, `src/template/decider_check.rs`, `docs/reference/error-codes.md`, `tests/decider_check_compile.rs`, `tests/decider_units_fallback.rs`

**Complexity**: testable

### Step 5: the run journal carries unit digests, confidence and rule deliveries

**Goal**: The run journal carries what the telemetry readout needs and nothing an extraction printed: unit-mode `decider_checked`, `decider_fallback` and `decider_check_answered` records carry the unit's key digest and its document and section, `decider_checked` carries the decider's confidence as an integer, and each rule delivery is a `rule_delivered` record.

Steps 1 to 4 left the inputs in place. The three session events already carry an optional `unit` (`UnitRef`, with the raw key, document and section), and the relays in `src/run_journal/mod.rs` (`decider_checked`, `decider_fallback`, the `DeciderCheckAnswered` arm) don't read it yet. The `rule_delivered` session event (step 2) is appended after the response prints, but the journal doesn't relay it. The consultation keeps the decider's unrounded probabilities (`src/decider/check.rs`), and the journal holds none. Step 4 made every compiled unit-mode spec carry `max_units`, and added `unit_document` and `unit_section` to the spec. Step 4 also put those two labels into `declaration_hash`; the design leaves them out, since neither is sent to the decider, so this step takes them back out.

**Acceptance Criteria**:

- [ ] For a unit-mode check, the journal's `decider_checked`, `decider_fallback` and `decider_check_answered` records carry `koto.unit.key` as `sha256:` plus 32 hex characters of the key's digest, and `koto.unit.document` and `koto.unit.section` as the check's declared `unit_document` and `unit_section` when declared, else as the digest of the printed value, else absent. Records of a check without `units` carry none of the three and are byte-identical to before.
- [ ] No raw unit key, document or section an extraction printed appears anywhere in the run journal; a test plants a sentinel in each and searches the journal for it.
- [ ] With the stub answering pass 0.12345, fail 0.8 and unclear 0.07655, `decider_checked` carries `koto.decider.confidence_bp` 8000 (the highest probability times 10,000, rounded half up). It's present whenever an answer was read, unit mode or not, and absent on `over_budget`, `extraction_failed` and any other outcome where no answer was read.
- [ ] The no-probability rule is amended to name `koto.decider.confidence_bp` as its one exception in all four places koto states it: `docs/reference/run-journal.md`, the module documentation in `src/run_journal/mod.rs` (both spots), and the test that a planted probability isn't copied, which now checks that no other probability is.
- [ ] Each `rule_delivered` session event is relayed as one `rule_delivered` journal record with `koto.state`, `koto.check.name`, `koto.rule.id`, `koto.rule.extent` (`summary` or `full`), `koto.seq` and `koto.visit.seq`, and in unit mode `koto.rule.unit_count` equal to the delivery's key count; twenty failing units of one rule give one record with `koto.rule.unit_count` 20. The record never holds the rule text. Its field names and shape are final here, because later work adds `koto.rule.via` and `koto.rule.delivery` to this same record.
- [ ] `koto.rule.unit_count` counts every failing unit of the rule, including when the response's finding cap truncated the findings.
- [ ] `tests/run_journal_doc_test.rs`'s collision tests list every new name; none equals or is a dot-prefix of an existing name, and `koto.unit` and `koto.rule` exist only as prefixes. A record with three 128-byte names and a 128-byte rule id is written.
- [ ] `unit_document` and `unit_section` are left out of `declaration_hash`: changing either keeps the hash, while adding `units: jsonl` still moves it.

**Check**: Run `cargo test --test run_journal_test` and `cargo test --test run_journal_doc_test` (both extended with this step), plus the stub-driven cases this step adds in `tests/decider_units_fallback.rs` (through the repo's usual test setup). They drive a unit-mode check with declared labels, one without labels, and a single-slice check through `koto next` and `koto decider answer --unit`, and read the journal's records for the fields, digests, confidence, sentinel absence and the `rule_delivered` records listed above.

**Dependencies**: Step 4

**Files**: `src/run_journal/mod.rs`, `src/decider/check.rs`, `src/template/decider_check.rs`, `docs/reference/run-journal.md`, `docs/reference/session-feed.md`, `tests/run_journal_test.rs`, `tests/run_journal_doc_test.rs`, `tests/decider_units_fallback.rs`

**Complexity**: testable

### Step 6: older readers and the docs and skills cover unit mode and rule delivery

**Goal**: An older koto reads what this feature writes, and every document and skill an author or agent reads names the new surface.

Steps 1 to 5 left the session events (`decider_checked`, `decider_fallback` and `decider_check_answered` with an optional `unit` and unit labels, and the new `rule_delivered`) and their journal records, documented in `docs/reference/session-feed.md` and `docs/reference/run-journal.md`, and the error codes in `docs/reference/error-codes.md`. The compatibility job `test/compat/decider-checks-v0_14_1.sh` (CI job `decider-checks-compat-v0-14-1`, fixtures under `test/compat/fixtures-decider-checks/`) drives single-slice decider checks only. Neither the decider authoring guide (`docs/guides/decider-authoring.md`) nor the koto-author and koto-user skills mention units, `max_units`, the unit labels, `--unit`, rule texts or the `rules` field.

**Acceptance Criteria**:

- [ ] The compatibility job also drives a unit-mode decider check (with `unit_document`, `unit_section`, `summary` and `rule_text`) through a failing evaluation, a fallback, a `koto decider answer --unit`, and a rule delivery under the new build; its log holds per-unit `decider_checked`, `decider_fallback` and `decider_check_answered` events and a `rule_delivered` event, and `koto template validate-feed` accepts them.
- [ ] koto v0.14.1 runs `koto status` and `koto next` on that session after it left the check, exits 0 and reports the state the new build reports, skipping the unknown `rule_delivered` event; `koto decider report` from v0.14.1 over the ledger holding per-unit `checked` lines exits 0.
- [ ] The job's `--self-test` mutations bite on the new events: dropping a `rule_delivered` line or a per-unit line fails the job.
- [ ] The new build reads a session log and a ledger written before this feature unchanged.
- [ ] Each document names the terms beside it: the koto-author template-format reference (`units`, `max_units`, `unit_document`, `unit_section`, `summary`, `rule_text`, the 32-consultation state bound and the default share); the decider authoring guide (`units`, `max_units`, `summary`, `rule_text`, `rules`); the error-code reference (`E-DECIDER-CHECK-UNITS`, `E-DECIDER-CHECK-RULE`, `unit_required`, `unit_not_expected`, `unknown_unit`); the run-journal reference (`koto.unit.key`, `koto.decider.confidence_bp`, `rule_delivered`, `koto.rule.extent`); the session-feed reference (`rule_delivered`, the optional `unit` fields); and the koto-user skill with its command and response references (`--unit`, `rules`, `units`).
- [ ] `cargo test --test doc_names` passes with every new name resolving.
- [ ] Public text says "telemetry" for the export and names no vendor or private repository.

**Check**: Run `test/compat/decider-checks-v0_14_1.sh` and its `--self-test` against a v0.14.1 binary and the new release build, as the CI job does, and `cargo test --test doc_names` (through the repo's usual test setup). Then grep each document in the table above for each of its terms; every term is present.

**Dependencies**: Step 5

**Files**: `test/compat/decider-checks-v0_14_1.sh`, `test/compat/fixtures-decider-checks/`, `.github/workflows/validate.yml`, `docs/guides/decider-authoring.md`, `docs/reference/error-codes.md`, `docs/reference/run-journal.md`, `docs/reference/session-feed.md`, `plugins/koto-skills/skills/koto-author/references/template-format.md`, `plugins/koto-skills/skills/koto-user/`

**Complexity**: testable

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
