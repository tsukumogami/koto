# Decision 5: Failed/Skipped Representation, Gate Output, and Retry Mechanics

**Prefix:** `design_batch-child-spawning_decision_5`
**Complexity:** critical
**Mode:** --auto (confirmed=false, assumptions recorded)
**Scope:** koto (public, tactical)

## Question

How are failed and skipped children represented on disk, what does the
`children-complete` gate output expose, and what evidence action does the parent
submit to retry a failed chain?

This is the single most interconnected decision in the batch-child-spawning
design. It touches: terminal-state semantics, the state file/event log format,
the `children-complete` gate schema, scheduler logic, retry/resume semantics,
and backward compatibility with non-batch workflows. Four sub-questions must be
answered together because each depends on the others:

1. What does "failed" *mean* in koto today, and what mechanism makes terminal
   success vs terminal failure a first-class distinction?
2. How is a skipped child (dependency-blocked) represented on disk — synthetic
   state file, parent-side record, or marker file?
3. What is the full output schema of `children-complete` once it has to report
   success, failure, and skip counts?
4. What evidence shape does the parent submit to retry a failed chain, and how
   does that interact with the state machine (epochs, transitions, re-spawning)?

## Context and Constraints

### Fixed by prior decisions

- **Skip-dependents is the default failure policy**, per-batch overridable
  (exploration decision, `explore_batch-child-spawning_decisions.md`).
- **`children-complete` evaluator is unchanged at its core**; output schema can
  extend (back-compat-preserving additions only).
- **Scheduler runs at the CLI layer inside `handle_next`**, after the advance
  loop (decision 1 of this design).
- **Storage strategy (c) is fixed**: the batch is fully derived from on-disk
  child state files plus the parent's event log. No sidecar files, no new
  header fields on the parent. Spawn records *are* child state files on disk
  (decision 2).
- **No new event types** if avoidable. The storage decision preferred reusing
  `EvidenceSubmitted` and existing events.
- **Reading A (flat declarative batch) is the chosen batch-evidence shape**
  (decision 3). The task graph is a declared DAG with `waits_on` edges.
- Per-batch override means the failure policy is carried in the batch
  evidence, not on the template.

### Grounding

- `src/gate.rs` — `GateOutcome` enum (`Passed | Failed | TimedOut | Error`),
  `StructuredGateResult { outcome, output: serde_json::Value }`.
- `src/cli/mod.rs:2471 evaluate_children_complete` — current implementation.
  Walks `backend.list()` filtered by `parent_workflow == workflow_name`, reads
  each child's events, derives machine state, loads the child's compiled
  template, and checks `template.states[current_state].terminal`. Emits
  `{ total, completed, pending, all_complete, children: [{name, state, complete}] }`.
- `src/engine/types.rs:47 TemplateState` — has `terminal: bool`, no
  distinction between success and failure. `StateFileHeader` has
  `schema_version, workflow, template_hash, created_at, parent_workflow`.
- `src/engine/types.rs EventPayload` — 11 variants: `WorkflowInitialized`,
  `Transitioned`, `EvidenceSubmitted`, `IntegrationInvoked`,
  `DirectedTransition`, `Rewound`, `WorkflowCancelled`,
  `DefaultActionExecuted`, `DecisionRecorded`, `GateEvaluated`,
  `GateOverrideRecorded`. No `WorkflowSkipped` variant.
- `src/engine/advance.rs:476` — comment "Fresh epoch: auto-advanced states
  have no evidence". `Rewound` events create an epoch boundary; evidence,
  decisions, and overrides from before the rewind are not visible to the
  current epoch (`derive_evidence_rewind_clears_prior_evidence` at
  `persistence.rs:750`).
- `wip/research/explore_batch-child-spawning_r1_lead-failure-routing.md` —
  fully specified skip-dependents semantics, including the open alternatives
  for skipped-child representation.
- `wip/research/explore_batch-child-spawning_r1_lead-koto-integration.md`
  sections 2, 3, 5 — fixes storage strategy (c), naming rule
  `<parent>.<task>`, and the resume derivation path.

## Required Deliverables

Five concrete answers, in the order the report addresses them:

1. Define terminal-success vs terminal-failure.
2. Pick the skipped-child representation.
3. Specify the `children-complete` gate output schema with example JSON.
4. Specify the `retry_failed` evidence action.
5. Walk through the resume case: 10-task batch, mid-failure, parent crash.

---

## 1. Terminal Success vs Terminal Failure

### Problem

Koto's current `TemplateState.terminal: bool` is a single boolean. The
evaluator at `src/cli/mod.rs:2545-2554` asks one question: "is the current
state terminal?". A `done` state and a `done_blocked` state both carry
`terminal: true`. The gate treats them identically, reporting `complete: true`
with no quality distinction.

That convention-based naming is adequate for a human reading the state file,
but the scheduler cannot safely dispatch dependents based on a state *name*.
The scheduler needs a protocol-level bit that says "this terminal state
counts as failure; do not propagate to dependents". Without that bit,
`skip-dependents` cannot work deterministically — you'd be coupling scheduler
behavior to a template naming convention.

Three mechanisms were considered.

### Option A: New `failure_mode` field on `TemplateState`

Add an optional field to the template state declaration:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    pub directive: String,
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub failure: bool,     // NEW: only meaningful when terminal=true
    // ...
}
```

Terminology: the field is `failure` (not `failure_mode`) for brevity. Templates
declare:

```yaml
states:
  done:
    terminal: true
    # failure: false (default)
  done_blocked:
    terminal: true
    failure: true
```

**Pros:**
- First-class in the compiled template. Scheduler and gate evaluator both read
  it directly with `tmpl.states[state].failure`.
- Template authors explicitly declare intent per state. A state can be
  terminal-success, terminal-failure, or (by convention) terminal-neutral.
- Orthogonal to existing fields. No conflict with `directive`, `transitions`,
  `gates`, `accepts`.
- Defaults to `false`, so all existing templates upgrade silently to
  "terminal = terminal-success", matching today's de-facto behavior.
- Composes with decision-5's gate output: `failure` propagates from the child
  template into the per-child entry of the gate output.

**Cons:**
- Adds a compiled-template field. Existing templates round-trip fine (serde
  default), but forward-compat bumps a mental version.
- Templates must be edited to mark failure states. The `done_blocked`
  convention still works, but it needs `failure: true` declared on every
  blocked terminal state to participate in skip-dependents.
- Two terminal bits is a mild Cartesian product. `terminal=false, failure=true`
  is nonsensical; needs a compile-time validation check.

### Option B: Reserved state name convention

Terminal states whose names begin with `done_blocked` or end with `_failed`
are treated as failure states by protocol. No template field.

**Pros:**
- Zero schema changes.
- Matches existing convention in koto templates.

**Cons:**
- Couples protocol to template naming. Renaming a state breaks scheduler
  routing silently.
- No compile-time check. A template author can create a terminal state named
  `failed_by_review` and assume the scheduler will treat it as failure — it
  won't unless the name matches the reserved pattern.
- Poor tooling affordance. Third-party templates, generated templates, or
  templates in other languages cannot easily discover the convention.
- Extension requires string-prefix gymnastics for every new failure variant
  (`skipped_due_to_dep`, `cancelled_by_user`, etc.).

### Option C: Enum replacing the boolean

Replace `terminal: bool` with `terminal: Option<TerminalKind>` where
`TerminalKind = Success | Failure | Skipped`.

**Pros:**
- Most type-safe. No impossible states.
- Supports more than two outcomes natively.

**Cons:**
- Breaks the existing compiled-template schema. Every existing template would
  need a mechanical rewrite, and `format_version` must bump from 1 to 2.
- Serde shape change: `terminal: true` currently serializes as a bare boolean;
  the new enum would be `terminal: "success"` or similar. All pre-bump state
  files and templates would fail to parse.
- The scheduler only cares about two cases (is-failure and is-not-failure);
  three cases adds semantic surface without a concrete consumer right now.

### Decision: Option A — `failure: bool` field on terminal states

**Rationale:**

1. **Back-compat is free.** `#[serde(default, skip_serializing_if = "is_false")]`
   means old templates round-trip unchanged. Old koto binaries reading new
   templates ignore the field (serde does not `deny_unknown_fields` on
   `TemplateState`), degrading to "treat as success" — which is the current
   behavior. Old binaries will *miss* skip-dependents semantics, but that's
   an acceptable silent fallback for a feature they don't support anyway.
