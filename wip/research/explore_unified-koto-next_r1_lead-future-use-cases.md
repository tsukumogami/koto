# Lead: How would delegation, evidence, and approval gates fit into unified koto next?

## Findings

### Use Case 1: Cross-agent delegation

In the current delegation design, the flow is:
1. `koto next` returns a directive with `DelegationInfo` — the agent should produce a prompt for a delegate
2. Agent writes the prompt to a file
3. `koto delegate run --prompt-file /tmp/prompt.txt` — koto invokes the delegate CLI, captures response, returns JSON

Under a unified `koto next` model, two interpretations:

**Interpretation A: koto still handles invocation**
`koto next --submit /tmp/prompt.txt` — the agent submits the prompt file; koto detects the current state expects delegation, invokes the delegate, returns the delegate response plus the next directive. This keeps koto owning the invocation mechanics (subprocess, timeout, stdout capture). The agent just submits the prompt.

**Interpretation B: agent handles invocation, submits response**
`koto next --submit /tmp/response.json` — the agent invokes the delegate itself (`claude -p < prompt.txt > response.txt`), then submits the response to koto. This removes koto's role in subprocess management. koto just stores the response as evidence and advances state.

The current design chose Interpretation A for `koto delegate run`. Under a unified model, Interpretation B fits more cleanly — the agent submits a file regardless of action type, and koto validates against current state expectations. But it shifts invocation responsibility to the agent.

**Shape of submission:** A JSON file with `{"type": "delegation_response", "response": "..."}` or a plain text file. The current state's `expects` schema tells the agent which format to use.

### Use Case 2: Evidence submission

Evidence submission is straightforward in the unified model. When a state requires evidence before gates open:

1. `koto next` (read) returns `{"expects": {"type": "evidence", "keys": ["test_output", "severity"]}}`
2. Agent runs tests, produces evidence
3. `koto next --submit /tmp/evidence.json` with `{"type": "evidence", "test_output": "...", "severity": "low"}`
4. koto validates against current state's required fields, stores in evidence map, re-evaluates gates
5. If gates pass: auto-transition, return next directive
6. If gates don't pass: return current directive with gate status

This is clean and generic. The file format is a JSON envelope with a `type` field. No per-use-case CLI flags needed.

**Key finding from codebase:** The engine already has an evidence map (`WithEvidence()`) but the CLI has no `--evidence` or `--submit` flag. Evidence submission is a missing CLI layer, not a missing engine capability.

### Use Case 3: Approval gates

Approval gates are fundamentally different from delegation and evidence:

| Property | Delegation / Evidence | Approval Gates |
|----------|----------------------|----------------|
| Initiator | Orchestrating agent | Human or external system |
| Timing | Agent submits when ready | External party decides when |
| Channel | CLI (`koto next --submit`) | External (GitHub comment, Slack, API call) |
| Blocking model | Agent is blocked, waiting for koto to accept | Workflow is blocked, waiting for external signal |
| Synchronous? | Yes — agent submits, koto responds immediately | No — agent submits query, polls or waits for event |

Approval gates can't fit the same `koto next --submit` model cleanly because:
1. The orchestrating agent can't submit the approval — it's waiting for someone else to do it
2. The agent's role during an approval-gated state is to poll (`koto next`) and wait, not to submit
3. The approval comes from an external command against the state file (e.g., `koto approve --workflow <name>`)

**Better model for approval gates:** The approval is written into the state file by a separate tool or API call (not by the orchestrating agent). The orchestrating agent calls `koto next` in a polling loop and eventually receives a directive with the gate satisfied. This matches the engine's current gate model — command gates and field gates can be satisfied by external processes writing to the state file directly.

### Can a single generic model cover all three?

| Use Case | Fits unified `koto next --submit`? | Notes |
|----------|-----------------------------------|-------|
| Delegation | Yes (Interpretation B) | Agent submits delegate response as evidence |
| Evidence submission | Yes, cleanly | JSON envelope with `type: evidence` |
| Approval gates | No — wrong initiator | Approval comes from outside the agent loop |

The unified model covers two of three. Approval gates require a separate input channel (an external write to state or a dedicated `koto approve` command), but they don't need to be part of `koto next` — the agent loop just polls `koto next` until the gate clears.

### Engine capabilities vs. CLI gaps

From reading the codebase:
- `engine.WithEvidence()` exists — evidence storage is implemented
- Gate evaluation at transition time exists
- No CLI flag for submitting evidence or data through `koto next`
- No `koto approve` command

The unified model is primarily a CLI design question, not an engine architecture question. The engine can already handle evidence-based gates. What's missing is the submission path.

## Implications

The unified `koto next` model works cleanly for the two agent-initiated submission use cases (delegation, evidence). The key design is:

1. `koto next` (no args) → always returns current directive + `expects` schema
2. `koto next --submit <file>` → validates file against `expects`, stores as evidence, re-evaluates gates, auto-transitions if gates pass

For delegation specifically, the model pushes subprocess invocation to the agent (agent runs `claude -p`, captures output, submits as evidence). This is simpler for koto but requires the agent skill to know how to invoke delegates.

Approval gates are out of scope for the unified model — they need a separate command or external API, but don't conflict with it.

## Surprises

The most important insight: the engine already supports evidence storage — the gap is entirely at the CLI layer. The unified `koto next --submit` model adds one flag and a validation step, not a fundamental engine change.

Delegation is more interesting: the current design (`koto delegate run`) keeps subprocess invocation inside koto. Under the unified model, if the agent submits the delegate response as evidence, the agent must own invocation. This is a meaningful architectural choice about where subprocess management lives.

## Open Questions

1. **Who owns delegate invocation?** If `koto delegate run` is removed in favor of agents submitting evidence, the agent skill must document how to invoke delegates. Is that simpler or more complex than koto owning it?

2. **How does the agent know what type of evidence to produce?** The `expects` schema in `koto next` output must be specific enough that the agent can construct the right JSON without additional documentation.

3. **Can approval gates work via external writes to the state file?** If yes, `koto approve` is just a convenience wrapper. If the state file format is too complex for external tools to write safely, a dedicated command is needed.

## Summary

Delegation and evidence submission fit cleanly into a unified `koto next --submit <file>` model using a typed JSON envelope, but approval gates are fundamentally different — they're initiated by external systems, not the orchestrating agent, and fit better as an external write to the state file that the agent detects by polling `koto next`. The key implication is that the unified model should focus on agent-initiated submissions (the two common cases) and treat approval as a separate input channel. The biggest open question is whether `koto delegate run` should be removed in favor of agents submitting delegate responses as evidence, since this shifts subprocess invocation to the agent skill layer rather than keeping it in koto.
