# Architecture Review: Auto-Advancement Engine Design

## Reviewer: architect-reviewer
## Date: 2026-03-17

---

## 1. Structural Fit Assessment

### Engine-layer placement (FITS)

The design places `advance_until_stop()` in `src/engine/advance.rs`. This is
structurally correct. The existing engine module already contains `persistence.rs`
(event I/O), `evidence.rs` (validation), `types.rs` (domain types), and
`errors.rs`. The advancement loop is workflow logic that depends on these modules,
so it belongs alongside them.

The dependency direction is clean: `engine/advance.rs` will depend on
`engine/types`, `engine/persistence`, `template/types`, and `gate` (via closures,
not direct import). The CLI handler sets up closures and calls the engine. No
upward dependency from engine to CLI.

### Pure function preservation (FITS)

`dispatch_next` in `src/cli/next.rs` stays pure. The engine calls it within the
loop body for classification. The design explicitly states this, and the
`advance_until_stop` signature confirms it -- all I/O is through injected
closures. This preserves the existing testability pattern.

### Event type reuse (FITS)

`EventPayload` already has `Transitioned`, `IntegrationInvoked`, and
`EvidenceSubmitted` variants. The engine emits transitions via the `append_event`
closure using existing payload types. The only new variant is `WorkflowCancelled`
for Phase 4, which follows the established pattern (add variant, add `type_name()`
match arm, add deserializer arm).

### CLI surface (FITS)

`koto cancel` is a new subcommand with a clear, non-overlapping purpose. It
doesn't duplicate any existing command. The `Command` enum gets a new `Cancel`
variant, following the established pattern.

---

## 2. Architectural Findings

### Finding 1: `dispatch_next` fallback creates a dual classification system (Advisory)

`dispatch_next` currently returns `EvidenceRequired` with empty `expects` as a
signal for auto-advance candidates (lines 98-111 of `next.rs`). The engine's
`StopReason` enum has explicit variants for every stopping condition. During
Phase 2, the handler must map `StopReason` back to `NextResponse`, which means
the empty-expects fallback becomes dead code for the auto-advance path.

The design acknowledges this ("Two classification systems coexist") and notes the
mapping is straightforward. This is contained -- `dispatch_next` is called only
from `handle_next` and from the engine's loop body. It doesn't compound because
no other code depends on the empty-expects fallback semantics.

**Recommendation:** During Phase 2, update the fallback branch in `dispatch_next`
to be explicit (e.g., return a distinct `AutoAdvance` variant or remove the
fallback entirely since the engine handles it). This prevents future callers from
misinterpreting the empty-expects signal.

### Finding 2: `append_event` re-reads file on every call (Advisory, acknowledged)

The design calls this out in Consequences/Negative. `append_event` in
`persistence.rs` (line 44) calls `read_last_seq(path)` which reads the entire
file to determine the next sequence number. In the advancement loop, this happens
per transition. For a 5-state chain, that's 5 full file reads.

The design's mitigation (pass expected next seq as parameter) is the right fix.
Since the engine already tracks state, it can track the last seq returned by the
closure and pass it forward. This doesn't need to block the initial
implementation -- the closure abstraction means the optimization can be added
without changing the engine's interface.

**Not blocking** because the closure signature `FnMut(&EventPayload) ->
Result<(), PersistenceError>` is the right abstraction boundary. The optimization
changes the closure's implementation, not the engine.

### Finding 3: Gate evaluator signature mismatch with closure interface (Blocking)

The design specifies the gate evaluation closure as:

```rust
G: Fn(&BTreeMap<String, Gate>, &Path) -> BTreeMap<String, GateResult>
```

This matches the existing `evaluate_gates` function signature in `src/gate.rs`
(line 39-42), which takes `gates: &BTreeMap<String, Gate>` and
`working_dir: &Path`.

However, the engine doesn't have access to the working directory -- it's a CLI
concern (derived from `std::env::current_dir()` in `handle_next`). The handler
must capture `current_dir` in the closure. This works, but the closure signature
leaks the `Path` parameter into the engine when the engine doesn't need to know
about working directories.

A cleaner closure signature would be:

```rust
G: Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, GateResult>
```

With the handler closing over `current_dir`:

```rust
let evaluate = |gates: &BTreeMap<String, Gate>| evaluate_gates(gates, &current_dir);
```

