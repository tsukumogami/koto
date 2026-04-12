---
scope: lead-failure-routing
exploration: batch-child-spawning
round: 1
---

# Research: Batch Child Spawning — Failure Routing Policies

## Executive Summary

Four failure routing policies address how koto should handle failures in a DAG of child workflows. **Skip-dependents** is recommended as the default: when a child fails, mark its direct and transitive dependents as skipped (terminal, with reason), continue independent branches, and let the batch complete with partial success. This maximizes fault isolation, enables recovery workflows, and aligns with the GitHub-issue use case where a failed PR should not block unrelated issues from merging.

Alternative policies (**Pause-on-failure**, **Fail-fast**, **Continue-independent**) serve specialized workflows and should be configurable per-batch. The policy setting belongs in the evidence that triggers materialization, making it opaque to the template layer and discoverable at the parent agent's spawn point.

---

## Policies Compared

### 1. Pause-on-Failure

**Behavior:** One child hits a failure → koto stops scheduling new tasks (even independent ones). The parent's `children-complete` gate returns a blocking condition with status `paused_on_failure`, citing the failed child. The consumer (human or agent) inspects the failure and decides via evidence submission how to proceed.

#### What the parent sees:

- **Mid-execution (`koto next <parent>`):**
  ```json
  {
    "action": "gate_blocked",
    "gate": "children-complete",
    "category": "temporal",
    "blocking_conditions": [{
      "name": "children-complete",
      "type": "children-complete",
      "reason": "paused_on_failure",
      "reason_detail": "Child plan.issue-1 failed at state review",
      "output": {
        "total": 3,
        "completed": 1,
        "pending": 0,
        "failed": 1,
        "paused": true,
        "children": [
          {"name": "plan.issue-1", "state": "failed", "complete": false, "reason": "tests did not pass"},
          {"name": "plan.issue-2", "state": "queued", "complete": false},
          {"name": "plan.issue-3", "state": "queued", "complete": false}
        ]
      }
    }]
  }
  ```

- **`children-complete` gate output (paused):**
  - `paused: true`
  - `failed_child`: name of the child that triggered pause
  - `failed_state`: state where the child failed
  - `queued`: number of tasks not yet spawned
  - Per-child reason field

#### Dependent handling:

A new "queued" state tracks tasks that the scheduler hasn't spawned yet (they await either the start trigger or a dependency resolution). When paused, "queued" tasks freeze in that state, persisted in koto as a task list on the parent workflow (not as child workflows).

The parent's state file includes:
```yaml
batch_id: batch-20260411-abc123
batch_status: paused_on_failure
failed_child: plan.issue-1
failed_at_state: review
queued_tasks:
  - name: plan.issue-2
    depends_on: []
  - name: plan.issue-3
    depends_on: [plan.issue-1]
```

#### Resume:

The consumer reads the failure reason via `koto context get plan.issue-1 error_summary`, decides whether to retry the child, skip it, or escalate. The parent accepts evidence (e.g., `{"action": "retry_child", "child": "plan.issue-1"}` or `{"action": "continue", "skip_blocked": true}`). On resume:

- **`retry_child`:** the scheduler re-spawns the named child from its initial state
- **`continue` with `skip_blocked=true`:** mark failed child and its dependents as skipped, resume scheduling independent tasks
- **`escalate`:** parent transitions to a blocked state, awaiting external intervention

#### GH-issue use-case fit:

**Frustration:** This policy would freeze all subsequent issues after the first failure—ideal for sequential workflows where task order matters globally, but a poor fit when issues 2 and 3 are independent. The pause creates a "call your team" moment, but that's expensive for autonomous agents.

**Advantage:** Forces human visibility over failures before continuing, reducing cascade risks.

---

### 2. Fail-Fast

**Behavior:** One child fails → koto marks the entire batch as failed, cancels or marks as skipped any task not yet spawned, the parent transitions via a failure route (or blocks with a corrective gate asking for a retry decision).

#### What the parent sees:

- **Mid-execution (`koto next <parent>`):**
  ```json
  {
    "action": "evidence_required",
    "state": "wait_for_children",
    "error": "Child batch failed: plan.issue-1 failed at state review",
    "blocking_conditions": [{
      "name": "children-complete",
      "type": "children-complete",
      "reason": "batch_failed",
      "category": "corrective",
      "output": {
        "total": 3,
        "completed": 1,
        "pending": 0,
        "failed": 1,
        "failed_children": ["plan.issue-1"],
        "batch_status": "failed",
        "all_complete": false
      }
    }],
    "expects": {
      "event_type": "evidence_submitted",
      "fields": {
        "action": {
          "type": "enum",
          "values": ["retry_batch", "skip_batch", "investigate"]
        }
      }
    }
  }
  ```

