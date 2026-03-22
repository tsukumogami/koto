<!-- decision:start id="directive-procedure-carrying" status="assumed" -->
### Decision: How Template Directives Carry Procedure Instructions to Agents

**Context**

The shirabe /work-on koto template must specify directive text for each of its 15 states. When an agent calls `koto next`, the directive is the only instruction it receives from koto — there is no other mechanism for the template to convey what work should happen in a state. This creates a tension: the real /work-on skill has 50-130 lines of procedure per major phase (the implementation phase is 133 lines covering the A-B-C-D cycle, coverage tracking, blocker handling, self-review, and conditional sub-agent spawning), but the template's directive fields are static strings embedded in compiled JSON.

The panel review surfaced two related problems. The skill implementer noted: "The design gives no actual directive text... For staleness_check: 'Evidence: staleness_signal: enum[...]' — this tells the implementer what fields to capture, not what the agent should actually do." The workflow practitioner added: "When koto next returns {state: implementation, directive: 'Write code and commit...'}, an agent resuming after 24 hours knows it's in the implementation state. It does NOT know which commits already exist, whether tests were passing, what approach it was taking."

Four options were evaluated: (a) self-contained directives with full embedded procedure; (b) concise action-oriented directives with resume-oriented artifact-check preambles; (c) short directives referencing SKILL.md phase files by name; (d) very short trigger directives with the skill wrapper injecting full phase context before each koto next call.

The coupling with Decision 2 (context injection) is real but bounded: whichever approach is chosen here determines the verbosity expected of the context_injection state's directive. Decision 2 already chose option (b) for that state specifically — the directive instructs the agent to run extract-context.sh, which is a concrete imperative action. That pattern is consistent with option (b) chosen here.

**Assumptions**

