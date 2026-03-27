# Decision 11: Content-aware gate types in templates

## Question

How do templates define content-aware gates that check koto's content store instead of the filesystem?

## Chosen: Option 3 -- Hybrid: built-in gate types + shell fallback

**Confidence: high**

## Rationale

The hybrid approach is the only option that satisfies all constraints simultaneously: backward compatibility with existing templates, clean template syntax for common operations, and full flexibility for edge cases.

Three facts from the codebase drive this decision:

**1. The engine architecture already supports it.** The `evaluate_gates` function in `src/gate.rs` dispatches on `gate.gate_type` -- it handles `"command"` gates and returns an error for unrecognized types. Adding new type branches (e.g., `"context-exists"`, `"context-matches"`) is a mechanical change. The closure-based evaluator in `advance_until_stop` (line 141 of `src/engine/advance.rs`) accepts any `Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, GateResult>`, so the caller can inject content-store-aware evaluation without modifying the engine loop.

**2. The Gate struct is ready for extension.** The `Gate` type in `src/template/types.rs` already has a `gate_type` field (serialized as `"type"`) and a `command` field with `skip_serializing_if = "String::is_empty"`. Built-in gate types would use `gate_type` for dispatch and add new optional fields (e.g., `key`, `pattern`) while ignoring `command`. The existing validation in `CompiledTemplate::validate()` rejects unknown gate types, so adding new recognized types is a controlled extension.

**3. Decision 8 established the content store model.** Content lives as files in `ctx/` within the session directory, with a manifest tracking metadata. Built-in gates can check the manifest's key list (for `context-exists`) or read a content file and match against a pattern (for `context-matches`) without spawning a shell process. This is faster, more portable, and doesn't depend on the `koto` binary being on PATH during gate evaluation.

The shell fallback remains essential. Templates that check things outside koto's control (CI status, git state, external service health) must use shell commands. Deprecating shell gates would break existing templates and force unnatural workarounds for legitimate use cases.

### Built-in gate types

Two gate types cover the majority of content-aware checks:

- **`context-exists`**: Passes when a key exists in the content store. Template syntax: `type: context-exists`, `key: plan.md`. Replaces `test -f {{SESSION_DIR}}/ctx/plan.md`.
- **`context-matches`**: Passes when a key's content matches a regex pattern. Template syntax: `type: context-matches`, `key: review.md`, `pattern: "## Approved"`. Covers cases where templates gate on content quality.

### Gate struct extension

The `Gate` struct gains two optional fields:

```rust
pub struct Gate {
    pub gate_type: String,
    pub command: String,     // used by "command" gates
    pub timeout: u32,        // used by "command" gates
    pub key: String,         // used by "context-exists", "context-matches"
    pub pattern: String,     // used by "context-matches"
}
```

All new fields use `#[serde(default, skip_serializing_if = "String::is_empty")]` to maintain backward compatibility. The validation logic enforces that each gate type has its required fields present.

### Evaluation changes

The `evaluate_gates` function adds match arms for the new types. For `context-exists`, it checks the manifest (or filesystem) for key presence. For `context-matches`, it reads the content file and applies a regex. Both return `GateResult::Passed` or `GateResult::Failed` without spawning processes.

The closure-based evaluator in the advance loop doesn't change. The gate evaluation closure already receives access to the session directory (via `working_dir`), and can be extended to also receive a `&dyn ContextStore` reference for built-in type evaluation.

## Assumptions

- The `ContextStore` trait (Decision 10) will have an `exists(&self, session: &str, key: &str) -> bool` method and a `get(&self, session: &str, key: &str) -> Result<Vec<u8>>` method that built-in gates can call.
- Content-match gates use Rust's `regex` crate, which is already in the dependency tree (via other crates). If not, this is a small addition.
- Gate evaluation has access to the session identity (to query the content store). This is already implicit: `working_dir` points to the session directory, and the content store resolves keys relative to it.
- All backends (local, cloud, git) will implement `ContextStore`, so built-in gates work identically regardless of backend. This satisfies PRD R6.

## Rejected

### Option 1: Built-in gate types only

Would break every existing template that uses shell command gates. The validation in `CompiledTemplate::validate()` currently rejects non-command gate types with an explicit error message directing users to accepts/when. Removing shell gate support eliminates the escape hatch for checks that don't involve koto's content store (CI status, git state, external services). The migration cost is not worth it when shell gates remain useful for a meaningful class of checks.

### Option 2: Shell fallback only

No engine changes, but forces templates to shell out for every content check. This means:
- Templates must assume `koto` is on PATH during gate evaluation, creating a circular dependency (koto evaluates gates that call koto).
- Shell process overhead for every gate check. Current templates already evaluate gates in a tight loop inside `advance_until_stop`.
- No portability improvement. Shell gates that call `koto ctx exists` still depend on a Unix shell, which limits cloud/serverless execution scenarios.
- Content checks become opaque string commands rather than structured declarations, making template validation impossible. The validator can't statically check that a shell command references a valid content key.

## Open questions

None. The implementation path is clear from the existing code structure.
