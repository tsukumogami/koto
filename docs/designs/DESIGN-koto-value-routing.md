---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-koto-value-routing.md
problem: |
  A `when` clause can test a template variable only with
  `vars.NAME: {is_set: bool}`, so a template that should branch on a
  variable's value hands the branch to directive prose. A `vars.NAME: <value>`
  entry already compiles in `skip_if` but never matches, because the runtime
  looks it up in evidence. Variables can also change after init in two ways
  (a `rebind: true` re-application and a capture), and the log records the new
  value of a rebind but not the old one.
decision: |
  A `vars.NAME` key in `when` or `skip_if` may hold a YAML string, matched by
  exact byte equality against the variable's current value in the one matcher
  function both evaluators share. The compiler checks a value condition
  against the variable's declaration and refuses four mistakes with
  `E-VAR-ROUTE-UNDECLARED`, `E-VAR-ROUTE-VALUE`, `E-VAR-ROUTE-OVERLAP` and
  `E-VAR-ROUTE-CAPTURE`. Routes may read declared variables, rebind ones
  included, but not captures. A value-routed `transitioned` event gains an
  optional `vars_matched` map, and `variables_rebound` gains an optional
  `previous` map with the old values. A new compat job proves koto v0.14.1
  reads the resulting logs.
rationale: |
  The matcher reuses the shape evidence routing already has (a key and a
  scalar), so the compiled form only changes where a template uses it and the
  overlap rule needs one new case. Allowing rebind variables is what makes
  the spans this feature exists to delete deletable: shirabe's mode, merge and
  pause switches are all rebind. Logging the old value on a rebind costs one
  optional field the code already computes. Captures are refused because they
  change inside a run, carry no declared constraint, and no span needs them.
  The canary turns out to need only directive substitution, which works today.
---

# DESIGN: koto value routing

## Status

Planned

## Context and Problem Statement

The requirements are in the PRD this design implements
(`docs/prds/PRD-koto-value-routing.md`), numbered R1 to R19; this section
states the technical problem those requirements meet.

A transition's `when` clause is a map from keys to expected values, AND-ed.
Evidence keys and `gates.*` keys compare by JSON equality. `vars.*` keys are
the exception: `condition_holds` in `src/engine/advance.rs` (for `when`) and
`conditions_satisfied` (for `skip_if`) recognize only
`{is_set: true|false}`, and any other value on a `vars.*` key falls through to
a dot-path lookup in the merged evidence map. That map never holds a `vars`
object, so the condition can never match. The compiler
(`src/template/types.rs`, the `vars_fields` validation) refuses such a value
in `when`, but runs no check on `vars.*` keys in `skip_if`, so there it
compiles and is silently dead. One shipped shirabe template carries one.

Variables reach the engine as a string map built from the log
(`bindings_from_events`): the `workflow_initialized` values, then each
`variables_rebound` and `variable_captured` in order, with the per-tick
overlay on top. Declared variables are fixed at init unless declared
`rebind: true`, which `koto init --attach-live` re-applies; captures are
written by `capture_stdout_as` whenever the producing state's action runs.
`variables_rebound` records only the new values, although the rebind plan in
`src/engine/variables.rs` already computes the previous ones.

A state whose transitions are all conditional resolves as soon as one
matches, with or without fresh evidence. So a state carrying only value
routes advances on entry, which is the behavior a template wants when koto,
not the agent, holds the branch.

## Decision Drivers

- Templates without a value condition must compile to byte-identical JSON and
  hash (PRD R14), and koto v0.14.1 must read the new logs (R15).
- Event changes must be additive and optional; a separate measurement effort
  reads them and needs the matched variable and value on the transition (R9)
  and unambiguous variable values for the run (R11).
- Immutable-after-init is the preferred default for a routed variable, but the
  deletable spans are mostly `rebind: true` settings (R18).
- One spelling must mean one thing in `when` and `skip_if` (R4).
- The decider never routes; `E-DECIDER-FLOOR` stays as it is (R13).
- Keep the change inside the matcher, the compiler's `vars.*` validation, the
  overlap rule, and two event payloads.

## Considered Options

### Decision 1: the matcher's written form

