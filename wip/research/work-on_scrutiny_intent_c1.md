# INTENT scrutiny — Issue 1 (walking-skeleton converge path), commit 8f1aa9a

Repo: `/home/dgazineu/dev/niwaw/koto-wt-request-store-converge`
Design: `docs/designs/DESIGN-request-store-converge.md`
Plan: `docs/plans/PLAN-request-store-converge.md`

## Scope under review

Issue 1 is the walking skeleton: the `WorkflowResult` envelope, the
`RequestStoreResult` event variant, the additive `ChildCompleted.result` field,
minimal terminal-tick promotion synthesizing the result, and a gate read that
inlines each child's result off the parent's own `ChildCompleted.result`. It
deliberately defers the index `has_result` flag (Issue 2), the durable child-log
record + live/cleanup dereference (Issue 3), and the gate pass-predicate +
recursion/concurrency (Issue 4).

Verdict up front: the implementation matches design intent for Issue 1, and the
type/function factoring is a clean foundation for Issues 2-4 with no forced
rework. All touched unit tests pass (`engine::types` 77 passed;
`gate_inlines_child_result_without_replaying_child_log` passes). Findings below
are advisory only.

## What matches the design

### Envelope shape (Decision 2) — exact match
`WorkflowResult { status: TerminalOutcome, summary: String, payload:
Option<serde_json::Value> }` (src/engine/types.rs ~567) is byte-for-byte the
design's Key Interfaces block. `status` reuses `TerminalOutcome` verbatim (no new
enum), `payload` is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
Round-trip + snake_case + payload-omitted tests present and green. This is the
uniform typed read the design's R2/R6 require, with the wire form fixed for every
downstream consumer.

### Event variant (Decision 3) — correct and forward-compatible
`EventPayload::RequestStoreResult { result: WorkflowResult }` is added with wire
`type: "request_store.result"`, wired into both the `payload_type()` match arm
and the `Event::deserialize` dispatch, with a dedicated `RequestStoreResultPayload`
deserialize struct. Unknown-to-older-koto degrades through the existing `Unknown`
arm. A round-trip test exists. Critically for Issue 3: the variant already exists
and is fully serializable, so Issue 3 only has to *append* it to the child log —
no type work, no dispatch work. The factoring is correct.

### Additive ChildCompleted.result — correct
`ChildCompleted` gains `result: Option<WorkflowResult>` with
`#[serde(default, skip_serializing_if = "Option::is_none")]`, mirrored in
`ChildCompletedPayload` with `#[serde(default)]`, threaded through the deserialize
reconstruction (types.rs ~1031). The `pre_feature_child_completed_without_result_
deserializes_to_none` test proves pre-feature parent logs round-trip. This is the
cleanup-race carrier the design's Decision 3 hinges on, and it is in place.

### Synthesis function factoring — well-positioned for Issue 4
`synthesize_workflow_result(outcome, final_state, child_events)` is a standalone
function (src/cli/mod.rs ~406), not inlined into the append site. Status = the
`TerminalOutcome` already projected; summary falls back to a final-state-derived
default per outcome variant; payload = terminal evidence fields when non-empty.
This is exactly the auto-promotion of Decision 1 (no extra agent step). Because it
is a separate function taking events, Issue 4's "no summary field -> final-state
default" AC is already satisfied here and Issue 3 can reuse the same function to
build the `RequestStoreResult` event for the child log. Good separation.

### Gate read is purely additive — does NOT pre-empt Issue 4
The most important foundation check: Issue 1 does NOT couple the gate pass
predicate to result-presence. `build_children_complete_output` still computes
`all_complete` from terminal completion only (batch.rs ~2421), and the gate
evaluator (mod.rs ~4401) still passes on `all_complete`. The result is inlined as
a read-only `entry.result` field after the predicate computation. This leaves
Issue 4 free to tighten the predicate ("non-skipped + no result = outstanding")
without unwinding anything Issue 1 did. This is the correct walking-skeleton
boundary.

### Keying for live vs cleaned-up children — anticipates Issue 3
`result_by_child` is keyed by composed `child_name` (`<parent>.<task>`), and the
inline loop tries both `entry.name` and the composed `<parent>.<entry.name>`
form, covering both the hook path (composed names) and the no-hook fallback
(short task names). Issue 3 adds the *live-child* branch (read
`request_store.result` off the child log when it still exists); the current
parent-log read is exactly the cleanup-race fallback Issue 3 keeps. No rework:
Issue 3 adds a branch, it does not replace this one.