- The skill wrapper (shirabe's SKILL.md entry point for /work-on) controls the call to `koto next` and can inject additional context before the agent reads the directive. This is the mechanism option (d) fully relies on, and option (b) can use selectively for complex phases.
- Template variable substitution (`{{ISSUE_NUMBER}}`) is not yet implemented in koto. Directive text that references issue-specific artifact paths must use fixed paths (e.g., `wip/IMPLEMENTATION_CONTEXT.md`) or generic references (e.g., "the plan file created in the analysis phase") until --var ships.
- The directive field has no enforced size limit in the koto template format. Option (a)'s embedded procedure is technically feasible — the template file would be large but not malformed.
- The relationship between template directives and SKILL.md phase files is a design choice, not an engine constraint. koto does not know or care whether the directive is comprehensive or minimal.
- The real extract-context.sh creates `wip/IMPLEMENTATION_CONTEXT.md` (fixed path), as confirmed in Decision 2 research. Directives can reference this path statically.

**Chosen: (b) Concise action-oriented directives with resume-oriented artifact-check preambles**

Each state's directive has two parts. The first part is an action summary: 10-25 lines that describe what the agent should accomplish in this state at a level sufficient for a template author to understand the state's purpose without reading SKILL.md. The second part is a resume preamble: 3-6 lines that explicitly instruct the agent to re-read relevant wip artifacts and check git state before continuing work. The resume preamble is included for any state where prior work (artifacts, commits) affects what the agent should do next.

The directive for the implementation state would read roughly: "Implement the plan step by step, following the A-B-C-D cycle (Write, Validate, Functional Test, Write Tests) for each implementation step. After completing each step, commit with the step marked complete in the plan file. Before submitting evidence, run tests and confirm they pass. On resume: read `wip/issue_N_plan.md` to find the first unchecked step, check `git log` to see commits already made, and review `wip/IMPLEMENTATION_CONTEXT.md` for design context." The full 133-line procedure lives in the skill's phase-4-implementation.md; the wrapper injects it for first-run agents.

For procedurally simple states (ci_monitor, pr_creation, validation_exit), the directive alone is sufficient — a 10-15 line directive covers all necessary guidance. For the two most complex states (analysis, implementation), the skill wrapper supplements the directive with the full phase file content before the agent begins work. This selective injection is option (d)'s mechanism applied narrowly, not as the primary architecture.

**Rationale**

Option (c) is eliminated outright by the constraint that templates must be standalone-readable. A directive that says "Run phase-4-implementation.md" is meaningless without the skill context, and fails any author reading the template in isolation.

Option (a) solves all problems but creates a maintenance burden that's structurally worse than the problem it solves. The real skill's phase files are the authoritative source of procedure. Embedding full copies in the template means every procedural update requires synchronized edits in two places. Drift between the template directive and the SKILL.md phase file would be undetectable. The template would also be ~800+ lines of prose embedded in YAML, which makes authoring and review difficult. The hello-koto template (the reference example) is 33 lines total — a 15-state work-on template with embedded procedure would be 25x larger. The principle "templates enforce workflow structure, not replace skill procedure" is the right one.

Option (d) inverts the relationship between template and skill: the template becomes an empty scaffold and the skill wrapper carries all meaning. This breaks standalone readability more subtly than option (c) — a template author can read that the directive says "implementation state" but can't understand what happens there. More critically, if the skill wrapper is unavailable (e.g., an agent runs koto directly without the shirabe wrapper), the agent receives useless directives. The template should carry enough content to be operable, even if suboptimally, without the wrapper.

Option (b) threads these constraints correctly. The directive is concise enough to avoid maintenance duplication but substantive enough that a template author reading it understands what the state does. The resume preamble directly addresses the workflow practitioner's critique — agents resuming mid-workflow are explicitly told to re-orient before continuing. For complex phases, the skill wrapper can inject full phase context without the template needing to embed it. The directive and the injected context are complementary, not redundant: the directive orients the agent to the workflow context, the injected phase file provides the procedural detail.

The coupling to Decision 2 is consistent: Decision 2 chose a concrete imperative directive for context_injection ("run extract-context.sh, create wip/IMPLEMENTATION_CONTEXT.md"). That's option (b) applied to a simple state. The same pattern scales to complex states with longer but still concise directives.

**Alternatives Considered**

- **(a) Self-contained full procedure**: Technically sufficient. Rejected because it duplicates procedure from SKILL.md phase files into the template, creating a maintenance split where changes to how a phase works require updating both the skill's references/phases/ directory and the template. The template file would be 800+ lines, making authoring difficult and review error-prone. The principle that templates enforce structure while skills carry procedure is architecturally sounder.
- **(c) Short directives referencing SKILL.md phase files by name**: Eliminated by the standalone-readability constraint. A directive that references an external file the template author may not have access to fails the stated requirement.
- **(d) Very short trigger directives with wrapper injection**: Makes the template an empty scaffold. An agent running koto directly (without the shirabe wrapper) would receive directives with no actionable content. The template should be useful standalone even at reduced fidelity. Option (b) achieves the same procedural completeness for complex phases via selective wrapper injection, without sacrificing template readability.

**Consequences**

Template directives follow a two-part structure: action summary (what to accomplish, key artifacts, evidence schema guidance) plus resume preamble (what to re-read and check before continuing). The action summary makes the template standalone-readable. The resume preamble closes the reorientation gap the workflow practitioner identified.

For states where the action summary is insufficient for first-run agents on complex phases (analysis, implementation), the skill wrapper injects the full phase file content. This injection is the wrapper's responsibility to document and maintain, not the template's. The template directive remains concise; the wrapper adds depth.

Template maintenance becomes: update SKILL.md phase files when procedure changes, update template directives only when the summary of what a state does changes. These are different change triggers — procedural refinement vs. workflow structure changes. This separation keeps the template stable while allowing skill procedure to evolve.

The directive text gap identified by the panel is directly resolved: each state now has explicit, imperative directive text describing what the agent should do. Evidence schema fields are not directive text — the design document must clearly distinguish between "what the agent does" (directive) and "what evidence the agent submits" (accepts schema).
<!-- decision:end -->
