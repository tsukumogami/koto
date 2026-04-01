# Phase 4 Round 4: Examples Coherence Review

Reviewing the 7 interaction examples for internal coherence after multiple
revision rounds. Each check maps to a specific review question.

## Verdict: PASS with 3 issues (1 moderate, 2 minor)

---

## Check 1: Do YAML templates use R1's gate type output schemas?

**Result: PASS (Examples 1-3), N/A (Example 4)**

R1 defines three initial gate types:

| Gate type | Output schema |
|-----------|---------------|
| `command` | `{exit_code: number, error: string}` |
| `context-exists` | `{exists: boolean, error: string}` |
| `context-matches` | `{matches: boolean, error: string}` |

- Example 1: `command` type, routes on `gates.ci_check.exit_code`. Matches R1.
- Example 2: `command` type, routes on `gates.lint.exit_code`. Matches R1.
- Example 3: `context-exists` type, routes on `gates.file_exists.exists`. Matches R1.
- Example 4: Uses `list_github_issue_labels` with `{labels: [string]}` schema.
  This is not in R1's initial table. See Check 7.
- Examples 5-7: No YAML templates (CLI-only demonstrations).

No example uses `context-matches`, but that is a coverage gap, not an
inconsistency.

## Check 2: Do CLI invocations match R5's syntax?

**Result: PASS**

R5 defines:
```
koto overrides record <name> --gate <gate_name> --rationale "reason"
koto overrides record <name> --gate <gate_name> --rationale "reason" --with-data '...'
```

- Example 1: `koto overrides record my-workflow --gate ci_check --rationale "..."` -- matches R5.
- Example 4: `koto overrides record my-workflow --gate labels --rationale "..." --with-data '...'` -- matches R5.
- Example 5: Two `koto overrides record` calls, each with `--gate` and `--rationale` -- matches R5.
- Example 6: Two `koto overrides record` calls from different sub-agents -- matches R5.
- Example 7: `koto overrides list my-workflow` -- matches R8's query command.

All invocations are consistent with R5's current syntax. The earlier r3 review
noted a `koto override` (singular) vs `koto overrides record` discrepancy; that
has been fixed in this revision.

## Check 3: Do JSON responses make sense given gate output?

**Result: MINOR ISSUE (I1)**

- Example 1 blocked response: `{"action": "gate_blocked", "state": "verify",
  "blocking_conditions": [{"gate": "ci_check", "output": {"exit_code": 1, "error": ""}}]}`
  The `output` field matches R1's command schema. Correct.

- Example 1 advance response: `{"action": "done", "state": "deploy", "advanced": true}`
  The current codebase uses `"action": "terminal"` for terminal states, not
  `"done"`. However, `deploy` is not necessarily terminal -- it's just the next
  state. The current `koto next` output contract uses action values:
  `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`,
  `terminal`, `action_requires_confirmation`. There is no `"done"` action.

  **I1 (minor)**: Examples 1, 4, 5, and 6 use `"action": "done"` for successful
  transitions. The actual `koto next` response after advancing would be the
  response for the *new* state (e.g., `evidence_required` if `deploy` needs
  evidence, `terminal` if it's terminal). `"done"` is not a defined action value.
  This is a simplification for readability, but readers familiar with the output
  contract may find it confusing. Consider adding a note that these are
  abbreviated for clarity, or use a real action value.

- Example 4 blocked response: `{"action": "gate_blocked", "state": "triage",
  "blocking_conditions": [{"gate": "labels", "output": {"labels": [], "error": ""}}]}`
  The output shape matches the hypothetical `list_github_issue_labels` schema.

- Example 5 blocked responses show progressive unblocking (first two gates
  blocked, then one, then advance). Consistent with R5's sticky override model.

- Example 7 audit response: Shows `actual_output` and `override_applied` per
  override event. Consistent with R6's event content requirements.

## Check 4: Is the narrative text accurate?

**Result: PASS**

- Example 1 narrative: "gate produces `{exit_code: 0}`, transition resolver
  matches..." -- accurate for the template shown.
- Example 1 override narrative: "engine reads the override event, substitutes
  `{exit_code: 0}`" -- matches R4's override default for command gates.
- Example 2 narrative: "Gate output and agent evidence coexist in the same
  transition resolver" -- matches R7.
- Example 3 narrative: "Even simple gates get a schema" -- matches R1's design
  philosophy.
- Example 4 narrative: "Without `--with-data`, the override would substitute
  `override_default`" -- matches R4/R5.
- Example 5 narrative: "Override events are sticky within an epoch" -- matches R5.
- Example 6 narrative: "Sub-agents push overrides independently" -- matches R5/D7.

## Check 5: Do examples reference each other correctly?

**Result: PASS**

- Example 5 and Example 6 both use the same two gates (`schema_check`,
  `size_check`) on a `validate` state advancing to `process`. Example 6
  explicitly presents as a multi-agent variant of the same scenario.
- Example 7 shows audit output for the overrides recorded in Examples 5/6
  (same gate names, same rationale strings, same state name). The
  `actual_output` and `override_applied` values are consistent with the
  command gate schema.

Cross-references are coherent.

## Check 6: Is "gate_blocked" the right action value?

**Result: PASS**

The question asks whether `gate_blocked` is still correct now that gates return
structured data instead of pass/fail.

Yes. The action value `gate_blocked` is the existing value in the codebase
(serialized at `next_types.rs:192`). The PRD's design (R1, R2) changes what
data gates produce, but the concept of "gates blocking a state" remains. When
any gate's output does not satisfy its pass condition (R1's pass condition
column), the state is blocked. The action value names the *situation*
(state can't advance because gates didn't pass), not the gate's output format.
Structured data changes what the `blocking_conditions` array contains, not the
action type.

