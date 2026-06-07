# Scope: request-store-converge

Upstream: docs/briefs/BRIEF-request-store-converge.md (Accepted)
Visibility: Public (koto). Execution mode: --auto.

## Problem (from brief)

koto fans a workflow out to child workflows and learns *which* children
finished, but not *what* they produced. Completion records only a
terminal-state name; a coordinator that needs each child's outcome must
read the child's session log — reintroducing the context load the fan-out
was meant to avoid.

## Outcome (from brief)

A coordinator converges a fan-out by reading children's closed results
inline at a converge point, learning each outcome without opening any
child log. Completion carries a result; convergence is a read, not a
re-derivation. Uniform and recursive at every tree depth.

## What already ships (v0.10.0 — verified in repo, do NOT re-spec)

- `materialize_children` template hook + `koto session start --parent
  --needs-agent --role --template --inputs` create parent-linked children.
- `koto next` on the parent returns `unassigned_children[]`
  (`UnassignedChild` in src/cli/next_types.rs).
- `claim_and_dispatch` (epoch-fenced, stale-claim-timeout) claims a child
  (src/engine/claim.rs).
- `_terminal_index.jsonl` at koto-root: append-only JSONL, one line per
  terminal transition; `TerminalIndexEntry` has `session_id`,
  `terminal_at`, `header_mtime_ns`, `terminal_state`
  (src/engine/terminal_index.rs). Bounded to 4096 bytes/line (PIPE_BUF)
  for atomic multi-writer appends — the hot/compacted scan path.
- `EventPayload` closed enum includes `WorkflowInitialized`,
  `Transitioned`, `EvidenceSubmitted`, `ChildCompleted`,
  `BatchFinalized`, `GateEvaluated`, etc. (src/engine/types.rs).
  `ChildCompleted` already carries a typed `TerminalOutcome`
  (`success`/`failure`/`skipped`) and `final_state` — a state name and
  classification, NOT a result payload.
- `koto next` already returns `GateBlocked` with `blocking_conditions[]`
  and `unassigned_children[]` (src/cli/next_types.rs).
- `request_store.`-prefixed config/evidence namespace is RESERVED
  (src/config/mod.rs, src/engine/caps.rs `[request_store.recursion]`).

## The gap

Completion records a terminal-state NAME (+ outcome classification), not a
closed RESULT. So converging means reading child logs. The feature makes
workflow completion carry a typed closed result, surfaced to the parent at
a converge point via `koto next`. No new `koto request` command noun;
reuse children/gate/terminal-index. Uniform + recursive.

## Open decisions (carried to PRD Decisions and Trade-offs)

- C1: auto-promote a child's terminal evidence into its closed result vs.
  an explicit result-submission step.
- C2: result payload shape — free-form JSON vs. typed envelope.
- C3: where the result lives + how converge reads it (keep the index scan
  LEAN; result in child session, index carries a pointer; parent's
  converge directive dereferences and inlines).

## Public-content constraint

Frame everything in koto terms. Do NOT reference any private repo, issue
numbers, internal codenames (incl. any host-plane codename that appears in
source comments), or private paths.
