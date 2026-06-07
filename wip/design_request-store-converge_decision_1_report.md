# Decision 1: How is a child's result recorded on completion?

Executed INLINE (subagents cannot spawn subagents).

## Question
Auto-promote the terminal evidence the completion path already writes into
the closed result, or require an explicit separate result-submission step.

## Options
- **1A — Auto-promote terminal evidence.** When a child reaches a terminal
  state, the completion path synthesizes a `request_store.result` event from
  the evidence and template flags it already has: outcome from the same
  `TerminalOutcome` projection used for `ChildCompleted` (failure /
  skipped_marker flags on the final state), summary from a designated evidence
  field (or a default derived from the final state), payload from the latest
  `EvidenceSubmitted.fields`. No extra agent call.
- **1B — Explicit result-submission step.** The agent calls a dedicated
  command (e.g. `koto next --with-data` against a result schema, or a new
  `koto result post`) before/at completion to attach the result.

## Chosen: 1A — auto-promote terminal evidence

The completion path already does two writes on the terminal tick:
`append_child_completed_to_parent` (projects `TerminalOutcome` from the final
state's `failure` / `skipped_marker` flags) and `append_terminal_index_for_session`.
Both run inside `handle_next` just before `backend.cleanup(child)`. Promotion
reuses that exact projection: the result's status is the same enum value, the
summary is read from a conventionally-named evidence field on the terminal
state's `accepts` block (falling back to a templated default), and the optional
payload carries the terminal `EvidenceSubmitted.fields`. The agent does nothing
new — it submits its terminal evidence the way koto already expects, and the
result is synthesized from that.

This keeps PRD R3 / AC3 true and removes the failure mode where a workflow
completes with no result (1B's main risk: an agent forgets the extra step).

## Rejected: 1B — explicit submission step
Adds an agent round-trip the fan-out exists to avoid, and introduces a state
where a child is terminal but has no result (every converge would have to
tolerate "done but resultless", complicating R5's blocked-set semantics).
A new `koto result post` noun also conflicts with PRD D4 / R7 (no new noun).

## Confidence: high. PRD D1 already assumed this under --auto; the code path
(`append_child_completed_to_parent`) is the natural promotion site.
