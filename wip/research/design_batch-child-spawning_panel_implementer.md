# Implementer Review: DESIGN-batch-child-spawning

Reviewer perspective: Rust developer who will implement this in `src/`.
Question: "Can I start coding tomorrow from this doc, or will I hit
ambiguities that send me back to the designer?"

## 1. Ambiguities That Would Block Me

### 1.1 `SessionError` type does not exist

The design's Key Interfaces section specifies `init_state_file` returns
`Result<(), SessionError>` and `BatchError` variants carry `source:
SessionError`. The actual `SessionBackend` trait at `src/session/mod.rs`
returns `anyhow::Result<T>` for every method. There is no `SessionError`
type anywhere in the codebase. I have three options: (a) keep using
`anyhow::Result` for `init_state_file` to match the existing trait, (b)
introduce a new `SessionError` enum and migrate all trait methods, (c)
introduce it only for `init_state_file`. Option (a) is simplest and
preserves consistency; the `BatchError` variants would wrap
`anyhow::Error` instead. But the design's error types reference
`SessionError` in multiple places, so the intent might be to introduce
it. Needs clarification before I write the trait extension.

### 1.2 `retry_failed` evidence flow through the advance loop is underspecified

Decision 5.4 says `retry_failed` is a "reserved top-level evidence key"
treated like `gates`, but the mechanics are unclear:

- Is `retry_failed` validated by the advance loop's evidence validation
  (`validate_evidence` at `src/engine/evidence.rs`)? The `tasks` type
  validation knows about `tasks`; does it also need to know about
  `retry_failed`?
- The design says "the advance loop sees the retry_failed evidence and
  transitions the parent back to awaiting_children via a template-
  defined route." This implies a template transition with
  `when: { retry_failed: ... }`, but the design never shows a template
  with this routing. Is `retry_failed` evidence-routed or is it
  intercepted before the advance loop?
- The design says the scheduler "detects the unconsumed retry_failed
  evidence" and runs `handle_retry_failed`. But the scheduler runs
  AFTER `advance_until_stop`. If the advance loop already consumed the
  evidence and transitioned the parent, how does the scheduler detect
  it's "unconsumed"? The null-clearing event presumably happens after
  the scheduler processes it, but the ordering between "advance loop
  processes evidence" and "scheduler processes retry_failed" needs a
  concrete code-level description.

### 1.3 Skipped-marker child template and state

Decision 5.2 says skipped children are init-spawned with a
`Transitioned -> skipped_marker_state` event, and shows a YAML state
`skipped_due_to_dep_failure` with `skipped_marker: true`. But:

- Which template do skipped children use? The design says "header
  pointing at the parent template" -- does that mean the parent's
  compiled template JSON? That would mean the skipped child's
  `WorkflowInitialized.template_path` points to the parent's cached
  template, but the parent template doesn't declare
  `skipped_due_to_dep_failure` as a state.
- Or does each child template need to declare a skipped-marker state?
  That would be a authoring burden the design doesn't mention.
- Or does the scheduler synthesize a minimal one-state template for
  skipped children? The design doesn't specify this.

This is the single most confusing point in the design. I can't write
`init_state_file` for skipped children without knowing what template
they point at and whether it needs to be compiled and cached.

### 1.4 `EvidenceSubmitted` payload shape change

Decision 4 adds `submitter_cwd: Option<String>` to
`EventPayload::EvidenceSubmitted`. Currently this variant has:

```rust
EvidenceSubmitted {
    state: String,
    fields: HashMap<String, serde_json::Value>,
}
```

Adding `submitter_cwd` is straightforward, but `EventPayload` uses
`#[serde(untagged)]` with a manual Deserialize impl that dispatches on
`event_type`. The helper struct `EvidenceSubmittedPayload` at line 310
must also gain the field. The design doesn't call this out, but it's
mechanical -- not a blocker, just a landmine for a PR reviewer.

### 1.5 `handle_next` exit point vs scheduler integration

`handle_next` currently calls `std::process::exit(0)` at line 2050
after printing the response. The scheduler needs to run between
`advance_until_stop` and the response serialization. The design says
"called from handle_next immediately after advance_until_stop returns"
but the current code structure is a single giant `match result {}`
block that exits the process. I'd need to:

