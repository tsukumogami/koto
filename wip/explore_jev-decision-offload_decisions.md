# Exploration Decisions: jev-decision-offload

## Setup
- Run in interactive mode. The session started in `--auto`, and the user switched to interactive while round 1 agents were running.
- Anchor the exploration in koto (public, tactical). The classifier runs inside the engine and shirabe consumes it; agents read both repos.
- Source document kept outside the repo. wip/ artifacts summarize it rather than copy it.

## Round 1
- Authority: the template author decides which decisions are classifier-eligible; koto has no fixed policy beyond what the compiler enforces.
- No-key path: reshaped workflows must behave exactly as they do today when no key is present. The classifier is only an accelerator.
- Abstraction: koto gets a generic typed-decider interface (choice / score / boolean questions), and Jev is its first provider.
- Payoff: context, wall-clock, consistency and cost all count; no single driver.
- Surface: no standalone command. A decision is a step reached through `koto next`; koto executes it when a key exists, and otherwise hands it to the agent, which answers with evidence.
- Include a devil's advocate analysis of the case for not doing this (added as a lead in round 1).
- Posture: staged in koto. The decider is built inside `koto next`, shadow is the default mode, and auto is enabled per value once own-traffic concordance clears a bar. The shirabe reshaping proceeds in parallel. Chosen over full auto (unmeasured calibration) and over deferring the decider (gives up consistency and the per-item pre-screen wins).
- Compiler floor: authors mark eligibility, but the compiler refuses classifier authority over gates, overrides, and irreversible or confirmation-guarded edges.
- Deterministic "decisions" (batch_outcome, pause_decision, cascade_status, verification matching, panel aggregation, retry caps) move to gates regardless of the classifier.
- Ready to crystallize after one round: the remaining gaps are design questions or need shadow data.
