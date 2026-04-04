<!-- decision:start id="parent-child-lifecycle-coupling" status="assumed" -->
### Decision: Parent-child lifecycle coupling policy

**Context**

koto tracks parent-child workflow relationships through a `parent_workflow` header field on child state files. The exploration phase decided on Abandon as the default parent close policy -- children continue independently when a parent completes. But three destructive parent operations (cancel, cleanup, rewind) create edge cases that need explicit policy.

The core tension is between consistency (children should reflect parent state changes) and the Abandon philosophy (koto is a contract layer, not an execution engine -- it can't kill agent processes). Today `koto cancel` appends a WorkflowCancelled event to a single workflow, `koto session cleanup` deletes a session directory entirely, and `koto rewind` walks back one state-changing event. None of these operations are aware of other workflows. Adding cascade behavior means scanning all session headers to discover children, which is viable but adds latency and complexity.

A further constraint: ChildWorkflowSpawned events on the parent side were deferred from MVP, so koto has no record of which parent state spawned which child. This rules out state-aware cascade (e.g., "cancel children spawned during the state being rewound").

**Assumptions**

- The `parent_workflow` header field is the authoritative source for discovering children. If this field is unreliable, cascade operations of any kind break.
- Agents check `koto next` periodically and will notice cancellation status. If agents ignore koto state, cancelled workflows can run indefinitely regardless of policy.
- ChildWorkflowSpawned events remain deferred from MVP. If added later, rewind-aware cascade becomes feasible and could be layered on.

**Chosen: Advisory-only (inform, don't act)**

koto never automatically cascades lifecycle operations to children. Instead, every lifecycle command that affects a parent includes child workflow information in its JSON output so the calling agent can decide what to do.

Specific behaviors:

- **Cancel** (`koto cancel <parent>`): Cancels only the parent workflow. The JSON response includes a `children` array listing the names and states of active child workflows. The agent decides whether to cancel them.
- **Cleanup** (`koto session cleanup <parent>`): Deletes only the parent session. Children become orphans -- their `parent_workflow` header points to a non-existent session. The agent should clean up children first if it wants to avoid orphans.
- **Normal completion**: No automatic action on children. The parent agent is responsible for managing child lifecycle before or after reaching a terminal state.
- **Rewind** (`koto rewind <parent>`): Walks back the parent state with no effect on children. If children exist, the JSON response includes an advisory `children` field. The agent decides whether to cancel or rewind children spawned during the now-rewound state.
- **Orphan discovery**: `koto list --orphaned` returns workflows whose `parent_workflow` references a session that no longer exists. This is an informational flag, not a cleanup trigger.

**Rationale**

Advisory-only is the natural extension of the Abandon default. It preserves agent control over child lifecycle -- the same principle that led to choosing Abandon in the first place. koto can't force-terminate agent processes, so automatic cancel cascade creates a false sense of safety: the child workflow would be marked cancelled, but the agent running it would continue until it calls `koto next`. Better to surface the information and let the agent handle it with full context.

The implementation cost is low: lifecycle commands need to scan headers for children (a `list()` call filtered by `parent_workflow`) and include the results in JSON output. No new event types, no blocking semantics, no new header fields beyond the already-decided `parent_workflow`.

Per-child policies (Temporal's model) are the theoretically cleanest alternative, but they add complexity for usage patterns that don't exist yet. Advisory-only can be upgraded to per-child policies later by adding `on_parent_cancel` and `on_parent_cleanup` header fields -- the advisory output provides the foundation that per-child policies would build on.

**Alternatives Considered**

- **Cancel cascades, cleanup blocks**: Automatically cancels children when parent is cancelled and blocks parent cleanup while active children exist. Rejected because it contradicts the Abandon default, requires ChildWorkflowSpawned events for rewind cascade (deferred from MVP), and takes control away from the agent for a marginal safety gain. The blocking cleanup semantic also creates a new failure mode where cleanup can fail unexpectedly.

- **Advisory cancel + cascading cleanup**: Cancel is advisory but cleanup cascades to all children recursively. Rejected because cascading cleanup is destructive and irreversible -- deleting child session directories removes evidence and decisions that the child agent or a human might need. Advisory-only with orphan flagging is safer and equally discoverable.

- **Per-child metadata with opt-in cascade flags**: Each child gets `on_parent_cancel` and `on_parent_cleanup` header fields set at init time. Rejected for MVP because it adds significant surface area (new header fields, branching logic in every lifecycle command, template-level configuration) for a feature whose usage patterns aren't established. Can be layered on later if advisory-only proves insufficient.

**Consequences**

Orphaned children are possible and expected. When a parent is cleaned up without first cleaning up children, those children persist with a dangling `parent_workflow` reference. This is by design -- the `--orphaned` list flag makes them discoverable, and agents or operators can clean them up. The system tolerates orphans rather than preventing them, which matches koto's philosophy of tracking state rather than enforcing lifecycle.

Agents bear full responsibility for child lifecycle management. This is consistent with koto's role as a contract layer, but it means agents that skip cleanup will leave orphans. Skill authors need to account for this in their workflow templates.

The advisory output in cancel/cleanup/rewind responses establishes a stable contract that per-child policies can build on later without breaking existing consumers.
<!-- decision:end -->
