# Exploration Decisions: batch-child-spawning

## Round 1

- Execution mode: switched to --auto on user request. Max rounds default 3.
- Scope narrowed: dynamic DAG execution with mid-flight task addition is in; cross-batch edges and distributed execution are out.
- Inter-child dependency ordering is required in v1 (user: "it's required -- these are GH issues that depend on each other").
- Failure routing: user deferred the choice; exploration must surface tradeoffs and recommend a default.
- Open interpretation: "task spawns sibling or grand-children" could mean (A) append to the same batch, or (B) start a nested batch. User declined to disambiguate — investigate both and recommend in convergence.
- Adversarial lead skipped: source issue is labeled `needs-design`, not `needs-prd` or `bug` — per --auto label-only rule, adversarial lead does not fire.
