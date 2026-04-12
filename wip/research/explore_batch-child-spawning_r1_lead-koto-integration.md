# Batch Child Spawning — koto Integration Lead

**Issue**: #129 — parent workflow declaratively submits a DAG of children as
evidence; koto materializes and schedules them.

**Precondition**: v0.7.0 primitives (parent_workflow header field, `koto init
--parent`, `children-complete` gate, `workflows --children/--roots/--orphaned`)
are already live. This document is about where the **new batch materialization
and scheduling logic** plugs into the existing engine, not about the primitives
themselves.

This lead covers the "where does the code go" dimension only. Failure routing,
prior-art comparisons, and the dynamic-additions story live in sibling leads in
this directory.

---

## 1. Where materialization logic lives

### The four candidate insertion points

When the parent workflow's current state receives a batch definition as
evidence and the engine needs to spawn child workflows, there are four
plausible places to put that logic:

**(a) A new gate type `batch-materialize` that fires on state entry.**
Would live alongside `GATE_TYPE_CHILDREN_COMPLETE` in
`src/template/types.rs:163` and `src/gate.rs` `evaluate_gates`. The gate's
evaluator would read the batch definition from either evidence or a state key,
compute the ready set, and call the spawn path.

**(b) A state-level action / new action verb.**
`TemplateState` already has `default_action: Option<ActionDecl>`
(`src/template/types.rs:62`). We could add a second optional field
(`batch_action: Option<BatchActionDecl>`) or a new `ActionDecl.action_type`
discriminant and invoke it inside the `execute_action` closure at
`src/cli/mod.rs:1750`.

**(c) A new step in the advance loop, after transition succeeds but before
returning the directive.** The loop in `advance_until_stop`
(`src/engine/advance.rs:166–517`) has seven numbered phases (signal, chain,
terminal, integration, action, gates, transition). A new phase 8, "scheduler
tick", would run between the successful transition resolution and the next
iteration.

**(d) At `koto next` CLI level, before invoking the engine.**
`handle_next` (`src/cli/mod.rs:1259–2068`) reads events, compiles the template,
runs the advance loop, maps the stop reason to a `NextResponse`. We could
insert a batch-scheduler call just before or just after `advance_until_stop`
(line 1835), driven off a template hint on the current state.

### Picked insertion point: **(d) CLI-level, with a dedicated scheduler tick**

**Justification.** The advance loop in `src/engine/advance.rs` is deliberately
I/O-free: it takes closures (`append_event`, `evaluate_gates`,
`invoke_integration`, `execute_action`) rather than talking to the session
backend. The spawn path, however, needs exactly what the advance loop lacks:
the concrete `&dyn SessionBackend` plus the compile cache (`compile_cached`)
and `handle_init`'s variable-resolution machinery. Forcing that through yet
another closure would inflate `advance_until_stop`'s signature (already nine
parameters, already tagged `#[allow(clippy::too_many_arguments)]` at line
165) and bleed session/template concerns into the pure state-machine core.

Candidate (a), a gate type, is tempting because children-complete already
lives there — but gates are read-only predicates in the current model. Giving
`evaluate_gates` side-effectful spawn authority would break the invariant that
gates are re-runnable and idempotent across epochs.

Candidate (b), a new action verb, has the same problem as (a) for a different
reason: `execute_action` is called inside `advance_until_stop` and
`ActionResult` only carries process-exit data. Spawning children fits neither
the shell-command nor the polling shape.

Candidate (c), a new loop phase, runs inside `advance_until_stop` where the
session backend isn't available.

(d) is the only option where we have all three inputs — session backend,
compiled template, and epoch events — already assembled, and where the output
(child workflow state files) fits naturally alongside the existing
`NextResponse` serialization.

### Where in `handle_next` the call goes

The concrete insertion point is in `handle_next` at `src/cli/mod.rs:1835`,
immediately after `advance_until_stop` returns. Pseudocode:

```rust
let result = advance_until_stop(current_state, &compiled, &evidence,
    &current_events, &mut append_closure, &gate_closure,
    &integration_closure, &action_closure, &shutdown);

// NEW: scheduler tick. Runs after the advance loop has settled on the final
// state. If the final state carries a `batch:` frontmatter block, run the
// scheduler to spawn any children whose `waits_on` dependencies are now
// satisfied. The scheduler mutates child state files via the same backend.
let scheduler_outcome = match &result {
    Ok(ar) => run_batch_scheduler(
        &backend,
        &compiled,
        &ar.final_state,
        &name,          // parent workflow name
        &current_events,
    )?,
    Err(_) => SchedulerOutcome::NotApplicable,
};

// 8. Map advance result → NextResponse (existing code).
```

