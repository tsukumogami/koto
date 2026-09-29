---
schema: prd/v1
status: Done
problem: |
  koto templates hold each variable's value for the whole session, but a
  transition's `when` clause can only ask whether `vars.NAME` is set. A
  branch on the value therefore lives in directive prose the agent must
  read and obey: nothing enforces it, nothing compiles it, and the session
  log never says which value picked the path.
goals: |
  A template author routes on a variable's value with a `when` clause,
  koto takes the route from the value it holds, the compiler refuses a
  mistyped variable or value, and each value-routed transition in the log
  names the variable and value that matched. Templates that don't route on
  a value compile and run exactly as before.
absorbed:
  - docs/briefs/BRIEF-koto-value-routing.md
---

# PRD: koto value routing

## Status

Done

The completeness, clarity and testability reviewers all passed it, the
first two on a second round. The downstream DESIGN owns the approach.

Absorbed [BRIEF-koto-value-routing](docs/briefs/BRIEF-koto-value-routing.md); carried in Absorbed Brief.

## Absorbed Brief

The feature exists because a template can hold a variable's value but
can't choose a path with it, so every branch on a value falls to directive
prose that the agent must obey and that nothing checks or logs. This
document's Problem Statement states that in full, including why a canary
and a measurement effort need the split taken and recorded by koto.

The outcome the brief asked for is an author who writes the branch as a
transition, finds a mistyped name or value at compile time, and leaves a
log that says which variable and value chose the path, with nothing
changing for templates that don't route on a value. Those are this
document's Goals, and the four people it imagined (a skill maintainer
deleting a branch, an author catching a typo, a maintainer explaining why
two runs diverged, an experiment owner splitting runs) are its User
Stories.

Its boundary held the feature to equality on template variables, the
compile-time checks, the transition record, a stated rule for variables
that change after init, documentation, and an inventory of deletable
shirabe prose. It left out shirabe changes, richer expressions, decider
routing, canary sampling and assignment, and conditional directive text.
Those are this document's Requirements and Out of Scope. Its two open
questions, whether the canary needs value routing and whether a routed
variable must be immutable, are closed in Decisions and Trade-offs.

## Problem Statement

A koto template declares variables that `koto init --var` fills in, such as
an execution mode, a merge switch, or the plan document a run follows. koto
keeps those values for the whole session and substitutes them into
directives and gate commands. It can't route on them. A transition's `when`
clause compares submitted evidence and gate output by value, but a
`vars.NAME` key accepts only `{is_set: true}` or `{is_set: false}`; any other
value is a compile error. A `skip_if` is looser in the wrong way: a
`vars.NAME: <value>` entry there compiles and then never matches, because
the runtime looks the key up in evidence rather than in the variables.

Template authors who need a branch on a value put it in directive prose:
"if the mode is auto, submit the recommended default; otherwise ask the
author." The agent must read the value from its own directive and act on it
on every run. The prose isn't enforced, a mistyped value in it is never
caught, and the session log records only the state the run moved to, not
that a variable's value sent it there. Workflow skills built on koto carry
several such branches today.

Two consumers need more than prose. A later canary wants to split runs on a
variable set at init and have koto take the split. A separate measurement
effort attributes each run to the path its variable chose, and for that it
needs the matched variable and value on the transition event and a
variable whose value can't silently change during the run.

## Goals

- A template author writes a branch on a variable's value as `when` routes,
  and the directive no longer has to explain the branch to the agent.
- A misspelled variable or a value the variable can't hold is caught at
  compile time, with an error that names both.
- A reader of a finished session's log can tell, from the transition event
  alone, which variable and value routed the run.
- A routed variable's value is either fixed for the run or every change to
  it is on the log with its old and new value (R11 picks per kind of
  variable).
- Nothing changes for a template that doesn't route on a value.

## User Stories

- As a workflow-skill maintainer, I want to replace "if the mode is auto, do
  X; otherwise Y" directive prose with two `when` routes on `vars.MODE`, so
  that koto takes the branch and the agent never reads it.
- As a template author, I want `koto template compile` to refuse
  `vars.MODE: atuo` with a coded error naming the transition, the variable,
  and the values `MODE` allows, so that I fix the typo before any session
  runs.
- As a workflow maintainer investigating why two sessions of one template
  diverged, I want the transition where their paths split to carry the
  variable name and value each run matched, so that I find the cause in the
  log without reading the template.
- As a maintainer running a canary, I want to start each run with an arm
  variable set at `koto init` and have koto take the split, so that I can
  group finished runs by the arm each started with.

## Requirements

### Functional