2. **No naming convention trap.** Scheduler never inspects state names. A
   template author can name a failure state whatever they want (`done_blocked`,
   `aborted`, `walkaway`), so long as they mark `failure: true`.
3. **Cheaper than Option C.** No `format_version` bump, no state file
   migration, no backward-incompat risk.
4. **Declared where it is needed.** The template already declares
   `terminal: true` to mark a state as terminal; adding one more line to mark
   a subset of those as failure is ergonomic.

### Compile-time validation

Add to `CompiledTemplate::validate`:

- **Rule F1**: `failure: true` requires `terminal: true`. Error: "state
  {name}: failure=true requires terminal=true (a non-terminal state cannot
  be a failure state)".
- **Rule F2**: A failure state should have no outbound `transitions`. Error
  (same existing check as for terminal states, reused verbatim).
- **Warning F3**: If a template uses `children-complete` gates and has
  skip-dependents-eligible states but declares zero failure states, emit a
  warning: "template declares no failure states; `skip-dependents` policy
  will never trigger (all terminal children will count as success)".

### Propagation into `StateFileHeader`

No change. The header does not embed terminal semantics. The gate evaluator
re-reads the child's compiled template to check
`tmpl.states[child_current_state].failure` — same lookup path as the current
`terminal` check at `src/cli/mod.rs:2550`.

### Caveat — the "skipped" terminal state

Skipped children reach a terminal state named (conventionally) `skipped`.
That state is declared `terminal: true`, but is it `failure: true`?

**Answer: no.** A skipped child is neither a success nor a failure. It is a
non-outcome. The gate output (section 3) distinguishes skipped from both
success and failure via the `skipped_because` field. The scheduler treats
`skipped` specially: "was this child skipped?" is a different question from
"did this child fail?". Skipped children never trigger re-propagation of
skipping; only failure does.

To encode this cleanly, we introduce **one more terminal marker** alongside
`failure: bool`: a reserved state-name bit exposed via a new template field
`skipped_marker: bool`. See section 2.

---

## 2. Skipped-Child Representation on Disk

The three alternatives outlined in the failure-routing research:

- **(a) Synthetic state file**: create a real child state file with a header
  plus a single synthetic `Transitioned` event to a `skipped` terminal state.
- **(b) Parent-side record only**: no child state file; the parent's event log
  records which tasks were skipped.
- **(c) Marker file**: a stub child directory with a minimal header-flag file
  but no full state log.

### Option (a): Synthetic state file

The scheduler, upon detecting that a queued task's dep is in a failure state,
calls `init_workflow` for that task exactly as it would for a normal task.
Immediately after the header and `WorkflowInitialized` event land, the
scheduler appends a `Transitioned` event to a `skipped` terminal state. The
child's event log is two events long:

```jsonl
{"schema_version":1,"workflow":"p.t6","template_hash":"…","created_at":"…","parent_workflow":"p"}
{"seq":1,"timestamp":"…","type":"workflow_initialized","payload":{"template_path":"tpl.json","variables":{}}}
{"seq":2,"timestamp":"…","type":"transitioned","payload":{"from":"start","to":"skipped","condition_type":"auto"}}
```

An additional `EvidenceSubmitted` event records the skip reason:

```jsonl
{"seq":3,"timestamp":"…","type":"evidence_submitted","payload":{"state":"skipped","fields":{"skipped_because":"p.t3"}}}
```

Or, cleaner: the skip reason lives in a dedicated context key (via the
existing context store) that the gate evaluator reads:

```
context set p.t6 skipped_because "p.t3"
```

Here `p.t6` was skipped because `p.t3` (its dep) was in a failure terminal
state.

**Pros:**
- Symmetric with successful children. Every task in the batch has an entry
  in `backend.list()`, so `koto workflows --children p` shows the full
  picture. No second discovery mechanism needed.
- The `children-complete` evaluator already walks `backend.list()`; zero new
  discovery logic. A skipped child is "just another terminal child" with a
  marker.
- Cloud sync covers it for free. Every child is a normal session.
- Observability wins. `koto status p.t6` returns a real state, not a parent
  lookup.
- Derivable-from-disk holds. Resume rebuilds the task graph from on-disk
  state files alone.

**Cons:**
- Per-child I/O cost. For a 100-task batch where 80 end up skipped because
  a root task failed, you'd create 80 tiny state files. Each is ~3 lines of
  JSONL plus a directory — trivial in bytes but 80 syscall-bundles.
- Semantics: the child "transitioned" from a state it never actually entered.
  The event log contains a synthetic transition, which is a slight fiction.
- Requires the skipped child template to have a `skipped` terminal state. If
  the template doesn't declare one, the scheduler cannot synthesize a
  transition (there's no target).

### Option (b): Parent-side record only

The parent's event log grows an entry per skip. Using only existing event
types, this could ride on `EvidenceSubmitted`:

```jsonl
{"seq":17,"type":"evidence_submitted","payload":{"state":"awaiting_children","fields":{"batch_skips":{"p.t6":{"reason":"dep_failed","because":"p.t3"}}}}}
```

Each skip appends a new evidence event. The gate evaluator must read the
parent's event log *and* `backend.list()`, merging them to compute the final
per-child report.

**Pros:**
- Zero child state files for skipped tasks. No I/O overhead.
- No change to the child template requirement (no need to declare a `skipped`
  state).
- Compact. One evidence field on the parent captures every skip.

**Cons:**
- Asymmetric discovery. `children-complete` today walks `backend.list()`;
  under this option the gate evaluator must *also* read the parent's event
  log and union it with the list result. Two sources of truth to merge.
- `koto workflows --children p` misses skipped tasks unless it also parses
  parent evidence. New coupling: the workflows-listing command becomes
  batch-aware.
- Evidence-as-record abuses the evidence semantics. Evidence is meant to be
  state-level input; skip records are scheduler side-effects. The field
  `batch_skips` is a synthetic convention that lives outside the `accepts`
  block.
- Cloud sync is fine for the parent log but the consumer-side gate evaluator
  is more complex.
- Crash recovery is fragile. If the scheduler spawned a real child for t3
  (which failed), then decided to skip t6, the skip event must hit the parent
  log atomically with the decision. A crash between "observe t3 failed" and
  "record skip of t6 in parent log" leaves the parent inconsistent — next
  `koto next` re-observes t3 failed and re-records the skip, which is fine
  only if the record is idempotent (keyed by task name).

### Option (c): Marker file

Create a marker file in the parent's session directory (e.g.,
`koto-<parent>.skips/p.t6`) with a tiny payload like
`{"reason":"dep_failed","because":"p.t3"}`. The gate evaluator walks both
`backend.list()` and this directory.

