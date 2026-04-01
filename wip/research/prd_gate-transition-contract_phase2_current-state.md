# Research: Current State of Gate/Transition/Override System in Koto

**Date**: March 30, 2026
**Analysis Phase**: Phase 2 Current-State Analysis
**Analyst Role**: Current-State Analyst
**Status**: Complete

---

## Executive Summary

The gate/transition/override system in koto has a fundamental architectural mismatch: gates are pure blockers that produce only pass/fail signals, while transitions are routed by agent evidence through `accepts` blocks and `when` conditions. There is no data flow between gates and transitions. Template authors work around this by adding manual `accepts` blocks with `override` enum values—a pattern that exists solely because the engine lacks a mechanism to unify gate output with transition routing.

**Key findings:**
1. Gates produce only three outcomes: `Passed`, `Failed { exit_code }`, or `TimedOut`/`Error` — no structured data
2. Transitions are routed entirely by agent evidence (`when` conditions on `accepts` field values)
3. On gate failure with no `accepts` block: workflow is blocked, agent has no path forward except `--to` (loses audit trail)
4. On gate failure with `accepts` block: engine falls through to evidence collection and requires agent to submit proof of override
5. The override workaround pattern: add `accepts` with enum `override` value + matching transition (`target: <state> when: status: override`)
6. Templates must manually couple gate existence, override enum values, and transition conditions—no compiler validation that they align
7. Zero structured data about why an override occurred; `koto decisions record` is optional and separate
8. No cross-session query for overrides; no visibility into override history beyond the event log

---

## Part 1: All Gate Types and Their Behavior

### Gate Types Defined in `src/template/types.rs`

Three gate types are supported:

| Type | Constant | Purpose | Input Fields | Output |
|------|----------|---------|--------------|--------|
| `command` | `GATE_TYPE_COMMAND` | Executes a shell command, checks exit code | `command: String`, `timeout: u32` (seconds, 0 = default 30s) | `Passed` if exit 0, `Failed { exit_code }` if non-zero, `TimedOut` if timeout exceeded, `Error { message }` on spawn failure |
| `context-exists` | `GATE_TYPE_CONTEXT_EXISTS` | Checks whether a content key exists in session store | `key: String` | `Passed` if key exists, `Failed { exit_code: 1 }` if not, `Error` if context store unavailable |
| `context-matches` | `GATE_TYPE_CONTEXT_MATCHES` | Checks whether content for a key matches a regex pattern | `key: String`, `pattern: String` (regex) | `Passed` if pattern matches, `Failed { exit_code: 1 }` if not, `Error` if key missing or regex invalid |

### GateResult Enum (`src/gate.rs`)

All gates return one of four outcomes:

```rust
pub enum GateResult {
    Passed,                      // Condition satisfied
    Failed { exit_code: i32 },   // Condition not met (command exit code or 1 for context checks)
    TimedOut,                    // Command timeout
    Error { message: String },   // Spawn failure, missing context store, invalid regex
}
```

**Critical observation**: No gate produces structured data. The only information returned is the outcome and, for failed command gates, an exit code. Context gates return hardcoded exit code 1 on failure.

### Gate Evaluation in `src/gate.rs`

**Function**: `evaluate_gates(gates: &BTreeMap<String, Gate>, ...) -> BTreeMap<String, GateResult>`

**Key behaviors**:
- All gates are evaluated regardless of prior results (no short-circuit)
- Command gates run with configurable `timeout` (default 30 seconds if timeout is 0)
- Command gates execute in a specific `working_dir` (usually the session directory)
- Context gates require both a `ContextStore` and a session name; if either is missing, they return `Error`
- Regex validation happens at evaluation time (invalid pattern = `Error`)
- Results are returned as a `BTreeMap<String, GateResult>` keyed by gate name

---

## Part 2: All Templates in the Repo That Use Gates

### Template: `test/functional/fixtures/templates/simple-gates.md`

```yaml
name: simple-gates
version: "1.0"
description: Tests gate-with-evidence-fallback behavior
initial_state: start

states:
  start:
    gates:
      check_file:
        type: command
        command: "test -f wip/check.txt"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        required: false
    transitions:
      - target: done
        when:
          status: completed
      - target: done
        when:
          status: override
      - target: done
  done:
    terminal: true
```

