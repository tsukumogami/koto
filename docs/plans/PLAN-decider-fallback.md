---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-decider-fallback.md
milestone: "Decider fallback to the agent"
issue_count: 5
---

# PLAN: decider-check fallback to the agent

## Status

Active

## Scope Summary

Make a decider-check criterion that gets no verdict fall back to the agent,
which answers it with `koto decider answer`, and record every fallback as a
`decider_fallback` event the run journal exports, as
`docs/designs/DESIGN-decider-fallback.md` specifies against
`docs/prds/PRD-decider-fallback.md`.

## Decomposition Strategy

Horizontal, in one pull request. The design's five phases have stable
interfaces between them: the two payload types (`DeciderFallback`,
`DeciderCheckAnswered`) are the contract every later unit reads, so they
land first, and each later unit builds one layer on them. A walking
skeleton buys nothing here: the end-to-end path is a single evaluator call,
and its integration risk is in the evaluator itself, which Issue 2 owns
whole. The work lands as one PR under the repository's consolidated
default, and no split branch fires: a release with the fallback but no
answer command, or no journal record, would be worse than either side
alone.

## Issue Outlines

### Issue 1: feat(decider): add the fallback and answer records and events

**Goal**: Add the `AgentAnswer`, `DeciderFallback` and
`DeciderCheckAnswered` types and the `decider_fallback` and
`decider_check_answered` session-log events, declared in the session-feed
contract.

**Acceptance Criteria**:
- [ ] `src/decider/check.rs` defines `AgentAnswer` (`pass`, `fail`,
      `unclear`, with `as_str` and `parse`), `DeciderFallback` and
      `DeciderCheckAnswered` with the fields the design's Key Interfaces
      list; optional hashes and sizes are skipped when absent.
- [ ] `EventPayload::DeciderFallback` and `EventPayload::DeciderCheckAnswered`
      sit before `InstructionsDelivered`, have `type_name` arms, and a
      payload that won't parse degrades to `Unknown`; round-trip and
      degrade tests cover both.
- [ ] Both events are declared in the frontmatter and body of
      `docs/reference/session-feed.md`, and `src/cli/validate_feed.rs`'s
      declared-events test lists them.
- [ ] `cargo test` passes with no behavior change: nothing writes the
      events yet.

**Dependencies**: None

**Type**: code
**Files**: `src/decider/check.rs`, `src/engine/types.rs`, `docs/reference/session-feed.md`, `src/cli/validate_feed.rs`

### Issue 2: feat(gate): fall back to the agent when a decider check gets no verdict

**Goal**: Make the check evaluator hand every criterion with no verdict
and no applicable answer to the agent, read answers back by slice, and
make the engine block on waiting criteria (PRD R1, R2, R3, R6 to R10,
R13, R15).

**Acceptance Criteria**:
- [ ] The evaluator resolves each criterion in the design's order
      (`NotGraded`; reusable decider verdict; applicable agent answer;
      early outcome, `Busy`, `CapSpent` or a consultation), and any
      `Unanswered` result is a fallback with a `DeciderFallback` record on
      `StructuredGateResult::decider_fallbacks`.
- [ ] The gate output always carries `failed`, `unanswered`, `awaiting`,
      `agent_answered` and `error` as the design's Decision 3 defines them;
      the gate is `failed` when `failed` is non-empty, else `pending` when
      `awaiting` is non-empty, else `passed`; a shadow check is never
      `failed`, and an agent-answered criterion is never under
      `unanswered`.
- [ ] A `pending` evaluation stamps no attempt and writes no rule counts,
      and `blocking_conditions_from_gates` renders a pending decider check
      as `corrective` with its findings.
- [ ] A waiting criterion gets one `warning` finding (`message_source:
      koto`) whose message starts with the reason and the quoted
      `koto decider answer` line, then the question and three
      descriptions; an agent `fail` on a veto criterion gets an `error`
      finding quoting the fail description, and on a shadow criterion is
      recorded under `agent_answered` and doesn't block.
- [ ] The engine blocks a decider check when `failed` or `awaiting` is
      non-empty, whatever the state accepts, and appends
      `decider_fallback` events after `decider_checked` and before
      `gate_evaluated`, failing the tick on an append error.
- [ ] `override_records` writes no candidate false fail for an
      `agent_answered` or `awaiting` id; an `override_default` with only
      the old three keys still compiles and moves past a waiting
      criterion.
- [ ] The unreadable-log path is `pending`, lists every criterion under
      `awaiting`, keeps `log_unreadable`, has one `warning` finding named
      for the check, and appends nothing.
- [ ] Tests cover each `UnansweredReason` in veto and shadow (one
      enumerating the enum), pass and `unclear` and `fail` answers on the
      same slice, re-answering, a changed slice, a changed declaration, an
      earlier visit, `extraction_failed` reuse, a decider verdict ahead of
      an answer, the three-criterion mixed case, evidence not routing past
      a waiting check, no `rule_counts` for a waiting criterion, and a
      marker-string test that no slice text reaches any event or finding.
