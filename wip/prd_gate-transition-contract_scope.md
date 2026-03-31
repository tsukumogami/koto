# /prd Scope: gate-transition-contract

## Problem Statement

Gates in koto are boolean pass/fail checks completely decoupled from transition routing. Gate results don't produce data, don't feed into `when` clauses, and don't affect which state the workflow advances to. Template authors must add `accepts` blocks with `override` enum values as a workaround just so agents can bypass failed gates. This model breaks down as gates become richer than simple command-line checks -- there's no way for a gate to return structured data that transitions can route on, and no way for overrides to specify what the gate result "should have been."

## Initial Scope

### In Scope
- Gates producing structured data (same shape as `--with-data` evidence) with per-gate schemas
- Gate output feeding into transition `when` clauses for automated routing
- Override mechanism that specifies a default gate result value per gate
- Compiler validation that gate schemas, override defaults, and transition `when` clauses form a complete contract
- Override rationale capture (the audit trail requirement from issue #108)
- Cross-epoch query surface for override events

### Out of Scope
- Visualization UI for override audit trails
- Redo/rewind triggered by override disagreement
- Evidence verification by koto (polling, parsing, embedded validation calls beyond gates)
- `--to` directed transition tracking
- Action skip tracking

## Research Leads

1. **How should gate output schemas be declared and what data do they produce?** Today gates are commands with exit codes. What does a gate schema look like when it produces structured data? How does it declare its output fields and types? How do different gate types (command, context-exists, context-matches) map to structured output?

2. **How do gate outputs and agent evidence coexist in transition resolution?** A state might have gates producing data AND an `accepts` block for agent choices. Do these merge into one evidence map? Are they namespaced? Who wins on conflict? How does the transition resolver handle both?

3. **How should override defaults work across multiple gates?** Each gate has its own schema. When an agent overrides, should each gate declare its own default override value? What does "default on override" mean for a gate with complex output (not just pass/fail)?

4. **What compiler validations are needed to ensure the contract is complete?** The compiler should verify that gate schemas, override defaults, and transition `when` clauses fit together. What specific checks ensure no dead ends, no missing override paths, no unreachable transitions?

## Coverage Notes

- The user envisions this as a unification: gate output and agent evidence become the same data flowing into the same transition resolver. This is a significant architectural shift from the current model.
- The relationship between this PRD and the existing PRD-override-gate-rationale needs to be clarified -- this likely supersedes it.
- The "override default value" concept needs pressure testing: is it per-gate, per-state, or something else?