## Advisory findings (non-blocking)

### A1. Summary source deviates from the design's literal "accepts block" wording
The design (Decision 1, Decision Outcome Summary, Phase 3) repeatedly says the
summary comes from "a conventionally-named field on the terminal state's
`accepts` block." The implementation instead reads `summary` from the latest
terminal `EvidenceSubmitted.fields` (mod.rs ~412-417), with a final-state default
fallback. This is a defensible interpretation — the agent's submitted evidence is
where a `summary` field would actually arrive at runtime, and the `accepts` block
only *declares* the accepted field, it does not *hold* a value. The behavior
(read a conventionally-named `summary`, else default) matches design intent; only
the mechanism wording differs. Why advisory and not blocking: it does not force
rework — Issue 4's AC ("accepts block has no summary field -> final-state
default") is about the *fallback*, which is present and correct. But Issue 4/5
should consciously confirm the convention is "evidence field named `summary`"
and document it that way (Issue 5 docs AC), rather than "accepts block", so the
DESIGN and the shipped convention agree. Flagging so the convention is pinned
before Issue 5 writes it into the koto-user reference.

### A2. payload carries ALL terminal evidence fields, including `summary`
`payload` is built from the entire terminal `EvidenceSubmitted.fields` map when
non-empty (mod.rs ~430). When a `summary` field is present it is promoted to
`summary` AND duplicated inside `payload`. This matches the design ("payload
carries the terminal evidence fields") and is harmless, but it is a mild
redundancy a reviewer of Issue 4/5 may want to note. Not a foundation problem;
payload is opaque by contract. No action required for Issue 1.

### A3. `synthesize_workflow_result` and the append helper are `#[cfg(unix)]`-gated
The synthesis + promotion live under `#[cfg(unix)]` (consistent with the existing
`append_child_completed_to_parent` gating). The envelope/event types in
`engine/types.rs` are not gated, so the wire format is portable. This matches the
existing codebase posture (the completion path is already unix-only) and is not a
regression. Noted only so Issue 5's "cargo test passes" claim is understood to be
unix-scoped, as the rest of the completion path already is.

### A4. Two append sites both updated — no missed path
Both terminal-tick call sites (`handle_next` ~2793 normal path and ~3878
cancellation-aware path) now pass `&events` / `&post_events` into
`append_child_completed_to_parent`, and the cancellation path was refactored to
read events once and reuse them for both the terminal-index append and the result
synthesis. No completion path is left synthesizing a `None` result by accident.
This is the correct breadth for Issue 1.

## Foundation assessment for Issues 2-5

- Issue 2 (bounded `has_result` on `TerminalIndexEntry`): untouched by Issue 1,
  isolated to `terminal_index.rs`. The synthesis already knows whether a result
  exists (it always produces one), so Issue 2 can set `has_result: true`
  unconditionally on the promotion path. No conflict.
- Issue 3 (durable child-log record + live dereference): the `RequestStoreResult`
  variant and `synthesize_workflow_result` already exist; Issue 3 appends the
  event to the child log and adds the live-child read branch beside the existing
  parent-log read. Pure addition.
- Issue 4 (gate predicate + GateBlocked + recursion + concurrency): the predicate
  is still `all_complete`-only, so Issue 4 tightens it without unwinding Issue 1.
  The recursive auto-promote already works at every depth because the synthesis is
  depth-agnostic (operates on any child's events). No depth-specific path was
  introduced.
- Issue 5 (tests + docs): types and gate read are test-covered already; Issue 5
  consolidates. The one thing Issue 5 must reconcile is the A1 convention wording.

No architectural choice in Issue 1 forces a breaking change in Issues 2-5.

## Conclusion

Blocking: 0. Advisory: 4 (A1-A4). The walking skeleton matches design intent,
the envelope/event/field shapes are exactly as the design's Key Interfaces
specify, the synthesis is factored as a reusable standalone function, and the
gate read is additive and does not pre-empt the Issue 4 predicate. The only item
worth carrying forward is A1: the shipped summary convention reads an evidence
`summary` field rather than the design's "accepts block" phrasing — semantically
equivalent and non-breaking, but the DESIGN/docs wording should be reconciled in
Issue 5.
