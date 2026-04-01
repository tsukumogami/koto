# PRD Review: Gate-Transition Contract -- Requirement/AC Consistency

Phase 4, Round 3 review. Focus: internal consistency between requirements and acceptance criteria after 4+ revision rounds.

## 1. Requirement-to-AC coverage

| Requirement | Covered by ACs | Notes |
|-------------|---------------|-------|
| R1 (gate types with schemas) | AC: gate produces structured data matching schema; AC: pass conditions for each type; AC: timeout/error schema shapes | Covered |
| R2 (structured evaluation) | AC: gate produces structured data; AC: command/context-exists/context-matches output shapes | Covered |
| R3 (gate output in transition routing) | AC: `when` clauses reference `gates.<name>.<field>`; AC: passing gates route without agent interaction | Covered |
| R4 (override defaults) | AC: compiler rejects bad override_default (schema mismatch, pass condition); AC: override applies defaults | Covered |
| R5 (command + flag) | AC: `koto override` appends event without advancing; AC: subsequent `koto next` reads override; AC: `koto next --override-rationale` same result as override+next; AC: multiple override calls without lock contention | Covered |
| R5a (selective override) | AC: `--gate ci_check` overrides only that gate; AC: `--gate a --gate b`; AC: without `--gate` overrides all; AC: `--gate nonexistent` ignored; AC: selective leaves state blocked; AC: targeting passed gate ignored | Covered |
| R6 (override event context) | AC: GateOverrideRecorded contains state, gate name, actual output, override default, rationale; AC: each invocation produces own event | Covered |
| R7 (gate + agent coexistence) | AC: state with both gates and accepts routes correctly; AC: `--with-data '{"gates":...}'` rejected | Covered |
| R8 (cross-epoch query) | AC: `koto overrides list` returns all events across session; AC: override events survive rewind | Covered |
| R9 (compiler validation) | AC: rejects bad schema; AC: rejects dead-end on override; AC: rejects nonexistent gate/field in `when` | Covered |
| R10 (backward compat) | AC: existing templates compile and run without changes | Covered |
| R11 (event ordering) | AC: EvidenceSubmitted has lower seq than GateOverrideRecorded (explicit R11 tag) | Covered |
| R12 (rationale size limit) | AC: 1MB limit returns validation error (explicit R12 tag) | Covered |

**No requirements lack AC coverage.**

## 2. Orphaned ACs (ACs that don't trace to a requirement)

| AC | Traces to |
|----|-----------|
| `--override-rationale ""` returns validation error | **No explicit requirement.** R5 says "rationale is mandatory (non-empty string)" but there's no numbered requirement for empty-string validation specifically. This traces to R5's description, so it's covered implicitly. Not a real orphan. |
| `--override-rationale` on a non-blocked state is a no-op | **No explicit requirement.** This is defensive behavior not stated in any R. Traces loosely to R5 (override behavior) but the no-op semantics aren't specified in any requirement text. |
| `koto next` response for gate-blocked state includes structured output in `blocking_conditions` | **No explicit requirement.** The interaction examples show this JSON shape, but no R says "the blocked response must include structured gate output." Traces to R2 (structured data) implicitly but deserves its own requirement or at minimum a note in R2. |

**3 ACs with weak/missing requirement tracing.** None are truly orphaned (they follow logically from the design), but 2 introduce behavior not stated in any requirement text.

## 3. Requirement numbering

R1, R2, R3, R4, R5, R5a, R6, R7, R8, R9, R10, R11, R12.

- No gaps (R5a is a sub-requirement of R5, acceptable)
- No duplicates
- Sequential

**No issues.**

## 4. Cross-references between requirements

**R5 references R4's override_default:** R5 says "the engine use the gate's declared default override values." R4 defines `override_default` as per-gate with compiler validation and sensible defaults when undeclared. These are consistent.

**R6 references R5 and R4:** R6 says "On each override (whether it advances the state or not)" -- this matches R5's two-path model (command doesn't advance, flag does). R6 says "applied override default" -- matches R4's definition. Consistent.

**R9 references R4:** R9 says "If `override_default` is declared, it matches the gate type's schema" and "satisfies the gate type's pass condition." R4 says the same. Consistent.

