# Justification scrutiny — Issue 1 (walking skeleton), request-store-converge

Commit: `8f1aa9a` — feat(engine): walking-skeleton converge path
Reviewer focus: justification — are the reported deviations genuinely explained
and reflective of real trade-offs, or do they hide incomplete/incorrect work?

Verdict: **0 blocking, 3 advisory.** Every reported deviation checks out against
the design and the code. The clippy baseline claim is verifiably true; the
short-name fallback is sound (not a band-aid); skipping TaskView/BatchFinalView
is legitimately outside Issue 1's ACs and does not break the converge read the
issue targets.

---

## (a) Clippy `--all-targets` baseline claim — TRUE

Reported: `cargo clippy -- -D warnings` (koto CI parity, lib+bins) is clean;
`--all-targets` surfaces pre-existing errors in test code, none authored here.

Verified:
- `cargo clippy -- -D warnings` → **clean** (Finished, no warnings). Matches the
  koto CI gate, which lints lib+bins only.
- `cargo clippy --all-targets` → 43 warnings counted (become errors under
  `-D warnings`; the "48" figure is in the same ballpark — the count shifts with
  which compile unit short-circuits first). Flagged files: `tests/*.rs`
  (terminal_index_compaction, batch_rewind_test, cli_session_start,
  integration_test) and pre-existing `src/` sites (next.rs, session.rs,
  advance.rs, gate.rs, batch.rs:4449+).
- The ONLY diff-touched file with all-targets hits is `src/cli/batch.rs`
  (lines 4449, 4464, 4478, 4495, 4512, 4534, 4652, 4672 — all `useless use of
  vec!`). `git blame` on each of those lines attributes them to commits
  `f02477c` and `b95f523`, **not** `8f1aa9a`. So they are genuinely pre-existing
  and untouched by this diff.
- The diff's own additions (the `gate_inlines_child_result...` test and the
  types.rs round-trip tests) introduce **zero** new clippy hits.

Conclusion: the baseline claim is accurate and falsifiable-checked. The CI gate
(lib+bins, the parity command) is clean. Choosing not to fix 43 unrelated
pre-existing test-code lints in a walking-skeleton issue is correct scoping, not
hidden debt. ADVISORY only in the sense that a future issue (Issue 5 consolidates
tests) might clean these up, but they are out of scope here.

## (b) Short-name result-map fallback — SOUND, not a band-aid

The keying logic (`build_children_complete_output`, diff lines 46-88):
- `result_by_child: HashMap<String, WorkflowResult>` is keyed by the event's
  `child_name`, which is ALWAYS the composed `<parent>.<task>` identity (set at
  `append_child_completed_to_parent`, mod.rs; and in the test at
  `child_name: format!("parent.{}", task_name)`). Canonical key.
- The inline loop tries `result_by_child.get(&entry.name)` then
  `.get(&composed)` where `composed = format!("{parent_name}.{entry.name}")`.

Entry naming has two sources:
- Hook path (`build_entries_from_tasks`, batch.rs:2557): `name: composed`
  (`parent.task`). Direct `.get(&entry.name)` hits the composed key. The
  `composed` fallback would build `parent.parent.task` — never matches, harmless.
- No-hook fallback path (`build_entries_from_disk`, batch.rs:2738-2739):
  `name: session_id`, where session_id is the on-disk id (composed when the
  child still exists) or falls back to the short `task_name` when the child has
  been cleaned up / is absent from the session-id map. In the cleaned-child case
  `entry.name` is short, `.get(&entry.name)` misses, and `.get(&composed)`
  reconstructs `parent.task` and hits.

This is precisely the case the fallback exists for, and the comment documents it
accurately. The key is always canonical; the two-step lookup only widens to catch
the one path where the entry name is short. No collision is introduced: composed
names are constructed identically on producer and consumer sides. This is a
correct keying reconciliation across two naming conventions, not a band-aid
masking a keying bug.

