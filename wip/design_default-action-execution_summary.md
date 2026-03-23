# Design summary: default-action-execution

## Input context (Phase 0)
**Source:** Issue #71, spawned from DESIGN-shirabe-work-on-template.md Phase 0b
**Problem:** koto can verify outcomes via gates but can't execute deterministic work.
Five work-on template states need auto-executing default actions.
**Constraints:** two execution models (one-shot + polling), reversibility safety,
output capture for fallback, variable substitution, override prevention

## Current status
**Phase:** 0 - Setup (Freeform, --auto)
**Last Updated:** 2026-03-22