**Pros:**
- Cheaper than a full state file.
- Isolated from the main event log.

**Cons:**
- Introduces a new storage path not handled by `backend.list()`. Cleanup,
  rewind, and cloud sync all need new hooks. This is the same coupling that
  got strategy (b) ("sidecar file") rejected in decision 2 of this design.
- Inconsistent with "storage strategy (c)" which was chosen to eliminate
  sidecar files.
- Provides no new capability over option (a). Only the I/O cost is lower,
  and not by much.

### Decision: Option (a) — synthetic state file, marked with `skipped_marker`

**Rationale:**

1. **Uniform discovery path.** `children-complete`, `koto workflows --children`,
   and the scheduler all use `backend.list() + parent_workflow filter`. No
   merged sources of truth. No new coupling in CLI commands.
2. **Storage strategy (c) compatible.** Derivation from on-disk state is the
   prior decision; option (a) preserves it exactly. Every batch task has
   exactly one place to look: its own state file.
3. **Cloud sync is trivial.** Cloud backend already handles arbitrary child
   sessions; skip records look the same.
4. **Crash-safe.** The "record the skip" step is the same operation as "init
   a child": directory created, header appended, first events appended. Idem-
   potency is the existing `backend.exists` check on re-entry, which also
   protects double-skips.
5. **I/O cost is acceptable.** For realistic batch sizes (tens of tasks), the
   extra file creates are negligible. For pathologically large fanouts
   (thousands of tasks, all skipped), the constraint already governs the
   scheduler's behavior regardless of representation.
6. **Synthetic transition is honest in the log.** The `condition_type: "auto"`
   marker (used today by advance-loop auto-advances) already encodes
   "scheduler-initiated transition". Skips reuse this pattern.

#### `skipped_marker` field

A terminal state can declare itself as "this is what a skipped task looks
like". The scheduler transitions into it when it wants to record a skip
without running any child logic:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub failure: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped_marker: bool,  // NEW
    // ...
}
```

A child template participating in a batch must declare exactly one state with
`skipped_marker: true`. Compile-time validation (rule F4): at most one
skipped-marker state per template; batch-eligible templates must declare one
if the parent uses skip-dependents.

Convention: the state is typically named `skipped`, but the name is not
protocol-load-bearing. The scheduler locates it by walking
`tmpl.states.values().filter(|s| s.skipped_marker).next()`.

#### Reason tracking: context keys, not event fields

The skip reason (which dep failed) lives in the child's context store, not in
its event log. After synthesizing the transition to the skipped state, the
scheduler writes one context key:

```
context set p.t6 skipped_because p.t3
```

The gate evaluator reads this key during `evaluate_children_complete`. If the
key is missing (e.g., on older skipped children), the evaluator reports
`skipped_because: null` and carries on.

Using the context store (not an event payload) means:
- No new event variant (the "no new event types if avoidable" constraint
  holds).
- `context get` works normally from the CLI, so users can inspect skip reasons.
- Cleanup path is the same as any other child context.

### Walkthrough: 3-task batch, task-1 fails, task-3 depends on task-1

Batch: `t1, t2 (independent), t3 (waits_on: t1)`. All three share one child
template with `success` and `done_blocked` (failure=true) and `skipped`
(skipped_marker=true) terminal states.

**Start state (before any `koto next p`):**

```
sessions/
├── koto-p.state.jsonl                  # parent with batch evidence
└── (no children yet)
```

**After `koto next p` #1**:

The scheduler sees t1 and t2 are ready (no unsatisfied deps), t3 is blocked on
t1. It spawns both:

```
sessions/
├── koto-p.state.jsonl
├── koto-p.t1.state.jsonl
└── koto-p.t2.state.jsonl
```

**The child runs and t1 fails (reaches `done_blocked`):**

```
koto-p.t1.state.jsonl:
  header: {parent_workflow: "p", ...}
  seq 1: workflow_initialized
  seq 2: transitioned -> review
  ...
  seq N: transitioned -> done_blocked   # failure=true
```

**Next `koto next p`:**

Scheduler walks its task graph:
- t1: on-disk state is `done_blocked`, which is `terminal=true, failure=true`.
  Classified as Failed.
- t2: on-disk state is still `in_progress`. Classified as Running.
- t3: no on-disk state file. Dep t1 is in a failure state. Classified as
  *skip-due-to-failed-dep*. The scheduler init-spawns `p.t3` using the child
  template, immediately synthesizes a transition to the `skipped` state, and
  writes the `skipped_because = p.t1` context key.

```
sessions/
├── koto-p.state.jsonl
├── koto-p.t1.state.jsonl               # terminal failure
├── koto-p.t2.state.jsonl               # still running
└── koto-p.t3.state.jsonl               # terminal skipped
    header: {parent_workflow: "p", ...}
    seq 1: workflow_initialized {template_path, variables}
    seq 2: transitioned {from: "start", to: "skipped", condition_type: "auto"}
```

Plus `context/p.t3/skipped_because = "p.t1"`.

**t2 eventually completes successfully:**

```
sessions/
├── koto-p.t1.state.jsonl               # terminal failure
├── koto-p.t2.state.jsonl               # terminal success
└── koto-p.t3.state.jsonl               # terminal skipped
```

**Gate output at this point:** `total=3, success=1, failed=1, skipped=1,
pending=0, all_complete=true`. The parent's `children-complete` gate passes
(all terminal), and the parent advances to its next state.

### Child template authoring note

Templates that want to participate in batches under skip-dependents must
declare at least:

```yaml
states:
  start: {...}
  ...
  done:
    terminal: true
  done_blocked:
    terminal: true
    failure: true
  skipped:
    terminal: true
    skipped_marker: true
