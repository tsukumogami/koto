# Explore Scope: advanced-field-ux

## Visibility

Public

## Core Question

The auto-advancement engine exists and works, but there's no authoritative spec for its behavioral contract. Callers misinterpret the `advanced` field and lack clear guidance on what each `koto next` response shape means and what they should do with it. We need to identify what's already documented, where the gaps are, and produce a PRD that pins down the contract so behavior can't drift by accident.

## Context

Issue #102 reported that the `advanced` field name misleads callers into thinking phases are "pre-cleared" rather than "newly entered." Tracing the code revealed `advanced` is a per-invocation boolean ("did the engine transition during this call?"), but this isn't documented anywhere callers can find it. The user wants a PRD covering the full auto-advancement behavioral contract, not just a field rename.

## In Scope

- The full `koto next` lifecycle: auto-advancement loop, output shapes, caller expectations
- Adjacent commands (`koto transition`, `koto submit`) if they interact with auto-advancement
- The `advanced` field semantics and naming (issue #102)
- Gap analysis: what's specified in docs vs. what's only in code/tests
- Edge cases: cycle detection, chain limits, signal handling, gate-with-evidence-fallback

## Out of Scope

- Template authoring guidance (how to write templates)
- Internal engine refactoring (implementation changes)
- Version provider or recipe systems

## Research Leads

1. **What do existing design docs already specify about auto-advancement behavior?**
   DESIGN-auto-advancement-engine.md, DESIGN-unified-koto-next.md, and DESIGN-koto-cli-output-contract.md may already cover parts of the contract. Need to map what's specified vs. what's implicit or missing.

2. **What does the AGENTS.md caller documentation promise about `koto next` responses?**
   This is what callers actually read. Need to check whether it matches the implementation and whether it covers edge cases callers encounter.

3. **What behavioral invariants does the test suite encode that aren't in any doc?**
   Integration tests and functional feature files may pin down behavior (cycle detection, chain limits, gate-with-evidence-fallback) that no prose document captures.

4. **How do `koto transition` and `koto submit` interact with auto-advancement state?**
   Do they trigger the advancement loop? Do they produce the same output shape with `advanced`? Are there contract gaps?

5. **What edge cases exist in the advancement loop that callers can encounter but aren't documented?**
   Cycle detection, chain limits, signal handling, UnresolvableTransition, ActionRequiresConfirmation. Are these in any spec, or only in code?
