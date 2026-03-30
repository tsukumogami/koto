<!-- decision:start id="template-knowledge-presentation" status="assumed" -->
### Decision: Template Knowledge Presentation

**Context**

The koto template authoring skill needs to convey template format knowledge to agents so they can produce valid, well-structured templates. The format has three conceptual layers: structural basics (states, transitions, variables), evidence routing (accepts/when blocks with mutual exclusivity), and advanced features (gates, self-loops, integration tags). Template knowledge exists in multiple forms today: two design docs totaling ~1050 lines, a 550-line authoring guide written for human readers, a reference implementation (hello-koto), and a complex production template (work-on, 370 lines). The compiler validates 13+ structural rules, providing a mechanical backstop regardless of how knowledge is presented.

Two established skill families inform the approach. The skill-creator (Anthropic's official) uses progressive disclosure: SKILL.md body for workflow, references/ for schemas and agent prompts. Shirabe skills (work-on, design, decision) use references/phases/ directories with per-phase instruction files loaded on demand. Both keep SKILL.md under 500 lines.

**Assumptions**

- Agents benefit more from pattern-matching against annotated examples than from reading specification prose. If wrong, the condensed guide compensates by making rules explicit.
- The 500-line SKILL.md guideline applies to this skill. If it needs to be longer, the progressive disclosure structure still works.
- The compiler's error messages are clear enough for agents to self-correct when a template fails validation. If wrong, the guide should map common compiler errors to fixes.

**Chosen: Condensed authoring guide plus graded example templates**

The skill bundles two types of reference material in its references/ directory:

1. **A condensed template format guide** (~200-250 lines) covering the YAML frontmatter schema, state/transition declarations, the accepts/when evidence routing system, gate types (command, context-exists, context-matches), variables, and the mutual exclusivity validation constraint. Organized by the three conceptual layers (structure, evidence routing, advanced) so agents can stop reading after the layer matching their target complexity. Each section includes a minimal YAML snippet showing the correct syntax. The guide extracts only authoring-relevant rules from the design docs, omitting rationale, alternatives considered, Go types, and implementation details.

2. **Two or three graded example templates** at increasing complexity: (a) a linear 3-state workflow with variables only, (b) a medium workflow with accepts/when evidence routing and enum types, and (c) a complex workflow with command gates, context-aware gates, self-loops for retry, and split topology. Each example has a brief header naming the concepts it demonstrates. Agents pick the closest example to their target complexity and adapt.

The SKILL.md body covers the authoring workflow (capture intent, select complexity tier, draft template, compile, iterate, write SKILL.md) and directs agents to read the guide for format rules and choose an example for pattern matching. The SKILL.md does not embed format knowledge beyond a high-level overview.

**Rationale**

The condensed guide addresses the core constraint: agents need to understand evidence routing mutual exclusivity and other non-obvious validation rules that can't be learned from examples alone. The guide makes these explicit without the ~4000 tokens of rationale, alternatives, and implementation detail in the full design docs.

The graded examples address pattern matching. Agents produce better output when they can see and adapt a complete working template than when they follow rules in isolation. The difficulty progression supports the layered teaching requirement: an agent building a linear workflow reads only the first example, while one building evidence-routed workflows reads the second.

The combination costs ~2500 tokens (guide + examples), roughly 40% of the full spec approach while covering the same authoring-relevant rules. It follows the progressive disclosure pattern established by skill-creator (SKILL.md + references/) and shirabe skills (main file + phase references).

The compiler provides the final safety net. Since `koto template compile` catches all structural errors, the skill doesn't need to teach every validation rule exhaustively. It needs to teach enough that agents get close on the first draft, then iterate using compiler feedback.

**Alternatives Considered**

- **Full spec embedding**: Bundle the complete DESIGN-koto-template-format.md and DESIGN-template-evidence-routing.md as reference files (~6000 tokens). Provides complete knowledge but is wasteful: most content is design rationale and alternatives that don't help authoring. Agents must extract authoring rules from design prose. No teaching structure -- organized by design decision, not by authoring difficulty.

- **Condensed guide only**: A purpose-built reference file without examples (~1500 tokens). Lightest context cost. Teaches rules but doesn't show them in the context of a complete template. Agents may struggle to apply rules correctly without seeing the full YAML structure of a real template. Risky for the evidence routing layer where the interaction between accepts, when, and transitions is easier to understand from a working example.

- **Examples only**: Bundle 3-4 annotated example templates with no spec reference (~1000 tokens). Strong pattern matching but doesn't explicitly teach validation rules or the mutual exclusivity constraint. Agents may produce templates that look structurally correct but fail compilation on non-obvious rules. Adequate for simple templates but insufficient for the evidence routing layer.

- **SKILL.md inline**: Embed all template knowledge in the SKILL.md body with no references/ files. Simplest structure but pushes SKILL.md to 500-700 lines, violates the progressive disclosure pattern, and loads all knowledge every time the skill triggers even when not needed.

**Consequences**

- The condensed guide must be maintained alongside the design docs. When the template format evolves, both sources need updates. This is the same maintenance pattern the authoring guide (custom-skill-authoring.md) already has.
- Agents producing simple linear templates will read ~1500 tokens of reference material (guide basics + first example). Agents producing complex templates will read the full ~2500 tokens. This scales with task complexity.
- The compiler remains the authoritative validator. The skill teaches "how to get close" and the compiler teaches "what to fix."
<!-- decision:end -->
