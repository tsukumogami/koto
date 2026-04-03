---
status: Implemented
problem: |
  Gates in koto are boolean pass/fail checks completely decoupled from
  transition routing. Gate results don't produce structured data, don't feed
  into transition when clauses, and don't affect which state the workflow
  advances to. Template authors must add accepts blocks with override enum
  values as a workaround to let agents bypass failed gates. This model breaks
  down as gates become richer than simple command-line checks.
goals: |
  Unify gates and transitions into a single contract where gates produce
  structured data that feeds directly into transition routing, overrides
  specify default gate values with mandatory rationale, and the compiler
  validates that the contract is complete.
source_issue: 108
---

# PRD: Gate-transition contract

## Status

Implemented

## Problem statement

Gates and transitions in koto are two independent systems that should be one.

Gates are boolean pass/fail checks. They run commands, check file existence, or
match regex patterns. They produce a single bit of information: did the check
pass or not. That result blocks or unblocks the state but never feeds into
transition routing. The engine doesn't know *what* the gate found, only
*whether* it passed.

Transitions route on evidence -- structured data submitted by agents via
`--with-data`. The transition resolver matches evidence fields against `when`
conditions to pick a target state. But this evidence comes exclusively from
agents. Gates can't contribute data to routing even when they have it (a
context-matches gate already reads file content but throws away everything
except the pass/fail signal).

This decoupling creates three problems:

1. **Template authors must manually couple gates to transitions.** On
   deterministic gate states, authors add `accepts` blocks with `override` enum
   values and matching conditional transitions. This is boilerplate that exists
   only because the engine can't route on gate results. Every gated state that
   should be overridable requires this workaround. The compiler doesn't validate
   that the workaround is complete.

2. **Overrides have no audit trail.** When an agent submits
   `{"status": "override"}` to bypass a failed gate, the rationale is lost. The
   event log captures the evidence submission but not why the agent overrode.
   `koto decisions record` exists as a separate call but nothing connects it to
   the override. Session visualization -- showing a human reviewer every gate
   override with reasoning -- is impossible.

3. **Gates can't express richer outcomes.** A CI gate might want to report
   coverage percentage. A content gate might want to return matched fields. A
   validation gate might want to return a severity level. None of this is
   possible with pass/fail. As koto moves toward gates that can independently
   verify evidence (polling CI, validating schemas, calling external services),
   the boolean model becomes a bottleneck.

## Goals

- Gates produce structured data with per-gate output schemas, using the same
  field types as evidence (string, number, boolean, enum)
- Gate output feeds into transition `when` clauses alongside agent evidence,
  using the same transition resolver
- Every gate declares a default override value in its schema, specifying what
  the gate result "should have been" when the agent overrides
- The compiler validates that gate schemas, override defaults, and transition
  `when` clauses form a complete, unambiguous contract
- Overrides produce auditable `GateOverrideRecorded` events with mandatory
  rationale, queryable across the full session
- Template authors no longer need `accepts` block workarounds on deterministic
  gate states

## User stories

**As a template author**, I want gates to declare what data they produce and
what transitions should match, so I don't have to manually create `accepts`
blocks with `override` enum values as a workaround for routing on gate results.

**As a template author**, I want the compiler to tell me if my gate schemas,
override defaults, and transition conditions don't fit together, so I catch
dead-end states before runtime.

**As an agent (workflow skill)**, I want to override a gate's output via
`koto overrides record` and have the engine substitute the override values
for transition routing on the next `koto next`, so I can unblock the
workflow when the gate's actual output doesn't match my intent.

**As a human reviewer**, I want to query all gate overrides in a session and
see the rationale, the gate's actual output, and what values were substituted,
so I can audit agent behavior.

**As a template author**, I want gates that return richer data (coverage
percentage, matched content, severity level) so transitions can route on
meaningful outcomes, not just pass/fail.

## Interaction examples

### Example 1: Gate with structured output and automated routing