ADVISORY: AC5's functional test (`gate_inlines_child_result_without_replaying_
child_log`) drives the **hook path** (it declares a `materialize_children` spec),
where the direct `.get(&entry.name)` hit fires — so the `.get(&composed)`
fallback branch is not the load-bearing path under test. The test's premise
(child absent from disk, result read only from the parent's own ChildCompleted)
is correctly demonstrated, but the specific short-name → composed fallback line
is not the branch exercised. Issue 3 (cleaned-child dereference) and Issue 5
(consolidated functional coverage) own the no-hook cleaned-child path. Noting the
coverage seam, not a defect.

## (c) Skipping TaskView/BatchFinalView — LEGITIMATELY OUT OF ISSUE-1 SCOPE

Reported: did NOT extend TaskView/BatchFinalView (the BatchFinalized frozen-view
path), claimed outside Issue 1 AC.

Findings:
- Issue 1 AC5 requires only that `build_children_complete_output` inlines each
  child's `result`, demonstrated by a test showing a parent reading a completed
  child's result without replaying the child log. The diff does exactly that and
  the test passes. The converge READ surface Issue 1 targets is the gate output's
  `children` array — correctly enriched.
- `TaskView` (batch_view.rs) is a SEPARATE rendering surface that powers
  `koto batch view`; `derive_batch_view`/`build_final_view` project gate-output
  children into `TaskView`. `TaskView` was NOT given a `result` field. So
  `koto batch view` rendering currently drops the result. But no Issue-1 AC, and
  no Issue-4 AC, requires `result` on the `batch view` TaskView surface — Issue 4
  speaks to the cleared **directive** carrying results inline (the `koto next`
  GateBlocked/advance path), not the `batch view` projection.
- IMPORTANT nuance that strengthens the coder's position: the frozen
  `BatchFinalView` is NOT actually result-blind. `BatchFinalView.children` is a
  `Vec<ChildGateEntry>`, and `ChildGateEntry.result` (added by this diff with
  `#[serde(default, skip_serializing_if)]`) round-trips through
  `from_gate_output`'s `serde_json::from_value` automatically. So the result IS
  preserved in the frozen ledger; only the downstream `TaskView` projection drops
  it. The coder's phrasing ("did not extend BatchFinalView") slightly understates
  this — the BatchFinalView *struct* carries result for free via its
  ChildGateEntry vec; what's genuinely un-extended is the `TaskView` projection.
- The `koto next` GateBlocked path (mod.rs:3447) builds the response from
  `blocking_conditions_from_gates` + directive, not from TaskView, so it is
  unaffected by the TaskView gap. And tightening the gate predicate / making the
  cleared directive carry results inline is explicitly Issue 4's job, not
  Issue 1's.

Conclusion: skipping TaskView does NOT leave the Issue-1 converge read broken on
any real path. The converge read (gate `children` array) works; the frozen ledger
retains result. The only unsurfaced spot is the human-facing `koto batch view`
TaskView rendering, which no AC in this issue or Issue 4 covers.

ADVISORY: the TaskView projection should eventually surface `result` so
`koto batch view` shows it, and Issue 5 (docs + consolidated coverage) is the
natural home. Flagging so it is not silently lost between issues — it is a real
surface gap, just not an Issue-1-scope one.

## (d) summary synthesis — REASONABLE

`synthesize_workflow_result` (mod.rs) reads `summary` from the latest terminal
`EvidenceSubmitted.summary` field, else a final-state-derived default
(`completed/failed/skipped at <state>`); `payload` carries the terminal evidence
fields when non-empty; `status` reuses the projected `TerminalOutcome`. This
matches Decision 1 (auto-promote terminal evidence) and Decision 2 (typed
envelope, status reuses TerminalOutcome) verbatim. The design explicitly defers
the richer `accepts`-block summary-field convention to later issues, and the
doc-comment says exactly that ("later issues thicken the convention and the
`accepts`-block lookup"). Honest about being a skeleton; no hidden gap.

## Test verification

`cargo test --lib`: 1191 passed, 0 failed. All five new tests
(`workflow_result_round_trips_with_snake_case_status`,
`workflow_result_omits_payload_when_none`, `request_store_result_event_round_trips`,
`child_completed_with_result_round_trips`,
`pre_feature_child_completed_without_result_deserializes_to_none`,
`gate_inlines_child_result_without_replaying_child_log`) pass. The pre-feature
round-trip test directly proves the additive-field forward-compat claim.

## Summary of dispositions

| Deviation | Disposition | Why |
|-----------|-------------|-----|
| clippy --all-targets pre-existing baseline | Advisory | Verified true: all flagged batch.rs lines blame to f02477c/b95f523, not this commit; CI-parity lint clean |
| short-name fallback keying | Advisory | Sound canonical-key + two-step lookup; only the hook branch is under test, fallback branch deferred to Issue 3/5 |
| skipped TaskView/BatchFinalView | Advisory | Outside Issue-1 ACs; converge read (gate children array) and frozen ledger both retain result; only koto batch view TaskView rendering unsurfaced |
| summary synthesis | (clean) | Matches Decision 1/2; deferral honestly documented |

No deviation hides incomplete or incorrect work. All three advisories are
coverage/surface seams that the plan explicitly routes to later issues.
