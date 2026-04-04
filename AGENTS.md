# koto — Agent Instructions

koto is a state machine engine for AI agent workflows. It enforces execution order through evidence-gated transitions, persists progress atomically, and makes every state transition recoverable.

You interact with koto by calling `koto next` in a loop. Each call returns JSON telling you what to do. You do it, then call `koto next` again.

## Prerequisites

- koto >= 0.5.0 must be installed and on PATH (`koto version` to verify)
- You need a compiled koto template (`.md` file with YAML frontmatter)

If koto is not installed or the version is too old, install the latest release:

```bash
# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m); [ "$ARCH" = "x86_64" ] && ARCH="amd64"; [ "$ARCH" = "aarch64" ] && ARCH="arm64"

# Download and install
gh release download -R tsukumogami/koto -p "koto-${OS}-${ARCH}" -D /tmp
chmod +x "/tmp/koto-${OS}-${ARCH}"
mv "/tmp/koto-${OS}-${ARCH}" ~/.local/bin/koto
```

## Command Reference

### koto init

Initialize a new workflow from a template.

```
koto init <name> --template <path> [--var KEY=VALUE ...]
```

| Argument/Flag | Required | Description |
|---|---|---|
| `<name>` | Yes | Workflow name. Must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`. |
| `--template <path>` | Yes | Path to the template `.md` source file. Compiled automatically on first use. |
| `--var KEY=VALUE` | No | Set a template variable. Repeatable. Required variables must be supplied; unknown keys are rejected. |

Returns JSON on success:

```json
{"name": "my-workflow", "state": "initial_state_name"}
```

Reserved variable names `SESSION_DIR` and `SESSION_NAME` are injected automatically and can't be declared in templates.

### koto next

The primary command for workflow interaction. Behavior depends on flags:

**Get the current directive** (no flags):

```bash
koto next <name>
```

**Submit evidence** (with `--with-data`):

```bash
koto next <name> --with-data '{"mode": "issue_backed", "issue_number": "42"}'
```

**Directed transition** (with `--to`):

```bash
koto next <name> --to <target_state>
```

| Flag | Description |
|---|---|
| `--with-data <json>` | Submit evidence as a JSON object. Must conform to the state's `accepts` schema. Max 1 MB. The `"gates"` key is reserved and rejected. Mutually exclusive with `--to`. |
| `--to <state>` | Force a directed transition to a named state. Must be a valid transition target. Mutually exclusive with `--with-data`. |
| `--no-cleanup` | Skip automatic session cleanup when the workflow reaches a terminal state. |
| `--full` | Always include the `details` field, even on repeat visits to a state. |

### koto cancel

Cancel a workflow. Prevents further advancement.

```bash
koto cancel <name>
```

Returns:

```json
{"name": "my-workflow", "state": "current_state", "cancelled": true}
```

After cancellation, `koto next` returns exit 2 with `error.code = "terminal_state"`. Cancellation doesn't auto-clean the session directory — use `koto session cleanup` for that.

### koto rewind

Roll back to the previous state. Non-destructive: the event log is preserved.

```bash
koto rewind <name>
```

Returns:

```json
{"name": "my-workflow", "state": "previous_state_name"}
```

Call repeatedly to rewind multiple steps. Can't go past the initial state.

### koto workflows

List all active workflows in the current directory.

```bash
koto workflows
```

Returns a JSON array. Each entry has `name`, `created_at`, and `template_hash`. Returns `[]` when no workflows exist.

### koto overrides record

Record an override for a blocked gate so the next `koto next` call treats it as passed.

```bash
koto overrides record <name> --gate <gate_name> --rationale "<why>"
```

| Flag | Required | Description |
|---|---|---|
| `--gate <gate>` | Yes | Name of the gate to override. Must exist in the current template state. |
| `--rationale <text>` | Yes | Explanation for the override. Max 1 MB. |
| `--with-data '<json>'` | No | Override value. Falls back to the gate's `override_default`, then the built-in default for the gate type. Fails if none available. |

Only use when `blocking_conditions[].agent_actionable` is `true`. When `agent_actionable` is `false`, the command will fail.

### koto overrides list

List all override history for a workflow across all states.

```bash
koto overrides list <name>
```

Returns a JSON object with `state`, `overrides.count`, and `overrides.items[]` (each with `state`, `gate`, `rationale`, `override_applied`, `actual_output`, `timestamp`).

### koto decisions record

Record a structured decision without advancing state.

```bash
koto decisions record <name> --with-data '{"choice": "option-a", "rationale": "because X", "alternatives_considered": ["option-b"]}'
```

`choice` and `rationale` are required. `alternatives_considered` is optional.

Returns:

```json
{"state": "current_state", "decisions_recorded": 2}
```

### koto decisions list

List decisions recorded in the current epoch.

```bash
koto decisions list <name>
```

Returns decisions for the current epoch only. After `koto rewind`, the count resets to 0.

### koto context add

Store content under a key in the session's context store.

```bash
koto context add <session> <key> [--from-file <path>]
echo "content" | koto context add <session> <key>
```

When `--from-file` is absent, reads from stdin. Overwrites existing content at that key. Keys are hierarchical path strings (e.g., `scope.md`, `research/r1/lead.md`). Keys must not start with `.` or contain `..`.

### koto context get

Retrieve stored content.

```bash
koto context get <session> <key> [--to-file <path>]
```

Writes to stdout, or to `--to-file` if specified (parent directories created automatically).

### koto context exists

Check whether a key exists in the session's context store.

```bash
koto context exists <session> <key>
```

Exit 0 means present, exit 1 means absent. No stdout output. Designed for shell conditionals:

```sh
if koto context exists my-workflow scope.md; then
  koto context get my-workflow scope.md | process_scope