**Chosen: a bare YAML string on the `vars.NAME` key.**
`vars.MODE: auto`. It is the shape evidence keys already use, it serializes
into the compiled template exactly as written, and it is disjoint from the
`{is_set: bool}` object by type, so the engine tells the two apart without a
tag. Non-string scalars are refused rather than coerced, because `koto init`
stores `"true"` as a string and coercing YAML's `true` would make the compiled
form depend on YAML's boolean and number formatting.

**Alternative: a tagged object, `vars.MODE: {equals: auto}`.** Symmetric with
`{is_set: ...}` and leaves room for other operators. Rejected: the PRD scopes
out every operator but equality, the tag adds nothing an author needs, and it
reads differently from the evidence routes beside it in the same clause.

**Alternative: a list of alternatives, `vars.EXEC_MODE: [interactive,
default]`.** Covers "otherwise" branches in one line. Rejected: two
transitions to the same target express it, and a list is a second matcher
shape with its own overlap rules (list against list, list against value).
No named span needs it.

### Decision 2: which variables a route may read, and how changes are logged

**Chosen: declared variables, rebind ones included, with every rebind change
logged with its old value; captures refused.** A declared variable without
`rebind` is immutable after init. A rebind variable changes only on an
accepted attach; the attach already appends `variables_rebound` when a value
changes, and this design adds a `previous` map beside `variables`. A value
condition on a capture fails with `E-VAR-ROUTE-CAPTURE`.

**Alternative: immutable only (refuse routes on rebind variables).** The
preferred default absent a reason (PRD Decisions), and the simplest story for the measurement
consumer. Rejected on the evidence of the inventory below: the execution
mode (`EXEC_MODE`, `MODE`), the merge switch (`MERGE`) and the pause switch
(`PAUSE_BEFORE_FINALIZE`) in shirabe's templates are all `rebind: true` on
purpose, because a resume changes them. Refusing them would leave most of
the prose this feature exists to delete. A canary variable is declared
without `rebind`, so it stays immutable under the chosen option anyway.

**Alternative: allow captures too, logging `previous` on
`variable_captured`.** Rejected: a capture changes inside a run whenever its
producing state re-runs, it has no `values:` or `pattern:` to check a route's
value against, and the only span that tests one (`RUN_INTENT != none` in
`/scope`) needs negation, which is out of scope. `is_set` on a capture stays
legal; it answers "has the producing command run", which is still useful.

### Decision 3: the transition record

**Chosen: `vars_matched`, an optional object mapping each value-condition
variable on the taken edge to its matched value.** A `when` clause can hold
more than one `vars.*` key, so a map covers every case, and it mirrors the
existing `skip_if_matched`. It is written on the one `transitioned` event the
advance loop appends. It holds the value conditions of the taken edge's `when`
clause and, when the transition fired from `skip_if`, those of the `skip_if`
map too, so a `skip_if` advance whose value condition sits only in the
`skip_if` map still records it.

**Alternative: a single `matched: {var, value}` pair.** Reads well for the
common one-variable case. Rejected because a clause with two value conditions
would have to drop one or change shape.

**Alternative: a separate `route_matched` event.** Keeps `transitioned`
untouched. Rejected: a reader would have to join two events to learn why a
transition fired, and the extra event costs a log line on every routed step.

### Decision 4: `skip_if` value conditions and compile-time checks

