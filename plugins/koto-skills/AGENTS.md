# koto Workflow Agent Instructions

> **Placement**: Copy this file to the root of your project repository. Codex and
> Windsurf discover agent instructions from `AGENTS.md` at the repo root. Other
> platforms that read `AGENTS.md` will also pick it up.

## What is koto?

koto is a state machine engine for AI agent workflows. It enforces workflow
progression through evidence-gated transitions. You call `koto` commands to
initialize a workflow, get directives, submit evidence, and advance between states.

## Prerequisites

- `koto` must be installed and on PATH
- Run `koto version` to verify; if missing, install from https://github.com/tsukumogami/koto

## Command Reference

### koto init

Initialize a new workflow from a template.

```
koto init <name> --template <path> [--var KEY=VALUE ...]
```

- `<name>` is a positional argument (alphanumeric, dots, hyphens, underscores)
- `--template` is the path to a template file (compiled and cached automatically)
- `--var` sets a template variable; repeatable for multiple variables

Returns JSON on success:

```json
{"name": "hello", "state": "awakening"}
```

### koto next

The primary command for workflow interaction. It does three things depending on
flags:

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

**Force full details** (with `--full`):

```bash
koto next <name> --full
```

The `--full` flag includes the `details` field regardless of visit count (see
the `details` field section below).

`--with-data` and `--to` are mutually exclusive.

### koto decisions record

Record a structured decision without advancing state. Useful for capturing
judgment calls during long-running states like implementation.

```bash
koto decisions record <name> --with-data '{"choice": "...", "rationale": "...", "alternatives_considered": ["..."]}'
```

The `choice` and `rationale` fields are required. `alternatives_considered` is optional.

### koto decisions list

List all decisions recorded for the current state.

```bash
koto decisions list <name>
```

Returns a JSON array of decision objects.

### koto rewind

Roll back to the previous state. Call repeatedly to rewind multiple steps.

```bash
koto rewind <name>
```

Returns JSON with the new state:

```json
{"name": "myworkflow", "state": "analysis"}
```

Cannot rewind past the initial state.

### koto cancel

Cancel a workflow, preventing further advancement.

```bash
koto cancel <name>
```

### koto workflows

List all active workflows in the current directory.

```bash
koto workflows
```

### koto template compile

Validate and compile a template source file.

```bash
koto template compile <source>
```

## Template Setup

Workflow templates define the states, transitions, and gates for a koto workflow.
Before running a workflow, ensure the template file exists at a stable project-local
path.

For any koto-skills workflow:

1. Check if the template already exists at a stable project-local path (e.g., `.koto/templates/<name>.md`).
2. If not, create the directory and copy the template there:

```bash
mkdir -p .koto/templates
```

Then write the template file from the skill's `koto-templates/` directory. The skill's
SKILL.md will specify the exact template path via `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md`.

## Response Shapes

Every `koto next` call returns JSON. The `action` field tells you what to do.
Each action value maps to exactly one response shape -- dispatch on `action` alone.

### action: "evidence_required"

The state has an `accepts` block. Execute the directive, then submit evidence
matching the `expects` schema.

```json
{
  "action": "evidence_required",
  "state": "entry",
  "directive": "Determine the workflow mode...",
  "details": "### Steps\n\n1. Check if an issue number was provided...",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "mode": {"type": "enum", "required": true, "values": ["issue_backed", "free_form"]},
      "issue_number": {"type": "string", "required": false}
    },
    "options": [
      {"target": "context_injection", "when": {"mode": "issue_backed"}},
      {"target": "task_validation", "when": {"mode": "free_form"}}
    ]
  },
  "blocking_conditions": [],
  "error": null
}
```

The `expects` object tells you exactly what evidence to submit:
- `fields` lists each field with its type, whether it's required, and allowed values for enums
- `options` shows how your evidence values map to target states

Submit evidence using `--with-data` with a JSON object whose keys match the field names.

**When gates fail on a state with accepts**: The response is still
`"evidence_required"`, but the `blocking_conditions` array is populated with
the failing gates. Fix the blocking conditions first, then call `koto next`
again -- the engine re-evaluates gates automatically. Once gates pass, you can
submit evidence normally.

```json
{
  "action": "evidence_required",
  "state": "analysis",
  "directive": "Analyze the issue and create a plan.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "plan_outcome": {"type": "enum", "required": true, "values": ["plan_ready", "needs_research"]}
    }
  },
  "blocking_conditions": [
    {"name": "baseline_exists", "type": "command", "status": "failed", "agent_actionable": false}
  ],
  "error": null
}
```

