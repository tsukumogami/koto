# CLI Usage Guide

koto's CLI manages workflow state for AI coding agents. All commands output JSON. All commands exit with code 0 on success and non-zero on failure. Errors are printed to stdout as JSON:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

## State file resolution

Each workflow has a state file named `koto-<name>.state.jsonl` in the current directory. The file uses JSONL format — one JSON event per line. The current state is the `state` field of the last event.

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

Exits non-zero if a workflow with that name already exists or if the template is invalid.

### next

Returns the directive for the current state. This is the main agent-facing command -- it tells the agent what to do next.

```bash
koto next <name>
```

**Output (JSON):**

```json
{"state":"assess","directive":"Review the PR at https://github.com/org/repo/pull/42 and summarize the changes.","transitions":["feedback"]}
```

The `transitions` array lists the states reachable from the current state. Exits non-zero if the workflow isn't found.

### rewind

Rolls back the workflow to the previous state by appending a rewind event to the JSONL file.

```bash
koto rewind <name>
```

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

Exits non-zero if the workflow is already at the initial state (only one event exists). Rewind is non-destructive -- it appends a new event rather than truncating history, so the full event log is preserved.

### workflows

Lists all active workflows in the current directory.

```bash
koto workflows
```

**Output (JSON):**

```json
["my-workflow","task-42"]
```

Returns an empty array `[]` when no workflows are found.

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
  transitions=$(echo "$result" | jq -r '.transitions[]')

  # Agent does the work described in .directive
  # ...

  # Check if we're done
  if [ -z "$transitions" ]; then
    break
  fi
done
```

To roll back after an unexpected result:

```bash
# Rewind to the previous state
koto rewind task-42
```

> **Note:** Workflow advancement (`koto transition`) is not available in this release. Transitions will be added in a future version.
