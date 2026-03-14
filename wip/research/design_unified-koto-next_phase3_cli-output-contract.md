# Phase 3 Research: CLI Output Contract and expects Field

## Questions Investigated

1. What does the current `koto next` output look like? What JSON fields exist today?
2. What does the current `Directive` struct contain? How is it serialized?
3. What does the PRD specify for the `expects` field? (R4: self-describing output, R6: transition-level conditions)
4. What should the complete `koto next` JSON output schema look like under the event-sourced model?
5. How does `expects` differ for: (a) a state with evidence requirements, (b) a state with only command gates, (c) a terminal state, (d) a state with a processing integration?
6. What error codes does the PRD specify? What should the structured `error` field look like?
7. What does the `advanced` field signal? How does it interact with auto-advancement chains?
8. How does integration output appear in the response? (PRD R8: integration output in response)
9. What exit codes does the PRD specify? How do they map to the structured output?
10. How do `--with-data` and `--to` flags interact with the output schema?

## Findings

### Finding 1: Current `koto next` Output (Pre-Unified Model)

**Current CLI output (cmd/koto/main.go, line 316):**
```go
return printJSON(d)
```

**Current Directive struct (pkg/controller/controller.go, lines 20-25):**
```go
type Directive struct {
    Action    string `json:"action"`              // "execute" or "done"
    State     string `json:"state"`               // current state name
    Directive string `json:"directive,omitempty"` // instruction text (execute only)
    Message   string `json:"message,omitempty"`   // completion message (done only)
}
```

**Current output format:**
- Execute directive: `{"action":"execute","state":"<state>","directive":"<text>"}`
- Terminal state: `{"action":"done","state":"<state>","message":"workflow complete"}`
- Minimal, does NOT include: `expects`, `advanced`, error codes, integration output, structured errors
- Output is printed directly via `json.Marshal(d)` with no additional wrapping

**Current flag handling (main.go, lines 286-317):**
- No `--with-data` flag implemented
- No `--to` flag implemented
- No integration support
- Only parses state/state-dir flags for file resolution

### Finding 2: Current TransitionError Type (Pre-Unified Model)

**TransitionError struct (pkg/engine/errors.go, lines 37-43):**
```go
type TransitionError struct {
    Code             string   `json:"code"`
    Message          string   `json:"message"`
    CurrentState     string   `json:"current_state,omitempty"`
    TargetState      string   `json:"target_state,omitempty"`
    ValidTransitions []string `json:"valid_transitions,omitempty"`
}
```

**Current error codes:**
- `terminal_state`: no outgoing transitions
- `invalid_transition`: target not in allowed list
- `unknown_state`: state not in machine definition
- `template_mismatch`: hash mismatch
- `version_conflict`: concurrent modification detected
- `rewind_failed`: rewind target not valid
- `gate_failed`: gates did not pass (currently unused in next)

**Current error output (main.go, lines 698-706, 708-713):**
- Wrapped in error object: `{"error": {...}}`
- Simple structure, no per-condition detail
- No information about which gates failed or why

### Finding 3: PRD Requirements for `expects` Field

**R4 (Self-describing output):**
> Every `koto next` response includes an `expects` field describing what the current state accepts. When the state accepts no submission (all conditions are koto-verified), `expects` is absent or null. An agent that has never seen the workflow template can determine its next action from the response alone.

**R6 (Transition-level conditions):**
> Workflow templates can declare conditions on individual outgoing transitions, not only on the state as a whole. For branching states, each transition has its own set of conditions. The agent satisfies one transition's conditions through evidence submission; koto advances to that transition's target automatically.

**R15 (Evidence field declaration):**
> Template authors can declare what evidence fields a state requires before it can advance. Each declared field has a name and a type or constraint. koto uses these declarations to generate the `expects` field in `koto next` output and to validate `--with-data` payloads.

**R14 (Per-transition condition declaration):**
> The template format allows conditions to be declared on individual outgoing transitions. A transition declaration includes a target state and an optional set of conditions. When all conditions on a transition are satisfied, that transition is eligible.

### Finding 4: Event-Sourced Model Impact on Output Contract

**From DESIGN-unified-koto-next.md (decision outcome):**
> `koto next` output gains `advanced: bool`, structured `error` (with code and message), and `expects` (with event type, field schema, and per-transition options). The `--with-data` flag submits an `evidence_submitted` event; `--to` submits a `directed_transition` event.

