# Lead: passed_through vs transition_count comparison

## Findings

### Data Availability in the Advancement Loop

The engine's `advance_until_stop()` function (src/engine/advance.rs:158-358) maintains two key data structures during auto-advancement:

**In every loop iteration, available at transition point (line 324-334):**
- `state: String` — current state name (before transition)
- `target: String` — destination state name (from transition resolution)
- `transition_count: usize` — count of transitions made in this call (line 177, incremented at 334)
- `visited: HashSet<String>` — set of states already traversed in this invocation (line 174, updated at 331)

**At the moment of appending a Transitioned event (line 324-328):**
```rust
let payload = EventPayload::Transitioned {
    from: Some(state.clone()),
    to: target.clone(),
    condition_type: "auto".to_string(),
};
```
The engine has access to both the from and to state names. It does NOT currently capture the sequence of states into a vector.

**Loop invariants:**
- `transition_count` is incremented once per resolved transition (line 334)
- `visited` contains all intermediate states ever auto-advanced-through in this invocation
- Starting state is NOT added to `visited` (line 181-184 comments explain this)
- Each iteration clears evidence for auto-advanced states (line 336)

**Cost of capturing passed_through:**
- Requires declaring `let mut passed_through: Vec<String> = Vec::new();` at function start (line 177)
- At line 332, after `state = target;`, push: `passed_through.push(target.clone());`
- Pass to AdvanceResult as a new field at line ~180+
- Total overhead: one Vec allocation + N string clones (where N = number of transitions)

**Cost of using transition_count:**
- `transition_count: usize` already exists and is already being tracked (line 177, 197, 334)
- No additional loop overhead
- Already incremented at exactly the right place (after append_event succeeds)
- Zero additional cost

### AdvanceResult Struct Changes

**Current AdvanceResult (src/engine/advance.rs:79-88):**
```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub advanced: bool,
    pub stop_reason: StopReason,
}
```

**Option 1: Add passed_through: Vec<String>**
```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub advanced: bool,
    pub stop_reason: StopReason,
    pub passed_through: Vec<String>,  // NEW: intermediate states traversed
}
```
- Backward compatible: new optional field, deserialization ignores if absent
- Breaking for Rust match patterns if not using `..` syntax (but internal struct, minimal impact)
- Every return statement in `advance_until_stop` must provide the field
- Approximately 18 return statements need updating (lines 189, 198, 216, 227, 237, 273, 300, 316, 339, 346, etc.)

**Option 2: Add transition_count: usize**
```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub advanced: bool,
    pub stop_reason: StopReason,
    pub transition_count: usize,  // NEW: number of transitions in this call
}
```
- Backward compatible: new field, can default to 0 if omitted
- Same match pattern impact as Option 1
- Simpler: just move the existing local `transition_count` variable into the struct

### NextResponse Serialization

**Current NextResponse serialization (src/cli/next_types.rs:138-234):**

All six variants serialize to a custom map. Each uses `serializer.serialize_map(Some(N))?` where N is the field count. Each variant manually calls `map.serialize_entry()` for each field.

Example (EvidenceRequired, lines 141-154):
```rust
NextResponse::EvidenceRequired { state, directive, advanced, expects } => {
    let mut map = serializer.serialize_map(Some(6))?;
    map.serialize_entry("action", "execute")?;
    map.serialize_entry("state", state)?;
    map.serialize_entry("directive", directive)?;
    map.serialize_entry("advanced", advanced)?;
    map.serialize_entry("expects", expects)?;
    map.serialize_entry("error", &None::<()>)?;
    map.end()
}
```

**If passed_through: Vec<String> is added:**

All six non-Terminal variants need updates:
1. EvidenceRequired (lines 141-154): increment `Some(6)` to `Some(7)` or `Some(8)`, add `passed_through` entry
2. GateBlocked (lines 156-170): increment count, add entry
3. Integration (lines 172-187): increment count, add entry
4. IntegrationUnavailable (lines 189-204): increment count, add entry
5. Terminal (lines 206-213): no change (Terminal has no passed_through in the variant)
6. ActionRequiresConfirmation (lines 215-231): increment count, add entry

- Cost: ~6 edits, each adding 1 line to serialize_map and 1 line for serialize_entry
- JSON size impact: Adds field name ("passed_through": 13 chars) + comma + array structure (at least 2 chars for []), plus N strings = variable size
- Example: `"passed_through": ["gather", "validate", "setup"]` = ~44 bytes for 3-state chain

