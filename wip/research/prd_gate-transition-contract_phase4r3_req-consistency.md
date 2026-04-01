# PRD Gate-Transition Contract: Requirements & AC Consistency Review

Phase 4 Round 3 -- Requirements consistency analysis.

## Methodology

Checked all requirements (R1-R12) and all acceptance criteria for:
- Stale references to old flag-only override model
- AC-to-requirement traceability
- Numbering gaps
- Cross-requirement contradictions
- Non-functional requirement validity under the command model

## Inconsistencies Found

### 1. R10 references `output_schema` -- a concept that doesn't exist in R1-R4

**Location:** R10 (line 379-388)

R10 says: "Existing templates without `output_schema` continue to compile and run" and "The transition resolver only receives `gates.*` namespaced data when the gate declares an `output_schema`."

But nowhere in R1-R4 or the template YAML examples is `output_schema` a declared template-level field. R1 defines schemas as properties of the *gate type* (documented building blocks), not as something template authors declare. The interaction examples show gates declared with just `type:` and optionally `override_default:`. The "Out of scope" section explicitly says "Template authors can't declare custom output schemas for existing gate types."

R10 appears to reference an earlier design where `output_schema` was a per-gate template field. It should reference the gate *type* having a schema instead -- the backward compatibility hinge should be whether the template uses `gates.*` references in `when` clauses, not whether it declares `output_schema`.

### 2. R10 references `gate_failed` flag -- undefined anywhere else

**Location:** R10 (line 381)

R10 says: "the `gate_failed` flag controls transition resolution." This `gate_failed` flag isn't defined in any requirement, isn't mentioned in any interaction example, and isn't referenced by any acceptance criterion. It appears to describe the *current* engine behavior being preserved for backward compatibility, but since it's used as a normative term in a requirement, it should either be defined (even briefly) or replaced with a description of current behavior.

### 3. Duplicate AC for compiler schema validation

**Location:** AC lines 406-407 and 419-420

Two acceptance criteria say the same thing:
- "Compiler rejects `override_default` that doesn't match the gate type's schema" (line 406-407)
- "Compiler rejects templates where `override_default` doesn't match the gate type's schema" (line 419-420)

These are identical checks phrased differently. One should be removed.

### 4. R11 event ordering assumes flag-on-next model only

**Location:** R11 (line 394-395)

R11 says: "When `--override-rationale` is combined with `--with-data`, `EvidenceSubmitted` and `GateOverrideRecorded` are emitted in strict sequence within the same invocation."

After D7, override can also be a separate `koto override` command. R11 only covers the flag path (`koto next --override-rationale --with-data`). It doesn't address the command path: what happens if an agent calls `koto override` and then `koto next --with-data`? In that case the events are in *separate* invocations with their own sequence numbers. R11's ordering guarantee is still valid for the flag path, but it's incomplete -- it should acknowledge it only applies to the `koto next` shorthand path, or explicitly state that cross-invocation ordering follows natural sequence numbering.

The corresponding AC (line 452-454) also only covers the flag path: "When `--override-rationale` is combined with `--with-data`." This is consistent with R11 but shares the same gap.

### 5. No AC traces to R1 (gate types as reusable building blocks)

**Location:** R1 (lines 263-296)

R1 defines the gate type registry concept, the initial gate types table, the error/timeout behavior, and future gate types. Several ACs cover individual gate type outputs (command, context-exists, context-matches at lines 446-462), and AC for timeout/error behavior exists (lines 457-462). However, no AC covers the core R1 concept that gate types are a *registry* of *reusable building blocks* -- there's no AC for "a new gate type can be registered with its own schema and parsing logic" or any validation that the registry pattern works. R1 is part functional requirement, part architecture description. The architecture aspect has no testable AC.

### 6. No AC traces to R10 (backward compatibility beyond compilation)

**Location:** R10 (lines 379-388)

R10 makes several claims: (a) existing templates compile and run, (b) gates without schemas behave as boolean pass/fail, (c) no `gates.*` data enters the resolver for legacy gates, (d) compiler warns but doesn't error, (e) `accepts` block workaround still works.

Only one AC covers R10: "Existing templates compile and run without changes (backward compatible)" (line 425). This covers claim (a) but not (b)-(e). Notably missing:
- No AC for the compiler *warning* about gates without schemas
- No AC for legacy gates *not* injecting `gates.*` data into the resolver
- No AC for the `accepts` block workaround continuing to function

