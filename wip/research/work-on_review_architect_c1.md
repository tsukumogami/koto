# Architect review — request-store-converge Issue 1 (walking skeleton), commit 8f1aa9a

Scope reviewed: `WorkflowResult` envelope + `EventPayload::RequestStoreResult` (`src/engine/types.rs`),
additive `ChildCompleted.result`, terminal-tick synthesis (`src/cli/mod.rs`),
converge inline read (`src/cli/batch.rs`). Lens: structural fit, layering, interface contracts,
dependency direction, wire/forward-compat.

## Verdict

No blocking findings. The change rides koto's existing event-model, additive-field, and
forward-compat machinery as the design intends. Layering is correct and the interface shapes
set the right contracts for Issues 2-5. Three advisory notes below.

---

## (a) Closed-enum + `Unknown` forward-compat discipline — CORRECT

- `RequestStoreResult` is added to `EventPayload` (`src/engine/types.rs:644`), to the
  `type_name()` arm (`:791`, wire `"request_store.result"`), and to the `Event::deserialize`
  dispatch (`:1043`). Any `event_type` not matched falls through the existing `other => Unknown`
  arm (`:1048`). An older koto build that predates this variant routes the event to `Unknown`,
  preserving `type_name` + `raw_payload` verbatim.
- The custom `Serialize for Event` (`:846`) emits `self.event_type` (the stored string), not
  `payload.type_name()`. So an old build that read a `request_store.result` event into `Unknown`
  re-serializes it with its original type string intact — the event survives a round-trip through
  a downlevel build without corruption or relabeling. This is the same discipline the codebase
  documents at `:854`. Correct degrade behavior.
- `EventPayload` is `#[serde(untagged)]` (`:431`), but that attribute is inert on the read path
  because `Event::deserialize` is a hand-written dispatch keyed on the `type` string, not serde's
  untagged trial-deserialization. The new variant therefore introduces no untagged-ambiguity risk
  (untagged would otherwise be order-sensitive). On the write path, untagged serialize emits the
  inner fields flat (`{"result": {...}}`), matching every other variant. No issue.
- Tests confirm the contract: `request_store_result_event_round_trips` (`:2646`) asserts the wire
  `type` and snake_case status; `workflow_result_round_trips_with_snake_case_status` (`:2614`).

## (b) Additive fields / pre-feature round-trip — CORRECT

- `ChildCompleted.result` is `Option<WorkflowResult>` with `#[serde(default, skip_serializing_if =
  "Option::is_none")]` on both the enum variant (`:624`) and the deserialize helper
  `ChildCompletedPayload.result` (`:1191`, `#[serde(default)]`). A pre-feature parent log carries no
  `result` key and deserializes to `None`; re-serialization omits the key. Asserted directly by
  `pre_feature_child_completed_without_result_deserializes_to_none` (`:2705`).
- `WorkflowResult.payload` is `Option<Value>` with the same skip-when-`None` idiom (`:705`),
  asserted by `workflow_result_omits_payload_when_none` (`:2630`).
- `status` and `summary` are required (non-optional) on `WorkflowResult`. That is the right choice:
  `WorkflowResult` is a brand-new type with no pre-feature on-disk instances, so there is nothing to
  round-trip against — making them required keeps the typed-envelope contract tight rather than
  laundering the "every completion is resultful" guarantee through serde defaults.
- No `CURRENT_SCHEMA_VERSION` bump is needed and none was made (`:199` documents that additive
  optional fields don't require a bump). Consistent.

## (c) Layering — CORRECT

- `WorkflowResult` and `RequestStoreResult` are defined in `src/engine/types.rs` (engine layer).
  The synthesis function `synthesize_workflow_result` and the parent-`ChildCompleted` copy live in
  `src/cli/mod.rs` (CLI layer). The converge read `build_children_complete_output` lives in
  `src/cli/batch.rs` (CLI layer). Dependencies flow CLI -> engine only.
- `src/engine/types.rs` has no `use crate::cli` import; the single `crate::cli::batch` reference at
  `:579` is a pre-existing doc-comment link, not a code dependency. No dependency inversion.
- This matches the design's stated component placement (DESIGN "Components": envelope + event in
  `engine/types.rs`, promotion in `cli/mod.rs`, converge read in `cli/batch.rs`). The engine owns
  the data contract; the CLI owns the synthesis policy (what summary, what payload) and the gate
  read. That is the correct split: synthesis is a policy decision that should not leak into the
  type layer, and the design explicitly defers thickening the summary-field convention to later
  issues — keeping it in CLI lets that evolve without touching the wire type.

