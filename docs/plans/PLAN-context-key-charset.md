---
schema: plan/v1
status: Active
execution_mode: single-pr
tracking_level: none
upstream: docs/designs/DESIGN-context-key-charset.md
milestone: "Context Key Charset Reconciliation"
issue_count: 4
---

# PLAN: Reconciling the Value and Context-Key Character Sets

## Status

Active

## Scope Summary

Land the diagnostic and documentation half of koto#227: one place that words a
context-key refusal, both surfaces using it, a third outcome on `koto context
exists`, and the two grammars written down where an author reads. The decision
to keep the grammars divergent is recorded in
`docs/designs/DESIGN-context-key-charset.md` and is not implemented as a code
change, because the decision is to change neither one.

## Decomposition Strategy

**Horizontal decomposition.** The design describes one shared helper and two
call sites that consume it, with a documentation layer on top. The helper is a
strict prerequisite for both call sites, so there is no vertical slice that
exercises the end-to-end path without it -- a walking skeleton would be the
helper plus a stub, which is the helper.

**Execution mode: single-pr.** The repo declares no delivery preference, so the
default applies and the work lands as one pull request. None of the three escape
branches fires. There is no hard constraint: nothing here has to reach the
default branch before the next step can run, and no merge gate sits between the
units. There is no incremental value in a split either -- a reader who met the
message helper alone would have a function nothing calls, and a reader who met
the CLI change without the documentation would have an exit status nothing
explains. And the repo has stated no preference for atomic delivery.

The four units below are sequencing inside one PR, not four pull requests.

## Issue Outlines

### Issue 1: One place words a context-key refusal

**Goal**: Add a companion to `validate_context_key` in
`src/session/validate.rs` that turns a refusal into the operator-facing
sentence, and re-point `src/gate.rs`'s `unusable_key_result` at it so the gate
stops composing its own text. Satisfies R4 and the message half of R3.

**Acceptance Criteria**:

- A public function beside `validate_context_key` takes a key and returns
  `None` when the key is usable and `Some(String)` when it is not.
- The returned string names the offending character and the component it
  appears in, and says a variable value may hold a space, `:` or `@` where a
  context key may not.
- `unusable_key_result` composes no message text of its own; it calls the
  companion and keeps its `Option<StructuredGateResult>` signature and its
  `field` parameter.
- The gate evidence shapes are unchanged: an unusable key still produces
  `{"exists": false, "error": <reason>}` and `{"matches": false, "error":
  <reason>}`.
- Unit tests cover a key refused for a space, for a `:`, for an `@`, for a
  leading hyphen, and an empty key, plus a usable key returning `None`.
- The existing tests in `tests/gate_field_substitution_test.rs` pass unchanged,
  which is the guard that the wording did not lose content in the move.

**Dependencies**: None

**Complexity**: testable.

### Issue 2: `koto context exists` reports a third outcome

**Goal**: Teach the CLI's existence probe to distinguish a key it cannot use
from a key that is not there, using the companion from Issue 1. Satisfies R2
and R5.

**Acceptance Criteria**:

- `handle_exists` in `src/cli/context.rs` calls the companion before the store
  and returns a three-outcome value rather than a bool.
- The `ContextCommand::Exists` arm maps present to exit 0, absent to exit 1, and
  unusable to exit 2, which is koto's existing status for input the caller must
  fix.
- The unusable case emits the flat `{"error": ..., "command": "context exists"}`
  body koto's other verbs use; the present and absent cases still emit nothing.
- `ContextStore::ctx_exists` is unchanged in signature and behaviour, and its
  doc comment names the companion as the way to tell the two cases apart.
- An integration test asserts all three statuses for the same session: a present
  key, an absent-but-valid key, and `Weekly Planning-note`.
- That test asserts the unusable status is non-zero as well as asserting it is
  2, so a future change that made it zero fails on the property rather than only
  on the number.