### 7. R5 flag path says `--override-rationale` but R5 command path says `--rationale`

**Location:** R5 (lines 329-341)

The command form uses `--rationale`: `koto override <name> --gate <name> --rationale "reason"`
The flag form uses `--override-rationale`: `koto next <name> --override-rationale "reason"`

This is likely intentional (shorter flag name on the dedicated command), but it creates a naming inconsistency. The ACs at lines 433-434 use `--rationale` for the command form and `--override-rationale` for the flag form, matching R5. However, R6 (line 349) only mentions the concept generically ("the rationale string") without specifying which flag name. R12 (line 397) only references `--override-rationale`. If R12's size limit applies to the rationale, it should apply to both `--rationale` (command) and `--override-rationale` (flag), but the requirement only names one.

### 8. AC for `koto override` read-back mechanism is underspecified relative to requirements

**Location:** AC lines 435-436

The AC says: "After `koto override`, a subsequent `koto next` reads the override event and substitutes gate defaults during gate evaluation."

But no requirement explicitly defines this read-back mechanism. R5 says the command "appends a `GateOverrideRecorded` event without advancing" and R6 describes the event contents. Neither requirement says how `koto next` should *consume* previously-appended override events. The AC introduces behavior (reading override events from the log and substituting defaults) that isn't grounded in any numbered requirement. This should either be folded into R5 or given its own requirement number.

### 9. R3 example uses `status: "passed"` but gate types don't produce a `status` field

**Location:** R3 (lines 313-316)

R3 gives this example namespace: `{"gates": {"ci_check": {"status": "passed"}}}` and the corresponding YAML: `gates.ci_check.status: passed`. But per R1's gate type table, the `command` gate produces `{exit_code: number, error: string}` -- there is no `status` field. The example should use `exit_code: 0` to match the gate type schema defined in R1.

### 10. AC `--override-rationale` on non-blocked state says "no-op" -- contradicts command model

**Location:** AC line 442

The AC says: "`--override-rationale` on a non-blocked state is a no-op." This makes sense for the flag path (`koto next --override-rationale` on a state where all gates pass -- nothing to override, just advance normally). But for the command path (`koto override` on a non-blocked state), should it also be a no-op, or should it error? The AC only uses the flag syntax. There's no corresponding AC for `koto override` targeting a non-blocked state.

### 11. No AC for `koto override` without `--gate` flag

**Location:** R5 + R5a (lines 329-347)

R5 says both paths accept `--gate <name>` (repeatable, optional) and "When `--gate` is omitted, all failing gates are overridden." The ACs at lines 429 and 215-218 cover the flag-on-next case (`--override-rationale` without `--gate`). But there's no AC specifically for `koto override <name> --rationale "reason"` (no `--gate`), verifying it overrides all failing gates via the command path.

### 12. R9 compiler validation doesn't mention `--gate` or command model

**Location:** R9 (lines 368-377)

R9's compiler checks are about template-time validation (schemas, override defaults, transition reachability). These are unaffected by the command vs. flag distinction since both are runtime concerns. This is *not* an inconsistency -- R9 is correctly scoped to compile-time. Noted for completeness: no issue here.

## Summary

| # | Severity | Location | Issue |
|---|----------|----------|-------|
| 1 | High | R10 | References `output_schema` template field that doesn't exist in the design |
| 2 | Medium | R10 | References undefined `gate_failed` flag |
| 3 | Low | ACs | Duplicate AC for compiler schema rejection |
| 4 | Medium | R11 | Event ordering only covers flag path, not command path |
| 5 | Low | R1 | Registry/extensibility aspect of R1 has no AC |
| 6 | Medium | R10 | Only 1 of 5 backward-compat claims has an AC |
| 7 | Medium | R5/R12 | `--rationale` vs `--override-rationale` naming; R12 size limit only names one |
| 8 | Medium | ACs | Override read-back mechanism in AC has no backing requirement |
| 9 | High | R3 | Example uses `status: "passed"` but gate type schema has no `status` field |
| 10 | Low | ACs | "no-op on non-blocked" AC doesn't cover command path |
| 11 | Low | ACs | No AC for `koto override` without `--gate` (override-all via command) |
