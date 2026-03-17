# Phase 3 Research: Serialization Mechanics

## Questions Investigated
1. How does the existing `Event` type implement custom serialization? Can we follow the same pattern?
2. What's the best approach for the `action` field (`execute` vs `done`)?
3. How to handle fields that should be `null` in some variants vs omitted in others?
4. Should `NextError` use a similar custom serialization pattern or can it use `#[serde(tag)]`?
5. What supporting types are needed?

## Findings

### 1. Event's custom serialization pattern

`Event` in `src/engine/types.rs` uses a manual `impl Serialize for Event` (line 98) with `serialize_map`. The pattern:

- `EventPayload` is `#[serde(untagged)]` -- each variant's fields serialize flat, with no discriminant wrapper.
- `Event` manually builds a 4-entry map: `seq`, `timestamp`, `type` (derived from `EventPayload::type_name()`), and `payload`.
- For deserialization, `Event` deserializes the whole thing as `serde_json::Value`, reads the `type` string, then dispatches to helper structs (`WorkflowInitializedPayload`, `TransitionedPayload`, etc.) for each variant.

This pattern works well when a discriminant field (`type`) is separate from the payload. For `NextResponse`, the situation is similar: the `action` field (`"execute"` or `"done"`) is a discriminant, and the remaining fields vary by variant.

**Verdict**: The `serialize_map` pattern is directly applicable to `NextResponse`. It gives full control over which fields appear and their order.

### 2. Strategy for the `action` field

The design specifies two values: `"execute"` (for EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable) and `"done"` (for Terminal).

Three options:

**Option A: `#[serde(tag = "action")]`** -- Won't work cleanly. Serde's internally-tagged enum uses the Rust variant name as the tag value. We'd need `#[serde(rename = "execute")]` on four different variants, which serde prohibits (duplicate tag values within the same enum).

**Option B: `#[serde(untagged)]` + manual action field** -- Each variant would need an `action` field. Fragile and repetitive.

**Option C: Custom `impl Serialize`** -- Match on the variant, write `"action": "execute"` or `"action": "done"`, then write variant-specific fields. This is what `Event` does and it's the right call here.

**Verdict**: Option C. Custom `impl Serialize for NextResponse` using `serialize_map`, exactly like `Event`.

### 3. Null vs omitted fields

The design doc shows these field presence patterns across variants:

| Field | EvidenceRequired | GateBlocked | Integration | IntegrationUnavailable | Terminal |
|-------|-----------------|-------------|-------------|----------------------|----------|
| action | "execute" | "execute" | "execute" | "execute" | "done" |
| state | present | present | present | present | present |
| directive | present | present | present | present | absent |
| advanced | present | present | present | present | present |
| expects | object | null | object | object | null |
| blocking_conditions | absent | array | absent | absent | absent |
| integration | absent | absent | object | object | absent |
| error | null | null | null | null | null |

Key observations:
- `expects` is explicitly `null` in GateBlocked and Terminal, not omitted. This is important -- agents can check `expects == null` to know no evidence submission is needed.
- `error` is explicitly `null` on all success variants. This lets agents check `error != null` as the single error detection path.
- `blocking_conditions` only appears on GateBlocked.
- `integration` only appears on Integration and IntegrationUnavailable.
- `directive` is absent only on Terminal.

With a custom `serialize_map`, this is straightforward. For each variant, we explicitly write the fields that should be present, using `serde_json::Value::Null` for fields that should be `null`:

```rust
impl Serialize for NextResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            NextResponse::EvidenceRequired { state, directive, advanced, expects } => {
                let mut map = serializer.serialize_map(Some(6))?;
                map.serialize_entry("action", "execute")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", expects)?;
                map.serialize_entry("error", &Option::<()>::None)?;
                map.end()
            }
            NextResponse::GateBlocked { state, directive, advanced, blocking_conditions } => {
                let mut map = serializer.serialize_map(Some(7))?;
                map.serialize_entry("action", "execute")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", &Option::<()>::None)?;
                map.serialize_entry("blocking_conditions", blocking_conditions)?;
                map.serialize_entry("error", &Option::<()>::None)?;
                map.end()
            }
            NextResponse::Terminal { state, advanced } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("action", "done")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", &Option::<()>::None)?;
                map.serialize_entry("error", &Option::<()>::None)?;
                map.end()
            }
            // ... Integration and IntegrationUnavailable similarly
        }
    }
}
```

