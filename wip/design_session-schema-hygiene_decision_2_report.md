# Decision 2: context_added event emission architecture

## Recommendation
Option A: Pass SessionBackend into handle_context_add

## Confidence
High

## Rationale
The call site in `run()` already holds a `Backend` that implements both `SessionBackend` and `ContextStore`. Passing `&backend as &dyn SessionBackend` alongside the existing `store` adds one parameter to `handle_add` and one argument at the single call site — a minimal, localized change. The ordering guarantee is mechanically enforced because `append_event` assigns `seq = last_seq + 1` by reading the file at call time; emitting the event immediately after `store.add()` returns (and before `handle_add` returns to the caller) guarantees the `context_added.seq` is less than any `seq` assigned by a later `koto next` invocation. R3.5 failure atomicity falls out naturally: if `append_event` fails, `handle_add` propagates the error to the caller.

## Option Analysis

### Option A: Pass SessionBackend into handler
**Pros:**
- Minimal footprint. One new parameter on `handle_add`, one new argument in the single dispatch site (`run()` at line ~899). No new types.
- Ordering guarantee is mechanical. `append_event` does a read-then-write of `seq`; calling it inside `handle_add` after `store.add()` returns is sufficient. Any subsequent `koto next` call will read a higher last seq and assign a larger number.
- R3.5 atomicity is automatic. If `store.add()` fails, the function returns early — no event emitted. If `store.add()` succeeds but `append_event` fails, the error propagates to the caller with no partial event written.
- Consistent with existing patterns. `handle_next` already takes both `&dyn SessionBackend` and `&dyn ContextStore` as separate parameters. `handle_decisions_record` and `handle_rewind` take `&dyn SessionBackend` directly. This pattern is already established.
- No new abstraction to maintain. The trait objects stay independent; their lifetime is the same because both are borrowed from the same `Backend` value at the call site.
- Call site already has both. At the `ContextCommand::Add` dispatch, `backend` is in scope and `store` is derived from it. Adding `&backend` as the extra argument requires one line of change in `mod.rs` and one extra parameter in `context.rs`.

**Cons:**
- If a second call site is ever added (e.g., a batch operation that calls context add), the caller must remember to pass both. This is a mild disciplinary cost, not a structural problem.
- The signature of `handle_add` grows slightly. This is negligible.

### Option B: LoggingContextStore wrapper
**Pros:**
- `handle_add` signature stays `(store: &dyn ContextStore, ...)`. No change to handler signature.
- The logging concern is encapsulated: the wrapper holds both the store and the backend, emitting the event inside `add()`.

**Cons:**
- Introduces a new type (`LoggingContextStore`) that must be wired at the call site anyway. The call site still needs access to `&dyn SessionBackend` to construct the wrapper, so the number of concerns visible to the call site does not decrease.
- The wrapper's `add()` method mixes two responsibilities: context storage and event emission. When the event emission path diverges from the storage path (e.g., a future in-process context add that should not log), this layering becomes awkward to work around.
- The `ContextStore` trait does not have access to a session backend today and would need the wrapper to carry one. This is a hidden coupling that is less readable than an explicit parameter.
- More code surface for a one-off concern. The only operation in `ContextStore` that needs event emission is `add`; wrapping the whole trait for this is disproportionate.

### Option C: Trait consolidation
**Pros:**
- No new parameters or types. The concrete `LocalBackend` (or `CloudBackend`) has both capabilities and can be passed directly.
- Avoids the thin-wrapper indirection of Option B.

**Cons:**
- The `Backend` enum is the unified dispatcher, but it is defined in `session/mod.rs` and is not meant to be exposed as a first-class parameter type to CLI handlers. CLI handlers currently receive trait objects, which allows future backends to be added without changing handler signatures.
- Passing a concrete `Backend` (or its inner types) to handlers breaks the trait-object boundary that the rest of the codebase enforces. `handle_next` receives `&dyn SessionBackend` even though the caller has a `Backend` — that abstraction is deliberate.
- Trait consolidation implies merging `SessionBackend` and `ContextStore` into a single supertrait, which is premature. These traits serve different concerns and not all implementors need both.
- `LocalBackend` and `CloudBackend` both implement both traits today, but future backends (e.g., a read-only audit backend) might implement `SessionBackend` but not `ContextStore`. Consolidation forecloses that.

### Option D: Two-phase reconciliation
**Pros:**
- No change to `handle_add`. The context add path remains unchanged.
- SessionBackend is never needed in the context path.

