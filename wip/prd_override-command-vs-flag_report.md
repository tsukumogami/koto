# Decision Report: Override as Flag vs Command

## Question
Should gate override be a flag on `koto next` or its own command?

## Chosen: Option C (Both -- command as primitive, flag as shorthand)

## Confidence: High

## Rationale

The research reveals that koto already has this pattern: `koto decisions
record` appends a decision event without advancing, and the advance loop reads
decision events when it runs. Override should follow the same pattern.

`koto override <name> --gate ci_check --rationale "reason"` appends a
`GateOverrideRecorded` event to the state file without advancing. The advance
loop reads override events during gate evaluation and substitutes the override
defaults for the named gates. `koto next` triggers advancement as usual.

`koto next --override-rationale "reason"` is shorthand that does both in one
call -- appends the override event then runs the advance loop. This is the
convenience path for single-agent workflows where the same agent overrides
and advances.

Why both matter:

**The separate command enables multi-agent composition.** In shirabe workflows,
a coordinator agent spawns sub-agents that each handle different concerns.
Sub-agent A might override the CI gate while sub-agent B overrides the lint
gate. With a separate command, they can push overrides independently (no lock
contention, since append-only events don't require the advance lock). The
orchestrator then calls `koto next` to advance.

**The flag preserves simplicity for single-agent workflows.** Most koto usage
today is single-agent. Requiring two commands (override then next) for every
gate bypass adds friction for no benefit in the common case.

**The pattern is proven.** `koto decisions record` (append without advance)
already works this way. Evidence submission via `--with-data` on `koto next`
is the "shorthand" equivalent. Override follows the same dual-path model.

## Implementation note

The advance loop needs to read override events from the current epoch to know
which gates have been pre-overridden. This follows the same pattern as
`derive_evidence` -- scan events for the current state and collect overrides.
The `derive_overrides_for_epoch` function (distinct from the cross-epoch
`derive_overrides` used by `koto overrides list`) returns overrides that
apply to the current state's gate evaluation.

## Assumptions
- Multi-agent override scenarios are a future use case, not immediate. But
  designing the primitive now avoids a breaking change later.
- The separate command doesn't take the advance lock (append-only, like
  decisions record).
- The flag shorthand is purely syntactic sugar -- it calls the same code path
  as the separate command followed by next.

## Rejected

**Option A (flag only)**: works for single-agent but blocks multi-agent
composition. If override is only a flag on `koto next`, only one agent can
override at a time (the one that calls next). Sub-agents can't push overrides
independently.

**Option B (command only)**: works for multi-agent but adds friction for
single-agent. Every override requires two commands. The 80% case (single
agent, just wants to get past a gate) becomes unnecessarily verbose.
