# Lead: What should the response contract look like post-auto-advance?

## Findings

### Current Response Contract

The `koto next` response contract includes six NextResponse variants:

1. **EvidenceRequired** — state requires evidence submission via `--with-data`
   - Fields: action="execute", state, directive, advanced (bool), expects (schema), error=null
   - Used when: state has `accepts` block and either no evidence provided or gates pass
   - Caller action: Submit `expects.fields` via `--with-data`

2. **GateBlocked** — workflow stopped due to failed command gates
   - Fields: action="execute", state, directive, advanced (bool), blocking_conditions (array), error=null
   - Used when: state has no `accepts` block and gates fail
   - Caller action: Fix gate prerequisites, call `koto next` again (without evidence)

3. **Integration** — integration runner executed and returned output
   - Fields: action="execute", state, directive, advanced (bool), expects (optional), integration {name, output}, error=null
   - Used when: state has `integration` declared and runner succeeded
   - Caller action: Review integration output; if state has `accepts`, submit evidence

4. **IntegrationUnavailable** — integration declared but runner not configured
   - Fields: action="execute", state, directive, advanced (bool), expects (optional), integration {name, available=false}, error=null
   - Used when: state has `integration` and no runner is configured
   - Caller action: Skip integration or use evidence fallback if state has `accepts`

5. **ActionRequiresConfirmation** — default action executed but needs user confirmation
   - Fields: action="confirm", state, directive, advanced (bool), action_output {command, exit_code, stdout, stderr}, expects (optional), error=null
   - Used when: state has `default_action` and action ran but marked RequiresConfirmation
   - Caller action: Review output and either submit evidence (if state has `accepts`) or use `--to` to proceed

6. **Terminal** — workflow reached a terminal state
   - Fields: action="done", state, advanced (bool), expects=null, error=null
   - Used when: state.terminal=true
   - Caller action: None; workflow is complete

### Field Semantics Today

The `advanced: bool` field is present in all non-terminal variants and communicates:
- **Current definition (PLAN-koto-cli-output-contract.md)**: "true when an event was appended before dispatching" — i.e., the caller triggered a transition via `--with-data` or `--to`
- **Engine behavior**: The auto-advancement engine sets `advanced=true` when it makes one or more transitions, even if triggered by event submission that then chains through multiple states
- **Semantic overload**: After auto-advancement, `advanced: true` means "some transitions occurred in this call" but doesn't distinguish between:
  - Agent caused it (submitted evidence that triggered the chain)
  - Engine caused it (looped through auto-advanceable states)

### What Callers Actually Need

From the work-on skill (AGENTS.md) and DESIGN-shirabe-work-on-template.md:

1. **"Am I at a state that needs evidence?"**
   - Used by skill to determine if it should invoke an agent
   - Answered by: `expects != null` (EvidenceRequired variant)
   - Answer is unambiguous and doesn't require looking at `advanced`

2. **"Should I fix something or provide input?"**
   - Used to route between gate-failure recovery, evidence submission, or confirmation
   - Answered by: response variant itself (GateBlocked, EvidenceRequired, Integration, etc.)
   - Answer is self-contained in the variant

3. **"Do I need to call `koto next` again?"**
   - This is where the double-call pattern emerges
   - Current workaround: "if `advanced: true`, call again"
   - Underlying question: "Is there more to do, or have we stopped at an actionable state?"
   - Root cause: After auto-advancement, caller doesn't know if the stopping state requires input or if the engine stopped for some other reason (like a gate failure)

### Post-Auto-Advance Scenario

If auto-advancement is extended (per architectural-layer findings):
- Engine continues looping past states with no `accepts` block and no blocking gates
- Stops at states requiring evidence, blocked by gates, having integrations, or terminal
- The `advanced` field still reports "at least one transition occurred in this call"

**The semantic problem persists even after auto-advance is implemented:**
- After `--with-data` submission that triggers a chain through 5 auto-advanceable states and stops at an evidence-requiring state: `advanced=true` doesn't tell the caller if it's now at a state awaiting input or if it's stuck on a gate
- But the variant itself (EvidenceRequired) tells the caller exactly what to do

**Therefore: `advanced` becomes even more vestigial post-auto-advance.**

### Observability Requirements from Issue #89

The acceptance criteria mention: "Response includes indication that advanced phase(s) were passed through (for observability)."

This suggests two possible interpretations:
1. **Intermediate state visibility**: The response should show which states were traversed during auto-advancement (e.g., `["enter", "validate", "setup"]`)
2. **Transition count/chain length**: The response should indicate how many transitions occurred in a single call

Current response provides: `advanced: bool` (binary, no quantification)

