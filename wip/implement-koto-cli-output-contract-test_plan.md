# Test Plan: koto CLI Output Contract

Generated from: docs/plans/PLAN-koto-cli-output-contract.md
Issues covered: 4

---

## Infrastructure Scenarios

### Scenario 1: NextResponse EvidenceRequired serializes correct JSON shape
**ID**: scenario-1
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Serialized JSON includes `action: "execute"`, `state`, `directive`, `advanced`, `expects` (object), `error: null`. Fields `blocking_conditions` and `integration` are absent.
**Status**: passed

### Scenario 2: NextResponse GateBlocked serializes correct JSON shape
**ID**: scenario-2
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Serialized JSON includes `action: "execute"`, `state`, `directive`, `advanced`, `blocking_conditions` (array), `expects: null`, `error: null`. Field `integration` is absent.
**Status**: passed

### Scenario 3: NextResponse Integration serializes correct JSON shape
**ID**: scenario-3
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Serialized JSON includes `action: "execute"`, `state`, `directive`, `advanced`, `expects` (object or null), `integration` (object), `error: null`. Field `blocking_conditions` is absent.
**Status**: passed

### Scenario 4: NextResponse IntegrationUnavailable serializes correct JSON shape
**ID**: scenario-4
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Serialized JSON includes `action: "execute"`, `state`, `directive`, `advanced`, `expects` (object or null), `integration` (object with `available: false`), `error: null`. Field `blocking_conditions` is absent.
**Status**: passed

### Scenario 5: NextResponse Terminal serializes correct JSON shape
**ID**: scenario-5
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Serialized JSON includes `action: "done"`, `state`, `advanced`, `error: null`. Fields `directive`, `expects`, `blocking_conditions`, and `integration` are absent or null per the field presence table (`expects: null`, `directive` absent).
**Status**: passed

### Scenario 6: NextErrorCode serializes as snake_case strings
**ID**: scenario-6
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: Each NextErrorCode variant serializes to its snake_case form: `gate_blocked`, `invalid_submission`, `precondition_failed`, `integration_unavailable`, `terminal_state`, `workflow_not_initialized`.
**Status**: passed

### Scenario 7: NextErrorCode exit code mapping
**ID**: scenario-7
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: `gate_blocked` and `integration_unavailable` map to exit code 1. `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized` map to exit code 2.
**Status**: passed

### Scenario 8: ExpectsSchema omits options when empty
**ID**: scenario-8
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: When `options` is an empty Vec, the `options` key is absent from serialized JSON. When non-empty, it appears.
**Status**: passed

### Scenario 9: ExpectsFieldSchema serializes field_type as "type" and omits empty values
**ID**: scenario-9
**Testable after**: Issue 1
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: JSON key is `"type"` not `"field_type"`. When `values` is empty, the `values` key is absent.
**Status**: passed

### Scenario 10: Evidence validation rejects missing required fields
**ID**: scenario-10
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/engine/evidence.rs`)
**Expected**: Submitting `{}` against a schema with a required field returns an error with per-field detail naming the missing field.
**Status**: passed

### Scenario 11: Evidence validation rejects type mismatches for each type
**ID**: scenario-11
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/engine/evidence.rs`)
**Expected**: Submitting a number where a string is expected, a string where a number is expected, a string where a boolean is expected, and a non-matching string for an enum field each produce a type mismatch error detail.
**Status**: passed

### Scenario 12: Evidence validation rejects unknown fields
**ID**: scenario-12
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/engine/evidence.rs`)
**Expected**: Submitting `{"unknown_field": "value"}` against a schema that does not declare `unknown_field` returns an error detail for the unknown field.
**Status**: passed

### Scenario 13: Evidence validation collects all errors without short-circuit
**ID**: scenario-13
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/engine/evidence.rs`)
**Expected**: Submitting a payload with multiple problems (missing required field AND unknown field AND type mismatch) returns error details for all three problems in one response.
**Status**: passed

### Scenario 14: Evidence validation accepts valid payload
**ID**: scenario-14
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/engine/evidence.rs`)
**Expected**: Submitting a payload matching all schema requirements returns `Ok(())`.
**Status**: passed

### Scenario 15: derive_expects returns None for state without accepts
**ID**: scenario-15
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: A `TemplateState` with `accepts: None` produces `None` from `derive_expects`.
**Status**: passed

### Scenario 16: derive_expects populates options from conditional transitions
**ID**: scenario-16
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: A state with `accepts` and two transitions with `when` conditions produces an `ExpectsSchema` with `event_type: "evidence_submitted"`, correct `fields`, and `options` containing both transition targets and their `when` maps.
**Status**: passed

### Scenario 17: derive_expects omits options when no transitions have when
**ID**: scenario-17
**Testable after**: Issue 2
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next_types.rs`)
**Expected**: A state with `accepts` but only unconditional transitions produces an `ExpectsSchema` with empty `options` (which serializes without the `options` key).
**Status**: passed

