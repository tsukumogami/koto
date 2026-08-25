---
schema: plan/v1
status: Active
execution_mode: single-pr
upstream: docs/designs/DESIGN-gate-command-undelivered-capture.md
milestone: "koto#225: an undelivered capture in a gate field"
issue_count: 3
---

# PLAN: An undelivered capture reaching a gate field

## Status

Active

Tracking level is `none`: single-pr with no GitHub issues or milestone, so the
Draft -> Active gate auto-fires rather than waiting on approval.

## Scope Summary

Implements
[DESIGN-gate-command-undelivered-capture](docs/designs/DESIGN-gate-command-undelivered-capture.md):
a gate field naming a capture no state has delivered stops the tick with the
existing `capture_unset` error instead of leaving the raw `{{KEY}}` token in a
string bound for `sh -c`, the context store or the regex engine. Closes koto#225.

## Decomposition Strategy

Horizontal. The design describes one path through three layers with stable
boundaries between them -- a helper that raises a refusal, two carriers that
transport it, and a renderer that reports it -- and every component already
exists. There is no integration risk for a walking skeleton to surface early, and
a thin vertical slice would leave a half-wired refusal path in the tree.

The reviewer-facing separation of mechanical churn from behavioural change lives
at commit granularity inside issue 1 rather than at the unit boundary. Round 1 of
the plan review rejected a four-unit shape that put it at the boundary: landing
the helper's new signature and check with the callers unwrapping is not
behaviour-neutral, because a template that triggers the refusal panics on the
unwrap. The mirror split leaves `GateCaptureRefusal` used in signatures and
constructed nowhere, which `dead_code` refuses under `-D warnings`. The type, its
producer and its consumers are one unit of meaning, so they land together.

## Issue Outlines

### Issue 1: fix(cli): refuse an undelivered capture in a gate field

**Goal**: Make a gate field naming an undelivered capture stop the tick with
`capture_unset` at exit 3, from both the advance loop and the polling loop.

**Acceptance Criteria**:
- [ ] `GateCaptureRefusal { gate, field, key, producer }` exists beside
      `substitute_gate_fields` in `src/cli/mod.rs`.
- [ ] `substitute_gate_fields` takes the capture map and returns
      `Result<BTreeMap<String, Gate>, GateCaptureRefusal>`, checking every field
      of every gate that `Gate::substitutable_fields` enumerates, for all gates,
      before substituting any.
- [ ] `Gate::substitutable_fields` is read, not modified; no field is added to it.
- [ ] `StopReason::GateRefusedUnsetCapture { state, gate, field, key, producer }`
      exists and is reached from two arms of `advance_until_stop`: the gate
      block's `Err`, and a new `ActionResult::GateRefused` from the action block.
- [ ] The gate-evaluator bound `G` returns
      `Result<BTreeMap<String, StructuredGateResult>, GateCaptureRefusal>`, and
      every `advance_until_stop` call site compiles.
- [ ] The renderer emits `capture_unset` at exit 3 naming all five values, and
      picks a distinct sentence when the producing state is the state being
      resolved.
- [ ] Landed as two commits: the mechanical bound change, then the behavioural
      wiring. Neither leaves the tree broken.
- [ ] `cargo test -- --test-threads=1`, `cargo fmt --check` and
      `cargo clippy -- -D warnings` clean.

**Dependencies**: None

**Type**: code
**Files**: `src/cli/mod.rs`, `src/engine/advance.rs`

### Issue 2: test(cli): pin the gate refusal and what must not change

**Goal**: Cover both directions in `tests/gate_field_substitution_test.rs`, and
demonstrate the new tests fail against `main`.

**Acceptance Criteria**:
- [ ] A `command` gate reading an undelivered capture refuses with
      `capture_unset`, exit 3, naming the state, gate, field, capture and
      producer; a side-effect file the gate command would have written is absent.
- [ ] `key`, `pattern` and `name_filter` each get the same refusal rather than
      the unusable-key, invalid-regex and zero-child outcomes they produce today.
- [ ] The polling case produces the same reason, exit status and five values as
      the non-polling case.
- [ ] A gate reading the capture its own state's non-polling action delivers
      still resolves; the same gate under a polling action refuses with the
      self-reference wording.
- [ ] No-regression set passes: a capture delivered earlier in the same tick, a
      capture delivered on an earlier tick, a declared variable, a gate with no
      `name_filter`, and the koto#230 empty-`pattern` and empty-`name_filter`
      refusals keeping their existing messages.
- [ ] Every new test is run against a detached worktree of `main`
      (`git worktree add --detach`) with the test file copied in, and the failure
      text is recorded for the pull-request body. A `git stash` is not evidence.
- [ ] `cargo test -- --test-threads=1` clean on the branch.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `tests/gate_field_substitution_test.rs`

### Issue 3: docs(changelog): record the gate refusal and assess the skills

**Goal**: Land the user-facing record and close the doc-comment thread koto#221,
koto#222 and koto#224 each edited in turn.

**Acceptance Criteria**:
- [ ] `CHANGELOG.md` carries an entry under Unreleased / Fixed describing the
      refusal, why it is a stop rather than a gate outcome, and what does not
      change.
- [ ] The clause naming `#225` as the remaining gap is removed from the
      `substitute_shell_command` doc comment in `src/cli/mod.rs`.
- [ ] All three skills under `plugins/koto-skills/skills/` are assessed against
      the change for broken contracts and new surface, and the assessment is
      recorded with its reason even where the answer is that nothing changes.
      Any skill that needs updating is updated in the same pull request.
- [ ] `cargo test --test doc_names` passes with no new entry in
      `tests/doc_names.allow`.

**Dependencies**: Blocked by <<ISSUE:2>>

**Type**: docs
**Files**: `CHANGELOG.md`, `src/cli/mod.rs`, `plugins/koto-skills/skills/`

## Implementation Issues

## Dependency Graph

## Implementation Sequence

The critical path is all three units; there is nothing to parallelize, which is
what a horizontal decomposition of a single path looks like.

1. **Issue 1** carries the whole behavioural change and the bound churn. It is
   the only unit that can fail in an interesting way, and the two-commit shape is
   what keeps it readable.
2. **Issue 2** cannot be written before issue 1 exists to test, and its
   fail-against-`main` evidence is collected from a detached worktree rather than
   from the branch, so it is gathered after the implementation rather than before.
3. **Issue 3** documents behaviour issue 1 introduced and cites evidence issue 2
   produced.

All three land in one pull request. No unit ends with a failing suite: the
fail-against-`main` demonstration happens in a separate worktree, so nothing
requires a red commit on the branch.