```yaml
states:
  verify:
    gates:
      ci_check:
        type: command                    # produces {exit_code: number}
        command: "run-ci"
    transitions:
      - target: deploy
        when:
          gates.ci_check.exit_code: 0    # route on the gate type's documented output
      - target: fix
        when:
          gates.ci_check.exit_code: 1
```

**CI passes:** gate produces `{exit_code: 0}`, transition resolver matches
`gates.ci_check.exit_code: 0`, advances to `deploy`. No agent interaction
needed.

**CI fails:** gate produces `{exit_code: 1}`, transition resolver matches
`gates.ci_check.exit_code: 1`, advances to `fix`. No agent interaction
needed -- the gate result drives routing automatically.

**CI fails + agent overrides:**

```bash
$ koto next my-workflow
{"action": "gate_blocked", "state": "verify",
 "blocking_conditions": [{"gate": "ci_check", "output": {"exit_code": 1, "error": ""}}]}

# Override the gate
$ koto overrides record my-workflow --gate ci_check \
    --rationale "Flaky test, unrelated to this change"

# Advance -- engine reads the override event, substitutes {exit_code: 0}
$ koto next my-workflow
# Transition resolver matches gates.ci_check.exit_code: 0 -> deploy
{"action": "done", "state": "deploy", "advanced": true}
```

The override default tells the engine: "when overridden, treat this gate as if
it returned `{exit_code: 0}`." The transition resolver routes normally.

### Example 2: Gate data and agent evidence coexisting

```yaml
states:
  review:
    gates:
      lint:
        type: command                    # produces {exit_code: number}
        command: "run-lint"
    accepts:
      decision:
        type: enum
        values: [approve, request_changes]
        required: true
    transitions:
      - target: merge
        when:
          gates.lint.exit_code: 0
          decision: approve
      - target: revise
        when:
          decision: request_changes
```

Gate output (`gates.lint.exit_code`) and agent evidence (`decision`) coexist in the
same transition resolver. The `when` clause can reference both. Lint runs
automatically; the agent provides its review decision.

### Example 3: Simple pass/fail gate (backward compatible)

```yaml
states:
  check:
    gates:
      file_exists:
        type: context-exists             # produces {exists: boolean}
        key: "config.yaml"
    transitions:
      - target: proceed
        when:
          gates.file_exists.exists: true
      - target: setup
        when:
          gates.file_exists.exists: false
```

Even simple gates get a schema. The boolean model is a special case of the
structured model, not a separate concept.

### Example 4: Override with explicit data (richer gate)

A future `jira` gate checks issue assignment. The gate returns structured
data, and transitions route on it. The agent overrides with explicit data
when the gate's actual output doesn't match their intent.

```yaml
states:
  triage:
    gates:
      ticket:
        type: jira                        # produces {assigned: boolean, status: string, ...}
        key: "{JIRA_TICKET}"
    transitions:
      - target: work
        when:
          gates.ticket.assigned: true
      - target: assign
        when:
          gates.ticket.assigned: false
```

```bash
# Gate returns assigned: false (ticket isn't assigned in Jira)
$ koto next my-workflow
{"action": "gate_blocked", "state": "triage",
 "blocking_conditions": [{"gate": "ticket", "output": {"assigned": false, "status": "open", "error": ""}}]}

# Agent assigned the ticket out-of-band, overrides with explicit data
$ koto overrides record my-workflow --gate ticket \
    --rationale "Assigned ticket to myself via Jira UI" \
    --with-data '{"assigned": true, "status": "in_progress"}'

$ koto next my-workflow
# Transition resolver matches gates.ticket.assigned: true -> work
{"action": "done", "state": "work", "advanced": true}
```

Without `--with-data`, the override would substitute `override_default`
(which for jira gates defaults to `{assigned: true}`). With `--with-data`,
the agent provides specific values -- here including the updated status.

### Example 5: Selective override with per-gate rationale

