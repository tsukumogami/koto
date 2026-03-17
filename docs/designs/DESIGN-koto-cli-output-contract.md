---
status: Proposed
spawned_from:
  issue: 48
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-unified-koto-next.md
problem: |
  koto's current `koto next` is a read-only stub that returns the current state's
  directive and transition targets. It can't submit evidence, evaluate gates, advance
  state, or perform directed transitions. Agents have no way to drive workflow forward
  through the CLI -- the command that was supposed to replace `koto transition` doesn't
  yet do anything beyond reading state.
decision: |
  Model the five koto next output variants as a NextResponse Rust enum with typed
  fields per variant, and the six error codes as a NextError enum. A pure dispatcher
  function classifies (current state + CLI flags + gate results) into the appropriate
  variant. Custom serde serialization produces the correct JSON shape per variant.
  The orchestration layer in cli/mod.rs stays simple -- load state, evaluate gates,
  call dispatcher, serialize, exit.
rationale: |
  The output contract is the core deliverable of this design. Modeling it as typed
  enums means the code is the contract -- NextResponse variants map 1:1 to JSON
  output shapes, and the compiler enforces exhaustiveness. The codebase already uses
  this pattern (EventPayload, EngineError), so it's a familiar convention, not a new
  abstraction. The monolithic handler alternative works but scatters the output
  contract across ad-hoc serde_json::json! calls where field omission bugs are silent.
  The pipeline approach solves a sequencing problem that doesn't exist -- stages are
  conditional and the auto-advancement loop in #49 breaks the linear model.
---

# DESIGN: koto CLI Output Contract

## Status

Proposed

## Upstream Design Reference

Parent: `docs/designs/DESIGN-unified-koto-next.md` (Phase 3: CLI Output Contract)

This tactical design implements the CLI output contract specified in the strategic
design's Phase 3. The strategic design defines the event-sourced state machine
architecture; this design specifies the exact JSON output schema, flag behavior,
gate evaluation, and advancement mechanics for `koto next`.

## Context and Problem Statement

`koto next` is the sole interface between agents and the workflow engine. After #45
(Rust CLI foundation), #46 (event log format), and #47 (template evidence routing),
the infrastructure exists to support evidence submission, conditional transitions,
and gate evaluation. But `koto next` itself remains a stub -- it reads state and
returns a directive, nothing more.

The problem has three parts:

1. **No evidence submission.** Templates can declare `accepts` blocks and `when`
   conditions, but there's no CLI mechanism to submit data. The `--with-data` flag
   doesn't exist yet.

2. **No state advancement.** `koto transition` was removed in #45. Its replacement
   (`koto next --to` for directed transitions, auto-advancement for conditional
   transitions) hasn't been implemented. Agents can read state but can't change it.

3. **No gate evaluation.** Command gates are declared in templates but never
   executed. States with gates always appear passable.

The output format also needs to become self-describing: agents should know from a
single `koto next` response what they can do next (submit evidence, wait for gates,
handle integration output) without external knowledge of the template structure.

## Decision Drivers

- **Agent autonomy**: agents must be able to drive workflows using only `koto next`
  output, without reading templates or state files directly
- **Correctness**: evidence validation, gate evaluation, and state transitions must
  be atomic and consistent with the event log model
- **Self-describing output**: the JSON response must tell the agent exactly what to
  do next, including what evidence fields to submit and what options are available
- **Error clarity**: structured errors with codes, not just messages, so agents can
  branch on failure type programmatically
- **Scope boundary**: auto-advancement loop, integration runner, `koto cancel`, and
  signal handling are deferred to #49 -- this design covers gate evaluation and
  single-step advancement only

## Considered Options

### Decision 1: Implementation architecture for koto next

**Context:** How to structure the `koto next` implementation -- the handler logic,
output formatting, error handling, and the relationship between CLI and engine layers.

**Chosen: Response type dispatch.**

The five output variants and six error codes are the core deliverable of this design.
Modeling them as `NextResponse` and `NextError` enums gives compile-time
exhaustiveness checks, prevents field-omission bugs in JSON output, and makes the
output contract testable as a pure function without CLI integration. The codebase
already uses this pattern -- `EventPayload` is a six-variant enum with custom
serialization, `EngineError` uses thiserror. The dispatcher function is a pure
function that takes loaded state and returns a typed result; the CLI handler just
serializes and exits.

