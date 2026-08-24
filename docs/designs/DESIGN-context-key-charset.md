---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-context-key-charset.md
problem: |
  A koto variable value may hold a space, a `:` or an `@`; a context key may
  hold none of the three, and one substitution step separates the two grammars.
  The gate surface reports the crossing, but `koto context exists` still answers
  an unusable key with the same silent non-zero status it uses for a key that is
  simply absent, and nothing an author reads states that the two grammars differ
  at all.
decision: |
  The grammars stay divergent. A single message-composing function next to
  `validate_context_key` becomes the one place a refusal is worded, the context
  gate is re-pointed at it, and `koto context exists` grows a third outcome that
  uses it -- exit 2, koto's existing "the caller must fix the input" status,
  carrying the flat JSON error the CLI already uses. The asymmetry is then
  written into the CLI reference and the template-authoring skill, and the
  reasoning for keeping the grammars apart is recorded here.
rationale: |
  A key is an address with three consumers -- a path component, a manifest key,
  and a shell argument in the commands templates run -- and koto substitutes a
  non-empty value into a command verbatim, so widening the key grammar to admit
  a space would legalize keys that word-split the moment a template passes one
  along. Narrowing the value grammar instead would break the titles and filter
  expressions it was widened for. That leaves making the boundary legible, which
  is a diagnostic change at two call sites and a documentation change, with no
  change to storage, the trait, or the wire format.
---

# DESIGN: Reconciling the Value and Context-Key Character Sets

## Status

Planned

## Context and Problem Statement

koto validates two kinds of user-supplied string against two grammars.

`VALUE_PATTERN` in `src/engine/substitute.rs` admits `[a-zA-Z0-9._/:@ -]` for a
variable value. `validate_context_key` in `src/session/validate.rs` admits a
`/`-separated key whose components each begin with an alphanumeric and continue
in alphanumerics, `.`, `_` and `-`, refusing `.` and `..` components and
leading, trailing or doubled slashes.

The value set is wider by three characters, and since a context gate's `key`
began resolving `{{KEY}}` references the two grammars meet on every tick that
evaluates such a gate. `key: "{{TITLE}}-note"` with `TITLE` set to `Weekly
Planning` produces `Weekly Planning-note`, which the store refuses.

The refusal itself is correct. What is wrong is everything around it.

**One surface reports it and the others do not.** `unusable_key_result` in
`src/gate.rs` validates a substituted key before handing it to the store and
returns a `GateOutcome::Error` naming the character. That covers the two context
gate types and nothing else. `LocalBackend::ctx_exists` reports a key that fails
the grammar as absent, and `koto context exists` maps that bool straight onto
exit 0 or exit 1 with no output at all. A template that probes for a key by
shelling out -- which a command gate or a `default_action` routinely does --
receives a bare no and proceeds as though the key were merely missing.
`docs/guides/cli-usage.md` already concedes the point in prose.

**The rule is unwritten.** The asymmetry is recorded in two Rust doc comments,
on `unusable_key_result` and on the `ContextStore` trait. Neither is read by
someone writing a template. No guide and no plugin skill says a value may carry
characters a key may not, so the boundary is discovered by crossing it.

**And the question underneath was never answered.** Whether the two sets should
converge has only ever been implied by what the code does. A maintainer
proposing to widen the key grammar or narrow the value grammar today would be
arguing against nothing.

The requirements this design answers are R1 through R10 in
`docs/prds/PRD-context-key-charset.md`.

## Decision Drivers

- **A key is an address, not content.** Whatever the key grammar admits has to
  survive being a filesystem path component, a manifest key, and a shell
  argument. That constraint is what makes the two grammars different, and any
  option that ignores it moves the failure later rather than removing it.
- **koto does not shell-quote a substituted value.** The shell-word form exists
  for the empty case and falls through to the plain form otherwise, so a value
  containing a space reaches `sh -c` as two words.
- **koto#180's cases are shipped behaviour.** Titles and filter expressions in
  values are the reason the value grammar is wide, and breaking them to fix a
  diagnostic is a bad trade in any direction.
- **One wording, or the two surfaces drift.** The gate and the CLI answer the
  same question about the same key. Two independently maintained messages would
  diverge, and an operator would learn which surface they were on from the
  wording rather than from the problem.