### Scenario 18: Gate evaluator passes gate on exit 0
**ID**: scenario-18
**Testable after**: Issue 3
**Category**: automatable
**Commands**:
- `cargo test` (integration tests in `src/gate.rs`)
**Expected**: A gate with command `true` (or `exit 0`) returns `GateResult::Passed`.
**Status**: pending

### Scenario 19: Gate evaluator reports failure on non-zero exit
**ID**: scenario-19
**Testable after**: Issue 3
**Category**: automatable
**Commands**:
- `cargo test` (integration tests in `src/gate.rs`)
**Expected**: A gate with command `exit 42` returns `GateResult::Failed { exit_code: 42 }`.
**Status**: pending

### Scenario 20: Gate evaluator handles timeout
**ID**: scenario-20
**Testable after**: Issue 3
**Category**: automatable
**Commands**:
- `cargo test` (integration tests in `src/gate.rs`)
**Expected**: A gate with command `sleep 60` and timeout of 1 second returns `GateResult::TimedOut`. The spawned sleep process and its process group are killed.
**Status**: pending

### Scenario 21: Gate evaluator handles non-existent command
**ID**: scenario-21
**Testable after**: Issue 3
**Category**: automatable
**Commands**:
- `cargo test` (integration tests in `src/gate.rs`)
**Expected**: A gate with a non-existent command returns `GateResult::Error` or `GateResult::Failed` (depending on how `sh -c` handles it -- likely exit 127).
**Status**: pending

### Scenario 22: Gate evaluator runs all gates without short-circuit
**ID**: scenario-22
**Testable after**: Issue 3
**Category**: automatable
**Commands**:
- `cargo test` (integration tests in `src/gate.rs`)
**Expected**: Given three gates (one passing, one failing, one timing out), all three results are present in the returned BTreeMap.
**Status**: pending

### Scenario 23: Dispatcher classifies terminal state
**ID**: scenario-23
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next.rs`)
**Expected**: `dispatch_next()` with a terminal `TemplateState` returns `NextResponse::Terminal` with the correct state name and `advanced` flag.
**Status**: pending

### Scenario 24: Dispatcher classifies gate-blocked state
**ID**: scenario-24
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next.rs`)
**Expected**: `dispatch_next()` with non-passing gate results returns `NextResponse::GateBlocked` with all blocking conditions listed.
**Status**: pending

### Scenario 25: Dispatcher classifies evidence-required state
**ID**: scenario-25
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next.rs`)
**Expected**: `dispatch_next()` with a state that has `accepts` and no gates returns `NextResponse::EvidenceRequired` with a populated `expects` field.
**Status**: pending

### Scenario 26: Dispatcher classifies integration state
**ID**: scenario-26
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next.rs`)
**Expected**: `dispatch_next()` with a state that has an `integration` field (and it is available) returns `NextResponse::Integration`.
**Status**: pending