**If transition_count: usize is added:**

Same six variants need updates:
1. Each variant gets increment to map field count
2. Each variant gets one new serialize_entry for transition_count

- Cost: ~6 edits, same as above
- JSON size impact: Adds field name ("transition_count": 16 chars) + comma + 1-5 digit number
- Example: `"transition_count": 3` = ~19 bytes

### NextResponse Struct Changes

**Option 1: Add passed_through to enum variants**

Each of the 6 non-Terminal variants gains a field:
```rust
NextResponse::EvidenceRequired {
    state: String,
    directive: String,
    advanced: bool,
    expects: ExpectsSchema,
    passed_through: Vec<String>,  // NEW
}
```

Changes needed:
- src/cli/next_types.rs lines 24-62: Add field to each variant (5 additions)
- src/cli/next_types.rs with_substituted_directive method (lines 67-135): Update each match arm to carry the field through (5 updates)
- src/cli/next.rs dispatch_next function: doesn't construct NextResponse yet (dispatches only), no change
- src/cli/mod.rs handle_next function: All 6 response constructions get +1 field each (lines ~1150, 1172, 1181, 1189, 1201, 1209, 1215)

**Option 2: Add transition_count to enum variants**

Same pattern, with simpler field type (usize vs Vec<String>):
- Same 5 variant updates
- Same method updates
- Same 6 response construction updates
- Slightly simpler: no need to collect/push strings into a vector

### CLI Integration (handle_next function)

**Current flow (src/cli/mod.rs:620-1280):**

1. Load workflow and template (lines 663-775)
2. Handle --to directed transition (lines 777-854)
3. Handle --with-data evidence submission (lines 871-955)
4. Acquire file lock (lines 957-981)
5. Register signal handlers (lines 983-992)
6. Merge evidence from current epoch (lines 994-1010)
7. Set up I/O closures (lines 1012-1115)
8. Call `advance_until_stop` (lines 1117-1126)
9. Map AdvanceResult to NextResponse (lines 1128-1265)

**For passed_through option:**

At step 9, when consuming `AdvanceResult`:
```rust
Ok(advance_result) => {
    let final_state = &advance_result.final_state;
    let advanced = advance_result.advanced;
    let passed_through = advance_result.passed_through;  // NEW: extract from result
    
    // ... match on stop_reason ...
    
    StopReason::Terminal => NextResponse::Terminal {
        state: final_state.clone(),
        advanced,
    },
    StopReason::EvidenceRequired => NextResponse::EvidenceRequired {
        state: final_state.clone(),
        directive: ...,
        advanced,
        expects: ...,
        passed_through: passed_through.clone(),  // NEW: pass through
    },
    // ... similar for all other variants ...
}
```

Changes needed:
- 1 line: extract `passed_through` from `advance_result`
- 5 lines: add `passed_through` to each non-Terminal variant construction

**For transition_count option:**

Nearly identical:
- 1 line to extract `transition_count`
- 5 lines to pass it through to variants

### Integration Test Impact

**Current test coverage (tests/integration_test.rs):**

22 integration tests check the `advanced` field:
- Lines 331, 1221, 1328, 1437, 1467, 1506, 1591, 1650, 1674, 2545, 2771 (11 explicit assertions)
- Additional tests in match/if chains that indirectly depend on response structure

Tests check patterns like:
```rust
assert_eq!(json["advanced"], true, "auto-advancement should reach terminal state");
```

**For passed_through option:**

Each test that cares about auto-advancement assertions needs updating:
- Remove: old `assert_eq!(json["advanced"], true)` assertions if they're checking the wrong thing
- Add: `assert_eq!(json["passed_through"].as_array().unwrap().len(), 3)` to verify chain length
- Add: `assert_eq!(json["passed_through"][0].as_str(), Some("gather"))` to verify path
- Cost: ~22 new assertions to validate the new field

Most tests won't care (they just need to verify the response is valid JSON), but tests specifically validating auto-advancement behavior will want to check `passed_through` content.

**For transition_count option:**

Similar update pattern:
- Add assertions like `assert_eq!(json["transition_count"], 3)` to verify count
- Cost: ~22 new assertions to validate the new field
- Simpler assertions than passed_through (just comparing numbers, not array contents)

### Contract Complexity Analysis

**Serialization Cost Comparison:**

