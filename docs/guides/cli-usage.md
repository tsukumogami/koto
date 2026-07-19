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

The `--with-data` value can be either inline JSON or a file reference. Prefix a path with `@` to read the payload from disk — useful for batch task lists and any payload large enough to be awkward on the command line:

```bash
# Inline JSON
koto next task-42 --with-data '{"decision":"proceed"}'

# Read from file
koto next coord --with-data @tasks.json
```

The 1 MB cap applies to both forms (file size is checked before reading). Use `@-` is **not** supported; only file paths are accepted after `@`.

- `--full` -- Include the `details` field in the response regardless of visit count. By default, `details` is included on first visit to a state and omitted on subsequent visits. This flag forces inclusion every time.
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

Every successful response is a JSON object with an `action` field and an `error` field set to `null`. The `action` value identifies the response type -- dispatch on it directly.

| Field | EvidenceRequired | GateBlocked | Integration | IntegrationUnavailable | Confirm | Terminal |
|-------|:---:|:---:|:---:|:---:|:---:|:---:|
| `action` | `"evidence_required"` | `"gate_blocked"` | `"integration"` | `"integration_unavailable"` | `"confirm"` | `"done"` |
| `state` | yes | yes | yes | yes | yes | yes |
| `directive` | yes | yes | yes | yes | yes | -- |
| `details` | optional | optional | optional | optional | optional | -- |
| `advanced` | yes | yes | yes | yes | yes | yes |
| `expects` | object | `null` | object or `null` | object or `null` | object or `null` | `null` |
| `blocking_conditions` | array | array | -- | -- | -- | -- |
| `action_output` | -- | -- | -- | -- | object | -- |
| `integration` | -- | -- | object | object | -- | -- |
| `unassigned_children` | array | array | array | array | array | array |
| `error` | `null` | `null` | `null` | `null` | `null` | `null` |

"yes" = always present. "--" = absent from the JSON (not `null`, just missing). "object or `null`" = present as an object when the state has an `accepts` block, `null` otherwise. "optional" = present on first visit to the state (or when `--full` is passed), absent on subsequent visits and when the state has no details content.

The `unassigned_children` array is present on every `NextResponse` variant (including Terminal `done` and Error) so coordinator-side consumers branch uniformly on the field rather than on the action label. Each element describes a child workflow waiting on agent dispatch with fields `child_session_id`, `role`, `template`, `inputs` (optional), `requested_by`, `created_at`, and `dispatch_epoch`. The discovery scan populates the list from headers under `~/.koto/sessions/*` whose request-store fields name the workflow being ticked as their `coordinator_of_record` and that have not yet been claimed; the list caps at `request_store.directive_batch_size` (default 50) per tick, with overflow surfaced on subsequent ticks.

The `advanced` field is a boolean indicating that at least one state transition occurred during this invocation. It's informational only -- dispatch on `action`, not on `advanced`.

**EvidenceRequired** -- the state expects the agent to do work and submit evidence:

```json
{
  "action": "evidence_required",
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
  "blocking_conditions": [],
  "error": null
}
```

The `expects.options` array is omitted when no transitions have `when` conditions. The `values` array on a field is omitted when empty.

The `blocking_conditions` array is always present on `evidence_required` responses. When gates fail on a state with an `accepts` block, the array is populated with the failing gates. Fix the conditions first, then call `koto next` again -- once gates pass, submit evidence normally. When no gates are blocking, the array is empty.

**GateBlocked** -- one or more command gates failed, timed out, or errored on a state without an `accepts` block:

```json
{
  "action": "gate_blocked",
  "state": "deploy",
  "directive": "Deploy to staging.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "ci_check",
      "type": "command",
      "status": "failed",
      "agent_actionable": false,
      "output": {"exit_code": 1, "error": ""}
    }
  ],
  "error": null
}
```