The new `run_batch_scheduler` function would live in a new module
`src/engine/batch.rs` (re-exported from `src/engine/mod.rs` which currently
only holds `pub mod advance; pub mod ...;`). It takes:

- `&dyn SessionBackend` (the same `backend` handle already in scope)
- `&CompiledTemplate` (already loaded at `src/cli/mod.rs:1419`)
- the final state name (from `AdvanceResult.final_state`)
- the parent workflow name
- the full event slice (needed to derive the batch definition and spawn
  records from the log)

It returns a `SchedulerOutcome` enum: `NoBatch`, `Scheduled { spawned: Vec<…>
}`, or `Error { reason }`. The existing response match at
`src/cli/mod.rs:1890` is amended to include the spawn count in the
`EvidenceRequired` branch's response, so the agent sees how many children
were spawned on this tick.

**Why not earlier, before `advance_until_stop`?** Because we need the
*final* state, not the starting state. A template might have a `plan → fanout`
auto-transition that runs in the same `koto next` call. Running the scheduler
on `plan` would spawn nothing; running it on the post-advance `fanout` state
spawns the batch. The CLI already has access to both — we just pick the
later one.

---

## 2. State persistence for the batch

The scheduler needs to know, between `koto next` invocations, **which tasks
in the batch have already been spawned**. Three storage strategies:

### (a) New `batch` section on the parent's state file header

`StateFileHeader` (`src/engine/types.rs:9–25`) currently has
`schema_version, workflow, template_hash, created_at, parent_workflow`. We'd
add:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub batch: Option<BatchHeader>,

