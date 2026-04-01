# Clarity Review -- Round 2

## Verdict: CONDITIONAL PASS

Round 1 found 11 ambiguities. The revised PRD addressed the most critical ones: pass conditions are now defined per gate type, namespace serialization is specified as nested JSON map with dot-path traversal, and event ordering is explicit. Seven new ambiguities remain, most at the implementation boundary rather than the contract level. None are blockers if addressed before design.

## New Ambiguities Found (7)

### A1: Pass condition for command gate doesn't cover abnormal termination

R1 defines the command gate's pass condition as `exit_code == 0`. The acceptance criteria say a command gate produces `{exit_code: N}` on failure. But what value does `exit_code` take when:

- The process is killed by a signal (SIGKILL, SIGTERM) -- typical OS convention is 128+signal or -1, but the PRD doesn't say.
- The command can't be spawned at all (binary not found, permission denied).

The current `GateResult` enum has separate variants for `Passed`, `Failed { exit_code }`, `TimedOut`, and `Error { message }`. The acceptance criteria (lines 432-435) say timeouts produce `{error: "timed_out"}` and spawn failures produce a structured error that doesn't match the pass condition. But the command gate's schema is `{exit_code: number}` -- these error outputs don't conform to that schema. Either:

(a) Error/timeout outputs extend the schema (e.g., `{exit_code: number, error: string?}`) and template authors must handle both shapes in `when` clauses, or
(b) Error/timeout produce the base schema with a sentinel value (e.g., `{exit_code: -1}`) and the `error` field is metadata not available to `when` clauses, or
(c) The gate type's schema is the *success* schema, and error states produce a separate error envelope that the transition resolver handles specially.

The PRD uses both shapes without reconciling them. The table in R1 says command output is `{exit_code: number}`, but the acceptance criteria reference `{error: "timed_out"}` which is a different shape entirely.

**Suggested fix:** Specify whether error/timeout outputs conform to the gate type's declared schema or use a separate error envelope. State how `when` clauses match (or don't match) error outputs.

### A2: Pass conditions are fixed per type but override_default is per instance -- mismatch when routing to non-passing states

R4 says override defaults must satisfy the gate type's pass condition. This means overriding always routes through the "passing" path. But Example 1 shows transitions for both `exit_code: 0` (deploy) and `exit_code: 1` (fix). A template author might reasonably want an override to route to "fix" instead of "deploy" -- acknowledging the gate failed but choosing to proceed anyway with a recovery path rather than the happy path.

The current design forces all overrides through the pass condition, which means overrides always take the happy path. If an author wants "override but go to fix," they can't express that with override_default. They'd need to use `--with-data` to submit agent evidence and route via a separate accepts field, which is exactly the workaround pattern the PRD is trying to eliminate.

This isn't necessarily wrong -- it's a deliberate constraint. But the PRD doesn't acknowledge it as a limitation or explain why overrides should always route through the passing path. A reader could interpret the override mechanism as "skip the gate and choose where to go" rather than "skip the gate and pretend it passed."

**Suggested fix:** Add to Known Limitations or Decisions: "Override defaults must satisfy the pass condition, meaning overrides always route through the passing path. Authors who need override-to-failure routing should use accepts blocks with agent evidence." Or alternatively, allow override_default to not satisfy the pass condition and document the implications.

### A3: Dot-path traversal is new behavior -- resolve_transition needs rewriting but the scope isn't called out

R3 says `when` clauses use dot-path keys (`gates.ci_check.exit_code`) that the transition resolver traverses against a nested JSON map. The current `resolve_transition` implementation does flat `evidence.get(field)` lookups against a `BTreeMap<String, serde_json::Value>`. It has no concept of nested map traversal.

The PRD specifies the desired behavior (dot-path traversal of nested maps) but doesn't acknowledge this as a new capability in the transition resolver. An implementer might read R3 and assume the resolver already supports this, since R3 is framed as "gate output feeds into transition routing" rather than "the transition resolver must be extended with dot-path traversal."

More specifically, the interaction between nested gate data and flat agent evidence needs clarification. If the evidence map is `{"gates": {"ci_check": {"exit_code": 0}}, "decision": "approve"}`, does the `when` clause `decision: approve` do a flat lookup while `gates.ci_check.exit_code: 0` does a dot-path traversal? Or are all lookups dot-path traversals (meaning `decision` is a single-segment path)?

**Suggested fix:** Add a sentence to R3: "This requires extending the transition resolver to support dot-path traversal into nested maps. Single-segment keys (e.g., `decision`) continue to work as flat lookups -- they are trivially single-segment paths." Or alternatively, specify that gate output is flattened into dot-separated keys (e.g., `"gates.ci_check.exit_code": 0`) so the resolver doesn't need nested traversal. Pick one.

