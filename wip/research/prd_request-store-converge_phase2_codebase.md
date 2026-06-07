# Phase 2 research (inline): codebase grounding

Subagents cannot spawn subagents, so Phase 2 research ran INLINE by
reading koto source directly. All facts below verified against the
worktree at branch docs/request-store-converge.

## Verified facts

1. **Terminal index** (`src/engine/terminal_index.rs`)
   - File `<koto_root>/_terminal_index.jsonl`, append-only JSONL.
   - `TerminalIndexEntry { session_id, terminal_at, header_mtime_ns,
     terminal_state }`. `terminal_state` is `"completed"` /
     `"abandoned"` — a classification, not a result.
   - Lines bounded to `MAX_INDEX_LINE_BYTES = 4096` (PIPE_BUF) so
     concurrent appends from independent agents are atomic. This is the
     hot discovery-scan path; the reader walks every line into a dedup
     HashMap on each tick. Conclusion: the index must stay LEAN — a
     result payload of arbitrary size cannot live in the index line.
   - Reader tolerates extra unknown keys (additive forward-compat).

2. **EventPayload closed enum** (`src/engine/types.rs`)
   - Variants: WorkflowInitialized, Transitioned, EvidenceSubmitted,
     GateEvaluated, BatchFinalized, ChildCompleted, IntentUpdated,
     Unknown (deserialize-only catch-all).
   - `ChildCompleted { child_name, task_name, outcome: TerminalOutcome,
     final_state }`. `TerminalOutcome` is a typed enum serialized
     snake_case: `success` / `failure` / `skipped`. So koto ALREADY has
     a typed outcome classification on completion — but NOT a result
     payload (summary + structured data) the parent can read.
   - `Unknown` variant gives graceful degradation for newer event types;
     a new converge event family is additive-safe for old readers.

3. **`koto next` output** (`src/cli/next_types.rs`)
   - `NextResponse` includes `GateBlocked { blocking_conditions[],
     unassigned_children[], ... }`. `BlockingCondition` and
     `UnassignedChild` are existing structured surfaces.
   - A converge gate that stays `GateBlocked` until child results are in,
     then surfaces them inline, fits this existing shape — no new
     top-level response noun required.

4. **Reserved namespace** (`src/config/mod.rs`, `src/engine/caps.rs`)
   - `[request_store.recursion]` config namespace is RESERVED and
     pre-staked; `request_store.`-prefixed evidence kinds are reserved.
   - This is where converge result wiring naturally attaches without a
     new command noun.

5. **Dispatch half** (`src/engine/claim.rs`, `discovery.rs`, `next.rs`,
   `init_child.rs`, `template/compile.rs`) — fan-out machinery exists and
   is OUT of scope. Verified `claim_and_dispatch`, `unassigned_children`,
   `materialize_children` references resolve to real code.

## Design-altitude implications (for PRD Decisions, not to resolve here)

- The result payload cannot ride inside the index line (4096-byte bound,
  hot scan). Strongly favors: result stored with the child session; index
  carries at most a pointer/flag; parent's converge directive
  dereferences and inlines. (Feeds C3.)
- `ChildCompleted` already proves a typed envelope is the koto idiom
  (typed outcome enum chosen over stringly-typed for exhaustive matching).
  Favors a typed-but-minimal result envelope over free-form JSON. (Feeds
  C2.)
- Terminal evidence is already written on the completion path
  (`koto next --with-data` terminal-evidence writer). Auto-promoting that
  evidence into the closed result avoids a second agent round-trip.
  (Feeds C1.)

## Public-content note

`src/engine/caps.rs` contains an internal host-plane codename in a comment.
That codename and any private repo/issue references MUST NOT appear in the
PRD. The PRD frames everything as koto's own substrate-agnostic behavior.
