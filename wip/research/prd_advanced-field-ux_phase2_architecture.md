# Phase 2 Research: Architecture Perspective

## Lead 2: Error code splitting -- Should `precondition_failed` be split into distinct error codes?

### Findings

Today, `precondition_failed` (exit code 2) covers **8 structurally different failure modes** across two layers:

**Pre-engine CLI validation (handle_next, before advance_until_stop):**
1. `--with-data` and `--to` used together (mutual exclusivity)
2. `--to` targets a state that isn't a valid transition from the current state
3. State has no `accepts` block but caller sent `--with-data`
4. Template declares a reserved variable name (defense-in-depth)
5. Advisory flock failed -- another `koto next` is already running

**Post-engine mapping (AdvanceResult/AdvanceError -> NextError):**
6. `UnresolvableTransition` -- state has conditional transitions but no accepts block (template bug)
7. `CycleDetected` -- advancement loop would revisit a state (template bug)
8. `ChainLimitReached` -- exceeded 100 transitions (template bug or very deep chain)

Additionally, all four `AdvanceError` variants (`AmbiguousTransition`, `DeadEndState`, `UnknownState`, `PersistenceError`) are caught by the blanket `Err(advance_err)` match arm at line 1841-1848, which maps them all to `precondition_failed`.

**Categorization by root cause:**

| # | Failure | Root cause | Caller can fix? |
|---|---------|------------|-----------------|
| 1 | Mutual exclusivity | Caller error (bad CLI args) | Yes -- pick one |
| 2 | Invalid --to target | Caller error (wrong target) | Yes -- check transitions |
| 3 | No accepts block | Caller error (wrong assumption) | Yes -- don't submit evidence |
| 4 | Reserved variable | Template bug | No |
| 5 | Flock contention | Concurrency | Retry (transient!) |
| 6 | Unresolvable transition | Template bug | No |
| 7 | Cycle detected | Template bug | No |
| 8 | Chain limit | Template bug or deep chain | No |
| 9 | AmbiguousTransition | Template bug | No |
| 10 | DeadEndState | Template bug | No |
| 11 | UnknownState | Template bug / corrupt state | No |
| 12 | PersistenceError | Infrastructure | No (should be exit 3) |

This reveals three natural clusters that are currently indistinguishable to callers:

