# Lead: What does the error model look like when one command serves multiple roles?

## Findings

### The core problem

When one command serves as both a read (return current directive) and a write (submit data, trigger transition), the response must unambiguously tell the caller which operation happened and whether it succeeded. Three distinct outcomes need distinct representations:

1. **Read success**: returned current directive, nothing was changed
2. **Write accepted**: data was valid, state advanced, here's the next directive
3. **Write rejected**: data was submitted but something was wrong

Without structured semantics in the JSON payload, agents are forced to infer intent from exit codes or parse text — both fragile.

### Patterns from structured output CLIs

**kubectl / GitHub CLI / gh**: JSON output with explicit `kind`, `status`, and `reason` fields. Agents can check `status` to know if the operation succeeded, and `reason` to understand why it failed. Exit codes are secondary signals for shell scripts; the JSON is the authoritative signal for programmatic consumers.

**gRPC status codes**: The most precise taxonomy for distinguishing error categories:
- `FailedPrecondition` — system is not in the right state to accept this input (e.g., transition called when no data was expected)
- `InvalidArgument` — the caller provided bad input regardless of state
- `OutOfRange` — valid input type but value out of acceptable bounds
- `AlreadyExists` / `Aborted` — conflict with concurrent state changes

This taxonomy maps directly onto koto's needs: a caller submitting a transition target when the current state expects evidence is `FailedPrecondition`, not `InvalidArgument`.

**HTTP semantics adapted to CLI**:
- 200: read or write succeeded
- 409 Conflict: state mismatch (called with data when state doesn't expect it)
- 412 Precondition Failed: gates not satisfied
- 422 Unprocessable Entity: data was received and understood but was semantically invalid

**Helm / Argo Workflows**: embed `phase` and `message` in status output. The agent can check `phase` (Pending / Running / Succeeded / Failed) without parsing text.

### What koto's JSON schema needs

An explicit `op` (or `kind`) field distinguishing read from write responses:

```json
{
  "op": "read",
  "state": "implement",
  "directive": "...",
  "expects": { ... }
}
```

```json
{
  "op": "write",
  "accepted": true,
  "state": "review",
  "directive": "..."
}
```

```json
{
  "op": "write",
  "accepted": false,
  "error": {
    "code": "precondition_failed",
    "message": "state 'implement' expects evidence submission, got transition target"
  }
}
```

Exit codes: 0 for read or accepted write, non-zero for rejected write or system error.

### Error categories koto needs to distinguish

| Category | Meaning | Agent response |
|----------|---------|----------------|
| `precondition_failed` | Submitted data when state doesn't expect it, or submitted wrong type | Re-read state, submit correct type |
| `invalid_input` | Data was the right type but failed validation | Fix the data and resubmit |
| `gate_blocked` | State gates not yet satisfied | Wait or take action to satisfy gates |
| `not_found` | State file missing or workflow not initialized | Escalate to user |
| `conflict` | Concurrent modification detected | Re-read and retry |

## Implications

koto's JSON output schema must include an explicit `op` field on every response. Agents should never need to infer whether a call was a read or a write from context. The error taxonomy should mirror gRPC's categories — `precondition_failed` vs. `invalid_input` vs. `gate_blocked` are distinct recovery paths for an agent.

Idempotency matters: calling `koto next` with no data must always be safe and return the same thing. The agent can always "re-read" without risk of side effects.

## Surprises

The most useful precedent isn't a CLI — it's gRPC's status code taxonomy. CLI tools typically use coarser error models (exit 0/1) because humans can read the error message. For agent-consumed CLIs, the machine-readable error code is the primary signal, making the gRPC taxonomy directly applicable.

## Open Questions

- Should koto distinguish "this state is waiting on external data" (blocking, agent should poll) from "this state is blocked by a gate the agent can satisfy" (agent should take action)? These have different recovery loops.
- How fine-grained should error codes be? Too many categories risks the agent not knowing which to handle; too few loses actionable signal.

## Summary

Multi-role CLIs need explicit operation type in JSON output (`op: read` vs `op: write`) and structured error codes that distinguish precondition failures from invalid input from gate blocks — because agents need to branch recovery logic on these categories, not just retry blindly. For koto, the gRPC status code taxonomy (`FailedPrecondition`, `InvalidArgument`, `Aborted`) maps cleanly to the three failure modes an agent needs to handle. The biggest open question is whether to distinguish "waiting on external data" (async block) from "gate the agent can satisfy" (synchronous action), as these require different agent behaviors.
