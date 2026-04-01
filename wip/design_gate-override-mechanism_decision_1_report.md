# Decision 1: Override Execution Model

**Question:** When a `GateOverrideRecorded` event exists for a gate in the current epoch, does the advance loop (a) skip gate execution entirely and use override data directly, or (b) execute the gate first to capture actual output for the audit trail, then substitute with override data?

**Chosen:** Option A — Skip execution

---

## Analysis

### What "actual gate output" means in R6

R6 states: "the engine emits a `GateOverrideRecorded` event containing: the state name, the overridden gate's name, the gate's actual output, the substituted values, and the rationale string."

The phrase "actual gate output" is ambiguous, but context resolves it. The override event is appended by `koto overrides record`, which runs *before* the next `koto next` invocation. At the time `koto overrides record` executes, the agent has already seen the gate's output from a prior `koto next` call — that output is what the agent is overriding. The PRD's Example 7 confirms this reading: the audit trail shows the output from the run that produced the `gate_blocked` response, not from a re-run during override application.

So "actual gate output" means "the output the gate produced when it last ran and blocked the state" — not "the output from a re-run triggered by the override application." This is fully compatible with Option A, provided the `GateOverrideRecorded` event stores the gate's last-known output at the time the agent calls `koto overrides record`.

The `koto overrides record` handler can read the gate's output from the most recent `GateBlocked`-context event in the current epoch (or require the agent to pass it via `--actual-output`). Either approach satisfies R6 without running the gate again during `koto next`.

### The `koto decisions record` precedent

`koto decisions record` appends a `DecisionRecorded` event without re-running any logic. It reads the current state from the event log and writes the decision payload — nothing else executes. `koto overrides record` is explicitly described in R5 and D7 as mirroring this pattern: "Each call overrides a single gate with a single rationale. It appends a `GateOverrideRecorded` event without advancing the workflow."

A command like "record this override" should not trigger side effects. Option B would make `koto next` secretly re-execute potentially slow shell commands, HTTP checks, or external service calls solely to populate an audit field that was already known. That contradicts the principle that `koto next` advances the workflow — it doesn't re-audit it.

### Performance cost of Option B is unbounded

Command gates run shell commands with configurable timeouts. Context gates are cheap, but future gate types (http, jira, json-command per the PRD's future registry) may be slow or have side effects. An agent might override three gates across multiple `koto overrides record` calls. Under Option B, `koto next` would re-run all three gates before discarding their results and substituting the override data. The re-runs are always wasted work by construction — if the gate passed, the override wouldn't be needed; if it still fails, the result is thrown away anyway.

For the specific use case driving this feature ("I've handled this out of band"), re-running the gate to confirm it still fails is both redundant and potentially harmful. Some gates are not idempotent (a gate that moves a Jira ticket or triggers a CI run would be called twice).

### Parallel gate evaluation

If gates are ever evaluated in parallel (a natural optimization for multiple independent gates), Option B creates a race: gate execution and override substitution must be serialized per gate. Under Option A, the advance loop simply checks for an override event before deciding whether to call the gate at all — the gate is either in the override set (skip it) or not (run it). This is a clean per-gate predicate with no ordering complexity.

### Epoch stickiness consistency

Override events are sticky within an epoch. Multiple `koto next` calls in the same epoch all see the same set of override events. Under Option B, each `koto next` call would re-run the overridden gates, producing potentially different outputs each time (the gate condition might change between calls), while still substituting the override data. This means the audit trail would contain different "actual outputs" for what is logically the same override. Under Option A, the actual output is recorded once when the agent calls `koto overrides record` — it's stable and self-contained.

### Resolving R6 under Option A

The `GateOverrideRecorded` event must carry actual gate output. Under Option A, this is populated at `koto overrides record` time:

1. The agent calls `koto next`, which returns a `gate_blocked` response with structured gate output in `blocking_conditions`.
2. The agent calls `koto overrides record --gate <name> --rationale "..."`, optionally with `--with-data`.
3. The `koto overrides record` handler reads the gate's last blocking output from the current epoch's event log (from the most recent advance invocation's gate evaluation results, which should be persisted in the event log or derivable from it). It stores this as `actual_output` in `GateOverrideRecorded`.
4. On the next `koto next`, the advance loop detects the override event, skips executing the gate, and injects the `override_data` directly into the gates evidence map.

This requires that gate outputs from blocking runs be persisted in the event log — but they must be, since `koto next` already injects gate output into the evidence map for transition routing, and that evidence needs to be derivable from the log for `koto status` and `koto query` to work correctly.

If gate outputs are not currently persisted as events, a `GateEvaluated` event (emitted during the advance loop for each gate run) would provide the anchor. The `koto overrides record` handler would then read the latest `GateEvaluated` event for the named gate within the current epoch to populate `actual_output`.

### Option B's one legitimate argument

Option B ensures the audit record reflects the gate's state at override-application time, not blocking time. If a gate condition changes between `koto overrides record` and `koto next` (e.g., the CI pipeline finishes and now passes), Option B would record a passing output alongside the override — revealing that the override was unnecessary.

This is a valid audit scenario but not the motivating use case. The PRD's primary concern is capturing why an agent overrode a gate that was blocking the workflow. That rationale is most accurate at blocking time, not at advance time. An agent who records an override because "CI was flaky on an unrelated test" shouldn't have the audit trail silently show a passing gate — that would imply the override was pointless when it wasn't at the time of the decision.

---

## Conclusion

Option A (skip execution) is the right choice. It matches the `koto decisions record` precedent, avoids unnecessary latency and side effects, composes cleanly with parallel gate evaluation, produces a stable and self-consistent audit trail, and satisfies R6 if gate outputs are persisted in the event log at blocking time. "Actual gate output" in R6 refers to the gate's output at the time the override was recorded — not a re-execution at advance time.

The key implementation consequence: `koto overrides record` must populate `actual_output` in `GateOverrideRecorded` by reading the gate's output from the current epoch's event log. This requires either a `GateEvaluated` event type that the advance loop emits when gates run, or another mechanism to make the last-known gate output queryable. Decision 3 (override event persistence and advance loop integration) should address this dependency explicitly.