- **A non-zero status must stay non-zero.** Callers today branch on success
  versus failure. A new outcome that flipped an unusable key to success, or that
  turned a shell conditional inside out, would break templates to improve a
  message.
- **The store's contract is load-bearing elsewhere.** `ContextStore` is a public
  trait with implementations beyond the local backend; a signature change costs
  every implementation and every call site.

## Considered Options

### Decision 1 -- Do the two grammars converge?

**Option 1A: keep both grammars as they are** (chosen). The value set stays
wide, the key set stays narrow, and the work is to make the boundary legible
and consistently reported.

**Option 1B: widen the key grammar** to admit a space, `:` and `@`, so every
legal value is a legal key. This is the option that would make the asymmetry
disappear rather than document it, and it is genuinely attractive: an author
with a title in a variable could use it directly, and the diagnostic this design
adds would have nothing left to report.

It fails on the third consumer of a key. A key is not only a path component and
a manifest entry; it is an argument in the `koto context add` and `koto context
get` commands that templates run, and koto substitutes a non-empty value into a
command verbatim. A key containing a space therefore word-splits at the point of
use. Widening the grammar would move the failure from "the store refuses this
key, and now says why" to "the store accepts this key and a later command
silently addresses a different one" -- later, quieter, and harder to attribute.
The filesystem argument is the weaker one and worth stating honestly: koto ships
for Linux and macOS only, so a `:` in a filename is awkward on macOS rather than
fatal, and it would not on its own have decided this.

Widening is also close to irreversible. Keys written under a wide grammar sit in
a session's context directory and its manifest; narrowing afterwards orphans
them.

**Option 1C: narrow the value grammar** to the key's character set, so the
crossing cannot arise. This has the same "the asymmetry disappears" appeal and a
smaller blast radius in the code -- one constant.

It fails against what values are for. A value ends up in a directive an agent
reads, in a command's argument, in a regex a gate compiles. Human text belongs
there. The three characters at issue are exactly the ones koto#180 admitted so
that a value could be a calendar title or a filter like
`from:user@example.com`, and removing them would break shipped, deliberate
behaviour in order to improve an error message.

**Option 1D: keep the grammars apart and add a key-safe rendering of a value**
at the substitution site -- a form that slugs a value into something the key
grammar accepts. This is the option that resolves the asymmetry as an
*expressive* limit rather than a diagnostic one, and it is the right shape if
the limit turns out to bite.

It is not the right shape now. It adds a substitution form and a rendering rule
to a surface that has three of them, in service of a use case nobody has
reported hitting; the reported problem is that the failure is silent, not that
the author cannot express what they meant. Deferring it costs nothing, because
the diagnostic this design lands is what would tell us whether anyone needs it.

### Decision 2 -- Where does the CLI's check live?

**Option 2A: in the CLI handler** (chosen). `handle_exists` performs the same
caller-side check the gate performs, and reports a third outcome upward.

**Option 2B: change `ContextStore::ctx_exists`** to return a result rather than
a bool, so every caller receives the validation error. This is the option that
would make it impossible to forget the check, which is a real advantage over
2A's convention.

It costs more than it buys here. The trait is implemented beyond the local
backend and consumed at call sites that do not care; the change ripples through
all of them to serve two that do. It also contradicts the shape that already
shipped: the gate checks caller-side, so a trait-side mechanism would leave koto
with two different answers to one question. The trait's own documentation
already warns that a `false` does not distinguish the two cases, which is the
cheap version of what 2B enforces.

**Option 2C: validate at argument parsing**, rejecting an unusable key before
any `context` verb runs. Uniform and early, and it would cover `add`, `get` and
`remove` at the same time.

It is redundant for those three -- they already surface the validation error
through their result type -- and it would change their error shape for no gain.
`exists` is the outlier because its contract is a boolean, so the fix belongs
where the boolean is produced.

### Decision 3 -- What does the CLI report for an unusable key?

**Option 3A: exit 2 with the flat JSON error** (chosen). koto already uses exit
2 for "the caller must fix the input" -- `invalid_submission`,
`precondition_failed`, `workflow_not_initialized` -- and an unusable key is
exactly that condition. It is distinct from 0 and from 1, and it is non-zero, so
a caller that treats any failure as "not present" is unaffected.