### Backward Compatibility Analysis

The NextResponse enum is the public API. Extending it requires:
- Adding new optional fields to existing variants
- NOT removing existing fields (backward compatibility requirement)

Feasible extensions without breaking change:
1. Add `passed_through: Vec<String>` — list of intermediate states traversed
2. Add `transition_count: usize` — number of transitions in this call
3. Add `advanced_by: "agent" | "engine" | "neither"` — disambiguate causality
4. Redesign `advanced` to mean something else (breaking change)

Non-backward-compatible redesigns:
- Removing `advanced` field
- Changing what `advanced` means
- Redefining variant presence rules

### What the Minimal Contract Should Be

If auto-advance is implemented, the response contract needs to communicate three things:

1. **What state am I in?**
   - Field: `state: String` ✓ (present)
   - This is mandatory and sufficient; callers always need to know where they are

2. **What should I do next?**
   - Field: response variant itself + `expects` (optional schema)
   - The variant (EvidenceRequired, GateBlocked, Terminal, etc.) is self-describing
   - `expects` provides the schema for evidence submission when variant is EvidenceRequired
   - This is sufficient; callers don't need to parse `advanced` to know their next action

3. **What happened on the way here?** (observability)
   - Currently: `advanced: bool` (binary, insufficient detail)
   - Needed: either `passed_through: Vec<String>` (states traversed) or `transition_count: usize` (count)
   - This is informational; doesn't affect what the caller should do next

### Design Decision Point: Observability

The `advanced` field conflates two concerns:
- **Behavioral signal**: "Should the caller call `koto next` again?" → Answer: No, the variant tells them what to do
- **Observability signal**: "How many transitions happened?" → Answer: One boolean is insufficient

**Option A: Replace `advanced` with `transition_count: usize`**
- More honest about what happened
- Backward-compatible if `transition_count: 0` means "no transitions in this call"
- Eliminates ambiguity about who caused the transitions

**Option B: Keep `advanced` as-is and add `passed_through: Vec<String>`**
- Backward-compatible extension
- `advanced` remains unchanged (binary signal)
- New field provides full observability without changing existing semantics
- Callers who don't care about observability ignore the new field

**Option C: Redesign `advanced` to `advancement_metadata: {by: "agent"|"engine"|"neither", count: usize, passed_through: Vec<String>}`**
- Clean semantic model
- Breaking change to the existing field
- Not acceptable unless API stability is being reset

**Option D: Keep as-is (no change to contract)**
- `advanced: bool` remains ambiguous
- Issue #89 observability requirement not met
- Callers still can't distinguish engine-driven advancement from agent-driven advancement
- Skills that care about observability must implement workarounds

### Acceptance Criteria Analysis

Issue #89 states: "Response includes indication that advanced phase(s) were passed through (for observability)."

- **Current contract**: `advanced: bool` — provides no count, no phase names
- **Minimal sufficient change**: Add `transition_count: usize` to all NextResponse variants
  - `transition_count: 0` means "no transitions in this call" (read-only state advancement check)
  - `transition_count: 1+` means "at least N transitions occurred"
  - Satisfies observability requirement without breaking changes
- **Richer option**: Add `passed_through: Vec<String>` (phase names) instead of count
  - Provides both observability and debuggability
  - Allows skills to know exactly which states were traversed
  - Useful for logging, user-facing progress reporting

### What Callers Consume Today

From AGENTS.md execution loop and work-on skill design:
- **Executed by skills**: Parse response variant, handle `expects`, handle `blocking_conditions`
- **Rarely checked**: `advanced: bool` — only used for the double-call workaround
- **Never consumed**: No skill currently uses `advanced` for anything other than "call again"

After auto-advance is implemented:
- **Same checks apply**: Variant tells caller what to do
- **Workaround becomes unnecessary**: Variant presence is sufficient signal
- **`advanced` field becomes purely informational**: Observability-only, not decision-making

### Caller Impact of Each Response Contract Option

| Scenario | Current | With transition_count | With passed_through | With no change |
|----------|---------|----------------------|-------------------|-----------------|
| Skill receives EvidenceRequired after auto-advance | Must call again to verify state | Can see count=3, knows engine ran | Can see ["plan", "implement", "check"] | Must call again |
| Logging auto-advance chain | No visibility | Can log "3 auto-advances" | Can log full path | Invisible to caller |
| Debuggable event log | Events recorded separately | Response shows summary | Response shows summary | Must read event log |
| Breaking change? | N/A | No (backward compatible) | No (backward compatible) | No change |

### Implications for Future Extensibility

