# Advocate: Monolithic Handler

## Approach Description

A single `run_next()` function (approximately 200-300 lines) replaces the current `Command::Next` match arm in `src/cli/mod.rs`. The function flows top-to-bottom through all `koto next` logic:

1. Parse flags (`--with-data`, `--to`)
2. Load state file, derive current state + evidence via log replay
3. Load and verify compiled template
4. If `--with-data`: validate payload against `accepts` schema, append `evidence_submitted` event
5. If `--to`: validate target, append `directed_transition` event, return immediately
6. Advancement evaluation: check terminal, evaluate gates, check `when` conditions against evidence, decide whether to auto-advance or stop
7. Build the appropriate JSON output variant and print it
8. Exit with correct code

Helper functions are extracted only for genuinely reusable operations (gate command execution with timeout/process-group kill, evidence validation against `accepts` schema, `expects` field derivation from template). These are plain functions, not traits or trait objects. No new modules, no new abstractions beyond what's needed.

## Investigation

### Current handler structure

The existing `Command::Next` handler (lines 208-307 of `src/cli/mod.rs`) is already monolithic -- it's a 100-line block that loads state, loads template, looks up the current state, and prints JSON. It uses `exit_with_error()` / `exit_with_error_code()` for early-return error handling. The pattern is straightforward: load, validate, compute, print, exit.

The other handlers (`Init`, `Rewind`, `Workflows`) follow the same pattern. There is no shared handler infrastructure, no middleware, no trait-based dispatch. Each command is a self-contained block in the `run()` match.

### Event/state model

`src/engine/persistence.rs` provides the building blocks: `read_events()`, `derive_state_from_log()`, `derive_evidence()`, `derive_machine_state()`, `append_event()`. These are all plain functions operating on `Vec<Event>` and `Path`. The current `Next` handler already calls `read_events()`, `derive_machine_state()`, and loads the template.

The monolithic approach would call the same functions, plus:
- `derive_evidence()` for current-epoch evidence (already exists)
- `append_event()` for evidence submission and transitions (already exists, used by `Init` and `Rewind`)
- New: gate command execution (shell spawn, timeout, process group kill)
- New: evidence validation against `FieldSchema` (type checking, required field checking, enum value checking)
- New: `when` condition matching against accumulated evidence
- New: `expects` field derivation from template `accepts` + `when` blocks

### Template types

`src/template/types.rs` has all the types needed: `TemplateState.accepts` (`Option<BTreeMap<String, FieldSchema>>`), `Transition.when` (`Option<BTreeMap<String, Value>>`), `TemplateState.gates` (`BTreeMap<String, Gate>`), `TemplateState.integration` (`Option<String>`), `TemplateState.terminal` (`bool`). The `FieldSchema` type has `field_type`, `required`, `values`, `description`.

### What needs to be built

The monolithic handler needs these concrete pieces of logic:

1. **Flag parsing**: Add `--with-data` and `--to` to the `Next` variant in the `Command` enum (clap derives). Approximately 5 lines.

2. **Evidence validation**: Given a `serde_json::Value` (from `--with-data`) and an `accepts: BTreeMap<String, FieldSchema>`, check required fields are present, field types match, enum values are in the allowed set. ~40-60 lines as a helper function.

3. **When-condition matching**: Given accumulated evidence (`Vec<&Event>` from `derive_evidence()`) and a transition's `when` block, determine if the evidence satisfies the condition. ~30-40 lines.

4. **Gate evaluation**: Execute a shell command with timeout. Spawn a process group, wait with timeout, kill the group on timeout. ~40-60 lines as a helper function. This is the only piece that touches OS APIs (process groups, signals).

5. **Expects derivation**: Given a `TemplateState`, build the `expects` JSON object from `accepts` and `when` blocks. ~30-40 lines.

6. **Output variant construction**: Build the five JSON output variants based on stopping condition. ~40-60 lines of conditional JSON construction.

7. **Main flow**: The top-to-bottom orchestration that calls the above. ~80-100 lines.

Total new code: ~300-400 lines, concentrated in `src/cli/mod.rs` plus a couple of helper functions that could live in the same file or in `src/engine/`.

## Strengths

- **Matches existing codebase style**: Every command handler in `src/cli/mod.rs` is already monolithic. The `Init` handler (lines 125-206) is 80 lines of linear flow. The `Rewind` handler (lines 308-384) is 76 lines. Adding a 200-300 line `Next` handler follows the established pattern. There's no existing abstraction layer to integrate with or work around.