### Scenario 27: Dispatcher classifies integration-unavailable state
**ID**: scenario-27
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo test` (unit tests in `src/cli/next.rs`)
**Expected**: `dispatch_next()` with a state that has an `integration` field (and it is unavailable) returns `NextResponse::IntegrationUnavailable`.
**Status**: pending

### Scenario 28: --with-data and --to are mutually exclusive
**ID**: scenario-28
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `cargo build && ./target/debug/koto next --with-data '{}' --to some_state test-wf`
**Expected**: Exit code 2. Error message indicates the two flags cannot be used together.
**Status**: pending

### Scenario 29: --with-data payload size limit enforced
**ID**: scenario-29
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- Generate a 2MB JSON string and pass via `--with-data`
**Expected**: Exit code 2. Error message indicates payload exceeds size limit.
**Status**: pending

---

## Use-case Scenarios

### Scenario 30: Full koto next on terminal state returns done response
**ID**: scenario-30
**Testable after**: Issue 1, Issue 4
**Category**: automatable
**Commands**:
- Create a template YAML with two states: `start` (unconditional transition to `done`) and `done` (terminal)
- `koto init test-wf --template template.yaml`
- Advance to terminal state (via `--to` or event manipulation)
- `koto next test-wf`
**Expected**: JSON output has `"action": "done"`, `"state": "done"`, `"advanced": false`, `"error": null`. Exit code 0.
**Status**: pending

### Scenario 31: Full koto next on state with gates that fail returns gate_blocked
**ID**: scenario-31
**Testable after**: Issue 1, Issue 3, Issue 4
**Category**: automatable
**Commands**:
- Create a template with a state that has a command gate `exit 1`
- `koto init test-wf --template template.yaml`
- `koto next test-wf`
**Expected**: JSON output has `"action": "execute"`, `"blocking_conditions"` array with one entry showing `"status": "failed"`, `"type": "command"`, `"agent_actionable": false`. Exit code 1.
**Status**: pending

### Scenario 32: Full evidence submission flow via --with-data
**ID**: scenario-32
**Testable after**: Issue 1, Issue 2, Issue 4
**Category**: automatable
**Commands**:
- Create a template with a state that has an `accepts` block requiring a `decision` enum field with values `["proceed", "escalate"]` and two conditional transitions
- `koto init test-wf --template template.yaml`
- `koto next test-wf` (should return EvidenceRequired with expects schema)
- `koto next --with-data '{"decision": "proceed"}' test-wf` (should accept and advance)
**Expected**: First `koto next` returns `expects` with `event_type: "evidence_submitted"`, `fields` containing `decision` with `type: "enum"`, and `options` listing the two transition targets. Second `koto next --with-data` succeeds (exit 0) and the workflow state changes.
**Status**: pending

### Scenario 33: Invalid evidence submission returns structured error
**ID**: scenario-33
**Testable after**: Issue 1, Issue 2, Issue 4
**Category**: automatable
**Commands**:
- Create a template with a state requiring a `name` (string, required) field
- `koto init test-wf --template template.yaml`
- `koto next --with-data '{"name": 42, "extra": "field"}' test-wf`
**Expected**: Exit code 2. JSON error has `"code": "invalid_submission"` and `details` array with entries for the type mismatch on `name` and the unknown field `extra`.
**Status**: pending

### Scenario 34: Directed transition via --to advances state and returns new state info
**ID**: scenario-34
**Testable after**: Issue 1, Issue 4
**Category**: automatable
**Commands**:
- Create a template with states `start` -> `analyze` -> `done`
- `koto init test-wf --template template.yaml`
- `koto next --to analyze test-wf`
**Expected**: Exit code 0. Response describes the `analyze` state. No gate evaluation occurs. A `directed_transition` event is appended to the state log.
**Status**: pending

### Scenario 35: Agent-driven workflow loop using only koto next output
**ID**: scenario-35
**Testable after**: Issue 1, Issue 2, Issue 3, Issue 4
**Category**: automatable
**Commands**:
- Create a template with: `gather` (accepts `result: string`, gate `echo ok`), conditional transition to `review` or `done`
- `koto init test-wf --template template.yaml`
- `koto next test-wf` (read expects, learn what to submit)
- `koto next --with-data '{"result": "pass"}' test-wf` (submit evidence)
- `koto next test-wf` (check new state)
**Expected**: Each `koto next` response is self-describing. The agent never needs to read the template file. The `expects` schema tells the agent what fields to submit, what types they are, and what options route to which states. The workflow progresses from `gather` through to its next state based on evidence.
**Status**: pending

### Scenario 36: Gate timeout kills entire process group
**ID**: scenario-36
**Testable after**: Issue 3, Issue 4
**Category**: automatable
**Commands**:
- Create a template with a state whose gate is `sh -c "sleep 300 & sleep 300 & wait"` with timeout 2
- `koto init test-wf --template template.yaml`
- `koto next test-wf`
- After the command returns, verify no orphaned sleep processes remain
**Expected**: `koto next` returns within ~2 seconds (not 300). The response shows `blocking_conditions` with `"status": "timed_out"`. No orphaned `sleep 300` processes remain (process group was killed).
**Status**: pending

### Scenario 37: koto next on non-existent workflow returns error
**ID**: scenario-37
**Testable after**: Issue 4
**Category**: automatable
**Commands**:
- `koto next nonexistent-workflow`
**Expected**: Exit code 1. JSON error message indicates workflow not found. Uses the existing pre-dispatch error shape `{"error": "...", "command": "next"}`.
**Status**: pending
