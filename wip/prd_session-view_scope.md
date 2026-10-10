# /prd Scope: session-view

## Upstream
docs/briefs/BRIEF-session-view.md (Accepted)

## Visibility
Public

## Mode
auto (under /scope's parent_orchestration sentinel)

## Scope summary
Requirements for a human-readable view of one koto session riding the two
existing surfaces: the local dashboard (TUI detail and a one-shot detail mode
behind a new flag) and the workflow view's session rendering (compact additive
summary, never content). The reading covers state and directive, execution
anchor and store origin, and every context key with size and legible content;
absences, unreadable keys and migrated sessions are stated plainly.

## Research basis
Four research files from the preceding exploration round, in
wip/research/explore_session-view-surfaces_r1_lead-*.md (workflows-render,
dashboard, context-store, large-values), plus the exploration findings and
decisions files. They cover both surfaces' current data contracts, the context
store's APIs and costs, migration refusal behavior, and the repo's
truncation/legibility precedents.
