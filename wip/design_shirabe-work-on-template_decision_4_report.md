<!-- decision:start id="introspection-outcome-model" status="assumed" -->
### Decision: Introspection outcome model in the koto work-on template

**Context**

The `introspection` state in the work-on template backs the /work-on skill's Phase 2, where a sub-agent re-reads the GitHub issue against the current codebase to assess whether the planned implementation approach is still valid. The real skill's Phase 2 sub-agent produces four possible recommendations: Proceed (approach still valid), Clarify (something is ambiguous; ask the user before continuing), Amend (update the issue scope, then continue), and Re-plan (the issue is fundamentally superseded; stop).

The current design's `introspection_outcome` enum has three values: `approach_unchanged`, `approach_updated`, and `issue_superseded`. This misses Clarify and Amend entirely. Panel critique identified two concrete problems: Clarify and Amend have no routing target in the state machine (agents in `introspection` that receive these recommendations from the sub-agent have nowhere to go), and `issue_superseded` routes to nothing explicit — the design implies advancing to `analysis`, but a superseded issue should stop the workflow, not continue.

koto has no built-in AskUserQuestion primitive. Human interaction must be modeled as a state whose directive instructs the agent to ask. Terminal states (`done`, `done_blocked`) cannot be resumed once reached — a new workflow must be initialized from scratch.

**Assumptions**

- The introspection sub-agent's Clarify and Amend work (including user interaction) happens inside the sub-agent's task, not as separate koto states. The orchestrating agent spawns the sub-agent via the Task tool, the sub-agent completes its internal loop (including any Clarify/Amend cycles), writes the introspection artifact, and returns a final recommendation. koto tracks the macro-level outcome, not the sub-agent's internal steps.
- The `introspection` gate (`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`) already enforces that the artifact was produced before the state advances. The artifact's content is what guides the outcome evidence, independent of whether a Clarify or Amend loop occurred inside the sub-agent.
- `done_blocked` is the correct terminal state for `issue_superseded`. The issue is not actionable; stopping and requiring human review is the right response.

**Chosen: Collapse Clarify and Amend into approach_updated, route issue_superseded to done_blocked**

The `introspection_outcome` evidence field becomes a three-value enum: `approach_unchanged`, `approach_updated`, `issue_superseded`.

- `approach_unchanged` — the sub-agent found the approach still valid; proceed to analysis
- `approach_updated` — the sub-agent found that the approach needs revision (including any Clarify or Amend cycles it completed); proceed to analysis with updated context
- `issue_superseded` — the issue is fundamentally invalid or obsolete; route to `done_blocked`

The `rationale` string field (already in the design) captures what happened: `"Clarified with user: the caching requirement changed to TTL-based, approach updated accordingly"` or `"Amended issue #71 to remove the stale auth section; approach updated for new auth flow"`. The event log preserves this permanently.

The `issue_superseded → done_blocked` routing is added explicitly. This fixes the current design's undefined routing gap.

**Rationale**

Clarify and Amend are sub-phases of the sub-agent's task, not koto workflow states. The sub-agent handles them internally: it queries the user, gets a response, and updates the introspection artifact. From koto's perspective, what matters is whether the orchestrating agent should proceed to analysis (with or without approach changes) or stop. That's a binary at the macro level, with `approach_unchanged` and `approach_updated` capturing the distinction that's useful for the audit trail.

Adding koto states for Clarify and Amend would model behavior that belongs inside the sub-agent's Task invocation. A `needs_clarification` value with a target state requires either a new `awaiting_clarification` state (with a directive saying "ask the user") or routing to `done_blocked` (wrong — Clarify is recoverable). The `awaiting_clarification` path adds states for sub-agent internal behavior, creates ambiguity about whether to loop back to introspection or proceed to analysis after getting the answer, and duplicates user-interaction modeling at the wrong level of abstraction. Amend has a different post-interaction destination than Clarify (always advance to analysis, not re-introspect), which would require a second user-interaction state with different routing — two new states for one conceptual need.