```

Decision 4 (child template selection) covers how the batch evidence declares
which template to use; this decision just specifies what that template must
contain. The compile-time check for batch-eligibility (warning if missing
`skipped` marker state) lives in the compiler validation pass.

---

## 3. The `children-complete` Gate Output Schema

### Today's schema

From `src/cli/mod.rs:2575-2586`:

```json
{
  "total": 3,
  "completed": 2,
  "pending": 1,
  "all_complete": false,
  "children": [
    {"name": "p.t1", "state": "review", "complete": false},
    {"name": "p.t2", "state": "done", "complete": true},
    {"name": "p.t3", "state": "done", "complete": true}
  ],
  "error": ""
}
```

Four top-level aggregate fields, a per-child array, and `error`. The gate
outcome (`Passed` if `all_complete`, `Failed` otherwise) is carried separately
in `StructuredGateResult.outcome`.

### Extended schema

The decision: preserve every existing field, add extended fields, and
populate the per-child entries with new optional attributes.

```json
{
  "total": 10,
  "completed": 7,
  "pending": 3,
  "success": 5,
  "failed": 1,
  "skipped": 1,
  "blocked": 3,
  "all_complete": false,
  "children": [
    {
      "name": "p.t1",
      "state": "done",
      "complete": true,
      "outcome": "success"
    },
    {
      "name": "p.t2",
      "state": "done",
      "complete": true,
      "outcome": "success"
    },
    {
      "name": "p.t3",
      "state": "done_blocked",
      "complete": true,
      "outcome": "failure",
      "failure_mode": "done_blocked"
    },
    {
      "name": "p.t4",
      "state": "done",
      "complete": true,
      "outcome": "success"
    },
    {
      "name": "p.t5",
      "state": "skipped",
      "complete": true,
      "outcome": "skipped",
      "skipped_because": "p.t3"
    },
    {
      "name": "p.t6",
      "state": "skipped",
      "complete": true,
      "outcome": "skipped",
      "skipped_because": "p.t3"
    },
    {
      "name": "p.t7",
      "state": "skipped",
      "complete": true,
      "outcome": "skipped",
      "skipped_because": "p.t3"
    },
    {
      "name": "p.t8",
      "state": "in_progress",
      "complete": false,
      "outcome": "pending"
    },
    {
      "name": "p.t9",
      "state": null,
      "complete": false,
      "outcome": "blocked",
      "blocked_by": ["p.t8"]
    },
    {
      "name": "p.t10",
      "state": null,
      "complete": false,
      "outcome": "blocked",
      "blocked_by": ["p.t8"]
    }
  ],
  "error": ""
}
```

### Field definitions

**Top-level aggregates (all unsigned integers, missing fields default 0 for
back-compat):**

| Field | Definition |
|-------|------------|
| `total` | count of tasks in the batch definition (the denominator) |
| `completed` | count of children in any terminal state (success + failed + skipped) |
| `pending` | total - completed (children that have not reached terminal) |
| `success` | children in terminal state with `failure=false, skipped_marker=false` |
| `failed` | children in terminal state with `failure=true` |
| `skipped` | children in terminal state with `skipped_marker=true` |
| `blocked` | children **not yet spawned** because a dep is non-terminal |
| `all_complete` | `pending == 0` AND no task is `blocked` (nothing left to run) |

Invariant: `success + failed + skipped + blocked + running = total`, where
`running = pending - blocked`. (`running` is not a top-level field today to
keep the schema minimal; the consumer can compute it as
`total - success - failed - skipped - blocked`.)

**Per-child entry fields:**

| Field | Always present? | Definition |
|-------|-----------------|------------|
| `name` | yes | child workflow name (e.g. `p.t3`) |
| `state` | yes (nullable) | child's current state; `null` for not-yet-spawned (`blocked`) tasks |
| `complete` | yes | boolean matching `terminal=true` on the child's current state |
| `outcome` | yes | enum: `success | failure | skipped | pending | blocked` |
| `failure_mode` | only on `outcome=failure` | the state name where the child stopped (for observability) |
| `skipped_because` | only on `outcome=skipped` | the failing dep name (from context key) |
| `blocked_by` | only on `outcome=blocked` | array of dep names that are still non-terminal |

**Outcome derivation table:**

| Child condition | `outcome` |
|-----------------|-----------|
| exists, terminal, not failure, not skipped_marker | `success` |
| exists, terminal, failure=true | `failure` |
| exists, terminal, skipped_marker=true | `skipped` |
| exists, non-terminal | `pending` |
| declared in batch, not spawned, dep not yet terminal | `blocked` |

### Outcome semantics

`all_complete = true` only when `pending == 0 AND blocked == 0`. This is the
key fix: today, `all_complete` is `pending == 0`, but under skip-dependents a
task can be "not pending" and "not blocked" and still "nothing to do" (e.g.
skipped because of a failed dep chain). The new definition correctly
identifies batch termination as "no running tasks AND no blocked tasks that
might still become ready".

The gate outcome mapping:

```
Passed := all_complete
Failed := !all_complete
```

Note: the gate **passes** when everything is terminal, *even if some children
failed or were skipped*. Failure-vs-success of the batch as a whole is a
template-level concern: the parent's post-gate state should read the
`success/failed/skipped` fields from the gate output (via the standard
`gates.<name>.output.*` evidence path) and route on them:

```yaml
states:
  awaiting_children:
    gates:
      batch:
        type: children-complete
        completion: terminal
    transitions:
      - target: analyze_results
        when:
          gates.batch.output.failed: 0
      - target: handle_partial_failure
        when:
          gates.batch.output.failed: ">0"
```

(Comparison operators like `">0"` are not currently supported by the when-
clause engine; this example assumes the existing equality-only routing.
The template can route on `gates.batch.output.all_complete = true` combined
with the `accepts` evidence on a follow-up state that inspects `failed`
explicitly. The surface already allows this via
`gates.batch.output.failed = 0` for the success branch and a default
fallback for the partial-failure branch. The design of when-clause
operator support is out of scope for this decision.)

### Back-compat: old consumers

Existing gate output consumers that read only `total, completed, pending,
all_complete, children[*].{name, state, complete}` continue to work. New
fields are additive; old consumers ignore them. The only behavioral change
is that `all_complete` now requires `blocked == 0`, which under the current
(pre-batch) codebase is always `0` anyway (there's no such thing as a blocked
task without a batch). So existing templates see no observable difference.

### Example: all-success batch (back-compat)

A non-batch, fanned-out workflow that spawns 3 children manually and waits
for all to finish today emits:

```json
{"total": 3, "completed": 3, "pending": 0, "all_complete": true,
 "children": [{"name":"c1","state":"done","complete":true}, ...]}
```

Under the new schema that same evaluator emits:

```json
{"total": 3, "completed": 3, "pending": 0,
 "success": 3, "failed": 0, "skipped": 0, "blocked": 0,
 "all_complete": true,
 "children": [{"name":"c1","state":"done","complete":true,"outcome":"success"}, ...]}
