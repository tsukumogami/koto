---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-gate-command-undelivered-capture.md
decision_provenance: inline-resolved
problem: |
  A gate field carrying a `{{KEY}}` reference to an undelivered capture is
  substituted with the token left in place, so the literal `{{KEY}}` reaches
  `sh -c`, the context store or the regex engine. The predicate that detects
  this already exists and no gate position calls it. The helper that substitutes
  a gate's fields returns a plain map with no error channel, and the two callers
  sit in different control-flow contexts, so there is nowhere obvious for a
  refusal to go.
decision: |
  `substitute_gate_fields` gains the capture map and returns a `Result`, so no
  caller can obtain substituted gates without handling a refusal. The advance
  loop's gate-evaluator bound widens to return a `Result` and the engine maps the
  error to a new `StopReason::GateRefusedUnsetCapture`; the polling path returns
  a new `ActionResult::GateRefused` that the engine maps to the same stop. One
  renderer turns that stop into the existing `capture_unset` error at exit 3.
rationale: |
  Both callers meeting at one `StopReason` is what makes the two paths agree by
  construction rather than by maintenance, and putting the check inside the
  helper makes a third caller that skips it a compile error. That is the same
  move koto#230 made one level down, and it is chosen because this issue is the
  fourth instance of two things that had to agree being maintained separately.
---

# DESIGN: An undelivered capture reaching a gate field

## Status

Planned

## Context and Problem Statement

The requirements are settled in
[PRD-gate-command-undelivered-capture](docs/prds/PRD-gate-command-undelivered-capture.md).
This document settles how they are met, and the technical problem is narrower
than the requirements are: the check itself is a one-line call to a predicate
that already exists, and everything hard is about where a refusal can go.

`first_unset_capture` in `src/engine/substitute.rs` takes a string, the map of
declared capture names to their producing states, the log's bindings and the
tick's overlay, and returns the first `{{KEY}}` in that string naming a capture
neither layer has delivered, paired with the state that would deliver it. Three
positions already call it, and each turns a hit into a typed run-time stop.

The gate positions cannot do the same thing as easily, for three reasons that
compose:

**The helper has no error channel.** `substitute_gate_fields` in `src/cli/mod.rs`
takes a `BTreeMap<String, Gate>` and returns a `BTreeMap<String, Gate>`. Every
gate field a tick resolves goes through it -- that consolidation is what koto#230
built, after koto#220 found the advance path and the polling path disagreeing
about which names resolve.

**Its two callers sit in different control-flow contexts.** One is the
`gate_closure` handed to `advance_until_stop`, whose bound is
`Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, StructuredGateResult>` -- a total
function with no way to say "stop the loop". The other is inside `action_closure`,
whose return type is `ActionResult`, an enum about what a `default_action` did.

**A gate outcome is the wrong channel even where one is available.** The engine
folds every non-passing outcome into a blocked tick, and gate output is injected
into the evidence map regardless of outcome so that `when` clauses can route on
`gates.*`. PRD R2 forbids a response a template can route past, which rules out
reporting the refusal as a gate result even though that is the shape nearest to
hand.

So the design question is a plumbing question, and the plumbing is what decides
whether the two paths can drift again.

## Decision Drivers

- **PRD R5: the two resolution positions must not disagree.** The polling site is
  where koto#220 found them drifted, so a design that relies on two call sites
  each remembering to do the same thing is answering the wrong question.
- **PRD R2 and R3: a typed stop, not a gate outcome, reusing `capture_unset` and
  exit 3.** Whatever carries the refusal has to reach the existing error path.
- **PRD R11: the check must not be hoisted ahead of a state's action.** A gate
  referencing the capture its own state's non-polling action delivers resolves
  correctly today and must keep doing so, so the check belongs where the
  substitution is, not earlier.
- **The codebase's stated preference for enumerations over conventions.**
  `Gate::substitutable_fields` and the `ActionDecl` pair exist because
  hand-maintained agreement failed four times; a design that adds a fifth thing a
  maintainer must remember is arguing against the repository.
- **Churn is a cost, not a veto.** `advance_until_stop` has 38 call sites, nearly
  all in its own tests. Mechanical churn the compiler finds is cheaper than a
  silent seam.

## Considered Options

### Decision 1 -- where the refusal is detected

**Option 1A: check at the two call sites.** Each caller loops the state's gates
and their substitutable fields, calls a shared predicate, and refuses on a hit
before calling the helper. No signature changes anywhere.