*Alternative rejected: Monolithic handler.* A single `run_next()` function matches
the existing handler style and minimizes new abstractions. But it scatters the output
contract across ad-hoc `serde_json::json!` calls where field name typos and missing
keys are silent runtime bugs. As the function grows to 200-300 lines, the five output
variants and their triggering conditions become harder to trace. The approach works
but doesn't leverage Rust's type system for the thing that matters most -- output
correctness.

*Alternative rejected: Pipeline stages.* Breaking processing into stage functions
with a shared `NextContext` struct provides per-stage testability. But the stages are
conditional (skip gates if none, skip evidence if no `--with-data`), which undermines
the clean sequential model. The `NextContext` accumulates `Option<T>` fields where
later stages unwrap what earlier stages guaranteed -- a type-safety regression the
pipeline was supposed to improve. The auto-advancement loop in #49 further breaks
linearity. The approach adds ceremony without the compile-time guarantees that
response dispatch provides.

## Decision Outcome

`koto next` will be implemented using typed response enums with a pure dispatcher
function. The architecture has three layers:

1. **CLI handler** (`src/cli/mod.rs`): Parse flags, load state, call dispatcher,
   serialize response, exit with correct code. Simple and thin.

2. **Dispatcher** (new module): Pure function that takes loaded state, template,
   flags, and gate results. Returns `Result<NextResponse, NextError>`. No I/O.

3. **Response types** (new module): `NextResponse` enum (five variants),
   `NextError` enum (six error codes), `ExpectsSchema`, `BlockingCondition`, and
   supporting types. Custom serde serialization for the `action` field.

Key properties:
- Output contract is self-documenting: read `NextResponse` to know all possible outputs
- Compiler enforces exhaustiveness on every match
- Dispatcher testable without spawning processes or setting up state files
- Gate evaluation and evidence validation are helper functions called before dispatch
- Exit codes derived from response/error variant, not computed separately

## Solution Architecture

### Overview

`koto next` becomes a three-phase operation: load state + evaluate environment
(I/O), classify the result (pure), serialize and exit (I/O). The pure classifier
(dispatcher) is the core -- it takes pre-computed inputs and returns a typed response
enum that serializes to the correct JSON shape.

### Components

**1. CLI flag extensions** (`src/cli/mod.rs`)

Add two optional flags to the `Next` command variant:

```
--with-data <json>   Submit evidence as JSON (validated against accepts schema)
--to <target>        Directed transition to a named state
```

These are mutually exclusive. Using both is a caller error (exit code 2).

**2. Response types** (`src/cli/next_types.rs`)

```rust
pub enum NextResponse {
    EvidenceRequired {
        state: String,
        directive: String,
        advanced: bool,
        expects: ExpectsSchema,
    },
    GateBlocked {
        state: String,
        directive: String,
        advanced: bool,
        blocking_conditions: Vec<BlockingCondition>,
    },
    Integration {
        state: String,
        directive: String,
        advanced: bool,
        expects: Option<ExpectsSchema>,
        integration: IntegrationOutput,
    },
    IntegrationUnavailable {
        state: String,
        directive: String,
        advanced: bool,
        expects: Option<ExpectsSchema>,
        integration: IntegrationUnavailableMarker,
    },
    Terminal {
        state: String,
        advanced: bool,
    },
}
```

Custom `impl Serialize` using `serialize_map` (same pattern as `Event` in
`engine/types.rs`). Each variant writes its specific fields plus `"action"`
(`"execute"` or `"done"`) and `"error": null`. No `Deserialize` needed -- this
is output-only.

```rust
pub struct NextError {
    pub code: NextErrorCode,
    pub message: String,
    pub details: Vec<ErrorDetail>,
}

pub enum NextErrorCode {
    GateBlocked,
    InvalidSubmission,
    PreconditionFailed,
    IntegrationUnavailable,
    TerminalState,
    WorkflowNotInitialized,
}
```

Error serialization: `#[derive(Serialize)]` with `#[serde(rename_all = "snake_case")]`
on `NextErrorCode`. The CLI handler wraps errors in `{"error": {...}}`.

Exit code mapping:

| Error code | Exit code | Meaning |
|-----------|-----------|---------|
| `gate_blocked` | 1 | Transient -- gates may pass on retry |
| `invalid_submission` | 2 | Caller error -- bad evidence payload |
| `precondition_failed` | 2 | Caller error -- state doesn't accept evidence |
| `integration_unavailable` | 1 | Transient -- tool not accessible |
| `terminal_state` | 2 | Caller error -- workflow already done |
| `workflow_not_initialized` | 2 | Caller error -- no state file |
| _(success)_ | 0 | Normal response |
| _(config error)_ | 3 | Template missing, hash mismatch, corrupt state |

