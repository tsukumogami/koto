# CLI Usage Guide

koto's CLI is the primary interface for managing workflow state. Agent-facing commands (`init`, `next`, `transition`, `query`, `rewind`, `workflows`) output JSON. Human-facing commands (`status`, `cancel`, `validate`) output plain text.

All commands exit with code 0 on success and non-zero on failure. Errors are always printed to stdout as JSON:

```json
{"error":{"code":"invalid_transition","message":"cannot transition from \"assess\" to \"done\": not in allowed transitions [feedback]","current_state":"assess","target_state":"done","valid_transitions":["feedback"]}}
```

## State file resolution

Most commands need to know which state file to operate on. There are three ways this gets resolved:

1. **Explicit path**: Pass `--state path/to/koto-name.state.json` to target a specific file.
2. **Auto-selection**: When exactly one `koto-*.state.json` file exists in the state directory, koto uses it automatically.
3. **State directory**: Pass `--state-dir path/` to change where koto looks. Defaults to `wip/`.

If multiple state files exist and no `--state` flag is given, koto fails with a message listing the available files.

## Commands

### init

Creates a new workflow from a template file.

```bash
koto init --name my-workflow --template path/to/template.md
```

**Required flags:**
- `--name` -- Workflow name. Used in the state file name (`koto-<name>.state.json`).
- `--template` -- Path to the workflow template file.

**Optional flags:**
- `--state-dir` -- Directory for the state file. Defaults to `wip/`. Created if it doesn't exist.
- `--var KEY=VALUE` -- Set a template variable. Can be repeated. Overrides template defaults.

**Output (JSON):**

```json
{"state":"assess","path":"wip/koto-my-workflow.state.json"}
```

**Example with variables:**

```bash
koto init --name task-42 --template workflow.md --var TASK="Fix login bug" --var BRANCH=fix/login
```

### next

Returns the directive for the current state. This is the main agent-facing command -- it tells the agent what to do.

```bash
koto next
koto next --state wip/koto-my-workflow.state.json
```

For non-terminal states, returns the interpolated template section:

```json
{"action":"execute","state":"assess","directive":"Assess the task: Fix login bug."}
```

For terminal states, returns a done signal:

```json
{"action":"done","state":"done","message":"workflow complete"}
```

The `next` command verifies template integrity before returning. If the template file changed since `init`, it fails with `template_mismatch`.

### transition

Advances to a target state. The target must be in the current state's allowed transitions list.

```bash
koto transition plan
koto transition plan --state wip/koto-my-workflow.state.json
```

**Output (JSON):**

```json
{"state":"plan","version":2}
```

The version counter increments with every state change. Transition verifies template integrity and checks for version conflicts before writing.

### query

Returns the full workflow state as JSON. Useful for programmatic inspection.

```bash
koto query
koto query --state wip/koto-my-workflow.state.json
```

**Output (JSON):**

```json
{
  "schema_version": 1,
  "workflow": {
    "name": "my-workflow",
    "template_hash": "sha256:e3b0c44...",
    "template_path": "/abs/path/to/template.md",
    "created_at": "2026-02-22T10:00:00Z"
  },
  "version": 3,
  "current_state": "implement",
  "variables": {
    "TASK": "Fix login bug"
  },
  "history": [
    {"from": "assess", "to": "plan", "timestamp": "2026-02-22T10:01:00Z", "type": "transition"},
    {"from": "plan", "to": "implement", "timestamp": "2026-02-22T10:02:00Z", "type": "transition"}
  ]
}
```

### status

Prints a human-readable summary. This is the quick-check command when you want to see where things stand.

```bash
koto status
koto status --state wip/koto-my-workflow.state.json
```

**Output (text):**

```
Workflow: my-workflow
State:    implement
History:  2 entries
```

### rewind

Resets the workflow to a previously visited state. The target must appear in the transition history (as a destination), or be the machine's initial state. You can't rewind to a terminal state.

```bash
koto rewind --to assess
koto rewind --to assess --state wip/koto-my-workflow.state.json
```

**Required flags:**
- `--to` -- The state to rewind to.

**Output (JSON):**

```json
{"state":"assess","version":4}
```

Rewind preserves the full history. A rewind entry is appended (not truncated), so you can trace what happened:

```json
{"from": "implement", "to": "assess", "timestamp": "2026-02-22T10:05:00Z", "type": "rewind"}
```

You can rewind from a terminal state -- this is the recovery path when a workflow reaches an undesired end state.

### cancel

Deletes the state file, abandoning the workflow. This is irreversible.

```bash
koto cancel
koto cancel --state wip/koto-my-workflow.state.json
```

**Output (text):**

```
workflow canceled
```

### validate

Checks that the template file's hash matches the one stored in the state file. Useful for debugging `template_mismatch` errors.

```bash
koto validate
koto validate --state wip/koto-my-workflow.state.json
```

**Output on success (text):**

```
OK: template hash matches
```

**Output on failure (JSON error):**

```json
{"error":{"code":"template_mismatch","message":"template hash mismatch: state file has \"sha256:abc...\" but template on disk is \"sha256:def...\""}}
```

### workflows

Lists all active workflows in the state directory.

```bash
koto workflows
koto workflows --state-dir wip/
```

**Optional flags:**
- `--state-dir` -- Directory to scan. Defaults to `wip/`.

**Output (JSON):**

```json
[
  {"path":"wip/koto-task-1.state.json","name":"task-1","current_state":"implement","template_path":"/abs/path/template.md","created_at":"2026-02-22T10:00:00Z"},
  {"path":"wip/koto-task-2.state.json","name":"task-2","current_state":"assess","template_path":"/abs/path/template.md","created_at":"2026-02-22T10:05:00Z"}
]
```

Returns an empty array `[]` when no workflows are active.

### template

The `template` subcommand group contains authoring tools for template development. These commands aren't needed for running workflows -- they're for people writing and debugging templates.

#### template compile

Compiles a source template and writes the compiled JSON to stdout. Warnings go to stderr. Exits non-zero on compilation failure.

```bash
koto template compile path/to/template.md
```

**Positional argument:**
- `<path>` -- Path to the source template file.

**Optional flags:**
- `--output <file>` -- Write compiled JSON to a file instead of stdout.

**Output (JSON to stdout):**

The compiled JSON representation of the template. Pipe to `jq` to explore specific fields:

```bash
koto template compile template.md | jq '.states'
```

**Warnings (stderr):**

```
warning: heading collision: "assess" appears twice
```

**Example with output file:**

```bash
koto template compile template.md --output compiled.json
```

Exit codes:
- `0` -- Compilation succeeded.
- Non-zero -- Compilation failed. Error details are printed as JSON to stdout.

This command always compiles fresh (it doesn't use caching). It's meant for the edit-compile-check loop during template development, and for CI validation of template files.

### version

Prints the koto version.

```bash
koto version
```

```
koto v0.1.0
```

## Typical agent workflow

The standard loop for an AI agent:

```bash
# Initialize from a template
koto init --name task-42 --template workflow.md --var TASK="Implement retry logic"

# Main loop
while true; do
  directive=$(koto next)
  action=$(echo "$directive" | jq -r '.action')

  if [ "$action" = "done" ]; then
    break
  fi

  # Agent does the work described in .directive
  # ...

  # Get available transitions from the template and advance
  koto transition <next-state>
done
```

If something goes wrong mid-workflow:

```bash
# Check where you are
koto status

# Roll back and retry
koto rewind --to plan

# Or abandon entirely
koto cancel
```
