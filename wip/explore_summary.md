# Exploration Summary: koto Template Format (v3)

## Problem (Phase 1)

koto needs a programming language for state machines. The source format (what humans write and store) and the execution format (what the engine reads) don't need to be the same -- like source code and compiled binaries. The v1 design conflated these, creating heading collision, format debates, and parsing fragility. The v2 design separated them but wasn't clear about which format is the primary artifact. v3 establishes the source format as the artifact of record -- what gets stored, versioned, and shared -- with one-way deterministic compilation to JSON for the engine.

## Decision Drivers (Phase 1)
- The source format is the primary artifact (stored, versioned, shared)
- Compilation from source to JSON must be deterministic
- The source format must be readable, writable, and render on GitHub
- LLMs may assist at the validation layer but NOT in the compilation path
- Zero external dependencies for the core engine (reads compiled JSON)
- Progressive complexity: simple templates should be simple to author

## Research Findings (Phase 2)
- Source/compiled separation is well-established: programming languages, Terraform, Protocol Buffers
- YAML frontmatter + markdown is the industry standard for human-authored structured documents
- No markdown schema exists -- structure validation requires a structured format (YAML/JSON)
- No AI agent workflow tool uses compiled templates; koto would be first

## Decision (Phase 5)

**Problem:**
koto templates must serve two audiences: humans who write and maintain workflow definitions, and the engine that executes them deterministically. The v1 design tried a single format for both, creating heading collision, dual transition sources, and parsing fragility. These problems stem from conflating the source format with the execution format.

**Decision:**
Template source files (.md with YAML frontmatter) are the primary artifact -- stored, versioned, and shared. A deterministic compiler produces JSON for the engine to read at runtime. Compilation is one-way, like a programming language compiler. The engine reads compiled JSON using only Go's stdlib (zero dependencies). Evidence gates (field_not_empty, field_equals, command with 30s default timeout) are declared per state, evaluated on exit. Evidence persists across rewind. CLI commands, search paths, and LLM-assisted validation are deferred to a separate tooling design.

**Rationale:**
The source/compiled separation eliminates heading collision (JSON has no headings) and format debates (each format does what it's designed for). The "programming language" model is familiar to every developer. YAML frontmatter is the industry standard for structured metadata in markdown documents. JSON as the compiled target uses Go's stdlib, keeping the engine dependency-free. The compilation step is invisible in practice -- koto init compiles in memory.

## Current Status
**Phase:** 8 - Design complete, scope narrowed to format spec only (CLI/tooling deferred)
**Last Updated:** 2026-02-22
