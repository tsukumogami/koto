# Phase 2 Research: Architecture Perspective

## Lead 1: Visualization Consumer Needs

### Findings

The visualization consumer needs to reconstruct a timeline of all gate overrides within a session. The architecture provides two key patterns for deriving data from the event log:

**Derive Pattern**: The persistence layer uses derived functions to reconstruct state from the event log:
- `derive_state_from_log()`: Finds the current state by replaying state-changing events (Transitioned, DirectedTransition, Rewound)
- `derive_evidence()`: Collects EvidenceSubmitted events for the current epoch (all events after the most recent state-changing event matching current state)
- `derive_decisions()`: Collects DecisionRecorded events for the current epoch using identical logic to evidence
- `derive_visit_counts()`: Counts state visits across epochs

This pattern is epoch-bounded: evidence and decisions are scoped to "events occurring after the most recent state-changing event whose `to` field matches the current state." Cross-epoch queries like `derive_decisions()` use "most recent state-changing event" to establish epoch boundaries.

**Event Replay**: The JSONL event log is immutable and designed for replay. The dispatch logic in `koto next` reads all events, replays to derive current state, evaluates gates, and decides response. Gates are evaluated fresh on each invocation, not cached.

**Query Surface**: Currently exists via CLI commands:
- `koto next`: Single-state diagnostics via NextResponse variants (GateBlocked returns blocking_conditions, EvidenceRequired returns expects)
- `koto decisions list`: Epoch-scoped, returns current state decisions as standalone JSON
- No cross-epoch query command exists; visualization would need to parse raw JSONL or build a new query surface

**Data Completeness Requirement**: For session visualization showing "all overrides with rationale," the override event must contain:
- Which state the override occurred in
- Which gate(s) failed (gate name and failure reason: exit_code, timeout, error message)
- Timestamp and sequence number (automatic via Event struct)
- The rationale provided by the agent
- The evidence submitted that bypassed the gate
- Whether the override succeeded (led to transition) or failed

The gate failure context (which gate, why it failed) is already captured in `StopReason::GateBlocked(BTreeMap<String, GateResult>)` at the point the override detection occurs in `advance_until_stop`. GateResult enum provides: Passed, Failed { exit_code }, TimedOut, Error { message }.

**Relationship to Other Events**: Override events would appear in this sequence:
1. Transitioned { to: gate-guarded-state } — engine transitions to a state with gates
2. DefaultActionExecuted (if state has default_action) — action runs
3. EvidenceSubmitted { state, fields } — agent provides evidence
4. GateOverrideRecorded { state, gates_failed, rationale, evidence_submitted } — NEW: override event recording the bypass
5. Transitioned { to: next-state } — agent's evidence resolved conditional transition, OR loop stops with GateBlocked if override event is only emitted for gate-blocking cases

