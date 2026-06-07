# Design Summary: request-store-converge

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-request-store-converge.md
**Problem (implementation framing):** koto records that a child finished (terminal-state name + typed outcome) but not what it produced. Convergence must carry a typed closed result on the completion path, store it without bloating the hot terminal-index scan, and surface it at the parent's converge point via the existing GateBlocked directive — no new command noun.

## Current Status
**Phase:** 0 - Setup (PRD)
**Last Updated:** 2026-06-07

## Open design decisions (from PRD)
- D1: result carried by completion path vs explicit submission step
- D2: typed minimal result envelope — exact field set + types
- D3: result storage location + pointer/dereference, index stays lean
