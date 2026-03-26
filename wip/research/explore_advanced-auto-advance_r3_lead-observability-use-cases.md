# Lead: What would callers actually do with observability metadata from auto-advanced transitions?

## Findings

### 1. Current Caller Behavior: Work-on Skill Analysis

**What the work-on skill does with `koto next` responses:**

- Parses the `action` field to determine workflow state (execute vs done)
- When `action: "execute"` and `advanced: true`: calls `koto next` again (mechanical retry)
- When `action: "execute"` and `expects != null`: submits evidence via `--with-data`
- When `action: "execute"` and `blocking_conditions != null`: logs blocking condition names, then waits for manual intervention
- When `action: "done"`: reports workflow completion

**Zero inspection of advanced metadata:**
- No logging of transition details
- No user-facing display of which states were traversed
- No counting of transitions for metrics
- No use of `advanced` field beyond the binary check "should I call koto next again?"

**What gets logged by work-on skill:**
- Workflow name and task description
- Current state name (from response.state)
- Phase milestone (e.g., "analysis", "implementation", "finalization")
- Error conditions and blocking details
- Completion status
- **Never**: what happened during auto-advancement, which intermediate states were visited, how many transitions occurred

**Implication**: If metadata were added to the response, the work-on skill would not consume it. The skill would continue treating auto-advancement as a mechanical implementation detail with no observability value from its perspective.

---

### 2. Koto's Existing Observability Architecture

**Response-level (from `koto next`):**
- `state: String` — current state name
- `directive: String` — natural language instruction for current state
- `action: "execute" | "done"` — workflow status
- `advanced: bool` — at least one transition occurred in this call
- `expects: ExpectsSchema | null` — evidence schema for current state (if applicable)
- `blocking_conditions: BlockingCondition[] | null` — gates that failed (if applicable)

**No dedicated `status` or `query` command** — koto lacks a snapshot command that replays the event log and returns computed state.

**Event-log level (state file JSONL format):**
Each `transitioned` event records:
- `from: String | null` — previous state
- `to: String` — new state
- `condition_type: String` — "conditional", "unconditional", or "auto"
- `timestamp: String` — RFC 3339 UTC
- `seq: u64` — monotonic sequence number

**What this means for observability:**
- Full audit trail is in the event log (which state→which state, when)
- Response tells caller the current position and what to do next
- Response does NOT tell caller the path taken to get here (except `advanced: bool` says "at least one transition")

---

### 3. Concrete Use Case Analysis

#### Scenario 1: Agent Debugging ("Why am I in state X?")

**Caller question**: "I submitted evidence that triggered the workflow. What states did it visit before stopping?"

**Current tools:**
- Response: current `state`, `directive`, and `action` tell caller where they are
- Event log: full transition history with timestamps and condition types

**What response-level metadata (`passed_through` or `transition_count`) would provide:**
- `transition_count: 3` — caller knows 3 transitions happened
- `passed_through: ["setup", "validate", "check"]` — caller sees exact path

**Would caller use it?**
- Debugging scenario: Maybe read it briefly, but would immediately read event log for full detail (timestamps, condition types, evidence values)
- Response is a summary; event log is the source of truth
- **Verdict: Low value in response; caller goes to event log anyway**

---

#### Scenario 2: Progress Reporting ("Show user which phases completed automatically")

**Caller question**: "I want to display to the end user: 'We auto-completed phases X, Y, Z. Now awaiting your input on phase A.'"

**Current tools:**
- Work-on skill doesn't expose intermediate state names to user
- Skill says "analyzing" or "implementing" (these are directives, not state names)
- Event log has state names and timestamps

**What response-level metadata would provide:**
- `passed_through: ["prep", "validate", "setup"]` — caller could format as "Completed: prep, validate, setup. Next: implementation"

**Would caller use it?**
- The work-on skill doesn't do this today
- A generic UI layer (not skill) could use it
- Skill-specific templates would more likely show directive text ("Phase X completed") than state names
- **Verdict: Useful for generic progress UI, but no skill consumer exists**

---