The `&Option::<()>::None` pattern serializes to JSON `null`. Alternatively, `&serde_json::Value::Null` works if serde_json is already a dependency (it is).

### 4. NextError serialization

The error response has a different shape entirely:
```json
{
  "error": {
    "code": "invalid_submission",
    "message": "...",
    "details": [...]
  }
}
```

No `action`, `state`, or other fields -- just an `error` wrapper. Two options:

**Option A: Separate struct with derive.** A `NextErrorResponse` struct with a single `error` field containing a `NextError` struct:

```rust
#[derive(Serialize)]
struct NextErrorResponse {
    error: NextError,
}

#[derive(Serialize)]
struct NextError {
    code: NextErrorCode,
    message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    details: Vec<ErrorDetail>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum NextErrorCode {
    GateBlocked,
    InvalidSubmission,
    PreconditionFailed,
    IntegrationUnavailable,
    TerminalState,
    WorkflowNotInitialized,
}
```

**Option B: Unified enum** wrapping both success and error cases:

```rust
enum NextOutput {
    Success(NextResponse),
    Error(NextErrorResponse),
}
```

**Verdict**: Option A for the error types (plain derive, no custom serialize needed). For the top-level dispatch, a unified `NextOutput` enum with a custom Serialize impl is clean: success variants delegate to `NextResponse`'s custom impl, error wraps in `{"error": ...}`.

Actually, there's a simpler approach. Since the success path already writes `"error": null` in every variant, and the error path is just `{"error": {...}}`, the cleanest model is:

```rust
enum NextOutput {
    Ok(NextResponse),   // custom serialize includes "error": null
    Err(NextError),     // custom serialize wraps in {"error": {...}}
}
```

With `NextOutput` having its own custom Serialize that delegates to `NextResponse::serialize` for Ok, and writes `{"error": <NextError>}` for Err.

### 5. Supporting types needed

Derived from the design doc's JSON shapes and `src/template/types.rs`:

**ExpectsSchema** -- represents the `expects` object:
```rust
#[derive(Debug, Clone, Serialize)]
struct ExpectsSchema {
    event_type: String,                           // always "evidence_submitted"
    fields: BTreeMap<String, ExpectsFieldSchema>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<TransitionOption>,
}
```

This is derived from the template's `accepts` block (`FieldSchema` in `src/template/types.rs`, line 55) and the transition `when` conditions. The field mapping:
- Template `FieldSchema.field_type` -> `ExpectsFieldSchema.type` (renamed with `#[serde(rename)]`)
- Template `FieldSchema.values` -> `ExpectsFieldSchema.values`
- Template `FieldSchema.required` -> `ExpectsFieldSchema.required`
- Template `Transition.when` conditions -> `ExpectsSchema.options`

**ExpectsFieldSchema** -- output-facing version of `FieldSchema`:
```rust
#[derive(Debug, Clone, Serialize)]
struct ExpectsFieldSchema {
    #[serde(rename = "type")]
    field_type: String,
    required: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    values: Vec<String>,
}
```

Note: the template's `FieldSchema` uses `field_type` internally but the output JSON uses `"type"`. We need a separate output type with `#[serde(rename = "type")]`, or we reuse `FieldSchema` and add the rename attribute there. Since `FieldSchema` already serializes with `field_type` as the key name, a separate output-facing type is cleaner.

**TransitionOption** -- represents entries in the `options` array:
```rust
#[derive(Debug, Clone, Serialize)]
struct TransitionOption {
    target: String,
    when: BTreeMap<String, serde_json::Value>,
}
```

