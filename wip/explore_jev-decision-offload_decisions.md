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