#### Scenario 3: Audit/Compliance ("Record which gates passed during auto-advancement")

**Caller question**: "Which automated gates were evaluated and passed as we moved through states?"

**Current tools:**
- Event log contains only `transitioned` events for passes (no explicit `gate_passed` events)
- `blocking_conditions` in response tells caller which gates FAILED at current state
- No way to see which gates passed

**What response-level metadata would provide:**
- `passed_through: ["setup", "validate"]` doesn't help (doesn't record gate outcomes)
- Would need richer metadata like `passed_through: [{state: "setup", gates_passed: ["lint", "type_check"]}, ...]`

**Would caller use it?**
- Depends on template design (if template cares about gate audit trail, template author knows to log it elsewhere)
- Issue #89 doesn't mention gate audit as a requirement
- **Verdict: Not addressed by simple `passed_through` or `transition_count`; would need richer structure**

---

#### Scenario 4: Performance/Metrics ("How many transitions did this call make?")

**Caller question**: "Is the workflow getting stuck in loops? Did this call complete quickly or did it chain through many states?"

**Current tools:**
- Response has `advanced: bool` (binary, no count)
- Event log can be counted post-hoc

**What response-level metadata would provide:**
- `transition_count: 1` — one transition, clean
- `transition_count: 15` — either looping or deep chain

**Would caller use it?**
- Metrics/monitoring system could track `transition_count` per call
- Operator could set alert if `transition_count > 5` (workflow is looping)
- **Verdict: Useful for monitoring, but no skill consumer exists**

---

### 4. Caller Type Analysis: Does Answer Differ?

**Skill (work-on):**
- Uses: `action`, `state`, `directive`, `expects`, `blocking_conditions`
- Ignores: `advanced` (except for retry decision)
- Would use additional metadata: **No** — treat auto-advancement as opaque implementation detail

**Library consumer (hypothetical future library API user):**
- Would use: `action`, `state`, `expects` for routing; event log for full audit
- Would use additional metadata: Maybe `transition_count` for metrics/monitoring

**Human debugger (examining response JSON):**
- Would use: `state`, `advanced`, event log
- Would use additional metadata: Yes, `passed_through` is more readable than event log parsing

**Caller category summary:**
| Caller Type | Would Use `passed_through` | Would Use `transition_count` | Why |
|-------------|:---:|:---:|---|
| Skill (work-on) | No | No | Doesn't care about intermediate states |
| Library consumer | Unlikely | Maybe | Would go to event log for authority |
| Human debugger | Yes | Yes | Quicker to read than JSONL |

---

### 5. Response-Level Metadata vs. Event Log Authority

**Critical insight**: The response is derived data; the event log is the source of truth.

**Caller scenarios where response metadata might be preferred:**
1. Operator writing a quick shell script: `koto next wf | jq '.passed_through | length'` (count transitions)
2. Monitoring dashboard: poll `transition_count` without parsing event log
3. Human debugging: quick visual scan of "which states did we visit?"

**Caller scenarios where caller goes to event log anyway:**
1. Audit ("which gates passed?"): event log has condition_type and gate details
2. Evidence inspection ("what data was submitted?"): event log has evidence_submitted payload
3. Performance analysis ("were gates timing out?"): event log has gate result details

**Conclusion**: Response-level metadata solves shallow observability (counting, progress reporting). Deep observability (why did a gate fail, what evidence was submitted) requires the event log.

---

### 6. Issue #89 Acceptance Criterion Analysis

**Criterion**: "Response includes indication that advanced phase(s) were passed through (for observability)."

**Interpretation**: The response should show which states were traversed during auto-advancement.

**Current state**: `advanced: bool` provides no indication of which states were passed through.

**Options to satisfy criterion:**
1. Add `transition_count: usize` — minimal, answers "how many?"
2. Add `passed_through: Vec<String>` — explicit, answers "which states?"
3. No change — criterion not met

**From workflow design perspective:**
- "phases" in Issue #89 likely means: states that the workflow designer marked as auto-advanceable (i.e., states with no `accepts` block and unconditional transitions)
- `passed_through` list would show exactly which phases were traversed
- More useful than `transition_count` for understanding workflow progress

