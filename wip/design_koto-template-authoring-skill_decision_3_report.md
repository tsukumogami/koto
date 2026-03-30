<!-- decision:start id="authoring-skill-self-hosting" status="assumed" -->
### Decision: Should the authoring skill use a koto template as its execution engine?

**Context**

We are building a skill that helps Claude Code agents author koto-backed skills -- skills that pair a koto workflow template (state machine) with a SKILL.md (agent instructions). The question is whether this authoring skill should itself be orchestrated by a koto template (dog-fooding), or follow the prose-only pattern used by most existing shirabe skills.

Currently, 1 of 10 shirabe skills (work-on) is koto-backed. The remaining 9 (decision, design, explore, plan, prd, release, etc.) use prose-based SKILL.md files with ad-hoc resume logic. The template authoring workflow follows a clear sequence: capture intent, draft template, compile/validate, iterate on errors, generate SKILL.md, test. This maps naturally to a state machine with 4-6 states and a validation self-loop.

**Assumptions**
- `koto template compile` provides sufficient error feedback for iterative template refinement within a state loop. If it doesn't, the iteration gate would need supplementary error parsing, but the structural choice still holds.
- The authoring workflow has discrete states with clear entry/exit conditions. Research into the work-on template and the skill-creator pattern confirms this.
- v1 will be hand-written. This is inherent to any self-hosted tool and represents a one-time cost.

**Chosen: Koto-backed (self-hosted)**

The authoring skill ships with a koto template defining the full workflow. The SKILL.md instructs agents to run the standard koto execution loop (`koto init`, `koto next`, evidence submission). The template includes states for intent capture, template drafting, compilation validation (with a self-loop for iteration), SKILL.md generation, and testing. Gates enforce prerequisites -- for example, the template must compile successfully before the agent can proceed to SKILL.md drafting.

The skill's own template serves as a living, inspectable example of what it produces. Agents authoring a new skill can reference the authoring skill's template to understand structure, gate patterns, evidence schemas, and directive writing conventions.

**Rationale**

The deciding factor is that this skill teaches agents to build koto templates. A skill that uses koto to orchestrate its own workflow is a stronger pedagogical tool than one that only describes the format in prose. Three specific advantages:

1. **Living example.** The template is both the orchestration mechanism and a reference artifact. When the agent needs to understand how to write a `context-exists` gate or a self-loop transition, it can inspect the very template governing its current workflow.

2. **Engine-managed recovery.** Koto handles resume, state persistence, and evidence routing. The prose-only skills in shirabe implement these via ad-hoc markdown checks ("if wip/prefix_X exists, skip to phase Y"). For a skill whose primary output is a koto template, having the agent interact with koto throughout the authoring process builds familiarity with the tool.

3. **Validation gates.** The compilation step is a natural gate: the template must pass `koto template compile` before the workflow advances. This is cleaner as a koto gate than as a prose instruction that the agent might skip.

The bootstrapping cost (v1 must be hand-written) is real but one-time. The 9/10 prose-only precedent in shirabe reflects historical timing -- those skills were written before koto template support matured -- not a deliberate architectural preference against templates.

**Alternatives Considered**
- **Prose-only (traditional)**: A SKILL.md with sequential phase files and no koto template, matching the pattern of decision, design, explore, and other shirabe skills. Rejected because the skill would describe koto templates without using one, missing the dog-fooding opportunity. Faster to build initially, but loses the living-example advantage that differentiates this skill from generic documentation.
- **Hybrid (prose outer + koto inner)**: Prose-based SKILL.md for the overall flow with a koto template only for the draft-compile-iterate loop. Rejected because mixing two execution models in one skill adds cognitive load without proportional benefit. No existing shirabe skill uses this pattern, so it would be a novel approach with no precedent to validate it. The full koto-backed approach is simpler conceptually -- one execution model, applied consistently.

**Consequences**
- The authoring skill will take longer to build initially (template engineering + SKILL.md, vs SKILL.md only). Estimate: 1-2 additional hours for the template.
- The skill's own template becomes a maintained artifact. Changes to koto's template format may require updates to both the skill's logic and its template structure.
- Future shirabe skills that adopt koto backing will have two reference implementations: work-on (complex, 15+ states) and the authoring skill (moderate, 4-6 states). This fills a gap -- currently there's no mid-complexity example.
- v1 is hand-written. v2+ can be authored by the skill itself, completing the dog-fooding loop.
<!-- decision:end -->
