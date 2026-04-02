# Security Review: gate-contract-compiler-validation

## Dimension Analysis

### External Artifact Handling
**Applies:** No

The compiler reads local YAML template files from disk. It downloads nothing, fetches nothing from a registry or network endpoint, and spawns no subprocesses. Input arrives entirely as author-controlled files on the local filesystem, deserialized through serde_json. There is no external artifact surface to audit here.

### Permission Scope
**Applies:** No

The compiler needs read access to the template YAML files the author points it at. It writes nothing to disk (output is a CompiledTemplate struct or an error). It does not open sockets, escalate privileges, modify system state, or touch files outside the template being compiled. The permission footprint is narrower than the existing `gate.rs` evaluator, which actually spawns shell subprocesses at runtime. This feature adds no new permissions to that baseline.

### Supply Chain or Dependency Trust
**Applies:** No

The new validation passes are pure Rust: pattern matching over `gate_type` string literals, iteration over serde_json maps, and simple dot-split path traversal. They introduce no new crate dependencies. The existing dependency chain (serde_json, anyhow) is unchanged. No new untrusted code paths enter the build.

### Data Exposure
**Applies:** No

The compiler processes template YAML files that the developer authored and has on disk. It produces a structured validation result — pass or error — in memory. Nothing is logged to a remote service, written to a file, or transmitted over a network. Gate schema definitions are compile-time constants in `types.rs`. No user credentials, environment variables, or secrets pass through this code path.

### Input Validation (resolve_gates_path)
**Applies:** Yes

`resolve_gates_path` (mirroring the existing `resolve_value` in `advance.rs`) splits a user-supplied path string on `.` and walks a nested serde_json map. Several properties limit the risk:

- **No execution**: the function only indexes into a JSON object; it calls no shell, evaluates no expression, and has no side effects.
- **No allocation amplification**: `str::split('.')` is lazy and produces at most `n+1` segments for a string of length `n`. serde_json map lookups are O(key length). Neither operation can trigger exponential behavior.
- **Memory safety**: Rust's ownership model prevents buffer overflows. Very long path strings or very long segment names produce linear memory use proportional to the input length — within serde_json's existing deserialization budget — and are already bounded by the YAML parser.
- **Null-safety**: `Option`-chaining via `?` means any missing segment returns `None` immediately, so deep or malformed paths cannot panic.

However, there is a design-level concern worth documenting: the design describes two separate validation passes that both walk `gates.*` paths — the structural segment-count check (pass 2) and the reachability evidence walk (pass 3). If the structural check rejects paths with segment counts other than 3, the evidence walker in pass 3 should only ever receive well-formed 3-segment paths. This ordering dependency must be enforced in the compiler: pass 3 must be gated on pass 2 succeeding. If they run independently or in the wrong order, a malformed path string could reach the evidence walker, which would silently return `None` and potentially produce a misleading reachability result rather than a clear error.

**Severity if ordering is not enforced:** Low. The worst outcome is a false-positive "dead-end" error or a missed reachability warning — incorrect compiler output, not a security vulnerability.

**Mitigation:** Ensure pass 3 is only entered after pass 2 returns clean. This is a correctness constraint that should be called out in the implementation spec and enforced at the function call boundary (e.g., pass 3 accepts only pre-validated gate paths, not raw `when` clause strings).

### Duplication Divergence Risk
**Applies:** Yes

The design introduces two pairs of duplicated logic:

**Pair 1: `gate_type_builtin_default()` in `types.rs` vs `built_in_default()` in `gate.rs`**

Both functions return the same hardcoded JSON defaults for the three gate types. They are currently identical. If a new gate type is added to `gate.rs` and the author forgets to update `types.rs`, the compiler's reachability check will use stale defaults — potentially reporting "no transition would fire" for a valid template that uses the new gate type's fields, or passing a template that should fail.

The divergence risk is manageable because:
- The set of gate types is small and changes infrequently.
- Both functions live in the same codebase, so a reviewer adding a gate type will likely see both.
- The failure mode is a compiler false-positive or false-negative, not a runtime security issue. A false-negative (compiler passes a bad template) is the more concerning case, but it only affects the authoring-time experience — the runtime engine uses its own evaluation path regardless.

**Severity:** Low. No user data, permissions, or execution paths are affected. The risk is incorrect compile-time feedback.

**Mitigation:** Add a compile-time or test-time assertion that both functions return the same value for each known gate type. A unit test calling both and comparing their output for each `GATE_TYPE_*` constant costs little and eliminates the drift risk.

**Pair 2: `resolve_gates_path()` in `types.rs` vs `resolve_value()` in `advance.rs`**

Both functions split a dot-path and walk a serde_json map. The runtime `resolve_value` function is the authoritative path-walker used when actually advancing workflow state. If the compiler's `resolve_gates_path` diverges — for example, if `resolve_value` is updated to handle escaped dots or array index notation and `resolve_gates_path` is not — the compiler could accept templates that behave differently at runtime, or reject templates that work fine.

Again, the current path format is simple (3-segment, no escaping, no arrays), so drift is unlikely in the near term. But the duplication creates a maintenance trap.

**Severity:** Low. Divergence produces incorrect compile-time validation, not runtime exploitation.

**Mitigation:** Extract the shared traversal logic into a single function (e.g., in `types.rs` or a shared `util` module) imported by both the compiler and the runtime engine. This eliminates the divergence risk entirely and is preferable to test-based synchronization. If extraction is deferred, add a cross-reference comment in both functions noting the duplication and the expectation that they remain in sync.

## Recommended Outcome

**OPTION 2 - Document considerations:**

Two items should be captured in the implementation spec or inline code comments:

1. **Pass ordering constraint**: The implementation must enforce that the `gates.*` path segment-count validation (pass 2) completes successfully before the reachability evidence walk (pass 3) processes any path strings. This prevents misleading reachability results from malformed paths. Document this ordering dependency in the compiler's validation pipeline.

2. **Duplication synchronization**: Both duplicated pairs (`gate_type_builtin_default` / `built_in_default` and `resolve_gates_path` / `resolve_value`) should be covered by cross-referencing comments and backed by unit tests that assert both return identical results for every known gate type and path. The preferred long-term resolution is to extract the shared logic into a single shared function.

## Summary

This design operates entirely within the local compile-time toolchain — no network access, no subprocess execution, no privilege escalation, no external data — which eliminates the highest-severity security dimensions. The residual risks are both Low severity and confined to compile-time correctness: a pass-ordering dependency that could produce misleading reachability errors if violated, and two pairs of duplicated logic whose divergence could cause the compiler to accept or reject templates incorrectly. Neither affects runtime security or user data. Both are straightforward to address through test coverage and code organization.