A `validate` state has two command gates. Override events are sticky within
an epoch -- they persist until the state transitions.

```bash
# Two command gates fail
$ koto next my-workflow
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "schema_check", "output": {"exit_code": 1, "error": ""}},
   {"gate": "size_check", "output": {"exit_code": 2, "error": ""}}
 ]}

# Override schema_check with its own rationale
$ koto overrides record my-workflow --gate schema_check \
    --rationale "Schema errors are in deprecated fields"

# Advance -- schema_check is overridden, but size_check still fails
$ koto next my-workflow
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "size_check", "output": {"exit_code": 2, "error": ""}}
 ]}

# Override size_check with its own rationale
$ koto overrides record my-workflow --gate size_check \
    --rationale "Large file is a test fixture"

# Advance -- both overrides in effect, state advances
$ koto next my-workflow
{"action": "done", "state": "process", "advanced": true}
```

Each gate gets its own rationale. Overrides accumulate across calls within
the same epoch.

### Example 6: Multi-agent override

Sub-agents push overrides independently. Orchestrator advances when ready.

```bash
# Sub-agent A overrides schema_check (no advancement)
$ koto overrides record my-workflow --gate schema_check \
    --rationale "Schema errors are in deprecated fields"

# Sub-agent B overrides size_check (no advancement)
$ koto overrides record my-workflow --gate size_check \
    --rationale "Large file is a test fixture"

# Orchestrator advances -- both overrides are read during gate evaluation
$ koto next my-workflow
{"action": "done", "state": "process", "advanced": true}
```

### Example 7: Override audit trail

```bash
$ koto overrides list my-workflow
{"overrides": [
  {"state": "validate",
   "gates_overridden": [
     {"gate": "schema_check",
      "actual_output": {"exit_code": 1, "error": ""},
      "override_applied": {"exit_code": 0, "error": ""}}
   ],
   "rationale": "Schema errors are in deprecated fields",
   "seq": 12, "timestamp": "2026-03-31T10:15:00Z"},
  {"state": "validate",
   "gates_overridden": [
     {"gate": "size_check",
      "actual_output": {"exit_code": 2, "error": ""},
      "override_applied": {"exit_code": 0, "error": ""}}
   ],
   "rationale": "Large file is a test fixture",
   "seq": 14, "timestamp": "2026-03-31T10:15:30Z"}
]}
```

Each override event captures what the gate actually returned, what default was
applied, and why. Self-contained for visualization.

## Requirements

### Functional