```

Old consumers read `total/completed/pending/all_complete` and are satisfied.
New consumers can additionally route on `success/failed/skipped`.

### Back-compat: the `blocked` concept for non-batch parents

Today, non-batch parents don't have a task graph — every child is either
present or not. `blocked` is always `0` for them. The gate evaluator computes
`blocked` by cross-referencing `backend.list()` (discovered children) with a
declared task list. If no task list exists (the parent has no batch
evidence), `blocked` is `0` trivially. This is where the gate evaluator
must look up the parent's latest batch evidence (if any) to know the
declared task list.

**Implementation note:** the gate evaluator needs to read the parent's event
log (not just `backend.list()`) to fetch the batch definition. Today it
doesn't: it only takes a `workflow_name` and a `Gate`. The extended
signature:

```rust
fn evaluate_children_complete(
    backend: &dyn SessionBackend,
    workflow_name: &str,
    parent_events: &[Event],     // NEW: for batch-definition lookup
    gate: &Gate,
) -> StructuredGateResult
```

The `parent_events` slice is already computed in `handle_next` (via
`backend.read_events(workflow_name)`); it just needs to be passed through.
Non-batch parents have no `batch_definition` field in their evidence, so
the evaluator short-circuits to the existing logic (zero `blocked`, no
per-batch schema fields).

---

## 4. The `retry_failed` Evidence Action

### Problem

After a batch completes with failures and skips, the parent's state is
blocked in a post-analysis state (e.g. `analyze_results`). The user or agent
decides to retry the failed chain. The parent submits evidence that:

1. Identifies which failed children to retry.
2. Tells the scheduler to re-queue them.
3. Cascades: all skipped children whose chain starts at a retried failure
   must also be re-queued.
4. Leaves successful children alone.

### Field name and shape

The evidence action is carried in a reserved evidence field named
`retry_failed`. Shape:

```json
{
  "retry_failed": {
    "children": ["p.t3"]
  }
}
```

Minimal form: just a list of child names. The scheduler walks the declared
batch DAG from each named failure down through its transitive dependents and
re-spawns all of them.

**Alternatives:**

- **All-failures**: `{"retry_failed": "all"}` to retry every failed child in
  the batch. Valid syntactic sugar; the scheduler expands to the full list.
- **Chain mode**: `{"retry_failed": {"children": ["p.t3"], "include_skipped":
  true}}`. `include_skipped: true` (default) re-queues dependents; `false`
  would retry only the named children and leave their skipped dependents in
  place. `false` is rarely useful but cheap to support.

**Field name rationale:**

`retry_failed` is explicit about what it does (retry the failed chain). It
does not conflict with any existing reserved evidence key (`gates` is the
only such key today, per `handle_next_gates_key_returns_invalid_submission`
at `src/cli/mod.rs:2853`).

### State machine interaction — epochs and rewinds

The submission of `retry_failed` evidence happens on the **parent** workflow,
not the children. The parent is in a post-batch state (`analyze_results`).
Accepting the evidence and routing it back into the batch-spawning state is
a regular template-level transition:

```yaml
states:
  analyze_results:
    accepts:
      retry_failed:
        type: object
    transitions:
      - target: awaiting_children     # re-enter the batch state
        when:
          retry_failed: "*"           # any non-empty retry_failed value
  awaiting_children:
    batch: {...}
    gates:
      batch:
        type: children-complete
        completion: terminal
    transitions:
      - target: analyze_results
        when:
          gates.batch.output.all_complete: true
```

Here the template author wires `retry_failed` evidence as the trigger that
moves the parent back to `awaiting_children`. The scheduler runs on every
`koto next` in `awaiting_children`, so on re-entry it does its normal work.

The trick: on re-entry, the scheduler must know which children to **reset**.
The reset is where epochs matter.

#### Resetting a child workflow

Two mechanisms are available:

1. **Rewind**. `koto rewind <child> <state>` moves the child back to an
   earlier state, appending a `Rewound` event. Evidence from before the
   rewind is cleared (epoch boundary). The scheduler calls rewind on each
   child in the retry set, targeting the child's initial state.

2. **Recreate**. Delete the child's state file and re-spawn via `init_workflow`.
   This is simpler but loses historical context.

**Decision: rewind.** The `Rewound` event is already supported by the engine
and carries the epoch-boundary semantics needed. The scheduler, upon entering
`awaiting_children` with a `retry_failed` evidence field, walks the retry set
and calls the equivalent of `koto rewind <child> <child_initial_state>` for
each.

Why rewind, not recreate:

- **No new event type.** Rewind is already an event. Recreate would require
  a new "child reset" event or abuse of `WorkflowCancelled`.
- **History preservation.** The child's event log retains the failed attempt
  plus the rewind marker. `koto query <child>` still shows the full history.
- **Epoch semantics.** On rewind, the child's evidence is cleared for the
  new epoch. The next `koto next <child>` starts fresh.
- **Cloud sync friendly.** Append-only semantics are preserved.

#### The retry sequence, step by step

On `koto next <parent>` with `retry_failed` evidence present in the
parent's `analyze_results` state:

1. Advance loop routes the parent from `analyze_results` to
   `awaiting_children` via the `retry_failed: "*"` transition. Appends a
   `Transitioned` event.
2. Scheduler tick runs in `awaiting_children`:
   1. Reads the latest-epoch `batch_definition` field.
   2. Reads the latest-epoch `retry_failed` field from the parent's evidence.
      This is not cleared by the transition (same epoch — no rewind).
   3. Computes the retry set: every child in `retry_failed.children`, plus
      every transitive dependent in the batch DAG (if
      `include_skipped: true`, which is the default).
   4. For each child in the retry set: call `rewind_child(child_name,
      initial_state)`. This appends a `Rewound` event to the child's log
      and clears its evidence epoch.
   5. Invoke the normal spawn/classify logic. Children that were previously
      `failed` or `skipped` now appear as `running` (they have state files
      but are not in a terminal state after rewind). The scheduler does not
      need to re-init them; they're already present.
3. Return the usual `GateBlocked` response to the user, who observes that the
   batch is now in progress again.

Question: **Does the parent need a fresh epoch?** No. The `retry_failed`
evidence is consumed by the transition and remains valid for the duration of
the new `awaiting_children` visit. The scheduler reads it on each `koto next`
tick until all children clear the retry set and the gate passes. To prevent
the scheduler from infinitely re-resetting children on every tick, the
scheduler must track **whether a retry has already been applied this epoch**.

### Retry-already-applied marker

The scheduler needs to distinguish "this is the first tick after a
`retry_failed` transition" from "subsequent ticks within the same epoch".
Without a marker, every `koto next p` would re-rewind every retry-set child.

**Mechanism: consume the retry_failed evidence on first use.**

The retry action is consumed once. After the scheduler applies the rewinds,
it writes an `EvidenceSubmitted` event that *clears* the `retry_failed`
field:

```jsonl
{"seq":42,"type":"evidence_submitted","payload":{"state":"awaiting_children","fields":{"retry_failed":null}}}
```

On subsequent ticks, `merge_epoch_evidence` sees `retry_failed: null` in the
latest submission and excludes it (last-write-wins within the epoch clears
the value). The scheduler classifies children as running based on their
rewound state and proceeds normally.

Alternative: maintain an in-memory "retry applied" flag within the
`handle_next` call. Simpler, but doesn't survive a crash between rewind and
write-retry-consumption (the next `koto next` would re-apply the rewind,
which is idempotent because rewinding to the initial state twice from the
initial state is a no-op — but the event log would accumulate spurious
rewind events).

**Decision: persist the consumption by writing a null clearing event.** This
is the most defensive choice and matches how the engine handles transient
commands today (each state change is an event).

### `retry_failed` on its own does NOT require a parent rewind

Crucially, the parent workflow does *not* rewind during retry. It moves
forward via a regular transition (`analyze_results -> awaiting_children`),
same as any other state change. The parent's event log grows linearly.

This has important consequences:

- **Successful children are not re-run.** Only children named in
  `retry_failed.children` and their dependents are rewound. Other successful
  children are left as-is; the gate evaluator still sees them as terminal.
- **The batch definition is unchanged.** The same task graph applies to the
  retry. Re-entering the state does not re-read new evidence.
- **Retry-of-retry works.** If after retrying, another failure occurs, the
  user can submit `retry_failed` again and the same machinery runs.

### Composition with decision 2 (storage strategy)

Strategy (c) derives everything from on-disk child state files. Rewind is
the mechanism that moves a child from "failed terminal" back to "running",
which the gate evaluator sees automatically on the next tick — no change
needed to the evaluator.

---

## 5. Resume Story: 10-task Batch, Failure, Crash

### Setup

Parent `p` runs a batch with 10 tasks, declared DAG edges:

```
t1 -> (t3, t4)
t2 -> t5
t4 -> (t6, t7, t8)
t6 -> t9
(t10 has no deps)
```

Visualized:

```
t1 --- t3
  \--- t4 --- t6 --- t9
         \--- t7
         \--- t8
