# Exploration Summary: koto State Machine Engine

## Problem (Phase 1)
AI coding agents running multi-step workflows lack reliable execution control -- they skip steps, lose progress across sessions, and can't resume predictably. The root cause is that workflow state lives in the agent's context window (lost on restart) or in file-existence heuristics (fragile). koto needs a state machine engine that enforces execution order and persists state atomically.

## Decision Drivers (Phase 1)
- Library-first design (importable Go package, not just CLI backend)
- Atomic persistence (survive process crashes)
- Multiple concurrent workflows in same directory
- Extensible transition model (blank transitions now, evidence gates later)
- Progressive disclosure (agent sees only current state)
- Machine-parseable JSON output for all agent-facing operations
- Resumability from any state after interruption
- Template integrity (detect mid-workflow modification)

## Research Findings (Phase 2)
- Industry tools split on state location: home directory (Claude Code, Gemini CLI) vs project directory (Beads, TaskMaster). koto abstracts this -- engine takes a file path.
- JSONL with hash-based IDs (Beads) is best for git-tracked state, but koto state files are ephemeral, so single-file JSON is fine.
- No file-based tool has truly solved concurrency. Atomic writes + version counter is sufficient for Phase 1.
- StateFlow (COLM 2024) validates evidence-gated transitions academically: 13-28% higher success than ReAct at 3-5x lower cost.
- Go libraries: qmuntal/stateless has external state storage API but adds indirection; custom ~200 lines is simpler and sufficient.

## Options (Phase 3)
- State machine: custom ~200 lines vs qmuntal/stateless vs looplab/fsm -> custom chosen
- Concurrency: atomic writes + version counter vs file locking vs single-writer -> version counter chosen
- Package layout: four packages (engine, controller, template, discover) vs single package -> four packages chosen
- Rewind: reset with history vs truncate history vs no rewind -> reset with history chosen
- Integrity: template hash + version vs full hash chain vs none -> template hash + version chosen

## Decision (Phase 5)

**Problem:**
AI coding agents running multi-step workflows lack reliable execution control. They skip prescribed steps, lose progress across session boundaries, and can't resume interrupted work predictably. The root cause is that workflow state lives either in the agent's context window (lost on restart) or in file-existence heuristics (fragile and easily confused). koto needs a state machine engine that enforces correct execution order and persists state atomically so any interruption is recoverable.

**Decision:**
Build a custom state machine engine as a Go library under pkg/engine/ with three companion packages: controller (directive generation), template (parsing), and discover (state file location). The engine uses a flat JSON state file with optimistic versioning, atomic writes via write-to-temp-rename, and template integrity verified via SHA-256 hash on every operation. Phase 1 handles blank transitions (state advancement without evidence requirements). Evidence gates will be designed and added separately.

**Rationale:**
A custom implementation (~200 lines for the core) avoids external dependencies for logic that's fundamentally simple -- a map of states with allowed transitions. The complexity lives in persistence and template parsing, not transition logic. Optimistic versioning detects concurrent modification without the cross-platform complexity of file locking. Starting with blank transitions lets us get the state machine mechanics right before designing the evidence system, which has its own set of decisions (gate types, accumulation semantics, rewind behavior).

## Current Status
**Phase:** 8 - Final Review
**Last Updated:** 2026-02-21
