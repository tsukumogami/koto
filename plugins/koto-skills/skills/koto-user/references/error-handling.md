# koto Error Handling

This file covers exit codes, error shapes, and how to respond to each category of error.

---

## Exit codes

| Code | Meaning | Agent response |
|---|---|---|
| 0 | Success | Parse and act on the JSON output |
| 1 | Transient — retry when the condition resolves | Wait, then retry; or report to user if externally blocked |
| 2 | Caller error — the agent must change behavior | Fix the request; do not retry without changing input |
| 3 | Infrastructure error — corrupted or misconfigured | Report to user; do not retry automatically |

---

## Two distinct error shapes

koto uses different JSON shapes depending on which command failed.

### Shape 1: koto next domain errors (structured)

`koto next` writes structured error JSON to **stdout** when a domain error occurs:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence failed validation",
    "details": [
      {"field": "status", "reason": "value 'unknown' is not in allowed values"}
    ]
  }
}
```

Fields:
- `error.code` — snake_case error code string (see table below)
- `error.message` — human-readable explanation
- `error.details` — array of per-field errors; always present but may be empty `[]`;
  populated only for `invalid_submission`

### Shape 2: all other subcommands (flat)

Every command other than `koto next` writes a flat error JSON to **stderr** on failure:

```json
{"error": "workflow 'my-workflow' already exists", "command": "init"}
```

Fields:
- `error` — human-readable error string
- `command` — the subcommand name that failed

---

## NextErrorCode table

All `koto next` error codes, their exit codes, and what to do:

| `error.code` | Exit | Retryable | Meaning | Agent action |
|---|---|---|---|---|
| `gate_blocked` | 1 | Yes | One or more gates failed; state has no `accepts` block | Wait for the external condition to change, then retry |
| `integration_unavailable` | 1 | Yes | Integration runner is not configured | Report to user; cannot be resolved by the agent alone |
| `concurrent_access` | 1 | Yes | Another `koto next` call is already running | Wait briefly, then retry |
| `invalid_submission` | 2 | No | Evidence failed schema validation | Check `error.details` for per-field reasons; fix the `--with-data` payload |
| `precondition_failed` | 2 | No | Caller violated a precondition | Read the error message; the workflow state must change before retrying |
| `terminal_state` | 2 | No | Workflow is already in a terminal state (done or cancelled) | Stop; start a new workflow if needed |
| `workflow_not_initialized` | 2 | No | Named workflow does not exist | Run `koto init` first, or check the workflow name |
| `template_error` | 3 | No | Template parse failure, hash mismatch, or cycle detected | Report to user; this requires human intervention |
| `persistence_error` | 3 | No | State file I/O failure or corruption | Report to user; this is an infrastructure problem |

Note: `gate_blocked` and `integration_unavailable` appear both as `error.code` values
(when `koto next` produces an error response) and as `action` values (when `koto next`
produces a successful response). The successful response shape includes `blocking_conditions`
detail; the error shape does not. Check the exit code to distinguish them.

---

## Handling agent_actionable: false

When `koto next` returns `action: "gate_blocked"` or `action: "evidence_required"` with
a non-empty `blocking_conditions`, check each item's `agent_actionable` field.

**When `agent_actionable: true`:**
The gate has a configured `override_default` value or a built-in default for its type.
The agent can call `koto overrides record` to record an override and unblock the gate:

```
koto overrides record my-workflow --gate ci_check --rationale "verified manually"
```

After recording the override, the next `koto next` call treats that gate as passed.

**When `agent_actionable: false`:**
The gate has no override default. The agent cannot resolve this condition. The right
response is to surface the blocking condition to the user with enough context for them
to act:

- Quote the gate name and its `output` field from `blocking_conditions`
- Explain what the gate checks (from the `directive` text and gate `type`)
- Wait for the user to resolve the external condition before calling `koto next` again

Do not poll `koto next` in a loop when `agent_actionable: false`. The condition is
externally controlled and will not change without user action.

---

## invalid_submission — reading per-field errors

When `error.code` is `invalid_submission` (exit 2), the `error.details` array contains
one entry per field that failed validation:

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

Fix each field according to its `reason`, then resubmit with `koto next --with-data`.
The `expects.fields` from the previous successful `koto next` call shows the schema.
If you no longer have that output, call `koto next <name>` without `--with-data` to
get the current state's `expects` schema again.

---

## terminal_state after cancel

After `koto cancel`, all subsequent `koto next` calls return exit 2 with
`error.code = "terminal_state"`. This is expected and not a bug. Use `koto workflows`
to confirm the workflow is no longer listed as active (it will not appear after cleanup).

---

## Checking for errors in shell

Since exit code 0 means success across all subcommands, the simplest check is:

```sh
output=$(koto next my-workflow)
if [ $? -ne 0 ]; then
  echo "koto next failed: $output" >&2
  exit 1
