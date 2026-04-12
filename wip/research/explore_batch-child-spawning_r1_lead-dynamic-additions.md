# Research: Dynamic Child Spawning in Batch Workflows

**Lead:** lead-dynamic-additions  
**Issue:** #129 (Declarative batch child spawning)  
**Status:** Initial Findings  
**Date:** 2026-04-11

## Question

When a running child in a hierarchical workflow discovers sub-tasks mid-flight (e.g., parsing a plan that reveals dependent issues), how should it add siblings or grandchildren to the parent's batch? This explores two readings and recommends one.

---

## Reading A: "Append to Same Batch"

The running child calls `koto init --parent <parent> --in-batch-of <batch-id> <task-spec>` (or similar) to append a new task to the parent's existing batch. The appended task shares batch identity, participates in the parent's `children-complete` gate, and joins the DAG. Grandchildren also land in the original batch (flattened).

### 1. CLI Surface

**Option A1:** Explicit batch-append command  
```bash
koto batch add <batch-id> <task-spec> [--vars KEY=VAL...]
```
Child calls this to append a task JSON object to a batch manifest file.

**Option A2:** Materialization via evidence submission  
Child writes discovered tasks to context:
```bash
koto context add <parent> pending_tasks '{"tasks": [...], "batch_id": "b1"}'
```
Parent's action reads pending tasks and spawns via `koto init --parent`.

**Option A3:** Init flag with batch identity  
```bash
koto init --parent <parent> --in-batch-of <batch-id> <task-name>
```
Registers task in the parent's batch header/state while also creating child workflow.

### 2. Persistence and Identity

**What identifies "the batch"?**
- A batch-id field in the parent's state file header (e.g., `batch_id: "b1.scheduled"`)
- Each task entry persisted in parent's state file or a dedicated `.batch.json` sidecar
- DAG graph reconstructed from children's `waits_on` metadata on disk OR stored in parent state

**Challenges:**
- Parent state file grows unbounded as tasks append
- DAG lives in two places: task metadata (`waits_on`) and parent state (redundant)
- Sidecar files need cleanup and coordination with state file lifecycle

### 3. Resume Semantics

**Critical case:** Parent crashes after child requests "add sibling X" but before X was spawned.

