# Design Summary: koto-template-authoring-skill

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** No guided way to create koto-backed skills. Template authoring requires understanding the YAML frontmatter schema, state machine semantics, evidence routing constraints, and file-system coupling conventions. A meta-skill that guides agents through authoring both the SKILL.md and its bundled koto template would lower the barrier and produce consistent output.
**Constraints:** Validation via `koto template compile` (not eval agents). Distribution via koto marketplace. Layered teaching (linear -> evidence routing -> advanced). Templates always bundled with skills.

## Current Status
**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-03-29