fi
action=$(echo "$output" | jq -r '.action')
```

For `koto next`, parse both the exit code and the `action` field. Exit 0 with
`action: "gate_blocked"` is a successful response indicating a blocked state — it is
not an error. Exit 1 from `koto next` means the `error.code` field explains why.

---

## Typed error envelope (batch tick errors)

Batch-scoped ticks (a parent state with `materialize_children`, or a `retry_failed` submission) can fail with a structured envelope that sits alongside the six domain codes above. The wire shape uses a dedicated `action: "error"` variant with a sibling `error.batch` field:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "message": "...",
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": {"reason": "duplicate_names", "duplicates": ["task-1"]}
    }
  }
}
```

`error.batch` carries a typed `BatchError` variant — agents can dispatch on `batch.kind` instead of string-matching on `message`. All variants use snake_case discriminators.

### Top-level enum families

| Family | Shape | Purpose |
|---|---|---|
| `BatchError` | `{kind, ...}` under `error.batch` | Top-level variant — one of `concurrent_tick`, `invalid_batch_definition`, `limit_exceeded`, `template_not_found`, `template_compile_failed`, `backend_error`, `spawn_failed`, `invalid_retry_request` |
| `InvalidBatchReason` | nested under `invalid_batch_definition.reason` | Structural rejection: `empty_task_list`, `cycle`, `dangling_refs`, `duplicate_names`, `spawned_task_mutated`, `invalid_name`, `reserved_name_collision` |
| `InvalidRetryReason` | nested under `invalid_retry_request.reason` | Retry-submission rejection: `no_batch_materialized`, `empty_child_list`, `child_not_eligible`, `unknown_children`, `child_is_batch_parent`, `retry_already_in_progress`, `mixed_with_other_evidence`, `multiple_reasons` |
| `LimitKind` | under `limit_exceeded.which` | Hard limit that tripped: `tasks`, `waits_on`, `depth`, `payload_bytes` |
| `SpawnErrorKind` | under `spawn_failed.spawn_kind` | Per-task scheduler spawn error classification |
| `CompileErrorKind` | under `template_compile_failed.compile_error` | Typed child-template compile failure |
| `ChildOutcome` | under `child_not_eligible.children[*].current_outcome` | Retryability classification — `failure`, `skipped`, `spawn_failed`, `pending`, `success`, `blocked` |

### InvalidRetryReason precedence

When a `retry_failed` submission violates more than one rule, the engine aggregates them into `multiple_reasons` ordered by this pinned precedence:

1. `unknown_children`
2. `child_is_batch_parent`
3. `child_not_eligible`
4. `mixed_with_other_evidence`
5. `retry_already_in_progress`

`no_batch_materialized` and `empty_child_list` short-circuit before aggregation. The precedence is stable across releases so agents can dispatch on the first reason.

### R0-R9 pre-append validation (summary)

The scheduler runs ten runtime rules on every task-list submission **before** appending any event — rejected submissions leave zero state on the parent's event log. A one-line summary per rule:

| Rule | Summary |
|---|---|
| R0 | Task list is non-empty. |
| R1 | Per-task: child template resolvable and compilable (failures become `spawn_failed`). |
| R2 | Per-task: `vars` resolve against the child template (failures become `spawn_failed`). |
| R3 | `waits_on` graph is a DAG — no cycles. Rejects the whole submission. |
| R4 | No dangling `waits_on` references to names absent from the submission. |
| R5 | Task names are unique within the submission. |
| R6 | Hard limits: `tasks.len() <= 1000`, `waits_on.len() <= 10` per task, DAG depth `<= 50`, payload `<= 1 MB`. |
| R7 | No collision with existing sibling children (enforced at init via `renameat2`). |
| R8 | Spawn-time immutability: for already-spawned tasks, submitted `template` / `vars` / `waits_on` must match the recorded `spawn_entry`. |
| R9 | Task name matches `^[A-Za-z0-9_-]+$`, 1-64 chars, not in the reserved set (`retry_failed`, `cancel_tasks`). |

See [batch-workflows.md](batch-workflows.md) for how the runner dispatches on each rejection, and `docs/designs/DESIGN-batch-child-spawning.md` in the koto repository for the full rule definitions and rationale.

---

For the complete error taxonomy and exit code reference, see `docs/guides/cli-usage.md`.
