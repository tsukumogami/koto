---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
upstream: docs/designs/DESIGN-koto-value-routing.md
milestone: "koto value routing"
issue_count: 4
---

# PLAN: koto value routing

## Status

Active

## Scope Summary

Implement `vars.NAME: <value>` routing in `when` and `skip_if` as the design
describes: the shared matcher, the four `E-VAR-ROUTE-*` compile checks, the
`vars_matched` and `previous` event fields, their documentation, and a
koto v0.14.1 compat job, landed as one pull request.

## Decomposition Strategy

**Horizontal decomposition, one pull request.** The engine and compiler
change is a prerequisite for everything else and must land as one unit:
the matcher alone would bring dead `skip_if` value conditions to life before
the compile checks exist (design, Implementation Approach step 2). The
integration tests, the documentation, and the compat job each consume the
finished engine behavior and are independent of one another. The repository
delivers consolidated by default and no split trigger applies: nothing here
is useful to a reader without the engine change, and there is no landing
order across repositories.

## Issue Outlines

### Issue 1: feat(engine): route on a variable's value in when and skip_if

**Goal**: Match `vars.NAME: <string>` by exact equality in the shared
matcher, refuse the four mistakes at compile time, and record
`vars_matched` and `variables_rebound.previous` (PRD R1 to R14).

**Acceptance Criteria**:
- [ ] `condition_holds` matches a string on a `vars.*` key by exact equality
  against the variable map; `conditions_satisfied` uses the same helper; a
  `vars.*` key never falls through to the evidence lookup.
- [ ] Unit tests: exact matching for each allowlist character, case and
  prefix mismatches, empty variable matches no value, AND with evidence
  keys, no-match stops at `evidence_required`, fallback taken on fresh
  evidence, `skip_if` value condition fires and doesn't fire.
- [ ] `E-VAR-ROUTE-UNDECLARED`, `E-VAR-ROUTE-VALUE`, `E-VAR-ROUTE-CAPTURE`
  refused in both `when` and `skip_if`; `E-VAR-ROUTE-OVERLAP` for equal
  values and for a value beside `{is_set: true}` on a shared `vars.*` key;
  the compiling negatives from the PRD compile; existing `is_set` and
  mutual-exclusivity tests pass unchanged.
- [ ] `Transitioned.vars_matched` is written from the taken edge (plus the
  `skip_if` map on a `skip_if` advance) and absent otherwise, including a
  two-variable clause; `VariablesRebound.previous` carries old values, is
  skipped when empty, and an unchanged re-application appends nothing.
- [ ] A template without value conditions compiles byte-identically;
  `tests/compat_baseline_test.rs` passes unchanged; `format_version` is
  unchanged.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` pass.

**Dependencies**: None

**Type**: code
**Files**: `src/engine/advance.rs`, `src/engine/types.rs`, `src/engine/variables.rs`, `src/template/types.rs`

### Issue 2: test: drive value routes end to end through the CLI

**Goal**: Prove the routing, records, init refusals and decider interaction
through `koto init`, `koto next` and an attach, on a fixture template.

**Acceptance Criteria**:
- [ ] A fixture with `MODE` routes advances to the `auto` or `interactive`
  target on entry according to `--var MODE=...`, with no evidence.
- [ ] The resulting `transitioned` event carries `vars_matched`;
  `workflow_initialized.variables` holds a passed, a defaulted and an empty
  variable.
- [ ] `koto init` refuses a value outside `values:` and one failing
  `pattern:` with exit 2 and no session directory.
- [ ] An `--attach-live` that changes a rebind variable appends
  `variables_rebound` with `previous`, and the next `koto next` routes on
  the new value.
- [ ] A state with a decider-declared field and value routes never moves to
  a target the variable's value doesn't allow; existing `E-DECIDER-FLOOR`
  tests pass unchanged.
- [ ] A fixture copying shirabe `/work-on`'s `entry` state (its source
  commit noted in the fixture) compiles, and after `mode: plan_backed` is
  submitted it reaches `plan_context_injection` both with `ISSUE_SOURCE`
  set to `plan_outline` (logged as `skip_if`) and unset (logged as `auto`).

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `tests/`, `test/functional/`

### Issue 3: docs: document value routing for authors, agents and log readers

**Goal**: Document the matcher, codes and fields in the references and both
skills, with a test that keeps the error-code reference complete, and run
the skill evals (PRD R16).

**Acceptance Criteria**:
- [ ] The template-format reference has a compiling value-route example,
  the matching rules, the routable-variable rule and the no-match behavior,
  and names every `E-VAR-ROUTE-*` code.
- [ ] `docs/reference/error-codes.md` lists the four codes; a test fails if
  the compiler emits an `E-VAR-ROUTE-*` code the reference doesn't list.
- [ ] `docs/reference/session-feed.md` documents `vars_matched` and
  `previous` as optional additive fields, in both the prose catalogue and
  the schema block, and `koto template validate-feed` accepts a value-routed
  session's log.
- [ ] The koto-user skill says a value-routed state can advance on entry,
  that `vars.*` keys in `expects.options` are not submittable, and where
  `vars_matched` appears; the koto-author skill shows a value route and the
  codes.
- [ ] `cargo test --test doc_names` passes; `scripts/run-evals.sh` runs for
  koto-author and koto-user and the graded result is recorded for the pull
  request.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: docs
**Files**: `plugins/koto-skills/skills/koto-author/`, `plugins/koto-skills/skills/koto-user/`, `docs/reference/error-codes.md`, `docs/reference/session-feed.md`

### Issue 4: ci: prove koto v0.14.1 reads value-routed session logs

**Goal**: A compat script and CI job, patterned on the decider-checks job,
showing koto v0.14.1 reads a log the new build wrote through value routes
and a rebind (PRD R15).

**Acceptance Criteria**:
- [ ] `test/compat/value-routing-v0_14_1.sh` drives a fixture through a
  value route, a `skip_if` value route and a rebind that changes a routed
  variable, stopped at a state with no value routes, and checks the log
  holds `vars_matched` and `previous`.
- [ ] koto v0.14.1's `koto status`, `koto next` and `koto context get` on
  that session exit 0, report the new build's state name, and return the
  new build's context value.
- [ ] `--self-test` passes only when the clean run passes and every mutated
  run (dropped event, stripped field, dropped transition) fails.
- [ ] A `value-routing-compat-v0-14-1` job in `.github/workflows/validate.yml`
  runs the script and its self-test with the pinned v0.14.1 installer.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `test/compat/value-routing-v0_14_1.sh`, `test/compat/fixtures-value-routing/`, `.github/workflows/validate.yml`

## Implementation Sequence

**Critical path:** Issue 1 -> Issue 3 (docs and evals run last, against the
final behavior).

**Recommended order:**
1. Issue 1 -- the engine and compiler change, with unit tests.
2. Issue 2 -- CLI integration tests on the finished behavior.
3. Issue 4 -- the compat script and job.
4. Issue 3 -- documentation, the error-code completeness test, and evals.

**Parallelization:** after Issue 1, Issues 2, 3 and 4 are independent.
