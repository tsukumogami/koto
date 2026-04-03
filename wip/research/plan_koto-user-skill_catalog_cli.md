# koto CLI Surface Catalog

Source files read:
- `src/cli/mod.rs`
- `src/cli/next.rs`
- `src/cli/next_types.rs`
- `src/cli/overrides.rs`
- `src/cli/context.rs`
- `src/cli/session.rs`
- `src/cli/vars.rs`
- `src/main.rs`

Generated for: `koto-user/references/command-reference.md`

---

## Exit code space

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Transient / retryable (gate blocked, integration unavailable, concurrent access, engine errors) |
| 2 | Caller error — agent must change its behavior (invalid submission, precondition failed, terminal state, workflow not initialized) |
| 3 | Infrastructure / config error (corrupted state, template hash mismatch, parse failure) |

`koto next` uses structured `NextError` JSON for domain errors. All other commands use flat `{"error": "<string>", "command": "<name>"}` JSON on stderr.

---

## Error response shapes

### koto next domain errors (structured)

```json
{
  "error": {
    "code": "<NextErrorCode>",
    "message": "<string>",
    "details": [{"field": "<string>", "reason": "<string>"}]
  }
}
```

`NextErrorCode` values and their exit codes:

| Code | Exit |
|------|------|
| `gate_blocked` | 1 |
| `integration_unavailable` | 1 |
| `concurrent_access` | 1 |
| `invalid_submission` | 2 |
| `precondition_failed` | 2 |
| `terminal_state` | 2 |
| `workflow_not_initialized` | 2 |
| `template_error` | 3 |
| `persistence_error` | 3 |

### All other commands (flat)

```json
{"error": "<string>", "command": "<subcommand name>"}
```

---

## Subcommand catalog

### `koto version`

**Syntax:** `koto version [--json]`

**Flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `json` | `--json` | flag | No | Output as JSON instead of human-readable text |

**Output:**

Without `--json`:
```
koto <version> (<commit> <built_at>)
```

With `--json`:
```json
{"version": "<string>", "commit": "<string>", "built_at": "<string>"}
```

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto init`

**Syntax:** `koto init <name> --template <path> [--var KEY=VALUE ...]`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name. Must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` |
| `template` | `--template <path>` | String | Yes | Path to the template `.md` source file |
| `vars` | `--var KEY=VALUE` | Vec<String> | No | Set a template variable. Repeatable. Value must match the template's `variables:` declarations. Unknown keys and missing required keys are rejected. |

**Output (success):**
```json
{"name": "<string>", "state": "<initial_state>"}
```

**Error output (flat JSON, various exit codes):**
- Exit 1: session creation failure
- Exit 2: name validation failure, workflow already exists, missing required variable, unknown variable
- Exit 3: template compile/parse failure, template hash mismatch

