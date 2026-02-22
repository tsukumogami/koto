# Documentation Plan: koto-engine

Generated from: docs/designs/DESIGN-koto-engine.md
Issues analyzed: 7
Total entries: 4

---

## doc-1: README.md
**Section**: (multiple sections)
**Prerequisite issues**: #9
**Update type**: modify
**Status**: completed
**Details**: Update README with project description, installation instructions, and basic usage examples showing the core CLI workflow (init from template, next to get directive, transition to advance, query/status to inspect). Requires #9 because the full CLI surface must be in place before documenting usage end-to-end.

---

## doc-2: docs/guides/cli-usage.md
**Section**: (new file)
**Prerequisite issues**: #9
**Update type**: new
**Status**: completed
**Details**: Create a CLI usage guide covering all subcommands: init, next, transition, query, status, rewind, cancel, validate, workflows. Include the --state flag and auto-selection behavior when one state file exists. Show JSON output examples for agent-facing commands and text output for human-facing commands. Requires #9 because that issue wires the remaining subcommands and completes the CLI surface.

---

## doc-3: docs/guides/library-usage.md
**Section**: (new file)
**Prerequisite issues**: #5, #6
**Update type**: new
**Status**: completed
**Details**: Create a Go library usage guide for consumers who import pkg/engine directly. Cover constructing a Machine programmatically, calling Init/Load/Transition/Rewind/Cancel, reading state with query methods, and handling TransitionError. Requires #5 (completes Engine API with rewind/cancel/query) and #6 (finalizes error types that library consumers handle).

---

## doc-4: docs/reference/error-codes.md
**Section**: (new file)
**Prerequisite issues**: #6
**Update type**: new
**Status**: completed
**Details**: Create an error code reference documenting all TransitionError codes (terminal_state, invalid_transition, unknown_state, template_mismatch, version_conflict, rewind_failed), their JSON shape, when each occurs, and how agents should handle them. Requires #6 because that issue implements the full error type system and JSON serialization.
