# Design Summary: mid-state-decision-capture

## Input Context (Phase 0)
**Source:** Issue #68 (needs-design), spawned from DESIGN-shirabe-work-on-template
**Problem:** koto can't accept structured decision records mid-state without triggering the advancement loop. Agent decisions during implementation are invisible.
**Constraints:** Must decouple from advancement, be minimal engine change, backwards compatible, rewind-safe.

## Current Status
**Phase:** 0 - Setup (Freeform)
**Last Updated:** 2026-03-22
