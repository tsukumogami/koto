# Advocate: Pipeline Stages

## Approach Description

Break `koto next` processing into a sequence of composable stages, each implemented as a standalone function that transforms a shared context struct. The pipeline is not a framework or trait-based abstraction -- it's a linear chain of function calls in the `Command::Next` match arm, with a `NextContext` struct threaded through.

Concretely:

```rust
struct NextContext {
    // Populated by stage 1 (parse flags)
    workflow_name: String,
    with_data: Option<serde_json::Value>,
    to_target: Option<String>,

    // Populated by stage 2 (load state)
    header: Option<StateFileHeader>,
    events: Option<Vec<Event>>,
    machine_state: Option<MachineState>,
    template: Option<CompiledTemplate>,
    template_state: Option<TemplateState>,

    // Populated by stage 3 (evaluate gates)
    gate_results: BTreeMap<String, GateResult>,

    // Populated by stage 4 (validate/submit evidence)
    evidence_accepted: bool,
    matched_transition: Option<String>,

    // Populated by stage 5 (advance state)
    advanced_to: Option<String>,
    new_template_state: Option<TemplateState>,
}
```

Each stage is a function like `fn load_state(ctx: &mut NextContext) -> Result<(), NextError>`, where `NextError` carries the structured error code and exit code. The top-level handler calls them in sequence:

```rust
let mut ctx = parse_flags(name, with_data, to_target)?;
load_state(&mut ctx)?;
evaluate_gates(&mut ctx)?;
validate_evidence(&mut ctx)?;
advance_state(&mut ctx)?;
format_output(&ctx)
```

If any stage returns an error, the handler converts it to JSON and exits with the appropriate code. The final `format_output` stage inspects the context to determine which of the five output variants to emit.

## Investigation

### Current handler structure (src/cli/mod.rs, lines 208-307)

The existing `Next` handler is a ~100-line flat function that:
1. Resolves the state file path and checks existence
2. Reads events with `read_events()`
3. Derives machine state with `derive_machine_state()`
4. Verifies template hash integrity
5. Loads and parses the compiled template
6. Looks up the current state in the template
7. Formats output (state, directive, transition targets)

This is already implicitly staged -- the code proceeds linearly through these concerns. But everything is inlined with early `exit_with_error` calls scattered throughout. There's no evidence handling, no gate evaluation, no advancement, and no output variant logic.

### What the new handler needs to do

From the design doc and problem context, `koto next` must:

1. **Parse flags**: `--with-data <json>` and `--to <target>` (new clap args)
2. **Load state**: read events, derive machine state, load template, verify hash (existing logic)
3. **Evaluate gates**: run command gates with 30s timeout, process group kill (new)
4. **Validate evidence**: parse `--with-data`, validate against `accepts` schema, match `when` conditions (new)
5. **Submit evidence**: append `EvidenceSubmitted` event to log (new)
6. **Advance state**: append `Transitioned` or `DirectedTransition` event (new)
7. **Format output**: emit one of five JSON output variants based on state (new complexity)
8. **Handle errors**: map to six error codes and three exit codes (new)

### How the types support this

The template types (`TemplateState`, `Transition`, `Gate`, `FieldSchema`) are well-structured for stage-based processing:

- `TemplateState.gates` (BTreeMap<String, Gate>) feeds directly into a gate evaluation stage
- `TemplateState.accepts` (Option<BTreeMap<String, FieldSchema>>) feeds evidence validation
- `Transition.when` (Option<BTreeMap<String, Value>>) feeds condition matching
- `TemplateState.terminal` determines the terminal output variant
- `TemplateState.integration` determines integration variants

The event types (`EvidenceSubmitted`, `Transitioned`, `DirectedTransition`) already exist for the write operations that `advance_state` needs.

The persistence layer (`append_event`, `derive_evidence`, `derive_state_from_log`) provides the building blocks. `derive_evidence` is already implemented but unused -- it was built for exactly this use case.

### How it fits the error model

The design requires six error codes mapping to three exit codes. A pipeline approach makes this clean: each stage knows which errors it can produce, and the `NextError` type can carry `(error_code: &str, exit_code: i32, message: String)`. The existing `exit_with_error_code` helper already supports per-error exit codes.

## Strengths

- **Matches the natural processing order**: The five output variants correspond to stopping conditions at different pipeline stages (terminal at load_state, gate-blocked at evaluate_gates, evidence-required at validate_evidence, integration at advance_state). The pipeline makes these decision points explicit rather than buried in nested conditionals.