### A4: Example 2 has an internal inconsistency

Example 2's YAML shows the lint gate producing `{exit_code: number}` (from the command type schema). The transitions reference `gates.lint.exit_code: 0`. But the paragraph after the YAML block says "Gate output (`gates.lint.status`) and agent evidence (`decision`) coexist." The field `status` doesn't exist in the command gate's schema -- it should be `exit_code`.

This is a copy-paste artifact from an earlier revision (round 1's PRD used a `status` field). It creates confusion about whether there's an implicit `status` field or whether the narrative is wrong.

**Suggested fix:** Change `gates.lint.status` to `gates.lint.exit_code` in the narrative paragraph after Example 2's YAML.

### A5: Compiler validation of when clauses referencing gates -- warn or reject?

R9 says the compiler validates that `when` clauses referencing `gates.*` fields use valid gate names and fields from the gate type's schema. But the acceptance criteria (line 412) say the compiler *warns* on nonexistent gate or field references. R9 doesn't say "warn" -- it says "validates," which most readers interpret as "rejects."

These are different behaviors. A warning lets the template compile (maybe the author knows what they're doing). A rejection blocks compilation. The PRD should pick one.

**Suggested fix:** Align R9 and the acceptance criterion. Either R9 should say "warns" or the acceptance criterion should say "rejects."

### A6: When --override-rationale and --with-data are combined, which data drives transition resolution?

R11 specifies event ordering (evidence first, override second). R7 says both gate output and agent evidence feed into the same resolver. But the PRD doesn't specify what happens when an agent submits `--with-data '{"gates.ci_check.exit_code": 0}'` alongside `--override-rationale`. Can agent-submitted evidence collide with or overwrite gate namespace data?

R7 says gate output is namespaced under `gates.<name>` to "prevent field collisions with agent evidence." But if the evidence map is a nested structure where `gates` is a top-level key, agent-submitted data with key `gates` could overwrite the entire gate namespace. If it's flat with dot-separated keys, agent data with key `gates.ci_check.exit_code` collides directly.

D1 mentions preventing collisions but doesn't address what happens when an agent deliberately submits data in the `gates.*` namespace. Is this an error? Silently ignored? Last-write-wins?

**Suggested fix:** Add to R7 or R3: "Agent-submitted evidence with keys in the `gates.*` namespace is rejected (or: is silently dropped, or: overwrites gate output with last-write-wins). The `gates` namespace is reserved for engine-produced gate data."

### A7: Override_default for context-exists defaults to {exists: true} -- what about the key's actual value?

R4 says context-exists gates default to `{exists: true}` when overridden. But the gate only checks existence, not the value. If downstream logic depends on the context key's content (accessed via a different mechanism, not the gate output), the override makes the gate "pass" but doesn't inject the missing context key. The override makes the *routing* work but doesn't make the *state* consistent.

This is arguably correct behavior (gates are routing mechanisms, not state managers), but a template author might expect that overriding a context-exists gate somehow provides the missing context. The PRD should be explicit that override only affects routing, not the underlying context store.

**Suggested fix:** Add a sentence to R4 or Known Limitations: "Override defaults affect transition routing only. They don't modify the context store or inject missing context keys."

## Previously Resolved (from Round 1)

The following round 1 ambiguities are now addressed:

- **Pass/fail definition** -- resolved by R1's pass condition table (exit_code == 0, exists == true, matches == true).
- **Namespace serialization** -- resolved by R3 specifying nested JSON map with dot-path traversal.
- **Event ordering** -- resolved by R11 and acceptance criterion specifying EvidenceSubmitted has lower sequence number.
- **Gate type parsing responsibility** -- resolved by R2 specifying engine contains parsing logic per type.
- **Nonexistent gate in --gate flag** -- resolved by acceptance criterion explicitly stating silent ignore.
- **Schema-less gate backward compatibility** -- resolved by R10 specifying no gates.* data enters resolver without schema.

## Summary

Seven new ambiguities found. The most impactful are A1 (error output schema mismatch), A3 (dot-path traversal is an unstated new capability), and A6 (namespace collision on combined evidence+override). A4 is a simple copy-paste bug. A2 and A7 are design clarifications that could go in Known Limitations. A5 is a warn-vs-reject inconsistency between R9 and acceptance criteria.

None of these would cause the PRD to fail outright -- an experienced implementer could make reasonable choices. But A1 and A3 could lead two developers to build materially different things, and A6 is a security-adjacent concern (can agents spoof gate output?).
