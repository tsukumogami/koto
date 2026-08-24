---
schema: plan/v1
status: Active
execution_mode: single-pr
upstream: docs/designs/DESIGN-substitution-drift-class.md
milestone: "Substitution drift class"
issue_count: 6
---

# PLAN: closing the substitution-drift class

## Status

Active

Tracking level `none`: koto#224 and koto#228 already exist and this plan files
no new issues, so the activation creates no remote artifacts and fires on
authoring rather than on approval.

## Scope Summary

Closes koto#224 and koto#228 together by giving `ActionDecl` the enumeration
`Gate` already has, making all three accessors compiler-enforced, substituting
a `children-complete` gate's `name_filter`, and refusing both a reference in
`default_action.fallback` and a `name_filter` that resolves to empty.

## Decomposition Strategy

Horizontal. The design describes layers with stable interfaces between them --
an enumeration in `src/template/types.rs`, a substitution site and a gate
evaluator in `src/cli/mod.rs`, a compiler loop, then tests and documentation --
rather than components that interact at runtime. There is no integration risk a
thin end-to-end slice would surface early, and the design's own Implementation
Approach is already layered.

One departure from pure layering is deliberate: units 1 and 2 are kept separate
so the guard test is red between them. That failure is the evidence the
enumeration is load-bearing, and collapsing the two units would hide it.

## Issue Outlines

### Issue 1: Enumerate ActionDecl's reference-bearing fields and make every accessor exhaustive

**Goal**: `ActionDecl` gains `substitutable_fields()` and `literal_fields()`
next to the struct, all three accessors destructure `self` exhaustively, and
`name_filter` joins `Gate::substitutable_fields()` -- which leaves the existing
guard test failing on purpose.

**Acceptance Criteria**:
- [ ] `ActionDecl::substitutable_fields()` returns `("command", ..)` and
      `("working_dir", ..)`; `ActionDecl::literal_fields()` returns
      `("fallback", ..)` only when the field is `Some`.
- [ ] All three accessor bodies open with a destructuring that binds every
      field of the struct by name, so adding a field to `Gate` or `ActionDecl`
      stops the accessor compiling.
- [ ] `Gate::substitutable_fields()` includes `("name_filter", ..)` when the
      field is `Some` and omits the entry when it is `None`.
- [ ] Each accessor's doc comment says why the destructuring is exhaustive, so
      the next reader does not simplify it back to `..`.
- [ ] The doc comment on `Gate::substitutable_fields` no longer says
      `name_filter` is deliberately absent.
- [ ] `every_field_the_compiler_validates_is_one_the_tick_substitutes` fails,
      and its message names `name_filter`. Recorded as the checkpoint it is.

**Dependencies**: None

**Type**: code
**Files**: `src/template/types.rs`

### Issue 2: Substitute name_filter through the plain form

**Goal**: `substitute_gate_fields` resolves `{{KEY}}` references in
`name_filter`, preserving `Option`, which turns the guard test green.

**Acceptance Criteria**:
- [ ] `substitute_gate_fields` maps `Some(f)` to `Some(substitute_plain(f, ..))`
      and `None` to `None`. No `.as_deref().unwrap_or("")` or equivalent
      collapse appears on the path.
- [ ] `every_field_the_compiler_validates_is_one_the_tick_substitutes` passes,
      with the fixture carrying `name_filter: Some("{{TOKEN}}".into())`.
- [ ] A gate re-evaluated inside a `default_action` polling loop resolves
      `name_filter` exactly as one evaluated outside it, because both sites go
      through this helper.
- [ ] The `substitute_gate_fields` doc comment no longer names `name_filter` as
      the one gate field outside it.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/cli/mod.rs`

### Issue 3: Route the compiler's action-field validation through the accessors

**Goal**: `Template::compile` validates action-declaration references from the
enumerations rather than from two hand-written loops, and refuses a reference
in `fallback`.

**Acceptance Criteria**:
- [ ] The two hand-written reference loops over `action.command` and
      `action.working_dir` are replaced by one loop over
      `ActionDecl::substitutable_fields()`, producing the same error wording
      from one site.
- [ ] A second loop over `ActionDecl::literal_fields()` refuses any `{{KEY}}`
      reference, with an error naming the state and the field, saying
      `fallback` is literal prose that is never expanded, and pointing at the
      directive as where a reference resolves.
- [ ] A `name_filter` naming an undeclared variable is refused, naming the
      state, the gate and the field -- for free, because the gate loop reads
      the accessor.
- [ ] The empty-command rejection, the absolute-`working_dir` rejection and the
      polling-timeout rule are unchanged.
- [ ] A test asserts every field `ActionDecl::literal_fields()` names is one
      the compiler refuses a reference in, and a sibling of the gate guard test
      covers `ActionDecl::substitutable_fields()`.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/template/types.rs`, `src/cli/mod.rs`

