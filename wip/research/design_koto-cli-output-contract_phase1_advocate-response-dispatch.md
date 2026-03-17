# Advocate: Response Type Dispatch

## Approach Description

Model the five `koto next` output variants as a Rust enum (`NextResponse`) with typed fields per variant. A separate `NextError` enum represents the six structured error codes. A dispatcher function takes the current `MachineState`, `CompiledTemplate`, CLI flags (`--with-data`, `--to`), and derived evidence, then returns `Result<NextResponse, NextError>`. Each enum variant serializes to the correct JSON shape via serde. Exit codes are derived from the response/error variant, not computed separately.

Concretely:

```rust
enum NextResponse {
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

enum NextError {
    GateBlocked { message: String, details: Vec<ErrorDetail> },
    InvalidSubmission { message: String, details: Vec<ErrorDetail> },
    PreconditionFailed { message: String, details: Vec<ErrorDetail> },
    IntegrationUnavailable { message: String, details: Vec<ErrorDetail> },
    TerminalState { message: String },
    WorkflowNotInitialized { message: String },
}
```

The dispatcher is a pure function (no I/O beyond reading already-loaded state). Gate evaluation and event appending happen before the dispatcher is called; the dispatcher only classifies what happened.

## Investigation

### What I Read

- `src/cli/mod.rs` (lines 208-307): current `koto next` handler. It's ~100 lines of procedural code that reads state, loads template, verifies hash, looks up the current state in the template, and prints a flat JSON object with `state`, `directive`, `transitions`. No evidence handling, no gates, no advancement.

- `src/engine/types.rs`: Event taxonomy with six `EventPayload` variants (`WorkflowInitialized`, `Transitioned`, `EvidenceSubmitted`, `IntegrationInvoked`, `DirectedTransition`, `Rewound`). Custom Serialize/Deserialize on `Event` to flatten the payload. `MachineState` struct holds `current_state`, `template_path`, `template_hash`.

- `src/engine/persistence.rs`: `read_events`, `append_event`, `derive_state_from_log`, `derive_evidence`, `derive_machine_state`. The replay infrastructure is already built -- `derive_evidence` correctly implements the epoch boundary rule.

- `src/template/types.rs`: `TemplateState` has `directive`, `transitions` (with `when` conditions), `terminal`, `gates` (command gates), `accepts` (field schemas), `integration`. All the template-side data structures that the dispatcher would inspect are already defined and parsed.

- `src/engine/errors.rs`: `EngineError` enum with four variants. Uses thiserror for `Display` and `Error` derives.

- `docs/designs/DESIGN-unified-koto-next.md`: Strategic design specifying five output shapes and six error codes. The output schema uses an `action` field (`"execute"` or `"done"`) plus variant-specific fields.

- `docs/designs/DESIGN-koto-cli-output-contract.md`: Tactical design stub -- problem statement only, decision pending.

### How the Approach Fits

The codebase already uses typed enums for domain concepts. `EventPayload` is a six-variant enum with custom serialization. `EngineError` is a four-variant enum with thiserror. The response dispatch approach follows the same pattern -- each output shape becomes a variant with typed fields, and serde handles the JSON output.

The template types (`TemplateState`, `Transition`, `FieldSchema`, `Gate`) provide all the data needed to classify the response. The dispatcher would inspect:
- `template_state.terminal` -> `Terminal` variant
- `template_state.accepts` + evidence match -> `EvidenceRequired` variant
- `template_state.gates` + gate results -> `GateBlocked` variant
- `template_state.integration` + integration result -> `Integration` or `IntegrationUnavailable`

The existing persistence layer (`derive_evidence`, `derive_machine_state`) provides the inputs. The dispatcher sits between persistence (already built) and JSON output (currently ad-hoc `serde_json::json!` calls).

## Strengths

- **Compiler-enforced exhaustiveness**: Every `match` on `NextResponse` must handle all five variants. Adding a sixth output shape (e.g., a future "paused" state) produces compiler errors at every call site that needs updating. The current ad-hoc `serde_json::json!` approach has no such guarantee -- it's possible to produce an output missing required fields without any compile-time warning.

- **Serialization correctness by construction**: Each variant's fields are typed. An `EvidenceRequired` response cannot accidentally omit the `expects` field because it's a required struct field. The current code builds JSON manually with `serde_json::json!`, where a typo in a field name or a missing key is a silent runtime bug.

- **Testability without CLI integration**: The dispatcher is a pure function returning `Result<NextResponse, NextError>`. Unit tests can construct a `MachineState` + `CompiledTemplate` + flags, call the dispatcher, and assert on the returned variant. No need to spawn a process, parse stdout, or set up state files. This is a significant improvement over the current approach where the handler is a monolithic function inside `run()` that calls `exit_with_error` (which calls `std::process::exit`).

- **Natural exit code derivation**: Each variant maps to exactly one exit code. `impl NextResponse { fn exit_code(&self) -> i32 }` and `impl NextError { fn exit_code(&self) -> i32 }` are trivial matches. The current `exit_code_for_engine_error` function relies on downcasting `anyhow::Error` to `EngineError`, which is fragile -- a new error type that doesn't get a downcast arm silently falls through to exit code 1.

