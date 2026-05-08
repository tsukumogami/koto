# Design Summary: local-dashboard

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-local-dashboard.md
**Problem (implementation framing):** Add a live terminal UI to a synchronous Rust CLI
that reads session JSONL files, derives hierarchy from parent-child headers, and renders
a scrollable session tree with polling — without introducing an async runtime.

## Current Status
**Phase:** 1 - Decision Decomposition
**Last Updated:** 2026-05-07
