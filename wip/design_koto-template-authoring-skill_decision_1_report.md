<!-- decision:start id="authoring-skill-phases-and-modes" status="assumed" -->
### Decision: Authoring skill phases and input modes

**Context**

The koto template authoring skill needs a defined phase sequence and must handle two input modes: creating a new koto-backed skill from scratch, and converting an existing prose skill to use a koto template. The skill-creator's proven workflow pattern (capture intent, draft, validate, iterate) provides the structural foundation, but koto-backed skills have distinct requirements: paired SKILL.md + template output, `koto template compile` as a mechanical validation step, and the coupling convention (koto-templates/ directory, ${CLAUDE_SKILL_DIR} references).

The shirabe koto adoption PRD identifies 7 existing skills needing conversion as the immediate use case, while new-from-scratch is a forward-looking capability. Both modes produce identical output artifacts and share most authoring steps.

**Assumptions**

- The 7 PRD conversions are the primary near-term use case. If this changes, the unified workflow still supports new-from-scratch equally -- the risk is that new-from-scratch might need richer scaffolding in the context gathering phase.
- `koto template compile` is sufficient validation for v1. If templates compile but produce bad runtime behavior, a future iteration could add eval-based validation as an optional phase.

**Chosen: Unified linear workflow with mode-conditional steps**

A single phase sequence where all phases run for both modes, with mode-specific behavior defined in directive prose rather than in the state graph. Eight phases:

1. **Entry** -- mode selection (new or convert) and basic inputs
2. **Context gathering** -- conversion: read existing SKILL.md, identify its structure; new: capture intent (what states, when to trigger, expected workflow shape)
3. **Phase identification** -- conversion: extract phases from prose, identify boilerplate vs domain logic, map resume patterns; new: derive phases from intent, determine state topology (linear, branching, fan-out)
4. **State design** -- define koto states, transitions, evidence routing, gate types, and variables; apply layered teaching (linear first, then evidence routing, then advanced features)
5. **Template drafting** -- write the YAML frontmatter and markdown directive body as a koto template source file
6. **Compile validation** -- run `koto template compile`, parse errors, fix and re-draft (self-loop); this is the primary quality gate
7. **SKILL.md authoring** -- conversion: refactor existing SKILL.md to remove workflow boilerplate, add koto init/execution loop; new: write SKILL.md from scratch with koto integration
8. **Integration check** -- verify the coupling convention: template lives in koto-templates/, SKILL.md references ${CLAUDE_SKILL_DIR}/koto-templates/<name>.md, output is a paired unit

Mode is tracked as a template variable (like work-on uses ISSUE_NUMBER). Directive content in each phase tells the agent what to do differently based on mode, rather than duplicating states. The compile validation phase self-loops on failure (up to 3 attempts) before escalating.

**Rationale**

The unified approach was chosen over four alternatives because it provides the best balance of simplicity and quality for both modes:

- It mirrors how work-on handles issue-backed vs free-form: a single state graph with mode as a variable and divergent behavior encoded in directive prose. This is a proven pattern in the codebase.
- The state graph stays compact (8 states vs 14+ for a forked approach), making the template itself a teaching example of good koto template design.
- Both modes receive equal treatment. A conversion-first approach (Alternative 4) would make new-from-scratch feel like a workaround via synthetic SKILL.md generation. A forked approach (Alternative 1) duplicates states and makes the template harder to understand.
- Compile validation has a clear, dedicated phase rather than being embedded in a coarse macro-phase (Alternative 3's "validate" grouping hides important detail).
- The self-loop at compile validation follows established koto patterns (work-on uses the same pattern at implementation and pr_creation).

**Alternatives Considered**

- **Forked entry, shared core**: Separate phase sequences for new and convert modes, converging at template drafting. Rejected because it duplicates states (14+ total), makes the template larger and harder to maintain, and doesn't match the simpler work-on pattern. The mode-specific differences are in *what agents do within each phase*, not in *which phases they visit*.
- **Phase-grouped workflow (3 macro-phases)**: Collapses everything into understand/author/validate. Rejected because it's too coarse -- compile validation, SKILL.md authoring, and integration checking need distinct states for proper gate enforcement and resumability. A 3-state template can't enforce the ordering constraints that make koto valuable.
- **Conversion-first with synthetic SKILL.md**: Optimizes for conversion and has new-from-scratch generate a synthetic SKILL.md first. Rejected because the indirection makes the new path feel second-class, and generating a synthetic SKILL.md that gets immediately parsed is unnecessary complexity. The unified approach handles both modes directly without this intermediate artifact.

**Consequences**

The authoring skill's own koto template becomes a mid-complexity example (8 states, 1 variable, mode-conditional directives, compile validation self-loop) -- useful as a teaching reference. Directive content in phases 2, 3, and 7 needs to clearly distinguish what agents should do in each mode, which puts more weight on prose quality. The compile validation self-loop means agents can iterate on structural errors without human intervention, but runtime behavior issues (templates that compile but don't work well) won't be caught until agents actually use the authored template.
<!-- decision:end -->