**Option 1B (chosen): fold the check into `substitute_gate_fields`.** The helper
takes the capture map as a fourth input and returns
`Result<BTreeMap<String, Gate>, GateCaptureRefusal>`.

1A is genuinely cheaper and would work. It is rejected because it leaves the
defect class open: a third caller could substitute gate fields and never ask,
and nothing would say so. That is the shape of every fix in this sequence --
koto#220's two paths, koto#222's two field lists, koto#224's third list --
and koto#230's answer each time was to make agreement structural. 1B applies the
same answer one level up. A gate that has been substituted is then, by
construction, a gate whose references were checked.

### Decision 2 -- how the advance-loop path reports

**Option 2A (chosen): widen the gate-evaluator bound.** `G` becomes
`Fn(&BTreeMap<String, Gate>) -> Result<BTreeMap<String, StructuredGateResult>,
GateCaptureRefusal>`, and `advance_until_stop` maps `Err` to a new `StopReason`.

**Option 2B: move the check into the engine.** `advance_until_stop` holds the
compiled template, the log's bindings and the per-iteration overlay, so it could
ask the predicate itself before calling the evaluator.

**Option 2C: stash the refusal in a cell the closure captures**, return a failing
gate map, and read the cell after the loop.

2B is rejected on layering. The engine deliberately does not substitute: gates
arrive already substituted, and `gate::evaluate_gates` says so in a doc comment
that calls itself "the whole of the guard" because nothing in the type
distinguishes an authored gate from a substituted one. Putting a
substitution-shaped question inside the one component that does not do
substitution would also require rebuilding a variable view the CLI already holds.

2C is rejected as unsound rather than inelegant. A failing gate map is a blocked
tick, and the engine has already decided the tick's outcome by the time the cell
could be read. The run would report a blocked gate and the refusal would arrive
too late to prevent it.

2A costs a mechanical edit at every `advance_until_stop` call site, nearly all of
them tests where `|gates| { ... }` becomes `|gates| Ok(...)`. It is chosen
because the new type is true: after this change a gate evaluation can refuse, and
a bound that says otherwise is a bound that lies at 38 places instead of one.

### Decision 3 -- how the polling path reports

**Option 3A (chosen): a new `ActionResult::GateRefused` variant** carrying the
gate name, the field, the capture and the producer.

**Option 3B: reuse `ActionResult::Refused`** by widening `ActionField` to cover
gate fields.

3B is rejected because `ActionField` is a closed two-member enum whose `as_str`
renders `command` and `working_dir`, and whose whole meaning is "a field of a
`default_action`". The existing renderer writes "has a default_action whose
{field} reads ..."; a gate refusal arriving there would produce a sentence about
an action for a defect in a gate. Widening the enum would make it mean two
things and oblige every existing reader to tell them apart.

The two variants converge immediately downstream: both map to the same
`StopReason`, so the divergence is one match arm wide.

### Decision 4 -- wording the polling self-reference

**Option 4A: the general message**, naming the producing state.

**Option 4B (chosen): a distinct message** when the producing state is the state
being resolved.

A polling action's gates are substituted before its command runs, so the overlay
cannot hold the capture that action delivers -- the code comment at the polling
site calls this inherent. A gate on that state referencing that state's own
capture is therefore undeliverable at that position, and PRD R12 requires the
refusal. koto#221 met the same case for the action side and chose 4B, comparing
the state against the producer and saying that the value "does not exist until
the command has produced it" rather than that the run "has not entered that
state". The reason given there holds unchanged here: the general sentence tells
an operator the run has not reached a state they are standing in, and points at a
routing fix for a problem that is not a routing one.

## Decision Outcome

The four decisions compose into one path. `substitute_gate_fields` becomes the
single place the question is asked, and both callers are forced by its return
type to answer it. Each caller converts the refusal into its own control-flow
vocabulary -- an `Err` from the gate evaluator, or an `ActionResult` variant --
and the engine converts both into one `StopReason`. One renderer turns that stop
into the `capture_unset` error the operator sees.

That shape is what satisfies PRD R5 structurally. The two positions do not agree
because someone maintains them in step; they agree because there is one predicate,
one error type, one stop and one message, and the only thing each path owns is
the two lines that carry the error from where it was raised to where it is
reported.

It also keeps PRD R11 intact without a special case. The check happens where the
substitution happens, so the advance path checks after the state's action has run
and a gate reading its own state's non-polling capture resolves normally, while
the polling path checks before the command runs because that is when it needs the
gates.

