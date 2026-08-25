---
schema: prd/v1
status: Accepted
problem: |
  A koto gate field may carry a `{{KEY}}` reference to another state's
  `capture_stdout_as` output. The compiler accepts it, but nothing checks at run
  time that the producing state has run, so the raw `{{KEY}}` token reaches the
  shell, the context store or the regex engine as ordinary text. The gate then
  answers on content nobody wrote, the run continues, and the operator gets no
  signal that the reference was the reason.
goals: |
  A run whose gate reads a capture no state has delivered stops on that tick and
  names the capture and the state that would have delivered it, before any gate
  command is built. The stop is a typed authoring stop rather than a gate
  outcome, so no template can route past it. A gate reading a capture that has
  been delivered, or a declared variable, behaves exactly as it does today.
absorbed:
  - docs/briefs/BRIEF-gate-command-undelivered-capture.md
source_issue: 225
---

# PRD: An undelivered capture reaching a gate field

## Status

Accepted

Absorbed [BRIEF-gate-command-undelivered-capture](docs/briefs/BRIEF-gate-command-undelivered-capture.md); carried in Absorbed Brief.

## Absorbed Brief

A koto gate may name another state's captured output, and koto lets it: the
reference is legal because a state that has already run may well have produced
the value the gate wants to test. Whether that state HAS run is a fact about the
tick, and nothing checks it. So the mistake this feature is about -- ordering a
gate before the state that feeds it, or naming the wrong capture -- produces no
error, no warning, and nothing in the recorded gate evidence that separates it
from a gate that genuinely evaluated. It is invisible at the moment it matters
and stays invisible afterwards.

That silence is the problem, and it is worth closing because the same silence
has been closed twice in neighbouring positions already. A directive reading an
undelivered capture is refused; since koto#221 a `default_action`'s `command`
and `working_dir` are too. An author who has learned that koto reports this
mistake in one field finds it stays silent in the fields whose answer decides
whether the workflow advances at all.

What should be different for that author is small and specific. They find out on
the tick it happens, from a message naming the capture and the state that would
have delivered it, so the fix is readable without tracing references by hand.
Nothing reaches a shell, a context store or a regex engine first. The guarantee
does not weaken inside a polling action, where the same gate is resolved again on
every interval. And a template that is correct today does not change: a delivered
capture still resolves, on the tick that delivered it or an earlier one, and a
declared variable still renders its binding.

A gate is not simply the action case again, and two differences are why this was
framed rather than patched. A gate is re-evaluated, in two positions on one tick,
so whatever happens has to happen identically in both. And a gate is allowed to
fail -- an ordinary blocked tick an author routes on, where an action failure is
a stop -- which makes the response a real question rather than an obvious one.
The Decisions and Trade-offs section below is where that question is settled.

The boundary the framing settled on runs around the gate rather than around one
of its fields. The command field is the reported case and the one that reaches
`sh -c`, but koto already enumerates a gate's reference-carrying fields in one
place, and singling out one of them re-creates the per-field drift that
enumeration exists to end. What stays outside is what a gate's fields do with a
value that genuinely resolved -- an empty pattern, an empty filter, a key the
store will not accept each keep their own refusal -- along with the lookup order,
the overlay, the three value forms, and anything the compiler accepts.

## Problem Statement

A koto template's gate can carry `{{KEY}}` references in four fields: the shell
`command` a `command` gate runs, the `key` and `pattern` a context gate reads,
and the `name_filter` a `children-complete` gate scopes itself with. The
compiler validates every such reference against the union of the template's
variables block, every state's `capture_stdout_as` name, and koto's runtime
names, so a reference to a capture compiles. It should: a state that has already
run may well have produced the value the gate wants to test.

Nothing checks at run time that the producing state actually ran. When it has
not, koto's substitution passes the unresolved token through unchanged -- the
documented behaviour for a name that does not resolve -- and the literal
characters `{{KEY}}` reach whatever the field feeds. What happens next depends on
which field it was and what the author wrote around it, and every path is bad in
a different way:

- A `command` gate greps for the value and tests a string that is partly a
  template token, so it answers about content nobody wrote. It can pass. A gate
  that writes the value puts the token on disk; one that consumes it as an
  argument runs a command with a nonsense argument that may still exit 0.
- A `name_filter` becomes a non-empty prefix that no child workflow name starts
  with, so the gate counts zero children and blocks forever. The compile-time
  warning that catches a missing trailing dot says nothing, because the token
  ends in one.
