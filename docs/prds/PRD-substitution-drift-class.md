---
schema: prd/v1
status: Accepted
problem: |
  koto template authors get no signal when a field consumes a {{KEY}}
  reference verbatim. The compiler's list of fields it validates references
  in and the tick's list of fields it substitutes are maintained by hand in
  different files, and when they disagree the raw token passes through
  silently. Two fields are open right now -- a children-complete gate's
  name_filter and a default_action's fallback -- and both are in neither
  list.
goals: |
  Every string field of a koto template gives an author a definite answer
  about a reference written into it, at compile time where the compiler can
  see it and at the point of use where only the runtime can. Which fields
  participate is enumerated once per owning struct, so the compiler's answer
  and the runtime's cannot disagree, and a field added later cannot land
  outside both.
absorbed:
  - docs/briefs/BRIEF-substitution-drift-class.md
motivating_context: |
  Four consecutive defects were this drift one field at a time. The most
  recent fix built the single enumeration for gate fields and left the
  action-declaration half unbuilt, so the two fields still open are exactly
  the ones the built half does not cover.
---

## Status

Accepted

Absorbed [BRIEF-substitution-drift-class](docs/briefs/BRIEF-substitution-drift-class.md); carried in Absorbed Brief.

Requirements and acceptance criteria for closing the substitution-drift class.
The two behavioural decisions -- what an empty resolved `name_filter` means,
and whether `fallback` substitutes or refuses -- are settled here under
Decisions and Trade-offs, because both are observable contract. How the
enumeration is shaped in code is left to the design.

## Absorbed Brief

**Why this exists.** koto's compiler and its tick each decide for themselves
which template fields resolve `{{KEY}}` references. When those two decisions
disagree the author is told nothing at all, and the symptom surfaces far from
the cause -- a gate that blocks forever, a context key spelled with braces, a
pointer an agent cannot follow. Four defects in a row were that drift one
field at a time, and the most recent fix built the single enumeration for gate
fields and left the action-declaration half unbuilt. The two fields still open
are exactly the ones that half would have covered, so fixing them individually
would leave the shape that produced all four intact.

**What should be different.** An author writing a reference into any template
string field gets a definite answer while they can still change it: the
reference resolves, or the compiler names the state and the field and refuses.
A resolved value that means something dangerously different from what was
written -- an empty prefix where the author asked for one fan-out -- is refused
at the point of use rather than applied. And which fields participate is a
property of the struct that owns them, written once, with a test that refuses
a change wiring one half without the other.

The journeys this covers are a gate author scoping a fan-out by the parent's
own name, an author writing failure prose that points at the session
directory, and a maintainer adding a substitutable field; they appear below as
User Stories. The boundary the framing drew is carried in Requirements and in
Out of Scope. Three in-repo precedents grounded it:
`docs/designs/current/DESIGN-template-variable-substitution.md` for the
substitution model,
`docs/designs/current/DESIGN-gate-contract-compiler-validation.md` for what the
compiler owes an author before a workflow runs, and
`docs/designs/current/DESIGN-koto-runs-commands.md` for why `fallback` is
spliced after substitution.

## Problem Statement

A koto template is read by two parts of koto that must agree. The compiler
reads it at `koto init` and rejects a `{{KEY}}` reference that names nothing
declared. The tick reads it on every `koto next` and rewrites those references
into values. Each has historically decided for itself which fields
participate, from a hand-written list in its own file, with nothing comparing
the two.

The author is the one who pays when they disagree, and pays late. A reference
in a field neither side handles compiles clean and reaches its consumer as
literal braces. Nothing in the output names a variable. What the author sees
instead is a `children-complete` gate that counts zero children and blocks
forever, or failure prose handed to an agent that points at a session
directory spelled `{{SESSION_DIR}}`. The distance between the cause and the
symptom is the whole cost: the author is debugging a gate, or an agent, rather
than reading a compiler error that names the state, the gate and the field.

Two fields are in that position today. A `children-complete` gate's
`name_filter` is a name prefix that exists to scope a gate to one fan-out
among several, and the natural way to write that scope is against the parent's
own name -- which is exactly the reference that does not resolve. A
`default_action`'s `fallback` is prose an agent reads at the moment something
failed, spliced onto the same response whose directive resolves references two
lines below, so one response disagrees with itself about what a reference
means.

Underneath both is one shape rather than two bugs. `Gate` now enumerates its
substitutable fields once, and the compiler and the tick both read that
enumeration, so gate fields cannot drift apart again. `ActionDecl` has no such
enumeration: its `command` and `working_dir` are validated by two separate
hand-written loops and substituted at two separate call sites, and `fallback`
appears in none of them. The next string field added to an action declaration
will be discovered the way the last four were.

## Goals