Collapsing into `approach_updated` preserves what koto needs to enforce (did the approach change, and should the workflow continue) while letting the sub-agent's internal loop remain in the sub-agent. The rationale field makes the audit trail useful. The fix is minimal: route `issue_superseded` to `done_blocked`, confirm `approach_unchanged` and `approach_updated` both route to `analysis`, and document in the template directive that `approach_updated` covers both Clarify and Amend outcomes.

The two-value enum (option d) would collapse `approach_unchanged` and `approach_updated` into a single "proceed" value, losing a distinction that has audit value. The 5-value enum (option a) expands the enum without solving the routing problem — `needs_clarification` and `needs_amendment` still need target states, and without them the expansion makes things worse by surfacing outcomes that have nowhere to go.

**Alternatives Considered**

- **Expand to 5-value enum (a)**: `approach_unchanged`, `approach_updated`, `issue_superseded`, `needs_clarification`, `needs_amendment`. Rejected because enum expansion alone doesn't solve the routing gap — `needs_clarification` and `needs_amendment` still need valid target states in the state machine. Without target states, the expansion creates new stuck-workflow paths rather than fixing the existing one. Option (a) is a prerequisite for option (b), not a solution on its own.

- **Separate user_interaction state (b)**: Add one or two non-terminal states (`awaiting_clarification`, `awaiting_amendment`) reachable from `introspection`, with directives that say "ask the user for clarification" or "amend the issue." Rejected because this models sub-agent internal behavior at the koto workflow level. The sub-agent (spawned via Task tool) is the right place to handle user interaction during introspection. Adding koto states for this creates two ambiguities: whether to return to `introspection` after clarification (Clarify may change the introspection outcome) vs. proceed directly to `analysis` (Amend means "update scope then continue"), and where to route if the user's answer makes the issue superseded after all. These ambiguities require additional states or conditional transitions that grow complexity without adding enforcement value. koto's enforcement is that the introspection artifact exists — it can't verify whether the sub-agent asked the right questions internally.

- **Two-value enum (d)**: `proceed_with_analysis: true/false`, with `done_blocked` for false. Rejected because it loses the `approach_unchanged` vs. `approach_updated` distinction, which is useful in the audit log for understanding whether the agent deviated from the original plan. Also doesn't clarify whether Clarify/Amend map to true or false — collapses meaningful outcomes into a single bit.

**Consequences**

The template's `introspection` state evidence schema becomes:

```yaml
accepts:
  introspection_outcome:
    type: enum
    values: [approach_unchanged, approach_updated, issue_superseded]
    required: true
  rationale:
    type: string
    required: true
transitions:
  - target: analysis
    when:
      introspection_outcome: approach_unchanged
  - target: analysis
    when:
      introspection_outcome: approach_updated
  - target: done_blocked
    when:
      introspection_outcome: issue_superseded
```

The directive for `introspection` should explicitly state that `approach_updated` covers both Clarify outcomes (where the sub-agent asked the user and updated the approach) and Amend outcomes (where the sub-agent updated the issue description and adjusted the approach). The rationale field should capture what changed.

What becomes easier: the state machine is complete and non-stuck. All four real sub-agent outcomes map to a koto evidence value with a defined routing target. The rationale field makes the audit trail useful for understanding what happened inside introspection.

What becomes harder: the template directive carries more weight. It must explain the Clarify/Amend → approach_updated mapping clearly enough that an agent running introspection submits the right evidence value. Without clear directive text, an agent might wait for a `needs_clarification` enum value that doesn't exist and get confused. This is a documentation burden on Phase 2 (template authoring), not a structural gap.

`done_blocked` remains terminal. An `issue_superseded` outcome means the workflow ends and a human must decide whether to update the issue, close it, or start fresh. This is intentional — a superseded issue is not a recoverable condition within the current workflow run.
<!-- decision:end -->
