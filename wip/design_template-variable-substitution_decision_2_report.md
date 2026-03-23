# Decision: Should substitution validate against declarations or values?

## Chosen

Option C: Compile-time validation of variable references.

During `koto template compile`, scan all directive text and gate command strings for `{{KEY}}` patterns. Reject any reference to a variable name not present in the template's `variables` block. At runtime, the substitution function only needs the resolved values map (`HashMap<String, String>`) since every reference is guaranteed to correspond to a declared variable.

## Confidence

High (90%).

## Rationale

The codebase already establishes a strong pattern of catching structural errors at compile time. The `CompiledTemplate::validate()` method in `src/template/types.rs` already validates transition targets against declared states, `when` fields against `accepts` declarations, and enum values against allowed lists. Variable reference validation fits this pattern exactly.

Key observations from the code:

1. **The compiler already has all the text.** `compile()` in `src/template/compile.rs` extracts directive content via `extract_directives()` and gate commands via `compile_gate()`. Both are available before the `CompiledTemplate` is built. Adding a scan for `{{KEY}}` patterns in these strings is straightforward.

2. **Init-time validation already handles user input errors.** The `WorkflowInitialized` event stores resolved variables. Init-time validation rejects unknown `--var` keys and enforces required variables. So the only source of an undefined `{{KEY}}` at runtime is a template authoring bug (typo), not a user input error.

3. **Template bugs should fail at compile time, not runtime.** A `{{TYPO}}` in a gate command for a state that's only reachable through a rare conditional path could go undetected for weeks. Compile-time validation catches it immediately when the template author runs `koto template compile`.

4. **Simplest runtime type.** The `Variables` type only needs to carry `HashMap<String, String>` (or the existing `HashMap<String, serde_json::Value>` from the event). No declarations need to be threaded to substitution call sites. The substitution function becomes a pure lookup-and-replace with no validation logic beyond "key exists in map" -- which is guaranteed to succeed for well-formed templates.

5. **Low compiler complexity.** The validation is a regex scan (`\{\{([A-Z_]+)\}\}` or similar) over directive strings and gate command strings, cross-referenced against `variables.keys()`. This is ~15 lines of code in the validation pass, comparable to the existing transition-target validation.

## Rejected Alternatives

**Option A: Validate against resolved values map only.** This works but provides worse diagnostics. The error "undefined variable reference: UNKNOWN" at runtime doesn't tell the template author whether UNKNOWN is a typo or was intentionally omitted. More importantly, the error surfaces late -- only when a workflow reaches the state containing the bad reference. Given that the compiler already validates every other structural reference (transition targets, when fields, enum values), leaving variable references as the one thing checked only at runtime would be inconsistent.

**Option B: Validate against template declarations at runtime.** This produces better error messages than Option A ("not declared in template" vs "was declared but not provided") but carries unnecessary complexity. It requires the `Variables` type to hold both the values map and the set of declared names, and it threads template metadata to every substitution call site. Since init-time validation already ensures every declared variable has a value (required or defaulted), the distinction between "not declared" and "declared but missing" can't actually occur at runtime. The extra diagnostic power is unreachable.

## Assumptions

1. The `{{KEY}}` syntax is stable and won't change to support nested expressions or filters. If it does, the compile-time scanner would need updating, but so would the runtime substituter.

2. Variable names follow a pattern matchable by a simple regex (uppercase letters, digits, underscores). The existing convention in test fixtures (`{{TASK}}`) confirms this.

3. Templates are always compiled before use. There's no path where a raw markdown template is loaded directly at runtime without going through `compile()`.

4. Gate commands are the primary injection-risk surface. Directives are consumed by agents as text, so an unresolved `{{TYPO}}` in a directive is less dangerous but still a bug worth catching early.
