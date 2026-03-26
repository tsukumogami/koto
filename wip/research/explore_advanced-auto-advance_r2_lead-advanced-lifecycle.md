# Lead: What happens to `advanced` after auto-advance?

## Findings

### Where `advanced` is Set

1. **Engine initialization** (src/engine/advance.rs:176):
   - `let mut advanced = false;` at function start
   - Single point of birth in `advance_until_stop()`

2. **The only mutation** (src/engine/advance.rs:333):
   - Set to `true` when a transition is resolved and executed
   - Line: `advanced = true;` (unconditional, atomic operation)
   - Occurs inside the auto-advance loop after `append_event()` succeeds
   - Represents "at least one transition happened in THIS invocation"

3. **Propagation through returns** (src/engine/advance.rs:189-350):
   - All 11 `AdvanceResult` constructions carry the current `advanced` value
   - No transformation; value is threaded through all stop reasons unchanged
   - Early returns (shutdown, chain limit, terminal, integration, gates, action confirmation) preserve whatever `advanced` is at that point

### Where `advanced` is Read/Consumed

1. **CLI output layer** (src/cli/mod.rs:1132):
   - Single extraction: `let advanced = advance_result.advanced;`
   - Flows into NextResponse variants as a field

2. **NextResponse serialization** (src/cli/next_types.rs:138-233):
   - Custom `Serialize` impl writes `advanced` to JSON in all 6 variants
   - EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable, Terminal, ActionRequiresConfirmation
   - Always present in response JSON

3. **NextResponse manipulation** (src/cli/next_types.rs:67-135):
   - `with_substituted_directive()` method preserves `advanced` through transformation
   - No consumption logic; just passes through

4. **Integration tests** (tests/integration_test.rs):
   - 22 assertions checking `advanced` value
   - Assert `true` when transitions expected: `assert_eq!(json["advanced"], true, "...")`
   - Assert `false` when no transitions expected: `assert_eq!(json["advanced"], false, "...")`
   - Examples: line 331, 1328, 1437, 1467, 1506, 1591, 1650, 1674, 2545, 2771

5. **Engine unit tests** (src/engine/advance.rs:850-1547):
   - 14 assertions inside `tests` module
   - Validate state machine transitions occur/don't occur as expected

### Field's Lifecycle

```
Birth           → Flow                → Death
┌─────────────┐   ┌────────────────┐   ┌──────────────┐
│ false (176) │ → │ Thread through │ → │ JSON output  │
│             │   │ loop returns   │   │ (client read)│
│   ↓         │   │ (189-350)      │   │              │
│ set true    │   │                │   │              │
│ (333) on    │   │ Extracted (1132)   │              │
│ transition  │   │ ↓              │   │              │
│             │   │ NextResponse   │   │              │
│             │   │ variants       │   │              │
│             │   │ (25-61)        │   │              │
│             │   │                │   │              │
│             │   │ Serialized     │   │              │
│             │   │ (151, 166, 183,│   │              │
│             │   │ 200, 210, 226) │   │              │
│             │   │                │   │              │
│             │   └─────────────────→  └──────────────┘
└─────────────┘
```

### Auto-Advance Behavior and `advanced` Semantics

The engine's auto-advance loop (lines 186-357) has three outcomes per invocation:

**Scenario A: Resolves transition → `advanced = true`**
- Matches conditional against evidence → triggers transition
- OR no conditionals but unconditional fallback exists + gates not failed → auto-advances
- Transitions are recorded as `EventPayload::Transitioned { condition_type: "auto" }`
- Evidence cleared for next iteration (line 336)
- Loop continues

**Scenario B: Needs evidence → `advanced` carries prior state (often `false`, sometimes `true`)**
- Conditional transitions exist but no evidence matches AND no unconditional fallback
- OR gates failed + unconditional fallback exists but suppressed (line 413-417)
- Loop stops, returns `EvidenceRequired` with whatever `advanced` is
- **Critical semantics**: `advanced = true` here means "prior transitions occurred in THIS invocation before hitting evidence requirement"

**Scenario C: Stop without transition → `advanced` carries prior state**
- Terminal state (line 215-220): `advanced` reflects whether transitions occurred before reaching terminal
- Integration declared (line 227-252): similar
- Action requires confirmation (line 273-282): similar
- Other stops (shutdown, chain limit, cycle, gate blocked): all preserve `advanced`

### Post-Auto-Advance Scenarios Where `advanced: true` is Still Returned

The key insight: **`advanced` is NOT eliminated by auto-advance; it becomes a CHAIN indicator**.

1. **Multi-transition chains**:
   - "plan → implement → verify" chain: first call returns `advanced: true` at "verify" because 2 transitions occurred
   - Evidence submission followed by auto-advance: returns `advanced: true` at evidence-required state after having advanced

2. **Terminal state reached via chain**:
   - "start → done" (unconditional) returns `advanced: true, action: done`
   - The flag indicates "we didn't start here"

3. **Evidence requirement after prior advance**:
   - State A → (auto) → State B, but State B needs evidence
   - Response: `advanced: true, action: execute` (State B)
   - Tells caller: "we progressed but need your input now"

4. **Gate blocked or integration after advance**:
   - States auto-advanced but stopped at gate → `advanced: true` + `blocking_conditions`
   - States auto-advanced but stopped at integration → `advanced: true` + `integration` output

