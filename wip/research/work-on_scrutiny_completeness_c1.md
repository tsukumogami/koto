# Completeness scrutiny — request-store-converge Issue 1 (commit 8f1aa9a)

Reviewer focus: COMPLETENESS. Does every acceptance criterion have a real, verifiable
implementation in the diff, and do the coder's evidence claims hold up against the
actual test bodies?

Build/test status: `cargo test --lib` → 1191 passed, 0 failed. All 6 claimed new tests
exist and pass.

## AC-by-AC verdict

### AC1 — WorkflowResult struct — SATISFIED
`src/engine/types.rs` adds `pub struct WorkflowResult { status: TerminalOutcome,
summary: String, payload: Option<serde_json::Value> }` with
`#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`. The `payload` field
carries exactly `#[serde(default, skip_serializing_if = "Option::is_none")]`.
Field types match the AC verbatim. Tests `workflow_result_round_trips_with_snake_case_status`
(asserts `status == "success"` snake_case, payload present round-trips) and
`workflow_result_omits_payload_when_none` (asserts `json.get("payload").is_none()`)
verify both serialization behaviors.

### AC2 — EventPayload::RequestStoreResult, wire `request_store.result`, both directions — SATISFIED
- Variant `RequestStoreResult { result: WorkflowResult }` added to the enum.
- `type_name()` match arm returns `"request_store.result"` (types.rs:791) — this is the
  "payload_type match" the AC names; it is the function that maps payload variant → wire
  string. Serialize uses the stored `event_type` string, and `EventPayload` is
  `#[serde(untagged)]`, so the variant serializes its inner `{ "result": {...} }` body
  correctly.
- `Event::deserialize` dispatch adds the `"request_store.result" =>` arm (types.rs:1043-1046)
  routing through a new `RequestStoreResultPayload { result }` helper struct.
- Test `request_store_result_event_round_trips` asserts `val["type"] ==
  "request_store.result"`, `val["payload"]["result"]["status"] == "skipped"`, and a full
  deserialize back into the variant. Both directions exercised.

### AC3 — ChildCompleted gains additive serde-optional result; pre-feature logs round-trip — SATISFIED
- `EventPayload::ChildCompleted` gains `result: Option<WorkflowResult>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- The private `ChildCompletedPayload` deser struct gains `#[serde(default) ] result:
  Option<WorkflowResult>`, and the dispatch arm wires `result: p.result`.
- Test `pre_feature_child_completed_without_result_deserializes_to_none` feeds a hand-written
  JSON log with NO `result` key and asserts `result.is_none()` — genuinely proves the
  additive-field round-trip for old logs. Test `child_completed_with_result_round_trips`
  proves the populated case round-trips.

### AC4 — Terminal tick synthesizes WorkflowResult from TerminalOutcome projection and attaches it — SATISFIED
- New `synthesize_workflow_result(outcome, final_state, child_events)` in src/cli/mod.rs:
  `status` = the same `TerminalOutcome` the completion path already projects; `summary`
  read from the latest terminal `EvidenceSubmitted.summary` field with a final-state-derived
  fallback per outcome; `payload` = terminal evidence fields as an opaque JSON object when
  non-empty.
- `append_child_completed_to_parent` calls `synthesize_workflow_result(...)` and sets
  `result: Some(result)` on the emitted `ChildCompleted` payload.
- Wired at BOTH genuine terminal-tick call sites in `handle_next`: the
  `NextResponse::Terminal` branch (mod.rs:2790-2797, passes the child's `&events`) and the
  WorkflowCancelled terminal branch (mod.rs:~3878, passes freshly re-read `&post_events`).
  The cancelled branch was also refactored to re-read events once and reuse them.
- Note (advisory, not blocking): there is no dedicated unit test that calls
  `synthesize_workflow_result` directly or drives a full terminal tick to assert the
  synthesized fields (summary fallback vs evidence-derived summary, payload assembly).
  AC4 is covered indirectly: the AC5 test constructs a `ChildCompleted` carrying a
  synthesized-shape result and the round-trip tests cover the envelope. The synthesis
  function's own branches (evidence summary, fallback summary per outcome, payload
  emptiness filter) are not directly asserted. The function is `#[cfg(unix)]`, consistent
  with the rest of the completion path. This is a test-thoroughness gap, not a missing
  implementation.

### AC5 — Gate inlines each child's result from parent's ChildCompleted.result; test reads a completed child WITHOUT replaying the child log — SATISFIED
- `ChildGateEntry` gains `result: Option<WorkflowResult>` (serde-optional). All struct
  literals updated (`build_entries_from_tasks`, `build_entries_from_disk`, and every test
  fixture).
- `build_children_complete_output` builds `result_by_child: HashMap<String, WorkflowResult>`
  populated ONLY from the parent's own `events` iteration (batch.rs:2316-2337), then inlines
  into each entry by composed `<parent>.<task>` name (with short-name fallback)
  (batch.rs:2428+). No child-log open/replay anywhere in this path — the result derives
  exclusively from the parent log.
- `child_entry_to_json` emits the `result` field when present.
- Test `gate_inlines_child_result_without_replaying_child_log` is functionally real and
  asserts what's claimed: it inits only the PARENT session, appends `EvidenceSubmitted`
  (tasks list) + a `ChildCompleted` carrying a populated `result`, and the child
  `parent.alpha` is DELIBERATELY never created. It asserts
  `!backend.exists("parent.alpha")` with a message tying absence to "result can only come
  from the parent's own ChildCompleted.result". It then calls
  `build_children_complete_output`, finds the `parent.alpha` entry, and asserts
  `result.summary == "alpha evaluated 42"`, `result.status == "success"`,
  `result.payload.score == 42`. Because the child is absent from disk, any replay would
  fail to produce the result — so the inlining provably comes from the parent log. The
  "child session absent from disk" evidence claim is verified by an explicit assertion, not
  just a comment.

## Evidence-claim audit
- "6 new tests": confirmed — `workflow_result_round_trips_with_snake_case_status`,
  `workflow_result_omits_payload_when_none`, `request_store_result_event_round_trips`,
  `child_completed_with_result_round_trips`, `pre_feature_child_completed_without_result_deserializes_to_none`,
  `gate_inlines_child_result_without_replaying_child_log`. All 6 ran and passed.
- "AC5 functional test asserting the child session is absent from disk": confirmed via the
  explicit `assert!(!backend.exists("parent.alpha"), ...)`.

## Blocking findings
None. Every AC has a real implementation matching the spec, and the named tests assert what
the coder claims.

## Advisory findings
1. AC4: `synthesize_workflow_result` has no direct unit test for its own branch logic
   (evidence-derived summary vs the three fallback summaries, and the payload
   empty-filter). Coverage is currently indirect. A small unit test over the function would
   harden the contract that later issues build on. Non-blocking for a walking-skeleton issue.