**Analysis**:
- **State**: `start` has a single command gate (`check_file`)
- **Gate behavior**: Checks if `wip/check.txt` exists (exit 0 = pass, exit 1 = fail)
- **Override workaround**: `accepts` block declares `status` enum with three values: `completed`, `override`, `blocked`
  - Two conditional transitions: one for `completed`, one for `override` (both point to `done`)
  - One unconditional transition: `target: done` (fires when gates pass and no evidence matches a conditional)
- **What happens on gate pass**: Auto-advances to `done` (unconditional transition)
- **What happens on gate fail**: Requires evidence; agent must submit `status` value; if `override` submitted, advances to `done`

**Pain point**: The `override` and `blocked` enum values only exist as a workaround. Template author had to manually couple the gate existence to the enum values and match them to transitions.

---

### Template: `test/functional/fixtures/templates/multi-state.md`

```yaml
name: multi-state
version: "1.0"
description: Tests full workflow pattern with multiple states
initial_state: entry

states:
  entry:
    accepts:
      route:
        type: enum
        values: [setup, work]
        required: true
    transitions:
      - target: setup
        when:
          route: setup
      - target: work
        when:
          route: work
  setup:
    gates:
      config_exists:
        type: command
        command: "test -f wip/config.txt"
    accepts:
      status:
        type: enum
        values: [completed, override]
        required: true
    transitions:
      - target: work
  work:
    accepts:
      status:
        type: enum
        values: [completed]
        required: true
    transitions:
      - target: done
  done:
    terminal: true
```

**Analysis**:
- **Entry state**: No gates; accepts evidence to route to either `setup` or `work`
- **Setup state** (gate state):
  - Gate: `config_exists` checks if `wip/config.txt` exists
  - Override workaround: `accepts` with `status` enum values `[completed, override]`
  - Transitions: Only one target (`work`); no conditional transitions (unconditional fallback only)
  - Behavior: If gate passes, auto-advances. If gate fails, requires evidence. Agent submits either `completed` or `override`; both lead to same transition (irrelevant which enum value used when gate failed)

**Pain point**: The `override` enum value is added but has no actual effect—the single unconditional transition fires regardless. This is busywork.

---

### Template: `test/functional/fixtures/templates/var-substitution.md`

```yaml
name: var-substitution
version: "1.0"
description: Tests variable substitution in gate commands
initial_state: check

variables:
  MY_VAR:
    description: Variable to substitute in gate command
    required: true

states:
  check:
    gates:
      var_gate:
        type: command
        command: "test -f wip/{{MY_VAR}}.txt"
    transitions:
      - target: done
  done:
    terminal: true
```

**Analysis**:
- **No accepts block**: This is a pure gate-blocking state
- **Gate**: Command gate with variable substitution; checks if `wip/<MY_VAR>.txt` exists
- **Behavior**: 
  - If gate passes: auto-advances to `done`
  - If gate fails: returns `GateBlocked` response; **agent has no way to override** (no `accepts` block, no evidence mechanism)
  - Only option: `koto next --to done` (loses audit trail)

**Pain point**: Template author cannot provide a way for agents to override. The state is a dead end on gate failure.

---

### Template: `test/functional/fixtures/templates/decisions.md`

No gates. (Included for completeness; shows evidence-only workflow.)

---

### Template: `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md`

Partial excerpt (full template is 88 lines):

```yaml
states:
  compile_validation:
    gates:
      template_exists:
        type: context-exists
        key: koto-templates/*.md
    accepts:
      compile_result:
        type: enum
        values: [pass, fail]
        required: true
    transitions:
      - target: skill_authoring
        when:
          compile_result: pass
      - target: compile_validation
        when:
          compile_result: fail
```

**Analysis**:
- **Gate**: `context-exists` checks if template exists in the session store at key `koto-templates/*.md`
- **Override workaround**: `accepts` with enum `[pass, fail]`
  - Conditional transition to `skill_authoring` when `pass`
  - Self-loop back to `compile_validation` when `fail`
  - NO unconditional fallback