**Chosen: give `skip_if`'s `vars.NAME: <value>` the `when` meaning, and run
the same four checks on it.** The shared matcher already serves both
evaluators, so one change fixes both. The one known template with such an
entry (shirabe's `/work-on` `entry` state) keeps compiling, because its
variable is declared, and starts matching.

What that changes for runs of it, precisely: the condition is
`skip_if: {vars.ISSUE_SOURCE: plan_outline, mode: plan_backed}`, and the
state's `when` routes send `mode: plan_backed` to `plan_context_injection`.
Plan-backed children get `ISSUE_SOURCE=plan_outline` as a variable from
shirabe's `plan-to-tasks.sh`, but no `mode` evidence, so on arrival the
`skip_if` can't hold and the state still asks the agent for `mode`, as today.
It can hold only after the agent submits `mode: plan_backed` on a run whose
`ISSUE_SOURCE` is `plan_outline`, and then it resolves to
`plan_context_injection`, the target evidence resolution picks for that
submission today. No run changes path or skips a state it didn't skip
before. The one difference is the record: that transition is logged with
`condition_type: "skip_if"`, `skip_if_matched` and `vars_matched` instead of
`condition_type: "auto"`. Activating it is therefore safe, and matches what
the entry reads as meaning. For shirabe's adoption: if the intent was to skip
the `mode` question for outline children, the `mode: plan_backed` key has to
come out of the `skip_if`, which is a change in shirabe, not here.

**Alternative: refuse value conditions in `skip_if`.** Rejected: it would
break that template's compile for no gain, and leave one spelling meaning
two things.

**Overlap rule.** The existing mutual-exclusivity rule treats two clauses as
exclusive when some shared key has unequal values. For a `vars.*` key that
is wrong in one case: a string and `{is_set: true}` are unequal JSON but can
both hold. The rule gets a `vars`-aware disjointness test (two strings are
disjoint when unequal; a string and `{is_set: false}` are disjoint; a string
and `{is_set: true}` are not), and a failing pair where either clause holds a
value condition reports `E-VAR-ROUTE-OVERLAP`. Pairs without value conditions
keep today's check and today's message.

**Alternative: a separate overlap pass for `vars.*` only.** Rejected: two
passes over the same pairs could disagree, and the existing rule already
handles clauses that differ on some other key.

### Decision 5: the canary

**Chosen: the canary needs a variable, not value routing.** koto substitutes
`{{VAR}}` into directives and details today
(`NextResponse::with_substituted_directive`). A single state whose directive
says "read `references/{{ARM}}.md`", with an `ARM` variable declared
`values: [with-rule, without-rule]` and no `rebind`, lets a run include or
omit an instruction with no duplicated states and no routing. The arm is
fixed at init, refused at init if mistyped, and recorded in
`workflow_initialized.variables`, which is where the measurement effort reads
it.

**Alternative: route to a duplicate state whose directive omits the
section.** Works with this feature and leaves `vars_matched` on the
transition. Rejected as the default because every canaried state doubles, and
every edge into and out of it doubles with it. It stays available for a
canary that needs different gates or transitions per arm, not only different
text.

### Decision 6: proving v0.14.1 compatibility

**Chosen: a new `value-routing-compat-v0-14-1` job running
`test/compat/value-routing-v0_14_1.sh`, patterned on the decider-checks job.**
The new build drives a fixture through a value route, a `skip_if` value route
and a rebind that changes a routed variable, stops at a state awaiting
evidence, and v0.14.1 then runs `koto status`, `koto next` and `koto context
get` on the session, stopped at a state with no value routes so both builds
report the same thing. v0.14.1 can't compile a template with a value condition,
so like the decider-checks job it is handed a session the new build created.
Templates without value conditions stay covered by the failure-reporting job,
which compiles every fixture under both builds, and by
`tests/compat_baseline_test.rs`.

**Alternative: extend an existing compat script.** Rejected: each existing
job's header states what it proves, and folding a new feature into one blurs
that and its self-test.

## Decision Outcome

A value condition is a string on a `vars.*` key. One function evaluates it
for both `when` and `skip_if`, so the two can't drift again. The compiler
checks it against the variable's declaration with the same constraint check
`koto init` uses, so a route can never name a value init would refuse, and
refuses overlapping routes with a rule that knows a value implies "set".
Routes may read any declared variable; the ones that can change (rebind)
leave the old and new value on the log, and the ones that can change without
a declaration (captures) are refused. Every value-routed transition names its
variables and values. The canary needs none of this and uses substitution.

### When a value is fixed, and what a route does if it changes

A declared variable without `rebind` is fixed by `workflow_initialized` and
never changes. A `rebind: true` variable can change only when a later
invocation attaches with `koto init --attach-live`; that appends a
`variables_rebound` event with `variables` (new values) and `previous` (old
values), and nothing else changes a declared variable. A value route is
evaluated whenever koto resolves the state's transitions, against the value
at that moment: the new value takes effect from the first tick after the
attach. A route already taken isn't revisited; a rewind that re-enters a
state resolves it against the current value. The run's history therefore
reads unambiguously from the log: the init value, each change with its old
and new value, and on every value-routed transition the value it matched.

### Matching rules

- Exact byte equality; case-sensitive; no trimming or normalization.
- The value must be a non-empty string drawn from `koto init`'s allowlist
  (ASCII letters and digits, space, `.`, `_`, `/`, `:`, `@`, `+`, `-`), and
  must satisfy the variable's `values:` or `pattern:` if it declares one.
- An empty variable counts as not set (as `is_set` has always said) and
  matches no value condition. Test emptiness with `{is_set: false}`.
- A value condition combines with the clause's other keys by AND.
- When no value route matches, the state behaves as today when no
  conditional transition matches (PRD R12).

## Solution Architecture

### Components

**Matcher (`src/engine/advance.rs`).** `condition_holds` gains one branch: a
`vars.NAME` key whose expected value is a JSON string holds when
`variables.get(NAME) == Some(expected)`. `conditions_satisfied` (the
`skip_if` evaluator) calls the same helper for `vars.*` keys instead of its
own copy, so both evaluators share one definition. A missing variable
matches nothing. A `vars.*` key never falls through to the evidence lookup
any more: a value that is neither `{is_set: ...}` nor a string (which the
compiler refuses) matches nothing, so submitted evidence shaped like
`{"vars": {...}}` can't satisfy a `vars.*` condition.

**Transition record (`src/engine/types.rs`, `src/engine/advance.rs`).**
`EventPayload::Transitioned` gains
`vars_matched: Option<BTreeMap<String, String>>`, serialized only when
`Some`. `take_transition` and the `skip_if` branch compute it from the taken
edge's `when` clause (the edge index `resolve_transition_edge` already
returns), plus the `skip_if` map on a `skip_if` advance: every `vars.*` key
with a string value, mapped to that value. Every other construction site
(about 45, most in tests) passes `None`, and the payload's deserializer
reads the field with a default.