- **Alignment with existing patterns**: The codebase already models `EventPayload` as a six-variant enum with custom serialization. `NextResponse` follows the same convention. Developers reading the code see a consistent pattern: domain concepts are enums, serialization is derived or custom-implemented per variant.

- **Self-documenting output contract**: The enum definition _is_ the output contract. Any developer reading the code knows exactly what JSON shapes `koto next` can produce by reading the `NextResponse` definition. With `serde_json::json!` calls scattered through a handler, the output contract is implicit and must be inferred from code paths.

## Weaknesses

- **Serialization complexity for `action` field**: The strategic design uses an `action` field (`"execute"` or `"done"`) that doesn't map naturally to serde's enum tagging. The `Terminal` variant should produce `"action": "done"` while all others produce `"action": "execute"`. This requires custom `Serialize` implementation (like `Event` already does) rather than `#[derive(Serialize)]`. Not hard, but adds ~30 lines of manual serialization code.

- **JSON shape flattening**: The design spec shows some variants with `expects: null`, `integration: null`, and others omitting those fields entirely. Serde's default enum serialization doesn't handle this well -- you'd need `#[serde(skip_serializing_if = "Option::is_none")]` on optional fields within a flattened struct, or custom serialization. The five output shapes in the design doc don't map cleanly to serde's built-in `#[serde(tag = "...")` or `#[serde(untagged)]` strategies because the discriminant (`action`) has only two values but there are five shapes.

- **Error/response boundary ambiguity**: The design shows `gate_blocked` as both an error code and a stopping condition. A response where gates are blocking isn't necessarily an _error_ -- it's a normal output shape. The dispatcher must decide whether `GateBlocked` is a `NextResponse` variant or a `NextError` variant. The design doc puts it in both places (output has `blocking_conditions`, errors have `gate_blocked` code). This creates a modeling tension that the enum approach surfaces rather than hides.

- **Two enums for one output stream**: Both `NextResponse` and `NextError` produce JSON on stdout. The caller (CLI handler) must match on `Result<NextResponse, NextError>` and serialize either one. This is a minor cost -- it's two match arms -- but it means the output contract is split across two type definitions rather than one.

- **Upfront modeling cost**: Defining the five response variants + six error variants + supporting types (`ExpectsSchema`, `BlockingCondition`, `IntegrationOutput`, `ErrorDetail`) requires significant upfront type definition before any behavior is implemented. For a design that's still "Proposed" (decision pending), this could mean rework if the output schema changes during review.

## Deal-Breaker Risks

- **None identified.** The approach is standard Rust enum modeling. The codebase already demonstrates this pattern with `EventPayload` and `EngineError`. The serialization complexity is real but manageable -- the codebase already has a custom `Serialize` impl for `Event` that handles the same kind of type-discriminant flattening. The error/response boundary ambiguity is a design question, not an implementation blocker -- it needs to be resolved regardless of the implementation approach.

The only scenario where this approach could fail is if the output schema turns out to be highly dynamic (e.g., templates can add arbitrary top-level fields to the response). The current design doc shows a fixed set of five shapes, so this risk is low. If the schema did become dynamic, the enum approach would need a catch-all variant with `serde_json::Value` fields, which defeats the purpose of typed variants.

## Implementation Complexity

- **Files to modify**: 3-4
  - `src/cli/mod.rs`: Replace the current `Command::Next` handler (~100 lines) with dispatcher call + serialization (~30 lines). Add `NextResponse`, `NextError`, and supporting type definitions (new module or inline).
  - New file `src/cli/next.rs` (or `src/cli/response.rs`): `NextResponse` enum, `NextError` enum, `ExpectsSchema` struct, `BlockingCondition` struct, custom Serialize impls. Estimated ~200-250 lines.
  - `src/engine/errors.rs`: May need additional error variants or may be replaced by `NextError` for the CLI boundary.
  - Test file(s): Unit tests for the dispatcher function and serialization (~150-200 lines).

- **New infrastructure**: Yes -- the `NextResponse` and `NextError` enums, the `expects` derivation logic (computing the schema from template `accepts` + `when` blocks), and the dispatcher function. The gate evaluation logic (shell execution with timeout and process group kill) is also new but is orthogonal to the dispatch approach -- it's needed regardless.

- **Estimated scope**: Medium. The type definitions and serialization are ~250 lines. The dispatcher function (classify state properties into response variant) is ~100 lines. The `expects` derivation (template accepts + when -> schema) is ~80 lines. Tests add ~200 lines. Total new code: ~600-650 lines. The current handler shrinks from ~100 lines to ~30 (load state, call dispatcher, serialize, exit).

## Summary

Response Type Dispatch fits this codebase naturally -- the project already models domain concepts as typed enums with custom serialization (`EventPayload`, `EngineError`). Modeling the five output variants as `NextResponse` enum variants gives compile-time exhaustiveness checks, prevents field-omission bugs that plague ad-hoc JSON construction, and makes the output contract testable as a pure function without CLI integration. The main costs are custom serialization for the `action` field flattening and upfront type definitions before behavior is implemented. No deal-breaker risks were identified; the approach handles all five output shapes and six error codes specified in the design.
