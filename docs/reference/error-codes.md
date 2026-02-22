# Error Code Reference

Every engine error is a `TransitionError` with a structured JSON shape. The CLI wraps it in an `error` envelope:

```json
{
  "error": {
    "code": "invalid_transition",
    "message": "cannot transition from \"assess\" to \"done\": not in allowed transitions [plan]",
    "current_state": "assess",
    "target_state": "done",
    "valid_transitions": ["plan"]
  }
}
```

The `code` field is stable and safe to match against programmatically. The `message` field is human-readable and may change between versions. Fields like `current_state`, `target_state`, and `valid_transitions` are included when relevant and omitted otherwise.

In Go code, errors are `*engine.TransitionError` values:

```go
type TransitionError struct {
    Code             string   `json:"code"`
    Message          string   `json:"message"`
    CurrentState     string   `json:"current_state,omitempty"`
    TargetState      string   `json:"target_state,omitempty"`
    ValidTransitions []string `json:"valid_transitions,omitempty"`
}
```

## Error codes

### terminal_state

The current state is terminal (no outgoing transitions). The workflow is finished.

**When it occurs:** Calling `transition` when the workflow has already reached a terminal state.

**Example:**

```json
{
  "error": {
    "code": "terminal_state",
    "message": "cannot transition from terminal state \"done\"",
    "current_state": "done",
    "target_state": "assess"
  }
}
```

**How to handle:** The workflow is complete. Call `next` to confirm -- it returns `{"action":"done"}`. If the terminal state is unexpected (e.g., the workflow reached an error state), use `rewind` to roll back to an earlier state and try a different path.

---

### invalid_transition

The target state isn't reachable from the current state. The transition isn't in the allowed list.

**When it occurs:** Calling `transition <target>` when `target` isn't in the current state's transitions.

**Example:**

```json
{
  "error": {
    "code": "invalid_transition",
    "message": "cannot transition from \"assess\" to \"done\": not in allowed transitions [plan]",
    "current_state": "assess",
    "target_state": "done",
    "valid_transitions": ["plan"]
  }
}
```

**How to handle:** Check `valid_transitions` to see what's actually allowed from the current state. For agents, this usually means the agent picked the wrong next state. Use the `valid_transitions` array to choose a correct target.

---

### unknown_state

A state name in the state file doesn't exist in the machine definition. This indicates a corrupted state file or an incompatible machine definition.

**When it occurs:**
- `Load` finds a `current_state` that isn't in the machine.
- `Init` is called with a machine whose `InitialState` doesn't exist in its own states map.

**Example:**

```json
{
  "error": {
    "code": "unknown_state",
    "message": "current state \"planning\" not found in machine definition",
    "current_state": "planning"
  }
}
```

**How to handle:** This shouldn't happen during normal operation. It means either the state file was manually edited, or the template/machine definition changed incompatibly. Cancel the workflow and reinitialize.

---

### template_mismatch

The template file on disk has a different SHA-256 hash than the one recorded when the workflow was initialized. Someone modified the template after the workflow started.

**When it occurs:** Calling `next`, `transition`, `rewind`, or `validate` when the template file has changed since `init`.

**Example:**

```json
{
  "error": {
    "code": "template_mismatch",
    "message": "template hash mismatch: state file has \"sha256:abc...\" but template on disk is \"sha256:def...\""
  }
}
```

**How to handle:** There's no override flag -- this is by design. The template defines the workflow rules, and changing them mid-execution breaks the integrity guarantee. Options:

1. **Restore the original template.** If the change was accidental (editor auto-save, formatting tool), revert the template file to its original content.
2. **Cancel and restart.** If the template change is intentional, run `koto cancel` and `koto init` with the updated template.
3. **Diagnose.** Run `koto validate` to confirm the mismatch. The state file's `workflow.template_hash` field shows the expected hash.

---

### version_conflict

The state file's version counter changed between when the engine read it and when it tried to write. Another process modified the file in between.

**When it occurs:** Calling `transition` or `rewind` when another process wrote to the same state file concurrently.

**Example:**

```json
{
  "error": {
    "code": "version_conflict",
    "message": "version conflict: expected version 3 but found 4 on disk"
  }
}
```

**How to handle:** Re-read the state file and check the current state. The other writer may have already made the transition you intended. In most cases, reloading and retrying is the right approach:

```bash
# Check what state the file is actually in
koto query

# Retry the operation if still needed
koto transition <target>
```

For library consumers, this means calling `engine.Load` again to get a fresh view of the state file.

---

### rewind_failed

The rewind target is invalid. Either the state was never visited, or the target is a terminal state.

**When it occurs:** Calling `rewind --to <target>` when:
- The target state has never appeared as a destination in the transition history (and isn't the initial state).
- The target state is terminal (rewinding there would leave the workflow stuck).

**Example (never visited):**

```json
{
  "error": {
    "code": "rewind_failed",
    "message": "cannot rewind to \"implement\": state has never been visited",
    "current_state": "plan",
    "target_state": "implement"
  }
}
```

**Example (terminal target):**

```json
{
  "error": {
    "code": "rewind_failed",
    "message": "cannot rewind to \"done\": target is a terminal state",
    "current_state": "implement",
    "target_state": "done"
  }
}
```

**How to handle:** Check the transition history (`koto query`) to see which states have been visited. You can only rewind to states that appear as a `to` field in the history, plus the machine's initial state which is always valid.