- **Each stage is independently testable**: `evaluate_gates` can be tested without loading state files. `validate_evidence` can be tested with synthetic `NextContext` values. This is especially valuable for gate evaluation (which involves process spawning and timeouts) and evidence validation (which has complex matching logic).

- **Low abstraction overhead**: This isn't a plugin system or trait-based middleware. It's a struct and five functions. The Rust compiler will inline aggressively. No dynamic dispatch, no trait objects, no registration.

- **Incremental implementation path**: Stages can be built one at a time. Start with `parse_flags` and `load_state` (refactoring existing code), then add `evaluate_gates`, then `validate_evidence` and `advance_state`, then `format_output`. Each stage addition is a self-contained PR.

- **Context struct documents the data flow**: The `NextContext` fields are a schema for what each stage needs and produces. New contributors can read the struct definition to understand the full pipeline without tracing control flow.

- **Evidence already designed for epoch-based replay**: `derive_evidence` in persistence.rs (lines 235-265) returns evidence events for the current state epoch. The pipeline can feed these accumulated evidence events into the validation stage alongside new `--with-data` input, making the "evidence so far + new submission" logic a straightforward merge in the context struct.

## Weaknesses

- **Context struct accumulation**: The `NextContext` grows `Option<T>` fields for data that's only valid after certain stages. This is a mild type-safety regression -- stage 4 accessing `ctx.template` has to unwrap an Option even though stage 2 guarantees it's populated. Rust's type system could enforce this with per-stage types, but that adds complexity the pipeline approach is trying to avoid.

- **Not all stages are independent**: Gate evaluation (stage 3) is only relevant if the state has gates AND the transition is about to happen. Evidence validation (stage 4) is only needed if `--with-data` was provided. The pipeline runs stages sequentially, but some stages are conditional. This means stages need early-return logic ("if no gates, skip"), which slightly undermines the clean sequential model.

- **Auto-advancement loop complicates linearity**: The design calls for auto-advancement through states until a stopping condition. This means stages 3-6 might run multiple times in a loop, not just once. The pipeline is naturally single-pass; wrapping stages 3-6 in a loop is doable but makes the "pipeline" metaphor less clean. (Note: the design doc says auto-advancement is deferred to #49, so this is a future concern, not immediate.)

- **Shared mutable context**: All stages mutate the same `NextContext`. This is fine for sequential execution but would complicate any future parallelism (unlikely for CLI, but worth noting). More practically, it means stage ordering bugs are runtime failures, not compile-time errors.

## Deal-Breaker Risks

- **None identified.** The pipeline approach is a straightforward refactoring pattern that Rust handles well. The main risk would be if the processing required non-linear flow (e.g., stage 5 needs to jump back to stage 3), but the design spec describes a strictly linear process with one loop point (auto-advancement, deferred to #49). The existing codebase already uses this linear pattern implicitly -- this approach just makes it explicit and testable.

The closest thing to a risk is the auto-advancement loop in #49, but even that fits: the pipeline becomes `parse -> load -> loop { gates -> evidence -> advance -> check_stop } -> format`. The stages themselves stay composable; only the orchestration adds a loop.

## Implementation Complexity

- **Files to modify**: 2-3. `src/cli/mod.rs` for the handler refactor and new stage functions. Possibly a new `src/cli/next.rs` module if the stages grow large enough to warrant extraction. `src/engine/errors.rs` for new error variants.
- **New infrastructure**: No. The `NextContext` struct and stage functions are plain Rust. No new crates, no traits, no macros. Gate execution needs `std::process::Command` with timeout (stdlib only, or `nix` crate for process group kill on Unix).
- **Estimated scope**: Medium. The refactor of existing code into stages is small. The new functionality (gate evaluation with timeout/pgid kill, evidence validation against schema, when-condition matching, five output variants) is the real work, and that's the same regardless of architectural approach. The pipeline structure adds maybe 50-100 lines of scaffolding (context struct + orchestration) but saves more than that in testing infrastructure.

## Summary

Pipeline Stages is a low-ceremony approach that matches how `koto next` naturally processes requests: flags in, state loaded, gates checked, evidence validated, state advanced, output formatted. The context struct makes data flow explicit and each stage independently testable, which matters most for gate evaluation (process spawning with timeouts) and evidence validation (schema matching). The main cost is Option-heavy fields on the context struct, but that's a mild ergonomic tax, not a structural problem.
