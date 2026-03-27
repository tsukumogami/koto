# CLI Usage Guide

koto's CLI manages workflow state for AI coding agents. All commands output JSON. All commands exit with code 0 on success and non-zero on failure. Errors are printed to stdout as JSON:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

## Session storage

Each workflow's state lives in a dedicated session directory under `~/.koto/sessions/<repo-id>/<name>/`. The `<repo-id>` is derived from the current repository, so sessions from different repos don't collide.

The state file inside each session directory is named `koto-<name>.state.jsonl` and uses an event log format:

- **Line 1 (header):** JSON object with `schema_version`, `workflow`, `template_hash`, and `created_at`.
- **Lines 2+:** Typed events, each with a monotonic `seq` number, `timestamp`, `type`, and a type-specific `payload`.

The current state is derived by replaying the log -- it's the `to` field of the last state-changing event (`transitioned`, `directed_transition`, or `rewound`).

```
~/.koto/sessions/a1b2c3/my-workflow/koto-my-workflow.state.jsonl
~/.koto/sessions/a1b2c3/task-42/koto-task-42.state.jsonl
```

There are no `--state` or `--state-dir` flags. All commands take the workflow name as a positional argument and resolve the session directory automatically.

## Commands

### init

Creates a new workflow from a template file.

```bash
koto init <name> --template <path>
```

**Positional argument:**
- `<name>` -- Workflow name. Used as the session directory name and in the state file name (`koto-<name>.state.jsonl`).

**Required flags:**
- `--template` -- Path to the workflow template file.

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

This creates a session directory at `~/.koto/sessions/<repo-id>/<name>/` and writes a state file inside it. The state file starts with three lines: a header, a `workflow_initialized` event (seq 1), and an initial `transitioned` event (seq 2, from: null, to: the template's initial state).

Exits non-zero if a workflow with that name already exists or if the template is invalid.

### next

Returns the directive for the current state. This is the main agent-facing command -- it tells the agent what to do next, what evidence to submit, and whether any gates are blocking.

```bash
koto next <name> [--with-data <json>] [--to <target>] [--no-cleanup]
```

**Positional argument:**
- `<name>` -- Workflow name.

**Optional flags:**
- `--with-data <json>` -- Submit evidence as a JSON object, validated against the state's `accepts` schema. On success, appends an `evidence_submitted` event and sets `advanced: true` in the response.
- `--to <target>` -- Directed transition to a named state. The target must be a valid transition from the current state. Appends a `directed_transition` event, then dispatches on the new state (skipping gate evaluation).

The `--with-data` and `--to` flags are mutually exclusive. Passing both produces a `precondition_failed` error with exit code 2. The `--with-data` payload is capped at 1 MB.

- `--no-cleanup` -- Skip automatic session directory cleanup when the workflow reaches a terminal state. Useful for debugging or when you need to inspect session artifacts after completion. Without this flag, koto removes the session directory once it outputs the terminal response.

**Runtime variable substitution:**

Before evaluating gate commands or serializing directives, `koto next` replaces `{{SESSION_DIR}}` tokens with the absolute path to the workflow's session directory. This lets templates reference session-local files without hard-coding paths:

```markdown
## plan

Write an implementation plan to {{SESSION_DIR}}/plan.md.

**Gate**: cat {{SESSION_DIR}}/plan.md | head -1
```

`SESSION_DIR` is a reserved variable name and can't be overridden by template-defined variables.

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

Lists all active workflows for the current repository.

```bash
koto workflows
```

**Output (JSON):**

```json
[{"name":"my-workflow","created_at":"2026-03-15T10:00:00Z","template_hash":"a1b2c3..."},{"name":"task-42","created_at":"2026-03-15T11:30:00Z","template_hash":"d4e5f6..."}]
```

Each object contains the workflow name, creation timestamp, and template hash read from the state file header. Returns an empty array `[]` when no workflows are found.

### session

The `session` subcommand group provides direct access to session directories. These are useful for skills that need to read or write session-local artifacts, and for manual cleanup during development.

#### session dir

Prints the absolute path to a session's directory. This is the primary way skills discover where to store artifacts.

```bash
koto session dir <name>
```

**Output (plain text):**

```
/home/user/.koto/sessions/a1b2c3/my-workflow
```

The path is printed even if the directory doesn't exist yet (no I/O validation). This lets callers check the path before or after `koto init`.

#### session list

Lists all sessions for the current repository as a JSON array.

```bash
koto session list
```

**Output (JSON):**

```json
[
  {
    "id": "my-workflow",
    "created_at": "2026-03-15T10:00:00Z",
    "template_hash": "a1b2c3..."
  },
  {
    "id": "task-42",
    "created_at": "2026-03-15T11:30:00Z",
    "template_hash": "d4e5f6..."
  }
]
```

Each object contains the session id (same as the workflow name), creation timestamp, and template hash read from the state file header. Returns an empty array `[]` when no sessions exist. Directories without a valid state file are skipped.

#### session cleanup

Removes a session directory and all its contents. Idempotent -- succeeds even if the session doesn't exist.

```bash
koto session cleanup <name>
```

Produces no output on success. This is the manual equivalent of the auto-cleanup that `koto next` performs when a workflow reaches a terminal state.

### context

The `context` subcommand group manages workflow content. Agents use these commands to submit artifacts, retrieve them, and check whether specific content has been produced. All content is stored opaquely by koto and keyed by session name and content key.

#### context add

Submits content to the store for a given session and key. Reads from stdin by default.

```bash
echo "plan contents" | koto context add <name> <key>
```

Or read from a file:

```bash
koto context add <name> <key> --from-file <path>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key (e.g., `plan.md`, `spirit-greeting.txt`).

**Optional flags:**
- `--from-file` -- Read content from the specified file instead of stdin.

Exits non-zero if the session doesn't exist or the input can't be read. Overwrites any existing content for the same key.

#### context get

Retrieves content from the store. Writes to stdout by default.

```bash
koto context get <name> <key>
```

Or write to a file:

```bash
koto context get <name> <key> --to-file <path>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key.

**Optional flags:**
- `--to-file` -- Write content to the specified file instead of stdout.

Exits non-zero if the session or key doesn't exist.

#### context exists

Checks whether a content key exists for a session. Produces no output.

```bash
koto context exists <name> <key>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key.

Exits 0 if the key exists, 1 if it doesn't. This is the CLI equivalent of the `context-exists` gate type in templates.

#### context list

Lists all content keys for a session as a JSON array.

```bash
koto context list <name>
```

Filter by prefix:

```bash
koto context list <name> --prefix "review/"
```

**Positional arguments:**
- `<name>` -- Workflow/session name.

**Optional flags:**
- `--prefix` -- Only list keys that start with this string.

**Output (JSON):**

```json
["plan.md", "review/feedback.md", "spirit-greeting.txt"]
```

Returns an empty array `[]` when no keys exist (or none match the prefix).

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
