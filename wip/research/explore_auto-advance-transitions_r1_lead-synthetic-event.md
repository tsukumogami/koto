# Lead: Synthetic event design for skip_if

## Findings

### EvidenceSubmitted payload structure and consumption

**File**: `src/engine/types.rs` (lines 140-157)

```rust
EvidenceSubmitted {
    state: String,
    fields: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submitter_cwd: Option<PathBuf>,
}
```

The `EvidenceSubmitted` event carries:
- `state`: The state where evidence was submitted (required).
- `fields`: Arbitrary evidence data as JSON values (required). This is a flat map, no hierarchy.
- `submitter_cwd`: Optional working directory of submitter (additive field, omitted when None).

The struct uses `#[serde(untagged)]` on `EventPayload`, meaning no variant wrapper is serialized — only the fields appear in the JSON payload. This keeps wire format compact.

**Consumption in state reconstruction**: `src/engine/persistence.rs` (lines 231-266, `derive_evidence`)

```rust
pub fn derive_evidence(events: &[Event]) -> Vec<&Event> {
    let current_state = match derive_state_from_log(events) {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find the most recent state-changing event whose `to` matches current state.
    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    });

    let start = match epoch_start_idx {
        Some(idx) => idx + 1,
        None => return Vec::new(),
    };

    events[start..]
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::EvidenceSubmitted { state, .. } if state == &current_state))
        .collect()
}
```

Key behavior:
- **Epoch boundary**: Evidence is scoped to the current epoch — after the most recent state-changing event (Transitioned, DirectedTransition, or Rewound) whose `to` field matches the current state.
- **State match**: Only events with `state == current_state` are returned; evidence tagged for other states is filtered out.
- **No filtering by submitter**: All EvidenceSubmitted events matching the epoch and state are collected, regardless of origin (agent-submitted vs. synthetic).

### How resuming agents see evidence

**File**: `src/engine/advance.rs` (lines 667-677, `merge_epoch_evidence`)

```rust
pub fn merge_epoch_evidence(events: &[Event]) -> BTreeMap<String, serde_json::Value> {
    let mut merged = BTreeMap::new();
    for event in events {
        if let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload {
            for (key, value) in fields {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}
```

**Usage**: `src/cli/mod.rs` (line 2324-2325)

```rust
let epoch_events = derive_evidence(&current_events);
let evidence = merge_epoch_evidence(&epoch_events.into_iter().cloned().collect::<Vec<_>>());
```

This merged evidence is passed to state evaluators via the `evidence` parameter in the advancement loop. Agents see it through:
1. **koto next / dispatch_next output**: The `expects` schema from the template specifies which fields are accepted.
2. **Evidence validation loop**: The merged map is checked against the state's `accepts` schema.
3. **Gate evaluation**: Gates can access `context.{fieldname}` to read merged evidence values.

**Critical insight**: The merge is **order-independent** (BTreeMap insertion with later values overwriting earlier ones by key). There is no timestamp or source attribution in the merged map — agents see only the final value for each key, not its provenance.

### Event type enum and state-changing event variants

**File**: `src/engine/types.rs` (lines 113-282, `EventPayload` enum)

Variants that change state (recognized by `derive_state_from_log`):
- `Transitioned { from: Option<String>, to: String, condition_type: String }` — Normal guard transition.
- `DirectedTransition { from: String, to: String }` — Forced manual transition.
- `Rewound { from: String, to: String }` — Rewind transition.

Other evidence-like events (do NOT change state):
- `EvidenceSubmitted { state, fields, submitter_cwd }` — Agent or system evidence.
- `DecisionRecorded { state, decision }` — Agent decisions.
- `GateEvaluated { state, gate, output, outcome, timestamp }` — Gate evaluation audit.
- `GateOverrideRecorded { state, gate, rationale, override_applied, actual_output, timestamp }` — Override record.
- `DefaultActionExecuted { state, command, exit_code, stdout, stderr }` — Action execution audit.

All non-state-changing events are **epoch-scoped**: they belong to the epoch initiated by the most recent state-changing event to their `state` field.

### CLI output and what agents see on resume

**File**: `src/cli/next_types.rs` (lines 26-118, `NextResponse` enum and serialization)

