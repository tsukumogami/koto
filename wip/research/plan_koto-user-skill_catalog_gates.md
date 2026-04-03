# Gate Type Catalog — Ground Truth for koto-author and koto-user Skills

Sources read: `src/gate.rs`, `src/template/types.rs`, `src/engine/advance.rs`,
`src/cli/next_types.rs`, `src/cli/overrides.rs`, and the functional test feature
files and fixture templates under `test/functional/`.

---

## 1. Gate Types

Three gate types are supported. The constant names in source are:

| Constant | String value |
|---|---|
| `GATE_TYPE_COMMAND` | `"command"` |
| `GATE_TYPE_CONTEXT_EXISTS` | `"context-exists"` |
| `GATE_TYPE_CONTEXT_MATCHES` | `"context-matches"` |

Any other value in a gate's `type` field is rejected at compile time with a D1
error ("unsupported gate type").

---

### 1.1 `command`

**What it evaluates:** Spawns a shell command in the workflow's working directory
with a configurable timeout. The process group is isolated so cleanup is reliable
on signals.

**Template fields:**

| Field | Required | Description |
|---|---|---|
| `type` | yes | `"command"` |
| `command` | yes (non-empty) | Shell command string. `{{VAR}}` variable references are substituted at runtime. |
| `timeout` | no | Timeout in seconds. `0` (the default) means use the engine default of 30 s. |
| `override_default` | no | See section 3. |

**Pass condition:** Exit code 0.

**Block conditions and outcomes:**

| Outcome | When | `exit_code` in output | `error` in output |
|---|---|---|---|
| `Passed` | Exit code == 0 | `0` | `""` |
| `Failed` | Exit code != 0 and != -1 | actual exit code (e.g. `1`, `42`, `127`) | `""` |
| `TimedOut` | Process did not finish within timeout (exit code reported as -1, stderr contains "timed out") | `-1` | `"timed_out"` |
| `Error` | Spawn error or OS error (exit code -1, stderr does not contain "timed out") | `-1` | error message string from OS |

Note: a nonexistent command causes the shell itself to exit 127, so the outcome
is `Failed` (not `Error`) with `exit_code: 127`.

---

### 1.2 `context-exists`

**What it evaluates:** Checks whether a named key exists in the session context
store. Context is written by agents via `koto context add` (or equivalent). Does
not inspect the content of the value.

**Template fields:**

| Field | Required | Description |
|---|---|---|
| `type` | yes | `"context-exists"` |
| `key` | yes (non-empty) | Context key to check (e.g. `"research/lead.md"`). |
| `override_default` | no | See section 3. |
| `command`, `timeout`, `pattern` | — | Ignored / must be absent. |

**Pass condition:** The key exists in the session context store.

**Block conditions and outcomes:**

| Outcome | When | `exists` in output | `error` in output |
|---|---|---|---|
| `Passed` | Key present in context store | `true` | `""` |
| `Failed` | Key absent from context store | `false` | `""` |
| `Error` | No context store or no session available at evaluation time | `false` | `"context-exists gate requires a context store and session"` |

---

### 1.3 `context-matches`

**What it evaluates:** Retrieves a context key and tests its UTF-8 content
against a regex pattern. The pattern is compiled and tested with Rust's `regex`
crate (RE2-compatible, no backreferences).

**Template fields:**

| Field | Required | Description |
|---|---|---|
| `type` | yes | `"context-matches"` |
| `key` | yes (non-empty) | Context key whose content is tested. |
| `pattern` | yes (non-empty, valid regex) | Regex pattern. Validated at compile time; invalid patterns cause a D1 error. |
| `override_default` | no | See section 3. |
| `command`, `timeout` | — | Ignored / must be absent. |

**Pass condition:** The key exists, its content is valid UTF-8, and the pattern
matches anywhere in the content (`is_match`, not full-string anchor).

**Block conditions and outcomes:**

| Outcome | When | `matches` in output | `error` in output |
|---|---|---|---|
| `Passed` | Key present, UTF-8, pattern matches | `true` | `""` |
| `Failed` | Key absent; or key present but content is not valid UTF-8; or pattern does not match | `false` | `""` |
| `Error` | No context store or session available | `false` | `"context-matches gate requires a context store and session"` |
| `Error` | Pattern is invalid regex (caught at runtime, not compile time — can only happen if template was not compiled) | `false` | `"invalid regex pattern: <detail>"` |

---

## 2. Output Fields in `blocking_conditions[].output`

These are the exact JSON field names and types as serialized. They appear inside
the `output` object of each element of `blocking_conditions` in the `koto next`
response.

