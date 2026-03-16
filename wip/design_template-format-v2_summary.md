# Design Summary: template-format-v2

## Input Context (Phase 0)
**Source:** Freeform topic (from /explore research on issue #47)
**Problem:** koto's template format v1 can't express evidence-driven routing or
processing integrations. The event log format (#46) needs templates to declare
evidence schemas and conditional transitions.
**Constraints:** No users to migrate. Must support the advancement engine's needs
(#49). Field gates and accepts/when must not overlap semantically.

## Explore Research Available
- `wip/research/explore_template-format-v2_r1_lead-v2-compiled-schema.md`
- `wip/research/explore_template-format-v2_r1_lead-mutual-exclusivity.md`
- `wip/research/explore_template-format-v2_r1_lead-gates-interaction.md`
- `wip/research/explore_template-format-v2_r1_lead-next-loading.md`
- `wip/research/explore_template-format-v2_r1_lead-integration-field.md`

## Approaches Investigated (Phase 1)
- **Strict Separation**: Field gates forbidden on accepts states. Two clean control models. No deal-breakers.
- **Coexistence with Precedence**: Both allowed, gates first. Complex mental model, semantic ambiguities.
- **Unified Model**: Remove field gates entirely. Simplest model, minor expressiveness gap.

## Selected Approach (Phase 2)
Unified Model: remove field gates entirely, keep only command gates alongside
accepts/when. Field gates are redundant in the event-sourced model where evidence
enters through --with-data and is scoped by the epoch boundary.

## Investigation Findings (Phase 3)
- **Rust types**: TemplateState adds accepts, integration, structured transitions. Gate removes field types, keeps only command. New FieldSchema and Transition types.
- **Compiler changes**: New mutual exclusivity validation (group by field, check duplicate values). when fields must reference accepts schema. Field gates rejected with helpful error.
- **Downstream impact**: hello-koto survives unchanged (command gates only). Integration tests need v2 template format. Plugin CI adapts transparently.
- No contradictions or deal-breakers found.

## Security Review (Phase 5)
**Outcome:** Option 3 (N/A with justification)
**Summary:** Purely declarative schema change with no new execution paths, downloads, or data exposure.

## Final Review (Phase 6)
**Architecture review:** No blockers. Applied 5 advisory improvements (transition mapping, validate() version check, Gate cleanup, scalar validation, template migration notes).
**Security review:** No blockers. Added scalar-only validation for when values.
**Strawman check:** Both rejected alternatives have genuine depth from advocate research.

## Current Status
**Phase:** 6 - Final Review (complete)
**Last Updated:** 2026-03-15
