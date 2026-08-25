---
schema: brief/v1
status: Accepted
problem: |
  A gate command may name another state's capture. When that state has not run,
  the raw `{{KEY}}` token is handed to `sh -c` and the gate answers on text
  nobody wrote, so the operator is told nothing and the run continues.
outcome: |
  A workflow whose gate reads a capture no state has delivered stops on that
  fact, naming the capture and the state that would deliver it, and behaves the
  same whether the gate is evaluated in the advance loop or inside a polling
  action.
motivating_context: |
  The third position found with the same asymmetry. koto#220 fixed the runtime
  names in a `default_action.command`, koto#221 refused an undelivered capture
  in that field and in `working_dir`, and koto#222 and koto#224 brought a gate's
  remaining fields into substitution. Gate commands were left resolving through
  the shared helper with no caller consulting the undelivered-capture guard.
---

# BRIEF: An undelivered capture reaching a gate command

## Status

Accepted

The framing stops at what the operator is owed. Which shape the refusal takes --
stopping the tick, or reporting the gate as errored -- and where in the call
graph it lives are the downstream PRD's and DESIGN's to settle; this brief
records that they are open, not which way they go.

## Problem Statement

A koto template's gate can run a shell command, and that command may carry
`{{KEY}}` references. The compiler accepts a reference naming any state's
`capture_stdout_as` output, because a capture is a legal thing for a gate to
read -- a state that has already run may well have produced the value the gate
wants to test.

Nothing checks at run time that the producing state actually ran. When it has
not, the reference resolves to nothing, and koto's substitution passes an
unresolved token through unchanged rather than erroring. The literal characters
`{{KEY}}` reach `sh -c` as ordinary text. What happens next depends on what the
author wrote around the reference, and none of the outcomes are good:

- A gate that greps for the value tests a string that is partly a template
  token, so it answers about content nobody wrote. It can pass.
- A gate that writes the value somewhere puts the token on disk, where a later
  state or a human reads it as data.
- A gate that consumes the value as an argument runs a command with a nonsense
  argument that may still exit 0.

In every case the run continues, and the operator has no signal that the
reference was the reason. There is no error, no warning, and nothing in the
recorded gate evidence that distinguishes this from a gate that genuinely
evaluated. The author's mistake -- ordering the gate before the state that
delivers the value, or naming the wrong capture -- is invisible at the moment it
matters and stays invisible afterwards.

The same asymmetry has been closed twice already in neighbouring positions. A
directive that reads an undelivered capture is refused, and since koto#221 a
`default_action`'s `command` and `working_dir` are refused too. A gate command
goes through the same substitution helper and is checked against the same
reference set at compile time, but no caller asks the undelivered-capture
question before evaluating it. So an author who has learned that koto tells them
about this mistake in one field finds that it does not in another, and the field
where it stays silent is the one whose answer decides whether the workflow
advances.

Two things make a gate different from the action case rather than a copy of it,
and they are why this is worth framing rather than patching. A gate is
re-evaluated: the same gate string is resolved in the advance loop and, for a
polling action, again inside the polling loop, so whatever happens has to happen
identically in both or the same template behaves one way outside the loop and
another inside it. And a gate is allowed to fail -- a failing gate is an ordinary
blocked tick that a template author routes on, where an action failure is a stop.
That makes the response to a broken gate reference a real question rather than an
obvious one.

## User Outcome

An operator running a workflow whose gate command names a capture that no state
has delivered finds out immediately, in terms that name the mistake. The output
tells them which capture name was read and which state in the template would
have delivered it, so the fix -- reorder the states, or correct the name -- is
readable from the message without opening the template and tracing the
references by hand.

Nothing reaches a shell first. The operator does not have to reason about what a
half-substituted command did to the working directory or the context store before
they were told, because the check happens before the command is built.

A template author whose gate reads a capture that *has* been delivered sees no
change at all, whether the value arrived on an earlier tick or from an earlier
state in the same tick. The same holds for a gate written against a declared
variable, which has always rendered its own binding.