### 2.1 `command` gate output schema

```json
{
  "exit_code": <number>,
  "error": <string>
}
```

| Field | JSON type | Semantics |
|---|---|---|
| `exit_code` | number (integer) | Process exit code. `0` = passed; positive = failed with that code; `-1` = timed out or spawn error. |
| `error` | string | `""` when the gate ran normally (pass or fail by exit code). `"timed_out"` on timeout. OS error message on spawn failure. Never null. |

All possible `output` values for `command`:

| Outcome | `exit_code` | `error` |
|---|---|---|
| Passed | `0` | `""` |
| Failed (any nonzero code) | e.g. `1`, `42`, `127` | `""` |
| TimedOut | `-1` | `"timed_out"` |
| Error (spawn/OS) | `-1` | error message |

### 2.2 `context-exists` gate output schema

```json
{
  "exists": <boolean>,
  "error": <string>
}
```

| Field | JSON type | Semantics |
|---|---|---|
| `exists` | boolean | `true` if the key was found; `false` otherwise (including error cases). |
| `error` | string | `""` on normal pass or fail. Error description when the context store is unavailable. Never null. |

All possible `output` values for `context-exists`:

| Outcome | `exists` | `error` |
|---|---|---|
| Passed | `true` | `""` |
| Failed (key absent) | `false` | `""` |
| Error (no store/session) | `false` | `"context-exists gate requires a context store and session"` |

### 2.3 `context-matches` gate output schema

```json
{
  "matches": <boolean>,
  "error": <string>
}
```

| Field | JSON type | Semantics |
|---|---|---|
| `matches` | boolean | `true` if the pattern matched; `false` otherwise (including error cases). |
| `error` | string | `""` on normal pass or fail. Error description when the store is unavailable or the pattern is invalid regex. Never null. |

All possible `output` values for `context-matches`:

| Outcome | `matches` | `error` |
|---|---|---|
| Passed | `true` | `""` |
| Failed (key absent, non-UTF-8, or no match) | `false` | `""` |
| Error (no store/session) | `false` | `"context-matches gate requires a context store and session"` |
| Error (invalid regex) | `false` | `"invalid regex pattern: <detail>"` |

---

## 3. `override_default` Field

### 3.1 What it is

An optional field on a gate declaration. When present, it supplies the default
value that `koto overrides record` will apply if neither `--with-data` nor a
higher-priority source is given.

It must be a JSON object whose shape exactly matches the gate type's output
schema. The compiler validates this at template compile time (D2 check):
- Must be a JSON object (not null, string, array, etc.).
- Must contain every field the gate type's schema requires (no missing fields).
- Must not contain any fields not in the gate type's schema (no extra fields).
- Each field's value must match the expected type for that field.

Violating any of these is a compile-time hard error regardless of `--allow-legacy-gates`.

### 3.2 Which gate types support it

All three gate types support `override_default`. The field is defined on the `Gate`
struct in `types.rs` and is not gated by type.

### 3.3 Valid shapes per gate type

`command`:
```json
{"exit_code": <number>, "error": <string>}
```

`context-exists`:
```json
{"exists": <boolean>, "error": <string>}
```

`context-matches`:
```json
{"matches": <boolean>, "error": <string>}
```

### 3.4 Three-tier resolution order for `koto overrides record`

Defined in `src/cli/overrides.rs` `resolve_override_applied()`. Highest priority
first:

1. **`--with-data <json>`** — explicit JSON supplied on the command line. Parsed
   and used as-is. No schema validation is applied by the resolver (the template
   compiler already validated the gate type at compile time).

2. **Gate-level `override_default`** — the value declared in the template's gate
   block. Used when `--with-data` is absent.

3. **Built-in default for the gate type** — the hardcoded per-type default:
   - `command`: `{"exit_code": 0, "error": ""}`
   - `context-exists`: `{"exists": true, "error": ""}`
   - `context-matches`: `{"matches": true, "error": ""}`

If none of the three tiers yields a value (unknown gate type, no `override_default`,
no built-in default), `koto overrides record` exits with code 2 and an error
message: "no override value available for gate (type '...'): provide --with-data
or set override_default on the gate".

**Important:** All three known gate types have built-in defaults, so `koto overrides record`
will always succeed for `command`, `context-exists`, and `context-matches` even
without `--with-data` or `override_default`. Custom/unknown gate types have no
built-in default and require `--with-data`.

---

## 4. `gates.<name>.<field>` Path Syntax in `when` Blocks

