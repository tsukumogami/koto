# Design Summary: backend-state-persistence

## Input Context (Phase 0)
**Source:** Freeform (bug in CloudBackend — state I/O bypasses backend)
**Problem:** 16 direct state file I/O calls in CLI layer bypass SessionBackend. Cloud sync never sees state changes.
**Constraints:** Don't change JSONL format. Don't break LocalBackend. Minimize call site changes.

## Current Status
**Phase:** 0 - Setup
**Last Updated:** 2026-03-28
