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
decision: |
  Build a self-hosted koto-backed skill (koto-author) within the existing koto-skills
  plugin. The skill uses a unified 8-state koto template to orchestrate authoring for
  both new and conversion modes. Reference material consists of a condensed format guide
  and graded example templates. A compile validation self-loop enforces structural
  correctness before advancing.
rationale: |
  Self-hosting means the skill's own template is a living, inspectable example of what
  it produces. The unified workflow keeps the template compact (8 states vs 14+ for a
  forked approach). The condensed guide plus examples balances knowledge completeness
  with context efficiency. The compile validation gate replaces prose-only instructions
  that agents can skip, addressing the exact failure mode the shirabe PRD identified.
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

## Solution Architecture

### Overview

The authoring skill is a new skill within the existing `koto-skills` plugin at
`plugins/koto-skills/`. It consists of a SKILL.md (workflow entry point and koto
execution loop), a koto template (8-state workflow definition), and reference
material (condensed format guide + graded examples). When invoked, it uses koto
to drive the agent through a structured authoring workflow, producing a new skill
directory as output.

### Components

```
plugins/koto-skills/
  .claude-plugin/
    plugin.json                              # Updated: add koto-author entry
  skills/
    hello-koto/                              # Existing skill
    koto-author/                            # New: the authoring skill
      SKILL.md                               # Workflow entry, koto execution loop
      koto-templates/
        koto-author.md                      # 8-state koto template (the skill's own)
      references/
        template-format.md                   # Condensed authoring guide (~200-250 lines)
        examples/
          linear-workflow.md                 # Graded example: 3 states, variables only
          evidence-routing-workflow.md        # Graded example: accepts/when, enum types
          complex-workflow.md                 # Graded example: gates, self-loops, split
```

### Key interfaces

**Input (via koto init-time variable):**
- `MODE`: "new" or "convert" -- set at `koto init` via `--var MODE=new`, determines
  directive behavior in each phase. This is an init-time variable (not runtime evidence),
  so the template's state graph doesn't branch on mode -- directives use `{{MODE}}` to
  conditionally instruct the agent.

**Output (produced by the agent during the workflow):**
A new skill directory at a user-specified location:
```
<target-plugin>/skills/<skill-name>/
  SKILL.md
  koto-templates/<skill-name>.md
  references/                               # Optional, if the skill needs them
```

**Koto integration:**
The SKILL.md instructs agents to run:
1. `koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md`
2. `koto next` to read the current directive and expected evidence schema
3. `koto next --with-data '{"field": "value"}'` to submit evidence and advance
4. Loop until the workflow reaches the done state

**Compile validation gate:**
The template's compile_validation state has a self-loop transition. The agent runs
`koto template compile <drafted-template>` and submits the result as evidence.
If compilation fails, the transition loops back to compile_validation. On success,
it advances to skill_authoring. The max-3 retry limit is enforced in the directive
prose (the agent is instructed to stop and report after 3 failures), not by a koto
engine mechanism. Koto doesn't have a built-in retry counter.

### Data flow

```
Agent invokes /koto-skills:koto-author
  |
  v
SKILL.md -> koto init (copies template to .koto/)
  |
  v
koto next -> entry state directive
  |
  v
Agent selects mode (new/convert), submits evidence
  |
  v
[context_gathering] -> [phase_identification] -> [state_design]
  |
  v
[template_drafting] -> agent writes template to target skill dir
  |
  v
[compile_validation] --fail--> [compile_validation] (self-loop, max 3)
  |                   --pass-->
  v
[skill_authoring] -> agent writes/refactors SKILL.md
  |
  v
[integration_check] -> verify coupling convention
  |
  v
[done]
```

Intermediate artifacts during authoring:
- The drafted template at `<target>/koto-templates/<name>.md`
- The SKILL.md at `<target>/SKILL.md`
- Compile output (transient, used for evidence submission)

### Template state details

| State | Gate | Evidence | Transitions |
|-------|------|----------|-------------|
| entry | none | mode: new/convert | -> context_gathering |
| context_gathering | none | context_captured: true | -> phase_identification |
| phase_identification | none | phases_identified: true | -> state_design |
| state_design | none | states_designed: true | -> template_drafting |
| template_drafting | none | template_drafted: true | -> compile_validation |
| compile_validation | context-exists: drafted template file | compile_result: pass/fail | pass -> skill_authoring, fail -> compile_validation |
| skill_authoring | none | skill_authored: true | -> integration_check |
| integration_check | context-exists: SKILL.md + template | checks_passed: true | -> done |

