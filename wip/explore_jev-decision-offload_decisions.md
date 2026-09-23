# Exploration Decisions: jev-decision-offload

## Setup
- Run in `--auto` mode: the session runs under a goal hook that expects no blocking prompts.
- Anchor the exploration in koto (public, tactical): the classifier executes inside the engine; shirabe is its consumer, and agents read both repos.
- No adversarial demand lead: auto mode reads label signals only, and the topic came from a plain request with no labels.
- Source document kept outside the repo; wip/ artifacts summarize it rather than copy it.