fi
```

### koto context list

List all context keys as a JSON array sorted alphabetically.

```bash
koto context list <session> [--prefix <prefix>]
```

`--prefix` filters to keys starting with the given prefix. Returns `[]` when empty.

### koto session dir

Print the absolute path of the session directory (plain text, not JSON).

```bash
koto session dir <name>
```

### koto session list

List all sessions as a JSON array sorted by `id`.

```bash
koto session list
```

Note: this command uses `id` where `koto workflows` uses `name`. Both refer to the same session identifier.

### koto session cleanup

Remove the entire session directory. Idempotent.

```bash
koto session cleanup <name>
```

Under normal operation, `koto next` auto-cleans on terminal state unless `--no-cleanup` was passed.

### koto template compile

Validate and compile a template source file.

```bash
koto template compile <source> [--allow-legacy-gates]
```

`koto init` runs this automatically. You don't usually need to call it directly.

### koto config

Configuration commands for environment setup. Most agents running on the default local backend need no configuration.

```
koto config get <key>
koto config set <key> <value> [--user]
koto config unset <key> [--user]
koto config list [--json]
```

## Template Setup

Before running a workflow, ensure the template file exists at a stable project-local path.

1. Check if the template already exists (e.g., at `.koto/templates/<name>.md`).
2. If not, create the directory and copy the template there:

```bash
mkdir -p .koto/templates
```

Then write the template file from wherever the skill or project provides it.

## Variable Substitution

Two variables are available in gate commands and state directives without any declaration:

| Token | Value |
|---|---|
| `{{SESSION_DIR}}` | Absolute path to the workflow's session directory |
| `{{SESSION_NAME}}` | The workflow name passed to `koto init` |

User-defined variables declared in the template's `variables:` block and supplied via `--var` use the same `{{KEY}}` syntax. Substitution is non-recursive.

## Response Shapes

Every `koto next` call returns JSON. The `action` field determines what to do. Dispatch on `action` alone.

### Field Presence by Action

| Field | evidence_required | gate_blocked | integration | integration_unavailable | done | confirm |
|---|---|---|---|---|---|---|
| `action` | always | always | always | always | always | always |
| `state` | always | always | always | always | always | always |
| `directive` | always | always | always | always | **absent** | always |
| `details` | conditional | conditional | conditional | conditional | **absent** | conditional |
| `advanced` | always | always | always | always | always | always |
| `expects` | always (object) | always (`null`) | object or `null` | object or `null` | always (`null`) | object or `null` |
| `blocking_conditions` | always (array) | always (array) | **absent** | **absent** | **absent** | **absent** |
| `integration` | **absent** | **absent** | always | always | **absent** | **absent** |
| `action_output` | **absent** | **absent** | **absent** | **absent** | **absent** | always |
| `error` | `null` | `null` | `null` | `null` | `null` | `null` |

### action: "evidence_required"

The state needs input from you. Three sub-cases exist:

**Sub-case A: Submit evidence directly.**
`blocking_conditions` is empty, `expects.fields` is non-empty. No gates are blocking. Submit evidence matching the schema.

```json
{
  "action": "evidence_required",
  "state": "review",
  "directive": "Check the output and submit your assessment.",
  "details": "Extended guidance shown on first visit only.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "outcome": {"type": "enum", "required": true, "values": ["approve", "reject", "defer"]},
      "notes": {"type": "string", "required": false}
    },
    "options": [
      {"target": "approved", "when": {"outcome": "approve"}},
      {"target": "rejected", "when": {"outcome": "reject"}}
    ]
  },
  "blocking_conditions": [],
  "error": null
}
```

Submit with:

```bash
koto next <name> --with-data '{"outcome": "approve"}'
```

`expects.options` shows which evidence values route to which target states. When `options` is absent, all evidence values lead to the same next state. `details` is omitted on repeat visits unless `--full` is passed.

**Sub-case B: Gates failed, evidence fallback available.**
`blocking_conditions` is non-empty, `expects.fields` is non-empty. You can either fix the gates and re-run, record an override (when `agent_actionable` is `true`), or submit evidence to proceed.

```json
{
  "action": "evidence_required",
  "state": "validate",
  "directive": "CI checks failed. Provide override evidence or fix the issue.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": {"type": "enum", "required": true, "values": ["completed", "override"]}
    }
  },
  "blocking_conditions": [
    {"name": "ci_check", "type": "command", "status": "failed", "agent_actionable": true, "output": {"exit_code": 1, "error": ""}}
  ],
  "error": null
}
```

**Sub-case C: Auto-advance candidate.**
`blocking_conditions` is empty, `expects.fields` is empty (`{}`). Call `koto next` again without `--with-data` to let it auto-advance. Rarely seen because the engine's advance loop usually handles these before returning.

### action: "gate_blocked"

Gates failed on a state with no `accepts` block. Fix the blocking conditions and call `koto next` again.

```json
{
  "action": "gate_blocked",
  "state": "deploy",
  "directive": "Waiting for CI to pass before proceeding.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {"name": "ci_check", "type": "command", "status": "failed", "agent_actionable": true, "output": {"exit_code": 1, "error": ""}}
  ],
  "error": null
}
```

Possible `status` values: `"failed"`, `"timed_out"`, `"error"`. Passing gates don't appear in the array.

Gate output shapes by type:
- `command` gates: `{"exit_code": <int>, "error": "<string>"}`
- `context-exists` gates: `{"exists": false, "error": "<string>"}`

When `agent_actionable` is `true`, you can override:

```bash
koto overrides record <name> --gate ci_check --rationale "verified manually"
koto next <name>
```

When `agent_actionable` is `false`, escalate to the user. Don't poll in a loop.

### action: "integration"

An integration runner executed and returned output.

```json
{
  "action": "integration",
  "state": "run_tests",
  "directive": "Review the test results and proceed.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {"confirmed": {"type": "boolean", "required": true}}
  },
  "integration": {"name": "test-runner", "output": {"passed": 42, "failed": 0}},
  "error": null
}
```

`expects` may be `null` when the state has no `accepts` block.

### action: "integration_unavailable"

The template declares an integration, but no runner is configured.

```json
{
  "action": "integration_unavailable",
  "state": "run_tests",
  "directive": "Run the test suite and report results.",
  "advanced": false,
  "expects": null,
  "integration": {"name": "test-runner", "available": false},
  "error": null
}
```

Report to the user. The template requires an integration that hasn't been set up.

### action: "done"

The workflow reached a terminal state. Stop.

```json
{
  "action": "done",
  "state": "complete",
  "advanced": true,
  "expects": null,
  "error": null
}
```

`directive` is **absent** (not `null`). `details` is also absent. After `done`, the session directory is cleaned up automatically unless `--no-cleanup` was passed.

### action: "confirm"

A default action ran and needs your review before the engine records it.

```json
{
  "action": "confirm",
  "state": "deploy",
  "directive": "Review the deployment output and confirm.",
  "advanced": false,
  "action_output": {
    "command": "deploy.sh",
    "exit_code": 0,
    "stdout": "Deployed to staging.\n",
    "stderr": ""
  },
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {"confirmed": {"type": "boolean", "required": true}}
  },
  "error": null
}
```

`action_output.stdout` and `action_output.stderr` are truncated to 64 KB each.

## The `details` Field

Template authors split state content using a `<!-- details -->` marker: text before becomes `directive` (always returned), text after becomes `details`.

`details` is included on first visit to a state and omitted on subsequent visits. Use `--full` to force inclusion:

```bash
koto next <name> --full
```

The field is absent (not `null`) when omitted and also absent when the template state has no details content.

## The `advanced` Field

A boolean indicating that at least one state transition occurred during this `koto next` call. Informational only. Don't dispatch on it.

- Evidence submission triggers a transition: `advanced: true`
- Gates pass and the engine auto-advances: `advanced: true`
- State is waiting for evidence on first query: `advanced: false`
- Gates still blocking on re-query: `advanced: false`

## Error Responses

### Two error shapes

**koto next** writes structured error JSON to **stdout**:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence failed validation",
    "details": [{"field": "mode", "reason": "required field missing"}]
  }
}
```