**Notes:**
- Template is compiled and cached before writing the state file.
- Reserved variable names (`SESSION_DIR`, `SESSION_NAME`) cannot be declared in template `variables:` blocks — they are injected at runtime.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto next`

**Syntax:**
```
koto next <name>
koto next <name> --with-data '<json>'
koto next <name> --to <state>
koto next <name> [--no-cleanup] [--full]
```

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |
| `with_data` | `--with-data <json>` | String (JSON) | No | Submit evidence as JSON. Validated against the state's `accepts` schema. Max 1 MB. Mutually exclusive with `--to`. The `"gates"` key is reserved and rejected. |
| `to` | `--to <state>` | String | No | Force a directed transition to a named state. Must be a valid transition target from the current state. Mutually exclusive with `--with-data`. |
| `no_cleanup` | `--no-cleanup` | flag | No | Skip session cleanup when reaching a terminal state. Useful for debugging. |
| `full` | `--full` | flag | No | Always include the `details` field in the response, regardless of how many times the state has been visited. |

**Output: six possible JSON shapes, distinguished by the `action` field**

All responses include `"error": null` when successful.

#### `action: "evidence_required"`

```json
{
  "action": "evidence_required",
  "state": "<string>",
  "directive": "<string>",
  "details": "<string | omitted>",
  "advanced": <bool>,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "<field_name>": {
        "type": "<string|boolean|enum>",
        "required": <bool>,
        "values": ["<string>", ...]
      }
    },
    "options": [
      {"target": "<state>", "when": {"<field>": <value>}}
    ]
  },
  "blocking_conditions": [<BlockingCondition>, ...],
  "error": null
}
```

- `details` is omitted when empty or on repeat visits (unless `--full`).
- `expects.options` is omitted when empty (no transitions with `when` conditions).
- `blocking_conditions` is an empty array `[]` when no gates are blocking.
- When a state has no `accepts` block, no `integration`, no blocking gates, and is not terminal, this variant is returned with empty `expects.fields` and `expects.options` — this signals an auto-advance candidate.

#### `action: "gate_blocked"`

```json
{
  "action": "gate_blocked",
  "state": "<string>",
  "directive": "<string>",
  "details": "<string | omitted>",
  "advanced": <bool>,
  "expects": null,
  "blocking_conditions": [<BlockingCondition>, ...],
  "error": null
}
```

- Returned when one or more gates failed/timed_out/errored and the state has **no** `accepts` block.
- When the state **does** have an `accepts` block, gates failing fall through to `evidence_required` instead (so the agent can submit override evidence).

#### `action: "integration_unavailable"`

```json
{
  "action": "integration_unavailable",
  "state": "<string>",
  "directive": "<string>",
  "details": "<string | omitted>",
  "advanced": <bool>,
  "expects": <ExpectsSchema | null>,
  "integration": {"name": "<string>", "available": false},
  "error": null
}
```

- Returned when a state declares an `integration:` but the integration runner is not yet implemented.
- `expects` is present when the state also has an `accepts` block, `null` otherwise.

#### `action: "integration"` (future)

```json
{
  "action": "integration",
  "state": "<string>",
  "directive": "<string>",
  "details": "<string | omitted>",
  "advanced": <bool>,
  "expects": <ExpectsSchema | null>,
  "integration": {"name": "<string>", "output": <any>},
  "error": null
}
```

- Not yet produced (integration runner deferred to issue #49). Shape is defined in `NextResponse::Integration`.

#### `action: "done"`

```json
{
  "action": "done",
  "state": "<string>",
  "advanced": <bool>,
  "expects": null,
  "error": null
}
```

- Returned when the workflow reaches a terminal state.

#### `action: "confirm"`

```json
{
  "action": "confirm",
  "state": "<string>",
  "directive": "<string>",
  "details": "<string | omitted>",
  "advanced": <bool>,
  "action_output": {
    "command": "<string>",
    "exit_code": <int>,
    "stdout": "<string>",
    "stderr": "<string>"
  },
  "expects": <ExpectsSchema | null>,
  "error": null
}
```

- Returned when a default action ran and requires agent confirmation.

#### `BlockingCondition` object (appears in `blocking_conditions` arrays)

```json
{
  "name": "<gate name>",
  "type": "<gate type: command | context-exists | context-matches | ...>",
  "status": "<failed | timed_out | error>",
  "agent_actionable": <bool>,
  "output": <any>
}
```

- `agent_actionable` is `true` when the gate has an `override_default` value or a built-in default for its gate type. When `true`, the agent can call `koto overrides record` to unblock the gate.
- `output` is the structured gate result (e.g., `{"exit_code": 1, "error": ""}` for command gates).
- Only non-passing gates appear in `blocking_conditions`. Passing gates are excluded.

**Classification priority (highest to lowest):**
1. Terminal state → `done`
2. Gates failed + no `accepts` block → `gate_blocked`
3. Gates failed + `accepts` block → `evidence_required` (falls through)
4. Integration declared → `integration_unavailable`
5. `accepts` block present → `evidence_required`
6. Fallback (no accepts, no integration, not terminal) → `evidence_required` with empty `expects.fields`

**Error output (structured NextError JSON):** See error response shapes above.

**Skill file:** `koto-user/references/command-reference.md` (primary); `koto-author/SKILL.md` (response schema section)

---

### `koto cancel`

**Syntax:** `koto cancel <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |

**Output (success):**
```json
{"name": "<string>", "state": "<current_state>", "cancelled": true}
```

**Error cases:**
- Exit 1: workflow not found, persistence error
- Exit 2: already cancelled, already in terminal state
- Exit 3: template read/parse failure, corrupt state file