Note: the integration_check directive should also verify that the output directory
is within the expected target path (no path traversal) and that the template doesn't
contain command gates with unsanitized variable interpolation.
| done | none | none | (terminal) |

## Implementation Approach

### Phase 1: Reference material

Write the condensed format guide and graded example templates. These are
self-contained and don't depend on the skill or template. The format guide
should include a security note warning against variable interpolation in
command gate strings.

The existing hello-koto template can serve as the simple example (or its
pattern can be referenced). The medium evidence-routing example is the
genuinely new artifact. The SKILL.md authoring phase should reference the
existing `docs/guides/custom-skill-authoring.md` for SKILL.md structure
conventions.

Deliverables:
- `references/template-format.md` -- condensed authoring guide (with command gate security note)
- `references/examples/evidence-routing-workflow.md` -- medium example (new)
- `references/examples/complex-workflow.md` -- complex example (new)
- Reference to hello-koto as the simple example and custom-skill-authoring.md for SKILL.md conventions

### Phase 2: Koto template

Write the authoring skill's own koto template. Use the reference material from
Phase 1 to inform the directive content in each state. Validate with
`koto template compile`.

Deliverables:
- `koto-templates/koto-author.md` -- the 8-state template

### Phase 3: SKILL.md and plugin registration

Write the SKILL.md with the koto execution loop and references to the template
and guide. Update plugin.json to register the new skill.

Deliverables:
- `SKILL.md` -- skill entry point
- `.claude-plugin/plugin.json` -- updated with koto-author entry

### Phase 4: End-to-end test

Use the skill to author a test skill (new mode). Verify the output compiles and
follows the coupling convention. Then test convert mode against a simple prose
skill.

Deliverables:
- Verified working skill for both input modes

## Consequences

### Positive

- Agents get a structured, repeatable path to authoring koto-backed skills instead
  of manual template writing
- The compile validation self-loop catches structural errors mechanically, reducing
  the chance of broken templates reaching production
- The skill's own template serves as a mid-complexity reference (8 states), filling
  the gap between hello-koto (trivial) and work-on (15+ states)
- The 7 shirabe skill conversions from the adoption PRD have a guided workflow

### Negative

- v1 must be hand-written (bootstrapping cost), can't use the skill to build itself
- The condensed format guide needs maintenance alongside the template format design
  docs -- when the format evolves, two sources need updates
- Mode-conditional directives put prose quality burden on the template author -- if
  mode-specific instructions are unclear, agents may do the wrong thing for their mode
- The skill can't catch runtime behavior issues -- templates that compile but produce
  poor workflows won't be flagged until someone actually runs them

### Mitigations

- Bootstrapping is a one-time cost; v2+ can be self-authored
- The format guide is ~200-250 lines, small enough that maintenance is manageable
- Graded examples complement the guide, reducing dependence on directive prose quality
- The compiler catches the most common and most dangerous errors (structural issues,
  evidence routing conflicts); runtime quality improves as agents gain experience

## Security Considerations

This design has a minimal attack surface. The skill reads and writes local markdown
files and runs a trusted local binary (`koto template compile`) -- both standard
agent behaviors within the Claude Code sandbox.

All four security dimensions were assessed:

- **External artifact handling**: the skill doesn't download or execute external inputs.
  In convert mode it reads existing SKILL.md files, which could theoretically contain
  adversarial content that influences template drafting. This is a general LLM concern,
  not specific to this design, and the `koto template compile` gate structurally
  validates all output regardless of how it was influenced.
- **Permission scope**: standard filesystem read/write within the working directory.
  No elevated permissions, network access, or process spawning beyond the compiler.
- **Supply chain**: no external dependencies. The skill, template, and reference
  material are all bundled in the plugin.
- **Data exposure**: no user data is transmitted. All artifacts stay local.

One concern surfaced during review: koto's command gate implementation performs
`{{VARIABLE}}` substitution in command strings before passing them to `sh -c`.
Templates with command gates that interpolate user-supplied values could enable
shell injection. This is a pre-existing koto concern (not introduced by this
design), but the authoring skill amplifies it by teaching agents to write command
gates. The condensed format guide should explicitly warn about this and recommend
`context-exists` gates over command gates when checking for user-supplied paths.

Produced templates should be reviewed by a human before deployment. The compiler
validates structure but not intent -- a structurally valid template with a
malicious command gate passes compilation.