**Option 3B: exit 3**, the infrastructure status. Wrong category: exit 3 is for
a template that will not compile, a disk that will not write, an anchor that
does not resolve. A malformed key is the caller's, not the machine's, and
filing it under infrastructure would tell an operator to look at their
environment.

**Option 3C: keep exit 1 and print the reason.** The smallest possible change,
and it preserves every existing caller exactly. It fails the requirement rather
than trading against it: R2 asks for the three outcomes to be *told apart*, and
a caller reading only the status still cannot. Printing without a distinct
status also makes the reason invisible to a command gate, which captures output
but routes on the status.

### Decision 4 -- Where does the shared message live?

**Option 4A: beside `validate_context_key` in `src/session/validate.rs`**
(chosen). The message explains a refusal that function produced, and both
consumers -- `src/gate.rs` and `src/cli/context.rs` -- already depend on the
`session` module. Putting the wording next to the rule it describes means the
next person to change the grammar sees the message in the same file.

**Option 4B: export it from `src/gate.rs`**, where it lives today. It would be
the smaller diff, but it makes the CLI depend on the gate evaluator for a
message that has nothing to do with gates, and it leaves the wording a file
away from the rule.

**Option 4C: leave both surfaces to word it themselves.** Rejected by R4: two
messages for one condition drift, and the drift is invisible until someone
compares them.

## Decision Outcome

The grammars stay as they are (1A). A key is an address with three consumers and
a value is content; converging them means giving up one of those two facts, and
neither is wrong. The asymmetry is real and stays -- what changes is that it is
written down, argued for, and reported the same way wherever it is met.

`validate_context_key` gains a companion in the same file that turns a refusal
into the one sentence both surfaces print (4A). `src/gate.rs` keeps its
`unusable_key_result` shape and calls that companion for the wording. The CLI's
`handle_exists` grows the same check (2A) and reports three outcomes instead of
two, which the command layer maps onto exit 0, exit 1, and exit 2 with the flat
`{"error": ..., "command": ...}` body koto's other verbs already emit (3A).

Nothing else moves. The store's trait, the on-disk layout, the manifest, the
session log, and both character sets are untouched, so the change is a
diagnostic one at two call sites plus documentation.

The key-safe rendering (1D) is explicitly deferred rather than rejected on the
merits. If the asymmetry turns out to be an expressive limit, that is the shape
that resolves it, and the diagnostic landing here is what would surface the
evidence.

## Solution Architecture

### Components

| Component | Change |
|---|---|
| `src/session/validate.rs` | Gains a message-composing companion to `validate_context_key`: given a key, it returns `None` when the key is usable and `Some(String)` carrying the operator-facing reason when it is not. The reason names the offending character and component -- which `validate_context_key`'s own error already does -- and adds the remedy sentence about the two grammars. |
| `src/gate.rs` | `unusable_key_result` stops composing its own text and calls the companion, keeping its `Option<StructuredGateResult>` signature and its `field` parameter so the `exists` and `matches` evidence keys are unchanged. |
| `src/cli/context.rs` | `handle_exists` calls the companion before the store. Its return type widens from `bool` to a three-outcome value: present, absent, or unusable with the reason. |
| `src/cli/mod.rs` | The `ContextCommand::Exists` arm maps the three outcomes onto exit 0, exit 1, and a new caller-error exit that emits the flat JSON error under `"command": "context exists"`. |
| `docs/guides/cli-usage.md` | The `context` section states both grammars and their three-character difference; the `context exists` entry documents the third status and its output, and the standing note about the probe's imprecision is rewritten to match. |
| `docs/reference/error-codes.md` | Gains the `context exists` condition alongside the other per-command entries. |
| `plugins/koto-skills/skills/koto-author/` | States the two grammars where an author choosing key names will read it, including that a `{{KEY}}` reference inside a gate's `key` is subject to the narrower one. |

### Data flow

Nothing changes about how a key reaches the store. Both new call sites sit
immediately before an existing store call and answer from the key alone:

```
gate tick:  substitute_gate_fields -> evaluate_*_gate -> [companion] -> ctx_exists/get
CLI verb:   clap parse             -> handle_exists   -> [companion] -> ctx_exists
```

The companion is pure: a key in, an optional reason out. It performs no I/O,
touches no session, and cannot fail.

### Interfaces

