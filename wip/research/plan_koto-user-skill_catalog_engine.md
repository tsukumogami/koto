# Engine Output Contract Catalog

Ground-truth reference for implementers writing `response-shapes.md`.
All facts are derived directly from:
- `src/cli/next_types.rs` — struct definitions and `Serialize` impls
- `src/cli/next.rs` — `dispatch_next` dispatcher
- `src/cli/mod.rs` — `handle_next` I/O handler (visit-count logic, StopReason mapping)
- `src/engine/advance.rs` — `advance_until_stop` loop, `StopReason`, `resolve_transition`
- `src/engine/evidence.rs` — `validate_evidence`
- `src/engine/errors.rs` — `EngineError`

---

## 1. Canonical `action` Values

Six `action` string values exist. All are snake_case in JSON.

| `action` value          | Rust variant                      | Serialized by           |
|-------------------------|-----------------------------------|-------------------------|
| `"evidence_required"`   | `NextResponse::EvidenceRequired`  | custom `Serialize` impl |
| `"gate_blocked"`        | `NextResponse::GateBlocked`       | custom `Serialize` impl |
| `"integration"`         | `NextResponse::Integration`       | custom `Serialize` impl |
| `"integration_unavailable"` | `NextResponse::IntegrationUnavailable` | custom `Serialize` impl |
| `"done"`                | `NextResponse::Terminal`          | custom `Serialize` impl |
| `"confirm"`             | `NextResponse::ActionRequiresConfirmation` | custom `Serialize` impl |

No other values exist. The custom `Serialize` implementation writes the
`"action"` key first in every map.

---

## 2. Per-Action Field Presence Table

Legend: **always** = always present in the JSON object; **conditional** = key is
present only when a condition is met; **absent** = key is never written for this
action; **null** = key is always written, value is JSON `null`.

| Field                | `evidence_required` | `gate_blocked` | `integration` | `integration_unavailable` | `done` | `confirm` |
|----------------------|---------------------|----------------|---------------|---------------------------|--------|-----------|
| `action`             | always              | always         | always        | always                    | always | always    |
| `state`              | always              | always         | always        | always                    | always | always    |
| `directive`          | always              | always         | always        | always                    | absent | always    |
| `details`            | conditional         | conditional    | conditional   | conditional               | absent | conditional |
| `advanced`           | always              | always         | always        | always                    | always | always    |
| `expects`            | always (object)     | null           | always (object or null) | always (object or null) | null   | always (object or null) |
| `blocking_conditions`| always (array)      | always (array) | absent        | absent                    | absent | absent    |
| `integration`        | absent              | absent         | always        | always                    | absent | absent    |
| `action_output`      | absent              | absent         | absent        | absent                    | absent | always    |
| `error`              | null                | null           | null          | null                      | null   | null      |

**Notes on `expects` presence:**

- `evidence_required`: always a non-null `ExpectsSchema` object (may have
  empty `fields` if the state is an auto-advance candidate with no `accepts`
  block — see section 6).
- `gate_blocked`: always serialized as `null` (written as
  `map.serialize_entry("expects", &None::<()>)`).
- `integration` and `integration_unavailable`: `Option<ExpectsSchema>` —
  `null` when the state has no `accepts` block, object when it does.
- `done`: always serialized as `null`.
- `confirm`: `Option<ExpectsSchema>` — `null` when no `accepts` block,
  object when present.

**Notes on `directive` absence in `done`:**

The `Terminal` variant has no `directive` field in its Rust struct. The
serializer does not write the key. Consumers must not expect `directive` when
`action == "done"`.

---

## 3. Field Meanings

### `action` (string)
The type discriminant. Always the first key in the output object. Determines
which other keys are present.

### `state` (string)
The state name the engine stopped in. Matches the state identifier in the
template.

### `directive` (string)
The state's directive text, with template variables (`{{VAR}}`) and runtime
variables (`{{SESSION_DIR}}`, `{{SESSION_NAME}}`) already substituted.
Present on all actions except `done`.

### `details` (string | absent)
Extended guidance for the agent on its first visit to the state. Controlled
by the visit-count logic — see section 5.

### `advanced` (boolean)
`true` when the engine made at least one transition during this invocation.
`false` when the engine stopped in the state it was already in (no new
transitions were appended). Useful for detecting auto-advancement vs.
returning the same state.

### `expects` (object | null)
The evidence schema the agent should use to submit `--with-data`. `null`
on `gate_blocked` and `done`. See section 4 for the full schema.