- Run `run_batch_scheduler` inside the `Ok(advance_result)` arm, before
  constructing the `NextResponse`.
- Attach the `SchedulerOutcome` to the response. But `NextResponse` is
  an enum with no `scheduler` field on any variant.
- Either add a `scheduler: Option<SchedulerOutcome>` to every variant,
  or wrap `NextResponse` in a container struct.

The design says "the scheduler result is an additive field for
observability" but doesn't specify which `NextResponse` variants carry
it. From the walkthrough, it appears on `GateBlocked` and
`EvidenceRequired` responses. Does it also appear on `Terminal`? The
walkthrough's final interaction (all children complete, parent
transitions to summarize) returns `workflow_complete` with no scheduler
field, but that's because `run_batch_scheduler` returned `NoBatch`. What
if the parent transitions through a batch state to a terminal state in a
single advance? Does the scheduler still run?

## 2. Type Definitions

### `MaterializeChildrenSpec`

Well-specified. `from_field: String`, `default_template: String`,
`failure_policy: FailurePolicy`. The `deny_unknown_fields` attribute
gives forward-compat. The `default_failure_policy` serde default
function needs to be written (returns `FailurePolicy::SkipDependents`).
Clear enough to type.

### `FailurePolicy`

Two variants, `snake_case` serde rename. Sufficient.

### `BatchError`