---

### 7. Surprises

**Surprise 1**: The work-on skill (only production consumer) does zero inspection of what's auto-advanced. It's a mechanical retry, treated as an implementation detail of koto, not as observable workflow behavior.

**Surprise 2**: No skill logs auto-advancement details to the user. The "progress" a user sees is phase milestones (analysis, implementation, finalization) — these are `directive` text, not auto-advanced state transitions. Auto-advancement is invisible to end users.

**Surprise 3**: The response-level vs. event-log split is clear: response is for routing decisions and quick observability; event log is for audit and debugging. Trying to fold everything into the response creates duplication without adding value for callers who already have the event log.

**Surprise 4**: None of the three response contract options (`transition_count`, `passed_through`, no change) is required by existing callers. The choice is entirely about future extensibility and operator convenience, not about fixing a blocker.

---

## Implications

### For Issue #89 Implementation

**Decision point**: Choose between `transition_count`, `passed_through`, or neither.

- **`transition_count` alone**: Satisfies criterion minimally. Useful for monitoring/metrics. Does not provide state names.
- **`passed_through` alone**: Satisfies criterion maximally. Provides complete path. More useful for human debugging and future enrichment with per-state metadata.
- **`passed_through + transition_count`**: Redundant. `transition_count` can be derived from `passed_through.len()`.

**Recommendation**: Choose `passed_through: Vec<String>` because:
1. Satisfies Issue #89 criterion more completely
2. Supports future enrichment (per-state gate outcomes, timing, evidence submitted)
3. No additional burden on koto engine (already tracks path internally during advancement)
4. Meaningful to humans; `transition_count` is redundant with array length

---

### For Workflow Design

- Templates don't need to change
- Template authors who want user-facing progress reporting should include directives for each intermediate phase
- Auto-advancement remains invisible in the template definition (determined by engine logic)

---

### For Caller Code

**Skills and library consumers:**
- Continue using response variants for routing (no change)
- Continue using `expects` for evidence validation (no change)
- Stop checking `advanced` as a decision point (already documented as legacy)
- If new metadata is added (e.g., `passed_through`), it's informational only — does not affect routing

**Monitoring/metrics systems:**
- If `transition_count` is added, could alert on workflows with high counts (looping detection)
- If `passed_through` is added, could track which states are most commonly auto-advanced

---

### For the Response Contract

**Backward compatibility achieved by:**
- Adding optional fields, not removing or renaming existing fields
- `advanced: bool` remains unchanged (kept for callers who do check it)
- New field (if chosen) is informational, not behavioral

**No change to variant presence rules or serialization semantics.**

---

## Open Questions

1. **Should `passed_through` include the starting state?** If at state A and transitions A→B→C, is `passed_through: ["B", "C"]` (intermediate) or `["A", "B", "C"]` (inclusive)? Suggest: intermediate (caller already knows starting state).

2. **Should `passed_through` be populated when no transitions occur?** If `transition_count=0`, is `passed_through: []` or omitted? Suggest: always present (schema consistency).

3. **Is there future value in per-state metadata?** E.g., `passed_through: [{state: "B", stopped_by: null}, {state: "C", stopped_by: "evidence_required"}]`? Suggest: defer; `Vec<String>` is extensible via new field.

4. **Who owns the decision on `transition_count` vs. `passed_through`?** Response contract choice is independent of behavioral fix. Suggest: author of response contract PR decides.

---

## Summary

Callers would use observability metadata from auto-advanced transitions for logging, progress reporting, and monitoring — but not for behavioral routing. The work-on skill (only production consumer) does zero inspection of what's auto-advanced. Response-level metadata is useful for shallow observability (what states did we visit?); deep observability (why did gates fail, what evidence was submitted) requires the event log. To meet Issue #89's acceptance criterion "Response includes indication that advanced phase(s) were passed through," add `passed_through: Vec<String>` to the response contract; this satisfies the criterion, supports future enrichment, and maintains backward compatibility. The behavioral fix (auto-advance in the engine) is independent of the response contract change and can proceed first.
