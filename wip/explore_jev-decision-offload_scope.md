# Explore Scope: jev-decision-offload

## Visibility

Public

## Core Question

Jev is a hosted "System 1" decision model: it takes a state payload (up to 32k
tokens) plus a set of typed questions (choice, score, noul) and returns
calibrated probabilities in ~100-300 ms at a fraction of a cent, with no text
generation. How should koto and shirabe evolve so that decisions a classifier
can make are made inside `koto next` when a Jev API key is available, without
the calling agent ever seeing them, and how should shirabe's workflows be
reshaped so more of their decisions become classifier-shaped?

## Context

The user sees two opportunities that are probably one change viewed from two
sides: (1) koto gains an embedded classifier so that, given a key, it resolves
eligible branch decisions itself during `koto next`; (2) shirabe's workflow
templates are reshaped to expose decisions in a form that capability can
consume, moving them off the main agent thread.

Key properties of Jev from the source document: typed primitives only (choice
over 2-255 keys with probability distribution and confidence; score over 2-10
anchored levels with expected value; noul returning P(true)); multiple
questions evaluated in parallel in one request; closed-world schemas that need
explicit escape-hatch options; no arithmetic, counting, or temporal reasoning;
accuracy drops with cluttered context; vulnerable to semantic injection from
text in the state; roughly 91.5% agreement with a frontier judge on rubric
checks; recommended routing by confidence bands (auto-execute above ~0.85,
fall back below). Wire format: `POST https://api.typesafe.ai/v1/systemone`
with `model`, `state`, `questions`; auth via bearer token.

Full source text is available to research agents at
`/Users/danielgazineu/.claude/jobs/62282d0d/tmp/jev-doc.md` (not committed).

Mode: interactive. The run started in `--auto`, and the user switched to interactive once round 1 was underway.

## In Scope

- koto engine changes: where a classifier would plug in, how templates declare
  classifier questions, how confidence and fallback work, what `koto next`
  returns when a decision was taken automatically
- shirabe workflow changes: which decisions move, how templates and skill prose
  change shape to take advantage
- Operational concerns: key handling, offline/no-key behavior, audit trail,
  determinism and replay, cost, testing, prompt-injection exposure
- How to measure whether classifier decisions are good enough

## Out of Scope

- Replacing generative work (drafting documents, writing code) with Jev
- Using Jev for shell-command safety gating in Claude Code itself
- Choosing between Jev and other classifier vendors in depth (noted only if it
  affects the abstraction)
- tsuku CLI, niwa, and other repos

## Research Leads

1. **Where does koto hand a decision to the calling agent today, and which extension point would a classifier use?** (lead-koto-decision-surface)
   Map the `koto next` contract, evidence submission, conditional transitions,
   and gate evaluators (`src/gate.rs`, `src/engine/`). Decide whether a
   classifier is a new gate type, an evidence provider, a transition resolver,
   or something else.

2. **How would a template declare a classifier decision, and how is its state assembled?** (lead-template-schema)
   Map template frontmatter and compile-time validation to Jev's primitives:
   question declaration, state assembly from context keys and command output,
   confidence thresholds, mandatory escape-hatch options, fallback to the agent.

3. **Which decisions in shirabe's workflows are classifier-shaped today?** (lead-shirabe-decision-inventory)
   Inventory decision points in shirabe skills and koto templates that the main
   thread makes now (routing, scoring, triage, verdicts, gate checks), and rate
   each against Jev's constraints: closed option set, bounded pruned state, no
   arithmetic or dates, tolerance for ~90% agreement.

4. **What workflow-shape changes would let shirabe move decisions into koto?** (lead-shirabe-workflow-shape)
   How shirabe skills drive koto today (templates, evidence, prose phases), and
   what pattern would turn prose judgments into declared decision states:
   gather-evidence-then-branch, pre-computed facts, splitting compound
   judgments into atomic questions.

5. **What operational constraints does an embedded network classifier impose on koto?** (lead-koto-operational-constraints)
   Does koto make network calls today? Key discovery, no-key and outage
   behavior, state-log audit and replay determinism, stability promises,
   testing without the network, cost and latency budgets, and the injection
   risk of feeding agent-written text into the state.

6. **How would we know classifier decisions are good enough, and roll them out safely?** (lead-evaluation-rollout)
   Can koto's session logs serve as a labeled dataset of past agent decisions?
   What would shadow mode, per-decision confidence thresholds, and concordance
   tracking look like?

7. **What is the strongest case for not doing this?** (lead-devils-advocate)
   Added by the user during round 1. Argue against embedding a hosted
   classifier: are the savings real, does the reshaping alone capture the
   value, what does a ~9% disagreement rate compound to, vendor maturity,
   audience fit for an OSS tool, and cheaper alternatives.