**R1. Value matcher.** A `when` clause may give a `vars.NAME` key a YAML
string, and the condition holds when the variable's current value equals
that string. Comparison is exact byte equality: case-sensitive, no trimming,
no normalization. The string must be written as a string: a YAML boolean,
number, or null on a `vars.*` key is refused at compile time (an author
routing on a `"true"`/`"false"` variable quotes the value). Only a single
value is accepted; a list of alternatives is not, and two transitions to
the same target cover two values. A value condition combines with the
clause's other keys by AND, exactly as evidence and gate keys do today.

**R2. Value domain.** A value condition's string must be one `koto init
--var` could store: ASCII letters and digits, space, and `.`, `_`, `/`, `:`,
`@`, `+`, `-` (the same allowlist init enforces). The empty string is not a
legal value condition; an author tests emptiness with `{is_set: false}`. A
variable whose value is empty counts as not set, as it does today, and
matches no value condition.

**R3. `is_set` unchanged.** `vars.NAME: {is_set: true|false}` keeps its
syntax, meaning, and compiled form in both `when` and `skip_if`.

**R4. `skip_if` means the same thing.** A `vars.NAME: <string>` entry in
`skip_if` holds exactly when the same entry in a `when` clause would, and
passes the same compile-time checks (R5 to R7, R11). Today such an entry
compiles and never matches; after this change it matches by value. An
existing template carrying one either keeps compiling and starts matching,
or fails with the R5 to R7 or R11 code that names the problem.

**R5. Undeclared variable.** The compiler refuses a value condition on a
name the template's `variables:` block doesn't declare, with error code
`E-VAR-ROUTE-UNDECLARED` naming the state, the transition (or `skip_if`),
and the variable.

**R6. Value the variable can't hold.** The compiler refuses a value
condition whose value is not a string, is empty, falls outside the R2
allowlist, is not among the variable's `values:`, or doesn't fully match
its `pattern:`, with error code `E-VAR-ROUTE-VALUE` naming the state, the
transition, the variable, the value, and the constraint it fails. A route
refused this way is one that could never fire.

**R7. Routes that can match at once.** Two conditional transitions out of
one state that a single variable value could both satisfy are refused with
error code `E-VAR-ROUTE-OVERLAP`, naming both targets and the variable. Two
value conditions on the same variable overlap when their values are equal.
A value condition and `{is_set: true}` on the same variable always overlap;
a value condition and `{is_set: false}` never do. The pair is not refused
when another key the two clauses share has different values (the existing
mutual-exclusivity rule, which is unchanged for templates without value
conditions). Every refusal in R5 to R7 and R11 is a compile error, not a
warning.

**R8. Init refuses out-of-set values.** `koto init` refuses a value outside
a variable's `values:` or failing its `pattern:`, with exit 2 and no session
created. This holds today; the requirement is that it keeps holding for
every variable a value route reads, so an out-of-set value never reaches a
route at runtime. A variable with no `values:` or `pattern:` accepts any
allowlisted value, and a route on it simply may not match.

**R9. Transition record.** A `transitioned` event whose taken edge has one
or more value conditions carries a new optional field, `vars_matched`: an
object mapping each such variable name to the value it matched. `is_set`
conditions aren't included. The field appears whether the transition fired
from evidence resolution or from `skip_if`, and is absent from every other
transition.

**R10. Init record.** `workflow_initialized.variables` keeps recording every
declared variable's value at init, defaulted and empty ones included.

**R11. Which variables a route may read, and when they change.** A value
condition may name a declared variable, whether or not it is `rebind: true`,
and is evaluated against the variable's value at the moment koto resolves
the state's transitions. A declared variable without `rebind` can't change
after `koto init`. A `rebind: true` variable changes only when an attach
re-applies it; every re-application that changes a value appends a
`variables_rebound` event that carries both the new value and, in a new
optional `previous` field, the old one. A re-application that changes
nothing appends nothing, as today. A value condition on a capture
(`capture_stdout_as`), whose value a later state can overwrite, is refused
with error code `E-VAR-ROUTE-CAPTURE`.

**R12. No match.** When a variable's value matches none of a state's value
routes, the state behaves as it does today when no conditional transition
matches: another matching conditional transition fires, else the
unconditional fallback under the existing rules, else koto asks for
evidence and the agent sees the state's directive.

**R13. Decider stays out of routing.** A decider can't set a variable, value
conditions don't change when or how a decider answer applies, and the
decider floor check (`E-DECIDER-FLOOR`) and its tests are unchanged.

### Non-functional

**R14. Compiled-template compatibility.** A template with no value condition
compiles to byte-identical JSON and the same template hash as before. A
template with value conditions compiles to JSON whose only difference from
today's shape is a string where `{is_set: ...}` could stand; no format
version changes.

**R15. Log compatibility.** koto v0.14.1 reads a session log the new build
wrote for a value-routed template, including a `transitioned` event with
`vars_matched` and a `variables_rebound` event with `previous`: `koto
status` and `koto next` exit 0 and report the same state name the new build
reports, and `koto context get` exits 0 and returns the value the new build
returns. A CI job runs this check, following the existing per-feature
compat jobs.

**R16. Documentation.** The template-format reference documents the
matcher, R2's domain, R11's variable rule, and R12's no-match behavior; the
error-code reference lists every new code; the session-feed contract
documents `vars_matched` and `previous` as optional additive fields; the
koto-author skill shows a value route and the new codes; the koto-user
skill tells an agent that a value-routed state can advance on entry
without asking for evidence, and where `vars_matched` appears.

**R17. Scripted checks.** Every acceptance criterion below except the two
about the design document's content ships with a Rust test or a compat
script, and CI runs it.

**R18. Deletable-prose inventory.** The design lists every place in
shirabe's `skills/*/SKILL.md`, `skills/*/references/`, and
`skills/*/koto-templates/` where prose or a gate branches on the value or
presence of a declared koto variable, found by searching those paths for
the template variables' names, and for each says whether it becomes a
`when` value route, a `skip_if`, or stays as it is, and why. Nothing in
shirabe changes here.

**R19. Canary analysis.** The design compares two ways a run can include or
omit an instruction: a value route to a duplicate state whose directive
omits it, and one state whose directive names a reference file through
`{{VAR}}`. It checks whether koto already substitutes variables into
directive text, and states which option the canary needs and whether that
option needs value routing.

## Acceptance Criteria

Matching and routing:

- [ ] A state with `vars.MODE: auto` and `vars.MODE: interactive` routes, run
  with `--var MODE=auto`, advances to the `auto` target on entry with no
  evidence submitted; with `--var MODE=interactive` it advances to the
  other.
- [ ] A route for `Auto` doesn't fire for `auto`; a route for `a` doesn't
  fire for `ab` or ` a`; for each of space, `.`, `_`, `/`, `:`, `@`, `+`,
  `-` and a digit, a route whose value contains it fires for exactly that
  value.
- [ ] A clause `{vars.MODE: auto, verdict: approve}` fires only when both
  hold.
- [ ] With `MODE` empty, neither a `vars.MODE: auto` route nor any value
  route fires, and a `{is_set: false}` route does.
- [ ] A state whose value routes don't match the variable's value and that
  has no other transition doesn't advance: `koto next` answers
  `evidence_required` with the state's directive.
- [ ] A `skip_if: {vars.MODE: auto}` state advances when `MODE` is `auto`
  and doesn't when it's `interactive`.
- [ ] Existing `is_set` tests in `when` and `skip_if` pass unchanged.

Compile-time checks:

- [ ] `vars.NOPE: x` on an undeclared `NOPE` fails with
  `E-VAR-ROUTE-UNDECLARED`, in `when` and in `skip_if`.
- [ ] Each of `vars.MODE: atuo` (outside `values:`), a value failing
  `pattern:`, `vars.MODE: ""`, `vars.MERGE: true` (unquoted boolean),
  `vars.MODE: 3`, and a value holding `$` fails with `E-VAR-ROUTE-VALUE`,
  and the message names the variable, value, and constraint.
- [ ] Two routes with `vars.MODE: auto`, and a `vars.MODE: auto` route beside
  a `vars.MODE: {is_set: true}` route, each fail with `E-VAR-ROUTE-OVERLAP`
  naming both targets; `auto` beside `interactive`, and a value route beside
  `{is_set: false}`, compile; two `vars.MODE: auto` routes that also carry
  `verdict: approve` and `verdict: reject` compile.
- [ ] A value condition on a `capture_stdout_as` name fails with
  `E-VAR-ROUTE-CAPTURE`; an `{is_set: ...}` condition on it still compiles.
- [ ] Each of the `E-VAR-ROUTE-UNDECLARED`, `E-VAR-ROUTE-VALUE` and
  `E-VAR-ROUTE-CAPTURE` cases above also fails when the condition is
  written in `skip_if` instead of `when`.
- [ ] `koto init` refuses a value outside `values:` and a value failing
  `pattern:`, each with exit 2 and no session directory created.

Records and compatibility:

- [ ] A value-routed transition's event carries `vars_matched` mapping each
  value-condition variable to its value, for both an evidence-resolved and
  a `skip_if` transition, and a clause with two value conditions records
  both; a transition with only `is_set`, evidence, gate, or no conditions
  carries no `vars_matched`.
- [ ] `workflow_initialized.variables` holds a passed, a defaulted, and an
  empty variable for a template with value routes.
- [ ] An attach that changes a `rebind: true` variable a route reads appends
  a `variables_rebound` event with the new value and `previous` holding the
  old one, and the next `koto next` routes on the new value; an attach that
  re-applies the same value appends no event.
- [ ] A state with a `vars.MODE: auto` route and an unconditional fallback,
  entered by evidence submission with `MODE` set to `interactive`, takes the
  fallback.
- [ ] A state with a decider-declared field and value routes: the decider
  answer never causes a transition that the variable's value doesn't allow,
  and the existing `E-DECIDER-FLOOR` tests pass unchanged.
- [ ] `tests/compat_baseline_test.rs` hashes are unchanged, and a template
  without value conditions compiles to byte-identical JSON before and after.
- [ ] A compat script drives a value-routed session with the new build,
  including a rebind that changes a routed variable, then runs koto
  v0.14.1's `koto status`, `koto next` and `koto context get` on it: each
  exits 0, the state names equal the new build's, and the context value
  equals the new build's; its `--self-test` shows each check fails when the
  log is mutated. A CI job runs both.
- [ ] Every `E-VAR-ROUTE-*` code the compiler can emit appears in
  `docs/reference/error-codes.md` (checked by a test), and `vars_matched`
  and `previous` appear in `docs/reference/session-feed.md`.
- [ ] The koto-author template-format reference contains a compiling value
  route example and names every `E-VAR-ROUTE-*` code; the koto-user skill
  mentions `vars_matched` and that a value-routed state can advance on
  entry (checked by the existing doc tests or a grep in CI).
- [ ] A template with value conditions compiles with the same
  `format_version` as today, and its compiled JSON differs from the same
  template with those conditions replaced by `{is_set: true}` only in those
  condition values.

Design content (checked by review, not script):

- [ ] The design lists the shirabe spans found by R18's search, each with a
  file, a section, and a disposition.
- [ ] The design states which canary option it recommends and whether that
  option needs value routing.

## Out of Scope

- Any change to shirabe. Its templates adopt value routing, and raise their
  koto floor, in their own release.
- Expressions beyond equality: lists of alternative values, ranges, regular
  expressions, arithmetic, negation (`!=`), and boolean combinations beyond
  the existing AND of a `when` clause's keys.
- Value conditions on captures (refused, R11) and on gate output or
  evidence, which already match by value.
- Routing on a decider's answer, or any change to `E-DECIDER-FLOOR`.
- Canary sampling, withholding instructions, how a canary assigns its
  variable, and a rule registry.
- Conditional text inside a directive.
- A compile-time check that a state's value routes cover every one of the
  variable's declared values; R12 defines what happens when none matches.

## Decisions and Trade-offs

**The canary and value routing (closes the BRIEF's first open question).**
koto already substitutes `{{VAR}}` into directives and details, so a
directive that names `references/{{ARM}}.md` swaps the instruction a run
reads with a variable and no routing. R19 makes the design write the
comparison out. Value routing is justified here by the deletable spans
(R18); the canary uses it only if it needs different states or edges per
arm. Taking the canary as the reason for value routing without checking was
rejected, because it would scope the matcher to a need that may not exist.

**Immutable or logged (closes the BRIEF's second open question).** The
preferred default was immutable after init, since a canary needs only an
init-time value. It doesn't fit the spans this feature exists to delete:
the execution mode, the merge switch, and the pause-before-finalize switch
in shirabe's templates are all `rebind: true`, re-applied on resume on
purpose, so refusing routes on rebind variables would leave that prose in
place. R11 therefore allows routes on rebind variables and requires every
change to be logged with its old and new value, while refusing routes on
captures, which change inside a run with no declared constraint and which
no named span needs. A canary variable declared without `rebind` stays
immutable, which is what the measurement effort needs.

**Scalar strings only.** A list of alternatives was considered for
"interactive or default" branches and rejected: two transitions to the same
target express it, and a list would add a second matcher shape and its own
overlap rules. Unquoted YAML booleans are refused rather than coerced,
because `koto init` stores `"true"` as a string and coercion would make the
compiled form depend on YAML's number and boolean formatting.

**Exact equality only.** Case-insensitive or trimmed matching was rejected:
`koto init` stores the value as given, `values:` compares exactly, and a
second comparison rule would let a route and its variable's constraint
disagree about the same value.

**Dead `skip_if` value conditions come alive.** A `vars.NAME: <value>` in
`skip_if` compiles today and never matches. R4 gives it the `when` meaning
rather than refusing it, so the one spelling means one thing everywhere. The
cost is that an existing template carrying such an entry changes behavior;
the design names the ones it finds.

**Error codes, not warnings.** A route that can never fire or can't be told
apart from another is a template bug with no legitimate use, so R5 to R7 and
R11 refuse compilation instead of warning.

## Known Limitations

- A template that adopts value routing needs a koto that understands it; an
  older koto refuses to compile it. Reading the log is what stays
  compatible, not compiling the template.
- A route on a variable with no `values:` or `pattern:` can't be checked for
  a mistyped value, only for a mistyped name.
- A value route on a `rebind: true` variable can take a different path after
  a resume than before it. The log records both the change and the route,
  but a reader attributing a run to one value has to account for it.