The `SpawnFailed` and `BackendError` variants reference `SessionError`
which doesn't exist (see 1.1). `LimitExceeded` uses `&'static str` for
`which` -- that's fine for known limit names. The design lists three
limits (1000 tasks, 10 waits_on, depth 50) but `which` is a string, so
extensible.

### `SchedulerOutcome`

`Scheduled.skipped` is `Vec<(String, String)>` (task, reason). That's
usable. `NoBatch` and `Error` are clear. One gap: the design's
walkthrough response shows `scheduler.spawned` as an array of child
workflow names (fully qualified `parent.task`), but `Scheduled.spawned`
is `Vec<String>`. Are these short names or fully qualified? The
walkthrough uses fully qualified names. Consistent, but the code comment
should clarify.

### `BatchView`

Referenced in `derive_batch_view` return type but never defined. The
design shows a JSON schema for `koto status` output (Decision 6) but
never maps it to a Rust struct. I'd need to define `BatchView`,
`BatchSummary`, and `BatchTask` structs myself from the JSON examples.
Not hard, but the design should have included them alongside the other
struct definitions in Key Interfaces.

## 3. Integration Points

### Inserting `run_batch_scheduler` after `advance_until_stop`

The insertion point exists but isn't clean. `handle_next` is a ~800-line
function with a `match result {}` block that processes the advance result
and calls `exit(0)`. The scheduler must run inside the `Ok` arm, between
getting `advance_result` and constructing `NextResponse`.

The `advance_result` gives me `final_state`, `advanced`, and
`stop_reason`. The scheduler needs `backend`, `compiled` (template),
`final_state`, `name` (parent workflow), and `events`. All are in scope
at that point. Mechanically feasible.

The harder part: attaching the scheduler outcome to the response. The
current `NextResponse` enum has no `scheduler` field. Every variant that
could carry it (`GateBlocked`, `EvidenceRequired`, `Terminal`) would
need extension. A wrapper approach (serialize `NextResponse` plus a
sibling `scheduler` field) would be less invasive but changes the
serialization contract.

### Extending `evaluate_children_complete` with `parent_events`

Current signature:

```rust
fn evaluate_children_complete(
    backend: &dyn SessionBackend,
    workflow_name: &str,
    gate: &Gate,
) -> StructuredGateResult
```

Adding `parent_events: &[Event]` is a parameter-level change. The
function is called from a closure in `handle_next` at line ~1800 (the
`gate_closure` that captures `backend` and `name`). That closure is
passed to `evaluate_gates` as `children_evaluator: Option<&dyn
Fn(&Gate) -> StructuredGateResult>`. The closure signature is
`&dyn Fn(&Gate) -> StructuredGateResult` -- adding `parent_events`
means the closure captures it from the outer scope, which is fine since
the events are already in scope. No signature change to `evaluate_gates`
needed. This is clean.

But the design also says `evaluate_children_complete` needs to know
about the `materialize_children` hook on the current state to handle
the "no children found" case differently. That means it also needs the
`CompiledTemplate` and `current_state`. These are also capturable in
the closure. The function signature grows to 5-6 parameters, but that's
acceptable for an internal helper.

## 4. Test Strategy

The design lists integration tests (linear batch, diamond DAG,
mid-flight append, failure with skip-dependents, retry_failed recovery,
crash-resume, limit-exceeded) but no unit tests.

### What I'd unit-test in `src/cli/batch.rs`:

- **`build_dag`**: cycle detection, dangling refs, duplicate names,
  empty input, single task, max-depth enforcement.
- **`classify_task`**: each classification variant (Terminal, Failed,
  Skipped, Running, NotYetSpawned/Ready, NotYetSpawned/BlockedByDep,
  NotYetSpawned/ShouldBeSkipped). Needs a mock `SessionBackend`.
- **`derive_batch_view`**: output shape matches the JSON schema from
  Decision 6. Pure function of (events + backend state).
- **DAG limit enforcement**: 1001 tasks rejected, 11 waits_on rejected,
  depth 51 rejected.
- **Task name validation**: names passing/failing
  `validate_workflow_name` after `<parent>.` prefix.
- **Template resolution**: absolute path passthrough, relative path
  against `template_source_dir`, fallback to `submitter_cwd`, both-
  fail error with both paths listed.

### Testability concern

`run_batch_scheduler` calls `init_state_file` on a real
`SessionBackend`. For unit tests, I'd need either a mock backend or to
use `LocalBackend` with a temp directory. The design's placement of
the scheduler in `src/cli/batch.rs` means it has direct access to
backend I/O, making pure-function testing impossible for the spawn
path. The `build_dag` and `classify_task` helpers can be pure if they
take pre-fetched data, but that depends on how I factor them.

I'd split the module into:
1. Pure functions (`build_dag`, `validate_dag`, `classify_tasks`) that
   take structured input and return structured output.
2. An orchestrator (`run_batch_scheduler`) that calls the pure functions
   plus does I/O.

The design's function signatures lean this way but don't explicitly
mandate it.

## 5. What's Underspecified

### 5a. Skipped-child template identity

See 1.3 above. The design says skipped children get "a header pointing
at the parent template" but doesn't explain how a state named
`skipped_due_to_dep_failure` exists in that template. This affects
`init_state_file` call parameters, the child's `WorkflowInitialized`
event, and how `evaluate_children_complete` reads the child's terminal
status. Every downstream consumer of skipped-child state files depends
on this answer.

### 5b. `NextResponse` serialization with scheduler field

The design shows `"scheduler": {...}` as a top-level field in the JSON
response, but `NextResponse` is a Rust enum with custom serialization
(each variant serializes differently). The design doesn't say whether
to add `scheduler` to each variant, wrap the enum, or use a different
approach. This determines the serialization contract for every
consumer of `koto next` output.

### 5c. `retry_failed` interaction with the advance loop

See 1.2. The design describes the retry flow in narrative form but
doesn't pin down whether `retry_failed` is processed by the advance
loop (as evidence that routes a transition) or intercepted before it
(as a special case in `handle_next`). The null-clearing event's
relationship to `merge_epoch_evidence` is mentioned but the exact
insertion point relative to `advance_until_stop` is absent.

## Top 3 Ambiguities

Ordered by time cost to resolve (most expensive first):

1. **Skipped-child template identity (1.3 / 5a).** Blocks writing
   the skip-synthesis path, `evaluate_children_complete`'s child
   template loading, and `derive_batch_view`. Would cost 1-2 days of
   false starts before discovering the right answer experimentally.

2. **`retry_failed` flow through advance loop vs scheduler (1.2 /
   5c).** Blocks the entire retry implementation. Is it advance-loop
   evidence routing or scheduler-layer interception? The answer changes
   where 200+ lines of code go and how `merge_epoch_evidence` interacts
   with the null-clearing idiom. Would cost a day of design-while-coding.

3. **`NextResponse` serialization with scheduler field (1.5 / 5b).**
   Blocks response serialization for every `koto next` response that
   includes batch state. Determines whether I modify a 6-variant enum,
   introduce a wrapper struct, or use a post-serialization JSON merge.
   Each approach has different test and backward-compat implications.
   Would cost half a day to prototype and validate.
