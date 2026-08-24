---
schema: brief/v1
status: Accepted
problem: |
  koto's compiler and its tick each carry their own hand-maintained idea of
  which template fields resolve {{KEY}} references, in different files. When
  the two disagree an author gets no signal: the raw token passes through and
  the failure surfaces somewhere else entirely.
outcome: |
  Every string field of a koto template gives an author a definite answer
  about a {{KEY}} reference -- it resolves, or the compiler names the field
  and refuses -- and which fields those are is enumerated once per owning
  struct, so a field added later cannot land outside the answer.
motivating_context: |
  Four consecutive defects were the same drift one field at a time: a
  default_action command the compiler accepted and the tick skipped, a gate
  key and pattern in neither, a gate name_filter still in neither, and a
  default_action fallback in neither. The third fix built the enumeration for
  gates and left the ActionDecl half of the shape unbuilt.
---

## Status

Accepted

The brief frames the class and stops before requirements. Which of the two
open fields substitutes and which refuses, and what an empty substituted value
means, are decisions the downstream PRD and DESIGN own.

## Problem Statement

A koto template is authored once and read by two different pieces of koto. The
compiler reads it at `koto init` and decides whether every `{{KEY}}` reference
in it names something declared. The tick reads it on every `koto next` and
rewrites those references into values. Both need to know which fields
participate, and until recently each answered that question from its own
hand-written list, in a different file, with nothing comparing them.

When the two lists disagree, the author is told nothing. That is the specific
harm, and it is worse than either half failing loudly. A field the compiler
validates but the tick skips gives the author a green compile and a raw token
at run time. A field neither touches gives the author a green compile and a
raw token, with no diagnostic anywhere naming the reference that did not
resolve. In both directions the symptom appears at a distance from the cause:
a gate that counts zero children and blocks forever, a context key the store
reports absent under a name spelled with braces, prose handed to an agent that
names a session directory the agent cannot find.

Four defects in a row were instances of exactly this, one field at a time. A
`default_action` command was validated and not substituted. A context gate's
`key` and `pattern` were in neither list. A `children-complete` gate's
`name_filter` is in neither. A `default_action`'s `fallback` is in neither,
and its own doc comment says the runtime half is deliberate -- which leaves
the compiler's silence as the whole of the defect.

The fix for the third of those built the missing machinery for one of the two
structs that own such fields: gate fields are now enumerated once, next to the
struct, and a unit test fails if the compiler validates a field the tick does
not resolve. The action-declaration half of the same shape does not exist. So
the class is half-closed, and the two fields still open are precisely the two
that the closed half does not cover and the unbuilt half would.

Fixing those two fields individually would leave the shape that produced all
four intact, and the next field added to an action declaration would be
discovered the same way the last four were -- by someone hitting it.

## User Outcome

A template author who writes a `{{KEY}}` reference into any field of a koto
template learns what koto will do with it, at the moment they can still change
it. Either the reference resolves and the author can rely on that, or the
compiler refuses at `koto init` and names the state, the field, and the
undeclared reference. There is no third case where the author is told nothing
and finds out later from a symptom that does not mention variables.

The same holds for a value that only becomes a problem after it resolves. A
reference that resolves to an empty string in a position where empty means
something dangerously different from what the author wrote -- a name prefix
that scopes a gate to one fan-out, where empty means every child -- is refused
at the point of use with a reason naming the reference, rather than silently
widening.

For the maintainer, which fields participate is a property of the struct that
owns them, written down once. Adding a field to that enumeration is what wires
it into compile-time validation, and the test suite refuses the change until
the runtime side is wired too. The two halves cannot drift apart again without
a test going red.

## User Journeys

### Scoping a fan-out gate by the parent's own name

A workflow author writes a state that spawns research children as
`{{SESSION_NAME}}.research.<n>` and gates the next state on those children
completing. The natural way to scope that gate is
`name_filter: "{{SESSION_NAME}}.research."`. Today the reference reaches the
gate verbatim, no child name starts with a literal `{{SESSION_NAME}}`, the
gate counts zero matching children and blocks, and the compile-time warning
that would have caught a missing trailing dot says nothing because the
trailing dot is there. After this work the reference resolves, and a reference
to a variable nobody declared is refused at compile time naming the gate and
the field.

### Writing failure prose that points at the session

An author gives a `default_action` a `fallback` that tells the agent where to
look when the command fails, and reaches for `{{SESSION_DIR}}` to name the
place. Today that compiles clean and the agent receives the braces, in the
same response whose directive resolved the same reference two lines below.
After this work the author gets a definite answer at `koto init` -- the
reference resolves, or the compiler says `fallback` is literal prose and
points at where a resolving reference belongs -- rather than discovering the
disagreement from an agent that could not follow the pointer.

### Adding a substitutable field as a maintainer

A koto maintainer adds a new string field to an action declaration and wants
it to accept references. They add it to the owning struct's enumeration of
substitutable fields. Compile-time validation picks it up from that
enumeration with no second edit. The guard test then fails, naming the field,
until the tick's substitution is wired too. The maintainer never has to know
that a second list exists somewhere else, because there is no second list.

## Scope Boundary

**In scope**

- A `children-complete` gate's `name_filter` participating in substitution and
  in compile-time reference validation, with a deliberate answer for what a
  value that resolves to empty means -- given that `name_filter` is optional
  and "absent" and "resolved to empty" are genuinely different states.
- A `default_action`'s `fallback` getting a definite compile-time answer,
  whether that is substitution or an explicit refusal of a reference in a
  field documented as literal prose.
- The action-declaration counterpart of the gate enumeration, so both the
  compiler and the tick read one list per owning struct, and a guard test
  covers the action half the way one already covers the gate half.
- The shipped documentation and skill guidance that currently assert the
  behaviour this work changes, updated in the same change.
- Regression coverage per field that fails against the current `main`.

**Out of scope**

- Reconciling koto's value allowlist with its context-key grammar. The two
  admit different characters, which is a design question about whether they
  should converge -- and the mismatch already reports itself rather than
  failing quietly. Tracked separately as koto#227.
- An undelivered capture name reaching `sh -c` from a *gate* command. That is
  the remaining half of a different pair of fixes, it lives in the refusal
  path rather than in the substitution lists, and it is tracked as koto#225.
- Any wider rework of how variables are scoped, resolved, or layered. The two
  enumerations are the boundary; the lookup order, the overlay, and the value
  forms are not being revisited.
- New template surface. Nothing here adds a field, a gate type, or a
  substitution form that authors did not already have.

## References

- `docs/designs/current/DESIGN-template-variable-substitution.md` -- the
  substitution model these fields participate in.
- `docs/designs/current/DESIGN-gate-contract-compiler-validation.md` -- what
  the compiler is responsible for catching before a workflow runs.
- `docs/designs/current/DESIGN-koto-runs-commands.md` -- why `fallback` is
  spliced onto the failure directive after substitution.