- An author writing a reference into any template string field learns what
  koto will do with it while they can still change it.
- A resolved value that means something dangerously different from what the
  author wrote is refused at the point of use, with the reason named, rather
  than applied.
- The set of fields that participate in substitution is a property of the
  struct that owns them, stated once, and the test suite refuses a change that
  wires one half without the other.
- Nothing koto ships continues to assert the behaviour this work changes.

## User Stories

**Scoping a fan-out gate.** As a workflow author spawning research children
under `{{SESSION_NAME}}.research.`, I want a `children-complete` gate's
`name_filter` to resolve the same reference the spawn command resolved, so
that the gate scopes to my fan-out instead of matching nothing and blocking.

**Catching my own typo.** As a workflow author, I want `koto init` to refuse a
reference in `name_filter` that names a variable I never declared, naming the
state, the gate and the field, so that I fix it before the workflow runs
rather than after a gate has silently matched no child.

**Not being widened silently.** As a workflow author whose `name_filter` is a
single reference, I want koto to refuse the gate when that reference resolves
to nothing, so that a gate written to watch one fan-out never quietly starts
watching every child.

**Writing failure prose.** As a workflow author writing a `default_action`
`fallback`, I want koto to tell me at compile time what it will do with a
`{{KEY}}` I write there, so that I am not left to discover from an agent that
the prose it received did not resolve.

**Adding a field.** As a koto maintainer adding a string field to an action
declaration, I want one place to declare that the field participates, and a
test that fails until both halves are wired, so that I cannot ship the drift
again.

## Requirements

### Functional

**R1.** A `children-complete` gate's `name_filter` SHALL have `{{KEY}}`
references substituted during a tick, in the same lookup order and at the same
points in the tick as the gate fields that already substitute -- including
when a gate is re-evaluated inside a `default_action` polling loop.

**R2.** The compiler SHALL validate `{{KEY}}` references in `name_filter`
against the template's declared variables, state capture names and koto's
runtime names, and SHALL refuse a template carrying an undeclared reference
there with an error naming the state, the gate and the field.

**R3.** When a `name_filter` is present in the template and its value is empty
after substitution, the gate SHALL report an error naming the reason rather
than evaluating. A `name_filter` that is absent from the template SHALL
continue to mean "no filter" and SHALL be unaffected. The two states SHALL
remain distinguishable at the point the gate decides.

**R4.** The compiler SHALL refuse a template in which a `default_action`'s
`fallback` contains a `{{KEY}}` reference, with an error that names the state
and the field, says that `fallback` is literal prose that is never expanded,
and points at where a reference would resolve. The runtime SHALL continue to
splice `fallback` after substitution, unexpanded, as its doc comment already
promises.

**R5.** `ActionDecl` SHALL carry an enumeration of the fields a tick
substitutes references in, alongside the struct that owns them, in the same
shape as the one `Gate` carries. The compiler's reference validation for
action-declaration fields SHALL read that enumeration rather than naming
fields individually.

**R6.** A test SHALL fail when a field named by either enumeration survives a
tick still carrying a raw `{{`, and its failure message SHALL name the field.
The action-declaration enumeration SHALL be covered by such a test, as the
gate enumeration already is.

**R7.** Every shipped document and skill reference that states the behaviour
these requirements change SHALL be corrected in the same change. This covers
at minimum the two skill references that name `name_filter` as the one gate
field that does not substitute, the guides that enumerate where substitution
reaches, and the source doc comments that name the open issues as remaining
gaps.

### Non-functional

**R8.** Each of R1 through R4 SHALL be covered by a regression test that fails
against the current `main` and passes with the change. The failure against
`main` is to be demonstrated by running the tests against a checkout of
`main`, not asserted.

**R9.** The change SHALL NOT alter the observable behaviour of any field
outside `name_filter` and `fallback`. Existing gate and action substitution
semantics, the lookup order, the value forms and the overlay are unchanged.

**R10.** Errors introduced by R2, R3 and R4 SHALL follow the diagnostic shape
koto already uses for the same class of failure -- naming the state, the gate
or field, and the reference, and carrying a remedy where the existing
neighbouring errors carry one.

## Acceptance Criteria

- [ ] A template whose `children-complete` gate has
      `name_filter: "{{SESSION_NAME}}.research."` compiles, and at run time the
      gate matches children spawned under the resolved prefix.
- [ ] A template whose `name_filter` references an undeclared variable is
      refused at `koto init`, and the error names the state, the gate and
      `name_filter`.
- [ ] A gate whose `name_filter` is present and resolves to the empty string
      reports an error naming the reason, and does not evaluate as though no
      filter were set.
- [ ] A gate with no `name_filter` in the template behaves exactly as it does
      today.
