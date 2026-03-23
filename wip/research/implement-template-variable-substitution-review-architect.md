# Architect Review: Issue 1 -- Variables substitution module and compile-time validation

**Verdict: request-changes**

One blocking finding (dependency direction), remainder is clean.

## Findings

### 1. Dependency direction violation: template imports engine (Blocking)

`src/template/types.rs:5` imports `crate::engine::substitute::extract_refs`.

The existing dependency graph has `engine` depending on `template` (e.g., `engine/advance.rs` imports `template::types::CompiledTemplate`, `template::types::TemplateState`). Adding `template -> engine` creates a **circular dependency** between the two modules. Rust's module system allows this within a single crate, but it violates the layered architecture where template is the lower-level structural type layer and engine is the higher-level runtime layer. Every other cross-module import confirms this direction: `engine` reads from `template`, `cli` reads from both, and `template` imports from neither.

**Fix:** Move `extract_refs` (and the `REF_PATTERN` constant it uses) out of `src/engine/substitute.rs` and into a location that `template` can import without reaching up into `engine`. Two options:

- **Option A**: Move `extract_refs` and `REF_PATTERN` into `src/template/types.rs` (or a new `src/template/variables.rs`). Then `engine/substitute.rs` imports the ref-extraction function from `template`, matching the existing dependency direction. The engine module already imports from template; this adds no new edge.
- **Option B**: Create a shared utility module (e.g., `src/variable_refs.rs`) at the crate root that both `template` and `engine` can import. This avoids coupling but adds a file for two items.

Option A is simpler and follows the existing pattern. The `extract_refs` function is a pure regex scan over template strings -- it belongs with template validation, not with the runtime `Variables` type.

### 2. Regex recompilation on every call (Advisory)

`substitute()` (line 63), `validate_value()` (line 95), and `extract_refs()` (line 112) each compile their regex on every invocation. For `substitute()`, this happens per-gate and per-directive during `koto next`. Not a structural concern -- it won't cause other code to copy a bad pattern -- but worth noting for Issue 3 when these get called in a loop. A `LazyLock<Regex>` or `OnceLock` would match the idiomatic Rust pattern. Not blocking.

### 3. SubstitutionError doesn't implement std::error::Error (Advisory)

`SubstitutionError` implements `Display` but not `std::error::Error`. The crate uses `thiserror` and `anyhow` elsewhere. When Issue 3 wires `from_events()` into `handle_next`, the error will need to propagate through `anyhow::Result`. Without `impl Error`, callers can't use `?` with anyhow context. This is contained to the substitute module and easy to fix in Issue 3, so not blocking now, but worth flagging for downstream.

## What fits well

- **Module placement**: `substitute.rs` in `engine/` is correct. The `Variables` type operates on engine-level `Event` types and provides runtime substitution. It belongs in the engine layer. (The exception is `extract_refs`, which is a template-level concern that should live in `template/`.)
- **Type narrowing**: `HashMap<String, serde_json::Value>` to `HashMap<String, String>` in both the enum variant and the deserialization helper struct. Both locations updated consistently. `#[serde(default)]` preserved. Clean change.
- **Compile-time validation**: Added to `CompiledTemplate::validate()` alongside existing structural checks (transition targets, when fields, enum values). Follows the established pattern exactly -- no parallel validation path.
- **API surface**: `Variables::from_events()` and `substitute()` match the design. The `validate_value()` and `extract_refs()` exports are correctly scoped for reuse by Issues 2 and 3.
- **Test coverage**: Substitution edge cases (single-pass, unclosed braces, lowercase passthrough, undefined panic), re-validation in `from_events`, and compile-time rejection of undeclared refs are all covered. The compile-time tests in `template/types.rs` cover both directive and gate command paths.
- **No Cargo.toml changes needed**: `regex` was already a dependency. The diff description mentioned adding it, but it was already present.
