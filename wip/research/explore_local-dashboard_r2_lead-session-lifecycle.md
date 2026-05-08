# Lead: koto session lifecycle duration and check-in patterns

## Findings

### Template State Counts & Complexity
Real koto templates cluster into three categories:
- **Simple (2-5 states)**: `hello-koto.md` (2), `code-review.md` (5), `batch-worker.md` (3)
- **Medium (6-8 states)**: `koto-author.md` (8), `complex-workflow.md` (7), `coord` batch coordinator (4)
- **Batch (distributed)**: Coordinator holds 2-4 states; each worker is 1-3 states; total parallelism depends on `waits_on` dependency structure

### Session Duration Patterns
Sessions measured by CLI invocation cadence and total workflow time:

1. **Simple linear** (code-review.md, no gates, no retries): 
   - ~5-10 `koto next` calls to terminal
   - Completion: 2-5 minutes (human agent time to read directive, do work, submit evidence per state)

2. **Medium with gates & retries** (koto-author.md, 8 states, validation loops):
   - ~15-30 `koto next` calls (average 2-3 calls per state when gates fail and loop back)
   - Completion: 15-45 minutes

3. **Batch with 100 children** (linear dependency chain):
   - Parent: 1 submit + ~100 re-ticks (one per child completion to unlock next tier) + final aggregate = ~102 parent calls
   - Per child: 1-5 calls depending on child template complexity
   - Total runtime: hours (sequential execution of dependency tiers; parallelism within each tier)

### Gate & Polling Behavior
- **No automatic background polling**: Each gate is evaluated on-demand during a `koto next` call
- **Self-loops exist** (e.g., `preflight` state waits for config file; agent calls `koto next` repeatedly until gate passes)
- **children-complete gates**: Coordinator evaluates on each `koto next` call; output includes per-child status (total, completed, pending, success, failed, skipped, blocked)
- **Batch coordination**: Parent must re-tick *after* each child completes to re-derive the scheduler's ledger and unlock dependencies. No automatic child spawning—workers are dispatched by the parent's agent.
- **No documented retry backoff**: Guidance says "Don't retry in a tight loop," but no built-in exponential backoff or timeout

### Evidence Submission Cadence
Agent loop is entirely pull-driven:
```
koto init workflow --template <path>
while true:
  response = koto next workflow
  action = response.action
  if action == "done": break
  if action == "evidence_required":
    (agent does work)
    koto next workflow --with-data @evidence.json
  else if action == "gate_blocked":
    (agent waits or fixes blocker)
    koto next workflow  # re-check gates
```
- **No built-in polling interval**: Agent controls cadence
- **No automatic ticking**: Batch coordinator doesn't wake up on child completion; agent must call `koto next coord` after observing or being notified of child progress
- **Batch parent re-ticking**: Agent must re-dispatch workers as materialized_children status changes (ready_to_drive transitions from false → true when dependencies unblock)

### Session State Durability
- State stored at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl` (append-only JSONL event log)
- Can read current state by replaying events; can resume from any prior point via `koto rewind`
- No background daemon holding state in memory; all I/O is file-based
- Cloud sync available (S3-compatible backend) but manual—not automatic background syncing

## Implications

### Ad-Hoc Dashboard Suffices For
- **Simple workflows** (5-30 calls, 2-30 minutes): Agent launches dashboard to see what state they're in, what directive the current state has, and what gates are blocking. Then closes it.
- **Minimal overhead**: No daemon needed to monitor; state is already on disk where the agent can inspect it
- **Human-paced work**: Since agents are LLM-driven and each state takes 1-5 minutes, a TUI invoked on-demand is adequate

### Daemon (Always-On) Would Be Useful For
- **Long-running batch workflows (hours)**: 100+ children with multi-tier dependencies need close monitoring. Parent must re-tick frequently to unlock downstream tasks.
- **Visibility into child progress in real-time**: Dashboard-as-daemon could display per-child status continuously without requiring agent to call `koto status` manually
- **Automated re-dispatching**: A daemon could watch materialized_children and auto-spawn workers as dependencies unblock, eliminating the agent's coordination overhead
- **Historical reconstruction**: For failed batch parents, a daemon could have logged every parent re-tick, making post-mortem analysis easier

### Reality Check: Current Usage Pattern
The typical koto workflow is **agent-driven polling**. An AI agent:
1. Calls `koto init` once
2. Enters a loop: `koto next` → read directive → do work → `koto next --with-data` → repeat
3. For batch parents: re-ticks parent after each child completes to update the scheduler ledger

There is no documented "always-on observability" use case yet. Observability is implicit in the event log; tools like `koto workflows` and `koto status` provide snapshot views, but no real-time monitoring.

## Surprises

1. **No automatic batch scheduling**: Batch children are not spawned automatically by the system. The parent's agent must dispatch them via `koto next coord.task-1`, etc. This means batch workflows are truly agent-driven end-to-end, not system-scheduled.

2. **Guidance against tight polling**: The docs explicitly warn "Don't retry in a tight loop. Children are running their own workflows and need time." Yet no backoff mechanism is built in. This suggests operators are expected to implement their own exponential backoff or use external orchestrators.

3. **Event log is the only source of truth**: No `workflow_completed` event. Consumers must inspect the final `transitioned` event and check if its `to` state matches the template's declared terminal states. This is noted as a "known gap" in the session-feed contract.

4. **Dispatcher classification order matters**: The dispatcher evaluates gates *before* checking if the state has an `accepts` block. So a state with both gates and accepts will return `evidence_required` (not `gate_blocked`) if gates fail—the agent can override evidence. This is a UX win but adds complexity to the routing logic.

5. **Templates guide batch structure but don't enforce it**: The `materialize_children` hook and `children-complete` gate are optional. A coordinator could manually manage children using plain `tasks` accepts fields and per-child evidence routing, abandoning the batch scheduler entirely.

## Open Questions

1. **Batch parent re-tick cadence**: Is there a recommended interval for how often a parent should re-tick? The docs say "don't retry in a tight loop" but don't define what "tight" means. 100 milliseconds? 1 second? Should operators use exponential backoff?

2. **Dependency unlocking latency**: When a child completes, how quickly should the parent re-tick to unlock its dependents? Is there a benchmark for the overhead of a parent tick (e.g., time to re-derive the scheduler's ledger)?

3. **Long-running sessions and drift**: If a batch parent with 100+ children runs for 8 hours, does the event log grow unbounded? Are there compaction or rotation policies?

4. **Cloud sync consistency**: The session-feed contract mentions "conflict resolution" for cloud sync (if two machines modify the same session). What's the typical scenario? Is it expected that multiple agents might drive the same workflow?

5. **Real-world batch sizing**: Are there any known limits or performance characteristics for batch sizes? Is 100 children typical or extreme? Is the scheduler O(n) on number of children per tick?

6. **Integration runners**: The response shapes document mentions "integration" action types with an `integration_unavailable` fallback. Are any integration runners planned (e.g., for CI/CD systems, external APIs, approval workflows)?

## Summary

Most koto sessions are short (10-30 CLI calls, 2-30 minutes) and entirely agent-driven; a launch-on-demand TUI is sufficient for typical use. Batch workflows with 100+ children running for hours would benefit from always-on monitoring to coordinate parent re-ticking and worker dispatch, but there is no explicit use case documented yet, and current batch orchestration relies on the agent's explicit re-ticking of the parent and dispatching of workers. The gap is between "ad-hoc dashboard for snapshot views" (works today) and "real-time batch supervisor" (not yet designed).