When an agent resumes at a state:
- If the state has an `accepts` block (via `ExpectsSchema.fields`), the agent sees `action: "evidence_required"`.
- The `expects` schema lists required and optional fields.
- The agent receives the merged epoch evidence map (keyed by field name).
- **No synthetic flag, no source attribution**: The agent has no way to distinguish whether "context_file: 'content'" came from agent submission, automatic injection, or a synthetic marker event.

Example `koto next` output for a state with evidence:

```json
{
  "action": "evidence_required",
  "state": "plan_context_injection",
  "directive": "Provide or confirm context",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "context": { "field_type": "string", "required": true, ... }
    }
  },
  "advanced": false
}
```

If `context` field is already present in merged evidence, it was merged into the `context` value the agent evaluates (not exposed separately).

---

## Implications

### Option A: Reuse `EvidenceSubmitted` with `synthetic: bool` marker

**Schema change** (minimal):

```rust
EvidenceSubmitted {
    state: String,
    fields: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submitter_cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    synthetic: Option<bool>,  // true if auto-generated by skip_if
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_if_reason: Option<String>,  // e.g., "context_file_exists" or "marker_condition_matched"
}
```

**Changes needed**:

1. **types.rs**: Add optional `synthetic` and `skip_if_reason` fields with serde defaults.
2. **persistence.rs**: No change needed (derive_evidence and merge_epoch_evidence already work with all EvidenceSubmitted events).
3. **CLI/query output**: Agents querying `koto query --events` see the full event including the new fields. Need to ensure serialization includes them when present.
4. **Merge behavior**: merge_epoch_evidence still merges fields identically; the `synthetic` flag and reason survive in the event log for audit but don't affect merge semantics.

**Pros**:
- Minimal schema churn; additive optional fields.
- Backward-compatible: old logs without `synthetic` round-trip cleanly (both fields default to None/absent).
- Single event type means existing code paths (derive_evidence, merge_epoch_evidence, epoch scoping) need zero changes.
- Agents inspecting the event log can see the provenance of each evidence value.

**Cons**:
- Semantically unclear: the event type is "evidence_submitted" but the `synthetic` flag contradicts that — it was not "submitted" by an agent; it was auto-generated.
- When merging fields, there's ambiguity: if two EvidenceSubmitted events (one synthetic, one agent) carry the same field, which reason wins? (The merge overwrites by key, so the last one wins; the reason is lost.)
- Agents inspecting `expects` see only the field schema, not whether the evidence is synthetic. Resume-time understanding requires reading the event log, not the live state.

### Option B: New event type `AutoAdvanced`

**Schema** (new variant):

```rust
AutoAdvanced {
    state: String,
    target: String,  // The state we're advancing to (like Transitioned.to)
    reason: String,   // e.g., "skip_if_condition_matched", "context_exists"
    evidence_fields: Option<HashMap<String, serde_json::Value>>,  // Optional evidence to merge
    condition_type: String,  // Mirrors Transitioned.condition_type for consistency
}
```

**Alternative naming**: Could be `SkipIfMatched`, `SyntheticTransition`, or `ConditionedAutoAdvance` to be more explicit.

**Changes needed**:

1. **types.rs**: Add new enum variant to EventPayload.
2. **persistence.rs**: Add `AutoAdvanced` case to `derive_state_from_log` (like Transitioned/DirectedTransition/Rewound). This makes AutoAdvanced a state-changing event.
3. **persistence.rs**: Conditionally handle AutoAdvanced in `derive_evidence` — does evidence from an AutoAdvanced event count as "submitted evidence for the old state" or does it auto-populate the new state's epoch? (Likely: AutoAdvanced events do NOT contribute to derive_evidence because they themselves transition state; any evidence they carry goes into the target state's epoch.)
4. **advance.rs**: merge_epoch_evidence unchanged (it looks only for EvidenceSubmitted; AutoAdvanced evidence is not merged by the existing logic).
5. **CLI/query output**: Agents see AutoAdvanced events in the log as distinct from EvidenceSubmitted, making provenance explicit.

**Pros**:
- Explicit semantics: the event says "this state auto-advanced" without ambiguity.
- Resuming agents query the log and see exactly when and why a state was skipped.
- Cleaner separation: EvidenceSubmitted = agent/external evidence; AutoAdvanced = system advancement.
- No merge ambiguity: evidence attached to AutoAdvanced doesn't overwrite agent evidence in the previous state.
- State derivation is accurate: `derive_state_from_log` correctly identifies AutoAdvanced as a state change.