### `blocking_conditions` (array)
Present only on `evidence_required` and `gate_blocked`. May be an empty
array `[]` on `evidence_required` when no gates failed. See section 4 for
the full item schema.

### `integration` (object)
Present on `integration` and `integration_unavailable`. See sub-types below.

**`integration` action:** `{ "name": string, "output": any }` — the
integration runner's name and its returned JSON value.

**`integration_unavailable` action:** `{ "name": string, "available": false }`
— the integration is declared in the template but no runner is configured.
`available` is always `false`.

### `action_output` (object)
Present only on `confirm`. Carries the output of the default action that
ran and requires user confirmation before the workflow advances.
Schema: `{ "command": string, "exit_code": integer, "stdout": string,
"stderr": string }`. `stdout` and `stderr` are truncated to 64 KB each.

### `error` (null)
Always serialized as `null` in successful responses. The key is always
present. Error responses use a different top-level shape: `{"error": {...}}`
— see section 8.

---

## 4. Sub-Object Schemas

### `expects` object

```
{
  "event_type": "evidence_submitted",
  "fields": {
    "<field_name>": {
      "type": string,        // "string" | "number" | "boolean" | "enum"
      "required": boolean,
      "values": [string]     // present only for "enum" type; omitted (not null) otherwise
    }
  },
  "options": [              // omitted (not null) when no conditional transitions exist
    {
      "target": string,     // target state name
      "when": {             // one or more field→value match conditions
        "<field_name>": <json_value>
      }
    }
  ]
}
```

**Serialization details:**

- `event_type` is always the string `"evidence_submitted"`.
- `fields` is a `BTreeMap` serialized as a JSON object. Keys are in sorted
  order (BTreeMap preserves insertion-order of `iter()`, which is
  lexicographic for BTreeMap).
- `values` is omitted entirely (not written as `null` or `[]`) when the
  field type is not `enum` or when the `enum` has no allowed values list.
  Specifically: `#[serde(skip_serializing_if = "Vec::is_empty")]`.
- `options` is omitted entirely when no transitions have `when` conditions.
  Same `skip_serializing_if = "Vec::is_empty"` attribute.
- Each `ExpectsFieldSchema` renames `field_type` → `"type"` via
  `#[serde(rename = "type")]`.

**Field type strings (the `"type"` key):**

| Value       | Accepted JSON types in `--with-data`          |
|-------------|-----------------------------------------------|
| `"string"`  | JSON string                                   |
| `"number"`  | JSON number (integer or float)                |
| `"boolean"` | JSON boolean                                  |
| `"enum"`    | JSON string matching one of `"values"` entries|

Unknown field types are rejected at evidence submission time with
`InvalidSubmission`.

### `blocking_conditions` array item

```
{
  "name": string,           // gate name (key in template `gates:` map)
  "type": string,           // gate type string from template gate definition;
                            //   falls back to "command" if gate not found in defs
  "status": string,         // "failed" | "timed_out" | "error"
  "agent_actionable": boolean, // true when koto overrides record can unblock this gate
  "output": any             // the structured gate result output (JSON object from gate runner)
}
```

**Serialization details:**

- `condition_type` in the Rust struct serializes as `"type"` via
  `#[serde(rename = "type")]`.
- Only non-passing gates appear in the array. Passed gates are filtered out
  before the array is built (`GateOutcome::Passed => return None`).
- `status` is one of three strings: `"failed"`, `"timed_out"`, `"error"`.
  There is no `"passed"` status — passed gates are absent.
- `agent_actionable` is `true` when the gate definition has either:
  - an instance-level `override_default` value, or
  - a built-in default for the gate's type (as determined by
    `built_in_default(&g.gate_type)`).
  It is `false` when neither condition holds (e.g., a plain `command`-type
  gate with no override).
- `output` is the raw JSON value returned by the gate runner. Its shape
  depends on the gate type. For `command`-type gates the typical shape is
  `{"exit_code": integer, "error": string}`.

---

## 5. `details` Field Behavior

`details` is a non-empty string taken from the template state's `details`
field. It is conditionally included based on visit count.

**Omitted entirely (key not written) in these cases:**

1. The template state has an empty `details` string. The key is never
   written — it is not serialized as `null` or `""`.
2. The state has a non-empty `details` string, but the visit count for that
   state is greater than 1 AND the `--full` flag was not passed.

**Included (key written, value is the details string) when:**

