---
status: Draft
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

Draft

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

**As an agent (workflow skill)**, I want to override a failed gate with
`--override-rationale "reason"` and have the engine use the gate's declared
default override values for transition routing, so I don't need to know the
gate's internal schema to bypass it.

**As a human reviewer**, I want to query all gate overrides in a session and
see the rationale, the gate failure context, and what default values were
applied, so I can audit agent behavior.

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
 "blocking_conditions": [{"gate": "ci_check", "output": {"exit_code": 1}}]}

$ koto next my-workflow --override-rationale "Flaky test, unrelated to this change"
# Engine applies ci_check's override_default: {exit_code: 0}
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

Gate output (`gates.lint.status`) and agent evidence (`decision`) coexist in the
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

### Example 4: Selective override with rationale

Each `koto next` call re-evaluates gates from scratch. Overrides aren't
sticky -- they apply to that call only.

```bash
# Two gates fail
$ koto next my-workflow
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "schema_check", "output": {"valid": false, "errors": 3}},
   {"gate": "size_check", "output": {"within_limit": false, "size_mb": 15}}
 ]}

# Override both in one call, targeting each by name
$ koto next my-workflow --override-rationale "Schema: deprecated fields; Size: test fixture" \
    --gate schema_check --gate size_check
# Both gates get their override defaults, transition proceeds
{"action": "done", "state": "process", "advanced": true}

# Or override all at once (no --gate flag)
$ koto next my-workflow --override-rationale "Both checks are irrelevant for this change"
# All failing gates get their override defaults
{"action": "done", "state": "process", "advanced": true}

# Selective override that leaves some gates blocked
$ koto next my-workflow --override-rationale "Schema errors are in deprecated fields" \
    --gate schema_check
# schema_check is overridden, but size_check still fails -- state stays blocked
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "size_check", "output": {"within_limit": false, "size_mb": 15}}
 ]}
# Agent must override size_check in the same call or a subsequent one
```

### Example 5: Override audit trail

```bash
$ koto overrides list my-workflow
{"overrides": [
  {"state": "validate",
   "gates_overridden": [
     {"gate": "schema_check",
      "actual_output": {"valid": false, "errors": 3},
      "override_applied": {"valid": true, "errors": 0}}
   ],
   "rationale": "Schema errors are in deprecated fields",
   "seq": 12, "timestamp": "2026-03-31T10:15:00Z"},
  {"state": "validate",
   "gates_overridden": [
     {"gate": "size_check",
      "actual_output": {"within_limit": false, "size_mb": 15},
      "override_applied": {"within_limit": true, "size_mb": 0}}
   ],
   "rationale": "Large file is a test fixture, expected",
   "seq": 14, "timestamp": "2026-03-31T10:15:30Z"}
]}
```

Each override event captures what the gate actually returned, what default was
applied, and why. Self-contained for visualization. Selective overrides produce
separate events per invocation.

## Requirements

### Functional

**R1: Gate types are reusable building blocks with documented schemas.** Each
gate type defines a public output schema and a pass condition. These are
documented so template authors know what fields are available when writing
`when` clauses. The schema is the gate type's contract -- it tells authors
exactly what data the gate produces and what "passing" means.

Initial gate types and their schemas:

| Gate type | Output schema | Pass condition |
|-----------|--------------|----------------|
| `command` | `{exit_code: number}` | `exit_code == 0` |
| `context-exists` | `{exists: boolean}` | `exists == true` |
| `context-matches` | `{matches: boolean}` | `matches == true` |

Future gate types extend this registry with richer schemas:

| Gate type (future) | Output schema | Pass condition |
|-----------|--------------|----------------|
| `json-command` | `{exit_code: number, data: object}` (stdout parsed as JSON) | `exit_code == 0` |
| `http` | `{status_code: number, body: object}` | `status_code >= 200 && < 300` |
| `jira` | `{assigned: boolean, status: string, assignee: string}` | `assigned == true` |

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
JSON map: `{"gates": {"ci_check": {"status": "passed"}}}`. In template YAML,
`when` clauses use dot-path keys to reference nested values:
```yaml
when:
  gates.ci_check.status: passed
```
The transition resolver traverses the dot-separated path to find the value in
the merged evidence map. Agent-submitted evidence lives at the top level (no
`gates.` prefix), preventing collisions.

**R4: Override defaults per gate.** Each gate in the template declares an
`override_default` -- the values to substitute when the agent overrides.
The compiler validates that override defaults match the gate type's schema
and satisfy the gate type's built-in pass condition. If no `override_default`
is declared, the gate type provides a sensible default (command:
`{exit_code: 0}`, context-exists: `{exists: true}`, context-matches:
`{matches: true}`).

**R5: Engine-level override with rationale.** `koto next` accepts
`--override-rationale <string>` with an optional `--gate <name>` flag to
target specific gates. When `--gate` is omitted, all failing gates are
overridden. When `--gate` is provided (repeatable), only the named gates are
overridden; other failing gates remain blocked. The rationale is mandatory
(non-empty string) and applies to all gates being overridden in that call.

**R5a: Selective override.** `--gate <name>` can be repeated to override
multiple specific gates in one call. If a named gate isn't actually failing,
it's ignored (no error). If after applying selective overrides some gates
still fail, the state remains blocked -- the agent must override those too
or wait for them to pass.