- **Genuine caller errors** (#1, #2, #3): The agent did something wrong and can fix it. These are the "true" precondition_failed cases.
- **Template/engine bugs** (#4, #6, #7, #8, #9, #10, #11): The template is malformed. No agent behavior change helps. These arguably belong on exit code 3 (infrastructure).
- **Concurrency/infrastructure** (#5, #12): Transient failures. #5 (flock) could resolve on retry. #12 (PersistenceError) is I/O failure that should map to exit 3.

### Implications for Requirements

**Minimum viable split (low risk, high value):**
- Keep `precondition_failed` for genuine caller errors (#1-3).
- Add `template_error` (exit 3) for template bugs (#4, #6-11). Callers can't fix these -- they need human intervention.
- Reclassify `PersistenceError` to exit 3 (infrastructure). Currently exit 2, which tells callers "change your behavior" when the real problem is disk I/O.
- Reclassify flock contention (#5) to exit 1 (transient) or a new `concurrent_access` code. Retry is the correct caller response.

**Why not split further?** Each additional error code is an API contract. The caller decision tree for `precondition_failed` is already "stop and report" -- splitting caller errors into `invalid_args` vs `state_mismatch` doesn't change the caller's action (both require changing the CLI invocation). The split that matters is separating "caller can fix" from "caller cannot fix."

**Backward compatibility:** Adding new error codes is additive -- existing callers that match on `precondition_failed` would need updating, but the PRD can specify a migration path. The exit code change (2 -> 3) for template bugs is the breaking change; callers matching on exit code 2 would miss these. However, since the caller response to both is "stop," the practical impact is low.

### Open Questions

1. Should flock contention get its own error code (`concurrent_access`) or be reclassified to an existing transient code? It's the only precondition_failed that resolves on retry.
2. Should the blanket `Err(advance_err)` catch-all be split into per-variant mappings? Currently `AmbiguousTransition` and `DeadEndState` get identical treatment, but they have very different diagnostic value.
3. Is `PersistenceError` -> exit 2 a deliberate design choice or an oversight? The Display impl in advance.rs just formats the message; there's no special handling.

---

## Lead 5: Gate-with-evidence-fallback visibility -- Should the response surface gate failure context?

### Findings

**How the fallback works today:**

In `dispatch_next` (src/cli/next.rs, lines 60-73): When gates fail on a state that has an `accepts` block, the function skips `GateBlocked` and falls through to `EvidenceRequired`. The blocking conditions are computed (line 42-58) but silently discarded.

In `advance_until_stop` (src/engine/advance.rs, lines 290-310): The engine sets `gates_failed = true` and passes it to `resolve_transition`, which prevents unconditional transitions from firing. The engine returns `StopReason::EvidenceRequired` -- not `GateBlocked` -- so the gate results are lost.

**What callers see:**

A caller receiving `EvidenceRequired` has no way to distinguish:
- (a) A normal state that needs evidence (gates passed or no gates defined)
- (b) A state where gates failed but evidence can override/recover

This matters because the caller's optimal strategy differs:
- In case (a): Examine the directive, fill out the evidence fields.
- In case (b): Understand what gates failed, decide whether to override, retry gates, or escalate. The `blocking_conditions` data exists but isn't surfaced.

**The two code paths diverge on what information they have:**

| Path | Has gate results? | Returns |
|------|-------------------|---------|
| `dispatch_next` (CLI, --to/initial) | Yes (passed in) | `EvidenceRequired` (discards blocking) |
| `advance_until_stop` (engine loop) | Yes (computed) | `StopReason::EvidenceRequired` (no gate data) |

The engine path is the bigger problem: `StopReason::EvidenceRequired` carries no payload at all, so the CLI mapping layer (mod.rs line 1708-1721) can't surface gate results even if it wanted to.

**What would it take to surface this?**

Option A -- Add `blocking_conditions` to `EvidenceRequired`:

```rust
// In NextResponse:
EvidenceRequired {
    state: String,
    directive: String,
    advanced: bool,
    expects: ExpectsSchema,
    blocking_conditions: Vec<BlockingCondition>,  // empty when gates passed
}
```

This requires:
1. `StopReason::EvidenceRequired` grows a `BTreeMap<String, GateResult>` field (or empty map when no gates failed).
2. The engine must thread gate_results through to the stop reason when it falls through.
3. The CLI mapping converts `GateResult` -> `BlockingCondition` (logic already exists, duplicated in two places).
4. JSON output gains a `blocking_conditions` field (empty array when no gates involved).

Impact: The `EvidenceRequired` JSON shape changes. An always-present `blocking_conditions` field (empty array = no gate issues) is the cleanest evolution. Callers that don't care ignore it; callers that do get full context.

Option B -- New response variant `EvidenceRequiredWithGateContext`:

Rejected: fragmenting the response space for what's essentially the same caller action (submit evidence). Option A keeps the type system simple.

Option C -- Add an optional `gate_context` field:

```rust
EvidenceRequired {
    state: String,
    directive: String,
    advanced: bool,
    expects: ExpectsSchema,
    gate_context: Option<GateContext>,  // None when no gates evaluated
}
```

Where `GateContext` contains the blocking conditions plus maybe which gates passed. This is slightly more semantic than Option A but adds a nested object.

**Duplicate logic concern:**

Both `dispatch_next` and the `StopReason::GateBlocked` mapping in `mod.rs` independently convert `GateResult` -> `BlockingCondition` using identical filter_map logic (compare next.rs lines 42-58 with mod.rs lines 1684-1700). Surfacing gate context in `EvidenceRequired` would require a third copy of this logic, or extracting it into a shared function.

### Implications for Requirements

1. **The PRD should require `blocking_conditions` on `EvidenceRequired` responses.** An empty array means "no gate issues." A populated array means "gates failed but you can submit override evidence." This is the minimum information callers need for correct behavior.

2. **The StopReason enum needs to carry gate data.** `StopReason::EvidenceRequired` should become `StopReason::EvidenceRequired { gate_results: BTreeMap<String, GateResult> }` (empty map when gates didn't fail). This is an internal change with no external API impact.

3. **The GateResult -> BlockingCondition conversion should be extracted.** It's currently duplicated between `dispatch_next` and the `StopReason::GateBlocked` handler. Adding a third use in the `EvidenceRequired` handler makes extraction worthwhile.

4. **Backward compatibility is clean.** Adding `blocking_conditions: []` to `EvidenceRequired` responses is additive. Callers parsing the JSON won't break -- they'll just see a new field they can ignore.

### Open Questions

1. Should the `blocking_conditions` include passed gates too? Currently only failing gates are included. Including all gates would let callers see the full picture, but most don't need it.
2. Should the `EvidenceRequired` response explicitly indicate *why* it was returned (gate fallback vs. normal evidence need)? A `reason` field like `"gate_fallback"` vs `"evidence_needed"` would make the distinction machine-readable without callers inspecting `blocking_conditions.len()`.
3. When the engine path is taken (advance_until_stop), should the gate results from the *last evaluated* state be threaded through, or should the engine re-evaluate gates at the final state? Currently the engine evaluates gates once per state during advancement; the results are available in the loop but not propagated.

---

## Summary

The `precondition_failed` code conflates three distinct failure categories: genuine caller errors (fixable), template bugs (not fixable by the agent), and infrastructure/concurrency issues (transient or operator-level). A minimal split into `precondition_failed` (caller errors), `template_error` (exit 3), and reclassifying `PersistenceError` to exit 3 would give callers actionable signal without fragmenting the error space. For gate-with-evidence-fallback, the response already computes blocking conditions but discards them; adding `blocking_conditions` to the `EvidenceRequired` response (empty array when no gate issues) is an additive, backward-compatible change that lets callers distinguish normal evidence requests from gate-failure overrides.
