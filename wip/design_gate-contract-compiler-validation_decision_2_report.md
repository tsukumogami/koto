# Decision 2: override_default validation strictness

## Question

What should override_default validation enforce? When a gate in a template
declares `override_default: <json_value>`, the compiler should validate it
against the gate type's output schema.

## Options evaluated

**A. Exact match** — all schema fields required, no extra fields, correct types.

**B. Subset match** — present fields must have correct types, extra fields
rejected, missing fields allowed.

**C. Type-check only** — wrong types on known fields rejected; missing and extra
fields ignored.

**D. Warn only** — compilation succeeds; warnings for type mismatches or unknown
fields.

## Analysis

### The runtime semantics of override_default

At runtime, `override_applied` (sourced from either `override_default` or
`built_in_default`) is injected as the gate's entire output into the
`gates.*` evidence map. The transition resolver then evaluates `when` conditions
against that map. This means whatever object lands in `override_applied` becomes
the complete, authoritative output for that gate. There is no merging with the
gate's actual output.

This runtime contract has a direct implication for validation: if
`override_default` is partial, the missing fields are simply absent from the
evidence map. Any `when` condition referencing the missing field silently fails
to match. For `command` gates, if `exit_code` is missing, every transition
checking `gates.ci_check.exit_code` fails — regardless of the declared value.

### Why exact match (A) is the correct choice

The purpose of `override_default` is to guarantee that when an agent overrides a
gate, the workflow can still route. The built-in defaults (`{exit_code: 0, error:
""}`, `{exists: true, error: ""}`, `{matches: true, error: ""}`) are exactly
the gate type's full output shape. Custom `override_default` values should meet
the same contract the engine itself guarantees for built-in defaults.

Exact match closes this contract at compile time:

- **All fields present**: the resolver can evaluate every `when` condition that
  references this gate. No silent misses due to missing fields.
- **No extra fields**: extra fields can't be referenced in `when` clauses anyway
  (they'd be rejected by D3 validation, which checks `when` clause references
  against the schema). Accepting extra fields would silently discard data the
  author thought was meaningful.
- **Correct types**: prevents type mismatches that would cause silent `when`
  condition failures at runtime (e.g., `exit_code: "0"` instead of `exit_code: 0`).

PRD R4 says: "The compiler validates that override defaults match the gate
type's schema." PRD acceptance criteria says: "Compiler rejects `override_default`
that doesn't match the gate type's schema." The word "match" across both
statements implies full conformance, not partial.

### Why subset match (B) is insufficient

Subset match looks like a reasonable compromise — it catches type errors and
extra fields while being lenient about missing fields. The argument "partial is
caught by reachability" (Decision 4) is appealing but wrong as a validation
strategy.

The reachability check (D4) fires when override defaults applied to all gates
lead to no valid transition. A partial `override_default` may still satisfy one
transition (the one that only checks the present field), while silently breaking
another transition that checks the missing field. Reachability would not catch
this because one transition does resolve.

Example: a `command` gate with `override_default: {exit_code: 0}` (missing
`error`). If any `when` clause checks only `exit_code`, that transition
resolves. Reachability passes. But a `when` clause checking `error: ""` on any
other transition now fails silently at runtime. Subset match would accept this
template, and D4 would not catch it.

Exact match catches this at compile time with a clear, actionable error: "gate
'ci_check': override_default is missing required field 'error' (command schema
requires: {exit_code: number, error: string})."

### Why type-check only (C) is insufficient

Option C accepts both missing and extra fields, only flagging wrong types. This
gives authors the weakest possible guarantee: the compiler confirms that `exit_code`
is a number if present, but won't tell them that `exit_code` is absent or that
`bogus_field: true` isn't part of the schema. This provides almost no useful
feedback at compile time.

The gate type schemas are small (2-3 fields each). There's no ergonomic cost to
requiring all fields. Type-check only is appropriate for schemas with open
extension points (e.g., `--with-data` for agent-provided overrides, which
follows R5 to validate type but can't know what keys are semantically meaningful
beyond the gate schema). For `override_default` declared in the template, the
author has full visibility into what fields exist and what the schema requires.

### Why warn only (D) is counterproductive

Option D turns a category of author errors into non-blocking warnings. The
rationale would be: "let templates compile even with questionable override
defaults." But this directly contradicts the PRD acceptance criteria, which
requires *rejection* on schema mismatch. More importantly, the failure mode when
this goes wrong is a silent dead end at runtime — the agent overrides a gate,
nothing transitions, the workflow stalls with no error, and the author has to
trace back to realize their `override_default` was malformed.

Early rejection at compile time is the correct point of enforcement. The runtime
path is not a graceful recovery path — it's a silent failure.

## Decision

**Chosen: Option A — Exact match**

`override_default` validation requires:
1. All schema fields present (no missing fields)
2. No extra fields beyond the schema
3. Each field's value type compatible with the schema type (JSON number, boolean,
   string as appropriate)

Error messages must name the state, gate name, field, and what was expected vs.
found. For example:

```
error: gate 'ci_check' in state 'verify': override_default missing required field
'error' (command schema: {exit_code: number, error: string})

error: gate 'ci_check' in state 'verify': override_default has unknown field
'status'; command schema has no such field

error: gate 'ci_check' in state 'verify': override_default field 'exit_code'
has wrong type: expected number, found string
```

## Interaction with other decisions

- **Decision 1 (schema registry location)**: The schema registry must expose
  the full field list and types for each gate type. Exact match validation
  iterates this list. This is a tighter coupling than subset or type-check
  would require, but the schema registry is already needed for D1.

- **Decision 3 (when clause validation)**: If D3 adopts field-level validation,
  the set of valid fields in `when` clauses is the same set required by exact
  match here. The two validations share the schema registry and reinforce each
  other.

- **Decision 4 (reachability check)**: With exact match, the reachability check
  operates on well-formed override defaults — every field is present and typed
  correctly. This simplifies D4 implementation: it doesn't need to handle missing
  fields or ignore unknown fields before calling the resolver.

## Rejected options

| Option | Rejection reason |
|--------|-----------------|
| B: Subset match | A partial override_default passes reachability (D4) even when it would silently break `when` conditions checking the missing field; the "partial caught by reachability" argument fails for non-exhaustive transition sets |
| C: Type-check only | Accepts missing fields and extra fields entirely; provides minimal compile-time value for schemas that are small and fully documented |
| D: Warn only | Directly contradicts PRD acceptance criteria ("compiler rejects"); converts hard errors into soft warnings, leaving silent runtime dead ends as the failure mode |