Each entry in `blocking_conditions` includes structured gate output in the `output` field. The shape of `output` depends on the gate type -- see the [gate output schemas](#gate-output-schemas) in the custom skill authoring guide for details. Passing gates don't appear in the array.

`status` reflects the `GateOutcome`: `"failed"` (pass condition not met), `"timed_out"` (command exceeded its timeout), `"error"` (spawn or evaluation error).

**Integration / IntegrationUnavailable** -- the state declares an integration. When the runner is available, you get `"integration"` with the output. When unavailable, you get `"integration_unavailable"` with `available: false`:

```json
{
  "action": "integration_unavailable",
  "state": "delegate",
  "directive": "Run the integration.",
  "advanced": false,
  "expects": null,
  "integration": {"name": "code_review", "available": false},
  "error": null
}
```

**Confirm** -- a default action ran and needs review before the engine records its result:

```json
{
  "action": "confirm",
  "state": "context_injection",
  "directive": "Review the action output.",
  "advanced": false,
  "action_output": {
    "command": "extract-context.sh --issue 42",
    "exit_code": 0,
    "stdout": "...",
    "stderr": ""
  },
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": {"type": "enum", "required": true, "values": ["accepted", "rejected"]}
    }
  },
  "error": null
}
```

Review the `action_output` and submit evidence if the state accepts it.

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

Terminal responses don't include `directive`, `details`, `blocking_conditions`, `action_output`, or `integration`.

**Dispatcher classification order:**

The dispatcher evaluates the current state in this order and returns the first match:

1. Terminal state -> `Terminal`
2. Any gate failed/timed_out/errored (no accepts block) -> `GateBlocked`
3. Integration declared -> `Integration` or `IntegrationUnavailable`
4. Accepts block exists -> `EvidenceRequired`
5. Gates failed but accepts block exists -> `EvidenceRequired` (with populated `blocking_conditions`)
6. Fallback -> `EvidenceRequired` with empty `expects` (auto-advance candidate)

**Error responses:**

All errors use a structured JSON format with `code`, `message`, and `details` fields:

```json
{"error": {"code": "<string>", "message": "<string>", "details": [...]}}
```

| Exit code | Error codes | Agent action |
|-----------|-------------|--------------|
| 1 | `gate_blocked`, `integration_unavailable`, `concurrent_access` | Retry after fixing or wait |
| 2 | `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized` | Change your approach |
| 3 | `template_error`, `persistence_error` | Report to user |

`template_error` covers structural template problems: cycle detected, chain limit reached, ambiguous transition, dead-end state, unresolvable transition, unknown state. `persistence_error` covers disk I/O failures. `concurrent_access` means another `koto next` is already running on this workflow -- wait and retry.

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

#### template export

Generates a visual representation of a compiled template. Supports two output formats: Mermaid text diagrams and interactive HTML.

```bash
koto template export <source> [--format mermaid|html] [--output <path>] [--check] [--open]
```

**Positional argument:**
- `<source>` -- Path to the template source file (`.md`, compiled on the fly) or pre-compiled JSON (`.json`).

**Flags:**
- `--format` -- Output format: `mermaid` (default) or `html`.
- `--output` -- Write output to a file path. Required for `--format html`. When omitted with `--format mermaid`, output goes to stdout.
- `--check` -- Compare what would be generated against the existing file at `--output` without writing. Exits 0 if fresh, 1 if stale or missing. Requires `--output`.
- `--open` -- Open the generated file in the default browser. Only valid with `--format html`.

**Flag compatibility rules:**

| Combination | Result |
|-------------|--------|
| `--format html` without `--output` | Error (exit 2) |
| `--open` without `--format html` | Error (exit 2) |
| `--open` with `--check` | Error (exit 2) |
| `--check` without `--output` | Error (exit 2) |

**Mermaid format** produces a `stateDiagram-v2` diagram showing states, transitions with condition labels, `[*]` markers for initial and terminal states, and gate annotations. GitHub renders this natively in markdown files.

```bash
# Print Mermaid to stdout
koto template export my-workflow.md

# Write to a sibling file for committing
koto template export my-workflow.md --output my-workflow.mermaid.md

# Check if committed diagram is fresh (for CI)
koto template export my-workflow.md --output my-workflow.mermaid.md --check
```

**HTML format** produces a self-contained interactive diagram using Cytoscape.js with dagre layout. Includes hover tooltips for gates and evidence schemas, click-to-highlight for tracing paths (one hop), pan/zoom, dark mode, and a `[*]` start marker. CDN scripts are loaded with SRI integrity hashes.

```bash
# Generate interactive HTML
koto template export my-workflow.md --format html --output my-workflow.html

# Generate and open in browser for local debugging
koto template export my-workflow.md --format html --output my-workflow.html --open

# Check if deployed HTML is fresh
koto template export my-workflow.md --format html --output docs/my-workflow.html --check
```

Unlike other template subcommands, export errors go to stderr as plain text (not JSON), since it's a developer-facing tool rather than an agent-consumed command.

### config

The `config` subcommand group reads and writes koto's configuration. koto looks for config in two places, merged in this order (later wins):

1. **Project config** -- `.koto/config.toml` in the current repository. Checked into version control and shared with collaborators.
2. **User config** -- `~/.koto/config.toml`. Machine-specific settings and credentials.

Project config uses an allowlist. Only non-secret keys are allowed -- credential keys like `session.cloud.access_key` and `session.cloud.secret_key` are rejected with an error if you try to set them in project config.

#### config get

Prints the resolved value of a config key.

```bash
koto config get <key>
```

**Positional argument:**
- `<key>` -- Dotted config key (e.g., `session.backend`).

Exits 0 and prints the value if set. Exits 1 if the key is unset.

#### config set

Writes a value to project config (default) or user config.

```bash
koto config set <key> <value>
koto config set --user <key> <value>
```

**Positional arguments:**
- `<key>` -- Dotted config key.
- `<value>` -- Value to set.

**Optional flags:**
- `--user` -- Write to `~/.koto/config.toml` instead of `.koto/config.toml`. Without this flag, the value is written to project config, which fails if the key isn't on the project config allowlist.

#### config unset

Removes a key from project config (default) or user config.

```bash
koto config unset <key>
koto config unset --user <key>
```

**Positional argument:**
- `<key>` -- Dotted config key to remove.

**Optional flags:**
- `--user` -- Remove from `~/.koto/config.toml` instead of `.koto/config.toml`.

#### config list

Dumps the fully resolved config as TOML. Credential values are redacted in the output.

```bash
koto config list
koto config list --json
```

**Optional flags:**
- `--json` -- Output as JSON instead of TOML.

#### Config keys reference

| Key | Values | Default | Project config |
|-----|--------|---------|:--------------:|
| `session.backend` | `"local"`, `"cloud"` | `"local"` | yes |
| `session.cloud.endpoint` | S3-compatible endpoint URL | -- | yes |
| `session.cloud.bucket` | Bucket name | `"koto-sessions"` | yes |
| `session.cloud.region` | AWS region | -- | yes |
| `session.cloud.access_key` | Access key ID | -- | no |
| `session.cloud.secret_key` | Secret access key | -- | no |
| `workflows.native` | `true`, `false` | `true` | yes |

Credential keys (`access_key`, `secret_key`) can also be provided through environment variables `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`. Environment variables take precedence over config file values.

`workflows.native` controls rendering koto sessions in Claude Code's `/workflows` screen (see [native-workflows-verification.md](native-workflows-verification.md)); it is on by default and a participating session self-discovers its target directory from `CLAUDE_CODE_SESSION_ID`. Set it to `false` to opt out. A fully headless run (no Claude Code environment) renders nothing regardless.

#### Example: local-only config (default)

No configuration needed. Sessions are stored at `~/.koto/sessions/` and never leave the machine.

#### Example: cloud sync with user config

```toml
# ~/.koto/config.toml
[session]
backend = "cloud"

[session.cloud]
endpoint = "https://s3.us-east-1.amazonaws.com"
bucket = "my-koto-sessions"
region = "us-east-1"
access_key = "AKIAIOSFODNN7EXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

#### Example: shared project config with per-user credentials

```toml
# .koto/config.toml (committed to repo)
[session]
backend = "cloud"

[session.cloud]
endpoint = "https://my-r2-account.r2.cloudflarestorage.com"
bucket = "team-koto-sessions"
```

Each team member sets credentials in their own user config or environment:

```bash
export AWS_ACCESS_KEY_ID="AKIAIOSFODNN7EXAMPLE"
export AWS_SECRET_ACCESS_KEY="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

### Cloud sync

When `session.backend` is set to `"cloud"`, koto syncs session state to S3-compatible storage. This works with AWS S3, Cloudflare R2, MinIO, and any other S3-compatible provider.

Cloud sync is built into existing commands -- `init`, `next`, and `context add` all sync automatically. There's no separate sync command to run.

Cloud sync is included in the default koto binary. To enable it, configure a backend:

```bash
koto config set session.backend cloud
koto config set session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
koto config set session.cloud.bucket my-koto-sessions
```

#### Conflict resolution

Version conflicts are rare but can happen if two machines modify the same session simultaneously. When koto detects a conflict, it pauses and asks you to choose which version to keep:

```bash
koto session resolve <name> --keep local
koto session resolve <name> --keep remote
```

**Positional argument:**
- `<name>` -- Workflow/session name.

**Required flag:**
- `--keep` -- Which version to keep: `local` (discard the remote version) or `remote` (discard local changes and pull the remote version).

**Optional flag:**
- `--children` -- How to reconcile the parent's children during the same call. One of four modes:
  - `auto` (default) — apply the strict-prefix rule to each child's state file. When one side is a byte-exact prefix of the other, the longer side wins; any other divergence surfaces as a conflict that needs its own `koto session resolve <child>`.
  - `skip` — leave child state files untouched.
  - `accept-remote` — overwrite local child state with remote.
  - `accept-local` — overwrite remote child state with local.

After resolving, normal operations resume.

### dashboard

Opens a live TUI showing all sessions for the current repository. Unlike other koto commands, `dashboard` outputs to the terminal rather than JSON, so it's not suitable for agent use.

```bash
koto dashboard [<name>] [--once] [--interval <ms>] [--status <liveness>] [--needs-you] [--all]
```

**Positional argument:**
- `<name>` -- Optional. When provided, filters the display to the named session only.

**Optional flags:**
- `--once` -- Print a snapshot and exit. Outputs tab-separated lines, one per session, then exits 0. Exits 0 even when the session directory is empty.
- `--interval <ms>` -- Override the default 500ms poll interval. Only affects the live TUI mode.
- `--status <liveness>` -- (`--once` only) Filter to a single liveness state. One of: `needs-you-blocked`, `needs-you-failed`, `needs-you-stalled`, `active`, `idle`, `pending`, `done`.
- `--needs-you` -- (`--once` only) Show only sessions in the needs-you band (blocked, failed, or stalled).
- `--all` -- (`--once` only) Include the receded set (done and abandoned sessions), which is excluded by default.

**TUI navigation:**

| Key | Action |
|-----|--------|
| `j` / `k` | Move cursor down / up |
| `Enter` | Open detail panel for selected session |
| `Escape` | Close detail panel |
| `r` | Force refresh |
| `q` | Quit |

The TUI shows all sessions as a tree with each session's current state, elapsed time, and task counts. It polls for changes at 500ms by default.

**`--once` output format:**

Each line is tab-separated with eight fields:

```
<id>\t<current_state>\t<elapsed>\t<status_bucket>\t<intent>\t<template>\t<idle>\t<liveness>
```

The `status_bucket` column (field 4) uses one of five values: `running`, `done`, `failed`, `blocked`, or `unknown`. The `liveness` column (field 8) carries the machine-readable liveness token (e.g. `needs-you-blocked`, `active`, `idle`) used by `--status`. Fields 5-7 (`intent`, `template`, `idle`) are sanitized of tabs and newlines.

**Examples:**

```bash
# Open the live TUI for all sessions
koto dashboard

# Watch a single session
koto dashboard my-workflow

# Snapshot for scripting
koto dashboard --once

# Snapshot for a specific session, faster poll in live mode
koto dashboard --once my-workflow
koto dashboard --interval 200
```

### version

Prints version information as JSON.

```bash
koto version
```

```json
{"version":"0.1.0","commit":"abc1234","built_at":"2026-03-14T00:00:00Z"}
```

### workspace prune

Reclaims a workspace tree rooted at a terminal session. The verb reads the root header, verifies the workflow has reached a terminal state (`completed` or `abandoned`), walks descendants via the session backend's `list()` filtered by `parent_workflow`, and removes the directories after operator confirmation.

```bash
koto workspace prune --root <session-id> [--dry-run] [--yes] [--force]
```

**Required:**
- `--root <session-id>` -- Root session id of the tree to prune. Must be a valid session id (the same allowlist `session start` enforces).

**Optional:**
- `--dry-run` -- Print the descendant set and exit 0 without reclaiming. Useful for inspecting what would be removed before committing.
- `--yes` -- Skip the interactive confirmation prompt. Required for cron-friendly invocation.
- `--force` -- Bypass the terminal-state safety gate. Allows pruning a tree whose root has NOT reached a terminal state. **Dangerous: a force-prune of a tree that still has a coordinator holding a claim corrupts that coordinator's view of the workspace.** Combining `--yes` with `--force` adds a second confirmation prompt that requires typing the literal string `force-prune` to proceed.

**Symlink refusal:** the verb `lstat()`s the root before any directory traversal. A root that is a symlink (pointing inside or outside `~/.koto/`) is rejected categorically with exit code 2 — this is a workspace-escape mitigation.

**Terminal-state safety gate:** without `--force`, the verb refuses to prune a tree whose root has not reached `completed` or `abandoned`. The error names the current state so the operator can decide whether to wait or force.

**JSON output (success):**

```json
{
  "name": "my-workflow",
  "pruned": true,
  "descendants_removed": 3,
  "cursors_gc": 1
}
```

`cursors_gc` is the count of stale `~/.koto/coordinators/<id>/scan_cursor.toml` files reclaimed as part of the prune run. See `docs/workspace-layout.md` for the full derived-file catalog and the prune-cadence sizing guide.

**Cron-friendly invocation:**

```bash
# Weekly Sunday at 02:00, suppressing the JSON output to silence cron mail.
0 2 * * 0 /usr/local/bin/koto workspace prune --root <id> --yes >/dev/null 2>&1
```

### session start

Creates a child session under a named parent. Drives two distinct on-disk shapes via a companion-flag contract.

```bash
koto session start <name> --parent <parent>
  [--needs-agent --role <r> --template <t> --inputs <json>]
  [--coordinator-of-record <coord-id>]
```

**Required positional:**
- `<name>` -- Name of the new child session. Validated against the same allowlist used by `koto init`.

**Required flag:**
- `--parent <parent>` -- Name of the parent workflow this session is a child of. Validated as a session id.

**Companion-flag contract for the request-store dispatch flow:**
- `--needs-agent` (boolean) -- Mark the session as awaiting agent dispatch. Writes `needs_agent = true` to the header.
- `--role <r>`, `--template <t>`, `--inputs <json>` -- All three are REQUIRED when `--needs-agent` is set. Any of them passed WITHOUT `--needs-agent` is rejected with a parse-time error naming the missing companion.
- `--coordinator-of-record <coord-id>` -- Optional. Defaults to the parent's recorded `coordinator_of_record`, falling back to the parent's session id when the parent is itself pre-request-store.

**Two shapes:**
1. **Plain start** — `--needs-agent` omitted and all four dispatch flags omitted. The header carries no dispatch-request marker.
2. **Dispatch-request start** — `--needs-agent` set along with `--role`, `--template`, `--inputs`. The header writes the dispatch-request fields; a coordinator can claim the session via the request-store protocol on its next `koto next` tick.

**Inputs validation:** `--inputs` must be valid JSON, ≤ 1 MiB, and nested ≤ 128 levels deep. Rejection is exit code 2.

**JSON output:**

```json
{
  "name": "task-42-child-a",
  "parent": "task-42",
  "needs_agent": true
}
```

When a `koto next` tick lands on a needs-agent child that has not yet been claimed, koto returns exit code 66 (EX_NOINPUT) with the typed error `needs_agent_not_dispatched` rather than the historical `corrupt state file` message. Route ticks through the coordinator's `koto next` on the parent root instead.

### next --redelegation-cap

The `koto next` verb accepts `--redelegation-cap <n>` to override the resolved `request_store.redelegation_cap` (default 3) for the current tick. Useful for one-off operator-driven retries when a respawn-heavy workload temporarily exceeds the steady-state cap. The override does NOT persist; subsequent ticks fall back to the resolved value.

### next --dispatch-epoch

The `koto next` verb accepts `--dispatch-epoch <n>` to write the current tick's `ChildDispatched` audit event with the supplied dispatch epoch. Used by recovery walks (Issue 11 cases 3b/3c) when a header rewrite has bumped a child's epoch and the coordinator's log needs to record the bump as a fresh dispatch.

## Typical agent workflow

The standard loop for an AI agent dispatches on the `action` field:

```bash
# Initialize from a template
koto init task-42 --template workflow.md

# Main loop
while true; do
  result=$(koto next task-42)
  action=$(echo "$result" | jq -r '.action')

  case "$action" in
    "done")
      # Terminal state -- workflow is done
      break
      ;;
    "gate_blocked")
      # Read .blocking_conditions, fix the issue, then re-query
      continue
      ;;
    "evidence_required")
      # Check .blocking_conditions first -- fix if non-empty
      # Do the work described in .directive
      # Submit evidence matching .expects schema
      result=$(koto next task-42 --with-data '{"decision": "proceed"}')
      ;;
    "integration"|"integration_unavailable")
      # Review .integration output (or proceed manually if unavailable)
      # Submit evidence if .expects is present
      ;;
    "confirm")
      # Review .action_output
      # Submit evidence if the state accepts it
      result=$(koto next task-42 --with-data '{"status": "accepted"}')
      ;;
  esac
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

## Batch workflows

A batch workflow has one coordinator (parent) that submits a task list, and many workers (children) that drive their own state machines independently. Templates with a `materialize_children` hook expose batch surface through existing commands.

### Batch surface on existing commands

- **`koto next <parent> --with-data @tasks.json`** — submit the task list. Responses from a batch-scoped parent carry a `scheduler` object with `materialized_children`, `spawned_this_tick`, and per-task `feedback.entries`. Dispatch workers based on `materialized_children`, not `spawned_this_tick`.
- **`koto workflows --children <parent>`** — list every child for a parent, with per-row batch metadata (short task name, outcome, waits-on dependencies).
- **`koto status <parent>`** — read-only view of the parent's current state. For batch parents, the response includes the materialized-children ledger so you can check progress without advancing state.

### Worked example: 3-task linear batch

Given a coordinator that declares `tasks` as an accepts field and routes on `children-complete` gate output, a minimal dependency chain (`task-1` → `task-2` → `task-3`) flows like this:

```bash
# 1. Parent is on the submission state — coordinator submits the task list
koto next coord --with-data @tasks.json
# => action: "gate_blocked" (children-complete waiting),
#    scheduler.materialized_children: [
#      {"name": "coord.task-1", "outcome": "running", "ready_to_drive": true, ...},
#      {"name": "coord.task-2", "outcome": "blocked", "ready_to_drive": false, "waits_on": ["task-1"]},
#      {"name": "coord.task-3", "outcome": "blocked", "ready_to_drive": false, "waits_on": ["task-2"]}
#    ]

# 2. For each entry where ready_to_drive == true, dispatch a worker:
koto next coord.task-1                      # worker drives the child
# ... worker submits evidence for each state until child reaches terminal ...

# 3. Coordinator re-ticks to observe progress:
koto next coord
# => materialized_children updated; task-2 is now ready_to_drive: true

# 4. Dispatch the next worker, re-tick, repeat until all children terminal.

# 5. Final coordinator tick fires the success route:
koto next coord
# => action: "evidence_required" or "done" (depending on post-batch template states)
```

Each coordinator tick re-derives the ledger from disk, so resume after a crash just means running `koto next coord` again. For the full runner surface (failure routing, `retry_failed`, typed error envelopes), see `docs/designs/DESIGN-batch-child-spawning.md` and the `koto-user` skill's batch references.