### 4.1 How it works

The `when` block of a transition is a map of `field_path -> expected_value`. When
the engine evaluates gates, it injects gate outputs under the reserved top-level
key `"gates"` in the evidence map. The transition resolver then does dot-path
traversal to check `when` conditions.

The full path format is exactly three dot-separated segments:

```
gates.<gate_name>.<field_name>
```

- `gates` — the reserved namespace. Agents cannot submit evidence with this key
  (the CLI rejects any `--with-data` payload containing a top-level `"gates"` key).
- `<gate_name>` — the name of the gate as declared in the state's `gates` block.
- `<field_name>` — one of the output fields for that gate type.

The compiler validates `gates.*` paths at compile time (D3 check):
- Must be exactly three segments.
- `<gate_name>` must be declared in the same state's `gates` block.
- `<field_name>` must be a valid field for the gate's type schema.

Values in `when` clauses must be JSON scalars (string, number, boolean). Arrays
and objects are rejected at compile time.

### 4.2 Available fields per gate type

`command`:
```yaml
when:
  gates.<name>.exit_code: 0       # number
  gates.<name>.error: ""          # string
```

`context-exists`:
```yaml
when:
  gates.<name>.exists: true       # boolean
  gates.<name>.error: ""          # string
```

`context-matches`:
```yaml
when:
  gates.<name>.matches: true      # boolean
  gates.<name>.error: ""          # string
```

### 4.3 Routing vs. blocking behavior

Whether a state with failing gates blocks or routes depends on whether any
transition's `when` clause references a `gates.*` key:

- **Structured mode** (at least one `when` clause references `gates.*`): Gate
  outputs are injected into the evidence map. The engine falls through to
  transition resolution even when gates fail, allowing `when` conditions to
  match on failure output (e.g. `gates.ci_check.exit_code: 1`). If a matching
  transition is found, the engine auto-advances. If no transition matches, the
  engine stops with `EvidenceRequired` (if the state has an `accepts` block) or
  `GateBlocked` (if it does not).

- **Legacy mode** (no `when` clause references `gates.*`): Gate output is NOT
  injected into the evidence map. If any gate fails, the engine returns
  `GateBlocked` immediately (unless the state has an `accepts` block, in which
  case it falls through to `EvidenceRequired` for evidence-based recovery).

### 4.4 Example: routing on exit code

```yaml
check:
  gates:
    ci_check:
      type: command
      command: "cargo test"
  transitions:
    - target: pass
      when:
        gates.ci_check.exit_code: 0
    - target: fix
      when:
        gates.ci_check.exit_code: 1
```

Both transitions fire without the agent needing to submit evidence. The engine
evaluates the gate, injects `gates.ci_check.exit_code` into the evidence map, and
resolves the transition automatically.

### 4.5 Mixed routing (gates.* and agent evidence in the same `when`)

A `when` clause may combine `gates.*` fields with agent evidence fields in the
same condition block. Both must match:

```yaml
transitions:
  - target: approved
    when:
      gates.lint.exit_code: 0
      decision: approve
```

This fires only when the gate returned exit code 0 AND the agent submitted
`{"decision": "approve"}`. States using mixed routing must declare an `accepts`
block for the agent evidence fields (compiler D3 rule 5).

---

## 5. `agent_actionable` Field

### 5.1 What it is

A boolean field on each element of `blocking_conditions`. Tells the consuming
agent whether it can call `koto overrides record` to unblock the gate without
waiting for the underlying condition to change.

### 5.2 How it is computed

Defined in `blocking_conditions_from_gates()` in `src/cli/next_types.rs`:

```rust
let agent_actionable = gate_defs
    .get(name)
    .map(|g| g.override_default.is_some() || built_in_default(&g.gate_type).is_some())
    .unwrap_or(false);
```

`agent_actionable` is `true` when **either**:
- The gate has an instance-level `override_default` declared in the template, **or**
- The gate type has a built-in default (i.e. the type is one of `command`,
  `context-exists`, or `context-matches`).

`agent_actionable` is `false` when:
- The gate definition is not found in the template state (defensive fallback), or
- The gate type is unknown and has neither `override_default` nor a built-in default.

**Practical effect for all three known gate types:** Since all of `command`,
`context-exists`, and `context-matches` have built-in defaults, `agent_actionable`
is always `true` for blocking conditions produced by these gate types. Template
authors can set `override_default` to customize the value used by `koto overrides
record`, but `agent_actionable` is already `true` regardless.

### 5.3 Per-gate-type summary

