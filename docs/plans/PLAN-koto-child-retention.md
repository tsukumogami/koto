---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-koto-child-retention.md
milestone: "Child retention at failure terminals"
issue_count: 4
---

# PLAN: koto-child-retention

## Status

Active

Authored by /plan in single-pr mode with no issues filed, so activation needed
no approval. /work-on implements the outlines below on one branch in order.

## Scope Summary

Implement the retention shape the DESIGN chose: failure terminals keep their
session, every terminal arrival delivers its result once whether or not the
session is kept, the terminal response says which applied, and retained
descendants go with a parent koto removes or replaces.

## Decomposition Strategy

**Horizontal, ordered by dependency.** The four outlines follow the DESIGN's
Implementation Approach. Delivery on arrival lands first, so no intermediate
state of the branch retains a session that withholds its result. Retention and
the response field build on it, the descendant sweep builds on retention, and
the docs describe the finished behaviour. A walking skeleton buys nothing
here: every change sits in or next to one function, `finish_terminal_tick`,
and there is no cross-component integration risk to surface early.

The work does not split. All four outlines land as one PR under the repo's
default consolidated delivery preference; none of them is useful to a reader
alone (retention without delivery is the defect this fixes), and no hard
constraint forces separate landings.

## Issue Outlines

### Issue 1: fix(cli): deliver a terminal result once per arrival, kept or not

**Complexity**: critical

**Goal**: Make result delivery independent of `--no-cleanup`, and make the
converge read only a result recorded for a child's current arrival. Covers PRD
R2, R3, R12, R14 and R15 and DESIGN Decision 2.

**Scope**:
- In `finish_terminal_tick` (`src/cli/mod.rs`), compute
  `arrival = !record.already_recorded`. On arrival, append
  `request_store.result` to the session's own log for every terminal (map or
  not, flag or not), write the terminal-index entry, and append
  `ChildCompleted` to the parent, all outside the cleanup guard. Leg
  promotion is unchanged.
- On a tick that is not an arrival but will remove the session, re-send
  `ChildCompleted` before removal (the deferred-notice path and the flagless
  tick after a flagged one). The index entry is not re-written there.
- In `src/cli/batch.rs`, the live-child dereference uses
  `recorded_result_for_current_arrival` instead of the latest
  `request_store.result`, and the parent-copy fallback is skipped for a child
  whose own log is readable and whose current state classifies (terminal or
  not).

**Tests** (`tests/child_retention_test.rs` unless named):
`a_kept_child_delivers_its_result_to_either_parent_shape`,
`repeat_ticks_of_a_kept_child_write_nothing`,
`repeat_ticks_of_a_kept_root_write_nothing`,
`a_child_parked_without_a_result_delivers_on_its_next_tick`,
`a_rewound_child_reports_no_stale_result`,
`a_child_back_in_a_terminal_reports_no_stale_result`,
`zero_one_or_two_parent_notices_give_the_same_gate`,
`a_failed_parent_notice_keeps_the_child_until_it_is_delivered`,
`a_kept_child_ticked_without_the_flag_is_removed_and_renotifies_once`; the
updated `tests/terminal_result_test.rs`, `tests/request_dispatch.rs`,
`tests/batch_scheduler_test.rs` and `tests/gate_field_substitution_test.rs`;
the existing fallback tests in `src/cli/batch.rs`
(`gate_skips_malformed_child_result_and_falls_back_to_parent_copy`,
`mid_log_malformed_child_result_still_falls_back_to_parent_copy`) still pass.
- Update the tests that pinned the fused behaviour
  (`tests/terminal_result_test.rs`, `tests/request_dispatch.rs`) to the new
  contract, stating each change in the commit.

**Acceptance Criteria**:
- [x] A child ticked to a success terminal with no `result:` map and with
  `--no-cleanup` has exactly one `request_store.result` on its own log and one
  `ChildCompleted` on its parent's, and its parent's gate reports
  `results_in: true` and passes, for a parent with an unconditional exit and
  for one keyed on `gates.<gate>.all_complete: true`.