t2 --- t5
t10
```

Current progress (before the crash):

- t1: success, terminal
- t2: success, terminal
- t3: **failed**, terminal
- t4: success, terminal
- t5: running (in `in_progress`)
- t6: running
- t7: skipped (initially? no — t7's only dep is t4 which succeeded, so t7 was
  spawned and is running)
- t8: running
- t9: not yet spawned (blocked on t6 running)
- t10: running

Re-examine with the stated crash setup: "task-3 just failed, tasks 5,6,7 are
skipped due to dep".

For tasks 5, 6, 7 to be skipped because of t3, the DAG must have t5, t6, t7
depending on t3. Let me redo with the user-specified topology:

```
t1 (no dep)
t2 (no dep)
t3 (waits_on: t1)          <-- fails
t4 (waits_on: t2)
t5 (waits_on: t3)          <-- skipped
t6 (waits_on: t3)          <-- skipped
t7 (waits_on: t3)          <-- skipped
t8 (waits_on: t4)
t9 (no dep)
t10 (waits_on: t2)
```

**Progress just before crash:**
- t1: success (terminal)
- t2: success (terminal)
- t3: **failed** (terminal, failure=true, in state `done_blocked`)
- t4: in_progress (running)
- t5, t6, t7: scheduler was about to skip them, but crash happened
- t8: not yet spawned (blocked on t4)
- t9: in_progress (running)
- t10: in_progress (running)

### On-disk state at the moment of the crash

Two scenarios, because "parent crashes" has a specific failure mode:

**Scenario A: crash before scheduler synthesized the skip transitions.**

The scheduler had observed t3 failed and had just finished classifying the
task graph. It began spawning skip children for t5, t6, t7. Say it wrote the
`koto-p.t5.state.jsonl` directory and header, but then the process died
before appending the `Transitioned -> skipped` event. On disk:

```
sessions/
├── koto-p.state.jsonl                  # parent, at state awaiting_children
├── koto-p.t1.state.jsonl               # success terminal
├── koto-p.t2.state.jsonl               # success terminal
├── koto-p.t3.state.jsonl               # failure terminal (done_blocked)
├── koto-p.t4.state.jsonl               # in_progress
├── koto-p.t5.state.jsonl               # header only, no events (!)
├── koto-p.t9.state.jsonl               # in_progress
└── koto-p.t10.state.jsonl              # in_progress
```

t5 has a state file but no events — the half-initialized state described in
the koto-integration research at line 582. Classified as "exists but not
terminal" = running, and `handle_next p.t5` would error with
`PersistenceError` due to the empty-events check at `cli/mod.rs:1337`.

**Mitigation** (per research line 594): the `init_workflow` helper makes the
header + first-event pair atomic by writing to a temporary file and renaming,
or by performing a "repair" pass at startup that deletes header-only state
files. The design must pick one.

**Decision (adjunct to this decision, for completeness): atomic init.**
Merge the header write and the `WorkflowInitialized` event write into a
single fsynced append. The backend abstraction already exposes
`append_header` and `append_event` as separate calls; the
`init_workflow` helper wraps both in a single "write header + event 1"
combined append. This eliminates the half-initialized case entirely and
keeps the derivable-from-disk invariant intact.

**Scenario B: crash after all three skips were synthesized but before the
parent's `koto next` returned.**

```
sessions/
├── koto-p.state.jsonl                  # parent, at awaiting_children
├── koto-p.t1.state.jsonl               # success terminal
├── koto-p.t2.state.jsonl               # success terminal
├── koto-p.t3.state.jsonl               # failure terminal
├── koto-p.t4.state.jsonl               # in_progress
├── koto-p.t5.state.jsonl               # skipped terminal
├── koto-p.t6.state.jsonl               # skipped terminal
├── koto-p.t7.state.jsonl               # skipped terminal
├── koto-p.t9.state.jsonl               # in_progress
└── koto-p.t10.state.jsonl              # in_progress
```

Plus context keys:
```
context/p.t5/skipped_because = "p.t3"
context/p.t6/skipped_because = "p.t3"
context/p.t7/skipped_because = "p.t3"
```

This is the "happy crash" case — every side effect made it to disk.

### `koto next p` after the crash (Scenario B)

1. **`handle_next` boot**. Reads `koto-p.state.jsonl` header and events.
2. **`derive_machine_state`** returns `current_state = awaiting_children`.
3. **Advance loop**. Evaluates the `children-complete` gate.
4. **Gate evaluator** walks `backend.list()`, filters to
   `parent_workflow == "p"`. Finds 9 children (t1-t7, t9, t10; t8 not
   spawned). Classifies each:

   ```
   t1 -> success
   t2 -> success
   t3 -> failure (state=done_blocked)
   t4 -> pending (in_progress)
   t5 -> skipped (state=skipped, context skipped_because=p.t3)
   t6 -> skipped
   t7 -> skipped
   t9 -> pending
   t10 -> pending
   ```

   Plus cross-references the parent's batch definition to find unspawned
   tasks:
   ```
   t8 -> not spawned, dep t4 is pending -> blocked
   ```

   Emits:
   ```json
   {
     "total": 10,
     "completed": 6,    // t1,t2,t3,t5,t6,t7
     "pending": 4,      // t4,t8,t9,t10
     "success": 2,      // t1,t2
     "failed": 1,       // t3
     "skipped": 3,      // t5,t6,t7
     "blocked": 1,      // t8
     "all_complete": false,
     "children": [
       {"name":"p.t1","state":"done","complete":true,"outcome":"success"},
       {"name":"p.t2","state":"done","complete":true,"outcome":"success"},
       {"name":"p.t3","state":"done_blocked","complete":true,"outcome":"failure","failure_mode":"done_blocked"},
       {"name":"p.t4","state":"in_progress","complete":false,"outcome":"pending"},
       {"name":"p.t5","state":"skipped","complete":true,"outcome":"skipped","skipped_because":"p.t3"},
       {"name":"p.t6","state":"skipped","complete":true,"outcome":"skipped","skipped_because":"p.t3"},
       {"name":"p.t7","state":"skipped","complete":true,"outcome":"skipped","skipped_because":"p.t3"},
       {"name":"p.t8","state":null,"complete":false,"outcome":"blocked","blocked_by":["p.t4"]},
       {"name":"p.t9","state":"in_progress","complete":false,"outcome":"pending"},
       {"name":"p.t10","state":"in_progress","complete":false,"outcome":"pending"}
     ]
   }
   ```

   Gate outcome: `Failed` (not `all_complete`).

5. **Scheduler tick**. The scheduler re-walks the task graph with the same
   classifier. Nothing to do: t5, t6, t7 are already terminal skipped; t4
   is still running; t8 is still blocked; t9 and t10 are still running.
   `SchedulerOutcome::Scheduled { spawned: [], already: [t1..t7,t9,t10],
   blocked: [t8] }`. The scheduler is fully idempotent on resume — every
   action it would take is gated by `backend.exists` (for spawn) or by the
   child's current terminal state (for skip).

6. **Response**. `GateBlocked` with the extended output above. The parent
   is still at `awaiting_children`, waiting for t4, t9, t10 to complete.

**Nothing was re-done, nothing was lost.** The on-disk state files *are*
the scheduler's memory, so the crash is transparent.

### `koto next p` after the crash (Scenario A — half-initialized t5)

Same as Scenario B, except t5 is observed as "exists, non-terminal, events
empty". The gate evaluator:

1. Calls `backend.read_events("p.t5")`. Returns empty events.
2. `derive_machine_state` returns `None` (no state file events).
3. The evaluator classifies t5 as outcome=`pending` with `state=null`, not
   terminal — because the child is not recognizably in any state.

This is a degraded classification. The batch cannot progress past t5 until
the half-initialized state is resolved.

**Recovery:** at startup, before running the advance loop, `handle_next`
calls a new `repair_half_initialized_children(parent)` helper that:
- Walks `backend.list()` filtered to the parent's children.
- For each child, reads the header and the first event.
- If the header exists but no events follow, deletes the state file.

Next time the scheduler tick runs, t5 no longer has a state file, so the
scheduler classifies t5 as "not spawned" and re-evaluates its deps. t5's
dep t3 is in a failure state, so the scheduler spawns+skips t5 (atomic
init, this time).

The repair step is strictly additive and only runs when the parent has a
batch (non-batch parents are unaffected).

### Scenario C: crash mid-rewind during `retry_failed`

This is the nastiest corner case. Consider:

1. User submits `retry_failed: {children: [p.t3]}` evidence.
2. Parent advances from `analyze_results` to `awaiting_children`.
3. Scheduler begins rewinding: writes `Rewound` to `p.t3`'s log. Successful.
4. Scheduler writes `Rewound` to `p.t5`'s log. Successful.
5. Crash before writing `Rewound` to `p.t6`'s log.

On disk:
- `p.t3` rewound to initial state (running, empty evidence)
- `p.t5` rewound to initial state (running, empty evidence)
- `p.t6` still in its `skipped` terminal state (not yet rewound)
- `p.t7` still in its `skipped` terminal state (not yet rewound)
- parent has not yet written the "retry consumed" event (`retry_failed: null`)

Next `koto next p`:
1. Advance loop: parent is in `awaiting_children`. No transition applies.
2. Gate eval: t3 is pending (not terminal anymore, it was rewound). t5 is
   pending. t6 and t7 are still skipped terminal.
3. Scheduler tick: reads `retry_failed: {children: [p.t3]}` evidence (still
   present because the consume-event wasn't written). Walks retry set: t3 +
   dependents (t5, t6, t7). Computes which are still in skipped/failed
   states and rewinds only those. t3 and t5 are already running (rewound) —
   skipped. t6 and t7 are still skipped — rewind them now.
4. Writes the consume event (`retry_failed: null`).

**Key property:** rewinding an already-rewound child is idempotent in
practice because the scheduler only rewinds children whose *current state*
is `failure` or `skipped_marker`. A child currently in its initial state
after a prior rewind is classified as "pending" and left alone. The retry
loop converges regardless of where the crash happened.

To make this precise: the scheduler's retry logic at each tick is:

```python
retry_set = transitive_closure(retry_failed.children, batch.dag)
for child in retry_set:
    child_state = classify(child)
    if child_state in {Failure, Skipped}:
        rewind(child, initial_state)