### Purpose of `advanced` Field

The field serves **two asymmetric roles**:

**Primary role (pre-auto-advance intent)**: Report agent-initiated changes
- Used by agents to detect whether their evidence submission caused progression
- "Did my action matter?" → check `advanced`

**Secondary role (actual implementation)**: Report any engine-initiated progression in this invocation
- Auto-advance chains that complete and stop still carry `true`
- Clients learn "workflow progressed beyond your starting state"

### Would `advanced` Become Vestigial?

**Not fully, but its semantics become diluted.**

If auto-advance "keeps looping until evidence is required," the scenarios where `advanced: false` returned are:
1. First call hits immediate evidence requirement (no transitions possible)
2. First call hits gate block on first state (gates prevent any transition)
3. First call hits integration on first state
4. First call hits action-requires-confirmation on first state
5. First call lands on terminal state but started there (no transitions)

These are **narrow edge cases** in well-designed templates. Most real workflows have:
- Unconditional starter transitions
- Chains of auto-advanceable states
- Terminal states reached after transitions

So `advanced: true` becomes the **majority case**, diluting its signaling value.

The field doesn't tell the caller:
- "How many transitions?" (only boolean)
- "Which states were visited?" (only the final state)
- "How long was the chain?" (no count)
- "Did the transition come from my input or auto-advance?" (boolean doesn't distinguish)

Post-auto-advance, the field becomes **a "not at starting state" indicator** rather than "agent action mattered."

### Implications for Issue #89 (Auto-Advance)

Issue #89 asks koto to "auto-advance past `advanced: true` phases."

**Current behavior**: Caller must check `advanced` to decide "call koto next again"
- Double-call pattern emerges if agent doesn't loop

**Post-fix behavior** (auto-advance only):
- Single call chains through all auto-advanceable states
- `advanced: true` becomes "we did internal work" not "you must act again"
- The field's original purpose (signaling need for re-invocation) is served by `action` field instead:
  - `action: "execute"` = agent must act
  - `action: "done"` = terminal
  - `action: "confirm"` = action requires confirmation
  - `action: "execute"` with blocking_conditions = gates block

**Redesign question**: Should `advanced` be:
1. **Kept as-is**: Minor noise in response (always true for productive calls)
2. **Removed**: Redundant; clients should use `action` to decide
3. **Redesigned**: Change semantics to something clients actually need
   - Transition count?
   - States visited array?
   - Whether final state differs from initial?

## Surprises

1. **`advanced` is never false inside the loop** — it's only set to `false` once at initialization. Every other reference is either a read or a return of the current value. This means the field's trajectory is: false → (optionally set true once or more) → returned in final result.

2. **Evidence is cleared after each auto-advance** (line 336: `current_evidence = BTreeMap::new();`). This means the "evidence epoch" resets; each auto-advanced state starts fresh, and only agent-submitted evidence (if any) carries between invocations.

3. **Gate failure has a special interaction with unconditional fallback** (lines 295-308, 413-417): If a state has gates AND an `accepts` block, gate failure doesn't stop advancement immediately — it falls through to transition resolution with `gate_failed=true`, which then blocks the unconditional fallback. This is a sophisticated interaction not immediately obvious from the `advanced` flag.

4. **The `visited` set doesn't include the starting state** (lines 181-184 comment). This is intentional: the starting state can legitimately be revisited in loops like "review → implement → review". Cycles are only detected on auto-advanced states, not the entry point.

5. **All NextResponse variants carry `advanced`** — even Terminal has it. This means the CLI output contract never elides the field, ensuring clients can always rely on its presence. Compare to other fields like `blocking_conditions` which are absent in some variants.

## Open Questions

1. **Is `advanced: true` on integration responses expected?** 
   - Integration states are usually entry points for external processes, not intermediate auto-advance steps
   - What does "advanced: true" mean when stopped at an integration? "We auto-advanced to here, now invoke the integration"?
   - Or should integrations only be visited at the start (advanced: false)?

2. **Should the field be versioned by scenario?**
   - Per-response-type different semantics (Integrated might mean "reached external process" while EvidenceRequired means "progressed internally")?
   - Or unified definition?

3. **Will agents actually rely on `advanced` post-auto-advance, or will they use `state` comparison instead?**
   - Agents could store initial state and compare: `final_state != initial_state` ≈ `advanced: true`
   - Making `advanced` redundant for client-side logic

4. **If removed, what breaks?**
   - Tests use it for validation (22 assertions)
   - Contracts in docs mention it
   - Skill code (work-on, etc.) might inspect it
   - No search found skill Python/TypeScript code, but absence of grep hits ≠ absence of usage

## Summary

The `advanced` field is set exactly once per invocation (when a transition resolves, line 333) and propagates unchanged through all 11 `AdvanceResult` returns to JSON output. It indicates "at least one transition occurred in this invocation," but post-auto-advance (where most invocations chain through multiple transitions), the field becomes a "not at start" indicator rather than "agent action mattered." The field remains non-vestigial (tests, contracts, and possible external code depend on it), but its signaling value is diluted: clients increasingly rely on the `action` field (execute/done/confirm) to decide what to do next, making `advanced` redundant for that purpose. The redesign question is whether to keep it as noise, remove it as redundant, or redesign it to carry more useful information (like transition count or states visited).