**Notes:**
- Appends a `WorkflowCancelled` event. After cancellation, `koto next` returns `terminal_state` error (exit 2).
- Cannot cancel a workflow already in a terminal state (use natural completion instead).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto rewind`

**Syntax:** `koto rewind <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |

**Output (success):**
```json
{"name": "<string>", "state": "<previous_state>"}
```

**Error cases:**
- Exit 1: workflow not found, persistence error
- Exit 1 (flat JSON): already at initial state

**Notes:**
- Appends a `Rewound` event. The previous state is derived by scanning the last two state-changing events (`Transitioned`, `DirectedTransition`, `Rewound`).
- Cannot rewind past the initial state.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto workflows`

**Syntax:** `koto workflows`

**No flags or arguments.**

**Output (success):** JSON array of workflow metadata objects:
```json
[
  {
    "name": "<string>",
    "created_at": "<RFC 3339 UTC timestamp>",
    "template_hash": "<sha256 hex>"
  }
]
```

**Notes:**
- Lists all active workflows in the current directory.
- Does not include current state; use `koto next <name>` to get state.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto decisions record`

**Syntax:** `koto decisions record <name> --with-data '<json>'`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |
| `with_data` | `--with-data <json>` | String (JSON) | Yes | Decision payload. Must be a JSON object with `choice` (string, required) and `rationale` (string, required). `alternatives_considered` (array of strings) is optional. Max 1 MB. |

**Required `--with-data` schema:**
```json
{
  "choice": "<string>",
  "rationale": "<string>",
  "alternatives_considered": ["<string>", ...]
}
```

**Output (success):**
```json
{"state": "<current_state>", "decisions_recorded": <int>}
```

`decisions_recorded` is the total count of decisions in the current epoch after recording.

**Error output:** Flat JSON, various exit codes.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto decisions list`

**Syntax:** `koto decisions list <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |

**Output (success):**
```json
{
  "state": "<current_state>",
  "decisions": {
    "count": <int>,
    "items": [<decision_object>, ...]
  }
}
```

Each item is the raw JSON object from the `decisions record --with-data` call (preserves all fields: `choice`, `rationale`, `alternatives_considered`).

**Notes:**
- Returns decisions for the current epoch only (decisions from before the last state reset are excluded).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto overrides record`

**Syntax:** `koto overrides record <name> --gate <gate> --rationale <text> [--with-data '<json>']`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |
| `gate` | `--gate <gate>` | String | Yes | Name of the gate to override. Must exist in the current template state. |
| `rationale` | `--rationale <text>` | String | Yes | Explanation for why the override is justified. Max 1 MB. |
| `with_data` | `--with-data <json>` | String (JSON) | No | Override value to substitute as gate output. Falls back to the gate's `override_default` value, then to the built-in default for the gate type. Error if no default is available and `--with-data` is omitted. |

**Override value resolution order:**
1. `--with-data` argument
2. Gate's `override_default` value (from template)
3. Built-in default for the gate type (e.g., `{"exit_code": 0, "error": ""}` for `command`)
4. Error if none available

**Output (success):**
```json
{"status": "recorded"}
```

**Error output:** Flat JSON, various exit codes.
- Exit 2: gate not found in current state, no override value available, workflow not found
- Exit 3: template hash mismatch, corrupt state file, template read/parse failure

**Notes:**
- Only applicable when a gate's `blocking_conditions[].agent_actionable` is `true`.
- Records the actual gate output (from the last `GateEvaluated` event) alongside the override, creating an audit trail.
- After recording an override, the gate is treated as passed on the next `koto next` call.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto overrides list`

**Syntax:** `koto overrides list <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Workflow name |

**Output (success):**
```json
{
  "state": "<current_state>",
  "overrides": {
    "count": <int>,
    "items": [
      {
        "state": "<string>",
        "gate": "<string>",
        "rationale": "<string>",
        "override_applied": <any>,
        "actual_output": <any | null>,
        "timestamp": "<RFC 3339>"
      }
    ]
  }
}
```

- `actual_output` is `null` when no `GateEvaluated` event was recorded for the gate.
- Returns **all** override history across all epoch boundaries (unlike `decisions list` which scopes to the current epoch).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto template compile`

