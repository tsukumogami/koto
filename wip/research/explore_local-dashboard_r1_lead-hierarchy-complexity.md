# Lead: What complexity exists in real nested orchestrator hierarchies?

## Findings

### 1. Documented Hierarchy Depth: 2-3 Levels

**Evidence:**
- Session-feed spec (docs/reference/session-feed.md, line 22-23): `parent_workflow` field defined as optional on header; comment on line 303 says "Name of the parent workflow for batch-spawned children. Absent for top-level sessions."
- StateFileHeader in src/engine/types.rs (line 34-36): `parent_workflow: Option<String>` carries only ONE parent reference — the immediate parent.
- DESIGN-batch-child-spawning.md Decision E1 (lines 184-245): Explicitly distinguishes "Reading A (flat declarative batch with sibling-level waits_on)" from "Reading B (nested batches via koto init --parent <running-child>)". The design explicitly supports grandchildren but treats nested batches as a separate, secondary capability.

**Why it matters:** Only one `parent_workflow` field exists per session header. A grandchild has only its immediate parent's name, not a grandparent reference. To reconstruct a full lineage (root → parent → child → grandchild), a dashboard would need to recursively walk backward through the hierarchy using parent_workflow pointers.

**Implication for display:** A tree view is feasible but not optimal at the data model level. The data provides parent pointers (backward references) rather than children pointers (forward references). Reconstructing a tree requires O(N) lookups per session to find all children by filtering parent_workflow == session_name across all sessions.

---

### 2. Two Distinct Orchestration Models Coexist

**Evidence:**
- DESIGN-batch-child-spawning.md Decision E1 (lines 184-245):
  - **Reading A:** Parent declares a flat task list via `materialize_children` hook with sibling-level `waits_on` dependencies. All children share the same parent; dependencies are expressed as "task B waits for task A", not nesting.
  - **Reading B:** A running child can spawn its own children via `koto init --parent <running-child>`. This creates true nesting: the child becomes a batch parent itself.

**Why it matters:** These are fundamentally different orchestration patterns:
- Reading A: "Parallel tasks with DAG dependencies" (GitHub-issue use case, all siblings under one parent)
- Reading B: "Hierarchical decomposition" (a child of a batch spawns its own sub-batch)

A single workflow can be a leaf in one batch AND a batch parent in another (e.g., an issue-implementation task that breaks its own work into sub-issues).

**Evidence for coexistence:** The design explicitly states (line 215): "Reading B can express everything Reading A expresses... Reading A can express everything Reading B expresses... the two compose rather than compete."

---

### 3. Sibling Sessions Exist and Are Central to the Design

**Evidence:**
- Batch scheduler tests (tests/batch_scheduler_test.rs, lines 234-268): Linear three-task batch creates three SIBLING child sessions: `parent.A`, `parent.B`, `parent.C`, all with the same `parent_workflow: "parent"`.
- Each sibling's `waits_on` list references sibling NAMES ("B waits_on A"), not full paths.
- Child-completed event (session-feed.md, lines 711-735): Records completion of a single child with full composed name (`parent.task-1`), task name (`task-1`), and outcome. The event is written to the PARENT's log, not the child's.

**Why it matters:** Sibling sessions create a flat-under-parent layer. A 1000-task batch (hard limit in design, line R6 of Decision 1) creates 1000 sibling sessions. The `children-complete` gate (used in tests and design) must wait for ALL siblings to reach terminal state.

**Implication for display:** A dashboard showing a parent with 1000 running children must handle a potentially large list. The design explicitly handles this: `children-complete` gate returns an aggregated view (`all_complete: true/false`, `total`, `success`, `failure`, `skipped`, etc.) rather than per-child details on every tick.

---

### 4. Parent-to-Child Relationship Is 1-to-Many for Batches

**Evidence:**
- DESIGN-batch-child-spawning.md Decision 1 (lines 568-666): The `tasks` field carries an array of task entries, each becoming a child. No limit on array size except R6 (lines 661): `tasks.len() <= 1000`.
- Test scenario (lines 234-268, 608-640): Single parent `parent` spawns multiple children `parent.A`, `parent.B`, `parent.C`.
- Batch validation R5 (line 660): "task names are unique within the submission" — enforces a flat namespace per submission.

