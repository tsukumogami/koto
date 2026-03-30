---
status: Proposed
problem: |
  There's no guided way to create a koto-backed skill. Template authoring requires
  understanding the YAML frontmatter schema, state machine semantics, evidence routing
  constraints, and the file-system coupling convention (koto-templates/ directory,
  ${CLAUDE_SKILL_DIR} references, SHA-256 hash-lock). Today this is done manually by
  reading docs and existing templates. A skill that guides Claude Code agents through
  authoring both the SKILL.md and its bundled koto template would lower the barrier
  and produce more consistent output.
---

# DESIGN: Koto template authoring skill

## Status

Proposed

## Context and problem statement

Koto is a workflow orchestration engine that uses markdown templates with YAML
frontmatter to define state machines. Skills are Claude Code's mechanism for
packaging reusable agent behaviors. Several skills already use koto templates as
their execution engine (e.g., work-on, explore), but creating new koto-backed
skills is a manual process that requires understanding multiple systems:

- The koto template format (states, transitions, evidence routing, gates, variables)
- The skill format (SKILL.md structure, reference files, agent prompts)
- The coupling convention (koto-templates/ subdirectory, ${CLAUDE_SKILL_DIR} paths)
- The distribution model (marketplace plugins)
- Compile-time validation rules (13+ checks including evidence routing mutual exclusivity)

The skill-creator skill exists for authoring generic Claude Code skills, but it
doesn't know about koto templates. We need a koto-native equivalent that produces
both the skill definition (what to do) and the template (how to do it) as a paired
unit.