**R7 references R3:** R7 says gate output is namespaced under `gates.<gate_name>`, matching R3's namespace definition. Consistent.

**R10 references R9 implicitly:** R10 says "compiler warns about gates without schemas but doesn't error." R9 lists what the compiler validates but doesn't mention the warning for legacy gates. Minor gap -- R9 could note that legacy gates get warnings not errors, but this isn't a contradiction.

**One minor gap: R9 doesn't mention the legacy gate warning described in R10.** Not a contradiction, but R9's list of compiler behaviors is incomplete relative to R10.

## 5. R5 rewrite: flag-only model remnants

R5 was rewritten from a single `--override-rationale` flag to a two-path model: `koto override` (command) + `koto next --override-rationale` (shorthand flag).

Checking all references to `--override-rationale` and override mechanics:

| Location | Text | Issue? |
|----------|------|--------|
| User story (agent) | "override a failed gate with `--override-rationale "reason"`" | **Assumes flag-only model.** Mentions only the flag, not the `koto override` command. Doesn't mention the command path at all. |
| Interaction Example 1 | `koto next my-workflow --override-rationale "..."` | Fine -- this is the shorthand path, consistent with R5. |
| Interaction Example 4 | `koto next my-workflow --override-rationale "..."` | Fine -- shows the flag shorthand. But **no interaction example shows `koto override` as a standalone command.** |
| Interaction Example 5 | `koto overrides list` | Fine -- query path, not override invocation. |
| R5 | Describes both paths | Canonical definition, no issue. |
| R11 | "When `--override-rationale` is combined with `--with-data`" | Only mentions the flag path. For the command path (`koto override`), does R11 apply? If someone does `koto override` then `koto next --with-data`, those are separate invocations so ordering is N/A. If someone does `koto next --override-rationale --with-data`, R11 applies. **Technically correct** since R11 says "within the same invocation" and only the flag path combines both. |
| R12 | "`--override-rationale` values are subject to..." | Flag syntax, but `koto override --rationale` uses `--rationale` not `--override-rationale`. **Inconsistency:** R12 only mentions `--override-rationale` (the flag) but the command path uses `--rationale`. R12 should cover both or reference them generically. |
| AC: `--override-rationale ""` returns error | Flag syntax only. Does `koto override --rationale ""` also return an error? R5 says "rationale is mandatory" for both paths, so the behavior should be the same. **AC only tests the flag path.** |
| AC: `--override-rationale` on non-blocked state is no-op | Flag path only. What about `koto override` on a non-blocked state? R5 doesn't say. **Missing AC for command path on non-blocked state.** |
| AC: `--override-rationale` with 1MB limit | Same issue as R12 -- only tests flag path. |
| AC: `koto override` appends event | Tests command path. Good. |
| AC: `koto next --override-rationale` same as override+next | Tests equivalence. Good. |

**Key findings for R5 rewrite:**

1. **User story assumes flag-only.** The agent user story only mentions `--override-rationale`, not `koto override`. Should mention both paths or be generic.
2. **No interaction example for `koto override` command.** All override examples use the flag shorthand. The standalone command path has no worked example showing the full flow (override, then next).
3. **R12 only references `--override-rationale` flag syntax**, not `koto override --rationale`. Size limit should apply to both.
4. **ACs for edge cases (empty rationale, non-blocked state, size limit) only test the flag path.** Missing symmetric ACs for the command path.

## Summary

| Category | Issue count |
|----------|-------------|
| Requirements with no AC | 0 |
| Orphaned/weakly-traced ACs | 2 (non-blocked no-op, blocking_conditions shape) |
| Numbering issues | 0 |
| Cross-reference inconsistencies | 1 minor (R9 vs R10 legacy warning) |
| R5 rewrite remnants | 4 (user story, missing example, R12 scope, AC asymmetry) |
| **Total issues** | **7** |

## Verdict

**Needs revision.** The requirement-to-AC mapping is solid -- every R has ACs and there are no true orphans. The main problem is the R5 rewrite: the document was updated to describe two invocation paths but the user stories, examples, edge-case requirements (R12), and several ACs still assume the flag-only model. These aren't contradictions, but they're gaps that will cause confusion during implementation and test authoring. The two weakly-traced ACs should either get explicit requirement backing or be noted as defensive behavior.