- A `key` becomes a string the context store will not accept, and a `pattern`
  becomes a regex that fails to compile. Both of those happen to surface an
  error today, but the error names the wrong thing -- an unusable key character,
  an invalid regex -- and sends the operator to fix a value rather than the state
  ordering that is actually wrong.

In every case the operator is not told that a capture reference was the reason,
and in the first two the run carries on as though the gate had genuinely
evaluated. The author's mistake -- ordering the gate before the state that
delivers the value, or naming the wrong capture -- is invisible at the moment it
matters.

koto already has a name for this defect and a predicate that detects it.
`first_unset_capture` returns the undelivered capture name paired with the state
that would have delivered it, and three positions consult it: directive
substitution, details substitution, and, since koto#221, a `default_action`'s
`command` and `working_dir`. Each turns a hit into a typed run-time stop. No gate
position consults it, so an author who has learned that koto reports this
mistake in one field finds that it stays silent in the fields whose answer
decides whether the workflow advances at all.

## Goals

An operator who hits this finds out on the tick it happens, from a message that
names the capture and the state that would have delivered it, so the fix is
readable without opening the template and tracing references by hand. Nothing
reaches a shell, a context store or a regex engine first, so there is no
half-substituted side effect to reason about before the diagnosis arrives.

The guarantee does not weaken inside a polling action. A gate that is resolved
once before a polling action starts and again on every polling interval gives the
same answer at both positions, so a workflow does not behave one way outside the
loop and another inside it.

Templates that are correct today are unaffected. A gate reading a capture that
has been delivered resolves it, whether the value arrived on an earlier tick or
from an earlier state on this one, and a gate reading a declared variable renders
its binding exactly as before. Nothing about which references the compiler
accepts changes.

## User Stories

**Ordering mistake, caught at the tick.** As a template author who wrote a gate
that tests a value captured by a later state, I want the run to stop and name
both the capture and its producing state, so that I can reorder the two states
instead of debugging a gate that answered on a template token.

**Inherited template, diagnosed by a non-author.** As an operator running a
template I did not write, I want the stop to name the producing state as well as
the capture, so that I can open the template at that state and see the ordering
problem without knowing the template's history.

**Polling action, same answer.** As an operator debugging a state with a polling
`default_action`, I want the refusal to arrive at the same point and say the same
thing whether the gate was resolved before the action started or inside the
polling loop, so that a polling workflow is not a place where the guarantee is
weaker.

**Working template, unchanged.** As a template author whose gate reads a capture
an earlier state in the same tick delivered, I want the gate to resolve the value
and evaluate against it exactly as before, so that a run that worked yesterday
works today.

## Requirements

**R1.** When a tick would resolve a gate's fields and any field carries a
`{{KEY}}` reference naming a `capture_stdout_as` output that no state has
delivered at that point, koto SHALL refuse the tick. The fields in scope are the
ones koto already enumerates as a gate's reference-carrying fields; the
requirement is stated against that enumeration rather than against a list
maintained here, so a field added to a gate later is covered by construction.

**R2.** The refusal SHALL be a typed run-time stop, distinct from every gate
outcome. It SHALL NOT be reported as a gate result, SHALL NOT appear in the
`gates.*` evidence a transition's `when` clause resolves against, and SHALL NOT
be reachable by a recorded gate override. A template MUST NOT be able to route a
run past it.

**R3.** The refusal SHALL exit with the same status and the same machine-readable
reason koto already uses for an undelivered capture reaching a directive or a
`default_action` -- `capture_unset`, exit 3 -- rather than introducing a second
name for the same defect.

**R4.** The refusal's operator-facing output SHALL name all five of: the state
whose gate was being resolved, the gate, the field within that gate that carried
the reference, the undelivered capture name, and the state that would have
delivered it. The last two are the pair `first_unset_capture` already returns;
the requirement is that the pair reaches the operator, not that it is recomputed.

**R5.** Both positions on a tick that resolve a state's gate fields -- the
advance loop, and the resolution that feeds a polling action's gate evaluation --
SHALL apply the same check, against the value layers each position has available,
and SHALL produce the same exit status and the same five named values when they
refuse. Neither position may pass a token through that the other would refuse
from the same layers.

**R6.** Nothing SHALL run before the refusal. No gate command is spawned, no
context store read or regex compile is attempted for the refusing gate, and no
gate-evaluation event is appended for it.

**R7.** A gate field whose `{{KEY}}` reference names a capture that HAS been
delivered SHALL resolve it and evaluate unchanged, whether the value was
delivered on an earlier tick or by an earlier state on this same tick.

