# Lead: What does the session hierarchy view need to show at each level?

## Findings

### F2 Session-Feed Data Contract: Event Types and Hierarchy Fields

**What is captured:**

The `docs/reference/session-feed.md` specification (F2) defines 15 event types across three tiers:

- **Tier 1 (Required Display):** 8 events covering workflow lifecycle, gates, evidence, cancellation, and batch finalization.
  - `workflow_initialized`, `transitioned`, `directed_transition`, `rewound`, `evidence_submitted`, `workflow_cancelled`, `gate_override_recorded`, `batch_finalized`
- **Tier 2 (Optional Display):** 6 events for enriched audit trails.
  - `integration_invoked`, `context_added`, `default_action_executed`, `decision_recorded`, `gate_evaluated`, `child_completed`
- **Tier 3 (Internal):** 1 event for developer/audit purposes.
  - `scheduler_ran`

**Parent/Child Relationship Field:**

The header (line 1 of JSONL) includes an optional `parent_workflow` field (lines 22-25 in `session-feed.md`). This is the **only designated parent-child link** in F2:

```
parent_workflow:
  type: string
  required: false
  nullable: true
```

From `src/engine/types.rs` (lines 21-36 in `StateFileHeader`), the field appears as:

```rust
pub parent_workflow: Option<String>,
```

**Batch-Scoped Children:**

When a child is batch-spawned, the `workflow_initialized` event carries a `spawn_entry` (optional, lines 41-44 in session-feed.md):

```json
{
  "spawn_entry": {
    "template": "impl-issue.md",
    "vars": {"ISSUE_NUMBER": "303"},
    "waits_on": ["B", "C"]
  }
}
```

This is used for batch scheduling but does **not** establish a hierarchy—the parent-child relationship is already known via the header's `parent_workflow` field.

**Child Completion Notification:**

When a child reaches terminal state and is auto-cleaned, a `child_completed` event is appended to the **parent's** log (lines 230-246 in session-feed.md, and Tier 2):

```json
{
  "type": "child_completed",
  "payload": {
    "child_name": "parent-wf.task-1",
    "task_name": "task-1",
    "outcome": "success",
    "final_state": "done"
  }
}
```

This allows parents to reconstruct batch outcomes even after children are deleted.

### Fields Most Relevant for Dashboard Summary Rows

Based on session-feed events and existing `koto status` output patterns:

**For all workflow levels (root and child):**

1. **`workflow` (header)** — Workflow name. Essential identifier.
2. **`current_state` (derived from events)** — Latest `transitioned` event's `to` field. Shows where the workflow is.
3. **`created_at` (header)** — Workflow creation timestamp. Enables "elapsed time" calculation.
4. **`template_hash` (header)** — Immutable identity; enables change detection.
5. **`is_terminal` (derived)** — Computed by checking if current state is marked `terminal` in the compiled template.

**For root workflows (additionally):**

6. **`parent_workflow` (header, null for roots)** — Distinguishes roots from children. Drives whether to show in "root-only" filter.

**For batch-parents (additionally):**

7. **`batch` section (derived from `batch_finalized` or `children-complete` gate)** — Aggregated counts (`total`, `success`, `failed`, `skipped`, `pending`, `blocked`, `spawn_failed`) and task-level details. Defined in `src/cli/batch_view.rs` (`BatchView`, `BatchViewSummary`, `TaskView`).

**For child workflows in a batch (additionally):**