## Solution Architecture

### The refusal type

A small struct in `src/cli/mod.rs`, next to the helper that raises it, carrying
exactly what PRD R4 requires the operator to be told minus the state, which the
engine adds because only the engine knows which state it is resolving:

```rust
struct GateCaptureRefusal {
    gate: String,
    field: &'static str,
    key: String,
    producer: String,
}
```

`field` is `&'static str` because it comes from `Gate::substitutable_fields`,
which already pairs each field's value with the name errors report it under.
Nothing here invents a field name.

### The helper

```rust
fn substitute_gate_fields(
    gates: &BTreeMap<String, Gate>,
    runtime_vars: &HashMap<String, String>,
    variables: &Variables,
    overlay: &VariableOverlay,
    captures: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Gate>, GateCaptureRefusal>
```

For each gate, in `BTreeMap` order, and for each `(name, value)` pair
`Gate::substitutable_fields` yields, it calls `first_unset_capture`. The first hit
returns `Err`. Only when every field of every gate is clean does it do the
substitution it does today and return `Ok`.

Checking all gates before substituting any is deliberate: a refusal is a stop, so
a partially substituted map is never useful, and ordering the check ahead of the
work keeps PRD R6 -- nothing runs before the refusal -- true by construction
rather than by inspection.

Iterating the accessor rather than naming `command` is what makes the guard cover
the gate rather than one field, per the PRD's second recorded decision. It is also
why a field added to `Gate` later is covered: the accessor destructures `self`
exhaustively, so it will not compile until someone classifies the new field, and
this loop then reads it.

### The engine's stop

```rust
StopReason::GateRefusedUnsetCapture {
    state: String,
    gate: String,
    field: &'static str,
    key: String,
    producer: String,
}
```

Reached from two arms of `advance_until_stop`. The gate block's call to the
evaluator becomes a `match`, whose `Err` arm returns the stop with the state it is
resolving. The action block gains an arm for `ActionResult::GateRefused`, which
returns the same stop with the state whose action was about to run.

Both arms return before anything else happens in that iteration: no
`GateEvaluated` event is appended, no transition is resolved, and in the action
case no command is spawned.

### Data flow on a tick

```
koto next
  -> advance_until_stop(state, ..., gate_closure, action_closure, overlay)
       |
       |-- action_closure(state, action, has_evidence)
       |     polling? -> substitute_gate_fields(state gates, ..., captures)
       |                   Err -> ActionResult::GateRefused ------------+
       |                   Ok  -> execute_with_polling(...)             |
       |                                                               |
       |-- gate_closure(state gates)                                    |
       |     substitute_gate_fields(gates, ..., captures)               |
       |       Err -> Err(GateCaptureRefusal) --------------------------+
       |       Ok  -> evaluate_gates(...)                               |
       |                                                                v
       +--------------------------------> StopReason::GateRefusedUnsetCapture
                                                        |
                                                        v
                                        NextError { code: CaptureUnset }  exit 3
```

### The message

One renderer arm in `src/cli/mod.rs`, beside the action arm it mirrors. It picks
between two sentences on `state == producer`, exactly as the action arm does:

- Different states: state S has a gate G whose *field* reads `{{KEY}}`, which
  state P delivers with `capture_stdout_as`; this run has not entered that state,
  so the value is unset and the gate did not run.
- Same state: state S has a gate G whose *field* reads `{{KEY}}`, the name that
  same state's action delivers with `capture_stdout_as`; the value does not exist
  until the command has produced it, so the gate did not run.

Both name all five values PRD R4 requires. The error code stays `capture_unset`
and the exit status stays 3, per PRD R3.

## Implementation Approach

Three steps, each of which compiles clean under `-D warnings` and passes the
suite on its own.

**Step 1 -- the whole path, in one step.** `GateCaptureRefusal`;
`substitute_gate_fields` takes the capture map, runs the check and returns
`Result`; `StopReason::GateRefusedUnsetCapture` and `ActionResult::GateRefused`;
the widened `G` bound and the engine's two arms; both callers converting the
error into their own vocabulary; and the renderer arm with its two sentences.

It is one step rather than the two it looks like, and the reason is worth stating
because the obvious split does not work. Landing the signature and the check
first, with the callers unwrapping, is not behaviour-neutral: a template that
triggers the refusal would panic on the unwrap, which is worse than both the
before and the after. Landing the engine-side vocabulary first instead leaves
`GateCaptureRefusal` used in signatures and constructed nowhere, which
`dead_code` refuses under `-D warnings`. Every seam through the middle of this
change produces an intermediate state that is broken in one direction or the
other, because the type, its producer and its consumers are one unit of meaning.