- [x] That parked child has exactly one terminal-index entry after its
  arrival tick, and ticking it three more times leaves its log, its parent's
  log and its terminal-index entry count unchanged.
- [x] A root kept with `--no-cleanup` at a terminal has exactly one
  terminal-index entry, and three more ticks leave its log and its index entry
  count unchanged.
- [x] A child parked at a terminal by an earlier version (no
  `request_store.result` after its last transition) gets one
  `request_store.result`, one index entry and one `ChildCompleted` on its next
  tick, and its parent's gate then reports `results_in: true`.
- [x] A child retried out of a terminal that holds an earlier arrival's
  failure result is reported by the gate as `pending`, with no result, until
  its new arrival records one.
- [x] A parent log carrying zero, one or two `ChildCompleted` events for one
  on-disk failed child's arrival yields, each time, that child listed with
  `outcome: "failure"` and the result from the child's own log.
- [x] With the parent log unwritable on a child's terminal tick, the tick
  exits 0, warns on stderr and keeps the child; the next tick with the parent
  writable appends the notice and removes the child.
- [x] A success terminal kept with `--no-cleanup` and ticked again without the
  flag is removed, its own log and the index gain nothing, and the parent gains
  at most one `ChildCompleted` matching the arrival's result.
- [x] `cargo test` for the touched modules passes; `cargo fmt --check` and
  `cargo clippy` are clean.

**Dependencies**: None

### Issue 2: feat(cli): keep a session that reaches a failure terminal

**Complexity**: critical

**Goal**: Keep every session whose terminal is `failure: true`, report
retention on the terminal response, and change what `--no-cleanup` says.
Covers PRD R1, R4, R5, R6, R7, R8, R10 and R11 and DESIGN Decision 1.

**Scope**:
- Add `terminal_retention(compiled, final_state, no_cleanup)` returning
  `Option<RetentionReason>` (`failure_terminal` wins over `no_cleanup`).
- Both terminal call sites (advance loop and `--to`) compute it before
  printing and attach `retention` to the response; `finish_terminal_tick`
  takes it and removes the session only when it is `None` and nothing
  deferred.
- `NextResponse::Terminal` (`src/cli/next_types.rs`) gains
  `retention: Option<Retention>`, serialized as
  `{"retained": false}` or `{"retained": true, "reason": "..."}`, set through a
  `with_retention` builder; other construction sites pass `None`.
- `--no-cleanup` help text: "Keep the session after it reaches a terminal
  state (a failure terminal is always kept)".
- Adjust tests that assumed a failed child is removed without the flag
  (for example `tests/batch_child_cleanup_test.rs`).

**Tests** (`tests/child_retention_test.rs`): one per criterion below, named
for what it asserts (`a_failure_terminal_keeps_a_root`,
`a_failure_terminal_keeps_a_child`, `a_directed_failure_terminal_keeps_the_session`,
`a_retained_failure_stays_on_a_flagless_tick`,
`a_directed_move_between_terminals_notifies_once`,
`the_gate_reads_a_retained_failure_for_either_parent_shape`,
`retry_failed_reaches_a_retained_child`, `rewind_reaches_a_retained_session`,
`a_removed_session_is_still_refused`, `a_new_arrival_notifies_once`,
`a_retried_child_that_succeeds_is_removed`,
`the_response_states_retention`, `the_help_text_describes_retention`,
`a_bound_failed_child_resolves_its_leg_once`,
`init_on_a_retained_root_name_is_refused`).

**Acceptance Criteria**:
- [x] A root and a child each driven to a `failure: true` terminal without
  `--no-cleanup`, through the advance loop and through `--to`, still exist;
  `koto context get` returns a key written before the terminal (the child's
  `failure_reason` included); `koto status` reports `is_terminal: true`, the
  state and a `result` with `status: "failure"`; `koto workflows` lists them.
