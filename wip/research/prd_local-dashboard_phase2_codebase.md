# Phase 2 Research: Codebase Analyst

## Lead 3: Terminal State Detection

### Findings

**How koto status detects terminal states (lines 3383-3386 of src/cli/mod.rs):**
```rust
let is_terminal = compiled
    .states
    .get(&machine_state.current_state)
    .is_some_and(|s| s.terminal);
```

The `koto status` command:
1. Reads the state file via `backend.read_events(name)` → returns `(StateFileHeader, Vec<Event>)`
2. Derives the current state using `derive_machine_state(&header, &events)` (src/engine/persistence.rs, line 408)
3. Loads the compiled template from `machine_state.template_path`
4. Checks if the current state's `terminal` field is `true` in `CompiledTemplate::states` (src/template/types.rs, line 61)

**Deriving the current state (src/engine/persistence.rs, line 235):**
```rust
pub fn derive_state_from_log(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::Transitioned { to, .. } => Some(to.clone()),
        EventPayload::DirectedTransition { to, .. } => Some(to.clone()),
        EventPayload::Rewound { to, .. } => Some(to.clone()),
        _ => None,
    })
}
```

The engine replays events in reverse to find the most recent state-changing event (`transitioned`, `directed_transition`, or `rewound`). Note: **There is NO `workflow_completed` event** (documented in docs/reference/session-feed.md, line 361).

**MachineState structure (src/engine/types.rs, line 798):**
```rust
pub struct MachineState {
    pub current_state: String,
    pub template_path: String,
    pub template_hash: String,
}
```

No `is_terminal` field exists on `MachineState`—terminal detection always requires loading the compiled template.

**TemplateState.terminal field (src/template/types.rs, line 61):**
```rust
#[serde(default, skip_serializing_if = "is_false")]
pub terminal: bool,
```

The compiled template is a JSON file indexed by state name. States are marked `"terminal": true` in the source YAML, compiled into JSON format.

### Implications for Requirements

