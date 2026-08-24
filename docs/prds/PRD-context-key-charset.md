---
schema: prd/v1
status: Accepted
problem: |
  A koto variable value may hold three characters a context key may not -- a
  space, a `:` and an `@` -- and one substitution step separates the two
  grammars. Nothing states the boundary where a template author reads, and the
  command-line existence check still answers a key that crosses it with the
  same silence it uses for a key that is simply absent, so an author meets the
  rule only by hitting it and gets nothing to grep for when they do.
goals: |
  The boundary between the two grammars is decided on the record and stated
  where authors read, and every surface a template can reach says which
  character it refused rather than reporting a bad key as an absent one. An
  author writing a variable reference into a context key learns the rule before
  they write, and an operator who hits it anyway can act on koto's answer.
absorbed:
  - docs/briefs/BRIEF-context-key-charset.md
source_issue: 227
---

# PRD: Reconciling the Value and Context-Key Character Sets

## Status

Accepted

Absorbed [BRIEF-context-key-charset](docs/briefs/BRIEF-context-key-charset.md); carried in Absorbed Brief.

## Absorbed Brief

koto validates two kinds of user-supplied string against grammars that were
never reconciled, and the gap between them is three characters wide. A variable
value may hold a space, a `:` and an `@`; a context key may hold none of the
three, because a key becomes a path component on disk. One substitution step
separates them, and since a context gate's `key` began resolving `{{KEY}}`
references the two meet routinely.

The problem that follows is not that a key gets refused. It is that nobody can
find out why. The rule lives in two Rust doc comments and in no guide, no skill
and no message an author sees before the fact, so the boundary is discovered by
crossing it. And the crossing is reported inconsistently: a gate now names the
character it refused, while `koto context exists` -- the verb a command gate or
a fallback action probes with -- answers an unusable key exactly as it answers a
missing one, with no output at all.

What a user should get instead is the rule stated where they are already
reading, before they write a key that cannot work, and the same plain answer
from every surface when they cross the line anyway: which character, in which
component, and that a value may carry it where a key may not. A maintainer
picking up either grammar should find the decision about whether the two
converge written down rather than inferred from what the code happens to do.

The framing draws on `docs/designs/current/DESIGN-template-variable-substitution.md`
for the substitution model the value grammar belongs to, and on
`docs/designs/current/DESIGN-local-session-storage.md` for the storage model the
key grammar is narrow for.

## Problem Statement

koto validates two kinds of user-supplied string against two grammars that were
never reconciled, and the relationship between them is written down nowhere an
author will find it.

