---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
upstream: docs/designs/DESIGN-koto-decider-checks.md
milestone: "Decider checks"
issue_count: 6
---

# PLAN: Decider checks that grade the agent's work

## Status

Active

## Scope Summary

Implement the `decider-check` gate type from
`docs/designs/DESIGN-koto-decider-checks.md`, meeting
`docs/prds/PRD-koto-decider-checks.md` R1 to R34, in one pull request:
declaration and compile rules, the pure outcome logic, the run-time
evaluator and its records, overrides and the report, the documentation, and
the koto v0.14.1 compatibility job.

## Decomposition Strategy

Horizontal. The design's components have stable interfaces between them: the
compiled `Gate` fields and the pure functions in `src/decider/check.rs` are
fixed before the evaluator consumes them, and the records the evaluator
writes are what the override path, the report, the contract and the
compatibility job read. Each layer is built fully and tested before the next
uses it. A walking skeleton would add little, because the one integration
seam that carries risk (the gate arm, the engine append and the stub-driven
tests) is a single outline on its own.

The work lands as one pull request: the repository has no delivery
preference header, so the consolidated default applies, and no hard
constraint or incremental-value case splits it. Each outline is independently
testable within that pull request.

## Issue Outlines

### Issue 1: feat(template): declare decider checks and refuse malformed ones

**Goal**: A template can declare a `decider-check` gate with an extraction
command, timeout, byte budget, label and one to four criteria, compiled with
defaults resolved, and every malformed declaration is refused with its own
`E-DECIDER-CHECK-*` code, while templates without the gate compile
byte-identically.

**Acceptance Criteria**:
- [ ] `GATE_TYPE_DECIDER_CHECK` is supported; the source keys `max_bytes`,
      `label` and `criteria` compile into one optional `Gate.decider_check`
      field, skipped when absent, and `Gate::substitutable_fields` names it
      (only `command` substitutes).
- [ ] A compiled criterion with nothing optional declared has mode `shadow`,
      threshold 0.9, and the check a budget of 2,560 and label `artifact`.
- [ ] Each refusal in the PRD's compile acceptance criteria (missing
      question, description, `rule_id` or `rule_ref`; threshold 0.49 or
      1.01; duplicate `rule_id`; a fifth criterion on a state; budget 0 or
      over 8,192; bad label; unknown mode; `overridable: false`; `poll:`;
      decider-check fields on another gate type) is refused with its
      distinct code and a message naming state, check and criterion; 0.5 and
      1.0 compile.
- [ ] A `when` clause, `skip_if` condition or assignment reading a decider
      check's output is refused with `E-DECIDER-CHECK-ROUTE`, and strict
      mode doesn't demand `gates.*` routing for a decider check.
- [ ] `gate_type_schema` and `built_in_default` cover the type
      (`failed`, `unanswered`, `error`).
- [ ] `tests/compat_baseline_test.rs` passes unchanged.

**Dependencies**: None

**Type**: code
**Files**: `src/template/types.rs`, `src/template/compile.rs`

### Issue 2: feat(decider): grade one criterion as pass, fail or escape

**Goal**: Pure, provider-neutral logic in `src/decider/check.rs` turns a
criterion and a slice into a request, a provider answer into an outcome, and
an outcome and mode into a blocking decision and a record, with no I/O.

**Acceptance Criteria**:
- [ ] `build_check_request` yields one `Choice` question with the template's
      question, `pass`/`fail` descriptions and the escape `unclear`, and the
      slice only as the labelled input.
- [ ] `verdict` gives `pass` or `fail` only when that value is strictly the
      highest and at least the threshold, `escape` otherwise (ties
      included); fail 0.9 is `fail`, fail 0.89 is `escape`; a missing,
      non-numeric, out-of-range or badly summed answer is unanswered with
      `unreadable_response`.
- [ ] `blocks` blocks only `fail` and unanswered, and only in veto.
- [ ] `declaration_hash` changes with the question, a description, the
      unsubstituted command, the budget or the label, and not with the mode,
      threshold or `rule_ref`.
- [ ] `DeciderCheck` serializes every field the design's table lists, and
      `LedgerRecord` gains `checked` and `check_overridden`, whose lines the
      ledger reader accepts; a `checked` line over the line bound drops
      `probabilities` and is marked `trimmed`.

**Dependencies**: None

**Type**: code
**Files**: `src/decider/check.rs`, `src/decider/mod.rs`, `src/decider/ledger.rs`

### Issue 3: feat(gate): evaluate decider checks and record each consultation

**Goal**: For an opted-in user, the advance loop evaluates a decider check
through a CLI evaluator that extracts, redacts, bounds, reuses or consults
under the shared cap and lock with one retry, blocks veto criteria on a fail
or an unanswered consultation with the right finding, and records a
`decider_checked` event and a ledger line per consultation; opted-out users
see a template without the checks.

**Acceptance Criteria**:
- [ ] Stub-driven tests cover every outcome row of the PRD's outcome table
      in both modes: a veto fail makes the check `failed` with a finding
      carrying the criterion's `rule_id`, `rule_ref` and `message_source`
      `decider`; an unanswered veto criterion makes it `error` with one
      finding named for the check (`message_source` `koto`, `no verdict was
      read` prefix) and never a finding with the criterion's `rule_id`;
      shadow is always `passed` with empty lists.
- [ ] `decider_checked` carries `visit_seq` from the arrival-or-rewind
      boundary (a self-transition keeps it) and `input_tokens` and
      `output_tokens` when the stub reports usage.
