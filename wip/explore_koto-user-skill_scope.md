# Explore Scope: koto-user-skill

## Visibility

Public

## Core Question

We have a `koto-author` skill for agents who write koto templates, but no
equivalent skill for agents who run koto-backed workflows. The two personas
have different knowledge needs and may warrant separate plugins. We also need
a structural mechanism — beyond a manual CLAUDE.md note — to keep both skills
current as koto's capabilities evolve.

## Context

`koto-skills` plugin lives at `plugins/koto-skills/` in the koto repo and
contains one skill: `koto-author`. The gate-transition roadmap (structured gate
output, overrides, compiler validation, backward compat) was fully implemented
in PRs #120–#125 without any corresponding skill update — demonstrating real
drift risk. The user uses Claude Code exclusively, so CLAUDE.md-based guidance
is on the table. CI-based enforcement (file heuristics or LLM-in-test) is also
under consideration.

## In Scope

- Design of the `koto-user` skill: content areas, structure, examples
- Plugin placement decision: same `koto-skills` plugin or separate
- Freshness mechanism: how to ensure skills track koto capability changes
- Whether integration tests can encode skill coverage assertions

## Out of Scope

- Updating koto-author content (the audit is evidence-gathering; content edits
  come after this exploration)
- koto-author skill structural redesign
- Changes to koto CLI behavior

## Research Leads

1. **What does the koto-user persona actually need to know to run a workflow successfully?**
   Map the agent journey from `koto init` through `koto next`, evidence submission,
   gate blocking, overrides, and rewind. This defines the skill's required content
   and scope.

2. **How is the koto-skills plugin structured, and would a second skill coexist cleanly?**
   Read `plugins/koto-skills/` — plugin manifest, discovery mechanism, how
   `koto-author` is installed and invoked. Determine whether adding `koto-user`
   to the same plugin is mechanical or requires structural changes.

3. **What specific koto runtime behavior is missing from koto-author after the gate-transition roadmap?**
   Audit `plugins/koto-skills/skills/koto-author/` against the current compiler
   and runtime: structured gate output, `gates.*` routing, `override_default`,
   `blocking_conditions` fields, `--allow-legacy-gates`. Report specific files and
   missing sections.

4. **What enforcement mechanisms could keep skills fresh automatically?**
   Evaluate: (a) CLAUDE.md trigger list (manual reminder, already added),
   (b) CI file-change heuristics (if `src/engine/advance.rs` changed, require
   skill file update), (c) LLM-in-test (call Claude in CI with skill content +
   koto source, ask "is this accurate?"), (d) structured coverage tests (assert
   that specific koto concepts are mentioned in skill files). For each: feasibility,
   cost, false-positive rate, maintenance burden.

5. **Is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)

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

   Questions:
   1. Is demand real? Are there issues, comments, or commit messages indicating
      agents struggle to use koto-backed workflows without guidance?
   2. What do people do today instead? Do agents read raw source, rely on inline
      koto --help, or fail silently?
   3. Who specifically asked? Cite issue numbers, commit messages, or PR references.
   4. What behavior change counts as success? What would a well-guided agent do
      differently with the skill than without it?
   5. Is it already built? Is there an existing koto-user skill, user guide, or
      equivalent reference that agents could use?
   6. Is it already planned? Are there open issues or roadmap items for a koto-user
      skill or agent-facing documentation?
