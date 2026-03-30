# Lead: How should overrides be represented in the event log?

## Findings

### Current State: Implicit Overrides, Lost Rationale

**Gate override mechanism** (src/engine/advance.rs, lines 292-316, 394-447):
- When a state has gates AND an accepts block, gates can fail without blocking advancement
- The transition resolver uses a `gate_failed` boolean flag to prevent unconditional fallback transitions
- If gates fail but the state accepts evidence, the engine requires evidence before advancing
- Any evidence submitted on a gate-failed state _implicitly_ overrides the gate by enabling a conditional transition

**Evidence submission** (src/cli/mod.rs, line 1485-1498):
- `koto next --with-data <json>` appends a single `EvidenceSubmitted` event with fields and state
- Evidence is ephemeral to the "epoch" — it lives in the current state until a transition occurs
- No rationale or reasoning is captured; only the evidence payload

**Decision recording** (src/cli/mod.rs, lines 2114-2119, 1936-2152):
- Separate command: `koto decisions record --with-data <json>`
- Creates a `DecisionRecorded` event with state and a flexible decision object
- Decision schema is fixed: `choice` (string, required), `rationale` (string, required), `alternatives_considered` (array, optional)
- This command is entirely orthogonal from evidence submission — no connection in the code

**Evidence merge logic** (src/engine/advance.rs, lines 450-463):
- `merge_epoch_evidence()` collects all EvidenceSubmitted events in the current state
- No other event types participate in the merge
- Last-write-wins for field conflicts within an epoch

**Integration test insight** (tests/integration_test.rs):
- Test `default_action_skipped_when_override_evidence_exists()` confirms that submitting evidence skips a default_action
- The test shows override happening via evidence epoch, not via explicit override event type

### Key Architectural Constraints

**Event types in EventPayload enum** (src/engine/types.rs, lines 30-73):
- Current variants: WorkflowInitialized, Transitioned, EvidenceSubmitted, IntegrationInvoked, DirectedTransition, Rewound, WorkflowCancelled, DefaultActionExecuted, DecisionRecorded
- 9 total event types; evidence and decision are separate

**How events are deserialized** (src/engine/types.rs, lines 156-240):
- Event type is a string discriminant in `type` field, not serde enum tagging
- Payload is untagged — each variant's fields are serialized flat
- Type name comes from `EventPayload::type_name()` method, enabling custom naming

**Persistence model** (src/engine/persistence.rs):
- JSONL format: header line + one event per line
- Events are immutable once written; no updates or deletes
- `append_event()` auto-assigns seq and timestamp

### The Current Problem

**Overrides are invisible in the log:**
1. A gate fails on state S
2. Agent submits evidence E via `koto next --with-data`
3. This creates an EvidenceSubmitted event
4. The engine checks gate_failed=true and requires evidence
5. The evidence matches a conditional transition, so S → T
6. **But there's no event that says "gate X was overridden by evidence E with rationale R"**

**Decisions are decoupled from overrides:**
- `koto decisions record` captures human reasoning but happens in a separate command
- There's no link between a DecisionRecorded event and the EvidenceSubmitted that preceded it
- Rationale is lost unless the agent manually records it via decisions record

**Queryability is limited:**
- Cannot ask: "What gates were overridden in this workflow?"
- Cannot ask: "Why was gate X overridden?" (would need to correlate evidence + decisions manually)
- Cannot ask: "When was gate X overridden relative to other events?"

### Option A: New Event Type `GateOverride`

**Schema:**
```rust
GateOverride {
    state: String,
    gate_name: String,
    gate_result: GateResult,  // or serde_json::Value to capture failure reason
    override_evidence: serde_json::Value,  // the evidence fields that bypassed the gate
    rationale: Option<String>,  // optional reason why agent overrode
}
```

**Where it fits:**
- Emitted when gate_failed=true AND evidence matches a conditional transition
- In advance_until_stop, after transition resolution in the loop
- Would include which gate failed and what evidence bypassed it

**Pros:**
- First-class event for a first-class action; searchable by gate name and state
- Clear intent: "this gate was overridden" vs. "evidence was submitted and happened to match"
- Opens future for `koto redo` — can replay overrides with original or new rationale

**Cons:**
- Where does rationale come from? Would need a new CLI parameter, or link to DecisionRecorded
- Agent flow becomes: `koto next --with-data --rationale` (longer command)
- Adds schema surface; rationale is optional, making it incomplete by default
- Breaks the assumption that EvidenceSubmitted is the only gate-override mechanism

### Option B: Extend `EvidenceSubmitted` with metadata

**Schema change:**
```rust
EvidenceSubmitted {
    state: String,
    fields: HashMap<String, serde_json::Value>,
    gate_failed: Option<bool>,  // signals override was intentional
    rationale: Option<String>,  // why this evidence was submitted
    overrides: Option<Vec<String>>,  // which gate(s) were bypassed
}
```