**Event types to be declared in templates:**
- `evidence_submitted`: payload schema (field declarations, types, constraints)
- `directed_transition`: no payload, just intent
- `transitioned`: internal, appended by engine, not submitted by agent
- `workflow_initialized`: initial event, created by `init`

**Template changes required:**
- States declare optional `event_schema` block with:
  - Field definitions (name, type, constraints)
  - Per-transition `when` conditions (what field values trigger which target)
- Example conceptual structure:
  ```
  event_schema:
    fields:
      decision: enum[refine, complete]
      reason: string
    when:
      - decision: refine -> target: refine_phase
      - decision: complete -> target: completion
  ```

### Finding 5: `expects` Field Variations by State Type

**Scenario A: State with evidence requirements (branching)**
```json
{
  "action": "execute",
  "state": "review_findings",
  "directive": "Review the findings and decide...",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {
        "type": "enum",
        "values": ["refine", "complete"],
        "description": "Whether to refine findings or complete"
      },
      "reason": {
        "type": "string",
        "description": "Justification for the decision"
      }
    },
    "options": [
      {
        "target": "refine_phase",
        "condition": "decision == 'refine'",
        "description": "Return to analysis"
      },
      {
        "target": "complete",
        "condition": "decision == 'complete'",
        "description": "Proceed to completion"
      }
    ]
  }
}
```

**Scenario B: State with only command gates (no agent submission)**
```json
{
  "action": "execute",
  "state": "wait_for_ci",
  "directive": "Waiting for CI checks to pass...",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "ci_pipeline",
      "type": "integration",
      "requires": "CI status check",
      "agent_actionable": false,
      "detail": "GitHub Actions workflow 'test.yml' must pass"
    }
  ]
}
```

Note: `blocking_conditions` is from R10 (advancement with gate failure detail), separate from `expects`. `expects` is null here because the gate is koto-verified (not agent-actionable).

**Scenario C: Terminal state**
```json
{
  "action": "done",
  "state": "complete",
  "message": "Workflow complete",
  "advanced": true,
  "expects": null
}
```

No submission expected; terminal state has no transitions.

**Scenario D: State with processing integration (delegate)**
```json
{
  "action": "execute",
  "state": "delegate_analysis",
  "directive": "Deep analysis required. Invoking delegate...",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "interpretation": {
        "type": "string",
        "description": "Agent's interpretation of delegate findings"
      },
      "approved": {
        "type": "boolean",
        "description": "Whether to proceed with findings"
      }
    }
  },
  "integration": {
    "type": "delegate",
    "available": true,
    "output": "{\"findings\": [...], \"confidence\": 0.95}"
  }
}
```

Key differences from other scenarios:
- `advanced: true` despite expecting input (auto-advanced through states to reach delegate state)
- Integration output included alongside directive
- Evidence submission is required after agent processes output
- Two-call flow: first to receive output, second to submit interpretation

### Finding 6: Error Codes and Structured Error Field

**PRD R9 specified error codes:**
- `gate_blocked`: conditions not yet satisfied; includes per-condition detail
- `precondition_failed`: submission provided but state doesn't accept one; or `--to` + `--with-data` together
- `invalid_submission`: submission format doesn't match, including partial submissions
- `integration_unavailable`: processing integration not accessible/timeout
- `workflow_not_initialized`: `koto next` before `koto init`
- `terminal_state`: `--to` on terminal state

**PRD R20 exit codes:**
- `0`: success
- `1`: transient (gates blocked, integration unavailable, version conflict)
- `2`: caller error (bad input, invalid submission, invalid transition, precondition)
- `3`: configuration error (corrupt state, invalid template, not initialized)

**Structured error field format:**
```json
{
  "error": {
    "code": "gate_blocked",
    "message": "Current state has unsatisfied conditions",
    "current_state": "wait_for_ci",
    "details": [
      {
        "name": "ci_pipeline",
        "type": "integration",
        "requires": "GitHub Actions test.yml must pass",
        "blocking": true
      },
      {
        "name": "approval_gate",
        "type": "evidence_gate",
        "requires": "reviewer approval",
        "blocking": true
      }
    ]
  }
}
```