## (d) Interface shapes set correct contracts for Issues 2-5 — CORRECT

- `synthesize_workflow_result(outcome, final_state, child_events) -> WorkflowResult`
  (`src/cli/mod.rs:2139`) takes exactly the inputs the terminal tick already holds (the projected
  `TerminalOutcome`, the final state name, the child's own events). It reads the latest terminal
  `EvidenceSubmitted.fields`, pulls `summary`, and falls back to a final-state-derived default —
  so every completion is resultful (no "done but resultless" state), matching design Decision 1.
  The signature is stable for later issues: the child-log `RequestStoreResult` append (deferred)
  and the `accepts`-block summary lookup (deferred) can be added without changing callers.
- The parent copy is attached at the single existing notification site
  `append_child_completed_to_parent` (`:2222`-`:2229`), reusing the Issue #134 cleanup-safety
  surface rather than inventing a parallel one. This is the design's load-bearing move (Decision 3:
  the parent-log copy is the converge-read source precisely because the child session is
  auto-cleaned), and it is implemented exactly there.
- The converge read (`build_children_complete_output`, `:2316`-`:2443`) inlines results from the
  parent's own `ChildCompleted.result`, never opening a child log — asserted by
  `gate_inlines_child_result_without_replaying_child_log` (`:4320`, which deletes the child from
  disk first). It reuses the existing per-child `children` array and adds one optional `result`
  field (`ChildGateEntry.result`, `:2061`), so the gate output stays one shape. No new
  `NextResponse` variant, no new gate type — matches Decision 4.
- Correctly scoped to the walking skeleton: Issue 1 does NOT yet emit the child-log
  `RequestStoreResult` event, does NOT add the index `has_result` flag, and does NOT tighten the
  gate pass predicate on `has_result` (`all_complete` at `:2421` is unchanged). These are named as
  later-issue deliverables in the design and their absence here is intentional, not a gap. The
  variant exists and round-trips so Issue 2+ can start emitting it without a schema change.

---

## Advisory findings

### A1 (advisory) — `result_by_child` keying duplicates an ambiguity the codebase already resolves once

`build_children_complete_output` keys `result_by_child` by `child_name` (composed
`<parent>.<task>`) at `:2334`, then at lookup (`:2437`-`:2439`) tries `entry.name` first and falls
back to the composed `<parent>.<entry.name>`. The sibling `event_snapshots` map keys by
`task_name` (`:2332`). So the function now carries two maps off the same `ChildCompleted` event
keyed on two different identities (task name vs composed name), with the result lookup compensating
by trying both forms. This works and is tested, but it is a small parallel-keying pattern: a future
issue adding another per-child projection off `ChildCompleted` has two precedents to copy and no
single helper that says "given a ChildCompleted, this is its entry key." Not compounding yet (one
function, contained), but worth collapsing to a single keying convention before Issue 4 thickens
this path. Does not break anything.

### A2 (advisory) — summary-field convention is an untyped string lookup, by design but undocumented at the read site

`synthesize_workflow_result` reads `fields.get("summary")` (`src/cli/mod.rs:2154`) — a magic string
naming the convention. The design acknowledges this is a convention with a default fallback and
defers thickening it to later issues. The advisory: the convention key `"summary"` is not named as
a constant or referenced from a shared location, so the producer side (templates) and this consumer
side can drift independently. When Issue 3 formalizes the `accepts`-block lookup, lift `"summary"`
to a named constant so the two sides share one definition. Contained to one function today.

### A3 (advisory) — no explicit degrade-to-`Unknown` test for `request_store.result`

The forward-compat degrade path (an older build routing `request_store.result` to `Unknown` and
re-serializing it verbatim) is sound by construction (the `other => Unknown` arm plus
`event_type`-preserving serialize), but there is no test that pins it for this specific type the way
`workflow_initialized_without_spawn_entry_round_trip_omits_key` pins the additive-field path. The
design lists "older-log graceful degrade to Unknown" as a Phase 1 deliverable. Adding a test that
constructs a `request_store.result` event, deserializes it through a path that doesn't recognize the
type (or asserts the `Unknown` round-trip preserves the type string), would lock the contract before
later issues start depending on it. Test-only; no production-code concern.