**All other subcommands** write flat error JSON to **stderr**:

```json
{"error": "workflow 'my-workflow' already exists", "command": "init"}
```

### Exit codes

| Code | Meaning | What to do |
|---|---|---|
| 0 | Success | Parse and act on JSON output |
| 1 | Transient | Wait for the condition to resolve, then retry |
| 2 | Caller error | Fix the request; don't retry without changing input |
| 3 | Infrastructure error | Report to user; don't retry |

### Error codes (koto next)

| `error.code` | Exit | Retryable | Agent action |
|---|---|---|---|
| `gate_blocked` | 1 | Yes | Wait for external condition, then retry |
| `integration_unavailable` | 1 | Yes | Report to user |
| `concurrent_access` | 1 | Yes | Wait briefly, then retry |
| `invalid_submission` | 2 | No | Check `error.details` for per-field reasons; fix `--with-data` payload |
| `precondition_failed` | 2 | No | Read the error message; workflow state must change before retrying |
| `terminal_state` | 2 | No | Stop; start a new workflow if needed |
| `workflow_not_initialized` | 2 | No | Run `koto init` first, or check the name |
| `template_error` | 3 | No | Report to user |
| `persistence_error` | 3 | No | Report to user |

Note: `gate_blocked` and `integration_unavailable` appear both as error codes and as action values. Check the exit code to distinguish them.