**Why it matters:** The 1-to-many relationship is the PRIMARY way children are created in modern koto. Each child inherits the parent's name as a prefix: `<parent>.<task_name>`.

**Edge case:** v0.7.0 hierarchical workflows (Decision E2.1, lines 181-200) also supported legacy `koto init --parent <parent_name>` syntax, allowing a single child per call. This is superseded by batch spawning but still supported.

---

### 5. Hard Limits on Batch Depth and Complexity

**Evidence:**
- DESIGN-batch-child-spawning.md Decision 1, R6 (line 661):
  - Max tasks per batch: 1000
  - Max `waits_on` per task: 10
  - Max DAG depth: 50 (defined as "node count along the longest root-to-leaf path")

**Why it matters:** These limits define practical nesting depth. A deeply-nested linear chain (task 1 → 2 → 3 → ... → 50) can have at most 50 nodes. But a bushy DAG (10-wide fan-out at each level) can't go very deep.

**Implication for display:** Horizon is fixed. A tree view will never need to render more than 50 vertical levels, but could have 1000 nodes at a given level.

---

### 6. Complexity Peaks at Two Specific Points

**1. Batch re-classification (runtime state transitions):**
- Scenario 18-19 (tests/batch_scheduler_test.rs, lines 723-1022): A child can transition between two persistent state files:
  - Real template state file (work → done / failed)
  - Skip-marker state file (a terminal skipped_marker state)
- The scheduler respawns the child's state file on disk when classification changes (skipped → ready, or ready → skipped).

**Evidence:** Test scenario 18, lines 783-790: After A fails, D materializes as `skipped_via_upstream_failure`. After A is cleared and succeeds, D is respawned as a real child (lines 826-842).

**Why it matters:** The same child NAME can have two different state files during its lifetime. A dashboard tracking "current state" must handle this: the state file is replaced, but the session name persists and the parent's event log carries a `ChildCompleted` event when the old state file is cleaned up.

**2. Failure routing cascades:**
- Decision E5 (lines 973-1000): When task A fails, all of A's dependents (and their dependents, recursively) are marked skipped under `skip_dependents` (default policy).
- Example (test line 762): A → D (waits_on A). A fails → D becomes skipped. This is transitive: if D had dependents, they'd also become skipped.

**Why it matters:** A single failure can cascade down the dependency chain. A dashboard must track not just immediate children but transitive dependencies to show "why is this child blocked?"

---

### 7. Parent-Child Relationship Metadata: WorkflowInitialized.spawn_entry

**Evidence:**
- src/engine/types.rs (lines 94-130): `SpawnEntrySnapshot` struct carries:
  - `template`: source template path (as submitted by scheduler)
  - `vars`: variable bindings (BTreeMap to ensure stable serialization)
  - `waits_on`: sorted dependency list
- Session-feed spec (lines 399-403): `spawn_entry` field on `workflow_initialized` event is optional and "present only for batch-spawned child sessions."

**Why it matters:** The spawn entry is IMMUTABLE for the child's lifetime. It's recorded at spawn time and used by R8 (spawn-time immutability check): if the parent resubmits a task list, the scheduler compares the new entry against the recorded one and rejects mismatches.

**Implication for display:** A dashboard can inspect a child's initialization to see exactly how it was spawned, including its original template and variable bindings. This is stable across the session lifecycle.

---

### 8. Multi-Level Retry Is Blocked at Level Boundaries

**Evidence:**
- DESIGN-batch-child-spawning.md Decision E1, lines 223-229:
  - "When a Reading A batch nests a Reading B child (a coordinator whose child is itself a batch parent), retry stays at the level where the failure happened."
  - "`retry_failed` submitted at the outer level on a nested-batch child rejects with `InvalidRetryReason::ChildIsBatchParent`."

**Why it matters:** A dashboard showing retry/recovery options must NOT expose cross-level retry. If a coordinator child C is a batch parent with its own children, the outer parent cannot retry C's children directly; only C can retry its own batch.

**Implication for display:** Retry UI depends on the role of the target (leaf child vs batch coordinator). This adds a new dimension to the session view: not just "what state is the session in" but "can this session be retried from this level?"

---

## Implications

1. **Tree view is necessary but expensive:** Without explicit children pointers, reconstructing a full tree from backward references (parent_workflow fields) requires O(N²) in the worst case (scan all sessions to find children for each node). A local dashboard indexing sessions by parent_workflow would reduce this to O(N) on first load.