**Branching state error (when submitted evidence doesn't satisfy any transition):**
```json
{
  "error": {
    "code": "gate_blocked",
    "message": "Submission satisfied no outgoing transitions",
    "current_state": "branching_state",
    "per_transition_failures": [
      {
        "target": "path_a",
        "condition": "decision == 'a'",
        "failed_because": "submitted decision='x', need 'a'"
      },
      {
        "target": "path_b",
        "condition": "decision == 'b'",
        "failed_because": "submitted decision='x', need 'b'"
      }
    ]
  }
}
```

### Finding 7: The `advanced` Field Signal and Auto-Advancement Interaction

**R5 (Advancement signal):**
> Every `koto next` response includes an `advanced` field indicating whether state changed during this call. Agents must not need to compare state names between calls to detect advancement.

**Semantics:**
- `advanced: false` → state unchanged, agent must take action or wait
- `advanced: true` → state changed (one or more transitions executed)
- Returned in ALL responses: success, failure, error states

**Auto-advancement chain behavior:**
- Single `koto next` call may execute multiple transitions if conditions are satisfied
- All intermediate states are committed atomically (each independently durable)
- Response reflects the final stopping state, not intermediate ones
- `advanced: true` means "at least one transition executed"
- Agent doesn't know intermediate state names (intentional per R2)

**Cycle detection (R2):**
- Tracks visited states within a single `koto next` call
- If advancement would re-enter a visited state, stops and returns that state's directive
- Each new `koto next` call starts with fresh visited set

**Examples:**
```
Call 1: state A (unsatisfied gate) -> advanced: false
Call 2: gate satisfied -> A -> B (auto) -> C (unsatisfied) -> advanced: true
Call 3: C's gate satisfied -> C -> D -> E (terminal) -> advanced: true
```

### Finding 8: Integration Output in Response

**R8 (Integration output in response):**
> When koto runs a processing integration (e.g., delegate CLI) during a `koto next` call, the integration's output is included in the response. The agent receives it as context for executing the directive and is responsible for interpreting the output.

**Two categories of integrations:**
1. **Condition integrations** (e.g., CI status check): koto runs to evaluate gates, output not returned
2. **Processing integrations** (e.g., delegate CLI): koto runs and returns output

**Integration output format:**
```json
{
  "integration": {
    "type": "delegate",
    "name": "deepseek-delegate",
    "available": true,
    "output": "<raw output from delegate>",
    "timeout_ms": 30000
  }
}
```

**R12 (Integration availability fallback):**
> For processing integrations, if the configured tool is not accessible or exceeds timeout, `koto next` returns the directive without integration output and includes `delegation.available: false` so the agent can handle the directive directly.

Alternative format when unavailable:
```json
{
  "integration": {
    "type": "delegate",
    "available": false,
    "error": "timeout after 30000ms",
    "fallback_directive": "Proceed without delegation output"
  }
}
```

Exit code is 1 (transient; agent may retry).

### Finding 9: Exit Code Mapping

**R20 Exit codes (detailed mapping):**

Exit 0 (success):
- `koto next` returns a directive (no advancement or advancement succeeded)
- State unchanged or one/more transitions executed

Exit 1 (transient, retry without intervention):
- Gates not yet satisfied (condition integration pending or evidence not submitted)
- Processing integration unavailable/timeout
- Version conflict (concurrent modification)
- Cycle detected during auto-advancement

Exit 2 (caller error, operator review):
- Invalid submission format
- Submission for state that doesn't accept one
- `--to` targeting invalid transition
- `--to` on terminal state
- `--to` and `--with-data` together
- Unknown flag/argument

Exit 3 (configuration error, operator intervention):
- Workflow not initialized (no state file)
- State file corrupt
- Template invalid (compilation fails)
- Template hash mismatch
- Integration misconfigured

**Current implementation (main.go):**
- Line 64: `os.Exit(1)` for all errors
- Lines 58-63: wraps TransitionError but doesn't differentiate exit codes
- No distinction between transient/caller/config errors

### Finding 10: Flag Interaction with Output Schema

**`--with-data <file>` flag (R3, R15):**
- Submits JSON file containing agent-supplied data
- Becomes `evidence_submitted` event in the log
- Must include all fields declared by current state (R3: partial submissions rejected)
- Validation happens before state change
- If valid and gates now satisfied, triggers advancement chain
- If invalid, returns `invalid_submission` error (exit 2)
- If state doesn't accept submissions, returns `precondition_failed` (exit 2)

**`--to <transition>` flag (R10a):**
- Named transition must be valid outgoing transition from current state
- Bypasses ALL condition evaluation (both shared and per-transition)
- Always a stopping point (no auto-advancement chain from target)
- Becomes `directed_transition` event with `directed: true` marker
- Returns directive for target state
- Invalid transition target returns caller error (exit 2)
- `--to` on terminal state returns `terminal_state` error (exit 2)
- Mutually exclusive with `--with-data` (returns `precondition_failed`, exit 2)

**Mutually exclusive validation:**
```go
// From design requirements, not yet in main.go
if withDataFile != "" && toTransition != "" {
    return &TransitionError{
        Code: "precondition_failed",
        Message: "--to and --with-data are mutually exclusive",
    }
    // Exit 2
}
```

**Output contract with flags:**
- No structural change to response schema based on flag
- Same `advanced`, `expects`, `error` fields present regardless
- `--with-data` may trigger advancement (affecting `advanced` value)
- `--to` bypasses normal conditions (but response still includes blocking_conditions for information)
- Integration output may be included if auto-advancement reaches delegate state

## Draft Output Schema

### Complete Success Response (No Advancement)

```json
{
  "action": "execute",
  "state": "current_state",
  "directive": "Full interpolated directive text with variables resolved",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "field_name": {
        "type": "string|enum|boolean|number",
        "description": "What this field means",
        "required": true,
        "enum": ["option1", "option2"]
      }
    },
    "options": [
      {
        "target": "next_state_name",
        "description": "What happens if this field value is submitted",
        "condition": "field_name == 'value'"
      }
    ]
  }
}
```

### Success Response with Auto-Advancement

```json
{
  "action": "execute",
  "state": "final_state_reached",
  "directive": "New directive after advancing through auto-satisfiable states",
  "advanced": true,
  "states_traversed": ["state_a", "state_b", "final_state_reached"],
  "expects": null,
  "blocking_conditions": [
    {
      "name": "manual_review",
      "type": "evidence_gate",
      "description": "Requires human review",
      "agent_actionable": true
    }
  ]
}
```

### Terminal State Response

```json
{
  "action": "done",
  "state": "complete",
  "message": "Workflow reached terminal state",
  "advanced": true,
  "expects": null
}
```

### Error Response: Gate Blocked (Transient)

```json
{
  "error": {
    "code": "gate_blocked",
    "message": "One or more conditions are not yet satisfied",
    "current_state": "waiting_state",
    "blocking_conditions": [
      {
        "name": "ci_check",
        "type": "integration",
        "description": "CI pipeline status check",
        "agent_actionable": false,
        "status": "pending"
      },
      {
        "name": "code_review",
        "type": "evidence_gate",
        "description": "Code review approval required",
        "agent_actionable": true
      }
    ]
  }
}
```
Exit: 1

### Error Response: Invalid Submission (Caller Error)

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "Submission does not match expected schema",
    "current_state": "decision_state",
    "expected_fields": {
      "choice": {
        "type": "enum",
        "values": ["option_a", "option_b"]
      },
      "justification": {
        "type": "string"
      }
    },
    "received_fields": {
      "choice": "invalid_option",
      "justification": "My reason"
    },
    "validation_errors": [
      {
        "field": "choice",
        "error": "value 'invalid_option' is not in allowed values"
      }
    ]
  }
}
```
Exit: 2

### Error Response: Branching State with No Matching Transition

```json
{
  "error": {
    "code": "gate_blocked",
    "message": "Submission satisfied no outgoing transitions",
    "current_state": "branch_state",
    "per_transition_analysis": [
      {
        "target": "path_a",
        "condition": "decision == 'a'",
        "status": "not_satisfied",
        "reason": "submitted decision='x', condition requires 'a'"
      },
      {
        "target": "path_b",
        "condition": "decision == 'b'",
        "status": "not_satisfied",
        "reason": "submitted decision='x', condition requires 'b'"
      }
    ]
  }
}
```
Exit: 1 (transient; agent may resubmit)

### Response with Processing Integration

```json
{
  "action": "execute",
  "state": "delegate_phase",
  "directive": "Analyze using delegate for deep reasoning",
  "advanced": true,
  "states_traversed": ["initial", "delegate_phase"],
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "interpretation": {
        "type": "string",
        "description": "Your assessment of delegate findings"
      },
      "proceed": {
        "type": "boolean",
        "description": "Whether to proceed based on analysis"
      }
    },
    "options": [
      {
        "target": "proceed",
        "condition": "proceed == true"
      },
      {
        "target": "refine",
        "condition": "proceed == false"
      }
    ]
  },
  "integration": {
    "type": "delegate",
    "available": true,
    "tool": "deepseek-delegate",
    "output": "{\"analysis\": \"...\", \"confidence\": 0.92}",
    "invoked_at": "2024-03-14T10:23:45Z"
  }
}
```

### Response with Unavailable Integration

```json
{
  "action": "execute",
  "state": "delegate_phase",
  "directive": "Would invoke delegate for deep reasoning, but tool unavailable. Proceed with manual analysis.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "analysis": {
        "type": "string",
        "description": "Your manual analysis"
      }
    }
  },
  "integration": {
    "type": "delegate",
    "available": false,
    "error": "timeout after 30000ms",
    "fallback": "Proceeding without delegate output"
  }
}
```
Exit: 1 (transient; agent may retry or proceed with fallback)

### Directed Transition Response

```json
{
  "action": "execute",
  "state": "target_state",
  "directive": "Directive for target state after --to override",
  "advanced": true,
  "transition_type": "directed",
  "expects": null,
  "note": "This transition was directed (bypassed condition evaluation)"
}
```

Exit: 0

## Implications for Design

### 1. Directive Struct Must Expand

Current struct is too minimal. New struct needed:
```go
type Directive struct {
    Action              string                 `json:"action"` // execute, done
    State               string                 `json:"state"`
    Directive           string                 `json:"directive,omitempty"`
    Message             string                 `json:"message,omitempty"`
    Advanced            bool                   `json:"advanced"` // NEW
    StatesTraversed     []string               `json:"states_traversed,omitempty"` // NEW
    Expects             *ExpectsSchema         `json:"expects,omitempty"` // NEW
    BlockingConditions  []ConditionDetail      `json:"blocking_conditions,omitempty"` // NEW
    Integration         *IntegrationOutput     `json:"integration,omitempty"` // NEW
    TransitionType      string                 `json:"transition_type,omitempty"` // directed | gate-cleared
}

