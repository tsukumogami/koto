# Decision: How to handle the event type change?

## Chosen

**Option A: In-place type change** -- change `HashMap<String, serde_json::Value>` to `HashMap<String, String>` in both `EventPayload::WorkflowInitialized` and `WorkflowInitializedPayload`.

## Confidence

High (95%)

## Rationale

The field is completely unused today. Every call site passes `HashMap::new()`, and no state files in the repo (or in the wild) contain populated variables. The `#[serde(default)]` attribute on the field means existing state files with `"variables": {}` deserialize identically whether the value type is `serde_json::Value` or `String` -- an empty JSON object maps to an empty HashMap regardless of the value parameter.

An in-place change is the right call because:

1. **No migration needed.** There's nothing to migrate. Empty maps are type-agnostic in serde.
2. **The type system should reflect intent.** Variables are strings for template substitution. Making the type `HashMap<String, String>` communicates this clearly and prevents callers from accidentally stuffing structured data where a flat string is expected.
3. **Forward compatibility is preserved.** If typed variables are ever needed, the path is additive: introduce a new event type or a new field alongside the string map. Keeping `Value` "just in case" provides flexibility that actively works against type safety.

The one theoretical risk -- a state file with non-string variable values -- cannot exist because no code path has ever written one.

## Rejected Alternatives

**Option B: Keep Value, convert at API boundary.** Adds a conversion layer to paper over a type mismatch that doesn't exist in practice. The event type would lie about what it actually stores, and every consumer would need to know the real contract lives elsewhere. Unnecessary indirection.

**Option C: Custom deserializer.** Solves a problem (gracefully handling non-string values in state files) that cannot occur. The field has never been populated. Writing a custom deserializer for an edge case with zero probability is the definition of overengineering.

## Assumptions

- No external tools or forks have written state files with populated `variables` fields. This is safe to assume given the field was introduced with no write path.
- The `#[serde(default)]` attribute remains on the field, so omitted or empty `variables` in existing state files continue to deserialize without error.
- If structured/typed variables are needed in the future, they will be introduced as an additive schema change (new field or new event type) rather than widening this field back to `Value`.