**Rebind record (`src/engine/types.rs`, `src/engine/variables.rs`).**
`EventPayload::VariablesRebound` gains
`previous: BTreeMap<String, String>`, skipped when empty and defaulted when
absent on read. `apply_rebind` writes `plan.previous` into it.

**Compiler (`src/template/types.rs`).** The `vars.*` validation runs over
both `when` clauses and `skip_if` maps. For each `vars.NAME` key:

| Value | NAME | Result |
|---|---|---|
| `{is_set: bool}` | declared or capture | accepted, as today |
| `{is_set: bool}` | neither | today's undeclared-variable error, unchanged |
| string | declared | checked by `check_value` (allowlist, `values:`, `pattern:`) and non-empty, else `E-VAR-ROUTE-VALUE` |
| string | capture only | `E-VAR-ROUTE-CAPTURE` |
| string | neither | `E-VAR-ROUTE-UNDECLARED` |
| anything else | any | `E-VAR-ROUTE-VALUE` (in `when`); in `skip_if`, the same |

The mutual-exclusivity rule uses a `values_disjoint(key, a, b)` helper that
applies the `vars.*` rule above and plain inequality elsewhere. Its error is
prefixed with `E-VAR-ROUTE-OVERLAP` only when the failing pair shares a
`vars.*` key and one side of it is a value condition; a pair refused for
sharing no key, or overlapping only on evidence keys, keeps today's message.
`skip_if_matches_when` (the compile-time simulation behind
`E-SKIP-AMBIGUOUS`) already compares a `skip_if` string against a `when`
string by equality, which is the right answer for value conditions; it is
left as it is, and a test pins that a `skip_if` value selects the `when`
route with the same value.

**Documentation.** The koto-author template-format reference gains a "Routing
on a variable's value" subsection under the `when` condition, the four codes
go into `docs/reference/error-codes.md` beside the `skip_if` codes, the
session-feed contract documents `vars_matched` and `previous` (and its YAML
schema block lists both as optional), and the koto-user skill says a
value-routed state can advance on entry and where `vars_matched` appears.

### Data flow

```
koto init --var MODE=auto ──► workflow_initialized{variables}
koto init --attach-live   ──► variables_rebound{variables, previous}   (rebind only)
                                    │
koto next ─► bindings_from_events ─► overlay ─► condition_holds(vars.MODE: "auto")
                                                     │ match
                                                     ▼
                         transitioned{from, to, condition_type, vars_matched{MODE: auto}}
```

### Compiled form

A template with `vars.MODE: auto` compiles to `"when": {"vars.MODE": "auto"}`
in the state's transition. Nothing else in `CompiledTemplate` changes and
`format_version` stays where it is. A template with no value condition
serializes exactly as before.

## Implementation Approach