8. **`outcome` (from parent's `child_completed` event)** — Terminal outcome: `success`, `failure`, or `skipped`.
9. **`final_state` (from parent's `child_completed`)** — The child's terminal state name.
10. **`reason` (from parent's `child_completed` + context)** — Failure reason if applicable.

### Current Display Patterns (from koto status and koto workflows)

**koto status output (from `src/cli/mod.rs`):**

```json
{
  "name": "workflow-name",
  "current_state": "phase-name",
  "template_path": "...",
  "template_hash": "...",
  "is_terminal": false,
  "batch": { ... },          // Only for batch parents
  "superseded_branches": [...] // Epoch-branched children (koto rewind)
}
```

**koto workflows output (from `discover.rs`):**

Returns a sorted list of `WorkflowMetadata`:

```rust
pub struct WorkflowMetadata {
    pub name: String,
    pub created_at: String,
    pub template_hash: String,
    pub parent_workflow: Option<String>,
}
```

Children are discovered by filtering on `parent_workflow` field. The `--children <name>` flag filters to show only direct children.

**Batch view detail (from `src/cli/batch_view.rs`):**

```rust
pub struct BatchView {
    pub phase: BatchPhase,  // "active" or "final"
    pub summary: BatchViewSummary,  // Counts
    pub tasks: Vec<TaskView>,  // Per-task detail
    pub ready: Option<Vec<String>>,  // Name vectors (active phase only)
    pub blocked: Option<Vec<String>>,
    pub skipped: Option<Vec<String>>,
    pub failed: Option<Vec<String>>,
}
```

Each `TaskView` includes: `name` (full `<parent>.<task>`), `task_name` (short), `outcome`, `reason`, `skip_reason`, `skipped_because_chain`.

### Parent-Child Relationship Discovery Mechanism

From `batch_view.rs` (line 147+) and the scheduler in `batch.rs`:

- **Discovery:** `backend.list()` returns all sessions; filter by `parent_workflow` field.
- **Direct children only:** The header stores only the immediate parent, not a full ancestry chain. A grandchild carries its parent's name, not a root ancestor reference.
- **No session registry or metadata surface:** F2 spec explicitly notes (lines 775-786 in session-feed.md) that lifecycle metadata is "not part of this contract's current scope" — it belongs to a "session registry layer above the raw event log."

### Hierarchy Depth and Nesting Capability

**Current capability:**

- Arbitrary nesting depth is **technically possible** (a child can spawn its own children via a `materialize_children` state).
- **No current depth limit** in the code; recursion is supported.

**But in practice:**

- No dashboard UI yet exists to show multi-level hierarchies.
- The `koto workflows --children <name>` command shows only **direct children**, not grandchildren.
- Superseded branches (epoch-branched sessions from `koto rewind`) are tracked at the root level only via the `superseded_branches` field in `koto status` output.

**Visualization challenge identified in the lead:**

"How should nested depth be visualized without overwhelming the display?" — This is an open design decision. Current CLI output is flat per level.

## Implications

### 1. Hierarchy Metadata is Split Across Two Sources

- **Header fields** (`parent_workflow`, `created_at`, `session_id`) are stored in the state file and are immutable.
- **Derived state** (current phase, batch view, terminal status) is recomputed on every `koto status` call by replaying events.

A dashboard must:
- Load the header first to establish parent-child links.
- Read event logs to derive live state and batch progress.
- Understand that `parent_workflow` is the single source of truth for hierarchy.

### 2. Three Distinct Views Needed

1. **Root-level summary:** All workflows with `parent_workflow: null`. Show name, state, created_at, elapsed time.
2. **Direct children of a parent:** Filtered list with task-level outcome and reason (from parent's `child_completed` events).
3. **Parent-scoped batch view:** Aggregated counts + per-task detail (from `batch_finalized` or live `children-complete` gate).

Each view has different refresh requirements:
- Root list: read headers only (fast, low refresh cost).
- Parent status: read header + full event log (moderate cost).
- Batch detail: read parent + all children's logs (expensive, scales with child count).

### 3. Last-Gate-Result Field is Not in F2

The lead asks about "last gate result" as a dashboard field. **F2 does not capture gate results at the session level.** Gate-evaluated events are recorded (Tier 2), but:
- No summary "gate status" field exists.
- Consumers must scan the event log and extract the most recent `gate_evaluated` event for each gate.
- Different gates (command gates, context-exists gates, children-complete) have different output schemas.

This is a gap: a dashboard cannot show "last gate result" without custom event scanning or a new summary field added to the header or a new event type.

### 4. Elapsed Time Must be Calculated Client-Side

F2 provides `created_at` (RFC 3339 UTC timestamp). Elapsed time = now - created_at. No duration field is stored.

### 5. Batch Finalization Invalidates Prior Batch Views

The `batch_finalized` event can be superseded by later retries or rewinds (via the `superseded_by` field in the event). A dashboard showing multiple batch checkpoints must handle this annotation.

## Surprises

### 1. No Explicit Terminal Event

F2's "known gaps" section (lines 788-805) notes:

> "There is no `workflow_completed` event. Consumers determine whether a session has reached a terminal state by inspecting the most recent `transitioned` event and checking whether its `to` field matches the template's defined terminal states."

This means:
- A dashboard cannot determine completion from the session log alone; it must also load the compiled template.
- A crashed or incomplete session (last event is not a transition to a terminal state) is inferred as "still running" or "abandoned"—no explicit signal.

### 2. Batch Views Are Computed, Not Logged

The `batch_finalized` event freezes the final batch shape at one moment, but:
- The `children-complete` gate is re-evaluated on every `koto next` call.
- The batch view returned by `derive_batch_view()` is recomputed each time—it's not a stored snapshot except at finalization.
- Live batch progress requires re-running the scheduler's classification logic.

### 3. Epoch Branching (Tilde Names)

When a batch parent is rewound, children are relocated to epoch-branched sessions with names like `parent~1.task-a`. The parent's `superseded_branches` list tracks these, but:
- This is only populated in `koto status` output, not stored in the header or events.
- A dashboard reading raw JSONL has no way to discover epoch branches without scanning the filesystem for `<name>~*` patterns.

### 4. No Invocation Record in F2

F2 does not capture how a session was invoked (`koto init`, `koto init --parent`, batch-spawned, etc.). The `spawn_entry` field in `workflow_initialized` is only present for batch-spawned children—the invocation model is implicit in whether the field is present.

## Open Questions

### 1. Should the Dashboard Show Arbitrary Nesting Depth?

The lead asks how to visualize nested depth without overwhelming the display. Options:
- **Flat list per level:** Show only direct children (current CLI behavior). Expandable tree view in UI.
- **Full ancestry chain:** Include a breadcrumb or path like `root > batch-parent > child`. Requires walking up the header chain (O(depth) lookups).
- **Depth limit:** Show up to 3 levels; collapse deeper trees.

**No current guidance in the codebase.** This is a PRD decision.

### 2. What is the "Live Update" Model?

The lead scope mentions "live-update behavior." Options:
- **Poll the session log:** Read headers and event logs at fixed intervals (e.g., every 5 seconds). Cheap for roots; expensive for large batches.
- **Websocket / event stream:** koto does not expose a streaming API. F3 (the dashboard) would need to build one.
- **File-system watcher:** Monitor state files for changes; reload on modification. Works locally; not suitable for cloud-synced sessions.

**F2 and current koto code do not prescribe this.** It's an architectural choice for F3.

### 3. Is "Last Gate Result" a New Feature?

The lead lists "last gate result" as a candidate field. F2 doesn't provide this at the session level. Implementing it requires:
- **Option A:** Add a summary field to the header (e.g., `last_gate_outcomes: {"ci-passes": "passed", ...}`). Breaks the "header is immutable" model.
- **Option B:** Scan the event log and extract the most recent `gate_evaluated` for each gate per state. Client-side logic; no server change.
- **Option C:** Add a new event type (e.g., `gate_result_summary`) on each state transition. Increases event log size.

**This should be clarified before the PRD is finalized.**

### 4. Should Children Be Discoverable Without Parent's Log?

Currently, the `parent_workflow` field in the child's header is the only link. The parent's `child_completed` event is a fallback for cleanup scenarios. 

For a dashboard to show a parent's children after they're deleted, it must:
- Read the parent's log and extract `child_completed` events, OR
- Query the filesystem for `<parent>.*` session directories.

The first approach is slow (reads parent's full log); the second is filesystem-bound (doesn't work with cloud sync). **No optimal solution exists yet.**

### 5. How Should the Dashboard Handle Partial/Corrupt State Files?

F2 specifies "partial-write recovery" (lines 326-340): readers discard a truncated final line. A dashboard must:
- Gracefully skip corrupted files.
- Warn the user, but do not block other sessions.
- Decide: should an incomplete log be shown as "still running" or "abandoned"?

**No explicit guidance in the scope or existing code.**

## Summary

F2 defines a parent-child hierarchy via the header's `parent_workflow` field; the session-feed contains 15 event types across three tiers, with Tier 1 events (workflow_initialized, transitioned, batch_finalized) being essential for dashboard summary rows. Current display patterns (koto status, koto workflows, batch_view) show that workflow name, current_state, created_at, and is_terminal are universally needed at each level, while batch-specific fields (phase, task outcomes, aggregate counts) apply only to parent workflows. However, the lead's request for "last gate result" is not captured by F2 and would require new schema or client-side event scanning; the lack of an explicit terminal event means dashboards must load the compiled template to determine completion, and the absence of a session registry in F2 defers lifecycle metadata management (ownership, project tag) to a future layer.