What a reviewer is owed is still owed, and it is delivered at commit granularity
inside the step: one commit for the mechanical bound change across the
`advance_until_stop` call sites, one for the behavioural wiring. Two commits in
one step read exactly as well as two steps and leave no broken tree between them.

**Step 2 -- the regression tests.** The integration tests in
`tests/gate_field_substitution_test.rs`: one per gate field for the refusal, the
polling-parity pair, the two self-reference cases, and the no-regression set that
pins a delivered capture, a declared variable, and the koto#230 empty-value
refusals keeping their own messages.

The failing-against-`main` evidence is collected against a detached worktree of
`main` with the new test file copied in, not against a stash, and the failure text
is recorded in the pull request.

**Step 3 -- documentation.** The `CHANGELOG.md` entry under Unreleased / Fixed,
the removal of the `#225` clause from the `substitute_shell_command` doc comment
-- the same paragraph koto#221, koto#222 and koto#224 each edited in turn -- and
the assessment of all three skills under `plugins/koto-skills/skills/`, recorded
with its reason even where the answer is that nothing changes.

## Security Considerations

Reviewed; the change removes a small amount of unintended execution and adds no
new input surface.

**What it removes.** Today an unresolved `{{KEY}}` reaches `sh -c` inside a gate
command. The token itself is inert -- braces are not shell metacharacters and the
value allowlist never applied, because no value was substituted -- so this is not
a command-injection vector. What it is, is a command running with an argument
nobody wrote, which for a gate that consumes the token (writing it to a file,
passing it to a subcommand, adding it to the context store) means an unintended
side effect that then persists. Refusing before the command is built removes it.

**What it adds.** Two strings reach the operator's error message that did not
before: the gate name and the field name. The gate name comes from the template's
own gate map and the field name is a `&'static str` from an enumeration in the
source, so neither is attacker-supplied in any sense the template author is not
already the author of. The capture name and producer state were already rendered
by the action arm. All five are serialized through `serde_json`, so no quoting
question arises.

**What it does not change.** No new file, network or process access. No change to
what the compiler accepts, so no template that was refused is now accepted. The
value allowlist, the context-key grammar and the `working_dir` containment check
are untouched. The refusal is strictly earlier than the work it prevents, so
there is no window in which a partial effect lands and is then reported as
refused.

**One consideration worth naming.** Making the refusal unroutable is itself the
security-relevant property. PRD R2 forbids reporting it as a gate outcome
specifically because gate output is routable and overridable, which would let a
template carry a run past a defect with the value still unset -- the same
reasoning koto#221 recorded for the action case.

## Consequences

### Positive

- The two gate-resolution positions agree by construction. There is one
  predicate, one error type and one stop, and neither path can be changed to
  answer differently without changing the shared piece.
- A future caller of `substitute_gate_fields` cannot skip the check. The
  compiler refuses, which is the same guarantee `Gate::substitutable_fields`
  gives one level down.
- The refusal covers every field the accessor enumerates, so `name_filter` --
  which today becomes a prefix no child name starts with, blocking the gate
  forever with no diagnostic -- is covered without a second mechanism.
- `advance_until_stop`'s signature now states that a gate evaluation can refuse,
  which is true and was not expressible before.

### Negative

- The gate-evaluator bound change touches every `advance_until_stop` call site,
  nearly forty of them. The churn is mechanical and compiler-enumerated, but it
  makes the diff large relative to the behaviour it changes.
- A gate that reads its own state's capture stops working under a polling action,
  where it previously half-worked. The PRD records this under Known Limitations.
- Refusing is stronger than blocking, so a template that relied on a gate sitting
  unsatisfied now stops. Such a template could never have made progress, but it
  fails differently than it did.

### Mitigations

- Step 1 is two commits, the mechanical bound change and the behavioural wiring,
  so a reviewer can read them separately without any intermediate tree being
  broken. It is one step rather than two because every seam through the middle
  leaves either a panic on the refusal path or a never-constructed type that
  `dead_code` refuses.
- The regression set pins both directions -- what must refuse and what must not
  change -- so an implementation that refuses too broadly fails as loudly as one
  that refuses too little.
- The self-reference wording is checked by its own test, so the case that would
  otherwise produce a confusing message about a state the operator is standing in
  is covered rather than argued.