pub struct BatchHeader {
    pub tasks: Vec<BatchTask>,      // the declarative DAG
    pub spawned: BTreeMap<String, String>,  // task_name -> child_wf_name
}
```

- **Stored**: the canonical task list (with `waits_on`) plus a map from task
  name to spawned child workflow name.
- **Write cost on each `koto next`**: high. The header is currently written
  once by `append_header` (`src/engine/persistence.rs:12`) and then never
  touched — the state file is strictly append-only JSONL after that. To
  mutate `batch.spawned` we'd have to rewrite the entire file, which breaks
  the append-only guarantee that makes state files safe to sync and tail. A
  partial rewrite is possible but invalidates `read_last_seq`
  (`src/engine/persistence.rs:88`) and the `expected_seq` check in
  `read_events` (`src/engine/persistence.rs:148`).
- **Cloud sync interaction**: bad. The cloud backend (`src/session/cloud.rs`)
  presumably relies on append-only semantics to do incremental uploads. A
  mutable header forces a full re-upload per spawn.
- **Verdict**: rejected. Breaks append-only.

### (b) Separate `<parent>.batch.jsonl` sidecar in the parent's session directory

`backend.session_dir(&name)` (`src/session/mod.rs:49`) already exists and
returns a writable PathBuf. We'd write a second JSONL file next to
`koto-<parent>.state.jsonl` containing batch-specific events:

```
{"seq":1,"type":"batch_defined","payload":{"tasks":[…]}}
{"seq":2,"type":"task_spawned","payload":{"task":"foo","child":"parent.foo"}}
{"seq":3,"type":"task_spawned","payload":{"task":"bar","child":"parent.bar"}}
```

- **Stored**: a strict append-only log of batch events, mirroring the main
  state-file semantics.
- **Write cost on each `koto next`**: one append per newly-spawned task,
  plus one read of the entire sidecar to rebuild the spawn map. For a 10-task
  batch the sidecar is at most 11 lines, so cost is negligible.
- **Cloud sync interaction**: fine. Another append-only JSONL matches what
  the cloud backend already handles.
- **Verdict**: works, but introduces a second log file that must stay in sync
  with the main log on rewind, cleanup, and cancel. `handle_rewind`
  (`src/cli/mod.rs:1163`) and `session::handle_cleanup` would need to know
  about it. That's new coupling.

### (c) Fully derived from on-disk child state files plus the parent's event log

No new storage. The batch definition is already in the main log as an
`EvidenceSubmitted` event (from `src/engine/persistence.rs:236 derive_evidence`).
"Which tasks have been spawned" is derived from `backend.list()` filtered by
`parent_workflow == parent_name`, matched against the declared task names by a
deterministic naming rule (see section 3).

- **Stored**: nothing new. Batch definition lives in the existing evidence
  event; spawn records live as child state files on disk.
- **Write cost on each `koto next`**: one `backend.list()` call (already cheap
  because it's how children-complete works today — see
  `src/cli/mod.rs:2478 evaluate_children_complete`) plus one string-compare
  pass per task in the batch. For a 10-task batch, O(10) name comparisons.
- **Cloud sync interaction**: best case — no new files. The existing
  `parent_workflow` header field on each child is the source of truth.
- **Verdict**: **recommended**.

### Recommendation: **(c)**

Strategy (c) piggybacks on the exact discovery mechanism that
`evaluate_children_complete` (`src/cli/mod.rs:2471`) already uses. It adds
zero new storage paths, zero new sync concerns, and zero new cleanup hooks.
The only requirement it imposes is that the child workflow name be
deterministic from the parent name + task name (handled in section 3).

The batch definition itself lives in the parent's event log as an
`EvidenceSubmitted` event — no new event type needed. The scheduler reads
the latest-epoch `batch_definition` field out of the evidence map, exactly
like the existing transition resolver reads `when`-clause fields.

---

## 3. Idempotency

On resume, the scheduler must not double-spawn a task. The spawn operation
ultimately calls `handle_init` (`src/cli/mod.rs:1029`), which at line 1059
rejects `backend.exists(name)`:

```rust
if backend.exists(name) {
    exit_with_error(serde_json::json!({
        "error": format!("workflow '{}' already exists", name),
        "command": "init"
    }));
}
```

So the question becomes: **what name do we pass to `koto init --parent`**?

### Naming strategy candidates

- **User-provided child workflow name** (the `name` field on each batch task).
  Simple. The agent picks unique names. Downside: if the agent re-runs a
  workflow with a new batch, collisions are easy. And there's no back-pointer
  from a batch task to its child without a spawn map.

- **Batch-id + task-name** (`batch-abc123.task-name`). Deterministic given a
  batch id, but requires generating and persisting a batch id per batch. More
  state, more hash-adjacent logic.

- **Parent-name + task-name** (`parent.task-name` or
  `parent__task-name`). No extra state. The child name is a pure function of
  parent name and task name. `backend.exists` is the idempotency check.

### Recommendation: **parent-name + task-name**

Use the naming rule `<parent_name>.<task_name>` (or `__` if `.` conflicts
with any existing validation in `validate_workflow_name` at
`src/discover.rs`). This gives us:

- **Free idempotency check**: `backend.exists("parent.task")` is the existing
  check at `src/cli/mod.rs:1059`. The scheduler loops over ready tasks, calls
  `backend.exists` on each, and skips those already present.
- **No new state**: nothing to persist. Resume is a pure function of
  "which child state files exist on disk."
- **Trivial parent back-pointer**: the child already has
  `parent_workflow = Some(parent_name)` in its header
  (`src/cli/mod.rs:1118`), and its short task name is the suffix of its
  workflow name.
- **Couples child name to parent name**: acceptable tradeoff. A parent
  workflow renaming isn't supported anyway (the name is the session id).

### The spawn loop, with the idempotency check in place

Pseudocode for the spawn step inside `run_batch_scheduler`:

```rust
for task in ready_tasks {
    let child_name = format!("{}.{}", parent_name, task.name);

    // Idempotency: skip if the child already exists. This is the same
    // check handle_init performs at src/cli/mod.rs:1059, but we do it
    // here to avoid a redundant compile_cached() call and to produce
    // a SchedulerOutcome::AlreadySpawned entry instead of an error.
    if backend.exists(&child_name) {
        continue;
    }

    // Reuse the same code path as `koto init --parent`. This is either
    // a refactor extracting the body of handle_init into a helper
    // `init_workflow(backend, child_name, template_path, vars, Some(parent_name))`
    // or a direct call to handle_init if we teach it to return a Result
    // instead of calling exit_with_error.
    init_workflow(
        backend,
        &child_name,
        &task.template_path,
        &task.vars,
        Some(parent_name),
    )?;
    spawned.push(child_name);
}
```

### Why the check prevents double-spawn on resume

Suppose the user runs `koto next parent`, the scheduler spawns `parent.task-3`
(writing the child's state file), and *then* the process crashes before
returning. On the next `koto next parent`, the scheduler runs again:

1. It reads the batch definition from the parent event log (unchanged).
2. It recomputes ready set (task-3 is still "ready" because the parent
   doesn't know about the spawn).
3. It calls `backend.exists("parent.task-3")`, which returns `true` because
   the crashed run persisted the state file before dying.
4. The spawn loop skips task-3.

No new state needed. The child state file on disk *is* the spawn record.

The only thing that can go wrong is a crash **between creating the session
directory (`backend.create` at `src/cli/mod.rs:1067`) and writing the header
(`backend.append_header` at `src/cli/mod.rs:1120`)**. The directory exists
without a state file, `backend.exists` checks for the state file specifically
(`src/session/mod.rs:52` "state file present, not just directory"), so we're
fine: the next run will see `exists() == false` and retry. The orphaned
directory gets cleaned up by the create call which is presumably idempotent
(or the retry tolerates an existing directory).

---

## 4. Scheduler step — pseudocode walkthrough

Full pseudocode for `run_batch_scheduler`, referencing the real functions it
would call into:

```rust
// src/engine/batch.rs (new file)