And an operator debugging a polling action gets the same answer they would get
outside one. A gate that is evaluated repeatedly does not change its mind about
whether a reference is usable partway through a tick.

## User Journeys

### An author orders a gate before the state that feeds it

A template author writes a state whose gate command tests a value captured by a
later state -- the ordering mistake, made because the template reads top to
bottom and the dependency runs the other way. They run `koto next`. Today the
workflow advances and the gate's own output looks ordinary. In the outcome this
brief frames, the run stops on that tick, names the capture the gate read and the
state that delivers it, and the author reorders the two states and re-runs.

### An operator inherits a template and hits it at run time

An operator who did not write the template runs it and the run stops with a
message about a capture name they have never heard of. Because the message names
the producing state as well as the capture, they can open the template at that
state and see that it sits after the gate that reads it. They are not the author
and do not need to be: the pair is enough to hand back a specific bug report, or
to fix the ordering themselves.

### A gate is re-evaluated inside a polling action

An author writes a state with a polling `default_action` whose gate command reads
a capture from a state further down the template. The gate is resolved once
before the action starts and again on every polling interval. The operator sees
the same refusal at the same point on the tick regardless of which of those
evaluations reached the reference first, so a polling workflow is not a place
where the guarantee quietly weakens.

### A delivered capture keeps working

An author writes a gate command reading a capture that an earlier state in the
same tick just produced -- the ordinary, correct use. They run `koto next` and
the gate resolves the value and evaluates against it, exactly as before. A run
that was working before this feature does not change behaviour because of it.

## Scope Boundary

**IN**

- Gate commands that carry a `{{KEY}}` reference naming a `capture_stdout_as`
  output no state has delivered at the point the gate is evaluated.
- Both positions where a gate's fields are resolved on a tick: the advance loop
  and the polling loop that re-evaluates a state's gates while a polling action
  runs. Whatever the response is, it is the same in both.
- The operator-facing content of the response: the undelivered capture name and
  the state that would have delivered it must both reach the operator. The pair
  is already computed inside the engine; the requirement is that it arrives.
- Preserving the current behaviour of a delivered capture, on the tick that
  delivered it and on any later one, and of a declared template variable.

**OUT**

- **Choosing the response shape.** Refusing the tick, as an undelivered capture
  in a `default_action` is refused, and reporting the gate as errored with the
  capture named are both live. The trade-off is real -- gate conditions are the
  surface a template routes on -- and settling it is the downstream PRD's and
  DESIGN's work, not this brief's.
- **Where the refusal lives in the call graph.** The helper that substitutes
  gate fields returns a map with no error channel, so a response either happens
  at the call sites or the helper's signature carries one. That is a design
  question about an existing interface.
- **A gate's other fields.** `key`, `pattern` and `name_filter` reach the
  context store and the regex engine rather than a shell, and each already has
  its own refusal for a value that resolved to nothing. This brief is the
  command field.
- **Reworking variable scoping.** The lookup order, the overlay that carries a
  capture produced earlier in the same tick, and the three value forms are
  settled and stay settled. Only the response to an undelivered name is open.
- **The `default_action` refusal.** The existing action-side guard is prior art
  this feature reasons against. Changing it is not part of the feature.
- **Compile-time rejection of the reference.** A gate reading a capture is a
  legal and useful thing to write; the reference is only wrong when the value
  has not arrived, which is a run-time fact. Nothing here narrows what the
  compiler accepts.

## References

- `docs/designs/current/DESIGN-substitution-drift-class.md` -- the accessor pair
  that enumerates which gate and action fields carry references, and why each
  is written as an exhaustive destructuring.
- `docs/prds/PRD-substitution-drift-class.md` -- the requirements behind the
  gate-field unification this brief's feature sits on top of.
- `docs/guides/default-action-authoring.md` -- how the action-side refusal
  reaches an author today.