**Approach A is mandatory:**
- The dashboard must load the compiled template file at `~/.koto/cache/compiled/<hash>.json` (or the path stored in the session's state file).
- Terminal state detection = `current_state in template.states && template.states[current_state].terminal`.
- Cannot rely on a presence/absence heuristic; the template must be accessible at dashboard runtime.
- The template is already cached by koto's compiler; the dashboard should assume it is present (same assumption `koto status` makes).

**Gotchas and constraints:**
1. **Template availability**: The compiled template path is stored in `MachineState.template_path` (readable via `derive_machine_state`). If the template is missing or deleted after session creation, dashboard must handle gracefully with an error or "unknown" status.
2. **Session-file format**: Must call `persistence::read_events()` to properly parse the JSONL file (handles seq validation, truncated final line recovery, schema version checks).
3. **State derivation is idempotent**: Reading the same session multiple times always yields the same `current_state` if no new events are written.

### Open Questions

1. Should the dashboard cache the compiled template in memory for repeated status checks on the same session, or always reload?
2. If the compiled template is missing, should the dashboard fall back to a heuristic (e.g., "assume terminal if no pending gates"), or fail explicitly?
3. Are there batch-scheduling scenarios where a session's template might differ between parent and child workflows?

---

## Lead 4: Gate Display Design

### Findings

**GateEvaluated event schema (src/engine/types.rs, line 229):**
```rust
GateEvaluated {
    state: String,
    gate: String,
    output: serde_json::Value,
    outcome: String,  // "passed" or "failed"
    timestamp: String,
}
```

Declared in session-feed.md (lines 209–228):
```yaml
gate_evaluated:
  tier: 2
  fields:
    state: string (required)
    gate: string (required)
    output: object (required)
    outcome: string (required) enum: ["passed", "failed"]
    timestamp: string (required) format: rfc3339
```

**Gate types and output schemas (src/gate.rs):**

1. **Command gate** (`GATE_TYPE_COMMAND`):
   - Output: `{"exit_code": <int>, "error": <string>}`
   - Exit code 0 → passed; non-zero → failed; -1 + "timed_out" → timeout
   - (lines 206–233)

2. **Context-exists gate** (`GATE_TYPE_CONTEXT_EXISTS`):
   - Output: `{"exists": <bool>, "error": <string>}`
   - Returns Passed if key exists in context store; Failed otherwise
   - (lines 118–146)

3. **Context-matches gate** (`GATE_TYPE_CONTEXT_MATCHES`):
   - Output: `{"matches": <bool>, "error": <string>}`
   - Returns Passed if content at key matches the regex pattern
   - (lines 148–204)

4. **Children-complete gate** (`GATE_TYPE_CHILDREN_COMPLETE`):
   - Output: Complex structure with child counts and status aggregates
   - Schema (lines 77–99): `{ "total": <int>, "completed": <int>, "pending": <int>, "success": <int>, "failed": <int>, "skipped": <int>, "blocked": <int>, "spawn_failed": <int>, "all_complete": <bool>, "all_success": <bool>, "any_failed": <bool>, "any_skipped": <bool>, "any_spawn_failed": <bool>, "needs_attention": <bool>, "children": [...], "error": <string> }`
   - Returned by `evaluate_children_complete()` in src/cli/mod.rs (lines 3461+)

**Retrieving last gate evaluation (src/engine/persistence.rs, line 371):**
```rust
pub fn derive_last_gate_evaluated(events: &[Event], gate: &str) -> Option<serde_json::Value> {
    // ... epoch scoping ...
    events[start..].iter().rev().find_map(|e| {
        if let EventPayload::GateEvaluated {
            gate: g, output, ..
        } = &e.payload
        {
            if g == gate {
                return Some(output.clone());
            }
        }
        None
    })
}
```

Returns the `output` field (the JSON schema) of the most recent `GateEvaluated` event for a named gate, scoped to the current epoch (after the last state transition).

**Gate blocking category (src/gate.rs, line 278):**
```rust
pub fn gate_blocking_category(gate_type: &str) -> &'static str {
    match gate_type {
        GATE_TYPE_CHILDREN_COMPLETE => "temporal",
        _ => "corrective",
    }
}
```

`"temporal"` gates (children-complete) block due to time-dependent child progression; `"corrective"` gates (command, context) require agent intervention.

### Implications for Requirements

**Option: Universal display format**
The dashboard can render all gate types via:
1. **Gate name** (from template state's `gates` map key)
2. **Gate type** (from `Gate.gate_type`)
3. **Last outcome** (from most recent `GateEvaluated.outcome`: "passed" or "failed")
4. **Output snippet**:
   - Command gate: show `exit_code` and `error` message
   - Context gates: show `exists`/`matches` boolean
   - Children-complete: show `all_complete`, `any_failed`, child counts
5. **Blocking category** (call `gate_blocking_category(gate_type)` for visual classification)

**Data assembly for the UI:**
1. Load the compiled template to get the state's `gates` map
2. For each gate name, call `derive_last_gate_evaluated(&events, &gate_name)`
3. Pair the output with the gate's type declaration from the template
4. Render a table/list with name, type, outcome, and key fields from `output`

**"Last gate result per state" semantics:**
- "Last evaluation per gate per epoch" (where epoch = time since last state transition)
- The `derive_last_gate_evaluated()` function already implements this
- If a gate has never been evaluated in the current epoch, return `None` (no row to show)

**Gotchas:**
1. **Output schema varies by gate type**: The `output` field is a free-form JSON object. The dashboard must know the gate type to parse it correctly.
2. **Gate names are user-defined**: They come from the template's `gates` keys, not constants.
3. **Epoch boundaries reset on rewind**: When a session is rewound to an earlier state, all gate evaluations from the later epoch are out of scope. Use `derive_last_gate_evaluated()` to respect epoch boundaries.
4. **Children-complete gate output is large**: The `children` array can be verbose; consider truncation in the UI.

### Open Questions

1. Should the dashboard show **all gates** from the current state's template, or only those that have been evaluated (i.e., have a `GateEvaluated` event)?
2. For command gates, should error messages be shown in full, or truncated if very long?
3. Should the dashboard distinguish between "gate not yet evaluated" and "gate evaluated and failed"?

---

## Summary

**Terminal state detection (Lead 3):** The dashboard must load the compiled template file and check `template.states[current_state].terminal`. There is no `workflow_completed` event. Use `derive_machine_state()` to extract the template path from the session, then parse the JSON template file and look up the `terminal` field. This is the same approach `koto status` uses (lines 3357–3394 of src/cli/mod.rs).

**Gate display design (Lead 4):** Gates carry type-specific JSON output schemas (command: `exit_code`/`error`; context: boolean flags; children-complete: aggregates and child list). The dashboard can use `derive_last_gate_evaluated()` to fetch the last evaluation per gate per epoch, pair it with the gate's type from the template, and render a simple table showing name, type, outcome, and key fields from the output. All gate types can be displayed with a single flexible layout that adapts to the schema.