### invalid_submission details

When `error.code` is `invalid_submission`, the `details` array has per-field errors:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence failed validation",
    "details": [
      {"field": "status", "reason": "value 'done' is not in allowed values [completed, override]"},
      {"field": "priority", "reason": "unknown field"}
    ]
  }
}
```

Fix each field, then resubmit. If you no longer have the schema, call `koto next <name>` without `--with-data` to get the current `expects`.

## Execution Loop

Every koto workflow follows the same pattern: init, get directive, act on the response action, repeat.

### Worked example: basic init and evidence submission

**1. Initialize:**

```bash
koto init my-flow --template .koto/templates/my-workflow.md --var MODE=new
```

```json
{"name": "my-flow", "state": "entry"}
```

**2. Get directive:**

```bash
koto next my-flow
```

Response includes `action: "evidence_required"` and an `expects` field with the evidence schema.

**3. Submit evidence:**

```bash
koto next my-flow --with-data '{"mode_confirmed": "new"}'
```

The engine evaluates evidence and advances to the next state.

**4. Repeat** — call `koto next my-flow` again to get the next directive.

### Worked example: branching, gates, and decisions

A more involved workflow with multiple paths and gate handling.

**1. Initialize with variables:**

```bash
koto init issue-74 --template .koto/templates/work-on.md \
  --var ARTIFACT_PREFIX=issue_74 \
  --var ISSUE_NUMBER=74
