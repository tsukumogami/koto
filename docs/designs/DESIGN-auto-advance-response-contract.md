---
status: Proposed
problem: |
  When koto's engine auto-advances through states, the CLI returns a response
  with `advanced: true` that tells callers "something moved" but not whether
  they need to act. Every caller works around this by immediately calling
  `koto next` again. The double-call is mechanical overhead with no decision
  value. The engine's stopping conditions and the response contract both need
  updating to eliminate the pattern and provide useful observability instead.
---

# DESIGN: Auto-advance and response contract evolution

## Status

Proposed

## Context and problem statement

Issue #89 asked koto to auto-advance past "advanced phases." Exploration
revealed that "advanced phases" don't exist -- `advanced: true` is a CLI
response flag meaning "at least one transition occurred in this invocation,"
not a template-level concept. The flag was introduced on March 17 to report
agent-initiated changes. Auto-advancement was added March 18 and overloaded
the same field for engine-initiated transitions. The semantic collision was
never discussed in design docs.

The engine already auto-advances via `advance_until_stop()` in
`src/engine/advance.rs`. It loops through unconditional transitions, evaluates
gates at every step, and stops at evidence requirements, gate failures,
terminal states, integrations, or cycle detection. But the CLI handler returns
after a single engine invocation, forcing callers to re-invoke `koto next` to
classify the new state. The work-on skill's execution loop codifies this as
step 2: "If `action: execute` with `advanced: true`, run `koto next` again."

Three rounds of exploration confirmed:

1. The double-call is emergent, not designed. No invariants are at risk.
2. The agent-vs-engine distinction in `advanced` doesn't matter for any
   real caller. The event log already provides full disambiguation.
3. Response variants (EvidenceRequired, GateBlocked, Terminal) are
   self-describing. Callers don't need `advanced` to decide what to do.
4. Observability belongs in the event log, not in every response.
   `transition_count` is enough as lightweight response metadata.

## Decision drivers

- **Eliminate the double-call.** Every koto caller implements the same
  workaround. The fix should make `koto next` return an actionable response
  in a single call.
- **Preserve state machine integrity.** Transitions must be recorded, evidence
  epochs must stay clean, gates must execute. The event log is the source of
  truth.
- **Keep responses lean.** `koto next` answers "what should I do?" Rich
  observability ("what happened along the way?") belongs in `koto state`
  or the event log. Mirrors git status vs git log.
- **Backward compatibility.** Adding fields is fine. Removing or changing
  field semantics is a breaking change. `advanced: bool` stays in the
  response but gets deprecated as a decision signal.
- **Single layer owns the behavior.** The engine already manages the
  advancement loop. Extending its stopping conditions is cleaner than adding
  a second loop in the CLI handler.

## Decisions already made

These choices were settled during exploration and should be treated as
constraints, not reopened.

### Round 1
- Issue #89 fits koto's philosophy: the double-call is emergent overhead,
  not intentional design
- No state machine invariants at risk: transitions recorded, evidence epochs
  clean, gates still execute
- The fix belongs in the engine layer (`advance_until_stop`), not the CLI or
  caller convention

### Round 2
- Agent-vs-engine semantic distinction: not worth encoding in the CLI
  response (event log handles it)
- The behavioral fix and response contract evolution are independent;
  behavioral fix can proceed first
- `advanced` field: keep for backward compat, deprecate as decision signal

### Round 3
- Response stays lean: `transition_count` for lightweight observability,
  not `passed_through`
- Rich observability via deferred `koto state` command (already designed,
  post-#49)
- `passed_through` not needed in the response -- event log + `koto state`
  handle it