The `blocking_conditions` array format does change from the current
`{name, type, status, agent_actionable}` to the proposed `{gate, output}`.
That structural change is the PRD's contribution, but the action value
`gate_blocked` remains correct.

## Check 7: Is the future gate type clearly marked?

**Result: MODERATE ISSUE (I2)**

Example 4 uses `list_github_issue_labels` as a gate type. The example's
introductory sentence says "A future `list_github_issue_labels` gate..." which
signals it's hypothetical. However:

**I2 (moderate)**: The `when` clause uses `gates.labels.contains: bug`, but the
described output schema is `{labels: [string]}`. The field `contains` is not a
field in the output schema -- it appears to be a proposed list-matching operator,
not a schema field. R1's transition resolver uses direct field matching
(`gates.ci_check.exit_code: 0`). There's no mechanism described in R3 for
operators like `contains` on list fields. The `--with-data '{"contains": "bug"}'`
in the override command also uses `contains` as a field name rather than a list
value.

This creates ambiguity: is `contains` a field in the gate's output schema, a
new resolver operator for list types, or a shorthand? R3 only describes
dot-path traversal for nested maps, not list membership testing.

The example works as a motivating illustration for future gate types, but the
routing mechanism it shows (`contains` on a list) goes beyond what R3 defines.
This should either be noted as requiring resolver extensions, or the example
should use a simpler schema that works with R3's current dot-path matching.

## Check 8: Additional coherence issue found

**Result: MINOR ISSUE (I3)**

**I3 (minor)**: Example 4's `--with-data '{"contains": "bug"}'` and the
subsequent transition resolution narrative ("Transition resolver matches
`gates.labels.contains: bug -> is_bug`") imply that `--with-data` on
`koto overrides record` substitutes into the `gates.*` namespace. But R5 says
`--with-data` data "is validated against the gate type's schema." If the gate
schema is `{labels: [string]}`, then `{"contains": "bug"}` doesn't match that
schema -- it would fail R5's validation. The example is internally inconsistent
with R5's schema validation requirement.

---

## Issue Summary

| ID | Severity | Location | Description |
|----|----------|----------|-------------|
| I1 | Minor | Ex 1, 4, 5, 6 | `"action": "done"` is not a defined action value in the current output contract. Should note this is abbreviated or use real action values. |
| I2 | Moderate | Ex 4 | `contains` operator in `when` clause and `--with-data` is not covered by R3's dot-path resolution. The example's routing mechanism goes beyond what the PRD defines. |
| I3 | Minor | Ex 4 | `--with-data '{"contains": "bug"}'` would fail R5's schema validation against `{labels: [string]}`. Internal inconsistency between example and requirement. |

Note: I2 and I3 are related -- both stem from Example 4 using a list-matching
pattern that the PRD's resolver mechanism doesn't support. Fixing one likely
fixes both.