When `blocking_conditions` is empty, there are no gate issues -- proceed directly
with the work described in `directive`.

### action: "gate_blocked"

Gates failed on a state that doesn't accept evidence. The directive tells you what
to do. Fix the blocking conditions and call `koto next` again.

```json
{
  "action": "gate_blocked",
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

Possible `status` values: `"failed"`, `"timed_out"`, `"error"`. Passing gates
don't appear in the array. Don't submit evidence -- fix the preconditions and
re-query.

### action: "integration"

The state declares an integration, and the runner executed it. The `integration`
object contains the output. If the state also accepts evidence, `expects` will
be present.

```json
{
  "action": "integration",
  "state": "delegate",
  "directive": "Review the integration output.",
  "advanced": false,
  "expects": null,
  "integration": {"name": "code_review", "available": true, "output": "..."},
  "error": null
}
```

### action: "integration_unavailable"

The state declares an integration, but the runner isn't available. The
`integration` object shows `available: false`. Proceed with the directive
manually.

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

### action: "done"

The workflow reached a terminal state. No further action needed.

```json
{
  "action": "done",
  "state": "done",
  "advanced": true,
  "expects": null,
  "error": null
}
```

### action: "confirm"

A default action ran and needs your review before the engine records its result.
Check the `action_output` and submit evidence if the state accepts it.

```json
{
  "action": "confirm",
  "state": "context_injection",
  "directive": "...",
  "advanced": false,
  "action_output": {
    "command": "extract-context.sh --issue 42",
    "exit_code": 0,
    "stdout": "...",
    "stderr": ""
  },
  "expects": { "..." : "..." },
  "error": null
}
```

## The `details` Field

The `details` field carries extended instructions for a state. Template authors
split state content using a `<!-- details -->` marker: text before the marker
becomes `directive` (always returned), text after becomes `details`.

`details` is included on first visit to a state and omitted on subsequent visits.
This avoids repeating lengthy instructions that the caller already has. Use the
`--full` flag to force inclusion regardless of visit count:

```bash
koto next <name> --full
```

The field is absent (not `null`) when omitted. It's also absent when the template
state has no details content.

## The `advanced` Field

`advanced` is a boolean indicating that at least one state transition occurred
during this invocation of `koto next`. It's informational only -- dispatch on
`action`, not on `advanced`.

Examples:
- Evidence submission triggers a transition: `advanced: true`
- Gates pass and the engine auto-advances: `advanced: true`
- State is waiting for evidence on first query: `advanced: false`
- Gates are still blocking on a re-query: `advanced: false`

## Error Responses

Errors include a structured `error` object with a `code`, `message`, and
`details` array. The process exit code signals the error category:

| Exit code | Error codes | Agent action |
|-----------|-------------|--------------|
| 0 | (success) | Process the response |
| 1 | `gate_blocked`, `integration_unavailable`, `concurrent_access` | Retry after fixing or wait |
| 2 | `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized` | Change your approach |
| 3 | `template_error`, `persistence_error` | Report to user |

**Error code details:**

| Code | Exit | Meaning |
|------|------|---------|
| `gate_blocked` | 1 | Gate preconditions not met. Fix and retry. |
| `integration_unavailable` | 1 | Integration runner missing. Proceed manually or retry later. |
| `concurrent_access` | 1 | Another `koto next` is already running on this workflow. Wait and retry. |
| `invalid_submission` | 2 | Evidence doesn't match the `expects` schema. Check the `details` array for per-field errors. |
| `precondition_failed` | 2 | Command flags are invalid (e.g., `--with-data` and `--to` together). |
| `terminal_state` | 2 | Workflow is already done. No further action possible. |
| `workflow_not_initialized` | 2 | No workflow with that name exists. |
| `template_error` | 3 | Template is malformed: cycle detected, chain limit reached, ambiguous transition, dead-end state, unresolvable transition, or unknown state. |
| `persistence_error` | 3 | Disk I/O failure reading or writing state. |

Example error (exit code 2):

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence validation failed",
    "details": [{"field": "mode", "reason": "required field missing"}]
  }
}
```

## Execution Loop

Every koto workflow follows the same pattern: init, get directive, act on the
response action, repeat.

### Simple example: koto-author entry state

The koto-author workflow starts at `entry`, where the agent confirms the authoring
mode. This shows the basic init + evidence submission loop.

**1. Initialize:**

```bash
koto init authoring --template .koto/templates/koto-author.md --var MODE=new
```

```json
{"name": "authoring", "state": "entry"}
```

