# Exploration Summary: koto Template Format (v2)

## Problem (Phase 1)

The previous design attempt tried to find a single format serving both deterministic machine parsing and human authoring. This created cascading complexity: heading collision, declared-state matching, dual transition sources, TOML-vs-YAML debates. These are symptoms of conflating two separate concerns. The real problem has two parts: (1) koto needs a machine-readable, fully structured, deterministic canonical format for state machines, and (2) humans need a way to author, edit, and understand template definitions that is natural to write and renders well in tools like GitHub. These don't need to be the same format.

## Decision Drivers (Phase 1)
- The canonical (machine) format must be deterministic to parse with zero ambiguity
- The human authoring format must be readable, writable, and render well on GitHub
- Conversion from human format to canonical format must be deterministic
- LLMs may assist at the validation layer (fixing input) but NOT in the parsing path
- Zero external dependencies for the core engine (parsing the canonical format)
- The human format should support rich directive content (markdown with tables, code blocks, headings)
- Backward compatibility with existing templates is a nice-to-have, not a hard requirement
- Progressive complexity: simple templates should be simple to author

## Research Findings (Phase 2)
- Previous research still valid: YAML frontmatter + markdown is the industry standard for single-format tools
- New research: investigating dual-format patterns (Terraform HCL/JSON, Protocol Buffers, CUE, MDX)
- Key question: does any tool in the AI agent space use a compiled template approach?

## Current Status
**Phase:** 1 - Problem reframed, Phase 2 research in progress
**Last Updated:** 2026-02-22