type ExpectsSchema struct {
    EventType string                 `json:"event_type"`
    Fields    map[string]FieldSchema `json:"fields"`
    Options   []TransitionOption     `json:"options,omitempty"`
}

type FieldSchema struct {
    Type        string        `json:"type"` // string, enum, boolean, number
    Description string        `json:"description"`
    Required    bool          `json:"required"`
    Enum        []interface{} `json:"enum,omitempty"`
}

type TransitionOption struct {
    Target      string `json:"target"`
    Description string `json:"description,omitempty"`
    Condition   string `json:"condition,omitempty"`
}

type ConditionDetail struct {
    Name            string `json:"name"`
    Type            string `json:"type"` // integration, evidence_gate
    Description     string `json:"description"`
    AgentActionable bool   `json:"agent_actionable"`
    Status          string `json:"status,omitempty"` // pending, blocked, satisfied
}

type IntegrationOutput struct {
    Type       string `json:"type"` // delegate, etc
    Available  bool   `json:"available"`
    Tool       string `json:"tool,omitempty"`
    Output     string `json:"output,omitempty"`
    Error      string `json:"error,omitempty"`
    Fallback   string `json:"fallback,omitempty"`
    InvokedAt  string `json:"invoked_at,omitempty"`
}
```

### 2. Controller.Next() Must Compute `expects`

Current implementation:
```go
func (c *Controller) Next() (*Directive, error) {
    // Returns static directive, no expects
}
```

Must change to:
- Read current state's event schema from compiled template
- Extract field declarations and per-transition conditions
- Build `expects` object
- Detect if state has blocking conditions (gates not satisfied)
- Populate `blocking_conditions` if gates failed
- Set `advanced` flag (always false for simple Next call, true for chained advancement)

### 3. Template Compilation Must Extract Event Schemas

Current `CompiledTemplate` has:
```go
type StateDecl struct {
    Directive   string
    Transitions []string
    Terminal    bool
    Gates       map[string]engine.GateDecl
}
```

Must add:
```go
type StateDecl struct {
    // ... existing fields ...
    EventSchema *EventSchema `json:"event_schema,omitempty"`
}