**Blocking** because the `Path` parameter in the engine's closure signature
creates an unnecessary coupling. If the gate evaluator's working directory
behavior changes (e.g., per-gate working directories from template config), the
engine's interface would need to change too. Closing over the path in the handler
keeps the engine decoupled.

### Finding 4: `resolve_transition` handles evidence but `dispatch_next` also classifies evidence states (Advisory)

The engine's `resolve_transition` function resolves transitions by matching
evidence against `when` conditions. Meanwhile, `dispatch_next` classifies states
as `EvidenceRequired` when an `accepts` block exists. In the advancement loop,
the engine must decide whether to call `resolve_transition` (to check if
existing evidence matches) or stop with `EvidenceRequired`.

The design's data flow (lines 324-341) handles this correctly: gates are
evaluated first, then `resolve_transition` is called. If it returns
`NeedsEvidence`, the engine stops. This is the right sequencing -- it means
`dispatch_next` is only called for the final state (where the loop stops), not
for intermediate states.

Wait -- actually, looking at the design more carefully, the loop body (lines
324-341) does NOT call `dispatch_next` at all. It does its own classification:
check terminal, check integration, evaluate gates, resolve transition. The
`dispatch_next` function is only used in the `StopReason -> NextResponse`
mapping at the end.

This means Phase 2 needs to either:
(a) Map `StopReason` to `NextResponse` directly (duplicating some of
    `dispatch_next`'s logic), or
(b) Call `dispatch_next` on the final state with appropriate gate results.

The design says "calls existing `dispatch_next` classifier" in the overview
(line 68) but the actual loop pseudocode doesn't use it. The
`StopReason -> NextResponse` mapping in the handler is the actual translation
point, and the design's `StopReason` variants align with `NextResponse` variants
well enough that option (a) is straightforward.

**Recommendation:** Clarify in the design whether `dispatch_next` is called
within the loop or only at the end for the final mapping. The current text is
contradictory. Either approach works, but the implementer needs to know which.

### Finding 5: No `WorkflowCancelled` consumer specified beyond terminal check (Advisory)

Phase 4 adds `WorkflowCancelled` to `EventPayload`. The design says
"`dispatch_next` / engine recognizes cancelled state as terminal." But
`WorkflowCancelled` isn't a state transition -- it's a workflow-level event.
`derive_state_from_log` won't change the current state when it sees this event.

The design needs to specify how the cancelled state is detected:
- Option A: `derive_state_from_log` treats `WorkflowCancelled` as producing a
  synthetic terminal state (e.g., "__cancelled__").
- Option B: `derive_machine_state` returns a separate `cancelled: bool` field,
  and `handle_next` checks it before entering the loop.
- Option C: `WorkflowCancelled` is not checked at loop time. Instead, `koto next`
  on a cancelled workflow returns an error because the pre-loop check detects
  the event.

Option C is simplest and matches the design's "verify workflow is not already
terminal or cancelled" check in the `koto cancel` flow. But it requires adding
a `is_cancelled` check to `handle_next` as well, not just to `koto cancel`.

**Recommendation:** Specify the cancellation detection mechanism. Without it,
the implementer will need to make an architectural choice that should be in the
design.

### Finding 6: Integration re-invocation prevention reads epoch events (Advisory)

The design specifies (lines 303-306): "Before invoking the integration runner,
the engine checks whether an `integration_invoked` event already exists in the
current epoch for the current state."

The existing `derive_evidence` function in `persistence.rs` (lines 235-265)
already implements epoch-scoped event filtering. The integration check needs the
same epoch logic but for `IntegrationInvoked` events instead of
`EvidenceSubmitted`. The engine should use the same epoch derivation, not
implement a parallel one.

**Recommendation:** Either generalize `derive_evidence` to return all events in
the current epoch (not just evidence), or add a parallel `derive_integration_events`
that reuses the epoch-boundary logic. The design should specify which approach to
avoid a parallel implementation.

### Finding 7: Flock and signal handling interaction is underspecified (Advisory)

The design says:
- Flock acquired before the loop, released on exit
- Signal handler sets `AtomicBool`, checked between iterations
- "The last fsync'd event is always durable before the check"

But: if a signal arrives during `append_event` (which calls `sync_data()`), the
event is either fully written or not -- this is fine due to the append-only
JSONL format and `sync_data()`. However, if a signal arrives during gate
evaluation (which can take up to 30 seconds), the `AtomicBool` is set but won't
be checked until the gate evaluation completes. The design acknowledges the
30-second delay.

The flock is released implicitly when the file descriptor is closed (process
exit), which is correct for advisory locks. But if the signal handler calls
`std::process::exit()`, the flock will be released. If the handler just sets the
flag, the loop will finish the current iteration and exit cleanly.

The design's approach (flag-only signal handler, check between iterations) is
correct and safe. The 30-second worst-case delay is acceptable as stated.

No structural concern here -- just confirming the design handles this correctly.

---

## 3. Implementation Phase Sequencing

### Phase 1: Transition resolution and advance engine -- CORRECT

Building the core engine first with mock closures is the right order. The engine
is the highest-risk component (cycle detection, transition resolution, stopping
conditions), and testing it with mocks catches logic bugs early.

One concern: the unit tests for `advance_until_stop` will need mock closures that
track call order and arguments. The test helpers should be reusable across
Phase 1 and Phase 2. Consider defining a `MockIO` struct in a test helper module
rather than inline closures in each test.

### Phase 2: Handler integration -- CORRECT

Wiring the engine into `handle_next` after the engine is tested makes sense.
This phase has the highest coupling risk (the 340-line function refactor), but
the engine's clean closure interface bounds the change.

The existing `handle_next` test coverage is through integration tests (CLI
invocations). Phase 2 should not break existing tests since the external behavior
is the same (or better -- auto-advancement reduces calls). New integration tests
verify multi-state chains.

### Phase 3: Signal handling and file locking -- CORRECT

These are cross-cutting concerns that are easier to add after the loop works.
Adding them earlier would complicate Phase 1 testing (signal handling in unit
tests is messy). The `AtomicBool` parameter is already in the `advance_until_stop`
signature, so Phase 1 can pass a never-set flag and Phase 3 wires it to the
signal handler.

### Phase 4: koto cancel -- CORRECT

Depends on knowing the event type system is stable (Phase 1-3). Adding a new
event variant and subcommand is low-risk and independent of the advancement
loop logic.

---

## 4. Simpler Alternatives Considered

### Alternative: Inline the loop in `handle_next` without a separate engine module

The design's rejected "Handler-Layer Loop" alternative would save ~200-400 lines
of abstraction (no closure setup, no `StopReason` type, no engine module). Given
that `handle_next` is already 340 lines and the loop adds cycle detection, signal
checking, and five stopping conditions, the engine extraction is justified.

The key argument for extraction is testability: the loop's correctness properties
(cycle detection terminates, signal flag is checked between iterations, every
stopping condition returns the right variant) are best verified with unit tests
using mock closures. Testing these through integration tests (CLI invocations with
real state files) is fragile and slow.

### Alternative: Use `dispatch_next` as the loop body instead of a new classification

The engine could call `dispatch_next` for each state and interpret the
`NextResponse` to decide whether to continue. This would eliminate the
`StopReason` type entirely.

This doesn't work because `dispatch_next` doesn't know about transitions -- it
classifies the current state's stopping condition but doesn't resolve which
target to transition to. The engine needs both classification (should I stop?)
and resolution (where do I go?). `resolve_transition` handles the second part,
which `dispatch_next` doesn't do.

The dual type system (`StopReason` + `NextResponse`) is justified.

---

## 5. Summary

The design fits the existing architecture. New code goes in the right modules,
dependencies flow in the right direction, existing abstractions are reused rather
than duplicated, and the CLI surface is extended without overlap.

Three items need attention before implementation:

1. **Gate closure signature** (Blocking): Remove the `&Path` parameter from the
   engine's gate evaluation closure. The handler should close over the working
   directory.

2. **`dispatch_next` role clarification** (Advisory): The design text says
   `dispatch_next` is called within the loop, but the pseudocode doesn't use it.
   Clarify whether it's used in the loop body or only for the final
   `StopReason -> NextResponse` mapping.

3. **Cancellation detection mechanism** (Advisory): Specify how `WorkflowCancelled`
   events are detected by `handle_next` and the engine. The current design only
   specifies detection in `koto cancel` itself.
