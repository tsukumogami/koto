---
schema: brief/v1
status: Accepted
problem: |
  koto has two character grammars that were never reconciled: what a variable
  value may hold, and what a context key may hold. The value set is wider by a
  space, a colon and an at-sign. Nothing states the boundary, and koto answers a
  key that crosses it as absent rather than as wrong at every surface but one.
outcome: |
  A template author knows the rule before they hit it, because koto states it
  where they are reading, and koto says which character it refused whenever the
  boundary is crossed -- at a gate and at the command line alike -- instead of
  answering a bad key with a bare "no".
motivating_context: |
  A context gate's key started substituting variable references, which is what
  first put a value's characters into a key's position. That change (koto#222)
  gave the gate surface a diagnostic at the same time; the grammar question
  underneath it, and the surfaces that still answer silently, were deferred to
  koto#227, which this brief frames.
---

# BRIEF: Reconciling the Value and Context-Key Character Sets

## Status

Accepted

The two-reviewer jury returned all-PASS. The three questions this brief
deferred -- how the grammar decision lands, how far the silent-answer fix
reaches, and whether an always-invalid authored key should now report -- carry
into the downstream PRD's Decisions and Trade-offs, which owns them.

## Problem Statement

koto validates two kinds of user-supplied string against two different
grammars, and nobody ever wrote down how the two relate.

A **variable value** may hold letters, digits, `.`, `_`, `/`, `-`, and also a
space, a `:` and an `@`. Those last three are deliberate: a value is often a
calendar title, or a filter expression like `from:user@example.com`, and the
set was widened on purpose to admit them (koto#180).

A **context key** may hold much less. It splits on `/`, and each component must
begin with a letter or digit and continue in letters, digits, `.`, `_` and `-`.
No space, no `:`, no `@`. It is narrow for a concrete reason: a key becomes a
path component on disk, and a key store that accepted arbitrary text would be
writing filenames it cannot promise to read back.

The value set is therefore wider than the key set by exactly three characters,
and the two grammars are separated by one substitution step. A value that koto
documents as legal, and whose legality koto's own tests assert, can be
substituted into a gate's key and produce a key koto refuses. `Weekly Planning`
is a legal value; `Weekly Planning-note` is not a legal key.

Two things follow, and they are the problem.

**The rule is unwritten.** No guide, no skill, and no error message an author
sees before the fact says that a value may hold characters a key may not. An
author writing `key: "{{TITLE}}-note"` has no way to learn where the boundary
sits except by crossing it. The asymmetry is currently recorded only in two
Rust doc comments, which are read by people changing koto rather than by people
using it.

**Most surfaces answer silently.** A gate now names the offending character,
but a gate is one caller. The store's own existence check reports a key that
fails the grammar as absent, and the command-line verb over it exits with the
same status for "there is no such key" and "that is not a key at all". A
template that shells out to check for a key -- which is the ordinary way a
command gate or a fallback action does it -- gets a bare no with nothing
pointing at why, which is the same defect the gate surface was just fixed for,
one call site over.

Neither half is a crash or a wrong answer. Both are a workflow that will not
advance while koto declines to say what is wrong with it, and an author with
nothing to grep for.

## User Outcome

A template author who puts a variable reference inside a context key finds the
rule stated where they are already reading, in the same place that tells them
what a value may hold. They learn that the two sets differ, by which three
characters, and why the narrower one is narrow -- before they write the
template, not after a gate refuses to open.

When an author does cross the boundary, koto tells them so in the same terms
wherever they meet it. A gate that cannot use its key says which character it
refused and in which component. The command-line existence check stops
answering "not a key at all" with the same silence it uses for "not here", so a
template that probes for a key by shelling out gets a reason too.

And an operator who reads koto's answer can act on it. The message names the
character, so the fix -- change the value, or change the key -- is the next
thing they do rather than the thing they work out from first principles.

## User Journeys

### An author writes a title into a key

A template author has a variable `TITLE` holding a calendar entry, `Weekly
Planning`. They write a context gate with `key: "{{TITLE}}-note"`, because a
per-title note is the obvious way to scope the check. The gate blocks. Instead
of a bare absent, koto tells them the key it was handed contains a space, names
the component it appears in, and says that a variable value may hold a space
where a context key may not. The author scopes the key on a slug variable
instead and moves on, having spent a minute rather than an afternoon.

### An author reads the rule before writing the template

A different author is writing their first koto template and is deciding how to
name the keys their workflow will store. They read the skill that documents
template authoring. It states both grammars next to each other, says the key
grammar is the narrower of the two, and says which characters a value may carry
that a key may not. They pick key names that will survive substitution, and
never meet the failure at all.

### A template probes for a key from a command

A workflow's fallback action shells out to koto's existence check to decide
whether to seed a context key or reuse it, passing a key composed from a
variable. The value carries an `@`. Today the probe answers no and the workflow
seeds a key it will then fail to read back under the same name. After this
work, the probe distinguishes the two cases: an unusable key is reported as
unusable, with the character named, rather than as an ordinary absence.

### A maintainer asks why the sets differ

A contributor picks up work near the context store and wants to know whether
the two grammars are supposed to converge, and whether narrowing the value set
or widening the key set was ever considered. They find the decision written
down with its reasoning -- which option was chosen, what each alternative would
have cost, and what the storage constraint on keys actually is -- rather than
inferring an answer from what the code happens to do.

## Scope Boundary

**In:**

- Deciding, explicitly and on the record, whether the two character sets
  converge or stay divergent, and writing the decision down with the reasoning
  for a future reader -- including what widening the key set would cost given
  that keys become path components, and what narrowing the value set would
  break.
- Closing the remaining silent answer on the context-store existence path, so
  that a key which fails the grammar is distinguishable from a key that is
  merely absent at the surfaces a template can reach.
- Stating the relationship between the two grammars in the author-facing
  documentation and skills, where someone writing a template will meet it.
- Test coverage for the motivating case: a value koto documents as legal -- one
  carrying a space, a `:`, or an `@` -- flowing through substitution into a
  context gate's key.

**Out:**

- **Widening what a variable value may hold.** The value set was widened
  deliberately for filter expressions and titles; touching it again is a
  decision in its own right, not a step taken in passing.
- **The three sibling issues in the same family** -- koto#224, a
  `children-complete` gate's name filter not being substituted; koto#225, an
  undelivered capture name reaching a shell in a gate command; and koto#228, a
  fallback's prose being neither substituted nor validated. Each is a field sitting outside the substitution
  or validation system; this brief is about the grammar two of those fields
  would be validated against, not about which fields are wired up. A sibling is
  settled here only if the grammar decision genuinely answers it, and the
  downstream PRD says which and why.
- **Reworking substitution as a system.** Consolidating every field koto reads
  into one substitution and validation pass is a larger change with its own
  framing; four related issues are a family, not a mandate for it.
- **Cutting a release.** Getting this in front of users is a separate call.

## References

- `docs/designs/current/DESIGN-template-variable-substitution.md` -- the
  substitution model the value grammar belongs to.
- `docs/designs/current/DESIGN-local-session-storage.md` -- the storage model
  the key grammar is narrow for.
