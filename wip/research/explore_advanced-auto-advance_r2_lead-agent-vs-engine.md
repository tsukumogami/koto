# Lead: Does agent-vs-engine distinction matter for callers?

## Findings

### 1. All Current Callers Ignore the Distinction

#### Work-on Skill (Only Production Consumer)
The work-on skill in `shirabe/skills/work-on/SKILL.md` uses `advanced` in exactly one way:
```
2. If `action: "execute"` with `advanced: true` — run `koto next <WF>` again
```

This is a **mechanical retry**, not a branching decision. The skill doesn't care *why* `advanced: true` (agent-initiated vs engine-initiated). It sees `advanced: true`, calls `koto next` again to get the next stopping condition, and moves on. The distinction plays no role in the skill's control flow.

#### Integration Tests
The integration tests (`tests/integration_test.rs`) assert on `advanced: true` only to verify auto-advancement happened:
```rust
assert_eq!(json["advanced"], true, "advanced should be true after auto-advancing");
```

Again, the tests don't branch on who caused the advancement. They simply verify that some advancement occurred.

#### CLI Usage Guide  
`docs/guides/cli-usage.md` defines `advanced: bool` as "true when an event was appended before dispatching" but provides no conditional logic that would require knowing the source of the event.

#### Event Log Replay
The `koto state` command and workflow replay scenarios access the full event log directly. They read `condition_type: "auto"` and `condition_type: "gate"` from `transitioned` events, plus `type: "evidence_submitted"` for agent-initiated events. **These callers already have the fine-grained distinction** via the event log and have no need for it in the CLI response.

### 2. The Event Log Already Provides Complete Disambiguation

The event log's event type taxonomy distinguishes agent-initiated vs engine-initiated at the source:

| Scenario | Event Type | Condition Type | CLI `advanced` |
|----------|-----------|-----------------|----------------|
| Agent submits evidence | `evidence_submitted` (then auto-advances) | `transitioned` with `"auto"` | `true` |
| Agent uses --to | `directed_transition` (then auto-advances) | `transitioned` with `"auto"` | `true` |
| Engine auto-advances | `transitioned` | `"auto"` | `true` |
| Gate-triggered transition | `transitioned` | `"gate"` | `true` |
| Default action execution | `default_action_executed` | N/A | `true` |

Any caller that needs to know who caused the advancement can **read the event log directly**. They get:
1. The exact sequence of events
2. Timestamps for each
3. The condition type that triggered each transition
4. The full evidence payload that was submitted

The CLI's `advanced: bool` is a lossy projection of this information. Callers needing the distinction don't use the CLI response—they use the event log.

### 3. Foreseeable Scenarios Don't Require the Distinction at CLI Level

#### Debugging Workflows
A developer debugging why a workflow advanced uses `koto state` or reads the event log directly. They want to see:
- Which transitions happened
- In what order
- What evidence was submitted
- Which gates passed or failed

None of this requires `advanced_by: "agent" | "engine"` in the CLI response. The event log provides all of it.

#### Observability Dashboards
A monitoring system tracking workflow progress would ingest the full event log (richer data) or implement custom state queries, not parse the `advanced` field to decide what's happening. Dashboard builders need timestamps, event sequences, and state changes—not a boolean that loses information.

#### Workflow Replay and Recovery
If a workflow needs to be replayed after a crash, the event log is the source of truth. Replaying against the same template produces the same outcome. The CLI response's `advanced` field is not part of the recovery path.

#### Multi-Agent Coordination
koto's current design assumes single-writer-single-consumer workflows (enforced by advisory flock). If multi-agent scenarios are added in the future:
- The distinction (who caused the advancement) would be important
- It's **already recorded in the event log** with full fidelity
- There is no reason to encode it lossy in the CLI response

The concurrent access design (DESIGN-auto-advancement-engine.md) explicitly says the flock prevents second callers from advancing. So multi-agent workflows aren't coordinating via a single koto state file anyway—they'd need a higher-level orchestrator that could read the event log.

### 4. The Design Documents Don't Specify Any Branching Logic on the Distinction

Searching the design documents:
- **DESIGN-unified-koto-next.md**: Introduces `advanced` as diagnostic feedback for agents, never discusses callers branching on the source
- **DESIGN-koto-cli-output-contract.md**: Specifies the field's meaning but no caller logic that depends on agent vs engine  
- **DESIGN-auto-advancement-engine.md**: Explains how auto-advancement works, but doesn't propose callers acting differently based on who advanced the state
- **DESIGN-shirabe-work-on-template.md**: Shows how templates would use workflows; the skill repeats the execution loop on `advanced: true` regardless of source