type EventSchema struct {
    Fields       map[string]FieldDecl      `json:"fields"`
    Transitions  map[string]TransitionDecl `json:"transitions"`
}

type FieldDecl struct {
    Type        string        `json:"type"` // string, enum, boolean, number
    Description string        `json:"description"`
    Required    bool          `json:"required"`
    Enum        []interface{} `json:"enum,omitempty"`
}

type TransitionDecl struct {
    Target     string                    `json:"target"`
    Conditions []string                  `json:"conditions,omitempty"` // CEL or simple field checks
    Description string                   `json:"description,omitempty"`
}
```

### 4. Flag Parsing Must Support `--with-data` and `--to`

Current:
```go
func parseFlags(args []string, multiFlags map[string]bool) (*parsedArgs, error)
```

Must add validation for:
- `--with-data <file>` reads JSON file, parses payload
- `--to <transition>` validates transition is valid outgoing from current state
- Mutual exclusion check: error if both provided
- File reading and validation before state modifications

### 5. Auto-Advancement Engine Required

Current `controller.Next()` is stateless read-only. Must add:
- Loop that evaluates conditions
- Tracks visited states (cycle detection)
- Appends transition events
- Stops at unsatisfied conditions, processing integrations, or terminal
- Sets `advanced: true` if any transitions occurred
- Returns stopping state's directive

### 6. Exit Code Differentiation Required

Current: all errors exit with code 1. Must implement:
```go
func exitWithCode(err error, code int) {
    if err != nil {
        printError(getErrorCode(err), err.Error())
    }
    os.Exit(code)
}
```

- Examine error code and issue type
- Exit 0 on success
- Exit 1 for transient errors (gate_blocked when integration pending, version_conflict)
- Exit 2 for caller errors (invalid_submission, precondition_failed, bad transition)
- Exit 3 for config errors (workflow_not_initialized, template_mismatch)

### 7. Per-Transition Conditions Must Be Evaluable

Template format change needed to declare conditions on transitions:
```yaml
states:
  review:
    directive: "Review and decide..."
    event_schema:
      fields:
        decision:
          type: enum
          values: [refine, complete]
      when:
        - decision: refine
          target: refine_phase
        - decision: complete
          target: completion