- **Full visibility of control flow**: The entire `koto next` state machine is visible in one function. When debugging "why did this agent get this output?", you read one function top to bottom. No trait dispatch, no dynamic configuration, no indirection. The five output variants and their triggering conditions are all in the same scope.

- **Minimal blast radius**: Changes are concentrated in `src/cli/mod.rs` and at most one or two new helper functions. No new modules, no new traits, no new crate-level abstractions. The existing `persistence.rs` and `types.rs` modules are consumed as-is. The risk of breaking existing functionality is low because we're extending, not restructuring.

- **Straightforward testing**: The helper functions (evidence validation, when-condition matching, gate execution) are pure functions that can be unit-tested directly. The main flow can be integration-tested via the CLI binary. No test infrastructure needed beyond what exists.

- **Fast to implement**: No design overhead for abstractions. Write the logic, write the tests. The existing `append_event()`, `derive_evidence()`, and `derive_machine_state()` functions handle the hard parts. The new code is mostly validation and JSON construction.

- **Naturally handles flag interactions**: The `--with-data` and `--to` flags interact with each other (mutually exclusive) and with the current state (e.g., `--to` on a terminal state is an error). In a monolithic handler, these interactions are explicit if/else branches. No need to compose separate middleware or chain handlers.

## Weaknesses

- **Handler length**: The `Next` handler will be 200-300 lines, significantly longer than any other handler. This isn't unreadable -- Rust's ownership and type system keeps the logic tight -- but it's a step change from the current 100-line handler. Code review requires holding more context.

- **Difficult to test the orchestration in isolation**: While individual helpers (validation, gate execution) are testable, the main flow -- "given this state file and these flags, produce this output" -- requires integration tests that set up state files, write templates, and invoke the handler. These tests exist for `Init` and `Rewind` already but are heavier than unit tests.

- **Gate execution couples CLI to OS**: The gate command execution (process groups, timeouts, signals) is inherently platform-specific. In a monolithic handler, this OS-coupled code lives adjacent to pure JSON construction logic. A helper function provides some separation, but the monolithic handler still orchestrates both.

- **Future extensibility requires editing the handler**: When `koto cancel` (#49) or new stopping conditions are added, the monolithic handler grows. Each new feature adds branches to the same function. Over time this could produce a handler that's difficult to reason about -- though koto's scope is narrow enough that this may never be a practical problem.

- **No reuse path for library consumers**: `pkg/` exposes a Go library (now Rust). If library consumers want the `koto next` logic without the CLI, they'd need to extract it from the handler. A monolithic handler optimizes for CLI use at the expense of library composability. However, the current `pkg/` API already delegates to different functions than the CLI uses, so this may be a non-issue.

## Deal-Breaker Risks

- **None identified.** The monolithic approach is the natural extension of the existing codebase. The handler complexity (~300 lines) is well within the range of readable Rust functions, especially with extracted helpers for gate execution and evidence validation. The five output variants are a finite, well-specified set. The flag interactions (`--with-data`, `--to`, neither) are a small combinatorial space. There's no scaling concern -- `koto next` has a fixed feature set defined by the design doc, and the scope boundary explicitly defers auto-advancement loops and integration runners to #49.

  The closest risk is that the handler grows unwieldy when #49 adds the auto-advancement loop (which chains through states until a stopping condition). But even then, the loop body is "evaluate current state, decide to advance or stop" -- the same logic the monolithic handler already contains, just repeated. Extracting the loop body into a `step()` function at that point is a natural refactoring, not a rewrite.

## Implementation Complexity

- **Files to modify**: 1 primary (`src/cli/mod.rs`), plus 1-2 new helper functions (could be in `src/cli/mod.rs`, `src/engine/`, or a new `src/cli/next.rs`)
- **New infrastructure**: No. Uses existing `persistence.rs` functions, existing `template/types.rs` types, existing error handling pattern.
- **Estimated scope**: Medium. ~300-400 lines of new code, ~50 lines of modified code (adding flags to `Command::Next`). 3-5 helper functions. Integration tests similar in structure to existing CLI tests.

## Summary

The monolithic handler is the natural fit for this codebase. Every existing command handler in `src/cli/mod.rs` follows the same pattern -- load, validate, compute, print -- and `koto next` is the same pattern with more branches. The approach minimizes blast radius (one file, no new abstractions), follows established conventions, and delivers full visibility of the control flow that agents depend on. Its main cost is a longer-than-average handler function, which is manageable given the finite scope of `koto next`'s feature set and mitigable by extracting helpers for gate execution, evidence validation, and expects derivation.
