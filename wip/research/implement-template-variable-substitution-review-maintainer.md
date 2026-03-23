# Maintainer Review: Issue 1 -- Variables substitution module and compile-time validation

**Verdict: approve** with advisory findings.

The code is well-structured and readable. Names match behavior, the module boundary is clean, tests document the contract, and the design rationale flows through into the implementation. A developer picking this up for Issue 2 or 3 will understand what's here and how to use it.

## Findings

### 1. Regex recompiled on every call -- invisible cost, confusing to the next optimizer

`src/engine/substitute.rs:63`, `:95`, `:112`

`Regex::new()` is called inside `substitute()`, `validate_value()`, and `extract_refs()`. The regex is compiled from a constant each time. A future developer profiling gate evaluation (which runs `substitute()` per gate per advance cycle) will wonder whether to cache it, and whether the constants are guaranteed to produce identical compiled regexes.

Use `std::sync::LazyLock` (stable since Rust 1.80) to compile each regex once:

```rust
use std::sync::LazyLock;
static VALUE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(VALUE_PATTERN).unwrap());
static REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(REF_PATTERN).unwrap());
```

This removes the `.expect()` calls (compile-time proof instead of runtime assertion) and makes it obvious to the next developer that regex compilation isn't a hot-path concern.

**Advisory.** The current call volume is low enough that this won't cause a bug, but the repeated `Regex::new` + `.expect()` pattern will confuse someone into thinking these might fail or be expensive.

### 2. SubstitutionError doesn't implement std::error::Error

`src/engine/substitute.rs:20-25`

`SubstitutionError` implements `Display` manually but doesn't implement `std::error::Error`. The rest of the codebase uses `thiserror::Error` (see `src/engine/errors.rs`). When Issue 3 wires this into `handle_next`, the developer will need to propagate `SubstitutionError` through `anyhow::Result` or convert it to `EngineError`. Without the `Error` trait impl, `?` won't work with `anyhow`, and they'll need to add the impl themselves or wrap it -- a detour they'll blame on this module.

Add `#[derive(thiserror::Error)]` or a manual `impl std::error::Error for SubstitutionError {}`. The crate already depends on `thiserror`.

**Advisory.** Issue 3 will hit this immediately, but it's a 1-line fix, not a misread.

### 3. Duplicated error message format string in compile-time validation

`src/template/types.rs:179-184` and `:189-194`

The directive and gate-command validation blocks use an identical format string:

```
"state '{}': variable reference '{{{{{}}}}}' is not declared in the template's variables block"
```

The quadruple-brace escaping (`{{{{{}}}}}`) is already hard to read. Duplicating it means the next developer who changes the wording will update one and miss the other. Extract a helper:

```rust
fn undeclared_var_error(state_name: &str, ref_name: &str) -> String {
    format!(
        "state '{}': variable reference '{{{{{}}}}}' is not declared in the template's variables block",
        state_name, ref_name
    )
}
```

**Advisory.** The strings are identical today and the two blocks are 10 lines apart, so divergence risk is low. But the brace escaping makes visual diffing unreliable.

### 4. validate_value is exported but the VALUE_PATTERN constant is not

`src/engine/substitute.rs:8,94`

`validate_value` is `pub` with a comment saying it's "exported for reuse by `koto init` validation (Issue 2)." But `VALUE_PATTERN` is a private `const`. If Issue 2 needs to show the user which characters are allowed (e.g., in a `--help` string or error message), they'll either duplicate the regex or reach into this module's internals. Consider making `VALUE_PATTERN` public, or adding a public constant like `ALLOWED_VALUE_CHARS` that describes the character set in human-readable form.

**Advisory.** Issue 2 might not need it -- the error message from `validate_value` already includes the pattern. But the asymmetry (public function, private constant it depends on) will make someone wonder if it was intentional.

### 5. No test for the gate-name in compile-time error message

`src/template/types.rs:186-196`

The gate command validation loop iterates over `state.gates.values()` but the error message only includes the state name, not the gate name. For a state with multiple gates, the developer debugging a template error won't know which gate has the bad reference. The directive validation has the same omission but there's only one directive per state, so it's fine there.

Compare with the existing gate validation at line 139 which includes both `state_name` and `gate_name` in its error. The next developer will expect consistency.

**Advisory.** Single-gate states are the common case today, but the inconsistency with other gate errors (which do name the gate) will be confusing.

## What's clear

- `Variables` struct and its API surface are well-named. `from_events` + `substitute` is exactly the mental model the next developer needs.
- The panic in `substitute()` is well-documented with a doc comment explaining why it's a panic and not a Result. The `should_panic` test confirms the contract.
- The single-pass substitution test (`substitute_single_pass_no_reprocessing`) documents a critical security invariant. Good.
- The type narrowing from `serde_json::Value` to `String` is clean -- both the enum variant and the deserialization helper struct were updated consistently.
- Compile-time validation tests cover both the happy path and the rejection path for both directives and gate commands. The lowercase-passthrough test documents the regex boundary clearly.
