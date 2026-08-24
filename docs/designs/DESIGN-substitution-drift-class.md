---
schema: design/v1
status: Accepted
upstream: docs/prds/PRD-substitution-drift-class.md
problem: |
  Which koto template fields resolve {{KEY}} references is decided twice --
  once by the compiler, once by the tick -- from lists maintained by hand in
  different files. Gate fields were unified behind one accessor; action
  declaration fields were not, and the two fields still open (a gate's
  name_filter and a default_action's fallback) are exactly the ones no
  enumeration covers.
decision: |
  Give ActionDecl two accessors next to the struct -- substitutable_fields and
  literal_fields -- and write every accessor, including the existing gate one,
  as an exhaustive destructuring so a new field cannot compile until it has
  been classified. name_filter joins the gate accessor and substitutes through
  the plain form; a name_filter that resolves to empty is refused in the
  children-complete evaluator, following the refusal koto already ships for an
  empty resolved pattern.
rationale: |
  Two promises, so two lists, each with its own error and its own test. The
  destructuring is what converts "remember to update the list" into a compile
  error, which is the only reminder this family of defects has not defeated.
  The plain form and the evaluator-sited refusal are both chosen by what the
  consumer does with the value, which is the axis koto already substitutes on.
---

# DESIGN: closing the substitution-drift class

## Status

Accepted

## Context and Problem Statement

A koto template is read by two parts of koto that have to agree about which
fields resolve `{{KEY}}` references. `Template::compile` decides whether a
reference names something declared; a tick rewrites references into values.
Historically each answered from its own hand-written list, and four consecutive
defects were those lists disagreeing one field at a time.

The most recent of those introduced `Gate::substitutable_fields()`, returning
`Vec<(&'static str, &str)>`. The compiler's gate-reference loop consumes it
directly, and the unit test
`every_field_the_compiler_validates_is_one_the_tick_substitutes` walks it and
fails if a field it names survives a tick still carrying a raw token. That
closed the class for gates.

`ActionDecl` has no counterpart. Its `command` and `working_dir` are validated
by two separate hand-written loops in `Template::compile` and substituted at
two separate call sites in `src/cli/mod.rs`; `fallback` appears in none of
them. The two fields the PRD requires -- a `children-complete` gate's
`name_filter`, and `default_action.fallback` -- are precisely one instance
inside the closed half and one inside the unbuilt half.

Three properties of the code shape the technical problem:

- `name_filter` is `Option<String>`, and it reaches its consumer as
  `gate.name_filter.as_deref()`. At that consumer `None` means "no filter" and
  `Some("")` is a prefix every child name starts with, so the two are
  observationally identical. A reference resolving to empty therefore widens a
  gate from one fan-out to every child.
- `fallback` is spliced onto the failure response's `directive` *after*
  substitution, deliberately, so that author prose is never exposed to
  expansion. That behaviour is documented on the field and the PRD keeps it.
  What has to change is the compiler's silence about a reference written there.
- The two promises differ. `command` and `working_dir` promise "a reference
  here resolves"; `fallback` promises "a reference here is refused". One list
  cannot carry both without a tag.

## Decision Drivers

- **One enumeration per owning struct.** Two hand-maintained lists is the
  defect, not a tidiness question. Anything that leaves a field's participation
  stated in a hand-written site reproduces it.
- **A new field must not be able to skip the enumeration.** An enumeration that
  a maintainer can forget to update has already been defeated four times.
- **Fail closed.** Where a resolved value would silently mean something wider
  than the author asked for, refuse it rather than apply it.
- **Follow the precedent koto already set** for the same question, so there is
  one answer per shape rather than one per site.
- **Do not disturb what works.** The gate accessor, the lookup order, the value
  forms and the overlay stay as they are, except where a driver above requires
  otherwise.

## Considered Options

### Decision 1 -- the shape of the `ActionDecl` enumeration

**A. One `substitutable_fields()` for `command` and `working_dir`, with
`fallback` validated by a hand-written check beside it.** The smallest diff and
an exact twin of the gate accessor. Rejected because it reproduces the original
defect for `fallback`: that field's participation would live in a hand-written
site, which is the thing this design exists to remove.

**B. Two accessors, `substitutable_fields()` and `literal_fields()`.** Each
carries one promise, so each can carry its own compiler error and its own test.

**C. One accessor returning `(name, raw, policy)` with a `RefPolicy` enum.**
Reads well and makes "every reference-bearing field is in exactly one category"
a single list. Rejected because every consumer immediately branches on the tag,
which is the same two code paths with a type in front, and because it forces
the same shape onto `Gate`, which has no literal fields -- relocating the
asymmetry rather than removing it.

**Chosen: B.**

### Decision 2 -- what forces a new field into the enumeration

**A. Nothing structural; rely on the doc comment.** What the gate accessor does
today, and what the four defects already defeated.

**B. Exhaustive destructuring inside each accessor.** Bind every field of the
struct by name and build the list from the bindings, so adding a field stops
the accessor compiling until the author names it and decides which list it
belongs to.

**C. A derive macro classifying fields by attribute.** The strongest guarantee
and much the most machinery, for two structs with eight fields between them.
Rejected on cost.

**Chosen: B**, applied to both `ActionDecl` accessors and to
`Gate::substitutable_fields`, so the guarantee belongs to the pattern rather
than to one struct.

### Decision 3 -- where the empty-`name_filter` refusal lives

**A. In `substitute_gate_fields`.** Wrong twice: the function returns a map
with no error channel, and it runs over every gate regardless of type, so a
stray `name_filter` on a `command` gate would fail runs it does not affect.

**B. In `evaluate_children_complete`, before the value reaches
`build_children_complete_output`.** The evaluator for the one gate type that
reads the field. It already returns `StructuredGateResult` and already
separates `Error` from `Failed`.

**C. In `build_children_complete_output`.** Deeper than the decision belongs;
that helper builds output for a converge predicate rather than deciding gate
outcomes.

**Chosen: B.** It is the structural twin of
`evaluate_context_matches_gate`'s refusal of a `pattern` that resolved to
empty, which koto shipped for this same reason. It is also the last point at
which the distinction survives: the value crosses into
`build_children_complete_output` as `Option<&str>`, where `Some("")` is a
prefix every name starts with.

**Not also refused at compile time.** The compiler could reject an authored
`name_filter: ""` the way it rejects an authored empty `pattern`. Left out: the
runtime refusal already covers the authored case, and a new compile-time
rejection would refuse templates that are legal today for a benefit already
delivered.

### Decision 4 -- which value form `name_filter` substitutes through

**A. Plain.** The consumer reads the value as itself, in a prefix comparison
that parses nothing further.

**B. Shell-safe.** Rejected, and actively wrong rather than merely
unnecessary. The form exists to render an empty value as `''` so a shell word
does not vanish; `name_filter` never reaches `sh -c`, and `''` is two literal
quote characters no child name starts with -- which would convert a
resolved-empty value from a detectable error into a prefix that silently
matches nothing, defeating Decision 3.

**C. Regex-literal.** Rejected. `name_filter` is not compiled as a regex, so
escaping would insert backslashes into a literal prefix and stop it matching
the names it was written for.

**Chosen: A**, on the axis koto already substitutes by -- what the consumer
does with the value -- and the same form `key` and `working_dir` take.

## Decision Outcome

`name_filter` joins `Gate::substitutable_fields()`. That single edit wires it
into the compiler's reference validation, because that loop reads the accessor.
The runtime half is wired by handling it in `substitute_gate_fields` through
`substitute_plain`, preserving `Option` rather than collapsing it. The existing
guard test fails between those two edits, which is the guard working.

`ActionDecl` gains `substitutable_fields()` naming `command` and
`working_dir`, and `literal_fields()` naming `fallback`. The compiler's two
hand-written action-field loops collapse into one loop over the first accessor
and one over the second, each with its own error: an undeclared reference for a
substitutable field, any reference at all for a literal one. Both accessors,
and the gate one, are written as exhaustive destructurings, so a field added to
either struct stops the build until it is classified.

A `children-complete` gate whose `name_filter` is `Some` and empty after
substitution is refused in `evaluate_children_complete` with an error naming
the reason. A `name_filter` that is `None` behaves exactly as it does today.

The four decisions do not conflict and one depends on another: Decision 4's
plain form is what keeps a resolved-empty value empty, which is what Decision
3's refusal detects. Choosing the shell-safe form would have made Decision 3
unimplementable at the site chosen for it.

## Solution Architecture

### Enumerations, in `src/template/types.rs`

```
impl Gate {
    fn substitutable_fields(&self) -> Vec<(&'static str, &str)>
        // ("command", ..), ("key", ..), ("pattern", ..),
        // and ("name_filter", ..) only when Some
}

impl ActionDecl {
    fn substitutable_fields(&self) -> Vec<(&'static str, &str)>
        // ("command", ..), ("working_dir", ..)
    fn literal_fields(&self) -> Vec<(&'static str, &str)>
        // ("fallback", ..) only when Some
}
```

The `Vec` return, rather than a fixed-size array, is what lets an
`Option<String>` field join at all: an absent field contributes no entry, and
there is no borrow of a `String` that does not exist. Each body opens with an
exhaustive destructuring of `self`, naming every field including the
non-strings, so the struct and the enumeration cannot diverge without a
compile error.

### Compiler, in `Template::compile`

The gate loop is unchanged in shape; it picks up `name_filter` for free
because it reads the accessor. The action block replaces its two hand-written
reference loops with two loops over the new accessors:

- over `substitutable_fields()`: an undeclared reference is an error naming the
  state, the field, and the reference -- the wording the two replaced loops
  already used, now produced once.
- over `literal_fields()`: *any* reference is an error naming the state and the
  field, saying `fallback` is literal prose that is never expanded, and
  pointing at the directive as the place a reference resolves.

Everything else the action block validates -- the empty-command rejection, the
absolute-`working_dir` rejection, the polling-timeout rule -- is untouched.

### Runtime, in `src/cli/mod.rs`

`substitute_gate_fields` gains one arm:

```
g.name_filter = gate.name_filter.as_deref()
    .map(|f| substitute_plain(f, runtime_vars, variables, overlay));
```

`None` maps to `None`. Both existing call sites -- the top-level gate closure
and the one inside the `default_action` polling loop -- go through this helper
already, so the polling site is covered without a second edit. That site is the
one an earlier fix in this family found had drifted, which is why it is called
out rather than assumed.

`evaluate_children_complete` gains a refusal before it calls
`build_children_complete_output`:

```
if matches!(gate.name_filter.as_deref(), Some("")) { -> GateOutcome::Error }
```

with an error in the shape `evaluate_context_matches_gate` already uses for an
empty resolved `pattern`: what the value would have done, why, and a remedy.

`ActionDecl::literal_fields` has no runtime consumer by design. `fallback`
continues to be read at the response layer and spliced after substitution,
unchanged.

### Tests

`every_field_the_compiler_validates_is_one_the_tick_substitutes` gains
`name_filter: Some("{{TOKEN}}".into())` in its fixture, which its own
staleness assertion already requires. A sibling test does the same for
`ActionDecl::substitutable_fields`. A third asserts that every field
`ActionDecl::literal_fields` names is one the compiler refuses a reference in,
so the literal half has a guard of the same kind rather than only the
substitutable half.

Behavioural regressions go in `tests/`, alongside
`gate_field_substitution_test.rs`, one per PRD requirement R1 through R4.

## Implementation Approach

1. **Enumerations and destructuring.** Add the two `ActionDecl` accessors;
   rewrite all three accessor bodies as exhaustive destructurings; add
   `name_filter` to the gate accessor. At this point the guard test fails,
   naming `name_filter` -- the guard working, and the checkpoint that proves it
   does.
2. **Runtime substitution.** Handle `name_filter` in `substitute_gate_fields`
   through the plain form, preserving `Option`. The guard test goes green.
3. **Compiler.** Replace the two hand-written action reference loops with loops
   over the accessors, and add the literal-field refusal with its error.
4. **Empty refusal.** Add the `Some("")` refusal to
   `evaluate_children_complete`.
5. **Regression tests**, then demonstrate each fails against a checkout of
   `main` with the test file copied in.
6. **Documentation and skills.** Correct the two skill references, the two
   guides, and the source doc comments that name these as open gaps -- including
   the sentence on `Gate::substitutable_fields` that says `name_filter` is
   deliberately absent.

Steps 1 and 2 are deliberately separate commits or at least a deliberate
intermediate state: the failing guard between them is the evidence that the
enumeration is load-bearing.

## Security Considerations

**Reviewed; no new attack surface, and one fail-open case closed.**

`name_filter` substitutes through the plain form and its resolved value is used
only in a prefix comparison against workflow names returned by the session
backend. It is never handed to `sh -c`, never compiled as a regex, never joined
to a path, and never used as a context-store key, so none of the escaping
concerns that shaped the `command`, `pattern` and `working_dir` forms apply.
Values reaching it are already constrained by koto's value allowlist, and
`SESSION_NAME` by `validate_workflow_name`.

The change is fail-closed in both directions it moves. A `name_filter` that
resolves to empty currently widens a gate to every child of the parent -- a
gate written to wait on one fan-out silently waits on all of them, which can
pass a gate on children the author never meant it to observe. Refusing it
removes that. Refusing a reference in `fallback` at compile time removes no
capability: the runtime never expanded it, so nothing that works today stops
working.

No new input crosses a trust boundary, no data is newly persisted, and no error
message introduced here echoes a value -- each names the field and the
reference, not the resolved content.

## Consequences

**Positive.** The two structs that own reference-bearing fields each enumerate
them in one place, and the enumeration is compiler-enforced rather than
remembered: adding a field to `Gate` or `ActionDecl` stops the build until it
is classified. The compiler's answer and the tick's cannot disagree for either
struct. Two documented gaps close, and the guides and skills stop asserting
behaviour koto no longer has.

**Negative.** The exhaustive destructuring means a field with nothing to do
with substitution -- `requires_confirmation`, `polling`, `timeout` -- must
still be named and dropped in three accessor bodies, which reads as noise to
someone who has not hit this class of defect. The accessor doc comments carry
the reason so the next reader does not simplify it back to `..`.

A template that today writes `fallback: "see {{SESSION_DIR}}"` and compiles
clean will stop compiling. That is the intended behaviour change and it is the
whole of what an author gains, but it is a change: a template that ran
yesterday can be refused today. The error is required to say what to do
instead.

**Mitigations.** The failing-guard checkpoint between implementation steps 1
and 2 is preserved deliberately, so that the mechanism is demonstrated rather
than asserted. The regression tests are required to fail against `main`, run
against a real checkout rather than a stash, so the before-state is evidence
rather than a claim.