**Dependencies**: Issue 1

**Complexity**: testable.

### Issue 3: The motivating case is covered end to end

**Goal**: Cover the case koto#227 was filed about, which has no test today: a
value koto documents as legal flowing through substitution into a context gate's
key. Satisfies R5 of the PRD's acceptance criteria and demonstrates the
regression against `main`.

**Acceptance Criteria**:

- A test drives a `context-exists` gate whose `key` is `"{{TITLE}}-note"` with
  `TITLE` set to a value carrying a space, and asserts the gate reports an error
  whose message names the space -- not a bare `{"exists": false, "error": ""}`.
- The same test covers a value carrying a `:` and a value carrying an `@`.
- A test asserts that the gate's message and the CLI's message for one key are
  the same string, so Issue 1's single-wording guarantee is mechanical.
- A test asserts a gate `key` and `pattern` still substitute and that a key
  resolving to nothing still produces the empty-key message, so nothing koto#222
  established regresses.
- The new tests are shown failing on a checkout of `main` with the test files
  copied in -- a worktree of `main`, not a stash, since the work is committed by
  this point.

**Dependencies**: Issue 1, Issue 2

**Complexity**: testable.

### Issue 4: The rule is written down

**Goal**: Put the relationship between the two grammars where an author reads,
and document the new status. Satisfies R6 and closes out R7's record.

**Acceptance Criteria**:

- `docs/guides/cli-usage.md`'s `context` section states both grammars and the
  three characters that differ, and says why the key grammar is narrower.
- The `context exists` entry documents exit 2 and its output, and the standing
  note that the probe "cannot distinguish a key that was never written from a
  store it could not read" is rewritten to match the new behaviour.
- `docs/reference/error-codes.md` carries the `context exists` condition
  alongside its other per-command entries.
- The template-authoring skill under `plugins/koto-skills/skills/koto-author/`
  states that a value may hold a space, `:` or `@` and a context key may not,
  and that a `{{KEY}}` reference inside a gate's `key` is subject to the
  narrower grammar. The other two skills are assessed and updated only where
  this change falsifies something they say.
- `cargo test --test doc_names` passes with no new entry in
  `tests/doc_names.allow`, and `cargo test`, `cargo fmt --check` and `cargo
  clippy -- -D warnings` are all clean.
- The pull request body states which sibling issues this change closes and which
  it deliberately leaves, so a reader does not have to guess whether koto#224,
  koto#225 and koto#228 were considered or missed. It closes koto#227 and none
  of the three.
- `substitute_shell_command`'s doc comment in `src/cli/mod.rs` is checked
  against this change and left alone unless it has been made wrong; koto#225
  stays named there, because this work does not close it.

**Dependencies**: Issue 2

**Complexity**: simple.

## Implementation Issues

_(omitted in single-pr mode -- the work items are the Issue Outlines above, and
no GitHub issues are filed at this tracking level)_

## Dependency Graph

## Implementation Sequence

**Order.** Issue 1, then Issue 2, then Issue 3, then Issue 4. Issue 2 depends
on Issue 1, Issue 3 depends on Issues 1 and 2, and Issue 4 depends on Issue 2.

**Critical path.** Issue 1 to Issue 2 to Issue 3. The message helper has
to exist before either surface can use it, and Issue 3's parity assertion
needs both surfaces in place to compare them.

**Parallelization.** There is little, and that is a consequence of the
horizontal shape rather than an oversight. Issue 4's documentation of the two
grammars could be drafted alongside Issue 1, but the half that documents exit
2 cannot be written before Issue 2 settles the status, so the unit is
sequenced after it rather than split in two for the sake of overlap.

**Where the regression demonstration sits.** Issue 3 is also the point at
which the work is checked against `main`: the test files are copied into a
worktree of `main` and shown failing there. Doing it earlier would demonstrate
nothing, because Issue 3 is where the tests that fail are written.