- **Behavior**:
  - If gate passes: blocks until evidence submitted (no auto-advance because there's no unconditional transition)
  - If gate fails: also blocks for evidence (accepts block forces evidence collection even on gate failure)
  - Agent must submit either `pass` or `fail` to route

**Observation**: This template uses `accept` values to route based on gate outcome, not to override it. The `compile_validation` logic is encoded in the agent (agent decides pass/fail), not in koto.

---

## Part 3: How Gates Interact with Transitions Today

### Key Code: `src/engine/advance.rs`

#### Gate Evaluation (lines 295-315)

```rust
let gate_results = evaluate_gates(&template_state.gates);
let any_failed = gate_results
    .values()
    .any(|r| !matches!(r, GateResult::Passed));
if any_failed {
    // If the state has an accepts block, fall through to transition
    // resolution instead of returning GateBlocked.
    if template_state.accepts.is_none() {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            stop_reason: StopReason::GateBlocked(gate_results),
        });
    }
    gates_failed = true;
    failed_gate_results = Some(gate_results);
    // Fall through to transition resolution with gate_failed=true.
}
```

**Key behavior**:
- **No `accepts` block**: Gate failure returns `GateBlocked` immediately; workflow stops
- **With `accepts` block**: Gate failure does NOT block; falls through to transition resolution with `gate_failed = true` flag

#### Transition Resolution (lines 319-435)

The `resolve_transition` function signature:

```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
    gate_failed: bool,
) -> TransitionResolution
```

**Three possible outcomes**:
```rust
pub enum TransitionResolution {
    Resolved(String),           // Single target matched or fallback chosen
    Ambiguous(Vec<String>),     // Multiple conditional transitions match
    NeedsEvidence,              // No match; requires evidence submission
    NoTransitions,              // State has no transitions (error)
}
```

#### gate_failed Parameter Behavior (lines 385-435)

When `gate_failed = true`:

1. **Conditional transitions** (`when` clauses) are checked against evidence as normal
2. **If exactly one conditional matches**: `Resolved(target)` — advance to that target
3. **If multiple conditionals match**: `Ambiguous` — error (mutual exclusivity violation)
4. **If no conditional matches AND an unconditional transition exists**:
   - **When `gate_failed = false`**: `Resolved(fallback)` (auto-advance)
   - **When `gate_failed = true`**: `NeedsEvidence` (block and require evidence)
5. **If no conditional matches AND no unconditional**: `NeedsEvidence`

**Critical behavior**: The `gate_failed` flag prevents unconditional transitions from firing when gates fail. This forces the agent to submit evidence even if an unconditional fallback exists.

```rust
if gate_failed {
    // Gate failed and no evidence matches a conditional transition.
    // Don't auto-advance via the unconditional fallback — require
    // evidence so the agent can provide override or recovery input.
    TransitionResolution::NeedsEvidence
} else {
    TransitionResolution::Resolved(fallback)
}
```

---

## Part 4: The Override Workaround Pattern

### Pattern Definition

When a state has gates, template authors must add an `accepts` block with an enum value named `override` (or similar) to allow agents to bypass a failed gate. The pattern consists of three coupled elements:

1. **Gate declaration** in the state
2. **Accepts block** with an enum field containing `override` as a value
3. **Conditional transition** with `when: { field: override }` pointing to the next state

### Example: `simple-gates.md`

```yaml
states:
  start:
    gates:
      check_file:
        type: command
        command: "test -f wip/check.txt"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]     # <-- includes override
        required: true
    transitions:
      - target: done
        when:
          status: completed                        # <-- route for pass path
      - target: done
        when:
          status: override                         # <-- route for override path
      - target: done                               # <-- fallback when gate passes
```

### How It Works

1. **Gate passes**: Auto-advances via unconditional transition
2. **Gate fails**: Returns `EvidenceRequired` (not `GateBlocked`) because `accepts` block exists
3. **Agent submits evidence**: Calls `koto next --with-data '{"status": "override"}'`
4. **Engine routes transition**: The conditional `when: { status: override }` matches; advances to target

### Fragility and Pain Points

| Issue | Consequence |
|-------|-----------|
| **Manual coupling** | Gate name doesn't appear in enum values; compiler can't verify they're related |
| **Naming convention** | No standard for the field name (`status`, `decision`, `action`) or value (`override`, `bypass`, `force`); templates are inconsistent |
| **Multiple overridable gates** | No way to distinguish which gate was overridden; all gates lumped into a single enum |
| **Enum value bloat** | For each gate, need an enum value; states with 3 gates might have 5+ enum values just for override paths |
| **Transition explosion** | Each override enum value needs its own transition (or be lumped with other values), multiplying the transition count |
| **No structured reason** | The override enum value carries no data about why; rationale must be captured separately via `koto decisions record` |
| **Irrelevant coupling** | In `multi-state.md`, the `override` enum value exists but has no effect (same target as auto-advance) |
| **No compiler validation** | Compiler doesn't check that override paths are consistent across similar states |

---

## Part 5: What Callers See

### CLI User Perspective (from `docs/guides/cli-usage.md`)

#### Evidence Required Response (when gates fail on state with accepts)

```json
{
  "action": "evidence_required",
  "state": "review",
  "directive": "Review the code changes.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "required": true, "values": ["proceed", "escalate"]}
    },
    "options": [
      {"target": "implement", "when": {"decision": "proceed"}}
    ]
  },
  "blocking_conditions": [
    {"name": "ci_check", "type": "command", "status": "failed", "agent_actionable": false}
  ],
  "error": null
}
```

**What the agent sees**:
- A `blocking_conditions` array populated with failed gates
- An `expects` schema that includes fields for overriding (e.g., the enum with `override` value)
- No indication that the enum values are override workarounds; they're just part of the schema
- No structured data about which gate each enum value overrides

#### Gate Blocked Response (when gates fail on state without accepts)

```json
{
  "action": "gate_blocked",
  "state": "deploy",
  "directive": "Deploy to staging.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {"name": "ci_check", "type": "command", "status": "failed", "agent_actionable": false}
  ],
  "error": null
}
```

**What the agent sees**:
- `blocking_conditions` with the failed gates
- `expects: null` — no fields to submit
- The agent is stuck; only option is `koto next --to <target>` (directed transition, loses audit trail)
- The directive text must tell the agent how to fix the gates manually

### Agent Instructions (from `docs/guides/custom-skill-authoring.md` and `plugins/koto-skills/AGENTS.md`)

**How agents are instructed to handle gates**:

From `AGENTS.md`:
> "When gates fail on a state with accepts: The response is still `"evidence_required"`, but the `blocking_conditions` array is populated with the failing gates. Fix the blocking conditions first, then call `koto next` again -- the engine re-evaluates gates automatically. Once gates pass, you can submit evidence normally."

**Key assumptions agents are told**:
1. Gates can fail and block transitions
2. Failed gates can sometimes be overridden by submitting evidence (if `accepts` block exists)
3. The override mechanism is implicit; agents must infer from the `expects` schema that enum values represent overrides
4. No mention of auditing overrides or capturing rationale

### Skill Author Perspective (from `docs/guides/custom-skill-authoring.md`)

From the **Gate evidence keys** section:
> "Document each gate from the template. The agent needs to know what conditions must hold before calling `koto next`."

Example:
> "The `awakening` state has one gate: **greeting_exists** (context-exists gate): checks for key `spirit-greeting.txt` in the content store. The agent must submit the greeting via `koto context add` before transitioning to `eternal`."

**Skill author burden**:
1. Extract gates from compiled template
2. Document each gate's purpose and failure condition
3. Document the evidence keys/enum values that override each gate
4. Ensure the documented overrides match what's actually in the template (no tool validation)
5. Tell agents how to submit evidence to override (implicit pattern; not standardized)

---

## Part 6: Pain Points and Confusions Identified

### 1. No Structured Data from Gates

**Problem**: Gates produce only pass/fail signals. Even a command gate that extracts structured output (e.g., `jq` returning JSON) is reduced to an exit code.

**Evidence**: 
- `GateResult` enum only has `Passed` or `Failed { exit_code }`
- No schema field on `Gate` struct to declare what a gate produces
- `context-matches` gates return hardcoded exit code 1; the matched content is discarded

**Impact**: 
- Transitions cannot route on structured gate output
- Template authors cannot declare "if this gate produces X, go to state Y"
- Agents cannot submit gate output as evidence; they must re-run the gate or manually provide the data

### 2. Gates and Transitions Are Decoupled

**Problem**: Gates and transitions operate in separate namespaces. A failed gate doesn't automatically feed data to transition routing.

**Evidence**:
- `evaluate_gates` returns `BTreeMap<String, GateResult>` (gate outcomes)
- `resolve_transition` takes `evidence: BTreeMap<String, serde_json::Value>` (agent-provided JSON)
- No bridge between them; gates don't populate evidence

**Impact**:
- Template authors must manually couple gate presence to enum override values
- Compiler doesn't validate that override paths exist for each gate
- Agents cannot see which enum values correspond to which gates

### 3. The Override Enum Value Workaround

**Problem**: Template authors must add enum values like `override`, `bypass`, `force` to allow agents to skip failed gates. This is busywork that exists only because the engine lacks a native override mechanism.

**Evidence**:
- `simple-gates.md`: `values: [completed, override, blocked]`
- `multi-state.md`: `values: [completed, override]`
- No field in `Gate` or `Transition` structs to declare overrides
- No compiler check that override values are consistent

**Impact**:
- Every gate-enabled state needs extra enum values and transitions
- Inconsistent naming (sometimes `override`, sometimes not)
- No way to query "what gates were overridden and why" without parsing the event log
- Skill authors must document override enum values manually

### 4. No Structured Override Rationale

**Problem**: Agents can override gates implicitly by submitting evidence, but there's no structured capture of why.

**Evidence**:
- The PRD-override-gate-rationale identifies this as a major gap
- Current workaround: `koto decisions record` is optional and separate from the override
- No override event in the state machine; only the evidence event

**Impact**:
- Human reviewers cannot audit why gates were bypassed
- No cross-session query for overrides
- No mandatory rationale capture
- Agents may not even think to record decisions

### 5. Compiler Allows Incomplete Override Paths

**Problem**: The compiler doesn't validate that all gates have override paths or that override enum values match across similar states.

**Evidence**:
- `var-substitution.md`: Has a gate but no `accepts` block; gate failure is a dead end
- No validation that enum override values are semantically linked to gates
- No check that states with similar gates use consistent override naming

**Impact**:
- Template authors may create unreachable states by accident
- Different templates use different enum value names for the same override pattern
- Agents see inconsistent interfaces across skills

### 6. No Data from Context-Aware Gates

**Problem**: `context-exists` and `context-matches` gates check the session store but don't return the content.

**Evidence**:
- `context-matches` validates a regex but discards the content
- No `content: String` field in `GateResult` for context gates
- Agent must call `koto context get` to see what was in the store

**Impact**:
- Gates cannot produce structured output for transition routing
- Example: a gate checking if a "status" file contains "ready" doesn't return the status—just pass/fail
- Transitions cannot route on the actual status value

### 7. Gate-Blocking States Freeze the Workflow

**Problem**: States without `accepts` blocks that fail gates have no recovery path short of `--to` (loses audit trail).

**Evidence**:
- `var-substitution.md` demonstrates this: gate failure returns `GateBlocked`, no evidence mechanism
- `koto next --to <target>` bypasses gate evaluation entirely and doesn't record rationale

**Impact**:
- Agents are forced to use `--to` to bypass gates on these states
- No audit trail of the bypass
- Skill authors cannot provide a graceful override mechanism

### 8. Multiple Gates, Single Override

**Problem**: When a state has multiple gates and some fail, template authors cannot distinguish which gate the agent is overriding.

**Evidence**:
- If a state has gates `gate_a`, `gate_b`, `gate_c` and gates A and C fail, submitting `override` doesn't specify which gates to bypass
- Override enum value applies to all failed gates or none

**Impact**:
- Templates with multiple gates are hard to design
- Agent cannot selectively override certain gates
- Incomplete data for audit (which gates were actually overridden?)

### 9. No Clear Semantics for `accepts` on Gate States

**Problem**: Template authors are unclear whether `accepts` is for gate override, transition routing, or both.

**Evidence**:
- `koto-author.md`: `compile_validation` state uses accepts to route based on agent-supplied pass/fail, not to override the gate
- `simple-gates.md`: `accepts` is clearly for override
- `multi-state.md`: Single unconditional transition; `accepts` values are irrelevant

**Impact**:
- Template semantics are ambiguous
- Agents see `expects` schema and don't know if enum values are overrides or routing conditions
- Skill authors must document intent in prose (no structural clarity)

### 10. Template Bloat from Override Patterns

**Problem**: Adding override paths multiplies the number of enum values and transitions.

**Evidence**:
- `simple-gates.md`: 3 enum values, 3 transitions for a single gate
- Scaling to multiple gates makes templates unreadable

**Impact**:
- Templates are verbose and hard to maintain
- More surface area for bugs (missing transitions, wrong targets)
- Harder to teach template authoring

---

## Part 7: Summary Table of All Gate-Enabled States

| Template | State | Gates | Gate Type(s) | Accepts Block? | Override Values | Transitions | What Happens on Gate Pass | What Happens on Gate Fail |
|----------|-------|-------|--------------|----------------|-----------------|-------------|--------------------------|--------------------------|
| **simple-gates.md** | `start` | `check_file` (1) | command | Yes | `[completed, override, blocked]` | 3 (2 conditional, 1 unconditional) | Auto-advance to `done` via unconditional | Requires evidence; routes to `done` if `override` or `completed` submitted, unknown behavior if `blocked` |
| **multi-state.md** | `setup` | `config_exists` (1) | command | Yes | `[completed, override]` | 1 (unconditional) | Auto-advance to `work` | Requires evidence; both enum values route to `work` (irrelevant) |
| **var-substitution.md** | `check` | `var_gate` (1) | command | No | N/A | 1 (unconditional) | Auto-advance to `done` | Returns `GateBlocked`; agent must use `--to done` (no override path) |
| **koto-author.md** | `compile_validation` | `template_exists` (1) | context-exists | Yes | `[pass, fail]` | 2 (conditional to `skill_authoring` on pass, self-loop on fail) | No auto-advance; requires evidence | Requires evidence; conditional routes based on agent-supplied pass/fail (not gate-driven) |

**Patterns observed**:
- All gate states with `accepts` blocks use an enum value pattern for overrides
- Naming is inconsistent: `override`, `completed`, `pass`, `fail`
- No state uses multiple gates (would require exponential enum expansion)
- No state with `accepts` block has a truly conditional override (all overrides route the same way)
- States without `accepts` blocks are dead ends on gate failure

---

## Part 8: Compiler Validation Gaps

The compiler (`CompiledTemplate::validate()` in `src/template/types.rs`) checks:

1. ✓ Gate type is one of the three supported types
2. ✓ Gate command is non-empty
3. ✓ Context gates have non-empty key and pattern (for context-matches)
4. ✓ Regex patterns are valid
5. ✓ Transition targets exist
6. ✓ When conditions reference declared fields in `accepts`
7. ✓ When condition values match field types (e.g., enum values are in the declared list)
8. ✓ Transitions with `when` clauses require an `accepts` block

**Missing validations**:
1. ✗ No check that gates have a corresponding override path in transitions
2. ✗ No check that enum values with override semantics are used consistently across states
3. ✗ No validation that states without `accepts` blocks have a fallback (gate failure is unrecoverable)
4. ✗ No type/schema for gate output; gates and transitions remain separate
5. ✗ No mutual-exclusivity validation for override paths (states with multiple gates)
6. ✗ No check that override enum values are semantically linked to gate names

---

## Part 9: Architectural Observations

### Current Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Template State                                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Gates: BTreeMap<String, Gate>                                 │
│  ├─ gate_name_1: command | context-exists | context-matches    │
│  └─ gate_name_2: ...                                            │
│                                                                 │
│  Accepts: Option<BTreeMap<String, FieldSchema>>                │
│  ├─ field_1: enum | string | number | boolean                  │
│  └─ field_2: ...                                               │
│                                                                 │
│  Transitions: Vec<Transition>                                   │
│  ├─ Transition 1: target: next_state, when: {field: value}     │
│  ├─ Transition 2: target: other_state, when: {field: value}    │
│  └─ Transition 3: target: fallback_state (no when clause)      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘

Engine Flow on `koto next`:
  1. Evaluate all gates → GateResult map
  2. Check if any gates failed
  3. If failed and no accepts:
     ├─ Return GateBlocked (stop)
  4. If failed and has accepts:
     ├─ Set gate_failed flag
     ├─ Wait for agent evidence
     ├─ When evidence arrives, resolve_transition(evidence, gate_failed=true)
  5. If not failed and no conditional matches and gate_failed=false:
     ├─ Use unconditional fallback
  6. If not failed and no conditional matches and gate_failed=true:
     ├─ Return NeedsEvidence (impossible if gate_failed=true, gates_failed!)

Critical gap: Gates produce GateResult; transitions consume BTreeMap<String, Value>.
No bridge. Template author must manually couple them via enum override values.
```

### Key Architectural Constraints

1. **Gates are pure predicates**: They answer "does this condition hold?" but don't produce data
2. **Transitions are data-driven**: They route on evidence values in the `accepts` map
3. **No data flow from gates to transitions**: A gate that could produce `{status: "ready"}` doesn't; it produces only `Passed` or `Failed`
4. **Override is implicit**: There's no "override" token in the data model; agents submit evidence that happens to match an override enum value
5. **No audit trail for overrides**: When an agent submits `override`, the engine doesn't record "this is an override of gate X"; it just routes based on evidence

---

## Part 10: Current Workarounds and Their Costs

### Workaround 1: Add Enum Override Values

**Template author does**:
```yaml
accepts:
  decision:
    type: enum
    values: [proceed, override]  # <-- added just for override
  transitions:
    - target: next_state
      when:
        decision: override       # <-- routes the override
```

**Cost**: 
- Extra enum value for each gate
- Extra transition for each override path
- Inconsistent naming across templates
- No semantic link to the gate name

### Workaround 2: Use `--to` for Forced Transitions

**Agent does**:
```bash
koto next workflow --to next_state
```

**Cost**:
- Bypasses gate evaluation entirely
- No override recorded; only a directed transition event
- No rationale captured
- Not an audit trail; indistinguishable from normal agent action

### Workaround 3: Record Override Rationale via Decisions

**Agent does**:
```bash
koto decisions record workflow --with-data '{"choice": "...", "rationale": "..."}'
koto next workflow --with-data '{"status": "override"}'
```

**Cost**:
- Two separate operations
- Rationale is optional; no enforcement
- Rationale and override are in separate events
- `koto decisions list` is epoch-scoped; no cross-session query

### Workaround 4: Document Overrides in SKILL.md Prose

**Skill author does**:
```markdown
## Evidence keys

The `setup` state has a gate `config_exists` that checks if the config file exists.
If it fails, submit `{"status": "override"}` to proceed.
```

**Cost**:
- Manual documentation; no validation
- Easy to get out of sync with template
- Agents must infer intent from prose

---

## Conclusion: What Needs to Change

The gate/transition/override system needs unification. Currently:

1. **Gates are isolated producers of pass/fail results** — they should produce structured data with a schema
2. **Transitions are isolated consumers of agent evidence** — they should also be able to route on gate output
3. **Overrides are implicit workarounds** — they should be first-class with structured rationale capture
4. **Templates couple gates and overrides manually** — the compiler should validate this coupling

The proposed "gate-transition-contract" design should:

1. Define per-gate output schemas alongside gate declarations
2. Enable transitions to route on both gate outputs and agent evidence (unified evidence map)
3. Provide a built-in override mechanism with mandatory structured rationale
4. Add compiler validation that gate schemas, override defaults, and transition when clauses form a complete contract

Without these changes, template authors will continue to add workaround enum values, and agents will continue to bypass gates implicitly with no audit trail.

---

## References

- `src/gate.rs` – Gate evaluation logic and GateResult enum
- `src/template/types.rs` – Gate and FieldSchema definitions; compiler validation
- `src/engine/advance.rs` – Gate-transition interaction via gate_failed flag
- `docs/guides/cli-usage.md` – CLI response shapes (what agents see)
- `docs/guides/custom-skill-authoring.md` – Skill author burden and patterns
- `plugins/koto-skills/AGENTS.md` – Agent instructions on gate handling
- `docs/prds/PRD-override-gate-rationale.md` – Proposed override mechanism
- Test templates in `test/functional/fixtures/templates/` – Real workaround patterns
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md` – Realistic skill template with gates