- [ ] Each existing test changed from the old missing-verdict-passes rule
      is listed, with the rule it pinned, for the pull request body.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/cli/check_evaluator.rs`, `src/gate.rs`, `src/engine/advance.rs`, `tests/decider_check_run.rs`, `tests/decider_check_fixtures.rs`

### Issue 3: feat(cli): add koto decider answer

**Goal**: Let the agent record its answer to a waiting criterion with
`koto decider answer <session> --gate <check> --criterion <id> --answer
<pass|fail|unclear>` (PRD R4, R5).

**Acceptance Criteria**:
- [ ] The subcommand loads the session as `koto overrides record` does,
      binds the answer to the latest `decider_fallback` for the check and
      criterion in the current visit, and appends one
      `decider_check_answered` with every field the design lists; it
      prints `{"status":"recorded",...}` and exits 0.
- [ ] It refuses `invalid_answer`, `unknown_criterion` and `no_fallback`
      with exit 2, the code in the JSON error, and no event appended; it
      never consults the decider or takes its lock.
- [ ] An answer recorded while another `koto next` holds the decider lock
      is appended at once and applies to the next evaluation.
- [ ] The three codes are in `docs/reference/error-codes.md`, the command
      is in `docs/guides/cli-usage.md`, and `tests/doc_names.rs` passes.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/cli/decider_answer.rs`, `src/cli/decider.rs`, `src/cli/mod.rs`, `docs/reference/error-codes.md`, `docs/guides/cli-usage.md`

### Issue 4: feat(journal): relay decider_fallback and the new decider attributes

**Goal**: Export each fallback as a `decider_fallback` run-journal record
and add `koto.decider.input_bytes`, `koto.decider.awaiting_count` and
`koto.decider.agent_answered_count` (PRD R11, R12, R18).

**Acceptance Criteria**:
- [ ] `relays()` and `relayed()` write one `decider_fallback` record per
      committed `decider_fallback` event with the attributes the design's
      table names; `decider_checked` records carry
      `koto.decider.input_bytes`; decider-check `check_evaluated` and
      `check_pending` records (the latter without `koto.poll.*`)
      carry both counts, 0 included; a test pins every value.
- [ ] A test collects every attribute name the journal writes across all
      kinds and fails if a new name equals another or either starts with
      the other followed by `.`.
- [ ] A `decider_fallback` record built from 128-byte names is at most
      4,096 bytes.
- [ ] `docs/reference/run-journal.md` has the kind in its table, a
      `### decider_fallback` section and the new rows, and
      `tests/run_journal_doc_test.rs` and the journal kind lists in
      `src/run_journal/tests.rs` and `tests/run_journal_checks_test.rs`
      include it.

**Dependencies**: Blocked by <<ISSUE:1>>, <<ISSUE:2>>

**Type**: code
**Files**: `src/run_journal/mod.rs`, `src/run_journal/tests.rs`, `tests/run_journal_doc_test.rs`, `tests/run_journal_checks_test.rs`, `docs/reference/run-journal.md`

### Issue 5: docs: prove v0.14.1 compatibility and describe the fallback

**Goal**: Update the v0.14.1 decider-checks compat proof, every doc and
skill that says a missing verdict passes, and the koto-user eval scenario
(PRD R16, R17, R19).

**Acceptance Criteria**:
- [ ] `test/compat/decider-checks-v0_14_1.sh`'s `nv` session stops at
      `review`, is answered with `koto decider answer`, and reaches
      `work`; koto v0.14.1 runs `koto status` and `koto next` on it
      without error; its self-test fails when either new event is
      dropped; `tests/compat_baseline_test.rs` passes unedited.
- [ ] The decider authoring guide, workspace layout, koto-author template
      reference, koto-user skill and its command reference describe the
      fallback, and no line in them matching `missing verdict` or
      `unanswered` says such a criterion passes or doesn't block.
- [ ] The koto-user missing-verdict eval asserts the response runs
      `koto decider answer`, and `scripts/check-evals-exist.sh` passes;
      the full eval run is left to the release's eval precondition.
- [ ] `docs/prds/PRD-koto-decider-checks.md` and
      `docs/designs/current/DESIGN-koto-decider-checks.md` each carry a
      note that `PRD-decider-fallback` supersedes their
      missing-verdict-passes rule.

**Dependencies**: Blocked by <<ISSUE:2>>, <<ISSUE:3>>, <<ISSUE:4>>

**Type**: docs
**Files**: `test/compat/decider-checks-v0_14_1.sh`, `docs/guides/decider-authoring.md`, `docs/workspace-layout.md`, `plugins/koto-skills/skills/koto-user/SKILL.md`, `plugins/koto-skills/skills/koto-user/evals/evals.json`, `plugins/koto-skills/skills/koto-author/references/template-format.md`

## Implementation Sequence

Critical path: Issue 1, then Issue 2, then Issue 4, then Issue 5. Issue 3
needs only Issue 1 and can be built alongside Issue 2. All five land in one
pull request; the order is the commit order, so every commit builds and
its tests pass.