**3. Supporting types** (`src/cli/next_types.rs`)

```rust
pub struct ExpectsSchema {
    pub event_type: String,                            // always "evidence_submitted"
    pub fields: BTreeMap<String, ExpectsFieldSchema>,
    pub options: Vec<TransitionOption>,                 // omitted when empty
}

pub struct ExpectsFieldSchema {
    pub field_type: String,     // serializes as "type"
    pub required: bool,
    pub values: Vec<String>,    // omitted when empty
}

pub struct TransitionOption {
    pub target: String,
    pub when: BTreeMap<String, serde_json::Value>,
}

pub struct BlockingCondition {
    pub name: String,
    pub condition_type: String,  // serializes as "type", always "command"
    pub status: String,          // "failed", "timed_out", or "error"
    pub agent_actionable: bool,  // always false for command gates
}

pub struct IntegrationOutput {
    pub name: String,
    pub output: serde_json::Value,
}

pub struct IntegrationUnavailableMarker {
    pub name: String,
    pub available: bool,  // always false
}

pub struct ErrorDetail {
    pub field: String,
    pub reason: String,
}
```

**4. Gate evaluator** (`src/gate.rs`)

Evaluates command gates by spawning `sh -c "<command>"` in a new process group
with a configurable timeout (default 30s). Uses `wait-timeout` (already a
dependency) for timeout and `libc::setpgid`/`killpg` via `pre_exec` for process
group isolation.

```rust
pub enum GateResult {
    Passed,
    Failed { exit_code: i32 },
    TimedOut,
    Error { message: String },
}

pub fn evaluate_gates(
    gates: &BTreeMap<String, Gate>,
    working_dir: &Path,
) -> BTreeMap<String, GateResult>;
```

All gates are evaluated (AND semantics) -- no short-circuit on first failure,
because the output must list all blocking conditions. Gate `timeout` field of 0
means use the default (30 seconds).

**5. Evidence validator** (`src/engine/evidence.rs`)

Validates a `--with-data` JSON payload against the current state's `accepts`
schema:

- All required fields present
- Field types match (`string`, `number`, `boolean`, `enum`)
- Enum values are in the allowed set
- No unknown fields (strict validation)

Returns `Ok(())` or `Err(NextError)` with `InvalidSubmission` code and per-field
`ErrorDetail` entries.

**6. Expects derivation** (`src/cli/next_types.rs`)

Structural assembly from template data:

1. If state has no `accepts` block: `expects = None` (serializes as `null`)
2. If state has `accepts`:
   - `event_type` = `"evidence_submitted"` (constant)
   - `fields` = map each `FieldSchema` to `ExpectsFieldSchema` (rename
     `field_type` to `type`, carry `required` and `values`)
   - `options` = filter transitions to those with `when` conditions, serialize
     target + when map. Omit `options` entirely if no transitions have `when`.

**7. Dispatcher** (`src/cli/next.rs`)

Pure function that classifies the current state into a response variant:

```rust
pub fn dispatch_next(
    state: &str,
    template_state: &TemplateState,
    advanced: bool,
    gate_results: &BTreeMap<String, GateResult>,
    flags: &NextFlags,
) -> Result<NextResponse, NextError>;
```