1. The template state has a non-empty `details` string, AND
2. Either `--full` was passed to `koto next`, OR the visit count for the
   current state is `<= 1` (first or only visit).

**Visit count definition:**

Visit count is derived from the persisted event log by `derive_visit_counts`.
It counts how many times each state name has appeared as `to` in
`Transitioned` events (or as the initial state). A state the engine is
currently stopped in for the first time has count 1. On repeat invocations
that return the same state without a new transition, the count stays at 1
until an actual `Transitioned` event records another visit.

**Summary:**

| State `details` | Visit count | `--full` | `details` in JSON |
|-----------------|-------------|----------|-------------------|
| empty           | any         | any      | absent            |
| non-empty       | 1           | any      | present           |
| non-empty       | > 1         | false    | absent            |
| non-empty       | > 1         | true     | present           |

`details` is absent on the `done` action regardless of the above — the
`Terminal` variant has no `details` field in its Rust struct.

---

## 6. The Three `evidence_required` Sub-Cases

The `"evidence_required"` action covers three distinct engine situations.
They share the same `action` value but differ in which fields are populated.

### Sub-case A: Pure evidence gate (no failed gates)

**Engine path:** `StopReason::EvidenceRequired { failed_gates: None }`

The state has an `accepts` block and conditional transitions, but the
current evidence doesn't match any condition. No gates are involved.

**Shape:**
```json
{
  "action": "evidence_required",
  "state": "...",
  "directive": "...",
  "advanced": false,
  "expects": { "event_type": "evidence_submitted", "fields": {...}, "options": [...] },
  "blocking_conditions": [],
  "error": null
}
```

`blocking_conditions` is an empty array `[]`.

### Sub-case B: Gates failed + accepts block (gate-plus-evidence fallback)

**Engine path:** `StopReason::EvidenceRequired { failed_gates: Some(...) }`

The state has both gates and an `accepts` block. One or more gates failed,
but the template also allows the agent to provide override evidence. The
engine falls through instead of returning `gate_blocked`.

**Shape:**
```json
{
  "action": "evidence_required",
  "state": "...",
  "directive": "...",
  "advanced": false,
  "expects": { "event_type": "evidence_submitted", "fields": {...}, "options": [...] },
  "blocking_conditions": [
    { "name": "...", "type": "...", "status": "failed", "agent_actionable": true/false, "output": {...} }
  ],
  "error": null
}
```

`blocking_conditions` is a non-empty array of the failed gates.

### Sub-case C: Auto-advance candidate (empty expects)

**Engine path:** `dispatch_next` fallback (step 6 in the dispatcher comment)
or `StopReason::SignalReceived` with no `accepts` block on the final state.

The state has no `accepts` block, no integration, no failed gates, and is
not terminal. The dispatcher returns `evidence_required` with an empty
`ExpectsSchema` as a signal that the state will auto-advance on the next
`koto next` call. In practice this shape is rarely seen by a human agent
because the advancement loop in `handle_next` auto-advances through such
states before returning.

**Shape:**
```json
{
  "action": "evidence_required",
  "state": "...",
  "directive": "...",
  "advanced": true,
  "expects": { "event_type": "evidence_submitted", "fields": {}, "options": [] },
  "blocking_conditions": [],
  "error": null
}
```

`expects.fields` is an empty object `{}`. `options` is omitted (empty
array is `skip_serializing_if`). The `"options"` key does NOT appear.

**Distinguishing the three sub-cases at runtime:**

| Sub-case | `blocking_conditions` length | `expects.fields` empty? |
|----------|------------------------------|-------------------------|
| A        | 0                            | no (has declared fields)|
| B        | > 0                          | no (has declared fields)|
| C        | 0                            | yes                     |

---

## 7. Fields Present on Some Actions but Not Others

| Field                | Actions where present               | Actions where absent or null     |
|----------------------|-------------------------------------|----------------------------------|
| `directive`          | all except `done`                   | `done`                           |
| `details`            | all except `done` (conditionally)   | `done` (struct has no field)     |
| `expects`            | all (but `null` on `gate_blocked` and `done`) | N/A — key always present  |
| `blocking_conditions`| `evidence_required`, `gate_blocked` | `integration`, `integration_unavailable`, `done`, `confirm` |
| `integration`        | `integration`, `integration_unavailable` | all others                  |
| `action_output`      | `confirm` only                      | all others                       |
| `error`              | all (always `null` on success)      | N/A — key always present         |

The field order in each JSON object matches the order `serialize_entry` calls
appear in the custom `Serialize` implementation:

1. `action`
2. `state`
3. `directive` (when present)
4. `details` (when present — conditionally inserted after `directive`)
5. `advanced`
6. `expects` (or `action_output` for `confirm`)
7. `blocking_conditions` (when present) / `integration` (when present)
8. `error`

---

## 8. JSON Serialization Details

### Field name casing

All field names in successful responses use **snake_case**:

- `action`, `state`, `directive`, `details`, `advanced`, `expects`,
  `blocking_conditions`, `error`
- Inside `expects`: `event_type`, `fields`, `options`
- Inside each `fields` entry: `type` (renamed from `field_type`), `required`,
  `values`
- Inside each `options` entry: `target`, `when`
- Inside each `blocking_conditions` item: `name`, `type` (renamed from
  `condition_type`), `status`, `agent_actionable`, `output`
- Inside `action_output`: `command`, `exit_code`, `stdout`, `stderr`
- Inside `integration`: `name`, `output` (or `name`, `available` for
  `integration_unavailable`)

No camelCase keys appear anywhere in successful `koto next` output.

### Error response shape

Error responses use a different envelope:

```json
{
  "error": {
    "code": "<snake_case_error_code>",
    "message": "<human-readable string>",
    "details": [
      { "field": "<field_name>", "reason": "<explanation>" }
    ]
  }
}
```

Error codes (`NextErrorCode`) serialize as `snake_case` via
`#[serde(rename_all = "snake_case")]`:

| Code string                  | Exit code | Meaning                                          |
|------------------------------|-----------|--------------------------------------------------|
| `"gate_blocked"`             | 1         | Gate check failed; retry when condition resolves |
| `"integration_unavailable"`  | 1         | Integration runner not configured                |
| `"concurrent_access"`        | 1         | Another `koto next` is already running           |
| `"invalid_submission"`       | 2         | Evidence failed validation (see `details`)       |
| `"precondition_failed"`      | 2         | Caller violated a precondition                   |
| `"terminal_state"`           | 2         | Workflow is already in a terminal state          |
| `"workflow_not_initialized"` | 2         | Named workflow does not exist                    |
| `"template_error"`           | 3         | Template parsing, hash mismatch, or cycle        |
| `"persistence_error"`        | 3         | State file I/O or corruption                     |

`details` in the error object is always an array (may be empty `[]`).
It is populated with per-field errors only for `invalid_submission`.

### `null` encoding

`error: null` is written by serializing `&None::<()>`. Rust's serde
renders `Option::None` as JSON `null` in all cases.

`expects: null` on `gate_blocked` and `done` is the same mechanism:
`map.serialize_entry("expects", &None::<()>)`.

### `BTreeMap` key ordering

`expects.fields` and `expects.options[].when` use `BTreeMap`, so their
keys appear in lexicographic order in the JSON output. This is deterministic
but implementers should not rely on it for semantic purposes.

---

## 9. Summary of `advance_until_stop` Stopping Conditions

This table maps each `StopReason` to the `action` value it produces and any
conditions that gate the mapping.

| `StopReason`                  | `action` in JSON              | Notes                                              |
|-------------------------------|-------------------------------|----------------------------------------------------|
| `Terminal`                    | `"done"`                      |                                                    |
| `GateBlocked`                 | `"gate_blocked"`              | State has no `accepts` block AND no `gates.*` routing |
| `EvidenceRequired { None }`   | `"evidence_required"`         | Sub-case A: no failed gates                        |
| `EvidenceRequired { Some }` | `"evidence_required"`         | Sub-case B: failed gates present                   |
| `Integration`                 | `"integration"`               | Integration runner returned successfully           |
| `IntegrationUnavailable`      | `"integration_unavailable"`   | Runner returned `Unavailable` or `Failed`          |
| `ActionRequiresConfirmation`  | `"confirm"`                   | Action ran with `requires_confirmation: true`      |
| `SignalReceived`              | `"evidence_required"` or `"done"` | SIGTERM/SIGINT; shape depends on final state  |
| `UnresolvableTransition`      | error `"template_error"`      | Not a successful response                          |
| `CycleDetected`               | error `"template_error"`      | Not a successful response                          |
| `ChainLimitReached`           | error `"template_error"`      | Not a successful response                          |
| `AmbiguousTransition`         | error `"template_error"`      | `AdvanceError`, not `StopReason`                   |
| `DeadEndState`                | error `"template_error"`      | `AdvanceError`, not `StopReason`                   |