- [ ] Retry: timeout, 503, malformed and mismatched answers get exactly two
      requests; 401 gets one; a first-attempt failure then answer records
      `attempts` 2 and counts once against the cap.
- [ ] The slice is redacted before measurement; 2,560 bytes is consulted and
      2,561 is `over_budget`; empty is `not_graded`; a failing or timed-out
      command is `extraction_failed` and not retried.
- [ ] Reuse within a visit sends nothing and appends nothing, and a reused
      fail still blocks; a changed slice, an unanswered outcome and a new
      visit are consulted again.
- [ ] The shared budget: a routing consultation on an earlier state leaves a
      criterion `cap_spent`, consulted on the next call; a held
      `decider.lock` gives `busy` with no request.
- [ ] Veto needs effective mode `auto`: user `shadow`, or project `shadow`
      over user `auto`, records mode `shadow` and never blocks.
- [ ] Opted out (mode `off`, or no key): no command runs, nothing is sent or
      recorded, and the response equals the one for the template without the
      checks.
- [ ] A pass leaves the state as it would be without the check; `koto next
      --to`, `koto status` and the polling action loop never run an
      extraction command.
- [ ] Records carry every field the PRD lists and never the slice, the key,
      or the response body; `gate_evaluated` for a decider check carries no
      streams.
- [ ] A stub that times out on every request bounds a four-criterion `koto
      next` at eight timeouts plus the commands and one second.

**Dependencies**: Blocked by <<ISSUE:1>>, <<ISSUE:2>>

**Type**: code
**Files**: `src/cli/check_evaluator.rs`, `src/gate.rs`, `src/findings.rs`, `src/engine/advance.rs`, `src/engine/types.rs`, `src/cli/mod.rs`, `src/cli/decider_port.rs`

### Issue 4: feat(decider): record overrides and tally checks in the report

**Goal**: An override of a blocking decider check writes one ledger record
per blocking criterion (candidate false fail or overridden unanswered), and
`koto decider report` tallies every criterion's outcomes and overrides.

**Acceptance Criteria**:
- [ ] Overriding a check with one failing and one unanswered veto criterion
      moves the state on; `actual_output` lists each under its kind; the
      ledger gains one `check_overridden` record of each kind, with
      `visit_seq` and `declaration_hash` taken from the visit's
      `decider_checked` events, reused verdicts included.
- [ ] After the override, later `koto next` calls in the visit run no
      extraction command.
- [ ] `koto decider report` and `--json` over a ledger holding every
      outcome and both override kinds show per-criterion,
      per-declaration-hash counts of pass, fail, escape, unanswered (with
      reasons), not graded, candidate false fail and overridden unanswered.

**Dependencies**: Blocked by <<ISSUE:3>>

**Type**: code
**Files**: `src/cli/mod.rs`, `src/decider/report.rs`

### Issue 5: docs: document decider checks and ship the two demonstration criteria

**Goal**: Authors, agents and feed consumers can find everything the feature
adds: the session-feed contract, error codes, authoring guide,
template-format reference and the koto-author and koto-user skills describe
decider checks, and fixture templates declare the comment and
acceptance-criterion criteria in shadow.

**Acceptance Criteria**:
- [ ] `docs/reference/session-feed.md` documents `decider_checked`, the
      decider-check `gate_evaluated` output, the `decider` message source,
      and the two ledger kinds field by field; the contract test and
      `koto template validate-feed` accept a log from the stub tests, with
      `schema_version` 1.
- [ ] `docs/reference/error-codes.md` lists every `E-DECIDER-CHECK-*` code
      with an example and a remedy.
- [ ] The authoring guide, template-format reference, and the koto-author
      and koto-user skills each have a decider-check section covering
      criteria, modes, the two findings and the override.
- [ ] The two fixture templates compile with both criteria in shadow, and
      their veto variants are exercised by the Issue 3 tests.
- [ ] `cargo test --test doc_names` passes.

**Dependencies**: Blocked by <<ISSUE:3>>, <<ISSUE:4>>

**Type**: docs
**Files**: `docs/reference/session-feed.md`, `docs/reference/error-codes.md`, `docs/guides/decider-authoring.md`

### Issue 6: ci: prove koto v0.14.1 reads logs and ledgers with decider checks

**Goal**: A CI job in `.github/workflows/validate.yml` shows koto v0.14.1
reads a session that passed through a decider check and a ledger holding the
new kinds, with a self-test that shows the check bites.

**Acceptance Criteria**:
- [ ] A script under `test/compat/` drives the new build through a
      decider-check state against the local stub until the session leaves
      it, then runs v0.14.1's `koto status` and `koto next` on the session
      (exit 0, same state) and `koto decider report` on the ledger (exit 0).
- [ ] Its `--self-test` fails when a `decider_checked` event or a ledger
      line is dropped or altered.
- [ ] The job is required by the aggregate `validate` job, and `cargo tree`
      on default features lists no crate `main` doesn't.

**Dependencies**: Blocked by <<ISSUE:3>>, <<ISSUE:4>>

**Type**: code
**Files**: `.github/workflows/validate.yml`

## Implementation Sequence

Critical path: Issue 1 and Issue 2 (in parallel), then Issue 3, then Issue 4,
then Issues 5 and 6 (in parallel). Issue 3 is the largest and the only one
that touches the engine and the CLI's tick; it lands after both inputs it
consumes are fixed so its tests exercise final shapes.