**What happens on resume:**
- Append request must be durable (persisted before ack'd, e.g., written to parent state file)
- Resume reads parent state, sees pending add-request, replays it
- Race condition risk: add-request written to state file but child spawn never happened; on resume, task spawned twice (or idempotency key required)

**Idempotency:** Requires a deduplication key per appended task (batch-id + task-name + hash of spec).

### 4. Children-Complete Gate Interaction

**Scenario 1:** Gate fires before new sibling added  
- Gate reads current children count, says "all done"
- Parent advances past the gate
- New sibling is then added
- **Problem:** Sibling is orphaned; gate already passed and won't re-evaluate

**Scenario 2:** Gate fires while sibling being added (race)  
- Gate snapshot at T1 sees N children
- Task append happens at T1.5
- Parent checks gate again at T2, now sees N+1 children, gate fails again
- **Problem:** Gate must be idempotent and re-evaluable after state changes

**Recommendation:** Gate needs explicit "batch done" signal distinct from "all currently-known children terminal". Options:
- Parent explicitly marks batch closed before allowing gate to pass (`koto next --batch-done`)
- Gate only counts children whose names match a closed set (template-declared)
- Gate waits for both children complete AND a marker event (e.g., `BatchFinalized` event)

### 5. Nesting Compatibility

**v0.7.0 supports 3 levels:** grandparent → parent → child.

**Reading A flattens this:**
- Grandparent spawns parent (child of grandparent)
- Parent spawns child1 (via `koto init --parent parent`)
- Child1 appends grandchild1 to "parent's batch"
- Grandchild1 has `parent_workflow: parent` (not child1)
- **Result:** Grandchild is a sibling of child1, not a true grandchild
- **Consequence:** Nesting semantics break; level 4+ would require new batch IDs

---

## Reading B: "Start Nested Batch"

The running child becomes a parent of its own fresh batch via `koto init --parent <child>` (already works in v0.7.0). The outer parent's `children-complete` gate only waits for direct children; inner batch is the child's concern. Nesting composes naturally.

### 1. CLI Surface

**Existing (no new syntax):**  
```bash
koto init grandchild --parent <running-child> --template issue.md
koto init grandchild --parent <running-child> --with-batch @tasks.json
```

**Optional declarative batch shorthand:**  
```bash
koto init --parent <running-child> --with-batch @manifest.json \
  --batch-spec '{"completion": "terminal", "failure_policy": "fail-fast"}'
```
Declares multiple children in one call; koto materializes them.

### 2. Persistence and Identity

**What identifies "the batch"?**
- No explicit batch-id; each child has `parent_workflow: <name>` in its header
- Grandchildren have `parent_workflow: <running-child>`
- DAG lives in children's `waits_on` metadata only
- Parent state file unchanged; no batch manifest needed

**Advantages:**
- Reuses existing parent-child lineage model (StateFileHeader.parent_workflow)
- Each batch is independently scoped to its parent
- No sidecar files or parent state bloat

### 3. Resume Semantics

**Critical case:** Parent crashes after child requests "spawn grandchild X" but before X was initialized.

**What happens on resume:**
- Request IS the spawn (child uses `koto init --parent <child>`)
- Child's state file logs this as `WorkflowInitialized` (new child) or context entry
- Resume sees whatever children exist on disk
- **Idempotency:** Child names are the key; if `grandchild-<N>` already exists, re-running `koto init` fails with "already exists"
- **Recovery:** Agent must detect the failure and skip or retry

### 4. Children-Complete Gate Interaction

**Scenario 1:** Parent gate passes, child later spawns grandchild  
- Parent's gate only checks children with `parent_workflow: <parent>` (running-child is already complete → gate passes)
- Running-child's own gate (if declared) checks grandchildren with `parent_workflow: <running-child>`
- **No conflict:** Each level has its own gate and child set

**Scenario 2:** Grandchild added during parent's gate evaluation  
- Parent's gate snapshot sees completed child (running-child reached terminal)
- Child had already spawned grandchildren before reaching terminal
- Gate correctly reports child as done (regardless of grandchildren status)

**Advantage:** No "batch done" signal needed; gate model is simple and recursive.

### 5. Nesting Compatibility

**v0.7.0 supports 3 levels:** grandparent → parent → child.

**Reading B preserves this:**
- Grandparent spawns parent (child of grandparent)
- Parent spawns child1 (child of parent)
- Child1 spawns grandchild1 (child of child1) ← new level
- Grandparent's gate waits for parent to reach terminal
- Parent's gate waits for child1 to reach terminal (grandchildren don't block parent's gate)
- Child1's gate (if any) waits for grandchild1 to reach terminal
- **Result:** True 4-level nesting preserved; no hard limit (soft warning at level 4+)

---

## Comparison Table

| Dimension | Reading A | Reading B |
|-----------|-----------|-----------|
| **CLI Surface** | `koto batch add` or `--in-batch-of` flag | `koto init --parent <child>` (existing) |
| **Batch Identity** | Explicit batch-id in parent state | None; parent-pointer lineage |
| **Persistence** | Parent state file (sidecar or inline) | Child state files (existing) |
| **DAG Location** | Parent state + child metadata (redundant) | Child metadata only (single source) |
| **Resume** | Append-request replay (needs durability) | Spawn IS the request (idempotency by name) |
| **Gate "Done" Signal** | Required (batch must close) | Not needed (recursive gates) |
| **Nesting Levels** | Flattens to siblings (breaks hierarchy) | True nesting preserved (scales to level 4+) |
| **Code Complexity** | New batch-append logic in CLI | Zero new logic (reuses existing parents) |
| **Race Condition Risk** | Gate passes before append → orphan | None (each level independent) |
| **User Experience** | New verb to learn (`koto batch add`) | Reuse familiar `koto init --parent` |

---

## Recommendation: Reading B

**Reading B (nested batch) is simpler, safer, and better aligns with koto's existing v0.7.0 primitives.**

### Rationale

1. **Composition with v0.7.0:** The parent-child lineage and `children-complete` gate were designed for recursive application. A child reaching terminal state (because its own children completed) naturally signals completion to the parent. Reading B exploits this; Reading A breaks it.

2. **Resumability:** Reading B's idempotency is name-based (child workflow already exists → skip). Reading A requires durable append-request logging and replay, introducing a new failure mode (task spawned twice on resume).

3. **Gate Safety:** Reading B's recursive gate model has no race conditions. Each level's gate is independent. Reading A requires an explicit "batch done" signal and coordination between parent and children, adding complexity.

4. **Nesting Beyond 3 Levels:** v0.7.0 doesn't define a hard 3-level limit; it's just the tested scenario. Reading A would hit a ceiling (flattening breaks the model), while Reading B scales naturally to any depth.

5. **No New CLI Verbs:** Reading B leverages `koto init --parent` (already exists). Reading A introduces `koto batch add` or equivalent, adding surface area and learning curve.

### Caveat: Declarative Batch Sugar (Optional Follow-up)

Reading B doesn't preclude **declarative batch materialization** as syntactic sugar:
```bash
koto init --parent <child> --with-batch @manifest.json
```
This would let a skill submit multiple tasks at once (each getting a `koto init`), but under the hood each child is still a separate workflow with `parent_workflow: <child>`. The batch is a convenience at the skill layer, not a koto primitive.

---

## Concrete Example: Parse Plan + Spawn Issues

**Scenario:** Parent plan workflow parses a document and discovers 3 issues (1→2→3 dependency).

### With Reading B (Recommended)

**Plan parent template:**
```yaml
states:
  parse_and_spawn:
    directive: |
      Read the plan. For each issue without unmet dependencies, spawn a child:
      
      koto init plan.issue-{{ISSUE_NUM}} --parent plan \
        --template implement-issue.md --var ISSUE={{ISSUE_NUM}}
    gates:
      children-done:
        type: children-complete
    transitions:
      - target: summarize
        when:
          gates.children-done.all_complete: true
```

**Child issue template:**
```yaml
states:
  implement:
    directive: |
      Implement issue {{ISSUE}}. Check if dependent sub-issues exist.
      If yes, spawn them:
      
      koto init plan.issue-{{ISSUE}}-sub-{{SUB_NUM}} \
        --parent plan.issue-{{ISSUE}} \
        --template sub-issue.md
    gates:
      subs-done:
        type: children-complete
        name_filter: "plan.issue-{{ISSUE}}-sub"
    transitions:
      - target: done
        when:
          gates.subs-done.all_complete: true
  done:
    terminal: true
```

**Flow:**
1. Parent spawns issue-1, issue-2, issue-3 (issue-3 blocked on issue-1)
2. issue-1 starts, discovers sub-issues, spawns issue-1-sub-a, issue-1-sub-b (nested batch under issue-1)
3. issue-1's gate waits for its sub-issues to complete
4. When issue-1 reaches terminal (all subs done), parent's gate marks it as "completed"
5. Parent spawns issue-3 once issue-1 terminal
6. Parent's gate passes when all issues terminal

**Nesting:** `plan` → `plan.issue-1`, `plan.issue-1` → `plan.issue-1-sub-a` (true 3-level nesting)

**Resume:** If parent crashes after spawning issue-1-sub-a but before issue-1 reaches terminal:
- Resume reads parent state, sees issue-1 exists (already spawned)
- Resume reads issue-1 state, sees issue-1-sub-a exists (already spawned)
- Re-running `koto next plan` and `koto next plan.issue-1` resumes from where they left off

---

## Implementation Notes

No changes required to koto engine for Reading B's core semantics. The existing:
- `StateFileHeader.parent_workflow` (v0.7.0)
- `children-complete` gate (v0.7.0)
- `koto init --parent` (v0.7.0)

...already support nested batching. Future enhancements:
- `--with-batch @manifest.json` on init for declarative multi-task spawning (convenience, not necessity)
- Per-task `waits_on` metadata in evidence to declare inter-child dependencies
- Batch failure policy (fail-fast, skip-dependents, continue-all) as a task-level or parent-level setting

These can be layered without changing the core v0.7.0 hierarchy model.

---

## Decision

**Adopt Reading B (nested batch with natural composition).**

- Child uses `koto init --parent <running-child>` to spawn nested batches.
- No new batch-id or append verbs.
- Reuses v0.7.0 parent-child lineage.
- Scales to any nesting depth.
- Resumes correctly via disk-based idempotency.
- Future: Batch manifests and dependency declarations as convenience sugar.