**BlockingCondition** -- represents gate evaluation results:
```rust
#[derive(Debug, Clone, Serialize)]
struct BlockingCondition {
    name: String,
    #[serde(rename = "type")]
    condition_type: String,
    agent_actionable: bool,
}
```

**IntegrationOutput** -- for successful integration invocation:
```rust
#[derive(Debug, Clone, Serialize)]
struct IntegrationOutput {
    name: String,
    output: serde_json::Value,
}
```

**IntegrationUnavailableMarker** -- for failed integration:
```rust
#[derive(Debug, Clone, Serialize)]
struct IntegrationUnavailableMarker {
    name: String,
    available: bool,  // always false
}
```

**ErrorDetail** -- structured error details:
```rust
#[derive(Debug, Clone, Serialize)]
struct ErrorDetail {
    field: String,
    reason: String,
}
```

## Implications for Design

1. **Custom Serialize is the right pattern.** The codebase already uses it for `Event`, and the `NextResponse` output contract has the same "discriminant field + variant-specific fields" shape that serde's built-in tagging can't express (multiple variants sharing the same tag value).

2. **No Deserialize needed for NextResponse.** This is CLI output only -- koto produces it, agents consume it. We only need `Serialize`. This simplifies things considerably compared to `Event`, which needs round-trip fidelity for state file parsing.

3. **Separate output types from template types.** The template's `FieldSchema` has `field_type` as its field name (matching the TOML/JSON storage format), but the CLI output uses `"type"`. Rather than adding conditional serialization to the template type, create a thin output-facing type (`ExpectsFieldSchema`) with the right `#[serde(rename)]`. The conversion from `FieldSchema` -> `ExpectsFieldSchema` is a simple map.

4. **`error: null` on every success variant is intentional.** The design wants agents to have a single, consistent check: `if response.error != null then handle_error()`. The custom serialize approach makes this trivial to include.

5. **Exit code is orthogonal to serialization.** The exit code (0/1/2/3) is determined by the CLI command handler, not by the serialization layer. The `NextOutput` enum can carry the exit code as metadata or the CLI layer can derive it from the variant.

## Surprises

1. **`Terminal` has no `directive` field.** All other `execute` variants include `directive`, but `Terminal` omits it entirely (not null, absent). This means the map size varies by variant, which serialize_map handles fine but is worth noting in tests.

2. **`error: null` appears in the Terminal variant too**, even though Terminal also has `expects: null`. The Terminal JSON is minimal: `{"action":"done","state":"...","advanced":true,"expects":null,"error":null}`. No directive, no blocking_conditions, no integration.

3. **The design says `expects` is `null` for GateBlocked**, even though gate-blocked states might have an `accepts` block (the agent just can't submit evidence yet because gates haven't passed). This is a deliberate choice -- the agent should focus on the blocking conditions, not the evidence schema. Worth documenting.

4. **`IntegrationUnavailable` still includes `expects`**, meaning the agent can fall back to manual evidence submission when the integration tool isn't available. This is the graceful degradation path from PRD R17.

5. **The template `FieldSchema` has a `description` field** (line 63 of `src/template/types.rs`) that doesn't appear in the CLI output `expects.fields` schema. Either it should be included (for agent-friendly output) or explicitly excluded. The design doc examples don't show it, so exclude it for now.

## Summary

The existing `Event` custom `Serialize` pattern in `src/engine/types.rs` maps directly to `NextResponse` -- both need a discriminant field (`type`/`action`) that doesn't align with serde's built-in tagging because multiple variants share the same tag value. A custom `impl Serialize` using `serialize_map` gives exact control over which fields appear, which are null, and which are absent per variant. No `Deserialize` is needed since this is output-only. Six supporting types are required (`ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption`, `BlockingCondition`, `IntegrationOutput`, `IntegrationUnavailableMarker`), plus error types that can use plain `#[derive(Serialize)]` since the error shape is uniform.