A **variable value** may hold letters, digits, `.`, `_`, `/`, `-`, and also a
space, a `:` and an `@`. Those three were admitted deliberately, so a value can
be a calendar title or a filter expression like `from:user@example.com`
(koto#180).

A **context key** may hold less. It splits on `/`, each component must begin
with a letter or digit and continue in letters, digits, `.`, `_` and `-`, and
`.` and `..` components are refused outright. It is narrow because a key becomes
a path component on disk.

Since a context gate's `key` began substituting `{{KEY}}` references (koto#222),
those two grammars meet. A template can write `key: "{{TITLE}}-note"`, set
`TITLE` to `Weekly Planning` -- a value koto's own tests assert is legal -- and
produce `Weekly Planning-note`, which the store refuses.

Two things are wrong, and only the second is about the gate.

**Nobody wrote the rule down.** The asymmetry exists in two Rust doc comments,
read by people changing koto rather than by people using it. No guide, no
skill and no message an author sees before the fact says a value may carry
characters a key may not. Worse, the question of whether the two sets *should*
differ has never been answered: the code implies an answer, and a maintainer
picking up work near either grammar has to infer it.

**The gate is fixed and the surfaces beside it are not.** koto#229 taught the
gate to check its substituted key and report which character it refused. That
is one caller. `koto context exists` -- the CLI verb a command gate or a
fallback action uses to probe for a key -- still maps an unusable key onto the
same exit status as a missing one, with no output at all. `docs/guides/cli-usage.md`
already concedes the point in prose: the probe "cannot distinguish a key that
was never written from a store it could not read". A workflow that probes with a
substituted key gets a bare no, decides the key is absent, and proceeds on a
false premise.

Neither half is a crash. Both are a workflow that will not advance while koto
declines to say what is wrong with it.

## Goals

- The convergence question is answered deliberately and the answer is durable,
  so the next person to touch either grammar reads a decision instead of
  inferring one from behaviour.
- An author learns the boundary from the documentation they are already
  reading, before they write a key that will not work.
- Every surface a template can reach distinguishes "not a key at all" from "not
  here", and says which character made it so, in the same words.
- Nothing koto#222 established regresses: a gate's `key` and `pattern` still
  substitute, and the empty-key guard still fires.

## User Stories

- **As a template author**, I want koto to tell me which character in my
  substituted key it refused, so that I can fix the template instead of
  guessing why a gate will not open.
- **As a template author**, I want the rule about what a key may hold stated
  next to the rule about what a value may hold, so that I choose key names that
  survive substitution before I ever hit the failure.
- **As a workflow operator**, I want `koto context exists` to distinguish a key
  it cannot use from a key that is not there, so that a probe in a command gate
  or a fallback action does not silently proceed on a false answer.
- **As a koto maintainer**, I want the reasoning behind keeping the two
  grammars apart recorded, so that a future proposal to widen or narrow either
  one argues against something rather than starting from nothing.

## Requirements

### Functional

**R1.** The two character grammars stay as they are. `VALUE_PATTERN` is not
narrowed and the context-key grammar is not widened. This is the decision the
work is required to *make*, not an assumption it starts from; see D1 under
Decisions and Trade-offs for the alternatives and the reasoning, which is the
artifact the goal above asks for.

**R2.** `koto context exists` reports a key that fails `validate_context_key`
distinguishably from a key that is merely absent. The three outcomes -- present,
absent, and not a usable key -- are told apart by exit status, and the third
carries a diagnostic on the process's error stream in koto's usual JSON error
shape.

**R3.** The diagnostic R2 produces names the offending character and the
component it appears in, and says that a variable value may hold a space, `:`
or `@` where a context key may not.

**R4.** The diagnostic wording is composed in exactly one place and used by
both the context gate and the CLI verb, so the two surfaces cannot drift into
saying different things about the same key.

**R5.** The unusable-key outcome of R2 remains non-zero, so a caller that today
treats any non-zero status as "not present" keeps working unchanged.

**R6.** The relationship between the two grammars is stated in the
author-facing documentation: in the CLI reference where the `context` verbs are
described, and in the plugin skill that guides template authoring. Both state
which three characters differ and why the key grammar is the narrower one.

**R7.** The decision from R1 is recorded in a durable artifact in the
repository, with the alternatives that were live and what each would have cost.

### Non-functional

**R8.** No new runtime dependency, and no change to the on-disk layout of the
context store or the session log.

**R9.** `cargo fmt --check` and `cargo clippy -- -D warnings` stay clean, and
`cargo test --test doc_names` stays green with no new entry in
`tests/doc_names.allow`.

**R10.** Behaviour koto#222 established is preserved: a context gate's `key` and
`pattern` substitute, and a key that resolves to nothing still reports the
empty-key error rather than a bare mismatch.

## Acceptance Criteria

- [ ] `koto context exists <session> "Weekly Planning-note"` exits with a status
      that is neither the present status nor the absent status, and prints a
      JSON error naming the space and the component it appears in.
- [ ] `koto context exists <session> <a-valid-key-that-is-absent>` exits exactly
      as it does today, with no output.
- [ ] `koto context exists <session> <a-valid-key-that-is-present>` exits zero,
      with no output.
- [ ] The unusable-key status is non-zero, verified by a test that asserts the
      status is not zero as well as asserting which status it is.
- [ ] A test drives a value carrying a space through substitution into a
      `context-exists` gate's `key` and asserts the gate reports an error whose
      message names the space, not a bare `{"exists": false, "error": ""}`.
- [ ] The same test covers a value carrying a `:` and a value carrying an `@`.
- [ ] The gate's message and the CLI's message for the same key are produced by
      the same function, verified by a test that compares them.
- [ ] A test asserts a gate `key` and `pattern` still substitute and that an
      empty resolved key still produces the empty-key message.
- [ ] `docs/guides/cli-usage.md` states the two grammars and their difference in
      the `context` section, and its note about the probe's imprecision is
      corrected to match the new behaviour.
- [ ] The template-authoring skill under `plugins/koto-skills/skills/` states
      that a value may hold a space, `:` or `@` and a context key may not.
- [ ] A durable artifact in the repository records the decision to keep the
      grammars apart, names widening the key grammar and narrowing the value
      grammar as the alternatives, and says what each would have cost.
- [ ] The regression the work closes is demonstrated against `main`, not
      asserted: the new tests are shown failing on a checkout of `main` with the
      test files copied in.
- [ ] `cargo test`, `cargo fmt --check`, and `cargo clippy -- -D warnings` are
      clean, and `cargo test --test doc_names` passes with no new allow entry.

## Out of Scope

- **Widening what a variable value may hold.** The set was widened deliberately
  under koto#180 and touching it again is its own decision.
- **koto#224, koto#225 and koto#228.** Each is a field koto reads that sits
  outside the substitution or validation system: a `children-complete` gate's
  name filter, an undelivered capture name reaching a shell in a gate command,
  and a `default_action`'s fallback prose. This PRD settles the grammar two of
  those fields would eventually be validated against; it does not wire any
  field into substitution, so none of the three is closed by it. They were
  considered and deliberately left -- see D4.
- **Changing the `ContextStore` trait's signature.** `ctx_exists` keeps
  returning a bool; see D2.
- **Reworking substitution as a system.** Consolidating every field koto reads
  into one substitution and validation pass is a larger change with its own
  framing.
- **Cutting a release.** None of koto#223, koto#226, koto#229 or this work is in
  a released binary, and shipping them is a separate decision.

## Decisions and Trade-offs

The upstream brief deferred three questions here. D1, D2 and D3 answer them in
order; D4 records the scope call the brief left conditional.

### D1 -- The two grammars stay divergent

**Decided:** neither grammar changes. The work makes the boundary legible and
diagnosed instead of moving it.

**Alternatives.** *Widen the key grammar* to admit a space, `:` and `@`, so
every legal value is a legal key. *Narrow the value grammar* to the key's
character set, so the crossing cannot arise.

**Why divergence wins.** The two grammars are narrow and wide for reasons that
are both correct, and converging them means giving up one of those reasons.

A value is content. It ends up in a directive an agent reads, in a command's
argument, in a regular expression. Human text belongs there, which is why the
set was widened for titles and filter expressions in the first place. Narrowing
it back would break exactly the cases koto#180 was filed to support, and would
do so to fix a diagnostic problem -- a bad trade in any direction.

A key is an address. It has three consumers, and all three constrain it: it
becomes a path component in the session's context directory, it is a key in the
store's manifest, and it is passed as a shell argument in the `koto context
add` and `koto context get` commands templates routinely run. That last one is
decisive against widening. koto substitutes a non-empty value into a shell
command verbatim -- the shell-word form exists for the *empty* case and falls
through to the plain form otherwise -- so a key containing a space word-splits
the moment a template passes it to a command. Widening the key grammar would
legalize keys that cannot survive the path they are most often carried down,
which is a worse failure than the one being fixed: it fails silently and later.

The portability argument usually made for a narrow key grammar carries less
weight here than it looks: koto ships for Linux and macOS only, so `:` in a
filename is awkward on macOS rather than fatal. The shell argument is the one
that decides it.

**What it costs.** The asymmetry stays real, and an author with a title in a
variable still cannot use it directly as a key. This PRD's answer is that they
should be told so clearly and early rather than have koto accept a key it
cannot reliably handle. If the asymmetry proves to be a genuine expressive
limit rather than a diagnostic one, the shape that resolves it is a key-safe
rendering of a value at the substitution site, not a wider key grammar -- and
that is a separate feature with its own framing.

### D2 -- The check goes at the caller, not at the trait

**Decided:** `ContextStore::ctx_exists` keeps returning a bool that reports an
unusable key as absent. The distinction is drawn by the callers that need it.

**Alternative:** change the trait so existence returns a result, and let every
implementation surface the validation error.

**Why.** The gate already does the caller-side check, because koto#229 put it
there rather than in the store. Adding a second, different mechanism for the
same question would leave koto with two answers to "how do I tell an unusable
key from an absent one", and the trait change ripples to every implementation
and every call site to serve two callers. Following the shape that already
shipped keeps one answer, and R4's single message function is what stops the
two call sites drifting.

**What it costs.** A future third caller has to remember to check. R4's shared
helper makes doing so a one-line call, and the trait's own documentation
already warns that a `false` does not distinguish the two cases.

### D3 -- Provenance does not change the diagnostic

**Decided:** the check is on the key as the store receives it. A key that was
always invalid because an author typed it that way reports exactly as one that
became invalid through substitution.

**Alternative:** report only when a `{{KEY}}` reference was involved, leaving
hand-authored invalid keys behaving as they always have.

**Why.** By the time a key reaches the store, nothing distinguishes the two,
and adding that distinction would mean threading provenance through the call
for the sole purpose of staying quiet in one of the cases. A hand-authored key
that the store refuses is broken in precisely the way the diagnostic exists to
name.

**What it costs.** This is a behaviour change on templates that never involved
substitution: a probe that used to exit "absent" now exits "unusable". R5 keeps
it non-zero, so a caller that branches on success versus failure is unaffected;
a caller that distinguishes exit statuses numerically would see the new one.
That is the narrow surface, and the release notes are where it gets called out.

### D4 -- No sibling issue is closed here

**Decided:** koto#224, koto#225 and koto#228 stay open.

**Why.** The brief left this conditional on whether the grammar decision
genuinely governs them. It does not. Each of the three is about a field that is
never substituted or never validated at all; this work settles what the *key*
grammar is and how a refusal is reported. A `children-complete` gate's name
filter would still not be substituted after this lands, an undelivered capture
name would still reach a shell in a gate command, and a fallback's prose would
still be neither substituted nor validated. Closing any of them would claim a
fix that does not exist.

## Known Limitations

- The unusable-key status is observable only to a caller that reads exit
  statuses numerically or reads the error stream. A template whose command gate
  runs `koto context exists` inside a shell conditional sees the same
  pass/fail it sees today; what changes is that the reason is now printed
  rather than absent.
- The other verbs in the `context` group -- `get`, `add`, `remove` -- already
  surface a validation error, so they need nothing here. `exists` was the
  outlier precisely because its contract is a boolean.