### Issue 4: Refuse a name_filter that resolves to empty

**Goal**: `evaluate_children_complete` reports an error naming the reason when
`name_filter` is present and empty after substitution, instead of evaluating as
though no filter were set.

**Acceptance Criteria**:
- [ ] `evaluate_children_complete` returns `GateOutcome::Error` when
      `gate.name_filter` is `Some` and empty, before calling
      `build_children_complete_output`.
- [ ] The error says what the empty value would have done (matched every child
      of the parent), why (a reference in the filter has no value), and carries
      a remedy -- the shape `evaluate_context_matches_gate` already uses for an
      empty resolved `pattern`.
- [ ] A gate whose `name_filter` is absent behaves exactly as it does today,
      including the existing no-vacuous-pass behaviour when zero children
      match.

**Dependencies**: Blocked by <<ISSUE:2>>

**Type**: code
**Files**: `src/cli/mod.rs`

### Issue 5: Regression coverage, demonstrated against main

**Goal**: One regression test per behaviour, each shown to fail against a
checkout of `main` rather than asserted to.

**Acceptance Criteria**:
- [ ] A test covers each of R1 through R4: `name_filter` substituting, an
      undeclared reference in `name_filter` refused at compile time, a resolved
      empty `name_filter` refused at the gate, and a reference in `fallback`
      refused at compile time.
- [ ] A test covers a `name_filter` resolving inside a polling loop.
- [ ] Each new test is run against a worktree of `main` with the test file
      copied in, and the failures are recorded. A `git stash` is not used.
- [ ] `cargo test -- --test-threads=1` passes on the branch.
- [ ] `cargo fmt --check` and `cargo clippy -- -D warnings` are clean.

**Dependencies**: Blocked by <<ISSUE:2>>, <<ISSUE:3>>, <<ISSUE:4>>

**Type**: code
**Files**: `tests/gate_field_substitution_test.rs`

### Issue 6: Correct every shipped sentence this change falsifies

**Goal**: Nothing koto ships still says `name_filter` does not substitute or
that `fallback` is unvalidated.

**Acceptance Criteria**:
- [ ] `plugins/koto-skills/skills/koto-author/references/template-format.md` no
      longer says `name_filter` is the one gate field that does not resolve a
      reference, and documents what it does now.
- [ ] `plugins/koto-skills/skills/koto-user/references/command-reference.md` is
      corrected the same way.
- [ ] `docs/guides/cli-usage.md` and
      `docs/guides/default-action-authoring.md` enumerate where substitution
      reaches correctly, including that `fallback` stays literal and that a
      reference in it is now refused.
- [ ] All three skills under `plugins/koto-skills/skills/` are assessed against
      the diff for broken contracts and new surface, per the repo's skill
      maintenance rule, and any gap is closed in the same change.
- [ ] `cargo test --test doc_names` passes with no new entry in
      `tests/doc_names.allow`.
- [ ] `CHANGELOG.md` records the change.

**Dependencies**: Blocked by <<ISSUE:2>>, <<ISSUE:3>>, <<ISSUE:4>>

**Type**: docs
**Files**: `plugins/koto-skills/skills/koto-author/references/template-format.md`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`, `docs/guides/cli-usage.md`, `docs/guides/default-action-authoring.md`, `CHANGELOG.md`

## Implementation Issues

Not populated. This plan is `single-pr` at tracking level `none`, so no GitHub
issues are filed; the work is tracked against the existing koto#224 and
koto#228, which the resulting pull request closes.

## Dependency Graph

Not populated. In `single-pr` mode the ordering lives in each outline's
**Dependencies** line above and in Implementation Sequence below.

## Implementation Sequence

**Critical path**: 1 -> 2 -> 4 -> 5. Four units deep; unit 5 cannot run until
every behaviour it tests exists, and unit 4 cannot run until the value it
inspects is actually substituted.

**Parallelization**: units 2 and 3 both unblock after unit 1 and touch
different concerns -- the runtime substitution site and the compiler -- so they
can proceed together. Units 5 and 6 both unblock after 2, 3 and 4 and are
independent of each other.

**The deliberate red checkpoint**: between units 1 and 2 the guard test fails,
naming `name_filter`. That is the mechanism working, and it is worth landing as
its own commit so the mechanism is demonstrated rather than described. Do not
collapse units 1 and 2 to avoid a red intermediate state.
