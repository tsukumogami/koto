# Phase 4 Round 3: Example-Requirement Consistency Review

Reviewing interaction examples against current requirements and decisions after
multiple revision rounds.

## Check 1: Do examples show both R5 invocation paths (command + flag)?

**Result: Minor gap (M1)**

R5 defines two paths:
- Command: `koto override <name> --gate <name> --rationale "reason"` (append, no advance)
- Flag: `koto next <name> --override-rationale "reason"` (append + advance)

D7 justifies both paths: the command enables multi-agent composition, the flag
preserves single-agent simplicity.

Examples 1 and 4 use the flag path (`koto next --override-rationale`). Example 5
shows `koto overrides list` (query, R8). No example demonstrates the `koto override`
command as a standalone step. The acceptance criteria cover it (lines 433-441) but
interaction examples should illustrate the two-step flow that D7 motivates.

**M1**: Add an example showing `koto override` followed by `koto next`, especially
since D7 calls out multi-agent composition as the rationale for having both.

## Check 2: Do YAML examples match R1's gate type schema table?

**Result: PASS**

R1 schema table:
- `command`: `{exit_code: number, error: string}`
- `context-exists`: `{exists: boolean, error: string}`
- `context-matches`: `{matches: boolean, error: string}`

Example 1: `gates.ci_check.exit_code` -- matches `command`. YAML comment says
`# produces {exit_code: number}` which omits `error` but is a shorthand hint, not
a schema declaration. Acceptable.

Example 2: `gates.lint.exit_code` -- matches `command`.

Example 3: `gates.file_exists.exists` -- matches `context-exists`.

No `context-matches` example exists, but that's coverage, not inconsistency.

## Check 3: Do JSON responses match what requirements say the CLI returns?

**Result: PASS**

Example 1 blocked response:
```json
{"action": "gate_blocked", "state": "verify",
 "blocking_conditions": [{"gate": "ci_check", "output": {"exit_code": 1}}]}
```
Matches the last acceptance criterion (line 466): structured gate output in
`blocking_conditions` with gate name and output fields.

Example 1 advance response: `{"action": "done", "state": "deploy", "advanced": true}`
follows the existing `koto next` response shape.

Example 4 blocked/advance responses are structurally consistent.

Example 4 uses gate output fields (`valid`, `within_limit`, `size_mb`) from
hypothetical future gate types not in R1's table. See N1 below.

## Check 4: Do override_default values match R4's description?

**Result: PASS**

R4 defaults: command `{exit_code: 0}`, context-exists `{exists: true}`,
context-matches `{matches: true}`.

Example 1 comment: "Engine applies ci_check's override_default: {exit_code: 0}" --
matches R4's command default.

Example 5's `override_applied` values use hypothetical gate types (see N1) but
the concept is consistent: substitute values that satisfy the pass condition.

No example shows explicit `override_default` declaration in template YAML. R4 says
it's optional (sensible defaults provided), so this is fine.

## Check 5: Does Example 2 still work after D6 and D7?

**Result: PASS**

D6 (override substitutes gate output, not destination): Example 2 doesn't show
an override -- just gate + accepts coexistence in `when` clauses. No conflict.

D7 (override is both command and flag): Example 2 doesn't invoke override.
No conflict.

The transition `- target: revise, when: decision: request_changes` routes on
agent evidence alone (no gate condition), which is a valid pattern under R7.

## Check 6: References to the old model?

**Result: Minor issue (M2)**

R10 (line 379) says: "Existing templates without `output_schema` continue to
compile and run" and "The transition resolver only receives `gates.*` namespaced
data when the gate declares an `output_schema`."

But D5 and the "Out of scope" section (line 479) eliminated user-declared
`output_schema` -- gate types own their schemas. There is no `output_schema` field
in template YAML. R10's wording is a vestige of the earlier model where gates
declared their own output schemas.

**M2**: R10 should describe backward compatibility in terms of `when` clause usage,
not `output_schema`. Suggested: "Existing templates without `when` clauses
referencing `gates.*` fields continue to compile and run."

The interaction examples themselves are clean -- no `output_schema` appears in
any YAML block.

## Additional finding: R3 illustrative example uses nonexistent field

R3 (lines 312-316) illustrates the namespace with:
```yaml
when:
  gates.ci_check.status: passed
```
and `{"gates": {"ci_check": {"status": "passed"}}}`.

No R1 gate type has a `status` field. The `command` type produces `{exit_code,
error}`. This is a requirements-section inconsistency rather than an interaction
example issue, but it could confuse readers cross-referencing R1 and R3. It was
flagged in the prior review round but persists.

---

## Summary

| # | Severity | Description |
|---|----------|-------------|
| M1 | Minor | No interaction example demonstrates `koto override` as a standalone command (R5/D7 two-step flow) |
| M2 | Minor | R10 references `output_schema` as a template field -- vestige of old model, contradicts D5 and out-of-scope |
| N1 | Nit | Examples 4-5 use gate output fields from undeclared gate types without noting they're hypothetical |
| -- | Carried | R3's illustrative `gates.ci_check.status: passed` uses a field not in any R1 schema (flagged in prior round) |

**Verdict: PASS -- 3 new issues (0 blocking, 2 minor, 1 nit) + 1 carried from prior round**
