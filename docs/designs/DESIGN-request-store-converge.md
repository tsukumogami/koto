---
status: Proposed
upstream: docs/prds/PRD-request-store-converge.md
problem: |
  koto v0.10.0 ships the fan-out half of coordinator-and-delegates: child
  workflows are created, linked, discovered, claimed, and recorded in the
  workspace-wide terminal index. But a child's completion records only a
  terminal-state name and a typed outcome classification, never the closed
  result the child reached. A coordinator that fanned work out must reopen
  each child's session log to learn what it produced, reintroducing the
  working-context load the fan-out existed to remove. The converge half is
  missing, and it must be added without bloating the hot terminal-index scan
  path or introducing a new top-level command noun.
decision: |
  A child's result is auto-promoted from the terminal evidence the completion
  path already writes, into a typed minimal envelope (status + summary +
  optional payload) persisted as a `request_store.result` event on the
  child's own session log. The terminal index gains one bounded additive
  field — a has_result flag — so the hot scan path stays lean and the full
  result is dereferenced lazily only at the converge point. The parent's
  converge point reuses the existing children-complete gate and the
  GateBlocked directive: blocked while any child in the converge set has no
  result, then the cleared directive inlines every child's result. The
  primitive is uniform and recursive — a child converges its own children and
  carries its own result up identically at every depth.
rationale: |
  Every element rides machinery koto already ships. Auto-promotion keeps R3
  true (no extra agent step) and reuses the terminal-evidence write the
  completion path already performs. A typed envelope matches koto's existing
  typed-outcome idiom so a parent reads any child uniformly. Storing the full
  result on the child session and keeping only a bounded flag in the index
  respects the PIPE_BUF line bound that makes concurrent appends atomic.
  Reusing the children-complete gate and GateBlocked means no new response
  variant and no new command noun, and convergence inherits the engine's
  forward-compatible event handling, atomic appends, and epoch fencing
  unchanged.
---

# DESIGN: request-store-converge

## Status

Proposed

## Context and Problem Statement

koto's coordinator-and-delegates model lets one workflow fan work out to child
workflows. As of v0.10.0 the dispatch half is complete: `materialize_children`
spawns child sessions linked to a parent (`parent_workflow` in the child
header), the parent's discovery scan surfaces children needing an agent
(`unassigned_children` in `koto next`), agents claim and drive children through
an epoch-fenced claim path, and a child that reaches a terminal state is
recorded in the workspace-wide terminal index (`_terminal_index.jsonl`). The
`children-complete` gate already lets a parent block until its batch finishes
and reports per-task counts (`pending`, `failed`, `skipped`, `all_complete`).

The technical gap is at the value boundary, not the control boundary. When a
child completes, two records are written: a `ChildCompleted` event appended to
the *parent's* log (carrying a typed `TerminalOutcome` of `Success` / `Failure`
/ `Skipped` plus the child's `final_state` name) and a `TerminalIndexEntry`
appended to the workspace index (carrying `session_id`, `terminal_at`,
`header_mtime_ns`, `terminal_state`). Neither carries the *result* the child
reached — the decision it made, the summary of its work, any structured payload
a downstream consumer needs. A coordinator that fanned out three evaluations
and wants to converge them can learn that all three are done, but to learn what
each decided it must open and replay each child's session log. That replay is
the exact working-context cost the fan-out exists to avoid.

The design must close this gap inside koto's existing engine substrate. Four
constraints make it a non-trivial architecture problem rather than a localized
change:

1. **The result has to be carried by the completion the agent already
   performs** — adding a mandatory extra agent round-trip would defeat the
   fan-out and create a failure mode where a workflow completes with no result.
2. **The terminal-index scan path is the hot path.** The discovery scan walks
   `_terminal_index.jsonl` line-by-line on every parent poll, and each line is
   bounded to `MAX_INDEX_LINE_BYTES` (4096, within Linux `PIPE_BUF`) so that
   concurrent `O_APPEND` writes from independent agents never interleave. An
   arbitrary-size result payload cannot live in an index line.
3. **No new top-level command noun and no new `koto next` response variant.**
   The reserved `request_store` config namespace already anticipates wiring
   convergence into existing structures; the converge point must reuse the
   `GateBlocked` directive surface.
4. **The schema must be forward-compatible** with koto's closed-enum +
   `Unknown` fallthrough event model, its NDJSON append-only logs, and its
   tempfile-rename atomic writes.

The source requirements for this design are recorded in the upstream PRD
(`docs/prds/PRD-request-store-converge.md`, R1–R11, AC1–AC11), which fixes the
constraints and defers three architectural decisions to this design: how the
result is promoted (D1), the exact envelope field set (D2), and where the
result is stored and how the converge point dereferences it while keeping the
index lean (D3).

## Decision Drivers

- **No extra agent step (PRD R3, AC3).** Recording a result must ride the
  completion the agent already signals. The terminal-evidence write and the
  `ChildCompleted` append already happen on that path.
- **Uniform typed read across all children (PRD R2, R6, AC2, AC7).** A parent
  reads any child's result through one accessor with no per-child special
  casing, at every tree depth. This favors a typed envelope over a free-form
  blob, mirroring koto's existing typed `TerminalOutcome`.
- **Lean hot scan path (PRD R9, AC9).** The terminal-index line stays within
  `MAX_INDEX_LINE_BYTES` regardless of result payload size. Whatever the index
  carries must be bounded; the full result is dereferenced elsewhere.
- **Reuse existing surfaces; no new noun (PRD R4, R5, R7, D4, AC6).** The
  converge point is the `children-complete` gate; the directive is
  `GateBlocked`; the storage substrate is the session event log and the
  terminal index. No `koto request` command, no new `NextResponse` variant.
- **Forward-compatible, additive, concurrency-safe (PRD R10, R11, AC10,
  AC11).** New events fall through the `Unknown` arm on older koto builds; new
  struct fields are `#[serde(default, skip_serializing_if)]`; result writes
  preserve the atomic-append / atomic-rename discipline so N concurrent
  completions never corrupt one another.
- **Standalone koto value.** A solo coordinator converges with no companion
  plugins or multi-repo workspace. The design depends only on koto's own engine
  and CLI; teaching and provisioning layers are out of scope.
- **Convergence consumes results, never transcripts (PRD R8, AC8).** The result
  is the legible end-of-work artifact; the dereference path reads the recorded
  result event, never replays the child's working log.
