# Security Review: DESIGN-batch-rewind

## Scope

Review of the Security Considerations section in `docs/designs/DESIGN-batch-rewind.md` against the current implementation of `handle_rewind`, `backend.cleanup()`, `sync_delete_session`, and the `augment_snapshots_with_child_completed` / `build_children_complete_output` paths.

## 1. Attack Vectors Not Considered

### 1a. TOCTOU in child enumeration and cleanup (Low severity)

The design calls for `handle_rewind` to list children via `backend.list()` then call `backend.cleanup(child_id)` for each. Between the list and the cleanup, a concurrent process could create a new child session with the same `parent_workflow` value. This child would not be cleaned up and would survive into the new epoch.

**Practical risk**: Low. The rewind operation is agent-initiated, not concurrent with child spawning under normal orchestration flow. The epoch filter on `ChildCompleted` mitigates the consequence even if a stale child survives.

### 1b. Concurrent child writing during cleanup (Low severity)

A running child process could be actively writing events while `backend.cleanup()` calls `remove_dir_all`. On Linux, `remove_dir_all` will succeed even if a file descriptor is open in another process, but the child process will get I/O errors on subsequent writes. The design doc does not discuss how running children are signaled or gracefully terminated before cleanup.

**Practical risk**: Low. The design states "backend.cleanup() is synchronous and removes the state file before returning, preventing further child events." This is true for the local backend (`remove_dir_all`), but a child process that has already buffered an event write could still attempt to write after the directory is gone, producing a spurious error. This is a noisy-failure case, not a data-corruption case.

### 1c. S3 prefix traversal via crafted session ID (Not exploitable)

`sync_delete_session` constructs an S3 prefix from the session ID and deletes all objects under it. If a malicious session ID could contain path separators or S3 prefix-traversal characters, it could target objects outside the session prefix. However, `validate_session_id` (called in `local.cleanup()`) restricts IDs to `[a-zA-Z][a-zA-Z0-9._-]*`, which prevents `/` or other dangerous characters. The design doc does not mention this, but the existing validation is sufficient.

### 1d. Denial of service via large batch cleanup (Informational)

If a parent has thousands of children, `handle_rewind` will call `backend.cleanup()` sequentially for each, which under `CloudBackend` means one `sync_delete_session` per child (list + delete objects). This could be slow or hit S3 rate limits. The design doc does not mention this, but it is an operational concern rather than a security vulnerability.

## 2. Are Mitigations Sufficient for Identified Risks?

The design identifies four claims in its Security Considerations:

| Claim | Assessment |
|-------|-----------|
| "No external inputs" -- rewind reads from parent log and `backend.list()` | **Correct.** The rewind target is derived from the event log, not from user-supplied input beyond the parent name (which is validated). |
| "No permission escalation" -- cleanup removes koto-created directories | **Correct.** `remove_dir_all` operates within the sessions directory. `validate_session_id` prevents path traversal. |
| "No data exposure" -- child sessions contain workflow state, not credentials | **Partially correct.** Child sessions contain workflow state and evidence submissions. Evidence could contain arbitrary agent-submitted content. However, cleanup deletes rather than exposes this data, so the risk direction is correct (deletion, not leakage). |
| "S3 deletion uses existing authenticated client" | **Correct.** No new credentials or scopes are introduced. |

The mitigations are sufficient for the identified risks. The design correctly identifies that it operates within the existing `backend.cleanup()` security boundary.

## 3. "Not Applicable" Justification Review

The design concludes with: "N/A -- no security dimensions apply beyond what the existing `backend.cleanup()` contract already covers."

This is **mostly justified** but slightly overbroad. Two dimensions deserve brief mention even if not actionable:

1. **Atomicity of the rewind+cleanup compound operation**: The design introduces a multi-step operation (append Rewound event, then cleanup N children) that is not atomic. A crash between steps 1 and 3 leaves the parent rewound but children not cleaned up. The epoch filter (step 4) provides correctness recovery, but the on-disk state is inconsistent until the next rewind or manual cleanup. This is not a security vulnerability per se, but the "N/A" dismissal does not acknowledge this failure mode.

2. **S3 best-effort deletion and data retention**: The design acknowledges in Consequences/Negative that S3 children may persist after network failure. This is a data-retention concern -- stale workflow state (which may contain agent evidence/context) persists longer than expected on a remote storage service. For workflows processing sensitive content, this is worth noting. The design does document this in Consequences but the Security section's "N/A" does not cross-reference it.

Neither of these rises to the level of a security finding, but the "N/A" framing should be softened to "No new attack surface; see Consequences for failure-mode data retention behavior."

## 4. Residual Risk Assessment

### Residual risk that should be escalated: None.

The design operates entirely within the existing security boundary of `backend.cleanup()` and `sync_delete_session`. The epoch filter adds defense-in-depth against the only identified race condition. The `validate_session_id` function prevents path traversal in session IDs passed to cleanup.

### Residual risk to track (non-escalation):

- **S3 data retention after failed cleanup**: Stale child sessions on S3 after network failure during rewind. Mitigated by `koto session resolve --children` and by the epoch filter preventing correctness impact. This is a known, documented, and accepted operational risk.

- **No graceful termination of running children**: If a child agent is actively running when rewind cleans up its session, the child will encounter I/O errors. The design should document that rewind-with-batch-cleanup is expected to be performed when children are idle. This is a usability/documentation gap, not a security gap.

## Summary

The Security Considerations section is accurate but slightly terse. The "N/A" conclusion is defensible given the narrow scope of changes, but should acknowledge the S3 data-retention failure mode already documented in Consequences. No attack vectors require design changes. No residual risk requires escalation.