**Syntax:** `koto template compile <source> [--allow-legacy-gates]`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `source` | positional | String | Yes | Path to the YAML/markdown template source file |
| `allow_legacy_gates` | `--allow-legacy-gates` | flag | No | Allow templates with gates that have no `gates.*` when-clause routing. Transitory flag — will be removed once legacy templates migrate to structured gate routing. |

**Output (success):** Prints the absolute path to the compiled cache file (`.json`).

**Error output:** Flat JSON with `{"error": "<string>", "command": "template compile"}`.

**Notes:**
- When `--allow-legacy-gates` is absent, the compiler runs in strict mode and rejects templates that use gates without `gates.*` routing (error code D5).
- The compiled JSON is cached; subsequent calls with the same source return the cached path.

**Skill file:** `koto-author/SKILL.md` (primary); `koto-user/references/command-reference.md` (brief mention)

---

### `koto template validate`

**Syntax:** `koto template validate <path>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `path` | positional | String | Yes | Path to a compiled template JSON file |

**Output (success):** No output (exit 0).

**Error output:** Flat JSON with `{"error": "<string>", "command": "template validate"}`.

**Skill file:** `koto-author/SKILL.md`

---

### `koto template export`

**Syntax:**
```
koto template export <input> [--format mermaid|html] [--output <path>] [--open] [--check]
```

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `input` | positional | String | Yes | Path to template source (`.md`) or compiled template (`.json`) |
| `format` | `--format <mermaid\|html>` | enum | No | Output format. Default: `mermaid` |
| `output` | `--output <path>` | String | No | Write output to file. Required when `--format html`. |
| `open` | `--open` | flag | No | Open the generated file in the default browser. Only valid with `--format html`. Mutually exclusive with `--check`. |
| `check` | `--check` | flag | No | Verify the existing file at `--output` matches what would be generated. Requires `--output`. Exits 1 if stale or missing. |

**Constraint rules:**
- `--format html` requires `--output`
- `--open` requires `--format html`
- `--open` and `--check` are mutually exclusive
- `--check` requires `--output`

**Output:**
- Without `--output`: writes raw mermaid text (or HTML) to stdout
- With `--output` (no `--check`): writes file and prints the output path
- With `--check`: exits 0 if fresh, 1 if stale/missing (with hint message on stderr)

**Skill file:** `koto-author/SKILL.md`

---

### `koto session dir`

**Syntax:** `koto session dir <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Session/workflow name |

**Output:** Absolute path to the session directory (plain text, not JSON).

**Skill file:** `koto-user/references/command-reference.md` (for reading session artifacts; also useful for debugging)

---

### `koto session list`

**Syntax:** `koto session list`

**No arguments.**

**Output:** Pretty-printed JSON array of session objects (schema determined by backend; includes at minimum the session name and creation timestamp).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto session cleanup`

**Syntax:** `koto session cleanup <name>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Session/workflow name |