The shirabe koto adoption PRD (tsukumogami/shirabe#48) provides immediate context:
7 existing skills need koto template conversions. Each follows the same pattern --
extract phases from prose SKILL.md, encode as koto states with gates and transitions,
maintain separation of concerns (SKILL.md = what to achieve, template = how to get
there). The PRD documents that 60-80% of current SKILL.md line counts are duplicated
workflow boilerplate (resume logic, phase ordering, gate checks), and that this
duplication causes real failures: phase skipping, lost decisions, brittle resume.
These 7 conversions are the immediate use case for this skill.

## Decision drivers

- The skill must teach agents how to write valid koto templates, not just generate them blindly
- Template validation should use `koto template compile` as a mechanical check
- The output must follow the established coupling convention (koto-templates/ directory)
- Distribution should use koto's existing marketplace infrastructure
- The teaching approach should layer concepts: linear workflows first, then evidence routing, then advanced features
- The skill-creator's workflow pattern (capture intent, draft, validate, iterate) is proven and should be adapted
- The shirabe koto adoption PRD identifies 7 skills needing conversion, establishing concrete patterns: gate selection (7+ types), fan-out collection (parallel agents + glob gate), graceful degradation (skills must work without koto), and phase decomposition from existing prose
- The skill should be aware of koto's evolving feature surface -- templates shouldn't depend on unshipped features without flagging it

## Decisions already made

These choices were settled during exploration and should be treated as constraints:

- **Validation via `koto template compile`, not eval agents**: the compiler is deterministic and catches structural errors. Agent-based evaluation (like the skill-creator's grader) adds complexity for uncertain benefit in v1.
- **Distribution via koto's existing marketplace as a new plugin**: infrastructure exists. Standalone SKILL.md files lack versioning and discoverability.
- **Layered teaching approach**: linear workflows -> evidence routing -> advanced features (gates, variables, integration). Evidence routing mutual exclusivity is the key non-obvious constraint.
- **Templates always bundled with skills**: the output is always a paired SKILL.md + koto-templates/<name>.md. Decoupled templates are out of scope.

## Considered Options

### Decision 1: Workflow phases and input modes

The skill needs a defined phase sequence that handles two input modes: creating a new
koto-backed skill from scratch, and converting an existing prose skill to use a koto
template. The shirabe koto adoption PRD identifies 7 existing skills needing conversion
as the immediate use case, while new-from-scratch is a forward-looking capability. Both
modes produce identical output artifacts (paired SKILL.md + template) and share most
authoring steps.

Key assumptions: the 7 PRD conversions are the primary near-term use case, and
`koto template compile` is sufficient validation for v1.

#### Chosen: Unified linear workflow with mode-conditional steps

A single 8-phase sequence where all phases run for both modes, with mode-specific
behavior defined in directive prose rather than the state graph:

1. **Entry** -- mode selection (new or convert) and basic inputs
2. **Context gathering** -- conversion: read existing SKILL.md, identify structure;
   new: capture intent (states, triggers, workflow shape)
3. **Phase identification** -- conversion: extract phases from prose, identify boilerplate
   vs domain logic, map resume patterns; new: derive phases from intent, determine state
   topology
4. **State design** -- define koto states, transitions, evidence routing, gate types,
   variables; apply layered teaching
5. **Template drafting** -- write YAML frontmatter and markdown directive body
6. **Compile validation** -- run `koto template compile`, parse errors, fix and re-draft
   (self-loop, max 3 attempts)
7. **SKILL.md authoring** -- conversion: refactor to remove workflow boilerplate, add koto
   integration; new: write from scratch with koto execution loop
8. **Integration check** -- verify coupling convention: template in koto-templates/,
   SKILL.md references ${CLAUDE_SKILL_DIR}/koto-templates/<name>.md

Mode is tracked as a template variable. Directive content in each phase tells the agent
what to do differently based on mode. The compile validation phase self-loops on failure.

#### Alternatives considered

**Forked entry, shared core**: separate phase sequences for new and convert modes,
converging at template drafting. Rejected because it duplicates states (14+ total),
makes the template larger and harder to maintain. Mode-specific differences are in
what agents do within each phase, not which phases they visit.

**Phase-grouped workflow (3 macro-phases)**: collapses into understand/author/validate.
Rejected because it's too coarse -- compile validation, SKILL.md authoring, and
integration checking need distinct states for gate enforcement and resumability.

**Conversion-first with synthetic SKILL.md**: optimizes for conversion, has
new-from-scratch generate a synthetic SKILL.md first. Rejected because the indirection
makes the new path feel second-class.

### Decision 2: Reference material and teaching strategy

The skill needs to convey template format knowledge so agents produce valid,
well-structured templates. The format has three conceptual layers: structural basics
(states, transitions, variables), evidence routing (accepts/when blocks with mutual
exclusivity), and advanced features (gates, self-loops, integration tags). The compiler
validates 13+ structural rules, providing a mechanical backstop.

Key assumptions: agents benefit more from pattern-matching against annotated examples
than from specification prose, and the compiler's error messages are clear enough for
self-correction.

#### Chosen: Condensed authoring guide plus graded example templates

Two types of reference material in the skill's references/ directory:

1. **A condensed template format guide** (~200-250 lines) covering YAML frontmatter
   schema, state/transition declarations, accepts/when evidence routing, gate types,
   variables, and the mutual exclusivity constraint. Organized by the three conceptual
   layers so agents can stop reading after the layer matching their target complexity.
   Each section includes a minimal YAML snippet.

2. **Two or three graded example templates** at increasing complexity: (a) a linear
   3-state workflow with variables only, (b) a medium workflow with accepts/when evidence
   routing, (c) a complex workflow with command gates, self-loops, and split topology.

The SKILL.md body covers the authoring workflow and directs agents to read the guide for
format rules and choose an example for pattern matching. It doesn't embed format
knowledge beyond a high-level overview.

#### Alternatives considered

**Full spec embedding**: bundle complete design docs (~6000 tokens). Provides complete
knowledge but is wasteful -- most content is design rationale that doesn't help authoring.

**Condensed guide only**: no examples (~1500 tokens). Teaches rules but doesn't show
them in context. Risky for evidence routing where the interaction between accepts, when,
and transitions is easier to understand from a working example.

**Examples only**: no spec reference (~1000 tokens). Strong pattern matching but doesn't
explicitly teach the mutual exclusivity constraint.

### Decision 3: Self-hosting

The question is whether the authoring skill should itself be orchestrated by a koto
template (dog-fooding), or follow the prose-only pattern used by most existing shirabe
skills. Currently 1 of 10 shirabe skills (work-on) is koto-backed.

Key assumptions: v1 will be hand-written (one-time bootstrapping cost), and the
authoring workflow has discrete states with clear entry/exit conditions.

#### Chosen: Koto-backed (self-hosted)

The authoring skill ships with its own koto template defining the 8-phase workflow from
Decision 1. The SKILL.md instructs agents to run the standard koto execution loop
(`koto init`, `koto next`, evidence submission). Gates enforce prerequisites -- the
template must compile successfully before the agent can proceed to SKILL.md drafting.

The skill's own template serves as a living, inspectable example. Agents authoring a
new skill can reference it for structure, gate patterns, evidence schemas, and directive
conventions. This creates a mid-complexity reference alongside work-on (15+ states).

#### Alternatives considered

**Prose-only (traditional)**: SKILL.md with sequential phase files and no koto template.
Rejected because the skill would describe koto templates without using one, missing the
living-example advantage.

**Hybrid (prose outer + koto inner)**: prose SKILL.md for overall flow, koto only for
the draft-compile-iterate loop. Rejected because mixing execution models adds cognitive
load without proportional benefit. No precedent validates this pattern.

## Decision Outcome

**Chosen: Unified koto-backed workflow + condensed guide + graded examples**

### Summary

The authoring skill is itself a koto-backed skill -- it uses a koto template to
orchestrate its own 8-phase workflow. This template doubles as a mid-complexity example
of good template design: 8 states, 1 mode variable (new/convert), mode-conditional
directives, and a compile-validation self-loop.

An agent invoking the skill starts at the entry state, selects a mode (new or convert),
and proceeds through context gathering, phase identification, state design, template
drafting, compile validation, SKILL.md authoring, and integration checking. Mode affects
what the agent does within each phase (e.g., context gathering reads an existing SKILL.md
in convert mode vs capturing intent in new mode), but the phase sequence is the same.

The compile validation state self-loops up to 3 times on failure: the agent submits the
drafted template to `koto template compile`, reads the errors, fixes the template, and
resubmits. A context-exists gate on the integration check state verifies that the final
output follows the coupling convention (template in koto-templates/, SKILL.md with
correct ${CLAUDE_SKILL_DIR} references).

Template format knowledge lives in references/ files: a ~200-250 line condensed guide
organized by conceptual layer (structure, evidence routing, advanced), plus 2-3 graded
example templates at increasing complexity. The SKILL.md stays lean -- it handles the
koto execution loop and points agents to reference material when needed.

### Rationale

The three decisions reinforce each other. The unified workflow keeps the template compact
(8 states instead of 14+ for a forked approach), which matters because the template is
also a teaching artifact. The condensed guide and examples provide the knowledge agents
need during the template drafting and state design phases, without bloating SKILL.md.
Self-hosting means the skill practices what it preaches -- agents learn koto patterns by
working within a koto workflow, and can inspect the governing template as a reference.

The compile validation self-loop is the linchpin: it means agents can iterate on
structural errors without human intervention, and the gate enforcement prevents
advancing past a broken template. This is cleaner as a koto gate than as a prose
instruction the agent might skip -- exactly the kind of problem the shirabe PRD
identified with prose-only skills.
