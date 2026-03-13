# Phase 2 Research: User Researcher

## Lead 1: Agent workflow loop use cases

### Scenario 1: Linear workflow

**Current agent loop:**
```
koto init --template <path> --name <workflow>
loop:
  koto next                        # get current directive
  <execute directive>
  koto transition <target>         # explicitly name next state
```

Agent must know valid target state names. If agent calls `koto transition` with an invalid target, it gets `valid_transitions` in the error and retries.

**Unified agent loop:**
```
koto init --template <path> --name <workflow>
loop:
  koto next                        # get directive; auto-advances when command gates clear
  if action == "done": break
  <execute directive>
  # no transition call needed — koto handles it
```

For linear workflows with command gates (koto verifies independently), the agent simply executes the directive and calls `koto next` again. koto re-evaluates gates, and once they clear, returns the next state's directive automatically.

**Required output fields:**
- `action`: "execute" or "done"
- `state`: current state name (for logging/debugging)
- `directive`: the instruction text
- `advanced`: bool — did this `koto next` call advance state from the previous call?

**Edge cases:**
- Agent calls `koto next` but command gate hasn't cleared yet (CI still running) → returns same directive, `advanced: false`
- Agent calls `koto next` after gate clears → returns new directive, `advanced: true`
- Agent must distinguish "waiting" from "error" — a gate that will never clear (failed CI) vs. one that's still running

---

### Scenario 2: Branching workflow

**Current agent loop:**
Agent reaches state with `transitions: [plan, escalate]`. Agent decides which to take, calls `koto transition plan` or `koto transition escalate`. Engine validates the target is in the allowed list and gates pass.

**Unified agent loop:**
Agent cannot name a target — auto-advancement requires exactly one branch's gates to be satisfied. The mechanism: agent submits evidence that satisfies one branch's transition-level gates.

```
koto next                          # returns directive + expects field
# expects: {type: "transition_choice", options: [{target: "plan", requires: {complexity: "low"}}, {target: "escalate", requires: {complexity: "high"}}]}
<agent decides: complexity is high>
koto next --submit evidence.json   # {"complexity": "high"}
# koto evaluates: escalate's gate satisfied (complexity == high), plan's gate not satisfied
# auto-advances to "escalate", returns that state's directive
```

**Required output fields:**
- `expects.type`: "transition_choice"
- `expects.options`: list of `{target, requires}` — what evidence satisfies each branch
- After submission: `advanced: true`, `state`: new state name

**Edge cases:**
- Agent submits evidence that satisfies no branch → error with structured detail of which gates failed and why
- Agent submits evidence that satisfies multiple branches → error or defined priority rule (PRD should specify)
- Template has single outgoing transition but uses evidence gate → should work transparently, no user-facing complexity

---

### Scenario 3: Delegation

**Current design (not yet implemented):**
Agent receives `Directive` with `DelegationInfo` field (from cross-agent delegation design). Agent produces prompt, calls `koto delegate run --prompt-file /tmp/prompt.txt`. koto invokes delegate subprocess, captures stdout, returns JSON.

**Unified model:**
Agent calls `koto next`. If current state has delegation tags and config rules match, koto detects this internally. koto invokes the delegate CLI (no separate command), captures the response, and includes it in the returned directive.

```
koto next
# koto: detects delegation config, invokes "gemini -p < directive", captures stdout
# returns: {action: "execute", state: "audit", directive: "...", advanced: false, delegation: {ran: true, delegate: "gemini", response: "..."}}
<agent uses delegate response in its own work>
koto next  # or koto next --submit evidence.json if the state expects evidence
```

**Required output fields:**
- `delegation.ran`: bool — did koto invoke a delegate this call?
- `delegation.delegate`: which target was used
- `delegation.response`: the delegate's output (may be large — PRD should specify size limits or streaming)
- `delegation.available`: false if delegate CLI not found (fallback: agent handles directive directly)

**Key question surfaced:** Does the agent act on the delegate's response, or does koto use it as evidence automatically? If the delegate audited code and found issues, the agent likely needs to do something with those findings (write them up, fix them, etc.) before advancing. The delegate response is informational input to the agent, not a gate-satisfying submission.

**Edge cases:**
- Delegate CLI not in PATH → `delegation.available: false`, agent handles directive itself
- Delegate exceeds timeout → error with `delegation.error`, agent can retry or handle
- Delegate returns non-zero exit → error code in response, agent must decide whether to retry or escalate

---

### Scenario 4: Evidence submission

**No current equivalent** — the engine supports `WithEvidence()` but no CLI flag exists. This is a new capability.

**Unified agent loop:**
```
koto next
# returns: {action: "execute", directive: "Run tests and report severity", expects: {type: "evidence", fields: {test_output: "string", severity: "enum[low,medium,high,critical]"}}}
<agent runs tests>
echo '{"test_output": "...", "severity": "low"}' > evidence.json
koto next --submit evidence.json
# koto stores evidence, re-evaluates gates, gates clear, auto-advances
# returns: {action: "execute", state: "next-state", directive: "...", advanced: true}
```

**Required output fields:**
- `expects.type`: "evidence"
- `expects.fields`: map of field name to type/constraint — what the agent must submit
- After submission: `advanced: bool`, gate status if not advanced

**Edge cases:**
- Agent submits evidence but gates still don't clear (e.g., `severity` must be "low" but agent submitted "high") → `advanced: false`, gate failure detail
- Agent submits evidence for wrong fields (typo in key) → validation error, not gate failure
- Agent submits evidence in a state that doesn't expect it → `precondition_failed` error

---

## Cross-scenario requirements

1. **`koto next` output must be self-describing.** The agent's next action must be determinable from the output alone, without session history or foreknowledge of template structure.

2. **`advanced` field is required.** Agents calling `koto next` in a polling loop must know whether state changed without comparing state names between calls.

3. **`expects` field enables generic submission.** Without it, agents must be pre-programmed with knowledge of each state's requirements. With it, the agent loop is generic: read `expects`, construct submission, call `--submit`.

4. **Error codes must support recovery branching.** An agent needs to distinguish: `gate_blocked` (wait or satisfy), `precondition_failed` (wrong operation for this state), `invalid_submission` (fix the data format), `delegate_unavailable` (handle directive directly).

5. **Directive must be interpolated.** The `directive` field must have variables already substituted — agents receive the final text, not a template with `{{variable}}` syntax.

6. **Delegation response in output.** When koto ran a delegate internally, the delegate's response must be accessible to the agent in the `koto next` output. Agents use it as context, not as a gate submission.

## Open Questions

- When `koto next` auto-advances and the new state also has gates that immediately clear (e.g., a command gate that passes instantly), does koto chain-advance through multiple states in a single call? Or does it return after the first transition?
- What's the size limit for evidence submissions? For delegate responses returned in output?
- When a state has both koto-owned gate checks (command gates) and agent-supplied evidence gates, does `koto next` run the command gates first and only prompt for evidence after they clear?

## Summary

The unified `koto next` model works cleanly across all four scenarios: linear workflows require only removing the explicit `koto transition` call; branching works via evidence-based transition-level gates where `expects.options` tells the agent what to submit; delegation becomes transparent when koto invokes the delegate internally and returns the response in output; evidence submission follows the `expects` → `--submit` → auto-advance loop. All four scenarios converge on the same requirement: `koto next` output must be self-describing — the agent's next action must be unambiguous from the response alone, with no session state or template foreknowledge required.
