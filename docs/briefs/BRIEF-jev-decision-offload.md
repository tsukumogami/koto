---
schema: brief/v1
status: Accepted
problem: |
  Agents running koto workflows settle every branch decision themselves, even
  narrow closed-set ones over inputs koto already holds, spending turns and
  context on answers that vary between runs and carry no confidence record.
  Authors can't safely find out which decisions a cheaper decider could take.
outcome: |
  An author declares a narrow decision once. After a configured decider has
  shown it agrees with agents, agents move past that decision unasked; with no
  decider, or an unsure one, they answer as today. Maintainers see every
  automatic decision and its agreement record.
motivating_context: |
  Hosted typed decision models (Jev is the first widely available one) return
  calibrated choices, scores, and yes/no probabilities without generating
  text. An exploration of shirabe's workflows found about a dozen decisions
  that fit that shape now and about fifteen more that would after reshaping.
---

# BRIEF: Decisions koto can settle without the agent

## Status

Accepted

Framed from an exploration of koto's advance loop and shirabe's workflows.
The downstream PRD owns the requirements, including how success is measured.

## Problem Statement

When a koto workflow reaches a branch, it stops and asks the agent to choose.
That's the right call for judgments that need reading, reasoning, or writing.
But many branches are narrow: pick one of three labels for a diff summary,
decide whether an issue body describes code or docs work, say whether an
acceptance criterion names something testable. The inputs are small and
often already stored by koto, and the answer is one value from a closed set.

The agent still pays full price for each of these. It spends a turn, loads
the criteria prose (sometimes a whole reference file) into a context it
needs for the real work, and gives an answer that can differ between runs on
identical input. The workflow records which value was chosen, but not how
clear-cut the choice was. So nobody can tell a confident call from a coin
flip, or notice when a decision is routinely close.

Template authors can't act on the difference. Every decision looks the same
to koto, so an author who suspects a decision is narrow enough for a cheaper
decider has no way to find out whether that decider would get it right on
real runs, short of handing it the decision and hoping.

## User Outcome

A template author marks a decision as eligible and describes it once: what's
being decided, what each answer means, and which inputs it depends on. Agents
running that workflow on a machine with a typed decision model configured
reach the next piece of real work without ever seeing the decision. Their
context holds only what that work needs. When the decider isn't sure, or on a machine
without one, agents get the decision as an ordinary prompt with the same
answers they'd have had before, and the workflow routes identically.

Maintainers don't have to take the automatic answers on faith. Every
automatic decision is on record with its confidence. Before any answer is
allowed to act, maintainers can watch the decider answer alongside agents and
compare the two. A decision only starts settling itself once the record shows
it agrees with agents often enough, including on the uncommon answers.

## User Journeys

### A template author makes a decision eligible

A shirabe maintainer is splitting `/execute`'s upstream-drift check so the
mechanical git work runs in koto and only the judgment remains. They declare
the remaining judgment as a typed question with a description per answer, an
escape answer for "can't tell", and the stored facts it reads, then ship the
template. Users without a decider are asked the same question with the same
answers as before; the only visible change is that each answer now arrives
with its description.

### An agent passes a decision without being asked

An agent drives `/work-on` on a machine where the maintainer has configured a
decider. The workflow reaches the state that classifies the issue as code,
docs, or task work. The decider answers with high confidence, koto records
the answer and advances, and the agent's next instruction is the
implementation step. The agent never loads the classification criteria or
spends a turn on the choice.

### A maintainer decides whether to trust a decision

A koto maintainer has run a new eligible decision in shadow mode for a few
weeks: the decider answers, agents still decide, and both answers are
logged. They read the agreement record for that decision, see that it
matches agents on the common answer but misses too many of the rare one, and
enable automatic settling only for the common answer, leaving the rare one
with the agent.

### A contributor runs the workflow with no decider at all

An open-source contributor with no API key clones a repo that uses these
templates and runs `/work-on` offline. Every eligible decision comes to them
as an ordinary prompt with the same answer set, and nothing in the run
depends on a network call or a paid service.

## Scope Boundary

### In

- A way for template authors to declare that a decision is eligible, with
  its question, per-answer meanings, an escape answer, and its inputs, in
  one declaration that serves the agent and the decider alike.
- koto settling eligible decisions itself during `koto next` when a decider
  is configured and confident, and handing them to the agent unchanged
  otherwise.
- A pluggable decider, with one hosted typed decision model (Jev) as the
  first provider.
- A record of every automatic decision and of shadow-mode comparisons, kept
  past session cleanup, so maintainers can judge whether a decision is safe
  to automate.
- Limits that keep human approvals, overrides, and irreversible or
  confirmation-guarded steps out of any decider's reach, whatever a template
  declares.
- Reshaping the first few shirabe decision states so they become eligible,
  and moving shirabe "decisions" that are really computable facts into gates.

### Out

- Using a decider for anything that produces text: rationales, plans, review
  findings, documents.
- A standalone command for asking the decider questions outside a workflow
  step. Decisions are reached through `koto next`, not called directly.
- Moving prose-only shirabe skills (explore, plan, review-plan, the document
  validation juries) onto koto templates. Most of the largest savings live
  there, but that's follow-on work once the mechanism exists.
- Deciding per workflow run whether to trust the decider. Trust is earned per
  decision and per answer from recorded agreement, not toggled by the agent.
- Comparing decision-model vendors, or running a model locally.