None of the designs contain pseudocode like "if advanced_by == 'agent' then X else Y."

### 5. If the Distinction Mattered, the Current Codebase Would Already Struggle

The distinction has been ambiguous since auto-advancement was added. If any non-trivial caller logic required knowing the source:
- Test failures would have surfaced
- The work-on skill's retry loop would need conditional branching (it doesn't)
- Design discussions would have been explicit about the distinction (they weren't)

The fact that code shipped and operates correctly (single-retry loop in work-on) indicates the distinction doesn't drive decision-making.

### 6. The Cost of Disambiguation

Adding `advanced_by: "agent" | "engine"` to the CLI response would:
1. Expand the output contract (breaking change for CLI parsing)
2. Create a new coupling: callers must understand two ways the state changed
3. Duplicate information already in the event log
4. Invite callers to make optimization decisions based on the CLI response instead of reading the log (architecturally weaker)

The design doesn't show a corresponding benefit.

## Implications

### The Distinction Is Architecturally Sound but Operationally Irrelevant

The engine correctly records whether each transition was auto-triggered or condition/gate-triggered. The event log is the authoritative source. The CLI response's `advanced: bool` was designed as agent feedback and remains adequate for that purpose.

Callers that need the fine-grained distinction already have it (via the event log). Callers that don't need it (the skill's mechanical retry) operate correctly with the current `advanced: bool`.

### The Current Design Requires No Changes

The work-on skill's behavior—"if advanced, call koto next again"—is correct and sufficient. It doesn't need to know *why* the state advanced. It just needs to know whether to verify the next stopping condition.

### A Disambiguation Would Solve a Non-Problem

If `advanced_by: "agent" | "engine"` were added, the work-on skill's logic would not change. The skill would still:
```
2. If `action: "execute"` with `advanced: true` — run `koto next <WF>` again
```

It doesn't care about `advanced_by`. The distinction wouldn't alter any control flow.

### Alternative: Make the Event Log Easier to Query

If future use cases arise where callers need to understand the sequence of events and who caused each, the better investment is:
1. Expose a `koto state --detailed` command that returns structured event summaries
2. Document how to parse the JSONL event log
3. Provide a library API to query the log

These are more powerful than adding a lossy field to the CLI response. They give callers the information they actually need for observability, debugging, and recovery.

## Surprises

1. **The skill doesn't inspect the distinction at all.** I expected the work-on workflow's complexity (multiple phases, self-loops) to require understanding why advancement happened. Instead, it uses a simple mechanical retry that works regardless.

2. **No tests assert on the source of advancement.** The integration tests verify `advanced: true` but never branch on it or use it to make decisions. This is consistent with the finding that no caller logic requires the distinction.

3. **The event log distinction is already complete.** The `condition_type: "auto"` vs `condition_type: "gate"` and the different event types (`evidence_submitted`, `directed_transition`, `transitioned`) form a rich audit trail. The CLI response's `advanced` field is redundant—callers that need details read the log.

4. **No foreseeable scenario requires the distinction in the response.** I thought debugging, observability dashboards, or replay logic might need to know "the agent caused this advancement." But in each case, callers either read the event log (where the distinction is sharp) or don't care about the distinction at all (the skill's mechanical retry).

## Open Questions

1. **Will multi-agent coordination ever need the distinction in the CLI response?** The current single-writer design (flock) prevents simultaneous advancement by multiple agents. If koto ever supports coordinated multi-agent workflows, would those agents need to know "agent B caused this advancement" from the CLI response? Likely yes, but they could also read the event log. The CLI response isn't a necessary vehicle.

2. **Is there a use case where callers must decide quickly (without reading the log) whether to call `koto next` again?** The work-on skill reads the event log implicitly (via the engine's evidence-merging logic), so it has the information. Callers that can't afford to read the log would be making decisions on incomplete information anyway. This suggests the CLI shouldn't pretend to give them what they need.

3. **Should the `advanced` field be redesigned or supplemented independently of this question?** Even if no caller cares about the agent-vs-engine distinction, callers might benefit from more precise fields: "was_terminal_reached", "gates_blocked", "evidence_accepted", etc. That's orthogonal to the current investigation.

## Summary

The agent-vs-engine distinction in the `advanced` field **does not matter for any real caller scenario identified in the codebase**. All current consumers—the work-on skill, integration tests, and documentation—treat `advanced: true` as a mechanical signal to retry or verify state, not as a decision point that requires knowing the source of advancement. The event log already provides complete disambiguation to callers that do need it, while callers that don't need it operate correctly with the current response. **Adding `advanced_by: "agent" | "engine"` would expand the contract without changing behavior, and would duplicate information available in the event log. The current design is adequate.**

