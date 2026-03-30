# Design Summary: koto-next-output-contract

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-koto-next-output-contract.md
**Problem (implementation framing):** The koto next serialization layer collapses four response variants into action: "execute", conflates error codes across fixable/unfixable categories, discards gate results on evidence-fallback, and lacks conditional directive inclusion. Changes span src/cli/ (Serialize impl, handler mapping, NextErrorCode), src/engine/ (StopReason threading), src/template/ (details field), and plugins/koto-skills/ (docs + koto-author).

## Current Status
**Phase:** 0 - Setup (PRD)
**Last Updated:** 2026-03-30