# After all rewinds succeed, consume the retry evidence:
append_evidence({"retry_failed": null})
```

The loop is safe to re-run after a crash because classification is re-derived
from disk on each invocation.

### Scenario D: parent itself crashes mid-transition

The parent writes a `Transitioned` event moving from `analyze_results` to
`awaiting_children`. Crash before the scheduler tick runs.

Resume:
1. `handle_next` boots. `derive_machine_state` reads the transitioned event.
   Current state: `awaiting_children`.
2. Advance loop runs. The `awaiting_children` state has a `children-complete`
   gate. Gate eval runs; classification is based on whatever was on disk at
   crash time (retry not yet applied, because the scheduler never ran).
3. Gate returns `Failed` (there are still failed children). Outcome:
   `GateBlocked`.
4. Scheduler tick runs. Reads `retry_failed` from evidence — still present.
   Runs the retry loop.

No loss. Parent transitions are atomic because they're single event appends
in the JSONL log.

---

## Rejected Alternatives Summary

### Decision 5.1 (terminal-success vs terminal-failure)

| Alternative | Why rejected |
|-------------|--------------|
| Reserved state name convention | Couples protocol to naming; no compile-time check |
| Enum replacing `terminal: bool` | Breaking change to template/state-file schema, format_version bump |

### Decision 5.2 (skipped-child representation)

| Alternative | Why rejected |
|-------------|--------------|
| Parent-side record only | Two sources of truth for `children-complete`; evidence semantic abuse; `koto workflows --children` gap |
| Marker file | Reintroduces sidecar storage that storage strategy (c) explicitly rejected; no capability win over synthetic state file |

### Decision 5.3 (gate output schema)

No rejected alternatives in the traditional sense; the schema is a
back-compat-preserving extension. One variant considered:
flattening `children[*].outcome` back to `state` alone was rejected because
it conflates "current state name" with "protocol-level outcome" — the two
must be distinguishable to allow state-naming flexibility.

### Decision 5.4 (retry evidence action)

| Alternative | Why rejected |
|-------------|--------------|
| `retry_batch` (whole batch) | Too coarse; re-runs successful children unnecessarily |
| Recreate children (delete + re-init) | No new event for delete; loses history; appends-only guarantee broken |
| Parent workflow rewind | Parent's post-analysis state contains the user decision; rewinding would clear it before it can be acted on |

---

## Consequences

### What becomes easier

- **Declarative retry**. The user submits one evidence field and the whole
  failed chain re-runs automatically, including skipped dependents.
- **Observability**. Every child task in a batch has a state file, so
  `koto workflows --children`, `koto status`, and `koto query` work
  uniformly.
- **Resume**. The scheduler is stateless across invocations. Every decision
  is re-derived from disk.
- **Non-batch templates are unaffected**. The `failure` field defaults to
  false; the `skipped_marker` field defaults to false; the gate schema
  extends additively; non-batch parents see `blocked=0` trivially.

### What becomes harder

- **Child templates must declare terminal semantics.** Authors have to add
  `failure: true` to their failure states and declare a `skipped` state
  with `skipped_marker: true` if they want to participate in batches.
- **The gate evaluator must read parent events.** Signature change from
  `(backend, workflow_name, gate)` to `(backend, workflow_name, parent_events,
  gate)`. All call sites updated; the parent-events slice is already
  available inside `handle_next`.
- **Half-initialized child state files need a repair pass.** Atomic init
  is the primary defense; a repair sweep on `handle_next` entry is the
  safety net.
- **Retry consumption event is a new idiom.** The scheduler must write a
  `{"retry_failed": null}` evidence event after applying the retry, which
  is a new pattern (evidence-self-clearing is not currently used anywhere).
  Not a breaking change, just a new convention.

### Implementation touch points

- `src/template/types.rs`: add `failure: bool` and `skipped_marker: bool`
  to `TemplateState`.
- `src/template/compile.rs` (or `types.rs` validation path): add rules F1,
  F2, F3, F4.
- `src/engine/types.rs`: no changes (no new events).
- `src/cli/mod.rs:2471 evaluate_children_complete`: extend to read parent
  events, compute the batch definition, classify children by outcome,
  emit the extended schema.
- `src/cli/mod.rs:1029 handle_init` / new `init_workflow` helper: atomic
  header + first event append.
- `src/cli/mod.rs handle_next`: add `repair_half_initialized_children`
  pre-pass for batch parents.
- New `src/engine/batch.rs`: the `run_batch_scheduler` function (from
  decision 1) consumes the retry logic here.
- `src/engine/advance.rs`: no changes.
- `src/session/context.rs`: used to store `skipped_because` — no API
  changes needed.

### Documentation touch points (both koto-author and koto-user skills)

- **koto-author**: add a page on batch-eligible templates, covering the
  `failure` and `skipped_marker` fields and the required state shapes.
- **koto-user**: add a page on inspecting a batch's gate output, reading
  `success/failed/skipped/blocked` aggregates, and submitting `retry_failed`
  evidence.

---

## Assumptions

Because this decision was reached in `--auto` mode without user confirmation,
the following assumptions are recorded:

- **A1.** Template authors are willing to add a `failure: true` field to
  terminal failure states. This is a one-line change per state and preserves
  the existing convention of `done_blocked` naming.
- **A2.** Templates used in batches will declare exactly one `skipped_marker`
  state. A compile-time warning (not an error) surfaces missing markers; a
  template that lacks one but is used in a batch will fail at scheduler time
  with a clear error.
- **A3.** The context store is available to the scheduler during child
  initialization. This is already true in current `handle_next` paths but
  has not been verified for the new atomic-init helper.
- **A4.** Rewinding a child to its initial state is safe at any point in the
  child's lifecycle, including before any user action on the child. The
  existing `handle_rewind` implementation does not special-case initial-state
  rewinds; this assumption holds if the engine can successfully re-enter the
  child's initial state from any non-initial state.
- **A5.** Writing a `retry_failed: null` clearing event is interpreted by
  `merge_epoch_evidence` as a last-write-wins null, which effectively removes
  the field from the merged evidence map. The current implementation treats
  evidence fields as values in a `HashMap<String, serde_json::Value>`; a
  null value is distinct from absence. A small implementation detail: the
  clearing event writes an empty object instead of null, so the field is
  effectively re-declared as empty and the scheduler's check becomes
  "is `retry_failed` an empty/missing object?".
- **A6.** The `blocked` count in the gate output does not need to be
  accurate for non-batch parents. Non-batch parents have no declared task
  graph, so `blocked = 0` trivially. The assumption is that this is the
  only behavior change observable to pre-batch gate consumers, and it is
  a no-op for them.

These assumptions should be verified during implementation of the batch
feature. None are expected to surface as blockers.

---

## Confidence

**High.** The constraints fixed by prior decisions (storage strategy (c),
skip-dependents default, no new event types, scheduler at CLI layer) narrow
the alternative space to a single coherent design. The remaining free
parameters are:

- terminal-failure encoding (Option A wins on back-compat)
- skipped representation (Option (a) wins on unified discovery)
- gate schema extensions (additive, no hidden tradeoffs)
- retry shape (rewind-based, clearing event)

Each sub-decision has a clear winner once the others are fixed. The hardest
part — whether to pay the I/O cost of synthetic skip files — was settled by
the derivability-from-disk invariant in the already-settled storage
decision. Anything else would reintroduce coupling that the earlier decision
explicitly rejected.

Implementation risks (atomic init, half-initialized repair, retry
idempotency) are mitigations for pre-existing concerns in the `handle_init`
code path, not new risks introduced by this decision.

---

## YAML Summary

```yaml
decision_result:
  status: COMPLETE
  chosen: >
    Failure-as-field on terminal states, synthetic state files for skipped
    children, extended children-complete gate output schema, and
    retry_failed evidence that triggers rewind-based re-queue of the
    failed chain.
  confidence: high
  rationale: >
    Prior decisions (storage strategy (c), skip-dependents default, no new
    event types) narrow the space to a single coherent design. Synthetic
    state files preserve unified discovery through backend.list(); a new
    failure: bool field is the minimal-breakage terminal-semantics
    addition; retry_failed evidence with rewind-based reset composes with
    the existing epoch model.
  sub_decisions:
    terminal_failure_mechanism:
      chosen: new failure bool field on TemplateState
      rejected:
        - name: reserved state name convention
          reason: couples protocol to naming, no compile-time check
        - name: enum replacing terminal bool
          reason: breaking schema change, format_version bump
    skipped_child_representation:
      chosen: synthetic state file with skipped_marker terminal state
      rejected:
        - name: parent-side record only
          reason: two sources of truth, evidence semantic abuse
        - name: marker file
          reason: reintroduces sidecar storage rejected by storage strategy (c)
    gate_output_schema:
      chosen: extended with success/failed/skipped/blocked aggregates and per-child outcome enum
      back_compat: additive, old consumers read total/completed/pending/all_complete unchanged
    retry_evidence_action:
      chosen: retry_failed field with children list, consumed on first scheduler tick via null-clearing event
      mechanism: rewind each child in the retry set (plus transitive dependents) to its initial state, append clearing event to parent
      rejected:
        - name: retry_batch (whole batch)
          reason: re-runs successful children unnecessarily
        - name: recreate children via delete
          reason: breaks append-only guarantee and loses history
        - name: parent rewind
          reason: would clear the very evidence being acted on
  assumptions:
    - Template authors will add failure true to their terminal failure states
    - Batch-eligible child templates declare a skipped_marker state
    - Context store is available during atomic child init
    - Rewinding to initial state is safe at any point in the child lifecycle
    - retry_failed clearing event effectively removes the field from merged evidence
    - blocked count stays zero for non-batch parents (no behavior change)
  constraints_honored:
    - skip-dependents default with per-batch override
    - children-complete evaluator core unchanged, output extended
    - scheduler at CLI layer in handle_next
    - storage strategy (c) fully-derived-from-disk
    - no new event types
    - composes with Reading A flat declarative batch
  report_file: wip/design_batch-child-spawning_decision_5_report.md
```