**Cons**:
- Larger schema churn: new event variant, new type_name(), deserialization case.
- If AutoAdvanced carries evidence, how is that evidence consumed? merge_epoch_evidence doesn't know about it; new logic is needed.
- Backward compatibility is clean (new event type is unknown to old code), but forward compatibility must be managed if the feature is optional.

---

## Surprises

1. **No synthetic flag exists yet**: Koto already uses the word "synthetic" in batch scheduling contexts (for skip-marked children), but there is no `synthetic` field on event payloads. The term is used in design docs and status output to label skip-marked children, but not in the event log.

2. **Evidence merge is order-independent and overwrites**: Later EvidenceSubmitted events with the same field key overwrite earlier ones. There's no union or timestamp-based merge. This means if skip_if auto-advances and emits evidence while agent evidence is already present, the later one wins by default. This is actually suitable for deterministic behavior (last write wins), but resuming agents can't tell which one was applied without reading the log.

3. **No per-field source attribution**: The merged evidence map presented to gates and agents has no source metadata (agent vs. synthetic, timestamp, reason). The event log preserves this, but the live state does not. This is why the lead explicitly requests that "the fact should be in the log — otherwise a resuming agent at analysis has no way to know whether context came from a PLAN outline or a GitHub issue."

4. **Epoch scoping is state-specific**: Evidence is scoped both by epoch (after the most recent state-change to the current state) AND by state field matching. This means if a state was visited, rewound, and revisited, old evidence is cleared. This is correct for resume-awareness but means synthetic evidence must always carry the correct state field.

5. **No built-in gate for "evidence exists"**: The dispatch logic checks `template_state.accepts.is_some()` to decide whether to show EvidenceRequired, but there's no gate condition that automatically checks "did agent provide evidence for field X?" This means skip_if predicates must be evaluated at a higher layer (likely the advance loop, not via gates).

---

## Open Questions

1. **When skip_if fires mid-loop, should it trigger a new Transitioned event or an AutoAdvanced event?**
   - If Transitioned with `condition_type: "skip_if"`, it looks like a normal gate-driven transition. This works but loses the synthetic/auto distinction.
   - If AutoAdvanced, it's explicit but requires more schema changes.

2. **If skip_if carries evidence (e.g., injected context), how should it be surfaced to the next state?**
   - Option: AutoAdvanced stores evidence in its payload; advance.rs extracts and merges it into the next state's merged_evidence.
   - Alternative: skip_if injects as a separate EvidenceSubmitted event (synthetic=true) before advancing. Then normal derive_evidence picks it up.

3. **For resume-awareness: does "resuming agent at analysis" mean reading koto query --events, or does it mean the next time koto next is called after a pause?**
   - If the latter, the merged evidence is already populated; the agent sees the field but not its origin.
   - If the former, the agent must inspect the event log JSON. In that case, a source marker (synthetic flag or event type) is essential.

4. **Should skip_if predicates be evaluated only during `advance_until_stop` loop, or also during initial state entry?**
   - If at entry: a workflow that starts in a skip_if state would auto-advance on first `koto next`. This is clean but requires special handling.
   - If only in loop: skip_if is a within-loop optimization. First entry requires agent evidence (if accepts exists).

5. **How does the "chains consecutive auto-advancing states within a single advance loop turn" requirement map to events?**
   - Example: if state A skip_if→ B skip_if→ C, do we emit one event (jump to C) or three (A→B, then B→C)?
   - For resume-awareness, three events are clearer. But event log bloat is a concern.

---

## Summary

The `EvidenceSubmitted` event already auto-discovers as epoch evidence (no filtering by source) and merges transparently into the state's live view. Adding a `synthetic: bool` marker and `skip_if_reason` field keeps schema minimal and backward-compatible but conflates two concerns ("evidence submitted" vs. "system-generated condition") and provides no event-log distinction. A new `AutoAdvanced` event type is more explicit semantically and cleaner for resume-aware agents, but requires deeper integration changes (state-change registration, potential evidence inheritance rules). The key tension is whether to reuse the existing EvidenceSubmitted type for simplicity or create a distinct event type for clarity — the answer depends on how heavily agents rely on log structure vs. live merged state for understanding provenance.