2. **Two-tier rendering:** Sessions have two roles:
   - **Leaf children** (no materialize_children hook): Render as simple task nodes with state + outcome.
   - **Batch coordinators** (with materialize_children hook): Render as collapsible groups with a ledger of spawned children.
   
   A single session can be both (coordinator child in an outer batch, batch parent to its own children).

3. **DAG view complements tree view:** The `waits_on` edges within a batch are not parent-child relationships; they're sibling dependencies. Showing these as a DAG (rather than nesting B under A) accurately reflects the semantics.

4. **Live-update behavior is simple within a batch:** The scheduler runs on every `koto next` call and is stateless. A dashboard can poll the parent session's event log and batch scheduler response for the current classification of all children. No persistent background cursors needed.

5. **Failure cascades require transitive closure:** To accurately show "why is task X blocked?" a dashboard must compute transitive dependencies (waits_on closure). This is computable once at dashboard load if the task list is static, but becomes dynamic if the parent resubmits new tasks.

---

## Surprises

1. **No dedicated nested-batch metadata:** I expected a `batch_id` or `submission_epoch` field to distinguish multiple batches from the same parent. Instead, batches are identified by the evidence submission event's `seq` number. This is elegant but means a dashboard must correlate scheduler outcomes with event log entries by sequence number.

2. **Skip markers are persistent state files, not transient flags:** I expected "skipped" to be a gate output or a field in a context object. Instead, skipped children have their own `state` files with a terminal `skipped_marker: true` state. This means a "skipped" child is a real workflow session on disk, not a metadata record.

3. **Batch depth limit (50) applies to the DAG, not the nesting:** I initially read the 50-level limit as applying to parent → child → grandchild nesting. It actually applies to the longest path in the task dependency DAG, which is orthogonal to orchestration nesting. A very deeply nested hierarchy (parent spawns child that spawns child...) could exceed 50 levels if it's not a batch.

4. **No session merging across batches:** I expected that retrying a failed child might "merge" its result with siblings. Instead, retry is scoped to the failed chain within its batch level. Cross-batch effects (e.g., outer parent retrying inner coordinator) are out of scope for v1.

---

## Open Questions

1. **Live-update semantics:** How should a dashboard handle the case where the same parent session has multiple materialized batches (submitted at different times)? Does the scheduler's "last seen evidence" determine the active batch, or can multiple batches coexist? (The spec says per-batch `failure_policy` on the hook, implying one batch per state, but it's not explicit whether a parent can have multiple `materialize_children` states or re-enter the same state.)

2. **Garbage collection policy for orphaned children:** The design mentions "orphan candidates" (children on disk whose short name is absent from the latest submission), but doesn't specify cleanup. Should a dashboard surface "orphan" as a session status? Should it offer manual cleanup UI?

3. **Cross-machine session references:** When a parent batch specifies relative paths for child templates (e.g., `./impl-issue.md`), and the parent's `template_source_dir` is not present on the dashboard machine, how should the dashboard render the template path? Should it surface a warning?

4. **Nested batch failure policy inheritance:** If an outer batch has `failure_policy: skip_dependents` and a nested coordinator child has `failure_policy: continue`, do they compose correctly? Does the outer policy skip the coordinator, even if the coordinator is still spawning children? (The design says retry is blocked at level boundaries, but doesn't detail how failure policies interact across levels.)

5. **Session-feed completeness for multi-level queries:** If a dashboard wants to answer "show me all work transitively spawned by this root", must it parse the entire session-feed of every descendant to reconstruct the tree, or is there a summary query available? (The current spec has per-session event logs; there's no "family tree" query.)

---

## Summary

Koto hierarchies have **3 levels maximum in practical practice** (root → batch parent → children), driven by hard limits (50-node DAG depth) and the v1 design constraint that retry is scoped to a single batch level. Sibling sessions are **the dominant pattern**—a 1000-task batch creates 1000 equal-level children under one parent, not a tree. **Both tree and DAG views are required**: the parent-child relationships form a tree, but sibling dependencies form a DAG within each batch. The biggest dashboard complexity is not depth but breadth and state transitions (a child can flip between running and skipped states, each with its own state file), plus transitive failure cascades that require computing the full waits_on closure.