**R8.** A gate field whose `{{KEY}}` reference names a declared template variable
SHALL render that variable's binding unchanged, including when the binding is the
empty string.

**R9.** The existing refusals for a value that genuinely resolved SHALL be
unaffected: an empty `pattern`, an empty `name_filter`, and a `key` the context
store will not accept keep their current outcomes and their current messages.

**R10.** Compile-time acceptance SHALL NOT change. A gate field referencing any
declared capture name still compiles, because whether the value has arrived is a
run-time fact.

**R11.** The check SHALL happen where a gate's fields are resolved, and SHALL
NOT be hoisted ahead of a state's `default_action`. A state's gates are resolved
after its action has run, so a gate referencing the capture that state's own
action delivers is a legitimate template that MUST keep working; a check moved
ahead of the action would refuse it on a value that was about to exist.

**R12.** A polling action's gate resolution SHALL refuse a reference to the
capture that same action delivers. The value cannot exist at that point -- the
command has not finished -- so the reference can never resolve there, and the
alternative is the token reaching the shell on every polling interval. The
message SHALL say that the gate reads the capture its own state delivers, rather
than reporting a state the run has not entered.

## Acceptance Criteria

- [ ] A template whose first state has a `command` gate referencing a capture
      delivered by a later state exits non-zero on `koto next` with reason
      `capture_unset` and exit status 3, and the run does not advance.
- [ ] The output of that run names the state, the gate, the field, the capture
      name, and the producing state.
- [ ] The gate's command does not run: a command with an observable side effect
      (writing a file) leaves no trace after the refusal.
- [ ] The same template shape with the gate on a state carrying a polling
      `default_action` produces the same reason, the same exit status, and the
      same five named values as the non-polling case.
- [ ] A `name_filter` referencing an undelivered capture refuses with
      `capture_unset` rather than blocking on a zero-child count.
- [ ] A `pattern` referencing an undelivered capture refuses with
      `capture_unset` rather than reporting an invalid regex.
- [ ] A `key` referencing an undelivered capture refuses with `capture_unset`
      rather than reporting an unusable key character.
- [ ] A gate command referencing a capture delivered by an EARLIER state in the
      same tick resolves the value and the gate evaluates against it; the run
      advances as it does today.
- [ ] A gate command on a state whose OWN non-polling `default_action` delivers
      the capture resolves the value and the gate evaluates against it, because
      the action has already run by the time the gate is resolved.
- [ ] A gate command on a state whose own POLLING `default_action` delivers the
      capture refuses with `capture_unset`, and the message says the gate reads
      the capture its own state delivers.
- [ ] A gate command referencing a capture delivered on an earlier tick resolves
      the value and the gate evaluates against it.
- [ ] A gate command referencing a declared template variable renders that
      variable's binding, and a gate declaring no `name_filter` still watches
      every child.
- [ ] A `pattern` that substitutes to the empty string still reports the
      empty-pattern error, and a `name_filter` that substitutes to the empty
      string still reports the empty-filter error, with their existing messages.
- [ ] Every regression test added for the criteria above fails against the
      commit this work branched from, demonstrated by running them there rather
      than asserted.
- [ ] `cargo test -- --test-threads=1`, `cargo fmt --check`, and
      `cargo clippy -- -D warnings` are clean.
- [ ] `cargo test --test doc_names` passes with no new entry in
      `tests/doc_names.allow`.

## Out of Scope

- **Where the refusal lives in the call graph.** The helper that substitutes a
  gate's fields returns a plain map with no error channel, and the polling call
  site sits inside a closure whose only refusal shape is action-field-shaped.
  Choosing between changing that helper's signature and checking at the call
  sites is the DESIGN's decision; R5 constrains the outcome, not the mechanism.
- **Reworking variable scoping.** The lookup order, the overlay that carries a
  capture produced earlier in the same tick, and the three value forms a field
  is substituted through are settled and stay settled.
- **The `default_action` refusal from koto#221.** It is the prior art this
  feature reasons against and the source of the exit status R3 reuses. Changing
  it is not part of this work.
- **A compile-time diagnostic.** Nothing here narrows what the compiler accepts;
  see R10.
- **`fallback` prose.** It is deliberately never expanded and any reference in it
  is already refused at compile time. It is not a substituted field and is not
  reached by R1.

## Known Limitations