- [x] Ticking the retained child again without the flag leaves it on disk.
- [x] The parent's gate lists the retained failed child as `failure` with its
  result and `results_in: true`, for a parent with an unconditional exit and
  for one keyed on `gates.<gate>.all_complete: true`.
- [x] `retry_failed` naming the retained child is accepted; the child's status
  shows its initial state and the gate lists it as `pending`.
- [x] `koto rewind` on a freshly retained failed child and on a retained failed
  root moves each back to its pre-terminal state and `koto status` reports
  `is_terminal: false`.
- [x] `retry_failed` naming a child removed at a success terminal is still
  refused with `unknown_children`, and `koto rewind` on a removed session still
  fails with "workflow not found".
- [x] A retried child that reaches a terminal again appends exactly one new
  `ChildCompleted`, and the gate reports the new result; a `koto next --to`
  from one terminal to another appends exactly one, carrying the new
  `final_state`.
- [x] A retained failed child that is retried and reaches a success terminal
  without `--no-cleanup` no longer exists after that tick, and the parent's
  gate lists it as `success` with the new result (read from the parent's
  `ChildCompleted`).
- [x] The `action: "done"` response carries
  `{"retained": true, "reason": "failure_terminal"}` for a failure terminal with
  or without the flag, `{"retained": true, "reason": "no_cleanup"}` for a success
  terminal with the flag, and `{"retained": false}` without it; a second tick
  of a retained failure terminal carries the same object.
- [x] `koto next --help` shows the new `--no-cleanup` text.
- [x] A leg-bound child that fails has its leg resolved once with
  `source: promoted` and the terminal as `final_state`, stays on disk, and a
  later arrival leaves the leg unchanged.
- [x] `koto init <name>` on a retained root's name is refused naming
  `koto session cleanup`.
- [x] `cargo test` for the touched modules passes; fmt and clippy clean.

**Dependencies**: <<ISSUE:1>>

### Issue 3: feat(cli): remove retained descendants with their parent

**Complexity**: critical

**Goal**: Give retained sessions an automatic end: when koto removes a parent
at its terminal, or replaces a finished parent, it removes the parent's
terminal descendants through a walk that never touches live work. Covers PRD
R9 and DESIGN Decision 3.

**Scope**:
- New `sweep_terminal_descendants(backend, parent)` in `src/cli/workspace.rs`:
  runs only when the parent's own log holds a `ChildCompleted`; walks
  `parent_workflow` links with a visited set and a depth cap; does not enter a
  non-terminal descendant; removes post-order, a session only when it is
  terminal and all its children were removed; leaves a session and its subtree
  alone on any read or classification error; re-checks status just before each
  `backend.cleanup`; warns and continues on a failed removal.
