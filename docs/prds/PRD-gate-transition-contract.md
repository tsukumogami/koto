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
        type: command
        command: "run-ci --json"
        output_schema:
          status:
            type: enum
            values: [passed, failed]
        override_default:
          status: passed
    transitions:
      - target: deploy
        when:
          gates.ci_check.status: passed
      - target: fix
        when:
          gates.ci_check.status: failed
```

**CI passes:** gate produces `{status: "passed"}`, transition resolver matches
`gates.ci_check.status: passed`, advances to `deploy`. No agent interaction
needed.

**CI fails:** gate produces `{status: "failed"}`, transition resolver matches
`gates.ci_check.status: failed`, advances to `fix`. No agent interaction
needed -- the gate result drives routing automatically.

**CI fails + agent overrides:**

```bash
$ koto next my-workflow
{"action": "gate_blocked", "state": "verify",
 "blocking_conditions": [{"gate": "ci_check", "status": "failed"}]}

$ koto next my-workflow --override-rationale "Flaky test, unrelated to this change"
# Engine applies ci_check's override_default: {status: "passed"}
# Transition resolver matches gates.ci_check.status: passed -> deploy
{"action": "done", "state": "deploy", "advanced": true}
```

The override default tells the engine: "when overridden, treat this gate as if
it returned `{status: passed}`." The transition resolver routes normally.

### Example 2: Gate data and agent evidence coexisting

```yaml
states:
  review:
    gates:
      lint:
        type: command
        command: "run-lint --json"
        output_schema:
          status:
            type: enum
            values: [clean, warnings, errors]
        override_default:
          status: clean
    accepts:
      decision:
        type: enum
        values: [approve, request_changes]
        required: true
    transitions:
      - target: merge
        when:
          gates.lint.status: clean
          decision: approve
      - target: revise
        when:
          decision: request_changes
      - target: merge
        when:
          gates.lint.status: warnings
          decision: approve
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
        type: context-exists
        key: "config.yaml"
        output_schema:
          exists:
            type: boolean
        override_default:
          exists: true
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

```bash
# Two gates fail
$ koto next my-workflow
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "schema_check", "output": {"valid": false, "errors": 3}},
   {"gate": "size_check", "output": {"within_limit": false, "size_mb": 15}}
 ]}

# Override just schema_check, leave size_check blocked
$ koto next my-workflow --override-rationale "Schema errors are in deprecated fields" \
    --gate schema_check
# Engine applies schema_check's override_default, but size_check still fails
{"action": "gate_blocked", "state": "validate",
 "blocking_conditions": [
   {"gate": "size_check", "output": {"within_limit": false, "size_mb": 15}}
 ]}

# Now override size_check too
$ koto next my-workflow --override-rationale "Large file is a test fixture, expected" \
    --gate size_check
# All gates resolved, transition proceeds
{"action": "done", "state": "process", "advanced": true}

# Override all at once (no --gate flag)
$ koto next my-workflow --override-rationale "Both checks are irrelevant for this change"
# All failing gates get their override defaults
{"action": "done", "state": "process", "advanced": true}
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

**R1: Gate output schemas.** Each gate in a template declares an
`output_schema` defining the structured data it produces. The schema uses the
same field types as `accepts` blocks (string, number, boolean, enum).

**R2: Gate evaluation produces structured data.** When a gate evaluates, it
produces a structured result matching its declared `output_schema`, not just
pass/fail. The engine populates gate output fields based on the gate type's
evaluation result.

**R3: Gate output feeds into transition routing.** Gate output is available to
transition `when` clauses via namespaced fields (`gates.<gate_name>.<field>`).
The transition resolver matches gate output alongside agent evidence using the
same matching logic.

**R4: Override defaults per gate.** Each gate declares an `override_default`
in its schema -- the values to use when the agent overrides the gate. Override
defaults must match the gate's `output_schema`.

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
- Every gate has an `output_schema` and an `override_default`
- Override defaults match their gate's output schema
- Transition `when` clauses reference valid gate output fields
- When override defaults are applied to all failing gates, at least one
  transition resolves (no dead ends on override)

**R10: Backward compatibility.** Existing templates without `output_schema`
continue to compile and run. Gates without schemas behave as today (boolean
pass/fail with no structured output). The compiler warns but doesn't error.

### Non-functional

**R11: Event ordering.** When `--override-rationale` is combined with
`--with-data`, `EvidenceSubmitted` and `GateOverrideRecorded` are emitted in
strict sequence within the same invocation.

**R12: Rationale size limit.** `--override-rationale` values are subject to
the same size limit as `--with-data` (1MB).

## Acceptance criteria

- [ ] A gate with `output_schema` produces structured data on evaluation, not
  just pass/fail
- [ ] Transition `when` clauses can reference `gates.<name>.<field>` and match
  against gate output
- [ ] A state with both gates and `accepts` block routes correctly using both
  data sources in the same `when` clause
- [ ] `--override-rationale` on a gate-blocked state applies override defaults
  for failing gates and advances via normal transition resolution
- [ ] `GateOverrideRecorded` event contains: state, gate name, actual output,
  override default applied, and rationale
- [ ] `koto overrides list` returns all override events across the session
- [ ] Compiler rejects templates where a gate has `output_schema` but no
  `override_default`
- [ ] Compiler rejects templates where `override_default` doesn't match
  `output_schema`
- [ ] Compiler rejects templates where applying all override defaults leads to
  no valid transition (dead end on override)
- [ ] Compiler warns on templates where a `when` clause references a
  nonexistent gate or field
- [ ] Existing templates without `output_schema` compile and run without
  changes (backward compatible)
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

## Out of scope

- **Visualization UI.** This PRD covers the data layer. Visualization is a
  future consumer.
- **Redo/rewind triggered by override disagreement.** The override data
  enables this; the redo mechanism is future work.
- **Dynamic override values.** Override defaults are static (declared in the
  template). Letting agents supply runtime override values is deferred.
- **Gate output from stdout parsing.** How command gates produce structured
  data from command output (JSON parsing, regex extraction) is a design
  concern. This PRD requires that they can, not how.
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

**D4: Backward compatibility via optional schemas.** New gate schemas could be
required on all templates immediately, but that would break every existing
template. We chose to make `output_schema` optional with a compiler warning,
so existing templates keep working. Gates without schemas behave as today
(boolean pass/fail). This lets adoption be gradual.