- **`children-complete` gate output:**
  - `batch_status: "failed"`
  - `failed_children: [...]`
  - `reason: "batch_failed"`
  - All queued tasks marked as `{"state": "cancelled", "reason": "batch_failed"}`

#### Dependent handling:

Tasks not yet spawned are never created. Existing code removes them from the batch's task list and marks them as cancelled in the parent's context (advisory record only, no child state files).

#### Resume:

The parent accepts evidence:
- `{"action": "retry_batch"}` — re-initialize the entire batch from scratch (all child states reset)
- `{"action": "skip_batch"}` — mark the batch terminal with a failure code, move on
- `{"action": "investigate"}` — transition to a manual investigation state

#### GH-issue use-case fit:

**Frustration:** If issue 3 depends only on issue 2 (not issue 1), fail-fast would cancel issue 3 even though issue 2 might pass. This is wasteful and blocks legitimate parallelism. Fail-fast is better for "all or nothing" batches (e.g., all-or-nothing coordinated release).

**Advantage:** Clear semantics, minimal state complexity.

---

### 3. Skip-Dependents (Recommended Default)

**Behavior:** One child fails → koto marks the failed child with a terminal `failed` state and its direct and transitive dependents as a new terminal state `skipped_due_to_dep_failure`. Independent branches continue, the batch completes when the scheduler has no more work to do.

#### What the parent sees:

- **Mid-execution (`koto next <parent>`):**
  ```json
  {
    "action": "gate_blocked",
    "gate": "children-complete",
    "category": "temporal",
    "blocking_conditions": [{
      "name": "children-complete",
      "output": {
        "total": 3,
        "completed": 0,
        "pending": 1,
        "skipped": 1,
        "failed": 1,
        "all_complete": false,
        "children": [
          {"name": "plan.issue-1", "state": "failed", "complete": true, 
           "reason": "tests did not pass", "reason_code": "test_failure"},
          {"name": "plan.issue-2", "state": "done", "complete": true},
          {"name": "plan.issue-3", "state": "skipped_due_to_dep_failure", "complete": true,
           "reason": "Dependency plan.issue-1 failed"}
        ]
      }
    }],
    "can_advance": false
  }
  ```

  After issue 2 also completes:
  ```json
  {
    "action": "evidence_required",
    "state": "analyze_results",
    "blocking_conditions": [],
    "expects": {
      "fields": {
        "decision": {
          "type": "enum",
          "values": ["proceed", "investigate_failures", "retry_failed"]
        }
      }
    }
  }
  ```

- **`children-complete` gate output:**
  - `skipped: 1` (new aggregate field)
  - `failed: 1`
  - Per-child `reason` and `reason_code` fields
  - `all_complete: true` (passes the gate when no work remains, even with failures/skips)

#### Dependent handling:

Tasks not yet spawned are created as child workflows but placed directly in a terminal state `skipped_due_to_dep_failure`, with a state file header field `skip_reason: "dependency_failed"` and context key `skipped_dependency: "plan.issue-1"`. The child's state log contains a single synthetic `WorkflowInitialized` event followed by a `Transitioned` event to the `skipped_due_to_dep_failure` state with `reason: "dependency failed"`.

Alternatively (simpler): the child is never spawned at all; the parent's context stores a record:
```json
{
  "batch_id": "batch-20260411-abc123",
  "skipped_tasks": [
    {"name": "plan.issue-3", "depends_on": "plan.issue-1", "reason": "dependency failed"}
  ]
}
```

**Note:** The first approach (create skipped child state files) is more consistent with koto's state-machine model but has higher I/O cost. The second (parent-side record) is simpler but means `koto workflows --children parent` doesn't show skipped tasks—only the parent's context records them.

#### Resume:

If the parent is interrupted mid-batch with one failure and one in-progress child:
- **Resume behavior:** The scheduler re-evaluates which tasks can run (none blocked by the failed one can). The in-progress child resumes independently. Skipped tasks remain skipped (no retry by default).
- **Retry after investigation:** The parent agent submits evidence `{"action": "retry_failed", "child": "plan.issue-1"}`, which resets the failed child and all skipped dependents to queued, and re-schedules them.

#### GH-issue use-case fit:

**Fit:** If issue 1 fails (tests don't pass, gate rejects work), issue 3 is automatically marked skipped. Issue 2, if independent, continues and can be merged. When the agent investigates issue 1, fixes it, and resubmits with `retry_failed`, issue 3 gets re-queued. **This is the expected behavior for GitHub PR workflows where independent PRs should not be blocked by unrelated failures.**

**Advantage:** 
- Maximizes parallelism (independent branches are unaffected)
- Clear reason codes for later analysis
- Natural recovery: `retry_failed` re-runs the chain
- Batch completes with a clear outcome: some succeeded, some failed, some skipped

---

### 4. Continue-Independent

**Behavior:** One child fails → koto records the failure, continues running every independent branch (no skipping), the batch completes when the scheduler is out of work. The parent sees a structured report of successes and failures.

#### What the parent sees:

- **Mid-execution (`koto next <parent>`):**
  ```json
  {
    "action": "gate_blocked",
    "gate": "children-complete",
    "category": "temporal",
    "blocking_conditions": [{
      "name": "children-complete",
      "output": {
        "total": 3,
        "completed": 1,
        "pending": 2,
        "failed": 0,
        "all_complete": false,
        "children": [
          {"name": "plan.issue-1", "state": "failed", "complete": true, 
           "reason": "tests did not pass", "reason_code": "test_failure"},
          {"name": "plan.issue-2", "state": "in_progress", "complete": false},
          {"name": "plan.issue-3", "state": "in_progress", "complete": false}
        ]
      }
    }]
  }
  ```

  After all finish:
  ```json
  {
    "action": "evidence_required",
    "state": "analyze_results",
    "blocking_conditions": [],
    "expects": {
      "fields": {
        "decision": {
          "type": "enum",
          "values": ["proceed_with_successes", "investigate_and_retry"]
        }
      }
    }
  }
  ```

- **`children-complete` gate output:**
  - `all_complete: true` (all have reached a terminal state, passed or failed)
  - `failed: 1`
  - Per-child reason and reason_code
  - No skipped aggregates; all tasks run regardless of dependencies

#### Dependent handling:

No special handling. Tasks depending on a failed task still run. The parent (or a later state) must decide whether to treat a downstream failure caused by an upstream failure as an expected outcome or an error. This is left to template logic.

#### Resume:

If interrupted mid-batch, the scheduler resumes all in-progress children independently. The parent can re-run a subset via evidence (e.g., `{"action": "retry_failed", "children": ["plan.issue-1"]}`).

#### GH-issue use-case fit:

**Fit:** If issue 1 fails, issue 2 and 3 still run. This is useful when issues 2 and 3 do not depend on issue 1 but might interact with it (e.g., they all modify the same file, and the parent needs to see all possible conflicts). 

**Frustration:** Issue 3 might fail *because* issue 1 failed (if there was a latent dependency), but the system doesn't know that. The parent must post-hoc reason about which failures are induced. Also, this doesn't align with the user's stated GH-issue use case: "issue 3 depends on issue 1 and 2 being merged first" — the dependency is semantic, not implicit from the DAG.

**Advantage:** Simplest to implement (minimal state tracking), maximum information (all runs complete).

---

## Comparison Table

| Policy | Parent sees failure mid-run? | Dependents run? | Batch terminal? | Restart cost | Complexity |
|--------|------|-----------|-----------|--------|---------|
| **Pause-on-failure** | Yes (gate blocks, reason: paused) | No (queued) | No | High (manual decision) | High (task list, pause state) |
| **Fail-fast** | Yes (gate blocks, reason: batch_failed) | No (cancelled) | Yes (failed) | Medium (retry whole batch) | Medium (batch state) |
| **Skip-dependents** | Yes (gate blocks, reason: temporal, partial progress shown) | No (marked skipped) | Yes (partial success) | Low (retry_failed re-queues) | Medium (skipped state, depend tracking) |
| **Continue-independent** | Yes (gate blocks, reason: temporal, shows failures) | Yes (always) | Yes (mixed) | Low (selective retry) | Low (track failures only) |

---

## Policy Scope: Global, Per-Batch, or Per-Task?

### Recommendation: Per-Batch

The failure policy should be configurable **per-batch**, not global or per-task. Here's why:

1. **Different workflows have different semantics.** A release workflow (all-or-nothing) wants fail-fast. A plan workflow (GH issues) wants skip-dependents. A research workflow (exploratory) wants continue-independent.

2. **Batch-level declaration is ergonomic.** The parent agent that composes the task list (e.g., a skill converting a plan into a batch) knows its own failure semantics and should declare them once, not repeat per-task.

3. **No template coupling.** The policy lives in the evidence that triggers materialization (or in a batch metadata struct), not in the template. Templates remain reusable.

4. **Per-task policies add complexity without clear value.** If task A wants fail-fast and task B wants skip-dependents, the scheduler must handle them differently—contradictory outcomes in one batch.

### Implementation approach:

The evidence shape (or a dedicated `--batch-config` flag) includes:

```json
{
  "batch_id": "batch-20260411-abc123",
  "failure_policy": "skip-dependents",
  "tasks": [
    {"name": "plan.issue-1", "template": "...", "vars": {...}, "depends_on": []}
  ]
}
```

Or a dedicated materialization action accepts:
```bash
koto batch-spawn parent \
  --task-list tasks.json \
  --failure-policy skip-dependents
```

The parent stores the policy in a context key so the gate evaluator can apply it per-child.

---

## Per-Child Detailed Specifications

### Skip-Dependents (Default): Detailed Design

#### Task state machine (in parent context):

```
queued → spawned → in_progress ──→ success ──┐
                                              ├─→ complete
                                  (or)
                  ┌────────→ failed ──┐
                  │                   │
  dependency_failed (skipped) ────────┘

Legend:
  - queued: not yet spawned (scheduler will create child workflow)
  - spawned: child init called, state file created
  - in_progress: child has reported activity
  - success: child reached terminal success state
  - failed: child reached terminal failure state or error
  - skipped: child marked terminal without being spawned (dependency failed)
```

#### Parent state file tracks batch:

```yaml
batch_id: batch-20260411-abc123
failure_policy: skip-dependents
task_graph:
  plan.issue-1:
    depends_on: []
    state: failed
    reason: "tests did not pass"
  plan.issue-2:
    depends_on: []
    state: success
  plan.issue-3:
    depends_on: [plan.issue-1]
    state: skipped
    reason: "dependency plan.issue-1 failed"
```

#### Scheduler logic:

1. At each `koto next` call on the parent, the scheduler checks the task graph
2. For each queued task:
   - If all dependencies are in `success`, move to `spawned` and call `koto init child --parent parent`
   - If any dependency is in `failed`, move to `skipped` and record reason
   - If any dependency is in `queued` or `in_progress`, leave as `queued`
3. For each `in_progress` task, call `koto status child`:
   - If terminal, move to `success` or `failed` based on final state
   - If non-terminal, leave as `in_progress`
4. Return gate output reflecting current task_graph state

#### Gate output:

```json
{
  "total": 3,
  "completed": 2,    // success + skipped + failed
  "pending": 1,      // queued + in_progress
  "success": 1,
  "failed": 1,
  "skipped": 1,
  "all_complete": false,  // pending > 0
  "children": [
    {
      "name": "plan.issue-1",
      "state": "failed",
      "complete": true,
      "reason": "tests did not pass",
      "reason_code": "test_failure"
    },
    ...
  ]
}
```

#### Resume handling:

If parent is interrupted with `plan.issue-3` still in `skipped` state:
- On resume, the scheduler re-checks: `plan.issue-1` is still failed, so `plan.issue-3` remains skipped
- No action taken unless parent submits evidence `{"action": "retry_failed", "child": "plan.issue-1"}`
- On retry, `plan.issue-1` and `plan.issue-3` move back to `queued`, scheduler re-spawns them

#### Idempotency:

- `koto init plan.issue-3 --parent plan` called twice → second call finds existing state file, no-ops
- Scheduler detects existing child via `backend.exists()`, updates task_graph state without re-spawning

---

## Prior Art Summary

### GitHub Actions

- **Model:** Job-level `needs:` declares dependencies; `if: failure()` gates can run on dependency failure.
- **Failure behavior:** By default, job continues if its dependencies fail (no auto-skip). `if: failure()` is explicit opt-in.
- **Limitation:** No dynamic DAG; jobs declared statically.

### Apache Airflow

- **Model:** Task-level `trigger_rule` sets failure semantics: `all_success` (default, block if upstream fails), `one_failed` (run only if one upstream failed), `all_done` (run regardless), etc.
- **Failure behavior:** Highly configurable per-task.
- **Strength:** Granular control.
- **Limitation:** Requires understanding of `trigger_rule` semantics; most users stick with defaults, which means cascade failures.

### Argo Workflows

- **Model:** DAG templates with explicit dependency edges; `continueOn:` allows continuing after failure.
- **Failure behavior:** By default, failure blocks DAG progress. `continueOn: [Error]` is opt-in.
- **Strength:** Default is safe (pause-on-failure-like), explicit opt-in for continue.

### Temporal Workflow SDK

- **Model:** Parent workflow awaits child results via `ChildWorkflowFuture.await()`.
- **Failure behavior:** Child exception propagates to parent; parent must catch and decide (exception handling is explicit).
- **Strength:** Type-safe, exception-based, language-integrated.
- **Limitation:** Requires close coupling between parent and child code.

### Make / Ninja

- **Model:** File-based targets with dependencies.
- **Failure behavior:** Default stops on first failure. `-k` flag continues on failure.
- **Strength:** Simple, predictable.
- **Limitation:** Binary choice, no per-target configuration.

---

## Recommendation

**Default: Skip-Dependents**

### Why:

1. **Aligns with GH-issue use case.** Issue 3 depending on issue 1 and 2 means: "only schedule 3 after 1 and 2 are done." If 1 fails, 3 should not run (it would fail anyway or do wrong work). Issue 2, if independent, should still run and merge.

2. **Maximizes fault isolation.** Failures don't cascade to unrelated work. The batch completes with a clear outcome: some succeeded, some failed, some skipped due to dependency.

3. **Recovery is clean.** After the parent investigates and fixes the root cause (issue 1), it submits `retry_failed`, and the system re-queues issue 1 and all skipped dependents. No manual re-spawning.

4. **Reasonable defaults prevent user frustration.** Pause-on-failure is too conservative (freezes unrelated work). Fail-fast wastes parallelism. Continue-independent ignores dependencies, leading to confusing cascades.

5. **Prior art validation.** Airflow's `all_success` is the default (block if upstream fails); Argo defaults to pause-on-failure. Both learned from production use that safe-by-default is preferable.

### Per-batch configurability:

Offer `fail_policy` field in batch evidence or a CLI flag. Document skip-dependents, fail-fast, and continue-independent as options. Pause-on-failure is a future extension (requires handling task queuing and pause state).

### Persisted state:

Store the task graph and policy in the parent's state file or a dedicated batch context key. On resume, the scheduler rehydrates the graph and applies the policy consistently.

### Next steps:

1. **Clarify task list schema.** How does a parent agent submit a batch? Evidence submission, a new action, or a dedicated `--batch-spawn` command?
2. **Implement task graph tracking.** How is the parent's task list persisted? Append-only log, context key, or state file sidecar?
3. **Define skip state files.** Do skipped children have state files? If so, what's the minimal structure? If not, how are they recorded for `koto workflows --children`?
4. **Resume semantics.** Can a parent re-spawn a child that was skipped? Must the dependency be re-run first, or can it be force-spawned?

---

## Appendix: Example Scenarios

### Scenario: GH-issue plan with mixed dependencies

**Template:** Plan workflow with three issues: 1 (no deps), 2 (depends on 1), 3 (no deps).

**Execution with skip-dependents:**

1. Scheduler spawns issue 1 and 3 (no deps).
2. Issue 1 fails (tests don't pass, gate rejects PR).
3. Scheduler marks issue 2 as skipped (depends on 1).
4. Scheduler continues issue 3 (independent).
5. Issue 3 completes successfully.
6. Gate output: `{success: 1, failed: 1, skipped: 1, pending: 0, all_complete: true}`
7. Parent advances to a result-analysis state.
8. Parent agent submits `retry_failed` for issue 1.
9. Scheduler re-spawns issue 1 and issue 2.

**Execution with fail-fast:**

1. Scheduler spawns issue 1 and 3.
2. Issue 1 fails.
3. Gate returns `batch_failed`; issue 3 is cancelled (not spawned if not yet, killed if in-progress).
4. Parent blocks, awaiting evidence (retry or skip entire batch).

**Execution with continue-independent:**

1. Scheduler spawns issue 1, 2, and 3.
2. Issue 1 fails.
3. Issue 2 still runs (even though its dependency failed); may fail or succeed.
4. Issue 3 succeeds (independent).
5. Gate output: `{success: 1, failed: 1 or 2, pending: 0, all_complete: true}`
6. Parent decides what to do with failures.

---

## Summary Table: What Each Policy Means for the GH-Issue Use Case

| Scenario | Skip-Dependents | Fail-Fast | Pause-on-Failure | Continue-Independent |
|----------|---|---|---|---|
| Issue 1 fails | Issue 2 skipped, 3 continues | All cancelled, batch failed | Pause all, manual decision | All continue, 2 may fail |
| Issue 2 fails independently | Issue 3 continues | All cancelled | Pause all | Issue 3 continues |
| Recovery | `retry_failed` on issue 1 re-runs 1+2 | Retry whole batch | Manual resume, then schedule | Selective rerun of issue 1 |
| Fit to use case | **Excellent** | Poor (wastes parallelism) | Conservative (calls human) | Confusing (deps ignored) |