| Aspect | passed_through | transition_count |
|--------|----------------|-----------------|
| JSON size per response | Variable (13 + N * avg_state_name) | Fixed (16 + 1-5 digits) |
| Example 3-state chain | ~44 bytes | ~19 bytes |
| Example 0-state (no advance) | ~18 bytes (empty array) | ~18 bytes (zero) |
| Field name clarity | High (explicit states) | Medium (requires external mapping) |
| Client parsing | More complex (array) | Simpler (integer) |
| Network efficiency | Worse (strings, array overhead) | Better (single int) |

**Implementation Complexity:**

| Phase | passed_through | transition_count |
|-------|----------------|-----------------|
| Engine changes (AdvanceResult) | High: track Vec, push on each transition | Low: move existing counter |
| Struct updates (NextResponse variants) | Medium: add Vec<String> field to 5 variants | Low: add usize field to 5 variants |
| Serialization updates | Medium: 5 serialize_map calls | Low: 5 serialize_map calls (same logic) |
| CLI integration | Low: 1 extract + 5 pass-through lines | Low: 1 extract + 5 pass-through lines |
| Tests | Medium: complex assertions on arrays | Low: simple numeric assertions |
| Documentation | Medium: explain state names and order | Low: explain count meaning |

### Future Extensibility

**passed_through: Vec<String> extensibility:**

The vector of state names allows future enrichment:
1. **Per-state metadata objects** — upgrade to `Vec<TransitionInfo>` where each contains state name, gates evaluated, evidence required, etc.
   ```rust
   pub struct TransitionInfo {
       pub state: String,
       pub gate_results: Option<BTreeMap<String, GateResult>>,
       pub duration_ms: u64,
   }
   ```
   - Current vector can be extended non-breaking: existing code sees Vec<String>, new code sees Vec<TransitionInfo>
   - State names are preserved for backward compatibility

2. **Action output per state** — if an auto-advancement chain executes default actions, log which actions ran at which states
   - Metadata structure can include action results
   - Useful for debugging slow chains

3. **Gate failure tracking** — which gates blocked advancement at each state (if any)
   - Per-state metadata object includes this

4. **Conditional matching** — which condition at each state triggered the next transition
   - Useful for template authors to understand auto-advancement paths

**transition_count: usize extensibility:**

The count alone supports only quantitative queries:
1. **"How many transitions happened?"** — Answered directly
2. **"Did any auto-advancement occur?"** — Answered by count > 0

To answer qualitative questions (which states, why did they trigger), callers must:
- Read the full event log (out-of-band)
- Infer chain from final_state if initial_state is known (error-prone)

Extending transition_count to richer data requires:
- Breaking change to the field type (from usize to object)
- OR adding parallel fields (transition_count + transition_metadata, clutters response)

### Request Contract Stability

**Backward compatibility analysis:**

Both options are backward compatible when **adding** to the response:

1. **Existing callers ignore new fields** — JSON parsers skip unknown fields, so both passed_through and transition_count can be added without breaking existing code

2. **Serialization format** — Both serialize as new fields in the response object; no existing field is removed or renamed

3. **NextResponse Serialize trait** — Both use the same pattern (serialize_map with explicit field count), so no trait breaking changes

4. **API consumers** — Code matching on NextResponse variants must handle the new fields:
   - Rust callers: if using `..` pattern, no changes; if enumerating fields, compile error (forces update)
   - JSON consumers: ignore new field, proceed as before

**Deprecation risk:**

If the `advanced: bool` field is later removed (hypothetically):
- Callers can check `transition_count > 0` as a replacement for `advanced`
- But `passed_through` is clearer (explicit state names, more semantic)

### Observability Semantics

**passed_through: Vec<String>:**
- **Meaning**: "You traveled through these states in order"
- **Use cases**: 
  - Skill logs: "Agent progressed through [gather, analyze, implement, review]"
  - Debugging: Compare expected path vs actual path
  - User-facing progress: "Completed stages: gather → analyze → implement"
- **Limitations**: 
  - Doesn't explain WHY each state was reached (no gate/condition info)
  - Doesn't include initial state (empty if no transitions)
  - Doesn't indicate time spent in each state

**transition_count: usize:**
- **Meaning**: "N transitions occurred in this call"
- **Use cases**:
  - Metric collection: "Average transitions per call"
  - Throttling: "If transition_count > 10, probably a bug in template"
  - Simple binary check: "Any advancement at all?"