```

Compilation must:
- Extract per-transition conditions
- Validate mutual exclusivity (only one transition can satisfy given evidence)
- Build condition evaluation expressions
- Include in `EventSchema` for runtime dispatch

## Surprises

1. **`expects` is orthogonal to `blocking_conditions`**: A state can have:
   - `expects: null` + `blocking_conditions: []` → auto-advance (no action needed)
   - `expects: {fields...}` + `blocking_conditions: []` → requires agent submission
   - `expects: null` + `blocking_conditions: [...]` → requires wait (gate pending)
   - Never both `expects` and `blocking_conditions` with content, because if there's an unsatisfied gate, agent can't fix it via submission

2. **Integration output doesn't prevent advancement**: A processing integration runs and returns output, BUT the state still expects evidence submission. The integration output is informational only. The agent must interpret it and submit evidence to advance. This is fundamentally different from condition integrations (CI checks), which block advancement directly.

3. **`--to` doesn't return a new auto-advancement chain**: Human override is a stopping point. The next `koto next` call (without `--to`) will re-evaluate conditions from the target state and potentially chain. This prevents double-advancement from manual override.

4. **Cycle detection happens within a single call, not across calls**: If the state graph has a loop, `koto next` detects it mid-advancement and stops, returning the state that would create the cycle. The next call starts fresh (no memory of visited states). This is simpler than cycle detection across calls and matches the design's per-call scoping.

5. **Evidence is per-state, not per-transition**: When an agent submits evidence in state A and moves to state B, state B starts with empty evidence even if there are multiple outgoing transitions from B. Per-transition conditions must be evaluated fresh with what the agent submits, not accumulated from prior states. This is guaranteed by the event-sourced model (evidence lives in `evidence_submitted` events, scoped to the state they were submitted in).

## Summary

The unified `koto next` output must include four new top-level fields: `advanced` (bool signaling state change), `expects` (null or object describing required submission schema), `blocking_conditions` (array of unmet gates with detail), and `integration` (output from processing integrations if applicable). Error responses must include typed error codes (`gate_blocked`, `invalid_submission`, `precondition_failed`, etc.) with structured detail for agents to determine next action without consulting template. Exit codes must differentiate transient (1), caller (2), and configuration (3) errors so CI pipelines can branch appropriately. The `Directive` struct expands significantly to carry this metadata, and the controller must compute `expects` from the template's new event schema declarations, which describe per-transition conditions and required evidence fields.
