# Decision: Action output capture and override prevention

## Chosen

Option A (DefaultActionExecuted event) + Option X (epoch evidence check before execution).

## Confidence

High (90%).

## Rationale

### Output capture: Option A wins

The event log is the single source of truth for everything that happened in a workflow. Every meaningful side effect -- transitions, evidence submissions, integration invocations, rewinds -- gets its own event type with dedicated payload fields. Default action execution is a new category of side effect with distinct payload requirements (stdout, stderr, exit_code, the command that ran). It deserves its own variant.

Key observations from the code:

1. **The EventPayload enum is a discriminated union with explicit type names.** Each variant maps to a `type_name()` string used for serialization (types.rs lines 64-77). The custom `Deserialize` implementation on `Event` switches on `event_type` to select the correct payload variant (types.rs lines 143-208). Adding `DefaultActionExecuted` follows the established pattern: one new match arm in `type_name()`, one new arm in the deserializer, one new helper struct for deserialization. This is mechanical.

2. **IntegrationInvoked is semantically wrong for actions.** `IntegrationInvoked` carries `state`, `integration` (name), and `output` (a single `serde_json::Value`). Actions need `command`, `exit_code`, `stdout`, and `stderr` as separate fields. Cramming these into a generic `output` JSON blob would work at the serialization level but loses type safety and makes the deserialization path ambiguous. The advance loop already checks for `IntegrationInvoked` events to handle re-invocation prevention (advance.rs comment at line 121); mixing action events into the same variant would require additional discrimination logic everywhere those events are queried.

3. **The sidecar approach (Option C) breaks atomicity.** Persistence in koto is JSONL-append with `sync_data()` after every write (persistence.rs lines 56-79). The event log is self-contained: you can copy the `.state.jsonl` file and have the full workflow history. Sidecar files would break this property. If a sidecar write succeeds but the event append fails (or vice versa), the state becomes inconsistent. Output files would also need their own cleanup lifecycle, which the engine currently has no mechanism for.

4. **Output size is bounded by action execution time.** Default actions are short-lived commands (git branch creation, file writes, CI triggers). Their stdout/stderr will typically be a few lines to a few KB. The event log already stores arbitrary JSON values in `IntegrationInvoked.output`. Storing action output inline keeps the single-file invariant and avoids premature optimization for a problem that doesn't exist.

Proposed variant:

```rust
DefaultActionExecuted {
    state: String,
    command: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}
```

The `command` field records what ran (after variable substitution), giving full auditability. `exit_code` distinguishes success from failure. Both `stdout` and `stderr` are captured as strings -- binary output is not a supported use case for workflow actions.

### Override prevention: Option X wins

The evidence epoch system already solves the temporal scoping problem. `derive_evidence()` in persistence.rs (lines 235-265) returns only `EvidenceSubmitted` events after the most recent state-changing event. The advance loop clears evidence on auto-advance (advance.rs line 267). This infrastructure is sufficient to answer "has the agent already provided evidence for this state visit?"

Key observations:

1. **Epoch boundaries are well-defined.** When the engine enters a state (via transition or rewind), that event becomes the epoch boundary. Any `EvidenceSubmitted` events after that boundary belong to the current epoch. Checking for evidence before running the action is a single call to `derive_evidence()` followed by `merge_epoch_evidence()` -- both already exist and are tested.

2. **No new template schema is needed.** Option Y would add a `pre_action_condition` field to `TemplateState` or `ActionDecl`, introducing template-level complexity for something the engine can determine at runtime. The override rule is universal: "if evidence exists for this state in the current epoch, the agent has spoken; skip the default action." This doesn't vary per state, so it shouldn't be configurable per state.

3. **The epoch is available at execution time.** The advance loop receives `evidence` as a parameter (advance.rs line 133). For the initial state, this is the merged epoch evidence from the caller. For auto-advanced states, it's an empty map (advance.rs line 267). Before calling the action closure, the engine checks: if evidence is non-empty, skip the action. This is a single `if !current_evidence.is_empty()` check.

4. **This covers the primary use case.** The agent or skill layer submits evidence via `koto evidence submit` before calling `koto next`. If evidence exists, the action is skipped. If not, the action runs. There's no ambiguity about ordering because evidence submission and advancement are separate CLI calls with atomically persisted events.

### How the pieces fit together

The execution flow within `advance_until_stop` for a state with a default action:

```
1. Signal check
2. Chain limit check
3. Terminal check
4. Integration check
5. Gate evaluation
6. [NEW] Override check: if current_evidence is non-empty, skip to step 8
7. [NEW] Action execution: call action closure, append DefaultActionExecuted event
8. [NEW] If action ran and gate fails on re-check: include action output in StopReason
9. Transition resolution
```

When the gate-with-evidence-fallback pattern from #69 activates (gate fails, state has accepts block), the `handle_next` dispatcher can scan the event log for `DefaultActionExecuted` events in the current epoch and include stdout/stderr in the response. This gives the agent the action output it needs to decide what evidence to submit.

## Risks

1. **Large stdout/stderr.** If an action produces megabytes of output, the event log grows proportionally. Mitigation: truncate stdout/stderr at a configurable limit (default 64KB) with a `truncated: true` flag. This can be added to the variant without changing the event schema since missing fields default to false.

2. **Action output encoding.** Non-UTF-8 output would fail `String` storage. Mitigation: replace invalid UTF-8 sequences with the replacement character during capture. Document that action output is treated as text.

3. **Evidence race condition.** If an external process submits evidence between the engine reading the epoch and executing the action, the action runs despite evidence existing. This is a theoretical concern: the advance loop holds no lock, and `koto next` is the only caller. In practice, the CLI serializes calls through a single process. If concurrent access becomes a use case, file locking on the state file would be the solution (orthogonal to this design).