**A gate that reads its own state's capture stops working under a polling
action.** R11 and R12 pull in opposite directions on purpose, and the seam is
visible to an author who writes one gate set for two consumers. A state's gates
are used twice: the polling loop evaluates them to cut a long-running action
short, and the advance loop evaluates them afterwards to judge what the action
did. A gate referencing the state's own capture is meaningful to the second
consumer and meaningless to the first, and today it half-works -- resolving in
the advance loop and shipping a raw token to the shell on every polling interval.
After this change it refuses instead. That is a behaviour change for a template
that appeared to work, and the honest description is that the template was
already broken in the half nobody was looking at.

**A gate can no longer sit blocked waiting for a capture.** Refusing the tick is
a stronger response than a failing gate normally gets, so an author cannot write
a gate that waits for a later state to deliver a value. The shape does not work
anyway -- a state cannot run while its own gate blocks, so such a gate waits
forever -- but a template relying on it will now stop with a diagnosis rather
than hang.

## Decisions and Trade-offs

### The refusal stops the tick rather than reporting the gate as errored

**Decided:** an undelivered capture in a gate field stops the tick with the same
typed `capture_unset` stop a `default_action` gets, rather than producing a
`GateOutcome::Error` carrying the capture name.

**Alternatives considered.** Reporting the gate as errored has real precedent and
was the stronger-looking option going in. koto#230 established exactly that shape
twice -- for a `pattern` that resolved to empty and a `name_filter` that resolved
to empty -- and both return an error outcome whose message says what the value
would have done, why, and what to do about it. There is a house style for "a gate
cannot use the value it resolved", and an undelivered capture looks like a member.

**Why the stop won.** Three things, and the third is what makes the first two
decisive rather than merely suggestive.

koto#221 faced this choice for the action case and rejected the routable option
on the record: the refusal is "deliberately not an action failure ... an
`__action__` condition is routable, so a template could have carried the run past
the defect with the value still unset." That argument is stronger for a gate, not
weaker. A gate's output is injected into the evidence map regardless of outcome,
by design, so that `when` clauses can route on `gates.*` -- gate conditions are
*the* routing surface, where `__action__` was a single synthesized condition. A
recorded gate override is a second route past.

An errored gate is not a stop at all. Every non-passing outcome is folded
together into a blocked tick, so the errored-gate shape leaves the run exactly
where an ordinary failing gate leaves it, with the message in a field the
operator may never read.

And the two koto#230 precedents are a different class from this one. Both fire on
a value that *resolved* and turned out unusable, which is why both messages carry
the remedy "give the variable a default" -- a run-time input fixes them. An
undelivered capture resolves to nothing at all: the token is still a template
token when substitution finishes, and no run-time input fixes it, because the
template's state ordering is wrong. That is the class `first_unset_capture`
names, and every existing consumer of that predicate turns a hit into a typed
stop. Following the koto#230 style here would put a defect that data cannot fix
into a channel whose whole purpose is for a template to make decisions from data.

**What it costs.** A gate that would have blocked now stops, which is a stronger
response than a failing gate normally gets, and an author who deliberately wanted
a gate to sit unsatisfied until a later state delivers a value loses that. The
cost is accepted because the shape does not work anyway: a state cannot run while
its own gate blocks, so a gate waiting on a capture from a state below it waits
forever.

### The boundary runs around the gate, not around the command field

**Decided:** R1 is stated against the enumeration of a gate's reference-carrying
fields rather than against `command` alone.

**Alternatives considered.** Restricting the requirement to `command` matches the
issue's title and the shell-reaching symptom that motivated it, and is the
narrower change.

**Why the whole gate won.** koto already keeps that enumeration in exactly one
place, next to the struct that owns the fields, and its own documentation says
why: four consecutive fixes were each an instance of two hand-maintained field
lists disagreeing. Singling out one field from that enumeration re-creates the
drift the enumeration exists to end. The other fields are not hypothetical
either -- a `name_filter` carrying an undelivered capture is a non-empty prefix
that matches no child, so the gate blocks forever with no diagnostic, which is
the symptom koto#224 fixed for the literal case arriving by a different road.
Nothing pins the current behaviour of a capture reference in the non-command
fields, so the wider boundary breaks no existing contract.

### The exit status is reused rather than minted

**Decided:** R3 reuses `capture_unset` and exit 3.

**Alternatives considered.** A distinct reason for the gate case would let a
caller tell a gate refusal from an action refusal without reading the message.

**Why reuse won.** It is the same defect in a different position, and the
operator's fix is the same. A second name would tell a caller which field the
engine happened to read first, which is not a distinction anyone acts on, and
would mean two names for one authoring mistake in a message set whose whole value
is that it points at one thing. R4 already carries the position in the message
for the operator who needs it.