**R6: Override event with full context.** On each override (whether it
advances the state or not), the engine emits a `GateOverrideRecorded` event
containing: the state name, each overridden gate's name, actual output, and
applied override default, and the rationale string. If selective override
leaves some gates still failing, the event still records what was overridden
(the state just doesn't advance yet).

**R7: Gate and agent evidence coexistence.** A state can have both gates
(producing `gates.*` data) and an `accepts` block (for agent-submitted
evidence). Both feed into the same transition resolver. Gate output is
namespaced under `gates.<gate_name>` to prevent field collisions with agent
evidence.

**R8: Cross-epoch override query.** A `derive_overrides` function returns all
`GateOverrideRecorded` events across the full session. `koto overrides list`
exposes this to the CLI.

**R9: Compiler validation of the contract.** The template compiler validates
that:
- Gate types referenced in templates are registered (known to the engine)
- If `override_default` is declared, it matches the gate type's schema
- If `override_default` is declared, it satisfies the gate type's pass
  condition
- Transition `when` clauses that reference `gates.*` fields reference valid
  gate names and fields from the gate type's schema
- When override defaults are applied to all failing gates, at least one
  transition resolves (no dead ends on override)

**R10: Backward compatibility.** Existing templates without `output_schema`
continue to compile and run. Gates without schemas behave as today: boolean
pass/fail, the `gate_failed` flag controls transition resolution, and no
`gates.*` data enters the transition resolver. The transition resolver only
receives `gates.*` namespaced data when the gate declares an `output_schema`.
No implicit schema is generated for legacy gates. The compiler warns about
gates without schemas but doesn't error. Templates that use `accepts` blocks
with `override` enum values as the workaround pattern continue to work
identically -- `--with-data '{"status": "override"}'` is still plain evidence
submission unrelated to the new override mechanism.

### Non-functional

**R11: Event ordering.** When `--override-rationale` is combined with
`--with-data`, `EvidenceSubmitted` and `GateOverrideRecorded` are emitted in
strict sequence within the same invocation.

**R12: Rationale size limit.** `--override-rationale` values are subject to
the same size limit as `--with-data` (1MB).

## Acceptance criteria

- [ ] A gate produces structured data matching its gate type's schema on
  evaluation, not just pass/fail
- [ ] A gate is considered "passed" when its output satisfies the gate type's
  built-in pass condition (command: exit_code == 0, context-exists: exists ==
  true, context-matches: matches == true)
- [ ] Compiler rejects `override_default` that doesn't match the gate type's
  schema
- [ ] Compiler rejects `override_default` that doesn't satisfy the gate type's
  pass condition
- [ ] Transition `when` clauses can reference `gates.<name>.<field>` and match
  against gate output
- [ ] A state with both gates and `accepts` block routes correctly using both
  data sources in the same `when` clause
- [ ] `--override-rationale` on a gate-blocked state applies override defaults
  for failing gates and advances via normal transition resolution
- [ ] `GateOverrideRecorded` event contains: state, gate name, actual output,
  override default applied, and rationale
- [ ] `koto overrides list` returns all override events across the session
- [ ] Compiler rejects templates where `override_default` doesn't match the
  gate type's schema
- [ ] Compiler rejects templates where applying all override defaults leads to
  no valid transition (dead end on override)
- [ ] Compiler warns on templates where a `when` clause references a
  nonexistent gate or field
- [ ] Existing templates compile and run without changes (backward compatible)
- [ ] `--override-rationale` with `--gate ci_check` overrides only `ci_check`;
  other failing gates remain blocked
- [ ] `--override-rationale` with `--gate a --gate b` overrides both named gates
- [ ] `--override-rationale` without `--gate` overrides all failing gates
- [ ] `--gate nonexistent_gate` is silently ignored (no error)
- [ ] Selective override that leaves some gates failing keeps the state blocked
- [ ] Each override invocation produces its own `GateOverrideRecorded` event
- [ ] `--override-rationale ""` returns a validation error
- [ ] `--override-rationale` on a non-blocked state is a no-op
- [ ] Override events survive rewind and are visible in `koto overrides list`
- [ ] Passing gates produce their structured output and transition routing
  works without agent interaction
- [ ] Command gate produces `{exit_code: 0}` on success, `{exit_code: N}` on
  failure
- [ ] Context-exists gate produces `{exists: true}` when key exists,
  `{exists: false}` when missing
- [ ] Context-matches gate produces `{matches: true}` when pattern matches,
  `{matches: false}` when it doesn't
- [ ] When `--override-rationale` is combined with `--with-data`,
  `EvidenceSubmitted` has a lower sequence number than `GateOverrideRecorded`
  (R11)
- [ ] `--override-rationale` with a string exceeding 1MB returns a validation
  error (R12)
- [ ] A gate that times out produces a structured error output (e.g.,
  `{error: "timed_out"}`) that doesn't match the gate's `pass_condition`
- [ ] A gate that errors (spawn failure, invalid regex) produces a structured
  error output that doesn't match the gate's `pass_condition`
- [ ] `--gate` targeting a gate that already passed is silently ignored

## Out of scope

- **Visualization UI.** This PRD covers the data layer. Visualization is a
  future consumer.
- **Redo/rewind triggered by override disagreement.** The override data
  enables this; the redo mechanism is future work.
- **Dynamic override values.** Override defaults are static (declared in the
  template). Letting agents supply runtime override values is deferred.
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

- **Override defaults are static.** The `override_default` is declared in the
  template at compile time. It can't vary based on runtime context (e.g.,
  "override to passed but with actual coverage value"). Dynamic override
  values would require the agent to submit gate-shaped evidence, which could
  be a future extension.
- **Rationale is shared across gates in one call.** When overriding multiple
  gates with `--gate a --gate b`, both share the same rationale string. If
  each gate needs distinct reasoning, the agent must make separate calls.
- **No caller identity.** Override events don't record who performed the
  override. This is a pre-existing gap across all koto event types.
- **Gate output depends on gate type implementation.** Each gate type
  (command, context-exists, context-matches) must be updated to produce
  structured data. New gate types automatically benefit from the schema
  contract.
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
