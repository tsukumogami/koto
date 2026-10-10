# Design Summary: session-view

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-session-view.md
**Problem (implementation framing):** The dashboard, one-shot feed and
workflow file must show a session's full holdings (facts + every key with
size and legible content) without ContextStore access they lack today,
without the cloud get's side effects, and without breaking three frozen
contracts (feed columns, workflow-file fields, top-level verb set).

## Current Status
**Phase:** 1 - Decomposition
**Last Updated:** 2026-10-10