Classification logic:
1. If terminal: return `Terminal`
2. If any gate failed: return `GateBlocked` with all failing conditions
3. If integration declared and available: return `Integration`
4. If integration declared and unavailable: return `IntegrationUnavailable`
5. If `accepts` block exists: return `EvidenceRequired` with derived `expects`
6. Otherwise: state auto-advances (handled by caller loop in #49)

### Key Interfaces

**CLI -> Dispatcher**: The handler loads state, evaluates gates, validates
evidence (if `--with-data`), appends events, then calls the dispatcher with
pre-computed results. The dispatcher never does I/O.

**Dispatcher -> Response types**: Returns `Result<NextResponse, NextError>`.
The CLI handler serializes whichever variant it gets and derives the exit code.

**Template -> Expects**: `derive_expects(&TemplateState) -> Option<ExpectsSchema>`
converts template declarations into the agent-facing schema.

**Gate evaluator -> Dispatcher**: `BTreeMap<String, GateResult>` passes
pre-evaluated gate outcomes. The dispatcher converts non-passing results into
`BlockingCondition` entries.

### Data Flow

```
Agent calls: koto next [--with-data <json>] [--to <target>] <name>
                                    |
                        +-----------+-----------+
                        |                       |
                  --with-data                 --to
                        |                       |
              validate against            validate target
              accepts schema              against transitions
                        |                       |
              append evidence_          append directed_
              submitted event           transition event
                        |                       |
                        |                  return immediately
                        |                  (no gate evaluation)
                        |
                   evaluate command gates
                                |
                   call dispatcher(state, template,
                        gates, flags)
                                |
                   serialize NextResponse/NextError
                                |
                   exit with derived code
```

`--to` directed transitions return immediately after appending the event. They do
not evaluate gates on the target state -- gate evaluation applies to the current
state during read-only or evidence submission calls. The `--to` path emits a
`directed_transition` event and reports the new state.

For this issue (#48), the flow is single-step: evaluate current state and return.
The auto-advancement loop (repeatedly advancing through states until a stopping
condition) is added in #49.

### Error handling layers

Two error paths coexist intentionally:

1. **Pre-dispatch I/O errors** (template load failure, hash mismatch, corrupt
   state file) use the existing `exit_with_error_code` / `anyhow` pattern with
   exit codes 1 or 3. These fire before the dispatcher is called.

2. **Domain errors** from the dispatcher use `NextError` with the six error codes
   and exit codes 1 or 2. These represent semantic failures (bad evidence, terminal
   state, gates blocked).

Both produce JSON on stdout. Pre-dispatch errors use the existing
`{"error": "<message>", "command": "next"}` shape. Domain errors use the new
structured `{"error": {"code": "...", "message": "...", "details": [...]}}` shape.

### Field Presence by Variant

| Field | EvidenceRequired | GateBlocked | Integration | IntegrationUnavail. | Terminal |
|-------|-----------------|-------------|-------------|---------------------|----------|
| action | "execute" | "execute" | "execute" | "execute" | "done" |
| state | yes | yes | yes | yes | yes |
| directive | yes | yes | yes | yes | no |
| advanced | yes | yes | yes | yes | yes |
| expects | object | null | object/null | object/null | null |
| blocking_conditions | no | array | no | no | no |
| integration | no | no | object | object | no |
| error | null | null | null | null | null |

"no" = field absent from JSON. "null" = field present with value `null`.

## Implementation Approach

### Phase 1: Response types and serialization

Define `NextResponse`, `NextError`, `ExpectsSchema`, and all supporting types in
`src/cli/next_types.rs`. Implement custom `Serialize` for `NextResponse`. Write
unit tests that assert serialized JSON matches the strategic design's examples
exactly.

Deliverables:
- `src/cli/next_types.rs` -- all response/error types with serde impls
- Unit tests for every variant's JSON output

### Phase 2: Evidence validation and expects derivation

Implement `validate_evidence()` and `derive_expects()`. Evidence validation checks
required fields, type matching, and enum value constraints against the template's
`accepts` schema. Expects derivation assembles the `ExpectsSchema` from template
data.

Deliverables:
- `validate_evidence()` function
- `derive_expects()` function
- Unit tests for validation edge cases (missing required, wrong type, unknown field)

### Phase 3: Gate evaluator

Implement `src/gate.rs` with process group spawning, timeout, and kill logic.
The evaluator takes a gate map and working directory, returns per-gate results.
Unix-only via `#[cfg(unix)]` and `libc` dependency.

Deliverables:
- `src/gate.rs` -- `GateResult` enum, `evaluate_gates()` function
- `libc` added to `Cargo.toml` as `[target.'cfg(unix)'.dependencies]`
- Integration tests with real shell commands (echo, sleep, false)

### Phase 4: Dispatcher and CLI integration

Implement the dispatcher in `src/cli/next.rs` and wire everything into the
`Command::Next` handler. Replace the current stub handler. Add `--with-data`
and `--to` flags via clap. The handler: loads state, validates evidence (if
submitted), appends events, evaluates gates, calls dispatcher, serializes, exits.

Deliverables:
- `src/cli/next.rs` -- dispatcher function
- Updated `Command::Next` in `src/cli/mod.rs` with new flags and handler
- Integration tests for the full `koto next` flow with all flag combinations

## Security Considerations

### Command Gate Execution

Gate evaluation executes arbitrary shell commands from templates. This is safe by
design when template sources are trusted (plugin-installed or committed to the
project repo via PR). The commands run with the user's full environment and
permissions -- no sandboxing.

Process group isolation (`setpgid`/`killpg`) ensures timeout kills reach child
processes, preventing zombie process accumulation. The 30-second default timeout
bounds resource consumption from hung commands.

If koto is extended to load templates from untrusted sources, gate commands would
need additional validation. This is out of scope for the current trusted-source
model.

### Evidence Validation

Strict validation against the `accepts` schema rejects unknown fields and
type mismatches. This prevents agents from injecting unexpected data into the
event log. Evidence payloads are persisted as-is in the JSONL state file after
validation.

Size limits on `--with-data` payloads are not specified in this design. Large
payloads could bloat the event log. A reasonable limit (e.g., 1MB) should be
enforced at the CLI level.

### State File Atomicity

Event appending uses the existing `append_event()` function which writes a
complete JSON line and calls `fsync`. Evidence submission and state transitions
each append one event atomically. A crash between two appends leaves the log in
a consistent state (the last complete line is the truth).

### Gate Command Literal Execution

Gate commands are literal strings from the template, never interpolated with
runtime values. Although the `variables` mechanism exists in templates (stored
in `workflow_initialized` events), variable values are not substituted into gate
command strings. If interpolation is added later, shell escaping must be applied
to prevent injection. This design does not introduce interpolation.

### Concurrent Access

The event log assumes single-writer semantics. `append_event` calls `fsync` but
does not acquire a file lock. Two simultaneous `koto next --with-data` calls
could both validate against the same state and both append events. This is
acceptable for the current single-agent model where one agent drives one
workflow. Multi-agent coordination would require file locking or an external
synchronization mechanism.

### Payload Size Enforcement

The `--with-data` payload must be size-limited at CLI argument parsing time,
before validation or event appending. A 1MB limit prevents both event log bloat
and memory exhaustion from pathologically large JSON payloads.

### Environment Inheritance

Gate commands inherit the full process environment. This is consistent with
standard developer tooling (make, npm scripts) but means environment variables
containing secrets (API keys, tokens from `.local.env` or shell profiles) are
accessible to gate commands. Template authors should be aware that gate commands
execute with the same environment as the `koto` process itself.

### Exit Code Information Leakage

Exit codes and structured error messages reveal workflow state to the calling
process. This is intentional -- agents need this information to operate. In
environments where workflow state is sensitive, state files should be
file-permission protected (which they already are, inheriting the user's umask).

## Consequences

### Positive

- **Compile-time output contract enforcement.** Adding a sixth output variant or
  changing a field on an existing variant produces compiler errors at every use
  site. The current ad-hoc JSON approach has no such guarantee.

- **Testable without I/O.** The dispatcher is a pure function. Unit tests can
  cover all five variants and six error codes without state files, templates, or
  shell commands.

- **Self-describing agent interface.** Agents get structured `expects` telling
  them exactly what to submit, including field types, required flags, and routing
  options. No template reading needed.

- **Gate evaluation isolated from logic.** Process spawning code lives in
  `src/gate.rs`, separate from the pure classification logic. Testing gates
  doesn't require testing the dispatcher and vice versa.

### Negative

- **Custom serialization overhead.** The `impl Serialize for NextResponse` is
  ~80 lines of manual `serialize_map` calls, one block per variant. Changes to
  the JSON output require editing both the enum definition and the serializer.

- **More types than the monolithic alternative.** Six supporting types
  (`ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption`, `BlockingCondition`,
  `IntegrationOutput`, `IntegrationUnavailableMarker`) plus two enums and two
  error types. This is more code than building JSON inline.

- **Unix-only gate evaluation.** Process group management via `libc` is
  Unix-specific. Non-Unix platforms get a compile error, not degraded behavior.
  This matches koto's current Unix-only target but limits future portability.

### Mitigations

- **Serialization overhead**: The custom serializer follows the established
  `Event` pattern. Developers familiar with that code can maintain this one.
  Serialization tests for every variant catch drift between types and output.

- **Type count**: All supporting types are in one file (`next_types.rs`).
  Each type is small (2-4 fields). The alternative -- remembering to include
  the right fields in scattered `json!()` calls -- is harder to maintain.

- **Unix-only gates**: `#[cfg(unix)]` on the gate module with a compile error
  on other platforms makes the constraint explicit. If portability becomes a
  requirement, the gate evaluator is isolated enough to add a platform
  abstraction without touching the dispatcher or response types.