`ContextStore` is unchanged, including `ctx_exists`'s bool return and its
documented behaviour of reporting an unusable key as absent. The gate evidence
shapes are unchanged: an unusable key still yields `{"exists": false, "error":
<reason>}` or `{"matches": false, "error": <reason>}`, so a `when` clause
routing on either key still has something to read.

The one surface that changes is `koto context exists`'s exit status, and only
for input that is not a key at all.

## Implementation Approach

The work is one atomic change: the message move, the two call sites, the tests,
and the documentation land together, because splitting them would leave either
two wordings in the tree or a documented behaviour that does not exist.

1. **Move the wording.** Add the companion beside `validate_context_key` with
   unit tests over the three characters at issue and the leading-hyphen case.
   Re-point `src/gate.rs` at it. The existing gate tests are the regression
   guard for this step: they assert on substrings of the message, so a wording
   change that lost content fails them.
2. **Widen the CLI outcome.** Change `handle_exists`'s return type, add the
   command-layer mapping and the new exit status, and add integration coverage
   for all three outcomes, asserting the unusable status is both non-zero and
   distinct.
3. **Cover the motivating case end to end.** Add a test that drives a value
   carrying a space, a `:` and an `@` through substitution into a context gate's
   `key` and asserts the message names the character. This is the case
   koto#227 was filed about and it has no test today.
4. **Assert the single wording.** A test that puts the gate's message and the
   CLI's message for the same key side by side, so 4A's guarantee is mechanical
   rather than conventional.
5. **Write the rule down.** The CLI reference, the error-code reference, and the
   template-authoring skill, per the component table.

Demonstrating the regression is part of step 2 and 3 rather than an assertion
about them: the new test files are copied into a worktree of `main` and shown
failing there before the change is credited.

## Security Considerations

**No new input reaches a privileged surface.** The companion is a pure function
over a string koto already holds; it performs no I/O and spawns nothing. The
key it inspects has already been through substitution, and substitution's own
value validation is unchanged.

**The change is in the refusing direction.** Every key that reaches the store
after this change reached it before, and one class of key that previously
reached `ctx_exists` and was answered `false` is now refused before the call.
Nothing that was rejected becomes accepted, so no path-traversal or
containment property weakens. The `.` and `..` component rules, the leading and
trailing slash rules, and the length bound are untouched and continue to be
enforced by the same function.

**The message quotes a key back to the caller.** It is emitted to a local
process's error stream and to gate evidence in the session log, both of which
already carry the same key verbatim -- `ctx_add`'s events record it, and the
gate's evidence records the substituted key today. The key is bounded by
`VALUE_PATTERN` and the 255-character key limit, so the message cannot be
made unbounded or made to carry a control sequence by an attacker who controls
a variable value. It is rendered with the debug formatter, which escapes what
it prints.

**The new exit status is not a downgrade.** An unusable key produces a non-zero
status both before and after. A caller that treats non-zero as "do not proceed"
behaves identically; a caller that distinguishes statuses gains information it
did not have. There is no direction in which a check that used to fail now
passes.

## Consequences

### Positive

- An operator who hits the boundary is told which character was refused and in
  which component, at whichever surface they hit it, in the same words.
- The convergence question has an answer with reasoning attached, so the next
  proposal to move either grammar argues against something.
- `koto context exists` stops conflating "not a key" with "not here", which
  removes a class of workflow that proceeds on a false premise.
- The two Rust doc comments that currently carry the asymmetry stop being its
  only record.

### Negative

- The asymmetry remains. An author with a title in a variable still cannot use
  it directly as a key, and this design makes that a documented limit rather
  than removing it.
- `koto context exists` changes behaviour for input that was always invalid, not
  only for input that became invalid through substitution. A caller that reads
  the status numerically sees a status it has not seen before. R5 keeps it
  non-zero, which bounds the surface to callers that distinguish statuses.
- The single-wording guarantee is a convention enforced by one test rather than
  by the type system. A third call site that forgets the companion would get no
  message at all.

### Mitigations

- The behaviour change is called out in the release notes for the version that
  carries it, alongside the other unreleased fixes in this area.
- The companion returns an `Option`, so a call site that uses it cannot
  half-use it: there is one way to ask and one thing to do with the answer.
- The trait's existing doc comment already tells a future caller that a `false`
  does not distinguish the two cases, and it is updated to name the companion as
  the way to tell them apart.