pub enum SchedulerOutcome {
    NoBatch,
    Scheduled { spawned: Vec<String>, already: Vec<String>, blocked: Vec<String> },
    Error { reason: String },
}

pub fn run_batch_scheduler(
    backend: &dyn SessionBackend,
    template: &CompiledTemplate,
    current_state: &str,
    parent_name: &str,
    events: &[Event],
) -> Result<SchedulerOutcome, BatchError> {
    // 1. Check if the current state declares a batch hook.
    //
    //    NEW template field: TemplateState.batch: Option<BatchHook>
    //    where BatchHook declares the evidence field that holds the task list
    //    (e.g. batch_definition_field: "tasks") and optional template
    //    overrides for child workflows.
    let state = template.states.get(current_state)
        .ok_or_else(|| BatchError::UnknownState(current_state.into()))?;
    let batch_hook = match &state.batch {
        Some(h) => h,
        None => return Ok(SchedulerOutcome::NoBatch),
    };

    // 2. Extract the task list from the parent's latest-epoch evidence.
    //    Reuse derive_evidence (src/engine/persistence.rs:236) which already
    //    returns the current epoch's EvidenceSubmitted events.
    let epoch_evidence = derive_evidence(events);
    let merged = merge_epoch_evidence(
        &epoch_evidence.into_iter().cloned().collect::<Vec<_>>()
    );
    let tasks_value = match merged.get(&batch_hook.field_name) {
        Some(v) => v,
        None => return Ok(SchedulerOutcome::NoBatch),
    };
    let tasks: Vec<BatchTask> = serde_json::from_value(tasks_value.clone())
        .map_err(BatchError::InvalidBatchDefinition)?;

    // 3. Build the DAG and validate no cycles.
    //    NEW helper in src/engine/batch.rs.
    let dag = build_dag(&tasks)?;   // returns Err on cycle
    dag.validate_no_cycles()?;

    // 4. Query which children already exist and their current states.
    //    Reuses the same list+filter pattern as evaluate_children_complete
    //    at src/cli/mod.rs:2471.
    let all_sessions = backend.list()
        .map_err(|e| BatchError::BackendError(e.to_string()))?;
    let existing_children: HashMap<String, SessionInfo> = all_sessions
        .into_iter()
        .filter(|s| s.parent_workflow.as_deref() == Some(parent_name))
        .map(|s| (s.id.clone(), s))
        .collect();

    // 5. Classify each task by its current status.
    //    - Terminal: its child state file exists AND the child's derived
    //      current state is marked terminal in the child's compiled template.
    //    - Running: exists but not terminal.
    //    - Blocked: a waits_on dependency is not terminal.
    //    - Ready: all waits_on dependencies are terminal, and the task itself
    //      has not been spawned yet.
    //
    //    Reuses derive_machine_state (src/engine/persistence.rs:395) to
    //    resolve each existing child's current state, exactly like
    //    evaluate_children_complete does at src/cli/mod.rs:2538.
    let mut task_status: HashMap<String, TaskStatus> = HashMap::new();
    for task in &tasks {
        let child_name = format!("{}.{}", parent_name, task.name);
        task_status.insert(task.name.clone(), classify_task(
            &task, &child_name, &existing_children, backend,
        ));
    }

    // 6. Compute the ready set: tasks whose waits_on are all Terminal and
    //    whose own status is NotYetSpawned.
    let ready: Vec<&BatchTask> = tasks.iter().filter(|t| {
        let own = task_status.get(&t.name).unwrap();
        matches!(own, TaskStatus::NotYetSpawned)
            && t.waits_on.iter().all(|dep|
                matches!(task_status.get(dep), Some(TaskStatus::Terminal)))
    }).collect();

    // 7. Spawn each ready task. Uses the same init_workflow helper that
    //    `koto init --parent` calls (refactored out of handle_init at
    //    src/cli/mod.rs:1029–1160).
    let mut spawned = vec![];
    for task in ready {
        let child_name = format!("{}.{}", parent_name, task.name);
        if backend.exists(&child_name) {
            // Belt-and-suspenders: classify_task should have caught this.
            continue;
        }
        init_workflow(
            backend,
            &child_name,
            &task.template_path,
            &task.vars,
            Some(parent_name),
        ).map_err(|e| BatchError::SpawnFailed { task: task.name.clone(), err: e })?;
        spawned.push(child_name);
    }

    // 8. Compute the remaining blocked/running sets for the caller.
    let already: Vec<String> = task_status.iter()
        .filter(|(_, s)| matches!(s, TaskStatus::Running | TaskStatus::Terminal))
        .map(|(n, _)| format!("{}.{}", parent_name, n))
        .collect();
    let blocked: Vec<String> = task_status.iter()
        .filter(|(_, s)| matches!(s, TaskStatus::BlockedByDep))
        .map(|(n, _)| format!("{}.{}", parent_name, n))
        .collect();

    Ok(SchedulerOutcome::Scheduled { spawned, already, blocked })
}
```

### Existing helpers reused

| Helper | File | Purpose |
|---|---|---|
| `derive_evidence` | `src/engine/persistence.rs:236` | pulls current-epoch evidence events |
| `merge_epoch_evidence` | `src/engine/advance.rs:623` | flattens multiple evidence submissions |
| `derive_machine_state` | `src/engine/persistence.rs:395` | classifies each child's terminal status |
| `backend.list()` | `src/session/mod.rs:58` | discovers existing children |
| `backend.exists()` | `src/session/mod.rs:52` | idempotency check |
| `validate_workflow_name` | `src/discover.rs` | validates child name format |
| `compile_cached` | `src/cache.rs` | compiles per-task child templates |

### New functions to add

| Function | File | Purpose |
|---|---|---|
| `init_workflow` | `src/cli/mod.rs` (refactored from `handle_init`) | extract body so both the CLI and scheduler can call it |
| `run_batch_scheduler` | `src/engine/batch.rs` (new) | top-level scheduler tick |
| `build_dag` / `validate_no_cycles` | `src/engine/batch.rs` | DAG construction |
| `classify_task` | `src/engine/batch.rs` | ready/blocked/running/terminal classification |
| `BatchHook`, `BatchTask`, `BatchError` | `src/engine/batch.rs` + `src/template/types.rs` | types |

---

## 5. Resume flow — concrete walkthrough

### Setup

- Parent workflow `p` is at state `awaiting_children`.
- The batch definition in the latest `EvidenceSubmitted` event is 10 tasks:
  `t1..t10`.
- `t1, t2, t3` have terminal child state files.
- `t4, t5` are running (not terminal).
- `t6` waits_on `t1` (terminal, so not blocked by dep). But `t6` *also* waits
  on `t-failed` which reached a failed terminal state. The task classifier
  sees `t-failed` as "Terminal" (by strict current definition — `children-
  complete` only checks `terminal: true`, not success). So `t6` is **ready**
  under the current model. This is a failure-routing concern and is covered
  in the sibling `lead-failure-routing.md` doc. For *this* walkthrough, we
  assume "blocked on a failed dep" means: `t-failed` is a terminal state
  that some user convention marks as failed, and the scheduler treats it as
  non-success. In the minimum viable design, "blocked on failed" is
  functionally identical to "blocked on not-yet-terminal" — we just don't
  spawn.
- `t7, t8, t9, t10` are not yet spawned. `t7` waits on `t4`; `t8, t9, t10`
  wait on `t6`.

### Step-by-step: what `koto next p` does

1. **`handle_next` boot** (`src/cli/mod.rs:1259`). Loads `p`'s state file via
   `backend.read_events`, yielding `(header, events)`. Header has no special
   batch-related fields.

2. **`derive_machine_state`** (`src/engine/persistence.rs:395`) returns
   `current_state = "awaiting_children"`.

3. **Advance loop** (`advance_until_stop` at `src/engine/advance.rs:166`).
   The `awaiting_children` state has a `children-complete` gate. Gate
   evaluation (`src/cli/mod.rs:2471 evaluate_children_complete`) lists
   children, sees 5 exist (t1..t5), 3 are terminal, 2 are not. Gate outcome
   is `Failed` because `all_complete = false`. Stop reason is either
   `GateBlocked` or `EvidenceRequired` depending on how the template routes
   the gate output.

4. **Scheduler tick** (new, at `src/cli/mod.rs:1835` — right after the
   advance loop). `run_batch_scheduler` is called with
   `final_state = "awaiting_children"` (the advance loop didn't move).
   - The state has a `batch` hook → proceed.
   - Parse tasks from the latest-epoch evidence event. Get 10 tasks.
   - Build DAG. No cycles.
   - Classify each task by name:
     - `t1,t2,t3` → `Terminal` (child state file exists + child is terminal)
     - `t4,t5` → `Running`
     - `t6` → `NotYetSpawned` (blocked-on-failed under the strict "failed
       counts as terminal" interpretation — see failure-routing lead)
     - `t7` → `NotYetSpawned` + `BlockedByDep` (waits on `t4`, which is
       `Running`)
     - `t8,t9,t10` → `NotYetSpawned` + `BlockedByDep` (wait on `t6` which
       is not yet terminal)
   - Under the strict model, `t6`'s deps are all terminal, so `t6` is
     **ready**. The scheduler spawns `p.t6`.
   - No other task is ready (everything else is running, already terminal,
     or waiting on something non-terminal).
   - Returns `SchedulerOutcome::Scheduled { spawned: ["p.t6"], …}`.

5. **Response mapping**. The existing match at `src/cli/mod.rs:1890` fires,
   producing a `GateBlocked` response. The scheduler outcome is attached as
   an extra field, so the agent sees both "5 of 10 children pending" (gate
   blocking condition) and "spawned p.t6 this tick" (scheduler hint). This
   is purely informational — the engine does not auto-advance past the
   gate just because it spawned a child.

### Where does the ready set come from?

The ready set is **not persisted**. It's recomputed from scratch on every
`koto next` by walking the task list and classifying each task against the
current state of the world (child state files on disk). The computation is
O(tasks × avg_deps) plus one `backend.list()` call — negligible for realistic
batch sizes (tens of tasks).

### How does it know which 4 to spawn?

It doesn't have to "know" — it derives. The four unspawned tasks are
identified by: "declared in the batch definition" AND "`backend.exists(child
name)` returns false". The ready subset of those is computed by the dep
check. No bookkeeping required.

### What if the crash happened between spawning `t3` and persisting that fact?

Under strategy (c), there *is* no separate "fact" to persist. Spawning `t3`
*is* writing the child state file. Two sub-cases:

- **Crash after `backend.create` but before `backend.append_header`**: the
  session directory exists without a state file. `backend.exists(child)`
  returns `false` (checks state file, not directory — `src/session/mod.rs:52`
  comment confirms this). The next `koto next` call retries and overwrites
  the directory.

- **Crash after `backend.append_header` but before `backend.append_event`
  (the WorkflowInitialized event at `src/cli/mod.rs:1132`)**: the state
  file exists with only a header. `backend.exists(child)` returns `true`.
  `read_events` returns an empty event list. The child is neither terminal
  nor "running" — it's in a degenerate state. `handle_next` on the child
  hits the `events.is_empty()` check at `src/cli/mod.rs:1337` and errors
  with `PersistenceError`.

  The scheduler classifies this child as "exists but not terminal", i.e.
  Running. Downstream tasks that depend on it are Blocked. The user has
  to manually clean up the half-initialized child (`koto session cleanup
  p.t3`) before progress can resume.

  **Mitigation**: either make `handle_init`'s append_header + append_event
  pair atomic (write both to a temp file, rename), or add a "repair"
  subcommand that detects header-only state files and deletes them.

### The single corner case that *does* require new state

If the batch definition itself can change between `koto next` calls — e.g.,
the agent resubmits evidence with a modified task list — then "derived from
on-disk state" isn't enough to tell whether a child was spawned under the
old definition or the new one. Two options:

1. **Reject batch edits**: once a `batch_definition` evidence event has been
   submitted, subsequent submissions for the same field are rejected by a
   new compile-time or runtime rule.
2. **Hash the batch definition and include it in the child name**:
   `p.t3.abc123def`. Then new-definition children coexist with old.

Option 1 is simpler and consistent with the "declarative evidence" framing
in #129. Option 2 belongs in the dynamic-additions sibling lead.

---

## 6. Compiler validation additions

`src/template/compile.rs` is 1,219 lines. The validation entry point is
`CompiledTemplate::validate` (`src/template/types.rs:309`). New rules to add:

1. **`batch` hook schema validation**. If `TemplateState.batch` is Some, the
   referenced evidence field must appear in the state's `accepts` block.
   Added to the loop at `src/template/types.rs:334`. Error message:

   > state {state}: batch hook references field {field} which is not
   > declared in the accepts block

2. **Batch hook requires children-complete gate**. A state with a `batch`
   hook should typically also have a `children-complete` gate so the workflow
   actually waits for the spawned children. Warning (not error) if the state
   has a batch hook but no children-complete gate — analogous to the D5
   "legacy gate detection" warning at `src/template/types.rs:594`.

3. **Batch hook mutually exclusive with integration and default_action**.
   The state can't do everything at once. Error, mirroring the existing
   check at `src/template/types.rs:547` that rejects "both integration and
   default_action".

4. **Child template path reference validation**. If `BatchTask.template_path`
   is referenced as a variable (e.g. `{{CHILD_TEMPLATE}}`), ensure that
   variable is declared — same pattern as the variable-reference check at
   `src/template/types.rs:519–527`.

5. **Batch hook reachability**. A state with a batch hook must be reachable.
   This is already implied by the existing reachability check, but may need
   to be tightened: a batch-hook state must be reached via a transition that
   routes on evidence containing the batch definition.

Minor: update `gate_type_builtin_default` at `src/template/types.rs:218` if
a new `batch-complete` gate type is added (not planned per the "picked
insertion point" in section 1).

---

## 7. Backward compatibility

### State file format

- `StateFileHeader` is untouched. v0.7.0 state files are read identically.
- No new event types. Batch definition flows through the existing
  `EvidenceSubmitted` event. Existing `Event::Deserialize`
  (`src/engine/types.rs:149`) doesn't change.
- The only concern is the *reserved* `"gates"` evidence key. A new reserved
  key (e.g. `"batch"`) would require a matching rejection check in
  `handle_next`, analogous to `src/cli/mod.rs:2853 handle_next_gates_key_
  returns_invalid_submission`. Old state files with no such field are fine.

### Template format

- New optional `TemplateState.batch` field. `#[serde(default,
  skip_serializing_if = "Option::is_none")]` keeps v0.7.0 templates
  round-trippable.
- `format_version` stays at 1. No schema bump unless we add a new reserved
  top-level field. (Current check at `src/template/types.rs:310` rejects
  anything other than format 1.)
- Compiler validation rules in section 6 are strictly additive: an old
  template that doesn't use `batch` hits none of them.

### CLI surface

- `koto init --parent` unchanged. The scheduler uses the same code path via
  an extracted `init_workflow` helper (refactor, not reshape).
- `koto next` response shape adds one optional field (`scheduler` / `batch`)
  on the non-error branches. Old clients that ignore unknown fields are
  unaffected. If that's too aggressive, gate behind `--full` or a separate
  `koto batch status` subcommand.
- `koto workflows --children` unchanged; the new children just show up
  naturally.

### Gate surface

- The `children-complete` gate (`src/gate.rs:77`) works unchanged. It
  already walks `backend.list()` filtered by `parent_workflow`, which is
  exactly how the scheduler finds spawned children. No interaction.

### Failure modes

- Old parent state files with no `batch` hook in their template: scheduler
  tick returns `NoBatch` immediately. Zero cost.
- A batch-hook template compiled against v0.7.0 (which didn't know about
  the field): the template *compiles* on v0.7.0 because serde ignores
  unknown fields on Deserialize... actually this depends on whether
  `CompiledTemplate` uses `#[serde(deny_unknown_fields)]`. Quick check:
  `src/template/types.rs:21–32` does **not** set `deny_unknown_fields`, so
  v0.7.0 will silently ignore the new field. This is good for forward
  compat but bad for diagnosability — a user who authors a batch template
  and runs it under an old koto binary will get a silent no-op instead of
  an error.

  **Mitigation**: bump `format_version` to 2 when the batch feature lands,
  OR add `deny_unknown_fields` on `TemplateState` (retroactively; affects
  all templates). The latter is a breaking change risk and should be
  considered separately.

