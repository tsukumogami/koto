# Design summary: auto-advance-response-contract

## Input context (Phase 0)
**Source:** /explore handoff (3 rounds, 11 research agents)
**Problem:** `koto next` forces callers to double-call when auto-advancement occurs, because the `advanced: true` response flag is ambiguous about whether the caller needs to act. The engine's stopping conditions and response contract need updating.
**Constraints:** backward compatibility required (can't remove `advanced`), engine layer owns the advancement loop, response stays lean (observability in event log)

## Current status
**Phase:** 0 - Setup (Explore Handoff)
**Last updated:** 2026-03-26
