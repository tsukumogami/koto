# Explore Scope: koto-template-authoring-skill

## Visibility

Public

## Topic Type: Directional

## Core Question

How should we build a skill, bundled in koto, that helps Claude Code agents author new skills whose execution is driven by koto templates? The skill-creator skill is both the guide for how we build this and the inspiration for what we produce. The output is always a paired skill + bundled template.

## Context

Koto is a workflow orchestration engine for AI coding agents. It uses markdown templates with YAML front-matter to define state machines that drive multi-step workflows. Skills are Claude Code's mechanism for packaging reusable agent behaviors.

Today there's no guided way to create a koto-backed skill. The skill-creator skill exists in another marketplace and creates generic skills, but it doesn't know about koto templates. We want a koto-native equivalent that produces both the skill definition (what to do) and the template (how to do it) as a unit.

Distribution should use whatever standard Claude Code mechanism works best. The user knows marketplaces work but finds them heavy for a single skill. A simpler option that doesn't sacrifice installability would be preferred.

## In Scope

- Studying the skill-creator skill's patterns and carrying them forward
- Defining the structure of "skill + bundled koto template" as output
- Template authoring guidance (format, states, transitions, directives)
- Distribution mechanism within Claude Code's standard options
- The skill living in the koto repo

## Out of Scope

- Decoupled templates (templates not bundled with their skill)
- Multi-template skills
- Changes to koto's template engine or runtime
- Changes to the skill-creator skill itself

## Research Leads

1. **How does the skill-creator skill work, and what patterns should we carry forward?**
   We need to study its structure, workflow phases, and output format to model our skill after it. Understanding what it does well tells us what to replicate; understanding its gaps tells us what to add.

2. **What does koto's template format look like, and what are the authoring constraints?**
   The skill needs to teach agents how to write valid templates. We need the full picture: YAML front-matter schema, state definitions, transition rules, directive format, and any validation koto performs.

3. **How do skills and koto templates couple at the file-system level?**
   What directory structure does a "skill with bundled template" actually look like? How does the skill reference its template? How does koto discover and use it?

4. **What's the simplest way to distribute a single skill via Claude Code?**
   Marketplaces work but carry overhead for a single skill. We should check whether Claude Code supports simpler distribution (direct skill install, single-skill plugins) that still allows `claude /install` style setup.

5. **What does the skill need to know about koto's state machine to guide template authoring?**
   States, transitions, directives, guards, context variables -- what concepts are essential for an agent writing a template? What's the minimal mental model?

6. **Is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)
   You are a demand-validation researcher. Investigate whether evidence supports
   pursuing this topic. Report what you found. Cite only what you found in durable
   artifacts. The verdict belongs to convergence and the user.

   ## Visibility

   Public

   Respect this visibility level. Do not include private-repo content in output
   that will appear in public-repo artifacts.

   ## Six Demand-Validation Questions

   Investigate each question. For each, report what you found and assign a
   confidence level.

   Confidence vocabulary:
   - **High**: multiple independent sources confirm (distinct issue reporters,
     maintainer-assigned labels, linked merged PRs, explicit acceptance criteria
     authored by maintainers)
   - **Medium**: one source type confirms without corroboration
   - **Low**: evidence exists but is weak (single comment, proposed solution
     cited as the problem)
   - **Absent**: searched relevant sources; found nothing

   Questions:
   1. Is demand real? Look for distinct issue reporters, explicit requests,
      maintainer acknowledgment.
   2. What do people do today instead? Look for workarounds in issues, docs,
      or code comments.
   3. Who specifically asked? Cite issue numbers, comment authors, PR
      references -- not paraphrases.
   4. What behavior change counts as success? Look for acceptance criteria,
      stated outcomes, measurable goals in issues or linked docs.
   5. Is it already built? Search the codebase and existing docs for prior
      implementations or partial work.
   6. Is it already planned? Check open issues, linked design docs, roadmap
      items, or project board entries.

   ## Calibration

   Produce a Calibration section that explicitly distinguishes:

   - **Demand not validated**: majority of questions returned absent or low
     confidence, with no positive rejection evidence. Flag the gap. Another
     round or user clarification may surface what the repo couldn't.
   - **Demand validated as absent**: positive evidence that demand doesn't exist
     or was evaluated and rejected.

   Do not conflate these two states. "I found no evidence" is not the same as
   "I found evidence it was rejected."