**R1: Gate types are reusable building blocks with documented schemas.** Each
gate type defines a public output schema and a pass condition. Gates always
run and always return data -- there's no separate "failure" state. The pass
condition determines whether the state auto-advances (all gates pass) or
stops for agent input (any gate doesn't pass). The schema is the gate type's
contract -- it tells template authors what fields are available for `when`
clauses and what "passing" means for override defaults.

Initial gate types and their schemas:

| Gate type | Output schema | Pass condition |
|-----------|--------------|----------------|
| `command` | `{exit_code: number, error: string}` | `exit_code == 0` |
| `context-exists` | `{exists: boolean, error: string}` | `exists == true` |
| `context-matches` | `{matches: boolean, error: string}` | `matches == true` |

The `error` field is empty on success. On failure, it contains the failure
reason. On timeout, the gate produces `{exit_code: -1, error: "timed_out"}`
(for command) or `{exists: false, error: "timed_out"}` (for context types).
On spawn/evaluation errors, `error` contains the error message. This means
the output shape is always consistent -- `when` clauses can route on any
field regardless of whether the gate succeeded, failed, timed out, or
errored.

Future gate types extend this registry with richer schemas:

| Gate type (future) | Output schema | Pass condition |
|-----------|--------------|----------------|
| `json-command` | `{exit_code: number, data: object, error: string}` | `exit_code == 0` |
| `http` | `{status_code: number, body: object, error: string}` | `status_code >= 200 && < 300` |
| `jira` | `{assigned: boolean, status: string, assignee: string, error: string}` | `assigned == true` |

Each gate type owns both the schema and the parsing logic that converts raw
execution results into structured output. Template authors pick a gate type,
reference its documented fields in `when` clauses, and optionally declare
`override_default` values (validated against the schema by the compiler).

**R2: Gate evaluation produces structured data.** When a gate evaluates, it
produces a structured result matching its gate type's schema, not just
pass/fail. The engine contains the parsing logic for each gate type:
- **command**: runs the command, captures exit code -> `{exit_code: N}`
- **context-exists**: checks key existence -> `{exists: true/false}`
- **context-matches**: runs regex match -> `{matches: true/false}`
Future gate types (e.g., `json-command` that parses stdout as JSON, `http`
that checks an endpoint) extend the system by registering new types with
their own schemas and parsing logic. Richer output comes through new gate
types, not through extending existing types' schemas.

**R3: Gate output feeds into transition routing.** Gate output is available to
transition `when` clauses via namespaced fields. The namespace is a nested
JSON map: `{"gates": {"ci_check": {"exit_code": 0, "error": ""}}}`. In
template YAML, `when` clauses use dot-path keys to reference nested values:
```yaml
when:
  gates.ci_check.exit_code: 0
```
The transition resolver traverses the dot-separated path to find the value in
the merged evidence map. Agent-submitted evidence lives at the top level (no
`gates.` prefix), preventing collisions.

**R4: Override defaults per gate.** Each gate type provides a default
`override_default` -- the values to substitute when the agent overrides
without providing explicit data. Command: `{exit_code: 0, error: ""}`,
context-exists: `{exists: true, error: ""}`, context-matches:
`{matches: true, error: ""}`. Template authors can declare a custom
`override_default` per gate to route overrides to a different transition.
The compiler validates that override defaults match the gate type's schema.

**R4a: Response format when gates don't pass.** When `koto next` encounters
gates whose output doesn't satisfy their pass condition and no override
exists, the response includes the gate name and its structured output in
the `blocking_conditions` array. The agent uses this data to decide whether
to override and what data to provide.

**R5: Override via `koto overrides record`.** Override is a subcommand of
`koto overrides`, mirroring the `koto decisions record` / `koto decisions
list` pattern:

```
koto overrides record <name> --gate <gate_name> --rationale "reason"
koto overrides record <name> --gate <gate_name> --rationale "reason" --with-data '{"field": "value"}'
```

Each call overrides a single gate with a single rationale. It appends a
`GateOverrideRecorded` event without advancing the workflow. Multiple agents
can push overrides independently before an orchestrator calls `koto next`.

**Without `--with-data`:** the gate's `override_default` values are
substituted. This is the simple case for gates with a clear "passing" value.

**With `--with-data`:** the provided data is substituted instead of the
override default. The data is validated against the gate type's schema. This
is for gates where the agent needs to specify what the output should be
(e.g., a label-checking gate where the agent provides which label to treat
as present).

A subsequent `koto next` reads override events from the current epoch and
substitutes the override data during gate evaluation, the same way it reads
`EvidenceSubmitted` events.

Override events are sticky within an epoch -- they persist in the event log
until the state transitions (new epoch). An agent can override gates across
multiple `koto overrides record` calls, then advance with a single
`koto next`.

**R5a: One gate, one rationale.** Each `koto overrides record` call targets
exactly one gate. To override multiple gates, make multiple calls. This
ensures each override has its own rationale.

**R6: Override event with full context.** On each override, the engine emits
a `GateOverrideRecorded` event containing: the state name, the overridden
gate's name, the gate's actual output, the substituted values (either
`override_default` or the agent-provided `--with-data`), and the rationale
string.

**R7: Gate and agent evidence coexistence.** A state can have both gates
(producing `gates.*` data) and an `accepts` block (for agent-submitted
evidence). Both feed into the same transition resolver. Gate output is
namespaced under `gates.<gate_name>` to prevent field collisions with agent
evidence. The `gates` top-level key is reserved -- evidence validation
rejects `--with-data` payloads containing a `gates` field to prevent agents
from overwriting engine-produced gate output.

**R8: Cross-epoch override query.** A `derive_overrides` function returns all
`GateOverrideRecorded` events across the full session. `koto overrides list`
exposes this to the CLI.

**R9: Compiler validation of the contract.** The template compiler validates
that:
- Gate types referenced in templates are registered (known to the engine)
- If `override_default` is declared, it matches the gate type's schema
- Transition `when` clauses that reference `gates.*` fields reference valid
  gate names and fields from the gate type's schema
- When override defaults are applied to all gates, at least one transition
  resolves (no dead ends on override)

*Implemented in [#123](https://github.com/tsukumogami/koto/pull/123). Design:
[DESIGN-gate-contract-compiler-validation](../designs/current/DESIGN-gate-contract-compiler-validation.md).*

**R10: Backward compatibility.** Existing templates continue to compile and
run. Gates still produce data internally (R1), but when transition `when`
clauses don't reference `gates.*` fields, the gate data doesn't enter the
resolver -- the legacy boolean pass/block behavior applies. This means
existing templates work without changes. The compiler warns about gates on
states where no `when` clause references their output, but doesn't error.
Templates that use `accepts` blocks with `override` enum values as the
workaround pattern continue to work identically -- `--with-data
'{"status": "override"}'` is still plain evidence submission unrelated to
the new override mechanism.

### Non-functional

**R11: Event ordering.** `koto overrides record` and `koto next` are
separate invocations. Events are ordered by their invocation timestamps via
sequence numbers. Override events from earlier calls have lower sequence
numbers than events from later calls.

**R12: Rationale size limit.** `--rationale` values on `koto overrides record`
are subject to the same size limit as `--with-data` (1MB).

## Acceptance criteria

- [ ] A gate produces structured data matching its gate type's schema on
  evaluation, not just pass/fail
- [ ] A gate is considered "passed" when its output satisfies the gate type's
  built-in pass condition (command: exit_code == 0, context-exists: exists ==
  true, context-matches: matches == true)
- [ ] Compiler rejects `override_default` that doesn't match the gate type's
  schema
- [ ] Template author can declare a custom `override_default` per gate that
  differs from the gate type's default, and the compiler validates it against
  the schema
- [ ] `koto overrides record` with `--with-data` substitutes the provided data
  instead of the override default
- [ ] `koto overrides record` with `--with-data` validates the data against
  the gate type's schema (rejects mismatched types)
- [ ] Override audit trail shows the agent-provided data (not the override
  default) when `--with-data` was used
- [ ] Transition `when` clauses can reference `gates.<name>.<field>` and match
  against gate output
- [ ] A state with both gates and `accepts` block routes correctly using both
  data sources in the same `when` clause
- [ ] `koto overrides record <name> --gate ci_check --rationale "reason"`
  appends a `GateOverrideRecorded` event without advancing the workflow
- [ ] After `koto overrides record`, a subsequent `koto next` reads the
  override event and substitutes gate defaults during gate evaluation
- [ ] `GateOverrideRecorded` event contains: state, gate name, actual output,
  override default applied, and rationale
- [ ] `koto overrides list` returns all override events across the session
- [ ] Override events are sticky within an epoch -- multiple
  `koto overrides record` calls accumulate, and `koto next` reads all of them
- [ ] Multiple `koto overrides record` calls from different agents can append
  to the same workflow without lock contention
- [ ] Each `koto overrides record` call produces its own `GateOverrideRecorded`
  event with its own rationale
- [ ] `koto overrides record` targeting a gate whose output already satisfies
  the pass condition still records the override (the agent's rationale is
  captured regardless)
- [ ] `koto overrides record` with `--rationale ""` returns a validation error
- [ ] `koto overrides record` with `--rationale` exceeding 1MB returns a
  validation error (R12)
- [ ] `koto next` resolves transitions using a merged evidence map: actual
  gate output for non-overridden gates, substituted data for overridden gates,
  plus any agent evidence. State advances only if a `when` clause matches.
- [ ] Compiler rejects templates where applying all override defaults leads to
  no valid transition (dead end on override)
- [ ] Compiler rejects templates where a `when` clause references a
  nonexistent gate or nonexistent field in a gate type's schema
- [ ] Existing templates compile and run without changes (backward compatible)
- [ ] Gates on states where no `when` clause references `gates.*` fields
  produce no structured output (legacy boolean behavior)
- [ ] Templates using `accepts` blocks with `override` enum values continue
  to work identically via `--with-data` (workaround pattern preserved)
- [ ] Override events survive rewind and are visible in `koto overrides list`
- [ ] Passing gates produce their structured output and transition routing
  works without agent interaction
- [ ] Command gate produces `{exit_code: 0, error: ""}` on success,
  `{exit_code: N, error: ""}` on failure
- [ ] Context-exists gate produces `{exists: true, error: ""}` when key
  exists, `{exists: false, error: ""}` when missing
- [ ] Context-matches gate produces `{matches: true, error: ""}` when pattern
  matches, `{matches: false, error: ""}` when it doesn't
- [ ] Override events from `koto overrides record` have sequence numbers
  reflecting their invocation order relative to other events (R11)
- [ ] A command gate that times out produces `{exit_code: -1, error:
  "timed_out"}` -- same schema shape, doesn't satisfy pass condition
- [ ] A command gate that errors (spawn failure) produces `{exit_code: -1,
  error: "<message>"}` -- same schema shape, doesn't satisfy pass condition
- [ ] A context-exists gate that errors produces `{exists: false, error:
  "<message>"}` -- consistent shape
- [ ] `koto overrides record` can override any gate regardless of its current
  output (the agent can substitute data even for gates that already "pass")
- [ ] `--with-data '{"gates": {...}}'` is rejected with a validation error
  (reserved namespace)
- [ ] `koto next` response for a gate-blocked state includes structured gate
  output in `blocking_conditions` (gate name + output fields, not just
  pass/fail)

## Out of scope

- **Visualization UI.** This PRD covers the data layer. Visualization is a
  future consumer.
- **Redo/rewind triggered by override disagreement.** The override data
  enables this; the redo mechanism is future work.
- **New gate type implementations.** This PRD covers the schema contract and
  override mechanism. Implementing new gate types (json-command, http, jira)
  is separate work that builds on this contract.
- **Custom gate output schemas.** Template authors can't declare custom
  output schemas for existing gate types. Richer output comes through new
  gate types (e.g., `json-command` that parses stdout as JSON). This keeps
  parsing logic in the engine, not the template.
- **`--to` directed transition tracking.** Separate mechanism, separate audit.
- **Action skip tracking.** Related but distinct.
- **Evidence verification by koto.** Future capability where koto
  independently validates agent-submitted evidence using gates. Enabled by
  this contract but not part of this PRD.

## Known limitations

- **Override default is a convenience, not the full mechanism.** The
  `override_default` provides a one-call shorthand for the common case. For
  richer gates where the agent needs to specify non-default values, they must
  use `--with-data` on `koto overrides record`.
- **One gate per override call.** Each `koto overrides record` call targets
  exactly one gate. Overriding N gates requires N calls. This is intentional:
  each gate gets its own rationale.
- **No caller identity.** Override events don't record who performed the
  override. This is a pre-existing gap across all koto event types.
- **Gate output depends on gate type implementation.** Each gate type
  (command, context-exists, context-matches) must be updated to produce
  structured data. New gate types automatically benefit from the schema
  contract.
- **Override without `--with-data` always uses the default.** When the agent
  doesn't provide explicit data, the gate type's `override_default` is
  substituted. This defaults to the "passing" value. To route to a non-default
  transition, the agent must provide `--with-data` with the desired values.
- **Dot-path traversal is new resolver capability.** The current
  `resolve_transition` does flat key matching against a `BTreeMap`. Gate
  output namespaced as nested maps (`{"gates": {"ci_check": {...}}}`)
  requires dot-path traversal in `when` clause matching. This is new work
  for the transition resolver, addressed in the design doc.
- **Compiler reachability is limited to enum fields.** The R9 reachability
  check (verifying override defaults lead to a valid transition) can only be
  done statically for enum-typed fields where all possible values are known.
  For numeric or string fields, the compiler can verify type compatibility
  but not whether a specific value matches a `when` condition. Reachability
  validation is best-effort for non-enum fields.

## Decisions and trade-offs

**D1: Namespaced gate output (`gates.<name>.<field>`) rather than flat
merge.** Gate output could merge flat into the evidence map (same namespace as
agent evidence). We chose namespacing because it prevents field name
collisions (a gate and an accepts block could both have a `status` field),
makes the data source explicit in `when` clauses, and lets the transition
resolver treat both sources uniformly without conflict resolution.

**D2: Override defaults in the gate schema, not at the state level.** Override
defaults could be declared once per state rather than per gate. We chose
per-gate because each gate has its own output schema and the default must
match that schema. A state-level default would need to know about all gates'
schemas, creating coupling. Per-gate keeps each gate self-contained.

**D3: This PRD supersedes PRD-override-gate-rationale.** The earlier PRD
(issue #108) scoped narrowly to adding `--override-rationale` to the existing
boolean gate model. This PRD broadens the scope to redesign the
gate/transition contract. The override rationale requirement (R5, R6, R8) is
preserved and strengthened -- override events now include both actual and
default gate output, not just the rationale.

**D4: Backward compatibility via gradual adoption.** Structured gate output
could be required on all templates immediately, but that would break every
existing template. We chose to have gates without `when` clauses referencing
`gates.*` fields behave as today (boolean pass/fail). The compiler warns but
doesn't error. This lets adoption be gradual.

**D5: Gate types are documented building blocks, not hidden compiler logic.**
Each gate type publishes a schema that template authors reference when writing
`when` clauses. The schema is the gate type's public contract, not an
implementation detail. Output schemas could alternatively be declared per-gate
in the template YAML, but there's no mechanism to connect a gate's raw
execution to a user-declared schema -- the parsing logic belongs in the gate
type, not the template. Future gate types (jira, http, json-command) extend
the registry with richer schemas and parsing logic, giving template authors
new building blocks to compose workflows from.

**D6: Override substitutes gate output, not transition destination.** Overrides
could target a specific transition destination (agent says "go to state X"),
bypassing the resolver entirely. We chose gate output substitution because:
(1) it preserves the resolver as the single routing authority -- one code path
for all transitions, (2) most overrides are "treat as passed" and don't need
routing control, (3) agents shouldn't need template topology knowledge for
simple overrides, and (4) gate output and agent evidence compose cleanly in the
resolver when both are present. For richer gates where the agent needs to
route to a non-default transition, `--with-data` on `koto overrides record`
lets the agent specify explicit gate output values. This keeps override
within the gate data model (the resolver still picks the target) rather than
bypassing it.

**D7: Override is a subcommand, not a flag on `koto next`.** Override could
be a flag on `koto next` (one call does override + advance) or a separate
command. We chose `koto overrides record` as a subcommand under the existing
`koto overrides` namespace, mirroring `koto decisions record`. One gate per
call, one rationale per call. This keeps concerns separated (`koto next`
advances, `koto overrides record` records overrides), enables multi-agent
composition (sub-agents push overrides independently), ensures every gate
gets its own rationale, and eliminates the complexity of a shorthand flag
that combines two operations. The pattern is proven: `koto decisions record`
works this way already.