- [ ] A gate's `name_filter` resolves the same way inside a `default_action`
      polling loop as it does outside one.
- [ ] A template whose `default_action.fallback` contains any `{{KEY}}`
      reference is refused at `koto init`, and the error names the state,
      names `fallback` as literal prose, and points at the directive.
- [ ] A `default_action.fallback` with no reference in it reaches the failure
      response's directive exactly as it does today.
- [ ] `ActionDecl` carries a substitutable-fields enumeration, and the
      compiler's action-field reference validation reads it.
- [ ] Adding a field to either enumeration without wiring the tick makes a
      test fail with a message naming that field. Demonstrated for the
      action-declaration enumeration.
- [ ] Every regression test added for the four behaviours above fails against
      a checkout of `main` with the test file copied in, demonstrated rather
      than asserted.
- [ ] `cargo test -- --test-threads=1` passes, `cargo fmt --check` is clean,
      and `cargo clippy -- -D warnings` is clean.
- [ ] `cargo test --test doc_names` passes with no new entry in
      `tests/doc_names.allow`.
- [ ] No shipped sentence in `docs/`, `plugins/koto-skills/skills/` or a
      source doc comment still asserts that `name_filter` does not substitute
      or that `fallback` is unvalidated.

## Decisions and Trade-offs

### D1 -- A `name_filter` that resolves to empty is refused, not applied

**Decided:** when `name_filter` is present in the template and resolves to the
empty string, the gate errors with a named reason instead of evaluating.
Absent stays "no filter"; the implementation must keep the two states apart.

**Alternatives:** apply the empty prefix, which matches every child. Or treat
resolved-empty as absent, which is the same thing by another name.

**Why this wins:** an empty prefix does not narrow the gate, it removes the
filter. An author who wrote `name_filter: "{{PREFIX}}"` asked for one fan-out;
silently giving them every child is the opposite of the request and it fails
open, which is worse than the failing-closed symptom that got these issues
filed. koto already made this exact call for a `context-matches` `pattern`
that resolves to empty, for the same reason, and refuses it at the gate with a
named reason. Following that precedent keeps one answer for one question. The
cost is that a template relying on a reference that legitimately resolves to
empty as a way of saying "no filter" would now error -- but nothing documents
that as a way to say it, and saying it by omitting the field is
unambiguous.

**Consequence for the design:** `name_filter` is `Option<String>`, so
"absent" and "resolved to empty" are different states and must stay so.
Collapsing them with `.as_deref().unwrap_or("")` at any point on the path
would make the requirement unimplementable.

### D2 -- `fallback` stays literal prose, and a reference in it is refused at compile time

**Decided:** the compiler refuses a `{{KEY}}` reference in
`default_action.fallback`. The runtime behaviour does not change.

**Alternatives:** substitute `fallback` like the other agent-facing strings,
which would make the failure response internally consistent -- the directive
and the prose spliced above it would resolve the same reference the same way.

**Why this wins:** the field's own doc comment already promises literal prose,
and the splice happens after substitution deliberately, so that author prose
is never exposed to expansion. The runtime half is documented, intended
behaviour, not a bug. What is missing is the other half: an author who writes
a reference gets no signal either way. Refusing is the smaller change, it
preserves a documented contract rather than changing one, and it ends the
silence -- which is the whole of the defect. Substituting would be defensible
and would arguably read better in the failure response, but it changes shipped
behaviour to fix a gap that a compile-time error closes without changing
anything.

**Trade-off accepted:** an author who wants a resolving reference in failure
prose must put it in the directive rather than the fallback. The error message
is required to say so, which is why R4 makes pointing at the directive part of
the requirement rather than a nicety.

### D3 -- Both fields are done as one unit

**Decided:** `name_filter` and `fallback` are specified and delivered
together, with the action-declaration enumeration.

**Alternatives:** two separate changes, one per field.

**Why this wins:** they are one defect at two layers. Delivered separately,
the second would either duplicate the enumeration work or land without it, and
the shape that produced four defects would survive either way. The
enumeration is only motivated by having both instances in hand.

## Out of Scope

- **Reconciling the value allowlist with the context-key grammar (koto#227).**
  The two admit different characters. Whether they should converge is a design
  question, and the mismatch already reports itself rather than failing
  silently, so it is not part of this class.
- **An undelivered capture name reaching `sh -c` from a gate command
  (koto#225).** That lives in the refusal path rather than in either
  enumeration, and it is the remaining half of a different pair of fixes.
- **Any rework of variable scoping, lookup order, the overlay, or the value
  forms.** The two enumerations are the boundary of this change.
- **New template surface.** No new field, gate type or substitution form.
- **Substituting `fallback`.** Ruled out by D2 rather than deferred; a future
  change that wants it should reopen D2 rather than treat it as unfinished
  work here.