| Gate type | Built-in default exists | `agent_actionable` (no `override_default`) | `agent_actionable` (with `override_default`) |
|---|---|---|---|
| `command` | yes | `true` | `true` |
| `context-exists` | yes | `true` | `true` |
| `context-matches` | yes | `true` | `true` |
| unknown type | no | `false` | `true` (if `override_default` is set) |

---

## 6. `--allow-legacy-gates` Flag

### 6.1 What it suppresses

The D5 compile-time diagnostic. D5 fires when a state has one or more gates but
none of its transitions' `when` clauses reference any `gates.*` key.

D5 treats such states as "legacy" because they use the old boolean pass/block
behavior: all gates pass → auto-advance; any gate fails → `GateBlocked` (or
`EvidenceRequired` when an `accepts` block is present). No routing based on gate
output is possible.

### 6.2 Strict vs. permissive mode

The compiler's `validate(strict: bool)` method controls D5 behavior:

- **`strict = true`** (used by `koto template compile`): D5 is a hard error.
  The error message names the specific state and gate and hints at
  `--allow-legacy-gates`.
- **`strict = false`** (used by `koto init`): D5 emits a warning to stderr
  ("warning: state ... gate ... has no gates.* routing (legacy behavior)") and
  validation continues. The workflow is initialized successfully.

D4 (gate reachability check — warns about schema fields never referenced in any
`when` clause) is also suppressed entirely in permissive mode (`strict = false`).
D4 is a template-author warning, not a runtime concern.

### 6.3 CLI usage

```
koto template compile --allow-legacy-gates <template-path>
```

When `--allow-legacy-gates` is passed, the compiler runs with `strict = false`.
This is the only way to make `koto template compile` exit 0 on a legacy-gate
template.

`koto init` always uses `strict = false` and never requires the flag.

### 6.4 Legacy gate runtime behavior (when allowed)

When a legacy-gate template runs:

- Gates are evaluated normally.
- If all gates pass: gate outputs are **not** injected into the evidence map
  (because `has_gates_routing` is false), and the engine falls through to
  transition resolution using only agent-submitted evidence.
- If any gate fails and the state has no `accepts` block: returns `GateBlocked`
  immediately.
- If any gate fails and the state has an `accepts` block: falls through to
  `EvidenceRequired` so the agent can submit override or recovery evidence.

The functional test fixture `legacy-gates.md` demonstrates this: the `verify`
state has a `command` gate and an `accepts` block with agent-evidence-only
transitions, no `gates.*` references.

---

## 7. Gate-Type-Specific Behavior Visible in Test Fixtures

### 7.1 `command` gate — `structured-routing.md`

The canonical structured-routing fixture:

```yaml
check:
  gates:
    ci_check:
      type: command
      command: "test -f wip/flag.txt"
  transitions:
    - target: pass
      when:
        gates.ci_check.exit_code: 0
    - target: fix
      when:
        gates.ci_check.exit_code: 1
```

- When `wip/flag.txt` exists: `exit_code` is 0 → routes to `pass` (terminal).
  `koto next` returns `action: "done"`, `state: "pass"`, `advanced: true`.
- When `wip/flag.txt` is absent: `exit_code` is 1 → routes to `fix` (terminal).
  `koto next` returns `action: "done"`, `state: "fix"`, `advanced: true`.
- The `error` field is available for routing but is not used in either transition
  in this fixture.

### 7.2 `command` gate — `structured-gates.md` (gate always fails)

```yaml
check:
  gates:
    ci_check:
      type: command
      command: "exit 1"
  transitions:
    - target: done
```

No `gates.*` when clause → legacy mode. Gate fails → `GateBlocked`. The
`blocking_conditions` array contains:

```json
[{
  "name": "ci_check",
  "type": "command",
  "status": "failed",
  "agent_actionable": true,
  "output": {"exit_code": 1, "error": ""}
}]
```

### 7.3 `context-exists` gate — `context-gate.md`

```yaml
check:
  gates:
    ctx_check:
      type: context-exists
      key: required_key
  transitions:
    - target: done
```

No `gates.*` when clause → legacy mode. When `required_key` is absent from the
context, `koto next` returns `action: "gate_blocked"` with:

```json
[{
  "name": "ctx_check",
  "type": "context-exists",
  "status": "failed",
  "agent_actionable": true,
  "output": {"exists": false, "error": ""}
}]
```

### 7.4 `command` gate with `accepts` fallback — `simple-gates.md`

The `simple-gates` fixture demonstrates the evidence-fallback path. The `start`
state has a `command` gate and an `accepts` block with agent-evidence transitions:

```yaml
start:
  gates:
    check_file:
      type: command
      command: "test -f wip/check.txt"
  accepts:
    status:
      type: enum
      values: [completed, override, blocked]
      required: true
    detail:
      type: string
      required: false
  transitions:
    - target: done
      when:
        status: completed
    - target: done
      when:
        status: override
    - target: done
```

Behavior:
- Gate passes (`wip/check.txt` exists): auto-advances to `done` via the
  unconditional fallback. `action: "done"`, `advanced: true`.
- Gate fails (`wip/check.txt` absent): `action: "evidence_required"`,
  `state: "start"`. The agent must submit `{"status": "completed", "detail": "..."}` (or another matching combination) to advance.
- Note: the `when` clauses in this fixture reference only agent evidence fields
  (`status`), not `gates.*` fields. This is a legacy-mode gate combined with
  an evidence-fallback accepts block.

### 7.5 Mixed routing — `mixed-routing.md`

A `when` clause that combines a `gates.*` field with an agent evidence field:

```yaml
transitions:
  - target: approved
    when:
      gates.lint.exit_code: 0
      decision: approve
  - target: rejected
    when:
      decision: reject
  - target: done
```

- Agent submits `{"decision": "approve"}` and `lint` gate exits 0: routes to
  `approved`.
- Agent submits `{"decision": "reject"}`: routes to `rejected` (the `gates.*`
  condition is not present on this transition, so only `decision` is checked).
- This state requires an `accepts` block because of the agent evidence fields in
  `when` (D3 rule 5).

### 7.6 Legacy-gate mode — `legacy-gates.md`

```yaml
verify:
  gates:
    ci_check:
      type: command
      command: "true"
  accepts:
    status:
      type: enum
      values: [done]
      required: true
  transitions:
    - target: complete
      when:
        status: done
    - target: complete
```

- `koto template compile` rejects this (D5) unless `--allow-legacy-gates` is
  passed.
- `koto init` emits a stderr warning and initializes anyway.
- The gate (`true`) always passes, so the engine auto-advances to `complete`
  via the unconditional fallback. `action: "done"`, `state: "complete"`.
- If the gate were to fail, the engine would fall through to `EvidenceRequired`
  (because `accepts` is present) rather than returning `GateBlocked`.

---

## 8. `blocking_conditions` Array — Full Shape Reference

Each element of `blocking_conditions` serializes to:

```json
{
  "name": "<gate_name>",
  "type": "<gate_type_string>",
  "status": "<outcome_string>",
  "agent_actionable": <boolean>,
  "output": { <gate-type-specific fields> }
}
```

Field notes:
- `"name"` — the gate's key in the state's `gates` map. Matches the key used in
  `gates.<name>.<field>` routing paths.
- `"type"` — the string value of the gate type (`"command"`, `"context-exists"`,
  or `"context-matches"`). Serialized from `condition_type` field in
  `BlockingCondition`, which maps to the Rust struct's `#[serde(rename = "type")]`.
- `"status"` — one of `"failed"`, `"timed_out"`, `"error"`. Never `"passed"`
  (passed gates are filtered out before building the array).
- `"agent_actionable"` — boolean; see section 5.
- `"output"` — gate-type-specific object; see section 2.

`blocking_conditions` appears in two `action` variants:
- `"gate_blocked"` — state has no `accepts` block; `expects` is `null`.
- `"evidence_required"` — state has an `accepts` block; `expects` is present.
  `blocking_conditions` may be non-empty, meaning gates failed but the agent can
  still provide evidence to proceed.

---

## 9. Gate Evaluation: No Short-Circuiting

All gates in a state are evaluated before any blocking decision is made. The
`evaluate_gates()` function in `src/gate.rs` iterates the full gate map
regardless of individual results. This means `blocking_conditions` always
contains every non-passing gate, never just the first one that failed.

Overridden gates (those with an active `GateOverrideRecorded` event in the
current epoch) are injected as synthetic `Passed` results without calling
`evaluate_gates`. No `GateEvaluated` event is emitted for overridden gates.

---

## 10. Reserved Key: `"gates"` in Evidence Submissions

The top-level key `"gates"` in agent-submitted evidence is reserved by the
engine. The CLI rejects any `--with-data` payload that contains a top-level
`"gates"` key with an `InvalidSubmission` error (exit code 2). This prevents
agents from forging gate output.

The reservation is enforced at the `handle_next` layer before the advance loop
runs. A `debug_assert!` in `advance_until_stop()` also enforces the invariant
in debug builds.