1. **Matcher and records.** Add the string branch to `condition_holds`, route
   `conditions_satisfied` through it, add `vars_matched` and `previous` to the
   two payloads and every constructor, and compute `vars_matched` at the two
   append sites. Unit tests in `advance.rs` and `variables.rs` cover exact
   matching over every allowlist character, empty values, AND with evidence,
   no-match behavior, `skip_if`, and both fields' presence and absence.
2. **Compiler checks.** Extend the `vars.*` validation to strings and to
   `skip_if`, add the four codes, and make the overlap rule `vars`-aware. This lands in the
   same pull request as step 1, since step 1 alone would bring dead `skip_if`
   entries to life before the checks exist. Tests in `types.rs`
   cover each code in `when` and `skip_if`, the compiling negatives, and a
   byte-identical compile of a template without value conditions.
3. **Integration tests.** A fixture template under `test/` with value routes
   drives `koto init`, `koto next` and an attach through the CLI, asserting
   the routes taken, the two event fields, `workflow_initialized.variables`
   holding passed, defaulted and empty values, init's refusal of out-of-set
   values, and the decider case (a decider-declared field beside value
   routes never moves the state to a target the variable doesn't allow).
4. **Docs and skills.** Template-format reference, error-code reference,
   session-feed contract and schema block, koto-author and koto-user skills.
   A test asserts every `E-VAR-ROUTE-*` code in the compiler appears in the
   error-code reference; the doc-names test covers paths and verbs.
5. **Compat.** `test/compat/value-routing-v0_14_1.sh`, its fixtures and
   `--self-test`, and the `value-routing-compat-v0-14-1` job in
   `.github/workflows/validate.yml`.
6. **Evals.** The change adds surface to koto-author and koto-user, so run
   `scripts/run-evals.sh` for both and report the graded result.

## Deletable-prose inventory

Found by searching shirabe's `skills/*/SKILL.md`, `skills/*/references/` and
`skills/*/koto-templates/` for each template variable's name, then reading
each hit for a branch on its value or presence. Dispositions are for shirabe
to adopt later; nothing in shirabe changes here.

| File | Section | Variable | Today | Disposition |
|---|---|---|---|---|
| `skills/execute/koto-templates/execute.md` | `pr_finalization`: `pause_decision` field and directive step 4 | `PAUSE_BEFORE_FINALIZE` (rebind, `values: ["true","false"]`) | agent reads the variable and relays it as `pause_decision` evidence | `when` value route: AND `vars.PAUSE_BEFORE_FINALIZE: "true"` / `"false"` into the state's existing completion condition; delete the `pause_decision` field and step 4's branch prose |
| `skills/execute/SKILL.md` | the pause-before-finalize passages | `PAUSE_BEFORE_FINALIZE` | prose describing the relay | shrinks to one line once the template routes |
| `skills/execute/koto-templates/execute.md` | merge gate (`test "{{MERGE}}" = true`, not overridable) | `MERGE` (rebind) | command gate on the value | `when` value route `vars.MERGE: "true"`; drops a shell process per tick |
| `skills/deliver/koto-templates/deliver.md` | `confirm` gate `test "{{MODE}}" = auto` | `MODE` (rebind, `values: [auto, interactive]`) | command gate on the value | `when` value route `vars.MODE: auto` |
| `skills/deliver/koto-templates/deliver.md` | execute leg: "when it reads `true`, append `--merge`" | `MERGE` | prose building an argument | stays: it builds a command line, not a route |
| `skills/deliver/koto-templates/deliver.md` | scope leg: `--coordinated` / `--no-coordinated` / neither | `COORDINATION` | prose building an argument | stays, same reason |
| `skills/scope/koto-templates/scope.md` | `resume_stale`: "Under `--auto` ... take Resume" | `EXEC_MODE` (rebind, `values: [auto, interactive, default]`) | agent reads the mode and picks the Resume evidence | `when` value route `vars.EXEC_MODE: auto` to the resume target; the directive keeps only the interactive prompt |
| `skills/scope/koto-templates/scope.md` | plan hop: "`--auto` when the execution mode is `auto`, `--interactive` otherwise" | `EXEC_MODE` | prose building an argument | stays |
| `skills/scope/koto-templates/scope.md` | setup directive, the intent gates (`test "{{RUN_INTENT}}" != none`), close-coordination-PR step | `RUN_INTENT` (a capture) | gates and prose on `!= none` | stays: a capture (refused, R11) and a negation (out of scope) |
| `skills/work-on/koto-templates/work-on.md` | `setup_plan_backed`: "When `SHARED_BRANCH` is set, submit `status: override`" | `SHARED_BRANCH` | `skip_if` needs the agent's `status: override` too | `when`/`skip_if` on `{is_set: true}` alone, which works today; delete the prose |
| `skills/work-on/koto-templates/work-on.md` | `pr_creation`: "If `SHARED_BRANCH` is set ... submit `pr_status: shared`" | `SHARED_BRANCH` | prose | `skip_if` on `{is_set: true}` to the `shared` target, which works today |
| `skills/work-on/koto-templates/work-on.md` | `entry` `skip_if`: `vars.ISSUE_SOURCE: plan_outline` with `mode: plan_backed` | `ISSUE_SOURCE` | dead condition | `skip_if`, now live (Decision 4); declare `values: [github, plan_outline]` to get typo checks |
| `skills/work-on/koto-templates/work-on.md` | `plan_context_injection`: "Behavior differs by ISSUE_SOURCE" and the `issue_source` relay field | `ISSUE_SOURCE` | agent relays the variable as evidence | `when` value route on `vars.ISSUE_SOURCE`; delete the relay field |
| `skills/work-on/SKILL.md`, `references/phases/phase-1-setup.md`, `phase-6-pr.md` | "If `SHARED_BRANCH` is set ... skip branch creation" | `SHARED_BRANCH` | prose | shrinks once the template routes |
| `skills/work-on/SKILL.md` | plan-backed mode chosen from `$ARGUMENTS` | none (resolved before init) | prose | stays: no koto variable holds it |
| `skills/execute/koto-templates/execute.md` | `spawn_and_await`: autonomy "when the run is authorized autonomous" | none | prose | stays: no variable |
| `SKILL.md` of explore, decision, prd, design, strategy, roadmap, charter, plan, review-plan | "check `$ARGUMENTS` for `--auto`, then the CLAUDE.md header" | none | prose | stays: these skills declare no mode variable |

## Security Considerations

A value condition compares two strings and runs nothing, so it adds no
command execution surface. Its values pass the same allowlist `koto init`
enforces, which already excludes every shell metacharacter, and a route
value never reaches a shell. The two new event fields carry variable names
and values that are already in the log (`workflow_initialized`,
`variables_rebound`), so they expose nothing new; a variable that must not
appear in a log must not be a koto variable today either. Refusing captures
closes the one case where a value produced by a command's stdout would steer
a route. Routing can't be driven by submitted evidence: `vars.*` keys read
only the variable map, never evidence, and a `--with-data` payload can't
write the map.

Two residual points for maintainers. A `rebind: true` variable can be changed
by anyone who can run `koto init --attach-live` on the session, the agent
included, so a route on one is a workflow aid, not an authorization
boundary; a check that must not be bypassed stays a non-overridable gate on
something the agent can't set. And a rebind variable omitted on attach
resets to its default, which can flip a route on resume; the
`variables_rebound` event with `previous` makes that visible after the fact.
`koto next` shows a state's value conditions in `expects.options[].when`, as
it shows `is_set` conditions today; the koto-user skill says those keys are
not something an agent submits.

## Consequences

**Positive.** Templates can hold branches koto enforces and logs, with typos
caught at compile time. The dead `skip_if` spelling is fixed rather than
papered over. The measurement effort gets the matched variable on every
routed transition and a complete change history for rebind variables. The
canary needs no engine change.

**Negative.** A shipped template's `skip_if` value condition changes from
dead to live. A template using value routes needs a koto with this change to
compile. A route on a rebind variable can pick a different path after a
resume than before it. Adding a field to `EventPayload::Transitioned`
touches every construction site.

A session a new koto drove through value routes, if handed to koto v0.14.1
while a value route is still pending, stops at `evidence_required` there:
v0.14.1 reads the log but can't evaluate the route. Reading is what stays
compatible; routing needs the new build.

**Mitigations.** The one live-by-change `skip_if` picks the same target
evidence resolution picks, and the design names it. Compile failure on an
older koto is loud, not silent. A rebind change and every route it affects
are on the log with old and new values. The field additions are mechanical
and covered by existing tests of each site.