---

## 8. "What I'd change in code" summary

| File | Delta |
|---|---|
| `src/engine/mod.rs` | Add `pub mod batch;`. Currently 8 lines; becomes 9. |
| `src/engine/batch.rs` (new) | Contains `run_batch_scheduler`, `SchedulerOutcome`, `BatchError`, `BatchTask`, `TaskStatus`, DAG builder, and the classify/ready/spawn loop. This is the bulk of the new code. |
| `src/template/types.rs` | Add `TemplateState.batch: Option<BatchHook>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Add `BatchHook` struct (field name, optional child template overrides). |
| `src/template/compile.rs` (or `types.rs::validate`) | Add the five validation rules from section 6. Most likely extend `CompiledTemplate::validate` at `types.rs:309`. |
| `src/cli/mod.rs` `handle_init` | Refactor body (lines 1029–1160) into a reusable `pub(crate) fn init_workflow(backend, name, template, vars, parent) -> Result<InitOutcome, InitError>` that returns errors instead of calling `exit_with_error`. `handle_init` becomes a thin wrapper that handles the exit-on-error. |
| `src/cli/mod.rs` `handle_next` | After `advance_until_stop` at line 1835, call `run_batch_scheduler` with `&backend, &compiled, &result.final_state, &name, &current_events`. Thread the outcome into the response mapping at line 1890. |
| `src/cli/next_types.rs` | Add an optional `scheduler: Option<SchedulerSummary>` field to the `NextResponse::GateBlocked` and `NextResponse::EvidenceRequired` variants (and optionally others). |
| `src/cli/mod.rs` `handle_next` reserved-key check | Add `"batch"` alongside `"gates"` as a reserved evidence key, if we decide to reserve it. Not strictly needed if the batch definition flows in under a user-declared field. |
| `src/engine/advance.rs` | No changes. The advance loop stays pure. |
| `src/engine/persistence.rs` | No changes to write paths. The new scheduler only reads. |
| `src/gate.rs` | No changes. `children-complete` works unmodified. |
| `src/session/mod.rs`, `local.rs`, `cloud.rs` | No changes. `backend.list()` + `parent_workflow` filter is already the exact discovery mechanism we need. |

**Net**: one new module, one refactor of `handle_init`, one new field in
`TemplateState`, five new compiler validation rules, and one new call-site
in `handle_next`. No changes to the advance loop, persistence, session
backend, or gate evaluator.

---

## Appendix: Key line-number anchors

These are the exact locations referenced above, for fast lookup during
implementation:

- `StateFileHeader` — `src/engine/types.rs:9–25`
- `Gate` struct — `src/template/types.rs:87–124`
- `TemplateState` — `src/template/types.rs:47–63`
- `CompiledTemplate::validate` — `src/template/types.rs:309–637`
- `advance_until_stop` — `src/engine/advance.rs:166–517`
- `resolve_transition` — `src/engine/advance.rs:564–617`
- `merge_epoch_evidence` — `src/engine/advance.rs:623–633`
- `append_header` — `src/engine/persistence.rs:12–36`
- `append_event` — `src/engine/persistence.rs:43–81`
- `read_events` — `src/engine/persistence.rs:148–220`
- `derive_state_from_log` — `src/engine/persistence.rs:222`
- `derive_evidence` — `src/engine/persistence.rs:236`
- `derive_machine_state` — `src/engine/persistence.rs:395`
- `handle_init` — `src/cli/mod.rs:1029–1160`
- `handle_next` entry — `src/cli/mod.rs:1259`
- `advance_until_stop` call site — `src/cli/mod.rs:1835`
- Response mapping switch — `src/cli/mod.rs:1890`
- `handle_status` — `src/cli/mod.rs:2373`
- `evaluate_children_complete` — `src/cli/mod.rs:2471–2586`
- `query_children` — `src/cli/mod.rs:2588–2609`
- `evaluate_gates` dispatch — `src/gate.rs:62–106`
- `SessionBackend` trait — `src/session/mod.rs:44–72`