**2. Get directive:**

```bash
koto next authoring
```

The response includes `action: "evidence_required"` and an `expects` field with
the evidence schema.

**3. Submit evidence:**

```bash
koto next authoring --with-data '{"mode_confirmed": "new"}'
```

The engine evaluates evidence and advances to `context_gathering`.

### Advanced example: work-on workflow

The work-on template handles issue-backed and free-form implementation tasks.
It has branching paths, evidence submission, gate checks, and decisions.

**1. Initialize with variables:**

```bash
koto init issue-74 --template .koto/templates/work-on.md \
  --var ARTIFACT_PREFIX=issue_74 \
  --var ISSUE_NUMBER=74
```

```json
{"name": "issue-74", "state": "entry"}
```

**2. Get directive and submit evidence at entry:**

```bash
koto next issue-74
```

The response includes `action: "evidence_required"` with the evidence schema.
Submit mode selection:

```bash
koto next issue-74 --with-data '{"mode": "issue_backed", "issue_number": "74"}'
```

The engine evaluates your evidence, routes to `context_injection` based on
`mode: issue_backed`, and continues advancing through gates.

**3. Handle gate-blocked or evidence-required responses:**

If `context_injection` gates pass, the engine auto-advances. If gates fail:

- On a state without `accepts`: you get `action: "gate_blocked"`. Fix the
  condition and call `koto next issue-74` again.
- On a state with `accepts`: you get `action: "evidence_required"` with a
  populated `blocking_conditions` array. Fix the conditions first, then submit
  evidence when gates pass.

When the state has an `accepts` block, submit evidence:

```bash
koto next issue-74 --with-data '{"status": "completed"}'
```

**4. Submit evidence at analysis:**

After the engine reaches `analysis`, get the directive and create a plan:

```bash
koto next issue-74
```

Write the plan file, then submit:

```bash
koto next issue-74 --with-data '{"plan_outcome": "plan_ready", "approach_summary": "Refactor the parser to handle nested templates"}'
```

**5. Record a decision during implementation:**

While working in the `implementation` state, capture a non-obvious judgment call:

```bash
koto decisions record issue-74 --with-data '{"choice": "Used visitor pattern instead of recursive descent", "rationale": "Visitor separates traversal from processing, making it easier to add new node types", "alternatives_considered": ["Recursive descent", "Iterator-based"]}'
```

This doesn't advance the workflow -- it just records the decision in the event log.

**6. List decisions:**

```bash
koto decisions list issue-74
```

```json
[
  {
    "choice": "Used visitor pattern instead of recursive descent",
    "rationale": "Visitor separates traversal from processing, making it easier to add new node types",
    "alternatives_considered": ["Recursive descent", "Iterator-based"]
  }
]
```

**7. Submit completion:**

```bash
koto next issue-74 --with-data '{"implementation_status": "complete", "rationale": "All changes committed, tests passing"}'
```

The engine advances through `finalization`, `pr_creation`, `ci_monitor`, and
finally reaches `done`.

## Error Handling

- **koto not found**: Tell the user to install koto and add it to PATH.
- **Template not found**: Verify the template path. Copy the template to
  `.koto/templates/` if it's missing.
- **Gate blocked** (exit code 1): The state's preconditions aren't met. Read the
  `blocking_conditions` array to understand what failed. Fix the issue and call
  `koto next` again.
- **Invalid submission** (exit code 2): Your evidence doesn't match the `expects`
  schema. Check the `details` array for per-field errors. Fix the evidence JSON
  and resubmit.
- **Terminal state** (exit code 2): You called `koto next --with-data` on a
  terminal state. The workflow is already done.
- **Template error** (exit code 3): The template has a structural problem (cycle,
  dead-end, ambiguous transition). Report to the user -- this isn't fixable by
  the agent.
- **Persistence error** (exit code 3): Disk I/O failed. Report to the user.
- **Concurrent access** (exit code 1): Another `koto next` process is running on
  this workflow. Wait a moment and retry.
- **State file already exists**: A previous workflow with the same name is active.
  Run `koto workflows` to check. Cancel with `koto cancel <name>` if needed,
  then re-init.

## Resume

If a session is interrupted mid-workflow:

1. Run `koto workflows` to find active workflows and their current states.
2. Run `koto next <name>` to get the current directive.
3. Continue from wherever the workflow left off.

State files persist between sessions. The workflow resumes from the last
completed transition. If the workflow is stuck in a blocking state that has been
resolved externally, use `koto rewind <name>` to walk back to a previous state
and try again.
