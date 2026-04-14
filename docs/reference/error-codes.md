# Error Code Reference

Every error from the koto CLI is a JSON object with an `error` field and a `command` field:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

The `error` field is a human-readable message. The `command` field identifies which subcommand produced the error. Both fields are always present.

## Error conditions by command

### init

**Workflow already exists** — a `koto-<name>.state.jsonl` file exists in the current directory:

```json
{"error":"workflow 'my-workflow' already exists","command":"init"}
```

Rename the workflow or delete the existing state file.

**Invalid template** — the template file can't be compiled:

```json
{"error":"failed to parse template: missing required field 'initial_state'","command":"init"}
```

Run `koto template compile <path>` to see the full compilation error.

---

### next

The `next` command has two error paths:

1. **Pre-dispatch I/O errors** use the flat format (`{"error": "...", "command": "next"}`). These fire before the dispatcher runs -- corrupt state files, missing templates, hash mismatches.
2. **Domain errors** use a structured format with a code, message, and optional field-level details. These come from the dispatcher and validation logic.

#### Structured domain errors

Domain errors use this shape:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence validation failed",
    "details": [
      {"field": "decision", "reason": "required field missing"}
    ]
  }
}
```

The `details` array is empty when the error isn't field-specific. The six error codes:

| Code | Exit | Meaning |
|------|:----:|---------|
| `gate_blocked` | 1 | One or more command gates failed or timed out. Transient -- may resolve on retry. |
| `integration_unavailable` | 1 | The state declares an integration but no runner is available. Transient. |
| `invalid_submission` | 2 | The `--with-data` payload is malformed, too large, or fails schema validation. Caller must fix the payload. |
| `precondition_failed` | 2 | A logical precondition wasn't met: `--with-data` and `--to` used together, `--to` targets an invalid state, or the state has no `accepts` block. |
| `terminal_state` | 2 | Evidence was submitted to a terminal state. The workflow is already done. |
| `workflow_not_initialized` | 2 | No state file found for the given workflow name. |

Exit code 1 means transient -- the agent can retry without changing its behavior. Exit code 2 means the agent must change something (fix the payload, pick a different target, etc.).

#### Exit code mapping

| Exit code | Category | When |
|:---------:|----------|------|
| 0 | Success | Normal response (any variant) |
| 1 | Transient | `gate_blocked`, `integration_unavailable`, engine I/O errors |
| 2 | Caller error | `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized` |
| 3 | Infrastructure | Corrupt state file, template hash mismatch, template parse failure |

#### Pre-dispatch I/O errors

These still use the flat format and aren't domain errors:

**Corrupt state file (exit code 3)** -- the state file exists but can't be parsed. This covers empty files, invalid JSON, and sequence number gaps:

```json
{"error":"state file corrupted: sequence gap at line 4: expected seq 3, got 5","command":"next"}
```

Inspect the file directly. The first line should be a header with `schema_version`, and each subsequent line should be a valid event with a monotonic `seq` number. A truncated final line (e.g., from a crash) is recovered automatically -- only interior corruption triggers this error.

**Template hash mismatch (exit code 3)** -- the compiled template on disk doesn't match the hash recorded at init time:

```json
{"error":"template hash mismatch: header says abc123 but cached template hashes to def456","command":"next"}
```

Reinitialize the workflow to pick up the new template.

**No events in state file** -- the state file has a header but no event lines:

```json
{"error":"state file has no events","command":"next"}
```

---

### rewind

**Corrupt state file (exit code 3)** -- same as `next` above.

**Already at initial state** -- only one state-changing event exists, so there's nothing to rewind to:

```json
{"error":"already at initial state, cannot rewind","command":"rewind"}
```

**Workflow not found:**

```json
{"error":"workflow 'my-workflow' not found","command":"rewind"}
```

---

### template compile

**Compilation failed** — invalid YAML, missing required fields, or unknown gate type:

```json
{"error":"missing required field 'initial_state'","command":"template compile"}
```

---

### template validate

**Schema invalid** — the compiled JSON doesn't match the expected schema:

```json
{"error":"invalid JSON: missing field `format_version`","command":"template validate"}
```

---

## Batch errors

Batch-scoped ticks (parents with `materialize_children`, or `retry_failed` submissions) emit a dedicated envelope with `action: "error"` and a typed `error.batch` field carrying a `BatchError` variant. Each variant uses a snake_case `kind` discriminator so agents can dispatch without string-matching on `message`.

| `batch.kind` | Exit | Meaning |
|---|:---:|---|
| `concurrent_tick` | 1 | Another `koto next` invocation holds the advisory flock on this batch parent. Retryable after backoff. Carries `holder_pid` (may be `null`). |
| `invalid_batch_definition` | 2 | A pre-append structural rule rejected the submission. Carries a nested `InvalidBatchReason` (`empty_task_list`, `cycle`, `dangling_refs`, `duplicate_names`, `spawned_task_mutated`, `invalid_name`, `reserved_name_collision`). |
| `limit_exceeded` | 2 | A pre-append hard limit (R6) was violated. Carries `which` (`tasks`, `waits_on`, `depth`, `payload_bytes`), `limit`, `actual`, and optional `task`. |
| `template_not_found` | 2 | A task's child template path did not resolve against any configured search base. Carries `task`, `path`, `paths_tried`. |
| `template_compile_failed` | 2 | A task's child template was found but failed to compile. Carries `task`, `path`, typed `compile_error`. |
| `backend_error` | 1 or 3 | Backend list/read failed during classification. Exit code 1 when `retryable: true`, else 3. Tick-wide. |
| `spawn_failed` | 3 | Per-task spawn failure after validation passed (`init_state_file` I/O, collision, compile). Carries `task`, `spawn_kind`, `message`. |
| `invalid_retry_request` | 2 | A `retry_failed` submission failed validation. Carries a nested `InvalidRetryReason` with pinned precedence: `unknown_children` → `child_is_batch_parent` → `child_not_eligible` → `mixed_with_other_evidence` → `retry_already_in_progress`. |

Example envelope:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "batch": {
      "kind": "limit_exceeded",
      "which": "tasks",
      "limit": 1000,
      "actual": 1500
    }
  }
}
```

All batch validation runs pre-append — rejected submissions leave no events on the parent's state file.