- The sweep lives in `src/cli/workspace.rs` beside prune and shares its
  `derive_terminal_status`; `collect_descendants` (prune's walk) gains a
  visited set.
- Call the sweep from `finish_terminal_tick` before a parent's removal, and
  from `init_entry`'s replace path before a finished session is replaced
  (after the old session's result is read, before it is removed).
- Trigger: the session's compiled template declares `materialize_children`,
  or its own log holds a `ChildCompleted`.

**Tests** (`tests/descendant_sweep_test.rs`, plus a unit test with a
`list()`-counting backend double in `src/cli/workspace.rs`): one per criterion
below (`removes_terminal_descendants_with_the_parent`,
`leaves_live_children_and_their_subtrees`,
`keeps_a_terminal_child_with_a_live_grandchild`,
`a_kept_parent_keeps_its_children`, `skips_an_unclassifiable_descendant`,
`stops_on_a_parent_cycle`, `prune_terminates_on_a_cycle`,
`a_leaf_tick_does_not_list_sessions`,
`replace_terminal_removes_retained_children`,
`prune_and_session_cleanup_reclaim_retained_sessions`,
`a_retained_child_with_a_lost_notice_is_still_swept`,
`skips_a_descendant_whose_template_is_missing`,
`keeps_a_descendant_whose_leg_is_still_open`).

**Acceptance Criteria**:
- [ ] A parent reaching a success terminal without the flag removes its
  retained failed child, a child it kept with `--no-cleanup` at success, and a
  terminal grandchild under that child, in the same tick.
- [ ] A non-terminal child of that parent, and a terminal child under the
  non-terminal child, are left in place.
- [ ] A terminal child with a live grandchild is left in place.
- [ ] A parent at a failure terminal, or at a success terminal with the flag,
  keeps its retained children.
- [ ] A descendant whose template file is missing is left in place and the
  parent is still removed.
- [ ] A `parent_workflow` cycle among descendants ends the sweep and the tick
  exits 0; `koto workspace prune --root` on a tree with a cycle terminates.
- [ ] A leaf session's terminal tick does not list sessions (asserted through
  a test backend that counts `list()` calls, or equivalent).
- [ ] A batch parent whose retained failed child never got its
  `ChildCompleted` through still sweeps that child when the parent is
  removed.
- [ ] `koto init <name> --attach-live --replace-terminal` on a retained root
  removes its retained terminal children and replaces it.
- [ ] `koto workspace prune --root <root> --yes` removes a retained root and
  every session under it and leaves another root's sessions untouched;
  `koto session cleanup <child>` removes one retained child and leaves its
  siblings and parent.
- [ ] `cargo test` for the touched modules passes; fmt and clippy clean.

**Dependencies**: <<ISSUE:2>>

### Issue 4: docs: describe failure-terminal retention and the retention field

**Complexity**: simple

**Goal**: Say what koto now does with a session at its terminal wherever
session lifecycle and cleanup are described. Covers PRD R13.

**Scope**:
- `docs/guides/cli-usage.md`: the retention rule, `--no-cleanup`'s new meaning,
  the `retention` response field, each reclaim path (recovery, parent removal
  or replacement, `koto workspace prune`, `koto session cleanup`), the sweep's
  reach over children kept with the flag, name reuse with a retained session,
  superseded-epoch children after a batch rewind, and that
  `koto session cleanup` on a retained parent leaves its retained children for
  `koto workflows --orphaned` and prune.
- `docs/workspace-layout.md`: retained session directories, the storage cost,
  and what removes them.
- `plugins/koto-skills/skills/koto-user/SKILL.md`,
  `references/command-reference.md`, `references/response-shapes.md`: the same
  facts at the depth each file keeps; no mention of `--no-cleanup` as a
  debugging aid or as withholding a result. Assess `koto-author` and
  `koto-adhoc` per the repo's skill-maintenance rule.
- Functional feature files under `test/functional/features/` that describe
  cleanup, updated if their scenarios change.
- `CHANGELOG.md`: an Unreleased entry.

**Acceptance Criteria**:
- [ ] `docs/guides/cli-usage.md` and `docs/workspace-layout.md` state that a
  failure terminal is kept, name the `retention` field and its three values,
  list each reclaim path, and note that a retained failure is now the common
  case for the name-reuse refusal.
- [ ] The three koto-user files name the `retention` field, and none of the
  files in `src/`, `docs/` or `plugins/` describes `--no-cleanup` as a
  debugging aid or as keeping a child's result from its parent.
- [ ] `CHANGELOG.md` has an Unreleased entry covering retention, delivery
  under `--no-cleanup`, the response field and the sweep.
- [ ] `cargo test --test doc_names` passes.
- [ ] A full `cargo test -- --test-threads=1` passes before the PR is marked
  ready, and the PR body maps each PRD acceptance criterion to the test that
  exercises it.

**Dependencies**: <<ISSUE:2>>, <<ISSUE:3>>

## Implementation Sequence

The critical path is 1, 2, 3, 4, and it is the whole plan: each outline
changes code the next one builds on, so there is no parallel track. Issue 4
could start once issue 2 lands, but the sweep's reach is part of what it
documents, so it waits for issue 3.
