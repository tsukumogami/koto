# Lead: What query patterns does the visualization use case need?

## Findings

### Current State of Override Handling

Overrides are today implicit (not first-class events) and handled in `src/cli/mod.rs` (line 1596-1599) and `src/engine/advance.rs` (line 264-272):
- When a default action executes, the engine checks `has_evidence: bool` (line 1597 in mod.rs)
- If evidence exists in the current epoch, the action is skipped (`ActionResult::Skipped`)
- Evidence comes from `EvidenceSubmitted` events filtered by `derive_evidence()` in `src/engine/persistence.rs` (line 236-266)
- No explicit `OverrideRecorded` event type exists; the override intent is lost unless separately recorded via `koto decisions record`

### Event Payload Structure

The event stream (`src/engine/types.rs` line 30-73) includes:
- `WorkflowInitialized`: template initialization
- `Transitioned`: state change via resolved condition
- `DirectedTransition`: explicit state jump via `--to` flag
- `EvidenceSubmitted`: agent provides data (payload: `state, fields: HashMap<String, serde_json::Value>`)
- `DefaultActionExecuted`: action ran (or was skipped)
- `DecisionRecorded`: agent-recorded decision (payload: `state, decision: serde_json::Value`) — **the only mechanism to capture override rationale today**
- No event type for "EvidenceBypassedAction" or similar

Each event has: `seq` (monotonic), `timestamp` (RFC 3339), `type` (string), `payload` (variant-specific).

### Epoch-Scoped Filtering

`koto decisions list` and evidence aggregation use epoch-scoped filtering (src/engine/persistence.rs lines 273-303, 236-266):
1. Find the most recent state-changing event (`Transitioned`, `DirectedTransition`, or `Rewound`) whose `to` matches current state
2. Return only `DecisionRecorded` or `EvidenceSubmitted` events after that epoch boundary

This breaks cross-epoch visibility: to find all overrides in a session, you'd need to reconstruct the event log manually.

### How `koto decisions list` Works

`handle_decisions_list()` (src/cli/mod.rs line 2158-2205):
- Calls `derive_decisions(&events)` which filters to current-epoch `DecisionRecorded` events only
- Returns JSON: `{ state: current_state, decisions: { count: N, items: [...] } }`
- **No rationale field is structured** — rationale only exists if the user manually includes it in the `decision` JSON

### Available Commands and What They Surface

- `koto next`: advances workflow, emits events, returns next directive or stop reason
- `koto decisions record`: appends `DecisionRecorded` event with arbitrary JSON payload (rationale is user-supplied, not structured)
- `koto decisions list`: returns current-epoch decisions only
- **No `koto query` command exists** — the lead assumed one but it doesn't exist; the closest is reading raw state file or scripting `next`

### Query Surface Needs (Inferred for Visualization)

To visualize all overrides with rationale, at minimum:
1. **"Show all overrides in this session"** — requires scanning all epochs, not just current; needs to identify state-epochs and find `EvidenceSubmitted` + `DefaultActionExecuted` pairs where action was skipped
2. **"Show override at state X"** — requires querying by state name, not epoch boundary
3. **"Get override rationale"** — requires either a separate `override_rationale` field in events or a linking mechanism between `EvidenceSubmitted` and override intent

Current epoch-scoped `derive_decisions()` can't answer (1) or (2) without iteration outside its bounds.

## Implications

1. **Override events are invisible in the JSONL**: There is no dedicated event type that says "action was skipped because evidence existed." Reconstructing overrides requires matching `EvidenceSubmitted` (state=X, time=T) against `DefaultActionExecuted` (state=X, time=T+Δ) with exit code or skip indicator.

2. **Rationale capture is optional and unstructured**: `DecisionRecorded` can hold rationale, but it's voluntary and not semantically linked to the override. The agent must separately call `koto decisions record` to preserve intent.

3. **Epoch boundaries are artificial for cross-epoch queries**: Filtering to "current epoch" works for forward-looking decisions (what's blocking now?) but not for audit/visualization (what happened?). A visualization showing "all overrides this session" needs iteration outside the epoch boundary logic.

4. **Query patterns require scanning the full event log**: Minimum viable visualization needs:
   - Filter by event type (e.g., `EvidenceSubmitted`, `DecisionRecorded`)
   - Filter by state name (cross-epoch)
   - Filter by timestamp range
   - Join evidence to actions to infer skips
   - No SQL-like query interface exists; consumers must parse JSONL and filter in application code.

## Surprises

- **No `koto query` command**: The lead mentions "koto query returns full state as JSON" but no such command exists. This might be a future feature or a misunderstanding in the requirements.
- **Override intent is implicit, not explicit**: The system relies on pattern-matching (evidence presence → action skipped), not on a dedicated event. This makes visualization reconstruction fragile and rationale-capture a separate, optional step.
- **`DefaultActionExecuted` doesn't indicate whether it was skipped**: The event payload doesn't distinguish executed vs. skipped states; the `ActionResult::Skipped` enum variant is used in code but never persisted as an event field.

## Open Questions

1. Does `koto query` exist elsewhere (CLI plugin, integration) or is it planned?
2. Should overrides become a first-class event type with structured rationale field, or should the visualization layer infer them from `EvidenceSubmitted` + `DefaultActionExecuted` pairs?
3. For the "redo" capability: should the system capture which evidence submission caused the skip, so an agent can be asked to re-run the action without that evidence?
4. What fields should the override event carry beyond state and rationale? (e.g., evidence snapshot, action command, timestamp of submission)?

## Summary

Overrides are today implicit (inferred from evidence presence) with rationale captured optionally via `DecisionRecorded`. `koto decisions list` is epoch-scoped and can't answer cross-epoch queries like "show all overrides in this session." Visualization requires scanning the full JSONL event log and inferring skips from `EvidenceSubmitted` + `DefaultActionExecuted` pairs, or shifting to an explicit `OverrideRecorded` event type with structured rationale. The minimum query surface is: filter-by-type, filter-by-state, filter-by-time-range; no formal query API exists.