**Where it fits:**
- Same emission point as today (src/cli/mod.rs, line 1485)
- Optional fields allow backward compatibility
- CLI flag: `koto next --with-data <json> --because "reason"`

**Pros:**
- Evidence and override context stay in one event
- No new event type; less schema churn
- Rationale travels with evidence chronologically
- Smaller learning curve: still evidence_submitted, just richer

**Cons:**
- Conceptually mixing two concerns: evidence data vs. gate-override intent
- If agent submits evidence that doesn't match a gate-failed state, rationale becomes noise
- Backward compatibility requires careful handling (optional fields must be truly optional)
- "Overrides" list is derived at emission time — engine would need to compute it before append

### Option C: Auto-generated `DecisionRecorded` on override

**Mechanism:**
- When gate_failed=true AND evidence matches conditional, engine auto-generates a DecisionRecorded event
- Auto-populated: `choice` = evidence fields, `rationale` = "Overrode gate <name> with submitted evidence"
- Agent can later use `koto decisions record` to add explicit rationale if desired

**Where it fits:**
- In advance_until_stop, after transition resolution
- Only when override actually happens (gate failed + evidence matched)
- DecisionRecorded would be appended alongside Transitioned event

**Pros:**
- Reuses existing DecisionRecorded structure — no schema extension needed
- Auto-capture means override is always recorded, no agent action required
- Forward-compatible with redo: DecisionRecorded already has the shape redo needs

**Cons:**
- Mixes auto-generated and manual decisions in same event stream
- Auto-rationale is generic ("Overrode gate X"), not specific to agent's reasoning
- Agent can't provide custom rationale at the time of evidence submission
- Blurs distinction between "decision" (explicit human choice) and "override" (reactive gate bypass)

### Implications for Queryability and Redo

**For visualization:**
- Option A (GateOverride event): direct query `SELECT * WHERE type='gate_override'`
- Option B (extended EvidenceSubmitted): query `WHERE type='evidence_submitted' AND gate_failed`
- Option C (auto DecisionRecorded): query `WHERE type='decision_recorded' AND rationale LIKE 'Overrode gate%'`

**For redo:**
- All options need to capture which gate failed and what evidence was used
- Option C (DecisionRecorded) is closest to a redo token — already has choice and rationale
- Options A/B would need additional transformation to become redo-actionable

**For audit trail:**
- Option A is most explicit: "gate was overridden"
- Options B/C require inference from correlated events

## Surprises

1. **Decisions are entirely optional.** The codebase has `koto decisions record/list` commands but gate overrides work _without_ them. This is by design, not a bug — overrides are implicit in evidence submission.

2. **Gate override is the fallback path in a three-path model.** The DESIGN-default-action-execution.md reveals that overrides are part of the "default/override/failure" paths: action runs (default), agent overrides with evidence (override), action fails and requires fallback directive (failure). This is intentional design, not accidental.

3. **Epoch-scoped evidence.** Evidence persists only for the current state, then is cleared on transition. This is elegant but means the override signal is implicit in the evidence-to-transition flow, not explicit in the log.

4. **No rationale field in EvidenceSubmitted.** Even though decisions record has a required `rationale` field, EvidenceSubmitted has no room for reasoning. The two are decoupled.

5. **Backward compatibility matters.** The codebase uses `#[serde(default)]` and `skip_serializing_if` extensively (e.g., in ActionDecl). Any schema extension must preserve existing event format.

## Open Questions

1. **Should override capture be automatic or agent-driven?** Option A/B require agent input (flag or extra command); Option C is automatic. Which matches the workflow intent?

2. **Should rationale be required or optional?** If required, every override needs explaining; if optional, audit gaps exist. What's the right bar?

3. **Do gate-failed states always accept evidence?** The code pattern is "accepts block exists AND gates fail → require evidence." Is this universal, or are there exceptions?

4. **How does this interact with directed transitions (--to)?** `koto next --to S` bypasses gates entirely. Should that also be logged as an override event?

5. **Is there a future where overrides are redoable?** If yes, the event structure needs to support redo semantics (which gate, original evidence, new rationale). If no, simpler structure suffices.

6. **What's the performance impact of adding new event types?** Current search/filter operations iterate all events. Adding GateOverride events (which fire frequently on override-heavy workflows) could impact scalability.

## Summary

Gate overrides are currently implicit in the event log — submitting evidence on a gate-failed state accomplishes the override, but no explicit event marks it. The rationale is lost unless agents separately call `koto decisions record`. Three representation choices exist: a dedicated `GateOverride` event type (explicit, searchable, but adds schema), extending `EvidenceSubmitted` with optional override metadata (keeps concerns together, fewer new types), or auto-generating `DecisionRecorded` events (reuses existing structure but auto-rationale is generic). Option C is closest to redo-readiness but least explicit; Option A is most explicit but highest schema cost. The choice depends on whether the priority is queryability (favors A), simplicity (favors C), or coherence (favors B).