Alternative interpretation: the override event may be emitted only when evidence successfully bypasses gates (gate-failed=true AND transition resolves to non-NeedsEvidence). In failure cases (evidence submitted but doesn't match any transition), gates remain blocking and no override event emitted — the state stays stuck. This matches "override = bypass," distinct from "submission failure."

**Query Shapes for Visualization**:
1. "All overrides in session": Filter all events by type=GateOverrideRecorded across all epochs
2. "Overrides at state X": Filter by type=GateOverrideRecorded AND state=X
3. "Overrides for gate Y": Filter by type=GateOverrideRecorded AND gates_failed[Y exists]
4. "Override timeline": Sort GateOverrideRecorded events by sequence number

These queries require a new `derive_overrides()` function in persistence.rs following the pattern of `derive_decisions()`, OR a general cross-epoch filter function that can retrieve any event type across all epochs (not epoch-scoped).

### Implications for Requirements

1. **Event Schema**: The GateOverrideRecorded event must include state name, gate failure details (names and GateResult values), rationale string, and optionally the evidence fields that were submitted. This avoids needing to correlate EvidenceSubmitted with GateOverrideRecorded.

2. **Query API**: A cross-epoch derive function is needed. Two options:
   - `derive_overrides(events: &[Event]) -> Vec<&Event>` — returns all GateOverrideRecorded events across all epochs
   - `derive_all_of_type(events: &[Event], event_type: &str) -> Vec<&Event>` — generic cross-epoch filter

3. **Backward Compatibility**: GateOverrideRecorded is a new event type. Existing workflows have no GateOverrideRecorded events. Visualization must handle "no overrides found" gracefully.

4. **Timeline Consistency**: Override events must be emitted at the exact point gates fail and evidence resolves. If emitted lazily during visualization, race conditions could occur (concurrent koto next calls interleaving writes). Better to emit at persistence time.

### Open Questions

1. Should GateOverrideRecorded include the full evidence fields that were submitted, or just the rationale? Rationale alone requires visualization to correlate with EvidenceSubmitted; full fields make the override event self-contained but larger.

2. If partial gates fail (state has multiple gates, one fails, one passes), and evidence is submitted, is an override event emitted? Or only when all gates fail? Or when ANY gate fails? Current design suggests "any gate failed = gates_failed in override event."

3. Should `--to` directed transitions (which bypass gates entirely without evidence) also emit an override-like event? These are a different mechanism (manual state jump) but similar outcome (gate bypass). Scope document says no, but architecture should note the gap.

4. Will visualization need to filter on gate name, or just "any override"? This affects whether to store gate names in a simple list vs. a map of name -> GateResult.

---

## Lead 2: CLI Rationale Acceptance

### Findings

The current CLI evidence submission path is `koto next --with-data <JSON>`. Evidence flows through this architecture:

**Current Evidence Flow**:
1. CLI parses `--with-data "<JSON string>"` (flag validation in next.rs, size limit check at 1MB)
2. Validates: JSON structure, field presence, type matching against template's accepts schema
3. Appends EvidenceSubmitted event with state and fields
4. Re-reads event log to include new event
5. Calls `derive_evidence()` to collect all EvidenceSubmitted for current epoch
6. Merges epochs evidence via `merge_epoch_evidence()` — last-write-wins per field
7. Passes merged BTreeMap to `advance_until_stop()` for transition resolution
8. Advancement loop resolves conditional transitions using merged evidence

Evidence validation is schema-based: the template's `accepts` block defines field names, types (string, number, boolean, enum), and required vs. optional. The validate_evidence() function in evidence.rs enforces this schema and collects all errors without short-circuiting for better UX.

**Options for Rationale Acceptance**:

*Option A: Separate `--rationale` flag*
```bash
koto next <name> --with-data '{"field":"value"}' --rationale "gate timeout, retry after deploy"
```
Pros: Separate concern (evidence vs. justification), rationale is required-by-default, non-JSON user-friendly.
Cons: Two arguments to parse, validation ordering, inconsistent with single `--with-data` pattern.

*Option B: Embedded in evidence JSON*
```bash
koto next <name> --with-data '{"field":"value","_rationale":"gate timeout, retry after deploy"}'
```
Pros: Single argument, evidence and rationale always together, existing validation pipeline.
Cons: Hijacks schema (reserved field name), mixes data types (user fields vs. system field), rationale must be JSON string.

*Option C: Template schema accepts rationale field*
```json
{
  "accepts": {
    "field": { "type": "string", ... },
    "rationale": { "type": "string", "required": true, ... }
  }
}
```
Pros: Rationale is a normal field, validated like any other, template designer controls requirement.
Cons: Shifts responsibility to template design, requires conditional validation (required_when gate_failed), out of scope per exploration.

*Option D: Separate subcommand or new command*
```bash
koto override <name> --gate <gate-name> --rationale "..." --with-data '...'
```
Pros: Explicit override semantics in command name, clear intent.
Cons: New command to implement, different from existing evidence submission path, duplicates advance logic.

**Evidence Schema Pattern**: The template compilation produces a FieldSchema for each accepts field. FieldSchema includes field_type, required, values (for enum), and description. The validate_evidence() function strictly checks against this schema. No override-specific validation exists today.

**Rationale Storage Path**: Once rationale is accepted (however), it must:
1. Be passed through the event persistence layer (append_event API accepts EventPayload only)
2. Be stored in the GateOverrideRecorded event alongside state and gate failure details
3. NOT be stored in EvidenceSubmitted (these are separate concepts per exploration)

Currently, EvidenceSubmitted takes a HashMap<String, serde_json::Value> for fields. The payload is event-type-specific; adding a rationale field to EvidenceSubmitted would require schema migration or "soft" reserved fields (fragile).

**Backward Compatibility**: Evidence format is frozen (already persisted in production workflows). Rationale addition cannot change how existing evidence is stored. Options A (separate flag) and D (new command) preserve evidence format. Option B (embedded field) requires template evolution. Option C delegates to templates.

**Advance Loop Integration**: The advance loop (advance_until_stop) doesn't currently have access to rationale. If rationale is provided at evidence submission time, it must be:
- Stored before advancement (in EvidenceSubmitted? no, wrong event type)
- Available when gates fail and override is detected (needs to be passed through or stored separately)
- Persisted as part of GateOverrideRecorded event

The cleanest integration: rationale is a separate concern. Evidence is submitted (EvidenceSubmitted event), advance loop runs, gates fail, override is detected in dispatch or early in advancement, then GateOverrideRecorded is emitted with rationale. Rationale must be captured at evidence submission time and threaded through to override event emission.

**Implementation Pattern from Decisions Subsystem**: The decisions subsystem (DecisionRecorded event) already requires rationale. `koto decisions record <name> --decision '{"field":"value"}' --rationale "..."` validates both decision and rationale as separate inputs. The CLI handler collects both, validates against schema, then appends a single DecisionRecorded event. A similar pattern could work for overrides: collect evidence and rationale separately at submission, validate both, then emit both EvidenceSubmitted and GateOverrideRecorded (or a single GateOverrideRecorded).

### Implications for Requirements

1. **Rationale as CLI Flag**: Most likely path. Add `--rationale <string>` flag to `koto next`, make it required when gates are currently failed. Implies detection of "are gates currently failed?" before evidence submission. Current next handler does this already (checks gate_results in dispatch logic).

2. **Conditional Requirement**: Rationale is mandatory only when overriding a gate (gates are failed AND evidence is submitted). If evidence is submitted on a non-gate-blocked state, rationale is not required. This requires CLI logic to detect: "are gates currently failed?" If yes, require --rationale.

3. **Event Emission Order**: Both EvidenceSubmitted and GateOverrideRecorded must be emitted atomically (same event batch) or in strict order (evidence first, override second). If they're separate calls to append_event, the log preserves order via sequence numbers. Visualization must handle both events present for an override.

4. **Validation Integration**: Rationale validation is minimal: non-empty string, length limits (e.g., <1KB to stay within payload size limits). No schema-based validation needed (unlike evidence fields). Can be done in CLI handler before calling append_event.

5. **Scope of --rationale**: Applies only to evidence submission on gate-failed states. Does NOT apply to:
   - Evidence submission on normal (non-blocked) states
   - `--to` directed transitions (separate mechanism, out of scope per exploration)
   - Default action executions

### Open Questions

1. Should rationale be required if evidence is submitted on a gate-blocked state, or only if the evidence succeeds in resolving the transition? Current proposal is: required if gates are blocked, regardless of whether evidence matches a transition. Rationale justifies "I'm submitting evidence despite gates failing," not "my evidence will fix the problem."

2. If an agent submits evidence (with rationale) on a gate-blocked state but the evidence doesn't match any transition, gates remain blocking. Is the rationale still stored in GateOverrideRecorded? Or only emitted if override succeeds (evidence matches conditional transition)? Semantically: "override attempted" vs. "override succeeded."

3. Should rationale length be limited? (e.g., <1KB max). The --with-data flag has a 1MB limit. Rationale could be large, but visualization UX may suffer with very long strings.

4. Should the flag be `--rationale`, `--override-reason`, `--reason`, or something else? Consistency with `--with-data` (data=evidence), so `--rationale` (rationale=justification) seems natural.

5. CLI preview: should `koto next` in GateBlocked state tell the agent "rationale is required"? Currently, GateBlocked response includes blocking_conditions (gate names and results). Should it also include a prompt like "Submit evidence with --rationale to override"?

---

## Summary

The visualization consumer requires a cross-epoch queryable GateOverrideRecorded event capturing which gate failed, why (GateResult details), the agent's rationale, and optionally the evidence submitted. A new derive_overrides() function following existing patterns will enable queries like "all overrides in session" and "overrides at state X." Rationale should be accepted as a separate `--rationale` CLI flag on `koto next`, required when evidence is submitted on a gate-blocked state, and must be threaded through the advance loop to be stored in the override event. Both EvidenceSubmitted and GateOverrideRecorded events must be emitted in strict sequence to preserve auditability.