**Output:** No output on success (idempotent — succeeds even if the session doesn't exist).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto session resolve`

**Syntax:** `koto session resolve <name> --keep <local|remote>`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `name` | positional | String | Yes | Session/workflow name |
| `keep` | `--keep <local\|remote>` | String | Yes | Which version to keep when resolving a version conflict. Must be `"local"` or `"remote"`. |

**Output:** No output described (delegates to cloud backend `resolve_conflict`).

**Error cases:**
- Error if `--keep` value is not `"local"` or `"remote"`.
- Error if the backend is not cloud (requires `session.backend = "cloud"`).

**Skill file:** `koto-user/references/command-reference.md` (cloud backend users only)

---

### `koto context add`

**Syntax:**
```
koto context add <session> <key> [--from-file <path>]
echo "content" | koto context add <session> <key>
```

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `session` | positional | String | Yes | Session/workflow name |
| `key` | positional | String | Yes | Context key. Hierarchical paths supported (e.g., `scope.md`, `research/r1/lead.md`) |
| `from_file` | `--from-file <path>` | String | No | Read content from this file instead of stdin |

**Output:** No output on success.

**Notes:**
- When `--from-file` is absent, reads content from stdin.
- Keys are hierarchical; `/` separates path components.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto context get`

**Syntax:**
```
koto context get <session> <key> [--to-file <path>]
```

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `session` | positional | String | Yes | Session/workflow name |
| `key` | positional | String | Yes | Context key |
| `to_file` | `--to-file <path>` | String | No | Write content to this file instead of stdout. Parent directories are created if absent. |

**Output:** Content written to stdout (or to `--to-file`).

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto context exists`

**Syntax:** `koto context exists <session> <key>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `session` | positional | String | Yes | Session/workflow name |
| `key` | positional | String | Yes | Context key to check |

**Output:** No output. Exit 0 if the key exists, exit 1 if not.

**Notes:**
- Designed for use in shell conditionals: `if koto context exists my-wf scope.md; then ...`

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto context list`

**Syntax:** `koto context list <session> [--prefix <prefix>]`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `session` | positional | String | Yes | Session/workflow name |
| `prefix` | `--prefix <prefix>` | String | No | Filter keys to those starting with this prefix |

**Output:** JSON array of key strings:
```json
["key1", "research/r1/lead.md", ...]
```

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto config get`

**Syntax:** `koto config get <key>`

**Arguments:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `key` | positional | String | Yes | Dotted key path (e.g., `session.backend`) |

**Output:** The resolved value as a string (plain text). Exits 1 if the key is not found.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto config set`

**Syntax:** `koto config set <key> <value> [--user]`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `key` | positional | String | Yes | Dotted key path (e.g., `session.backend`) |
| `value` | positional | String | Yes | Value to set |
| `user` | `--user` | flag | No | Write to user config (`~/.koto/config.toml`) instead of project config (`.koto/config.toml`) |

**Output:** No output on success.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto config unset`

**Syntax:** `koto config unset <key> [--user]`

**Arguments and flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `key` | positional | String | Yes | Dotted key path |
| `user` | `--user` | flag | No | Remove from user config instead of project config |

**Output:** No output on success.

**Skill file:** `koto-user/references/command-reference.md`

---

### `koto config list`

**Syntax:** `koto config list [--json]`

**Flags:**

| Rust field | CLI spelling | Type | Required | Description |
|------------|-------------|------|----------|-------------|
| `json` | `--json` | flag | No | Output as JSON instead of TOML |

**Output:** Resolved configuration as TOML (default) or JSON. Sensitive fields are redacted.

**Skill file:** `koto-user/references/command-reference.md`

---

## Variable substitution (runtime context)

Not a CLI subcommand, but relevant to understanding gate commands and directives.

Two reserved variable names are injected by the runtime into every gate command and directive string at evaluation time. These cannot be declared in template `variables:` blocks.

| Token | Injected value |
|-------|---------------|
| `{{SESSION_DIR}}` | Absolute path to the workflow's session directory |
| `{{SESSION_NAME}}` | The workflow name passed to `koto init` |

User-defined variables (declared in the template's `variables:` block and supplied via `koto init --var`) are also substituted using the same `{{KEY}}` syntax. Unknown tokens are left unchanged.

**Relevant to:** `koto-author/SKILL.md` (template authoring); `koto-user/references/command-reference.md` (understanding what `SESSION_DIR` means in directives and gate output)

---

## Skill file routing summary

| Subcommand | koto-user | koto-author |
|------------|-----------|-------------|
| `version` | Primary | — |
| `init` | Primary | Mention (runs init to start a workflow) |
| `next` | Primary (full detail) | Reference (response schema) |
| `cancel` | Primary | — |
| `rewind` | Primary | — |
| `workflows` | Primary | — |
| `decisions record` | Primary | Mention |
| `decisions list` | Primary | Mention |
| `overrides record` | Primary | Mention (gate override_default authoring) |
| `overrides list` | Primary | — |
| `template compile` | Brief mention | Primary |
| `template validate` | — | Primary |
| `template export` | — | Primary |
| `session dir` | Primary | — |
| `session list` | Primary | — |
| `session cleanup` | Primary | — |
| `session resolve` | Primary (cloud users) | — |
| `context add` | Primary | Mention (context-exists gate, SESSION_DIR) |
| `context get` | Primary | Mention |
| `context exists` | Primary | — |
| `context list` | Primary | — |
| `config get` | Primary | — |
| `config set` | Primary | — |
| `config unset` | Primary | — |
| `config list` | Primary | — |
