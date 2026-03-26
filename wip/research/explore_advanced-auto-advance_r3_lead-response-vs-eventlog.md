# Lead: Response-level observability vs event log queries

## Findings

### How koto Currently Surfaces Observability

**Status quo (from DESIGN-koto-cli-output-contract.md):**
- `koto next` returns the CURRENT state and a directive
- The `advanced: bool` field communicates "at least one transition happened"
- No intermediate state visibility
- Full event history is available by reading `koto-<name>.state.jsonl` directly (raw JSONL)

**What exists but isn't yet exposed:**
- The JSONL event log (`koto-<name>.state.jsonl`) contains EVERYTHING: every transition, evidence submission, gate evaluation, timestamp, sequence number
- Each event has `seq`, `timestamp`, `type`, and a type-specific `payload`
- State derivation rule: current state is the `to` field of the last state-changing event (`transitioned`, `directed_transition`, or `rewound`)
- Evidence scoping: `evidence_submitted` events after the most recent state-changing event to the current state

**Planned but deferred (DESIGN-event-log-format.md, Decision 5):**
- `koto state` — a snapshot command that replays the log and returns computed JSON (current state, active evidence, full history, variables)
- Distinguished from `koto log` (which would dump raw JSONL)
- Deferred to post-#49 because it depends on response contract and auto-advancement decisions

### Current Ergonomics: Getting Observability After `koto next`

**Scenario: A skill wants to know "which states did I pass through?"**

