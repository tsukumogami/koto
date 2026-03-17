# CLI Usage Guide

koto's CLI manages workflow state for AI coding agents. All commands output JSON. All commands exit with code 0 on success and non-zero on failure. Errors are printed to stdout as JSON:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

## State file resolution

Each workflow has a state file named `koto-<name>.state.jsonl` in the current directory. The file uses an event log format:

- **Line 1 (header):** JSON object with `schema_version`, `workflow`, `template_hash`, and `created_at`.
- **Lines 2+:** Typed events, each with a monotonic `seq` number, `timestamp`, `type`, and a type-specific `payload`.

The current state is derived by replaying the log -- it's the `to` field of the last state-changing event (`transitioned`, `directed_transition`, or `rewound`).

```
koto-my-workflow.state.jsonl
koto-task-42.state.jsonl
```

There are no `--state` or `--state-dir` flags. All commands that operate on a workflow take the workflow name as a positional argument and resolve the state file automatically from the current directory.

## Commands

### init

Creates a new workflow from a template file.

```bash
koto init <name> --template <path>
```

**Positional argument:**
- `<name>` -- Workflow name. Used in the state file name (`koto-<name>.state.jsonl`).

**Required flags:**
- `--template` -- Path to the workflow template file.

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

The state file starts with three lines: a header, a `workflow_initialized` event (seq 1), and an initial `transitioned` event (seq 2, from: null, to: the template's initial state).

Exits non-zero if a workflow with that name already exists or if the template is invalid.

### next

Returns the directive for the current state. This is the main agent-facing command -- it tells the agent what to do next, what evidence to submit, and whether any gates are blocking.

```bash
koto next <name> [--with-data <json>] [--to <target>]
```

**Positional argument:**
- `<name>` -- Workflow name.

**Optional flags:**
- `--with-data <json>` -- Submit evidence as a JSON object, validated against the state's `accepts` schema. On success, appends an `evidence_submitted` event and sets `advanced: true` in the response.
- `--to <target>` -- Directed transition to a named state. The target must be a valid transition from the current state. Appends a `directed_transition` event, then dispatches on the new state (skipping gate evaluation).

These flags are mutually exclusive. Passing both produces a `precondition_failed` error with exit code 2. The `--with-data` payload is capped at 1 MB.

**Response variants:**

Every successful response is a JSON object with an `action` field (`"execute"` or `"done"`) and an `error` field set to `null`. The remaining fields depend on the variant.

| Field | EvidenceRequired | GateBlocked | Integration | IntegrationUnavailable | Terminal |
|-------|:---:|:---:|:---:|:---:|:---:|
| `action` | `"execute"` | `"execute"` | `"execute"` | `"execute"` | `"done"` |
| `state` | yes | yes | yes | yes | yes |
| `directive` | yes | yes | yes | yes | -- |
| `advanced` | yes | yes | yes | yes | yes |
| `expects` | object | `null` | object or `null` | object or `null` | `null` |
| `blocking_conditions` | -- | array | -- | -- | -- |
| `integration` | -- | -- | object | object | -- |
| `error` | `null` | `null` | `null` | `null` | `null` |

"yes" = always present. "--" = absent from the JSON (not `null`, just missing). "object or `null`" = present as an object when the state has an `accepts` block, `null` otherwise.

**EvidenceRequired** -- the state expects the agent to do work and submit evidence:

```json
{
  "action": "execute",
  "state": "review",
  "directive": "Review the code changes.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "required": true, "values": ["proceed", "escalate"]}
    },
    "options": [
      {"target": "implement", "when": {"decision": "proceed"}}
    ]
  },
  "error": null
}
```

The `expects.options` array is omitted when no transitions have `when` conditions. The `values` array on a field is omitted when empty.

**GateBlocked** -- one or more command gates failed, timed out, or errored:

```json
{
  "action": "execute",
  "state": "deploy",
  "directive": "Deploy to staging.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {"name": "ci_check", "type": "command", "status": "failed", "agent_actionable": false}
  ],
  "error": null
}
```

Possible `status` values: `"failed"`, `"timed_out"`, `"error"`. Passing gates don't appear in the array.

**Integration / IntegrationUnavailable** -- the state declares an integration. When the runner is available, you get `Integration` with the output. When unavailable, you get `IntegrationUnavailable` with `available: false`:

```json
{
  "action": "execute",
  "state": "delegate",
  "directive": "Run the integration.",
  "advanced": false,
  "expects": null,
  "integration": {"name": "code_review", "available": false},
  "error": null
}
```

**Terminal** -- the workflow has ended:

```json
{
  "action": "done",
  "state": "done",
  "advanced": true,
  "expects": null,
  "error": null
}
```

Terminal responses don't include `directive`, `blocking_conditions`, or `integration`.

**Dispatcher classification order:**

The dispatcher evaluates the current state in this order and returns the first match:

1. Terminal state -> `Terminal`
2. Any gate failed/timed_out/errored -> `GateBlocked`
3. Integration declared -> `Integration` or `IntegrationUnavailable`
4. Accepts block exists -> `EvidenceRequired`
5. Fallback -> `EvidenceRequired` with empty `expects` (auto-advance candidate)

**Error responses** use the structured format described in the [error code reference](../reference/error-codes.md). Domain errors exit with code 1 (transient) or 2 (caller error). Infrastructure errors (corrupt state, template hash mismatch) exit with code 3.

### rewind

Rolls back the workflow to the previous state by appending a `rewound` event to the state file.

```bash
koto rewind <name>
```

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

The `rewound` event payload contains `from` (the current state) and `to` (the state being rewound to). Exits non-zero if the workflow is already at the initial state (only one state-changing event exists). Rewind is non-destructive -- it appends a new event rather than truncating history, so the full event log is preserved.

### workflows

Lists all active workflows in the current directory.

```bash
koto workflows
```

**Output (JSON):**

```json
[{"name":"my-workflow","created_at":"2026-03-15T10:00:00Z","template_hash":"a1b2c3..."},{"name":"task-42","created_at":"2026-03-15T11:30:00Z","template_hash":"d4e5f6..."}]
```

Each object contains the workflow name, creation timestamp, and template hash read from the state file header. Returns an empty array `[]` when no workflows are found.

### template

The `template` subcommand group contains authoring tools for template development. These commands aren't needed for running workflows -- they're for people writing and debugging templates.

#### template compile

Compiles a source template to FormatVersion=1 JSON and caches the result. Outputs the compiled JSON file path on success.

```bash
koto template compile <source>
```

**Positional argument:**
- `<source>` -- Path to the YAML template source file.

**Output:** The path to the compiled JSON file.

```
/home/user/.cache/koto/abc123.json
```

Uses SHA256-based caching: if the source hasn't changed, the cached path is returned without recompiling. Exits non-zero with a JSON error on compilation failure.

#### template validate

Validates a compiled template JSON file against the expected schema.

```bash
koto template validate <path>
```

**Positional argument:**
- `<path>` -- Path to the compiled template JSON file.

Exits 0 if the file is valid. Exits non-zero with a JSON error if the schema check fails.

### version

Prints version information as JSON.

```bash
koto version
```

```json
{"version":"0.1.0","commit":"abc1234","built_at":"2026-03-14T00:00:00Z"}
```

## Typical agent workflow

The standard loop for an AI agent:

```bash
# Initialize from a template
koto init task-42 --template workflow.md

# Main loop
while true; do
  result=$(koto next task-42)
  action=$(echo "$result" | jq -r '.action')

  # Terminal state -- workflow is done
  if [ "$action" = "done" ]; then
    break
  fi

  # Agent does the work described in .directive
  # ...

  # Submit evidence if the state expects it
  expects=$(echo "$result" | jq -r '.expects // empty')
  if [ -n "$expects" ]; then
    result=$(koto next task-42 --with-data '{"decision": "proceed"}')
  fi
done
```

Use `--to` for directed transitions when the agent needs to jump to a specific state:

```bash
koto next task-42 --to feedback
```

To roll back after an unexpected result:

```bash
koto rewind task-42
```
