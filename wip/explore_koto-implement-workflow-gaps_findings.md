# Exploration Findings: koto-implement-workflow-gaps

## Core Question

koto has an intentionally minimal CLI surface. What integrations does koto need to
support `/implement` running on top of its existing surface, and — given that
/implement requires a multi-workflow orchestrator layer — what should the first
koto-backed shirabe skill actually be?

## Round 1

### Key Insights

- The integration runner is a stub: koto's integration system is designed as a
  deferred-closure architecture but the runner always returns `IntegrationUnavailable`.
  The config system is explicitly deferred. (Lead: koto-integration-system)
- CI polling fits koto's model if wrapped inside the integration runner — koto stays
  fire-and-forget, the runner handles the wait. (Lead: integration-shape-mismatches)
- /implement's multi-issue state model is structurally incompatible with koto's
  single-workflow event log. Requires an orchestrator layer above koto.
  (Lead: state-model-fit)
- DESIGN-workflow-tool-oss prescribes a phased path: just-do-it → work-on →
  implement-doc. Start with zero external dependencies. (Lead: workflow-tool-oss-design)

### Tensions

- /implement requires orchestrator layer above koto — too complex for phase 1
- Integration runner config system is deferred — needed for /implement but not for simpler workflows

### Gaps

- Research files not written (environment issue); findings from summaries only
- Exact /implement state file schema not deeply examined

### Decisions

- /implement is not the first target: requires multi-workflow orchestrator layer above koto, deferred
- Focus shifted to merged work-on/just-do-it as the first koto-backed shirabe skill
- Integration runner config system deferred: phase 1 may not need it

### User Focus

User identified that shirabe already has /work-on and /just-do-it could merge into it
cleanly, making the merged skill a natural first koto integration target.

## Round 2

### Key Insights

- The merge is low-cost: both skills follow setup → research → analyze → implement →
  finalize → PR. The only structural difference is phases 0-2 (GitHub issue vs.
  free-form description). Make issue optional as an input parameter; phases 4-6 are
  identical. (Lead: work-on-just-do-it-overlap)
- The koto template maps cleanly: ~7-8 states, mostly auto-advancing, one branch
  at staleness/introspection check (expressible as enum field + dual `when` conditions).
  Existing koto template features are sufficient. (Lead: koto-template-design)
- No integrations needed for phase 1: the agent handles all external actions (git, PR
  creation, CI monitoring). CI monitoring is agent-driven with evidence submission
  (`koto next --with-data decision=approved`) rather than a koto command gate.
  (Lead: agent-vs-integration)

### Tensions

None significant. All three leads aligned and reinforced each other.

### Gaps

- Skip pattern: when staleness check returns "fresh," workflow jumps directly to
  analysis. Theoretically supported by koto's `when` conditions but needs a worked
  example in the design.
- Shirabe invocation mechanics: exactly how shirabe initializes koto (SessionStart
  hook, `koto init`, directive loop) needs design.

### Decisions

- Merged work-on/just-do-it is the design target, centered in shirabe
- No koto integrations required for phase 1 (agent-as-integration decision)
- GitHub issue is an optional input parameter, not a structural difference
- Agent handles CI monitoring via evidence submission, not koto command gates

### User Focus

User confirmed Design Doc is the right artifact. Direction is clear.

## Decision: Crystallize

## Accumulated Understanding

**What we're building:** A single merged work-on/just-do-it skill in shirabe, backed
by a koto workflow template, where the GitHub issue is an optional input parameter. This
is shirabe's first koto-backed skill and the first real-world proof of koto's template
engine.

**How the template works:** ~7-8 states in a mostly linear koto template. Auto-advances
through setup and analysis phases. One branching point: staleness/introspection check
uses an enum evidence field (`staleness: fresh | stale`) with `when` conditions routing
either to skip introspection (fresh) or run it (stale). The agent handles all external
actions; koto enforces phase order.

**What's not needed yet:** Integration runner config system, CI polling gates,
multi-workflow orchestrator. All deferred.

**Open design questions (belong in the design doc):**
- Full state list and directive text for each state
- Evidence schema at each gated transition
- Skip pattern: how `when` conditions express non-adjacent state jumps cleanly
- Shirabe invocation: SessionStart hook, `koto init` call, directive loop mechanics
- Session resume: how koto's event log handles a multi-session workflow
- Merge mechanics: how optional GitHub issue input changes phases 0-2
