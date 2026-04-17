# Design Summary: batch-rewind

## Input Context (Phase 0)
**Source:** Issue #137 (rewind does not clear materialized batch children)
**Problem:** koto rewind past a materialize_children state leaves stale child sessions on disk/cloud; re-submission appends rather than replaces.
**Constraints:** Cloud sync in scope — local rewind must propagate. Non-batch rewind must stay unchanged. Preserve progress where possible.

## Current Status
**Phase:** 0 - Setup (Freeform)
**Last Updated:** 2026-04-16