**Cons:**
- Directly violates R3.3 and R3.4. The PRD requires synchronous emission and a strict ordering guarantee. Reconciliation at `koto next` time is not synchronous — the event is emitted later, not during the `context add` operation.
- The ordering guarantee in R3.4 is defined in terms of `seq`: `context_added.seq < seq(next koto next event)`. Reconciliation at `koto next` time would assign a seq at `koto next` time, which is the opposite of the requirement: the `context_added` event would have a higher seq than the transition event that just occurred, or at best the same seq epoch (which would be undefined ordering).
- The manifest sidecar is mutable. `koto context add` can overwrite a key; reconciliation would need to differentiate "first add" from "overwrite", requiring version tracking not present in the manifest today.
- Creates a class of silent failures: if `koto next` is never called after a `context add`, the event is never emitted. R3.5 requires surfacing errors, not deferring them.

## Call Chain Findings

The full call chain for `koto context add` today:

1. `main()` in `src/main.rs` calls `run(app)`.
2. `run()` in `src/cli/mod.rs` matches `Command::Context { subcommand }`, calls `build_backend()` to get a `Backend` (line 891), then casts it to `let store: &dyn ContextStore = &backend` (line 892).
3. `Command::Context / ContextCommand::Add` dispatches to `context::handle_add(store, &session, &key, from_file.as_deref())` (line 899). The `backend` binding is in scope but is not passed.
4. `handle_add` in `src/cli/context.rs` (line 12) accepts only `store: &dyn ContextStore`, `session: &str`, `key: &str`, `from_file: Option<&str>`. It reads content, calls `store.add(session, key, &content)`, and returns.
5. `store.add()` dispatches through the `ContextStore` impl on `Backend`, which dispatches to `LocalBackend::add()` or `CloudBackend::add()`.
6. `LocalBackend::add()` in `src/session/local.rs` (line 470) writes the content file, locks the manifest, updates `manifest.json`, and returns. No event is emitted anywhere in this path.

`SessionBackend` is not reachable from `handle_add`. The `run()` call site has both `backend` (which implements `SessionBackend`) and `store` (which is `&backend` cast to `&dyn ContextStore`), but only `store` is forwarded.

`handle_next` (line 1656) takes both `backend: &dyn SessionBackend` and `context_store: &dyn ContextStore` as separate parameters, establishing the pattern that Option A follows. `handle_decisions_record` (line 2994) takes only `backend: &dyn SessionBackend` and calls `backend.append_event()` directly.

The `seq` number is auto-assigned by `persistence::append_event()` (line 43 of `src/engine/persistence.rs`): it reads the last seq from the file and appends with `last + 1`. This means any call to `backend.append_event()` inside `handle_add` — after `store.add()` returns — will assign a seq that is strictly less than any seq assigned by a future `koto next` invocation, satisfying R3.4 without any additional coordination.

## Key Assumptions

- The single call site for `handle_add` is the `ContextCommand::Add` arm in `run()`. If additional call sites exist or are planned (e.g., batch context add), they must also be updated to pass the `SessionBackend`.
- The `Backend` value in `run()` lives for the duration of the command invocation. Borrowing `&backend as &dyn SessionBackend` alongside `&backend as &dyn ContextStore` does not violate Rust's borrow rules because both are shared (immutable) borrows.
- `append_event` in `persistence.rs` uses the last-seq-plus-one strategy without external locking beyond the implicit serialization that comes from single-process, single-threaded CLI invocations. The ordering guarantee holds as long as no concurrent `koto next` call is in progress during `koto context add` — the same assumption the rest of the system makes.
- R3.5's "surface the error" requirement means `handle_add` must return `Err` if `append_event` fails. The existing error propagation pattern (`?` operator or explicit `map_err`) satisfies this.

## Rejected Options

**Option D** is rejected outright. It violates R3.3 (synchronous emission) and R3.4 (ordering guarantee) by design. The reconciliation model produces events with seq numbers that are higher than the transition event that follows the context add, inverting the required ordering.

**Option B** is rejected as disproportionate. It introduces a new wrapper type for a concern that can be addressed with a single parameter. The wrapper's encapsulation benefit is illusory — the call site still supplies both the store and the backend — while the hidden coupling inside the wrapper is harder to audit than an explicit function signature.

**Option C** is rejected because it breaks the trait-object boundary that the codebase consistently enforces across all CLI handlers. Passing a concrete `Backend` or a combined supertrait to handlers introduces coupling between handler code and backend implementation strategy that trait objects are specifically designed to avoid.
