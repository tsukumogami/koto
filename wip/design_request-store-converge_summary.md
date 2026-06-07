# Design Summary: request-store-converge

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-request-store-converge.md
**Problem (implementation framing):** koto records that a child finished (terminal-state name + typed outcome) but not what it produced. Convergence must carry a typed closed result on the completion path, store it without bloating the hot terminal-index scan, and surface it at the parent's converge point via the existing GateBlocked directive — no new command noun.

## Current Status
**Phase:** 0 - Setup (PRD)
**Last Updated:** 2026-06-07

## Open design decisions (from PRD) — RESOLVED
- D1: auto-promote terminal evidence (no extra agent step)
- D2: typed minimal envelope {status: TerminalOutcome, summary: String, payload: Option<Value>}
- D3: result on child log (request_store.result event) + bounded has_result flag in index + result copy on parent ChildCompleted to survive cleanup; converge reads via children-complete gate + GateBlocked

## Security Review (Phase 5)
**Outcome:** Option 2 — document considerations (no design changes)
**Summary:** Limited attack surface; local append-only writes, no new deps/permissions/network. Load-bearing: defensive bounding/parsing of the agent-produced envelope and intentional in-trust-boundary duplication into the parent log; index concurrency preserved by carrying only a bounded boolean.

## Execution note
Phases 2 (decisions), 5 (security) run INLINE — subagents cannot spawn subagents.

## Current Status
**Phase:** 5 - Security
**Last Updated:** 2026-06-07
