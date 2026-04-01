# PRD Feasibility Review: Gate-Transition Contract (Round 2)

**Date:** 2026-03-30  
**Status:** Critical Issues Identified — Proceed with Caution  
**Review Scope:** Phase 4, Round 2 — Focus on NEW feasibility issues from updated model

---

## Executive Summary

The PRD is **partially feasible** but requires significant architectural work to realize the structured gate output model. The current `resolve_transition` resolver uses flat equality matching and does not support dot-path traversal (Concern #1). The gate type registry is compile-time, which limits extensibility (Concern #2). Override defaults can create unreachable states if mismatch (Concern #3). Command gates deliberately discard stdout/stderr, which blocks future richer gate types (Concern #4).

**Verdict:** FEASIBLE WITH SUBSTANTIAL REWORK  
**New Concerns:** 4 blocking, 2 conditional  
**Estimated Effort:** 3-4 weeks for full implementation

---

## Detailed Analysis

### Issue 1: Dot-Path Traversal in Transition Resolver

**Status:** CRITICAL BLOCKER  
**Finding:** The resolver does NOT support dot-path traversal.

#### Current Implementation

```rust
// src/engine/advance.rs, line 411–413
let all_match = conditions
    .iter()
    .all(|(field, expected)| evidence.get(field) == Some(expected));
```

The resolver performs **flat field matching**: it looks for exact keys in the evidence BTreeMap and compares them to expected values. The `field` parameter is treated as a literal key, not a path.

#### PRD Requirement

The PRD specifies (R3, line 303–311):

```yaml
when:
  gates.ci_check.exit_code: 0  # dot-path key, nested traversal required
```

Gate output is namespaced under `{"gates": {"ci_check": {"exit_code": 0}}}` and `when` clauses must traverse this structure using dot-separated paths.

#### Gap

**The current resolver cannot traverse nested structures.** It would look for an exact key `"gates.ci_check.exit_code"` in the evidence map, which would not exist. The evidence map would have only top-level keys (agent evidence) and no `gates.*` namespace keys.

#### Implementation Required

1. **Enhance the evidence merge function** (`merge_epoch_evidence` at line 453–463) to include gate output in the merged evidence map, namespacing it under `gates.<gate_name>.<field>`.
2. **Extend the transition resolver** to support dot-path key lookups:
   - Parse `"gates.ci_check.exit_code"` into path segments `["gates", "ci_check", "exit_code"]`.
   - Navigate the nested JSON structure to retrieve the value.
   - Compare against the expected value.

#### Example Implementation Sketch

```rust
fn get_nested_value(evidence: &BTreeMap<String, serde_json::Value>, path: &str) 
    -> Option<serde_json::Value> 
{
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = evidence.get(segments[0])?.clone();
    for segment in &segments[1..] {
        current = current.get(segment)?.clone();
    }
    Some(current)
}
```

#### Feasibility: MODERATE

This is a well-defined, bounded task. The logic is straightforward, though testing must cover nested structures of arbitrary depth and ensure type safety when comparing JSON values.

**Effort:** ~1 week

---

### Issue 2: Gate Type Registry Extensibility

**Status:** DESIGN CONCERN  
**Finding:** The registry is compile-time hardcoded, limiting extensibility.

#### Current Implementation

```rust
// src/gate.rs, line 44–56
for (name, gate) in gates {
    let result = match gate.gate_type.as_str() {
        GATE_TYPE_COMMAND => evaluate_command_gate(gate, working_dir),
        GATE_TYPE_CONTEXT_EXISTS => evaluate_context_exists_gate(gate, context_store, session),
        GATE_TYPE_CONTEXT_MATCHES => {
            evaluate_context_matches_gate(gate, context_store, session)
        }
        other => GateResult::Error {
            message: format!(
                "unsupported gate type '{}'; only command, context-exists, \
                 and context-matches gates are evaluated",
                other
            ),
        },
    };
    results.insert(name.clone(), result);
}
```

The registry is a **match statement on gate type strings**. Each gate type is hardcoded as a distinct match arm with its own evaluation function.

#### PRD Requirement

The PRD envisions (D5, line 507–515) extensibility to new gate types (`json-command`, `http`, `jira`) with new output schemas and parsing logic. These are documented as "future gate types" that should extend the system without breaking existing templates.

#### Gap

**Adding new gate types requires code changes and recompilation.** There is no plugin mechanism, trait-based dispatch, or runtime type registration. The `GateResult` enum itself is hardcoded (Passed, Failed, TimedOut, Error) and does not support arbitrary structured output.

#### Trade-off Analysis

The PRD acknowledges (D5) that gate types are "documented building blocks, not hidden compiler logic," implying that the set of gate types is known and curated, not user-extensible at runtime. However, even for future built-in types, the current match statement will become unwieldy as the number of types grows.

#### Alternatives

1. **Keep compile-time dispatch (current approach):** Simple, type-safe, suitable if new gate types are added infrequently (~1–2 per year).
2. **Introduce trait objects:** Define a `GateEvaluator` trait with implementations for each type. Register instances at runtime. More flexible but introduces dynamic dispatch overhead and requires a registry mechanism.
3. **Code generation:** Use a proc macro or build-time script to generate the match statement from documented gate type definitions. Scales well for many types.

#### Feasibility: HIGH (if accepting current design intent)

The current approach is **reasonable if the PRD scope is limited to documented, built-in gate types.** The problem statement (line 515) suggests this is the intent: "Template authors pick a gate type, reference its documented fields in `when` clauses..."

However, if the vision includes user-defined or third-party gate types, the registry pattern must change.

**Recommendation:** Document that gate types are built-in and curated. If extensibility beyond built-in types becomes a requirement, revisit this design.

**Effort (if no change needed):** 0  
**Effort (if adding 3–5 new built-in types):** ~2 weeks (per type)

---

### Issue 3: Override Defaults and Reachability Validation

**Status:** CONCERN — POTENTIAL DEAD-END STATES  
**Finding:** The PRD does not address mismatch between override defaults and transition conditions, creating possible unreachable states.

#### PRD Language

**R4** (line 313–319): Each gate declares an `override_default`. "The compiler validates that override defaults match the gate type's schema and satisfy the gate type's built-in pass condition."

**R9** (line 359–360): "When override defaults are applied to all failing gates, at least one transition resolves (no dead ends on override)."

#### Scenario: Mismatch

```yaml
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "run-ci"
        override_default: {exit_code: 0}  # Default override says "pass"
    transitions:
      - target: deploy
        when:
          gates.ci_check.exit_code: 0
      - target: fix
        when:
          gates.ci_check.exit_code: 1
```

If the CI fails (`exit_code: 1`) and the agent overrides with the default (`exit_code: 0`), the transition resolver would match `gates.ci_check.exit_code: 0` and advance to `deploy`. This is correct.

But consider a different template:

```yaml
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "run-ci"
        override_default: {exit_code: 0}  # "pass" on override
    transitions:
      - target: fix
        when:
          gates.ci_check.exit_code: 1  # Only transition for failure
```

If CI fails (`exit_code: 1`) and the agent overrides with the default (`exit_code: 0`), the transition resolver would look for a transition matching `exit_code: 0`. There is none. The state would **remain blocked** even after override, because the override default doesn't match any transition condition.

#### Current Validation

The compiler validation (src/template/types.rs, line 150–297) checks:
- Gate types are recognized (line 190–236).
- Transition targets exist (line 179–186).
- Accept field schemas are valid (line 238–254).
- Evidence routing is validated (line 256–257, not shown in excerpt).

**It does NOT validate** that when all failing gates are overridden with their defaults, at least one transition matches.

#### PRD Requirement

**R9** (line 359–360) explicitly requires this: "When override defaults are applied to all failing gates, at least one transition resolves (no dead ends on override)."

#### Gap

**The compiler cannot fully validate R9 for non-enum fields.** The PRD acknowledges this (Known limitations, line 474–477):

> "Compiler reachability is limited to enum fields. The R9 reachability check (verifying override defaults lead to a valid transition) can only be done statically for enum-typed fields where all possible values are known. For numeric or string fields, the compiler can verify type compatibility but not whether a specific value matches a `when` condition. Reachability validation is best-effort for non-enum fields."

However, the PRD still includes **R9** as a requirement without explicitly gating it to enum-only fields. This creates ambiguity: is the compiler expected to validate enum-based reachability, or all fields?

#### Feasibility: CONDITIONAL

1. **For enum fields:** Feasible. The compiler knows all possible values. It can enumerate combinations and verify each leads to a valid transition.
2. **For numeric/string fields:** The compiler can warn but cannot guarantee reachability. Validation becomes best-effort.

**Recommendation:** Clarify R9 to state: "The compiler validates that when override defaults are applied to all *enum-typed* failing gates, at least one transition resolves. For numeric or string fields, the compiler warns if a field is referenced in a `when` condition but not in the override default."

**Effort (if implementing full R9 validation):** ~1.5 weeks  
**Effort (if gating to enum-only):** ~1 week (mostly documentation)

---

### Issue 4: Command Gate Output — Stdout/Stderr Discarded

**Status:** DESIGN DECISION VALIDATION  
**Finding:** Command gates capture but discard stdout and stderr, limiting future richer gate types.

#### Current Implementation

```rust
// src/action.rs, line 26–80
pub fn run_shell_command(command: &str, working_dir: &Path, timeout_secs: u32) -> CommandOutput {
    // ... spawns command, captures stdout and stderr ...
    let stdout = child.stdout.take().map(|mut s| { ... }).unwrap_or_default();
    let stderr = child.stderr.take().map(|mut s| { ... }).unwrap_or_default();
    CommandOutput { exit_code, stdout, stderr }
}
```

The function **captures and returns both stdout and stderr** in a `CommandOutput` struct.

But in `evaluate_command_gate` (src/gate.rs, line 121–141), the output is used only to extract the exit code:

```rust
fn evaluate_command_gate(gate: &Gate, working_dir: &Path) -> GateResult {
    let output = run_shell_command(&gate.command, working_dir, gate.timeout);
    
    if output.exit_code == -1 {
        // ... error handling ...
    } else if output.exit_code == 0 {
        GateResult::Passed
    } else {
        GateResult::Failed { exit_code: output.exit_code }
    }
}
```

**The stdout and stderr are discarded.** Only the exit code is returned in the `GateResult`.

#### PRD Requirement

**R1** (line 269–275) documents the initial gate types:

| Gate type | Output schema | Pass condition |
|-----------|--------------|----------------|
| command | `{exit_code: number}` | `exit_code == 0` |

The schema explicitly specifies `{exit_code: number}` — no stdout or stderr in the output.

However, the PRD envisions future gate types (line 279–282):

| Gate type (future) | Output schema | Pass condition |
|-----------|--------------|----------------|
| json-command | `{exit_code: number, data: object}` | `exit_code == 0` |

A `json-command` gate would need to **parse stdout as JSON** and include it in the output. This requires access to stdout, not just the exit code.

#### Gap

The current design deliberately **restricts** command gates to exit code only, which is appropriate for R1. But to implement future gate types (json-command, http, etc.), the evaluation pipeline must be extended to pass structured output through `GateResult`.

**This is not a blocker for the initial implementation (R1), but it is a prerequisite for future extensibility.**

#### Feasibility: HIGH

The fix is straightforward:
1. Extend `GateResult` to support structured output (e.g., `GateResult::Passed(serde_json::Value)` instead of just `GateResult::Passed`).
2. Update `evaluate_command_gate` to return exit code in a structured format.
3. Update `evaluate_context_exists_gate` to return `{exists: boolean}`.
4. Update `evaluate_context_matches_gate` to return `{matches: boolean}`.

This requires updating all call sites, but the changes are localized to gate.rs and advance.rs.

**Effort:** ~1.5 weeks (including tests)

---

## Summary of Concerns

| # | Concern | Severity | Category | Effort | Blocker |
|---|---------|----------|----------|--------|---------|
| 1 | Dot-path traversal not supported in resolver | CRITICAL | Core Logic | ~1 week | YES |
| 2 | Gate type registry is compile-time hardcoded | MODERATE | Design | 0 (if accepted) | NO |
| 3 | Override reachability validation incomplete | HIGH | Validation | ~1–1.5 weeks | CONDITIONAL |
| 4 | Command gate stdout/stderr discarded | MODERATE | Extensibility | ~1.5 weeks | NO |

---

## Implementation Roadmap

### Phase 1: Core Gate Output (Week 1–2)

1. **Extend GateResult enum** to carry structured JSON output instead of just pass/fail.
2. **Update evaluate_* functions** to return structured output:
   - `evaluate_command_gate` → `{exit_code: number}`
   - `evaluate_context_exists_gate` → `{exists: boolean}`
   - `evaluate_context_matches_gate` → `{matches: boolean}`
3. **Test all gate types** with new output format.

### Phase 2: Resolver and Evidence Integration (Week 2–3)

1. **Namespace gate output** in merged evidence: `merge_epoch_evidence` adds `gates.<name>.<field>` keys.
2. **Implement dot-path traversal** in transition resolver.
3. **Update tests** for transition resolution with gate data.

### Phase 3: Compiler Validation (Week 3–4)

1. **Implement R9 reachability validation** for enum-typed override defaults.
2. **Add compiler warnings** for numeric/string fields.
3. **Document limitations** in user-facing docs.

### Phase 4: Override Events and Audit Trail (Week 4)

1. **Implement GateOverrideRecorded event** (R6).
2. **Add `derive_overrides` query function** (R8).
3. **Test override event lifecycle** across epochs.

---

## Recommendations

1. **Clarify R9 scope:** Restrict reachability validation to enum-typed fields. Document as best-effort for numeric/string fields.

2. **Accept compile-time gate type registry:** If the vision is limited to curated built-in gate types, the current match-based design is appropriate. Document this intent clearly in user-facing specs.

3. **Prioritize Concern #1:** Dot-path traversal is the foundation for all structured gate output. Implement this early.

4. **Defer future gate types:** The `json-command` and `http` gate types are out of scope for this PRD. Plan them as separate issues with their own PRD cycle.

5. **Document gate output schemas:** Create a schema registry or documentation that template authors can reference. Each gate type should publish its output schema (e.g., "command produces {exit_code: number}").

---

## Verdict

**FEASIBLE WITH REWORK**

The PRD's vision of structured gate output feeding into transition routing is sound and implementable. However, the current codebase does not support dot-path traversal or structured gate output. The required changes are well-defined and localized:

- **Critical:** Implement dot-path traversal in the resolver (~1 week).
- **High:** Extend GateResult to carry structured output (~1.5 weeks).
- **Conditional:** Complete compiler validation for override reachability (~1–1.5 weeks, with scope clarification).

**Total Effort:** 3–4 weeks of focused development.

**Risk:** Moderate. The changes touch core transition logic and gate evaluation; comprehensive testing is essential. The override audit trail (R6, R8) is orthogonal and lower risk.

**Dependencies:** No external dependencies. All work is within the koto engine.