- **Limitations**:
  - Doesn't indicate which states (requires reading event log)
  - Doesn't explain causality (agent-triggered vs engine-triggered)
  - Opaque for user-facing visibility (can't say "completed these stages")

### Data Availability at Each Point in the Call

**In advance_until_stop loop (inside engine):**
- ✓ Both state names available (from, to)
- ✓ Transition count available (incremented)
- ✓ Total loop iterations known when exiting
- ✓ Cycle detection data available (visited set)
- ✗ Gate results not propagated to response (only in StopReason)
- ✗ Action execution details not captured (only appended to event log)

**At AdvanceResult construction (end of loop):**
- ✓ Final state name available
- ✓ Advanced boolean available
- ✓ Stop reason available
- ✓ Transition count available (if we move it to struct)
- ? State sequence available only if explicitly collected in Vec
- ✗ Intermediate gate results not accessible
- ✗ Action outputs not accessible

**In handle_next (CLI layer):**
- ✓ Full AdvanceResult available
- ✓ Original initial state known
- ✓ Event log available (could reconstruct full path)
- ✓ Template available (could derive per-state metadata)
- ? Would need to re-query engine or look up states in template for enrichment

**Architectural implication:**
- `transition_count` requires no additional data capture beyond existing loop counters
- `passed_through` requires new data collection (Vec<String> push on each transition) — minimal cost
- Both are immediately available at response construction; neither requires additional queries

### Contract Clarity for Callers

**After receiving EvidenceRequired response with passed_through:**
```json
{
  "action": "execute",
  "state": "implement",
  "directive": "Implement the feature.",
  "advanced": true,
  "passed_through": ["gather", "analyze", "setup", "implement"],
  "expects": { ... },
  "error": null
}
```
Caller can immediately infer:
- "The engine auto-advanced through [gather, analyze, setup] to reach implement"
- "The engine stopped because implement needs evidence"
- "The chain was 4 states long"
- "The initial state wasn't gathered (different from current state after chain)"

**After receiving EvidenceRequired response with transition_count:**
```json
{
  "action": "execute",
  "state": "implement",
  "directive": "Implement the feature.",
  "advanced": true,
  "transition_count": 3,
  "expects": { ... },
  "error": null
}
```
Caller can immediately infer:
- "3 transitions occurred"
- "We ended up at implement"
- ??? "Which states were traversed?" (Must read event log or re-call with different flags)

## Implications

### For Issue #89 Implementation

**Acceptance criterion**: "Response includes indication that advanced phase(s) were passed through (for observability)."

**Interpretation:**
- "passed through" strongly suggests intermediate states (qualitative)
- "phase(s)" (plural) suggests multiple transitions
- "observability" suggests external visibility for logging/debugging

**passed_through: Vec<String>** directly satisfies all three aspects:
- ✓ Shows which phases (states) were passed through
- ✓ List format makes the plurality explicit
- ✓ Qualitative observability (can name the stages in logs)

**transition_count: usize** partially satisfies:
- ~ Shows that multiple transitions happened (but not which)
- ✓ Count format makes plurality explicit (count > 1)
- ~ Quantitative observability only (can't name stages without external lookup)

### For Caller Code

**Skills using koto responses (work-on skill, AGENTS.md pattern):**

Currently check: response variant (EvidenceRequired? GateBlocked? Terminal?)

With passed_through:
- Can log: `"Progressed through [gather, analyze] before stopping"`
- Can drive per-stage callbacks in agents
- Can tell user "you're halfway done with the workflow"

With transition_count:
- Can log: `"Made 2 transitions before stopping"`
- Requires out-of-band lookup of state names (event log)
- Generic counter, not workflow-specific

### For Debugging

**Scenario: Auto-advancement chain produces unexpected result**

With passed_through:
```
koto next workflow-name
→ response.passed_through = ["gather", "analyze", "bad-state"]
→ Skill logs: "Chain unexpectedly reached bad-state"
→ Template author checks transition from analyze → bad-state
→ Finds bug quickly
```

With transition_count:
```
koto next workflow-name
→ response.transition_count = 3
→ Skill logs: "3 transitions occurred"
→ Template author must read event log or re-run with verbose flags
→ Finds bug more slowly
```

### For Metrics and Monitoring

**Scenario: SLA tracking auto-advancement performance**

With transition_count:
- "Average transition_count per call" — useful metric
- "P99 transition_count" — useful SLA boundary
- Simple to aggregate

With passed_through:
- "Average chain length" — same as transition_count
- "Most common paths" — additional insight passed_through enables
- More complex aggregation (string matching, etc.)

### For API Stability

**If passed_through is added:**
- Future upgrade to `Vec<TransitionDetail>` (object in array) — non-breaking
- Existing code seeing Vec<String> still works
- Future code seeing more fields in objects is forward-compatible

**If transition_count is added:**
- Future upgrade to `transition_metadata: object` — breaking change
- Must either deprecate transition_count (messy) or add both fields (bloat)
- Parallel fields (transition_count + transition_details) less clean

## Surprises

1. **transition_count requires ZERO additional data capture** in the engine loop, since `transition_count` is already being tracked as a local variable. Moving it from stack to struct is literally just changing where it lives; no new instrumentation needed. This is a major implementation win over passed_through.

2. **passed_through is more extensible than appeared** — by preserving state names in a sequence, future metadata enrichment becomes non-breaking (can extend from Vec<String> to Vec<TransitionInfo>). transition_count offers no such path; if richer data is needed later, it requires a breaking change or parallel fields.

3. **Neither field answers the question callers actually stopped asking** — "Should I call koto next again?" The response variant (EvidenceRequired, GateBlocked, Terminal) already answers this completely. The only reason for passed_through/transition_count is pure observability, not behavioral signaling. This was noted in R2 research but bears repeating: the `advanced: bool` field conflates two concerns, and neither alternative option addresses this conflation directly.

4. **JSON size difference is substantial for high-chain-length workflows** — A 10-state chain with average 20-char state names: passed_through ≈ 250+ bytes vs transition_count ≈ 22 bytes. For API-driven workflows with many auto-advanced calls, this compounds. transition_count wins on wire efficiency by ~10x.

5. **Caller code complexity inversion** — With passed_through, the caller can immediately log human-readable paths. With transition_count, the caller must either (a) store more state (track which states are being entered), (b) read the event log out-of-band, or (c) ignore the metric entirely. This suggests passed_through is more useful for actual skill implementations, despite transition_count being cheaper to compute.

6. **The distinction between "agent caused it" and "engine caused it" is still unresolved** — Both options track how many/which states were reached, but neither explains causality. An agent submitting evidence that triggers a 5-state chain looks identical to the engine auto-discovering a 5-state chain in terms of passed_through/transition_count. If #89 later requires distinguishing these, another field (or redesign) will be needed anyway.

## Open Questions

1. **Should passed_through include the starting state?** If the agent submits evidence at state A and the chain goes A→B→C→D (stop), is passed_through = ["B", "C", "D"] (intermediate only) or ["A", "B", "C", "D"] (inclusive)? This affects readability and semantics.

2. **How should transition_count relate to the existing `advanced: bool`?** Currently `advanced = true` means "at least one transition occurred." Should `transition_count > 0` mean the same thing, or should there be a distinction (e.g., agent-initiated transitions vs auto-advanced)?

3. **Should either field be present in Terminal responses?** Currently Terminal only includes [state, advanced, error]. If passed_through or transition_count is added to all variants, should Terminal also get it (showing the full path to terminal), or should it remain minimal?

4. **For metric purposes, should there be a corresponding field in error responses?** If an advancement loop errors out, should the error response include how many transitions succeeded before the error (for debugging)? Neither option currently addresses this.

5. **What's the intended audience for the observability signal?** Skills/agents (need human-readable paths for logging)? SREs/monitoring (need counts for metrics)? Template authors (need full paths for debugging)? The answer affects which option is better.

6. **Should advanced: bool be deprecated if passed_through or transition_count is added?** If not deprecated, we have three ways to say "did transitions happen": `advanced`, `passed_through.len() > 0`, and `transition_count > 0`. Is that acceptable technical debt?

## Summary

**Implementation cost:** transition_count is dramatically cheaper (zero new data collection, uses existing counter), while passed_through requires Vec allocation and N string clones per transition chain. **Caller observability:** passed_through provides explicit state names enabling human-readable logging and debugging, while transition_count requires out-of-band lookup for equivalent detail. **Future extensibility:** passed_through preserves state names for non-breaking enrichment to per-state metadata objects, while transition_count has no upgrade path that doesn't break the contract or require parallel fields. **Contract satisfaction:** passed_through directly satisfies the acceptance criterion ("indication that advanced phase(s) were passed through"), while transition_count satisfies it more narrowly (counts phases but doesn't name them). The choice hinges on whether observability should prioritize implementation efficiency (transition_count wins) or caller usability and API longevity (passed_through wins).
