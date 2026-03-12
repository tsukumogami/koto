# Lead: How do other tools unify multi-phase CLI interactions into a single command?

## Findings

### Pattern 1: Idempotent read-detect-write (git, kubectl apply)

git's `--continue` flags (`merge --continue`, `rebase --continue`, `cherry-pick --continue`) follow a state-aware model: the command detects the current repo state and adapts. Calling `git merge --continue` with no conflict resolution done errors out; calling it after resolution advances the merge. The command doesn't need a flag to know what stage it's in — it reads `.git/MERGE_HEAD` etc. kubectl `apply` is similar: send the desired state, let the tool reconcile against current state and act accordingly. Both are idempotent on read, act only when preconditions are met.

### Pattern 2: Combined plan+apply with optional approval (Terraform)

`terraform apply` can run a full plan-then-apply cycle in one command (`-auto-approve`) or stop after plan for human review. For automation, `-auto-approve` flattens the two phases into one. Terraform's design separates concerns at the binary level (plan produces a plan file, apply consumes it) but exposes them as one command for convenience. This works well for automation but requires external infrastructure for safe approval flows.

### Pattern 3: Reconciliation loops (Kubernetes controllers, Dapr, Temporal)

Orchestration frameworks don't use user-facing CLIs at all for state evolution — the controller runs a reconcile loop internally. The user only interacts at the edges (submit desired state, observe current state). This is the model most aligned with koto's architecture: the agent is the "controller" calling `koto next` in a loop. The CLI is the agent-koto protocol, not a human-facing tool.

### Pattern 4: Approval-gate patterns (CircleCI, GitHub Actions)

CI systems treat approval as a separate input channel: a human approves via the UI/API, and the pipeline resumes automatically. The pipeline doesn't "wait" by polling a command — it's event-driven. For agent-automation contexts this pattern suggests approval state should be pushed into koto's state file externally, and `koto next` just reads it.

### Pattern 5: AWS S3 sync (detect-and-act)

`aws s3 sync` reads source and destination, computes the diff, and applies changes in one command. No separate "plan" step. Works well for idempotent operations but doesn't help with multi-step stateful workflows.

### What works well for automation

- **Idempotent read**: calling the command with no data should always be safe and return current state
- **Implicit branching on state**: the command adapts behavior based on what state the system is in, not on which flags were passed
- **Structured output**: JSON output with well-defined schemas that scripts/agents can parse without screen-scraping
- **Single exit code convention**: 0 = success (including "nothing to do"), non-zero = error

### What doesn't work well for automation

- **Interactive prompts**: any command that blocks waiting for TTY input is unusable in non-interactive pipelines
- **State encoded in command name**: `git merge` vs `git merge --continue` vs `git merge --abort` — discoverable for humans but the agent must know which to call

## Implications

koto's unified `koto next` aligns with the reconciliation loop pattern more than with user-facing CLIs. The agent is effectively a controller calling `koto next` in a loop. This means:

- `koto next` with no data should always return current state (idempotent read)
- `koto next` with data should validate against current state expectations and either advance or reject
- The JSON output schema must clearly signal whether this was a read or a write, and what to do next
- The model is not novel in orchestration systems — it's novel only in being expressed as a CLI

## Surprises

Truly unified single-command workflows at the user-facing CLI level are rare. Most tools either separate read/write (`git status` vs `git commit`) or unify them only in orchestration frameworks where humans never touch the CLI. koto's design is in a middle category: an orchestration protocol exposed as a CLI for agent consumption. The right precedents are frameworks (Temporal, Dapr), not user-facing CLIs.

## Open Questions

- How does the agent know, from `koto next` output, what format of data to submit? The JSON output needs a `requires` or `expects` field.
- What should `koto next` return when called with data during a state that doesn't expect data? Error with explanation, or silently ignore and return current state?

## Summary

The most successful unified patterns follow an idempotent read-then-write model where calling the command with no data is always safe and returns current state, while calling it with data attempts a state-dependent write. koto's unified `koto next` aligns most closely with orchestration framework controller loops (Temporal, Kubernetes), not user-facing CLIs, suggesting the right design precedents are in that space. The key open question is how the JSON output communicates to the agent what data format the current state expects, making the agent's next action unambiguous.