```

```json
{"name": "issue-74", "state": "entry"}
```

**2. Submit mode selection:**

```bash
koto next issue-74
koto next issue-74 --with-data '{"mode": "issue_backed", "issue_number": "74"}'
```

The engine routes to `context_injection` based on `mode: issue_backed` and continues advancing through gates.

**3. Handle gate-blocked or evidence-required responses:**

If gates fail on a state without `accepts`, you get `action: "gate_blocked"`. Fix the condition and call `koto next issue-74` again.

If gates fail on a state with `accepts`, you get `action: "evidence_required"` with a non-empty `blocking_conditions` array. Fix the conditions first, then submit evidence.

**4. Submit evidence at analysis:**

```bash
koto next issue-74
koto next issue-74 --with-data '{"plan_outcome": "plan_ready", "approach_summary": "Refactor the parser"}'
```

**5. Record a decision during implementation:**

```bash
koto decisions record issue-74 --with-data '{"choice": "Used visitor pattern", "rationale": "Separates traversal from processing", "alternatives_considered": ["Recursive descent", "Iterator-based"]}'
```

This doesn't advance the workflow. It records the decision in the event log.

**6. Submit completion:**

```bash
koto next issue-74 --with-data '{"implementation_status": "complete", "rationale": "All changes committed, tests passing"}'
```

The engine advances through remaining states and reaches `done`.

## Error Handling

| Situation | What to do |
|---|---|
| `koto` not found | Tell the user to install koto and add it to PATH. |
| Template not found | Verify the path. Copy the template to `.koto/templates/` if missing. |
| Gate blocked (exit 1) | Read `blocking_conditions`. Fix the issue and call `koto next` again. |
| Invalid submission (exit 2) | Check `error.details` for per-field errors. Fix the JSON and resubmit. |
| Terminal state (exit 2) | The workflow is done. Don't call `koto next --with-data` on it. |
| Template error (exit 3) | Report to user. The template has a structural problem. |
| Persistence error (exit 3) | Report to user. Disk I/O failed. |
| Concurrent access (exit 1) | Another `koto next` process is running. Wait and retry. |
| Session already exists | A previous workflow with this name is active. Run `koto workflows` to check. Cancel with `koto cancel <name>` if needed, then re-init. |
| `agent_actionable: false` | Can't override. Escalate to the user. Don't poll. |

## Resuming a Workflow

koto preserves state across interruptions. To resume:

1. Run `koto workflows` to find active workflows and their current states.
2. Run `koto next <name>` to get the current directive.
3. Continue from wherever the workflow left off.

If the workflow is stuck in a blocking state that has been resolved externally, use `koto rewind <name>` to walk back and retry.