Whichever contract is chosen, it should accommodate future needs:
- Per-transition metadata (which condition triggered each jump)
- Reason for stop (gate failure, evidence required, terminal, etc.) — partially addressed by variant
- Directive-relevant state at each step (gates that failed at each step)

The `passed_through: Vec<String>` approach is more extensible than `transition_count: usize` because it preserves state names for later enrichment with per-state metadata.

## Implications

### For Issue #89 Implementation

The accepted solution (extend auto-advancement in the engine per architectural-layer findings) is compatible with any response contract choice. The engine change is independent of the response contract change.

**Recommended combination**:
1. Implement auto-advance in the engine (extend loop condition)
2. Extend response contract with `transition_count: usize` or `passed_through: Vec<String>`
3. Deprecate reliance on the `advanced` field as a signal for "call again"

The response variants themselves are sufficient for callers to determine next steps. The `advanced` field should be deprecated in favor of explicit transition metadata.

### For Caller Code

Post-auto-advance, skills and library consumers should:
1. Use response variant to determine next action (already doing this)
2. Use `expects` to validate evidence input (already doing this)
3. Stop checking `advanced` flag as a decision point
4. Optionally use `transition_count` or `passed_through` for observability (new capability)

The double-call workaround becomes unnecessary because:
- The response variant tells the caller what to do
- No ambiguity about where the engine stopped
- The contract is self-describing

### For Template Design

Templates continue to work unchanged because:
- Auto-advancement logic is entirely in the engine
- Template structure (states, accepts, transitions, gates) is not affected
- Per-state directives remain relevant (whether reached via agent input or auto-advancement)

However, templates that care about observability can be enhanced to provide different directives based on known auto-advancement paths (future enhancement, not required).

### For Library API

The public `dispatch_next` function and NextResponse enum should:
- Keep `advanced: bool` field for backward compatibility
- Add optional observability fields (`transition_count` or `passed_through`)
- Document that `advanced` is legacy; use response variant instead

Breaking API version (if ever desirable):
- Replace `advanced: bool` with rich metadata object
- Remove ambiguity entirely

## Surprises

1. **The `advanced` field solves no caller problem that the response variant doesn't already solve.** Every NextResponse variant is self-describing about what the caller should do. The double-call workaround exists not because `advanced` is useful, but because callers need to know where they ended up after auto-advancement. The variant answers that question.

2. **Observability and behavioral signaling are conflated in one boolean field.** The field tries to communicate two things (did state change in this call? should the caller call again?) but the variant already answers the second question perfectly. Only the first (observability) lacks detail.

3. **Auto-advance implementation doesn't require response contract changes.** The engine can be extended to loop longer without changing the response format. But the absence of observability metadata (transition count or paths) means Issue #89's acceptance criteria ("indication that advanced phase(s) were passed through") isn't fully met by just extending auto-advance.

4. **Skills only use `advanced` as a workaround, not as a design feature.** AGENTS.md, DESIGN-shirabe-work-on-template.md, and the work-on skill all treat the double-call as mechanical overhead, not as intentional observability. This suggests the original design intent is no longer relevant.

## Open Questions

1. **Should `advanced` be deprecated entirely, or kept for backward compatibility?** If kept, should the documentation update to clarify it's legacy/informational?

2. **Is observability (transition count or paths) in the response contract required, or is it acceptable for callers to derive this from the event log?** The event log provides full auditability; the response is just an optimization.

3. **Should the `passed_through` field include the starting state?** E.g., if agent at state A submits evidence that triggers transitions A→B→C→D (stop because D needs evidence), should `passed_through` be ["B", "C", "D"] (intermediate) or ["A", "B", "C", "D"] (inclusive)?

4. **If `passed_through` is added, should it be populated even when `transition_count=0`?** (Answer: Yes, it would be an empty array, maintaining schema consistency.)

5. **Does the response contract need to distinguish between "stopped because evidence is required" vs "stopped because gates block further advancement"?** The variant already does this, but should there be explicit metadata? (Partially addressed by `stop_reason` in the engine, but not exposed in the CLI response.)

## Summary

The current response contract's `advanced` field is ambiguous and serves no behavioral purpose post-auto-advance — the response variants are self-describing about what the caller should do. To meet Issue #89's observability requirement while maintaining backward compatibility, the contract should be extended with either `transition_count: usize` (counting) or `passed_through: Vec<String>` (detailed), not by redefining `advanced`. The implementation of auto-advance in the engine is independent of response contract changes and can proceed first; response contract design can follow. Callers should stop relying on `advanced` as a decision signal and instead use the response variant and `expects` field, which provide all the information needed to determine next steps.
