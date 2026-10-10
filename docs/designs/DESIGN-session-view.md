---
upstream: docs/prds/PRD-session-view.md
status: Proposed
problem: |
  Placeholder — completed in Phase 6.
decision: |
  Placeholder — completed in Phase 6.
rationale: |
  Placeholder — completed in Phase 6.
---

# DESIGN: A human-readable view of a session

## Status

Proposed

## Context and Problem Statement

A session's working state is spread across stores the shipped surfaces never
join up for a reader. The context store holds every key's bytes, with a
manifest (`ctx/manifest.json`) that already records each key's size, SHA-256,
writer and creation time. The state log holds the header (identity, intent,
`execution_dir`, `origin{anchor, store}`) and the events the current state is
derived from. The compiled template holds the directive text `handle_status`
interpolates. The dashboard (`src/cli/dashboard*.rs`) renders a multi-session
list and a per-session detail pane (Summary, History, Remaining) from state
files it reads by path, never touching the context store; its one-shot mode
prints a fixed 8-column feed. The workflow view (`src/workflows_surface/`)
projects each session into a `koto-<uuid>.json` status file on every commit.

The PRD asks these surfaces to answer "what does this session hold?" for a
person: every manifest key with size, writer and legible content (text
excerpted to a bound with a visible truncation marker; binary — invalid UTF-8
in a sampled prefix, or NUL — shown as type, size and hash); the session facts
(name, id, intent, template, state, directive, anchor, store origin) with
explicit absent markers; plain statements for unreadable keys and for migrated
sessions (naming the successor and workspace); all of it read-only, bounded in
output, with a fixed remote-request count for metadata on cloud sessions, with
the existing feed columns and workflow-file fields untouched, and with no new
top-level verb or subcommand tree.

The technical problems that creates:

1. **Data access.** The dashboard is handed `&dyn SessionBackend` and reads
   state files by path, so it has neither `ContextStore` access nor migration
   awareness; `handle_status`'s directive interpolation is trapped in a
   function that exits the process on error. Reading content through the cloud
   backend's `get` pulls objects into the local store and appends events — a
   side effect a read-only view must handle deliberately.
2. **Placement.** The TUI detail pane, the one-shot mode and the workflow file
   each have an existing shape (tabs; a positional column contract; a
   version-numbered JSON contract with a shape-guard fixture) that the view
   must extend without breaking.
3. **Legibility.** koto has cut-and-flag precedents (`redact::safe_cut_len`,
   capture truncation flags, the workflow view's 240-character preview) but no
   size formatter, no binary detection, and no control-character sanitizer
   beyond tab/newline/CR in the feed path.

## Decision Drivers

- **Completeness without blocking.** Every manifest key renders (PRD R2), and
  the TUI stays responsive under its 500ms refresh while content loads
  (R6) — so the data path must separate cheap metadata from per-key content.
- **Read-only.** The view adds no writes and no per-key event spam (R10,
  AC11); the cloud `get` side effects force an explicit read policy (R9).
- **Bounded everything.** Output bounded per value and per invocation (R3,
  N1); metadata cost on cloud fixed regardless of key count — one manifest
  fetch and one migration check (N2).
- **Contract stability.** The feed's column positions (R7, AC4), the workflow
  file's existing fields (R8, AC7), and the top-level verb set (N4, AC9) are
  all frozen; every change is additive or behind a new flag.
- **Migration is an answer, not an error.** A migrated session renders its
  successor's name and workspace (R5, AC6), which requires the view to reach
  the backend's marker check rather than bypass it.
- **Terminal safety.** Arbitrary bytes reach a terminal only escaped (R3,
  AC10); the TUI's ratatui cell buffer protects one path, the one-shot
  stream has no protection today.
- **Maintainability.** The session-facts derivation should land where both
  surfaces and `handle_status` can share it, not as a third copy of state
  derivation.

## Considered Options

<!-- Phase 3 fills this from the decision reports. -->

## Decision Outcome

<!-- Phase 4 fills this. -->

## Solution Architecture

<!-- Phase 4 fills this. -->

## Implementation Approach

<!-- Phase 4 fills this. -->

## Security Considerations

<!-- Phase 5 fills this. -->

## Consequences

<!-- Phase 4/6 fill this. -->