**Option A: Response-level metadata (proposed in Issue #89)**
```bash
koto next my-workflow --with-data '{"decision": "proceed"}'
# Response includes: advanced=true, passed_through=["plan", "validate", "setup"]
# Caller extracts from one response
# Cost: ONE call, immediate, response is small
```

**Option B: Event log query (current)**
```bash
koto next my-workflow --with-data '{"decision": "proceed"}'
# Caller reads koto-my-workflow.state.jsonl directly
# Caller parses JSONL, filters for transitioned events after the last known seq
# Caller derives passed_through = [from/to pairs]
# Cost: ONE call + filesystem I/O + JSON parsing + sequence number bookkeeping
```

**Option C: Two-call pattern with planned `koto state` (future)**
```bash
koto next my-workflow --with-data '{"decision": "proceed"}'  # First call
koto state my-workflow  # Second call (proposed, deferred to #49+)
# Second call replays log, returns full snapshot
# Caller extracts state history from response
# Cost: TWO calls, replay latency, response is large (includes evidence, variables, etc.)
```

### Comparison of Ergonomics

**For a skill that wants transition metadata (which states did I pass through?):**

| Approach | Calls | Parsing | Availability | Response Size | Observability Detail |
|----------|-------|---------|--------------|---------------|----------------------|
| Response metadata | 1 | Parse JSON | Immediately | Small (array of state names) | Just state names (not why) |
| Event log query | 1 | Parse JSONL + filter by seq | Immediately | Large (full history) | Full history (can derive anything) |
| `koto state` snapshot | 2 | Parse JSON | After state change | Very large (state + evidence + history) | Full computed view |

**For a skill that wants gate failure details:**

| Approach | How it works | Availability |
|----------|--------------|--------------|
| Response metadata | Could add `blocking_conditions` detail per state (but response would grow) | Immediate |
| Event log query | Must parse event sequence and find failed gate + its output | Immediate (if stored) |
| `koto state` snapshot | Would need to compute gate history (complex replay logic) | After call |

**For a CI/CD pipeline that needs "how many transitions happened in this chain?":**

| Approach | Answer |
|----------|--------|
| Response metadata | `passed_through.len()` = exact count |
| Event log query | Count `transitioned` events after last known seq | Count via filtering |
| `koto state` snapshot | Parse history array | Replay-derived |

### The "Two-Call Irony" Unpacked

**Issue #89 acceptance criteria:** "Response includes indication that advanced phase(s) were passed through (for observability)."

**The irony:** If we add response-level observability (e.g., `passed_through: Vec<String>`), we eliminate the reason for the double-call pattern that Issue #89 is supposed to solve.

**Current double-call:**
```bash
koto next my-workflow  # Call 1: "Am I at a blocking state?"
# Response: advanced=true, state="verify" (but skill doesn't know if there was a chain)
koto next my-workflow  # Call 2: Workaround to "verify we landed at an actionable state"
# Response: state="verify", expects={...decision field...}
```

**With response-level observability:**
```bash
koto next my-workflow  # Single call
# Response: advanced=true, passed_through=["plan", "validate", "verify"], state="verify", expects={...}
# Skill sees: "I went through 3 states, landed at verify which needs evidence"
# No need for call 2
```

**The problem was not observability — it was ambiguity.** The `advanced` field told the skill "something happened" but not what, so the skill had to call again to verify it landed at an actionable state. The variant itself (EvidenceRequired) answers that question.

### Why Event Log Should Remain Separate

**1. Concern separation (git's model):**
- `git status` (fast, behavioral signal) ≠ `git log` (deep, observational)
- koto's `next` (behavioral: what should the caller do?) ≠ koto's `log`/`state` (observational: what happened?)

**2. Response bloat:**
- If every `koto next` response includes full history, response grows linearly with workflow depth
- A 50-state workflow's 20th call would include 50+ entries in the response
- For agents over high-latency links (cloud), this is measurable overhead

**3. Access pattern mismatch:**
- During `koto next` execution: caller needs current state + directive + next action → FAST, SMALL response
- During observability/debugging: caller needs full history + evidence + gate details → DEEP, OPTIONAL query

**4. The event log is the source of truth:**
- Response metadata is a SUMMARY/PROJECTION derived from the log
- Deriving two different summaries (one for response, one for a query) risks inconsistency
- If there's a bug in the summary logic, which one is right? The log.

### How Work-On Skill Currently Handles This

From AGENTS.md and DESIGN-shirabe-work-on-template.md:

**Current pattern:**
```bash
result=$(koto next my-workflow)
action=$(echo "$result" | jq -r '.action')
if [ "$action" = "execute" ]; then
  # Execute directive
  # ...
  result=$(koto next my-workflow --with-data '...')  # Workaround double-call
fi
```

**Why the double-call exists:**
- Skill needs to know if there's more work to do after `--with-data` submission
- The response after evidence submission says `advanced=true`, but the skill can't tell if it landed at another evidence-requiring state or a gate-blocked state
- So it calls again to be sure

**Post-auto-advance, the response variants solve this:**
```bash
result=$(koto next my-workflow --with-data '...')
action=$(echo "$result" | jq -r '.action')
if [ "$action" = "execute" ]; then
  # landed at another actionable state; handle it
elif [ "$action" = "done" ]; then
  # terminal; stop
fi
# No double-call needed; the variant is self-describing
```

**The skill doesn't care about `passed_through` for routing.** It only cares: "what do I do next?" The response variant answers that.

### Observability Use Cases: Do They Actually Require Response-Level Data?

**Use case 1: "Show the user which states were traversed"**
- Source: Event log is perfect (has timestamps, condition_type, from/to per transition)
- Response summary: Could include `passed_through: Vec<String>`, but why not call `koto state`/`koto log`?
- Conclusion: Event log query is the right tool

**Use case 2: "Alert if more than N auto-advances in one call" (safety check)**
- Source: Response could include `transition_count: usize`
- Alternative: Caller tracks sequence numbers across calls, detects count from log deltas
- Conclusion: Response metadata OR event log query both work; event log is more detailed (shows which states)

**Use case 3: "Debug: why did the engine stop at this state?"**
- Source: Response variant (EvidenceRequired, GateBlocked, Terminal) + response fields (blocking_conditions if present)
- Alternative: Event log shows full chain, reason for stop is the absence of next transition
- Conclusion: Response variant IS the right tool; event log provides deep context

**Use case 4: "Audit: which evidence was submitted and when?"**
- Source: Event log has `evidence_submitted` events with timestamp, fields, state
- Response: Could never include this (too large, already committed to log)
- Conclusion: Event log query is the only tool

**Use case 5: "Progress reporting: display to user as 'State A → State B → State C (current)'"**
- Source: Could come from response `passed_through` OR event log
- Response metadata: Lightweight, user-facing summary
- Event log: Full data for rich formatting (timestamps, condition_type, etc.)
- Conclusion: Both could work; response is lighter, event log is richer. Choice depends on use case.

### Should Response Be Lean (Behavioral) or Rich (Observational)?

**Argument for lean response (behavioral only):**
- Response is for deciding "what do I do next?" — the variant answers this
- Observability is a separate concern with a different query interface
- Mirrors git's split: status (behavioral) vs log (observational)
- Keeps response fast and parseable
- Event log is the canonical source; response shouldn't duplicate it

**Argument for rich response (observational + behavioral):**
- One call instead of two for simple observability
- Agents often want both pieces of info (next action + how we got here) in one go
- Response is already JSON; adding fields is backward-compatible
- Avoids requiring a second command (`koto state`/`koto log`)

**The deciding factor:**
The response is sent to agents (skills) EVERY TIME koto next is called. The event log is queried SOMETIMES (when debugging, auditing, or building dashboards).

If we add observability to the response:
- Every single call includes it (even when agent doesn't care)
- Response grows linearly with workflow depth
- Agents that don't need it pay the cost (latency, parsing)

If observability stays in the event log:
- Agents that care explicitly query it
- Log query can be sophisticated (filtering, aggregation) without bloating responses
- Agents that don't care never pay the cost

### The Right Abstraction Boundary

**Response is for behavioral signals:**
- Current state (mandatory)
- What to do next (variant + expects/blocking_conditions)
- Whether state changed (advanced: bool, or transition_count: usize for better semantics)

**Event log is for observational queries:**
- Which states were visited
- When each transition happened
- What evidence was submitted
- Which gates failed and why
- Full audit trail for any purpose

**Separation preserves:**
- Clean responsibility: response = "what should I do?" ; log = "what happened?"
- Performance: response stays small, log is queried on demand
- Correctness: event log is source of truth, response is a summary that can be re-derived
- Extensibility: new observability needs don't bloat every response

### How Other CLI Observability Tools Handle This

**kubectl (Kubernetes CLI):**
- `kubectl get pod` (current state, behavioral)
- `kubectl logs pod` (full history/output, observational)
- `kubectl describe pod` (richer summary, but separate command)

**docker:**
- `docker ps` (current state)
- `docker logs container` (full log, separate)
- `docker inspect container` (detailed state, separate)

**aws/gcloud:**
- `aws s3 ls` (current state)
- `aws s3api head-object` (metadata, behavioral)
- CloudWatch Logs (full audit trail, separate service)

**Pattern: Observability is a separate concern with a separate query interface.**

## Implications

### For Issue #89 Implementation

**Recommendation: Implement auto-advance in the engine; keep response lean.**

The engine loop (DESIGN-auto-advancement-engine.md) should be extended to continue advancing through states until hitting a stopping condition. This solves the double-call problem because the response variant is self-describing.

For observability (the stated acceptance criterion: "indication that advanced phase(s) were passed through"), implement it as:

1. **Short-term (in this issue):** Add `transition_count: usize` to all NextResponse variants
   - Backward-compatible (new field)
   - Answers "how many states?" without bloat
   - Lightweight summary

2. **Medium-term (post-#49):** Implement `koto state` command (already deferred in DESIGN-event-log-format.md)
   - Replays the log
   - Returns full snapshot (current state, history, evidence, variables)
   - Callers that need rich observability can query it separately

3. **Avoid:** Adding full `passed_through: Vec<String>` or gate failure details to every response
   - These are large summaries that belong in an optional query, not every response
   - Event log already has them with more detail

### For Skills and Agents

Post-auto-advance:
- **Stop using:** The `advanced` field as a decision signal ("should I call again?")
  - The response variant answers that question
- **Start using:** The response variant to determine next action
  - EvidenceRequired → submit evidence
  - GateBlocked → fix prerequisites, call again
  - Terminal → done
- **Optional:** Query `koto state` or read the event log for detailed observability
  - Not required for normal operation
  - Available when needed for auditing/debugging

### For Template Design

Templates are unaffected. The template structure (states, accepts, transitions, gates) drives the engine's behavior regardless of what the response looks like. Response contract changes are purely in the CLI layer.

### For Future Extensibility

Keeping the response lean leaves room for:
- Future response fields that DO affect behavior (e.g., `estimated_time_to_completion` for long-running workflows)
- Without turning every response into a changelog

The event log, meanwhile, can be queried with increasingly sophisticated filters (by state, by time range, by evidence field, etc.) without touching the response contract.

## Surprises

1. **The acceptance criteria "indication that advanced phase(s) were passed through" doesn't require response-level data.** The event log already has it, fully detailed. The response needs only a lightweight summary (transition count) or nothing at all if observability is a separate query.

2. **The double-call pattern persists even with observability metadata in the response.** Skill code will still need to call again if it wants to know gate failure details, evidence status, or other deep information. Adding `passed_through` solves only the "how many transitions?" question. The real fix is the response variants, which eliminate the need to call again to determine next action.

3. **Response-level observability and the response contract are orthogonal concerns.** Extending auto-advance in the engine solves the behavioral problem (fewer calls to reach an actionable state). Observability is a separate problem with a separate solution (event log queries). They can be implemented independently.

4. **`koto state` command (deferred) is the right tool, not response metadata.** It replays the log, applies the replay rules, and returns a computed view. This is exactly what skills need for observability — and it's already designed, just deferred to #49+.

5. **Skills don't read the event log today because there's no documented interface.** AGENTS.md and work-on-template.md describe the response contract but not how to parse the JSONL event log. A simple `koto log` command (or `koto state`) that provides structured output would make log-based observability ergonomic.

## Open Questions

1. **Should we implement `koto log` (raw JSONL dump) or `koto state` (replayed snapshot) first?**
   - `koto log` is simpler (just dump the file, maybe with filtering)
   - `koto state` is more user-friendly (computed view, easier to interpret)
   - Decision: depends on scope of work deferred from #49

2. **If response gets `transition_count: usize`, should it also include `passed_through: Vec<String>` for consistency?**
   - Decision: `transition_count` is sufficient; `passed_through` can come from `koto state`
   - Rationale: count is O(1) to include, array is O(n) and scales with workflow depth

3. **Should the event log be directly queryable by agents, or should all queries go through CLI commands?**
   - Option A: Document the event log format in AGENTS.md; agents can parse it directly
   - Option B: Implement `koto state` / `koto log` commands; agents only use CLI
   - Decision: Option B (CLI interface) is more sustainable; event log schema could change

4. **For skills that need observability mid-execution (during a long-running workflow), should they poll `koto state` or read the event log incrementally?**
   - Polling `koto state` replays the entire log each time (expensive)
   - Reading event log incrementally requires tracking sequence numbers (complex)
   - Decision: Defer to integration runner design (#49+); may warrant a streaming API

5. **Should response-level observability be a forward-compatible extension, or should it wait for full observability design?**
   - Option A: Add `transition_count` now (lightweight, backward-compatible)
   - Option B: Extend response contract only in conjunction with full observability design
   - Decision: Option A is reasonable; transition_count is a simple, self-contained addition

## Summary

The event log is the correct mechanism for observability — it's the source of truth, already captures everything, and avoids duplicating summaries in responses. Response-level data should be limited to behavioral signals (current state, variant, blocking_conditions) plus lightweight metadata (transition_count for observability awareness). A deferred `koto state` command (already planned in DESIGN-event-log-format.md) provides the structured query interface for rich observability without bloating every response. This separation mirrors how git, kubectl, and other successful CLI tools handle the same problem: status/behavioral commands stay lean; observability/audit queries are separate.
