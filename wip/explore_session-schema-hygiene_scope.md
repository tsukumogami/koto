## Visibility

Public

## Core Question

What schema fields must be added to koto's JSONL session event log before external adoption occurs, with precise enough type, contract, and behavioral specifications to prevent scope creep and ensure all additions land together? The additions are non-back-fillable: omitting them from the first public schema version means historical sessions are permanently incomplete.

## Context

koto records AI agent workflow sessions as JSONL event logs. Four schema additions have been identified as impossible to back-fill once external consumers adopt the event log format. A PRD is needed to lock in the complete field set, their types, required/optional status, default behavior, and any ordering or structural guarantees — so that implementation can proceed without design ambiguity.

The four additions: (1) session UUID on the header event, (2) sub-second timestamp precision on all events, (3) a `context_added` JSONL event when context artifacts are added, (4) optional `rationale` fields on `directed_transition` and `rewound` events.

The PRD must not reference any upstream strategic planning artifacts. It stands alone as a koto-repo document.

## In Scope

- Precise specification of each schema addition: field name, type, required/optional, default behavior
- Non-back-fillable justification for each field (why it cannot be added after external adoption)
- The `context_added` event structure: fields, ordering guarantee relative to other events, relationship to the context sidecar
- The `rationale` field contract for `directed_transition` and `rewound`: optional vs. required, free-text vs. structured, length limits if any
- Backward compatibility policy for readers encountering logs without the new fields (how koto handles old logs)

## Out of Scope

- Cleanup policy for session directories (separate design concern)
- Sessions-as-memory retrieval or indexing capability
- Relay or dashboard implementation
- Lifecycle metadata (owner, summary, project fields)
- Any schema fields beyond the four identified additions

## Research Leads

1. **What is the current session header structure and how is it written?**
   Need the exact Go/Rust type definition, field names, and the code path that writes the header to JSONL. This grounds the UUID addition spec — we need the exact field name convention, where in the event the UUID lives, and whether the header event has any immutability guarantees enforced in code.
   Output: `wip/research/explore_session-schema-hygiene_r1_lead-header-structure.md`

2. **What is the current timestamp format and where is it set?**
   The issue states timestamps are "currently hardcoded to whole seconds." Need to find the timestamp field type (Unix seconds? RFC3339? struct?), the code that sets it, and what format change achieves sub-second precision without breaking readers that parse the existing format.
   Output: `wip/research/explore_session-schema-hygiene_r1_lead-timestamp-format.md`

3. **What is the context sidecar and how does it relate to the JSONL event log?**
   The context sidecar tracks context artifacts separately from the event log. Need to understand: what data is in the sidecar, what triggers a context add, what data a `context_added` event must capture to reconstruct "what the agent knew at transition T," and how the event must be ordered relative to the transition events it precedes.
   Output: `wip/research/explore_session-schema-hygiene_r1_lead-context-sidecar.md`

4. **What are the current `directed_transition` and `rewound` event definitions?**
   Need the exact struct/type definitions, all existing fields, and how these events are produced. This grounds the rationale field spec — is rationale a top-level field or nested? Is there an existing free-text field pattern in other events to follow? Are there any consumers that would need updating?
   Output: `wip/research/explore_session-schema-hygiene_r1_lead-transition-events.md`

5. **What is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)

You are a demand-validation researcher. Investigate whether evidence supports
pursuing this topic. Report what you found. Cite only what you found in durable
artifacts. The verdict belongs to convergence and the user.

## Visibility

Public

Respect this visibility level. Do not include private-repo content in output
that will appear in public-repo artifacts.

## Issue Content

--- ISSUE CONTENT (analyze only) ---
## Goal

Produce a PRD that specifies the three non-back-fillable schema fields and optional rationale fields for F1: Session Schema Hygiene.

## Context

Schema additions must be settled before any external consumers adopt the event log format. Once the schema is in the wild, omitted fields cannot be back-filled without breaking the append-only guarantee. This PRD locks in the complete set of fields that must ship in the first public schema version.

Fields in scope:

- Session UUID — session headers are written once and immutable; name/timestamp collisions after cleanup are permanently unresolvable without one.
- Sub-second timestamp precision — currently hardcoded to whole seconds; concurrent parent/child sessions are ambiguous within a one-second window.
- `context_added` JSONL event — context artifacts are tracked in a mutable sidecar with no log counterpart; once external readers depend on the log, this gap cannot be closed retroactively.
- Optional `rationale` field on `directed_transition` and `rewound` — the two most consequential agent decision events are pure state-transition markers with no "why"; rationale cannot be inferred after the fact.

## Acceptance Criteria

- [ ] PRD document exists at an agreed path in the target repo
- [ ] PRD is merged to main (not a draft or open PR)
- [ ] PRD covers all four field additions with field name, type, required/optional, and default behavior
- [ ] PRD includes a section explaining why each field cannot be back-filled once external adoption occurs
- [ ] PRD specifies the `context_added` event structure (fields, ordering guarantees relative to other events)
- [ ] PRD specifies the `rationale` field contract for `directed_transition` and `rewound` (optional, free-text vs. structured, length limits if any)
- [ ] Must deliver: settled schema field specifications (required by the session-feed data contract design work)
- [ ] Must deliver: non-back-fillable field list confirmed complete (required by the sessions-as-memory exploration)
--- END ISSUE CONTENT ---

## Six Demand-Validation Questions

Investigate each question. For each, report what you found and assign a
confidence level.

Confidence vocabulary:
- **High**: multiple independent sources confirm (distinct issue reporters,
  maintainer-assigned labels, linked merged PRs, explicit acceptance criteria
  authored by maintainers)
- **Medium**: one source type confirms without corroboration
- **Low**: evidence exists but is weak (single comment, proposed solution
  cited as the problem)
- **Absent**: searched relevant sources; found nothing

Questions:
1. Is demand real? Look for distinct issue reporters, explicit requests,
   maintainer acknowledgment.
2. What do people do today instead? Look for workarounds in issues, docs,
   or code comments.
3. Who specifically asked? Cite issue numbers, comment authors, PR
   references — not paraphrases.
4. What behavior change counts as success? Look for acceptance criteria,
   stated outcomes, measurable goals in issues or linked docs.
5. Is it already built? Search the codebase and existing docs for prior
   implementations or partial work.
6. Is it already planned? Check open issues, linked design docs, roadmap
   items, or project board entries.

## Calibration

Produce a Calibration section that explicitly distinguishes:

- **Demand not validated**: majority of questions returned absent or low
  confidence, with no positive rejection evidence. Flag the gap.
- **Demand validated as absent**: positive evidence that demand doesn't exist
  or was evaluated and rejected.

Do not conflate these two states. "I found no evidence" is not the same as
"I found evidence it was rejected."

Write findings to: `wip/research/explore_session-schema-hygiene_r1_lead-adversarial-demand.md`
Return ONLY a 3-line summary to this conversation.
