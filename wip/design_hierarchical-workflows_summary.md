# Design Summary: hierarchical-workflows

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** koto's per-workflow state machine has no parent-child awareness, forcing consumers to build external orchestrators that duplicate state tracking when workflows need to fan out over collections.
**Constraints:** koto is a contract layer (doesn't launch agents), advance loop changes must be minimal, both Local and Cloud backends must work, backward compatibility required for existing workflows.

## Current Status
**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-04-04
