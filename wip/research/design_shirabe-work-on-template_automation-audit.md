# Automation Audit: work-on Template States

**Date:** 2026-03-22
**Scope:** 17 states in `shirabe/koto-templates/work-on.md` as specified in
`DESIGN-shirabe-work-on-template.md`

## Background and Key Constraint

koto currently does two things in `advance_until_stop`:

1. **Evaluates gates** (shell exit codes only) to determine whether to auto-advance
2. **Surfaces directives** when a state requires evidence

koto does NOT run commands on state entry to perform work. The engine's evaluation
order is: signal → chain limit → terminal → integration → gates → transition
resolution. There is no "on_entry" action hook. If a state needs work done before
its gate can pass, that work must currently be triggered outside koto (by the agent
or a calling script).

This is the central constraint for every "koto-gated" classification below: koto
can verify that work happened, but it cannot yet initiate the work itself.

---

## State-by-State Classification

### 1. `entry`

**Classification: agent-judgment**

Routes issue-backed vs. free-form mode. The agent must read the invocation arguments
and decide which mode applies. Nothing deterministic can make this decision without
understanding the caller's intent. No gate is defined; evidence-only by design.

---

### 2. `context_injection`

**Classification: koto-gated** (degrades to agent-judgment until `--var` ships)

**What needs to run:** `extract-context.sh --issue <N>` — a shell script that reads
the GitHub issue and linked design docs, then writes `wip/issue_<N>_context.md`.

**Gate:** `test -f wip/issue_{{ISSUE_NUMBER}}_context.md`

**koto capability gap — two independent blockers:**
- `--var` flag on `koto init` is not implemented; `{{ISSUE_NUMBER}}` is never
  substituted, so the gate command is malformed and always fails.
- Even if `--var` ships, koto has no state-entry action hook. The script must be run
  by the agent (or a calling wrapper script) before koto evaluates the gate.

**What koto would need for autonomous execution:** a state `on_entry` action field
that runs a shell command when the engine first arrives at a state. With that, the
template could declare `on_entry: extract-context.sh --issue {{ISSUE_NUMBER}}` and
koto would run it, then evaluate the gate on the next `koto next` call.

**Current behavior:** gate fails unconditionally → gate-with-evidence-fallback
triggers (once Phase 1 engine changes land) → agent submits
`context_injected: complete`.

---

### 3. `task_validation`

**Classification: agent-judgment**

Assesses whether a free-form task description is clear and scoped for direct
implementation. This requires reading the description and making a judgment call:
is this specific enough? Is the scope tractable? A shell command cannot answer either
question. Always evidence-gated by design.

---

### 4. `validation_exit`

**Classification: koto-autonomous** (trivially — it's terminal)

koto stops at this state because it is terminal. No agent action is required; the
workflow is over. The agent was already told the verdict in `task_validation` or
`post_research_validation` before reaching here.

**Current capability:** already works. Terminal states need no new capability.

---

### 5. `research`

**Classification: agent-judgment**

Lightweight codebase exploration for free-form tasks. The agent reads files, checks
git log, and forms a `context_summary`. Shell commands can run specific queries, but
synthesizing what's relevant to an arbitrary task description requires interpretation.
Evidence-gated with unconditional transition (no routing decision needed, but the work
itself is judgment).

---

### 6. `post_research_validation`

**Classification: agent-judgment**

Reassesses the task against codebase findings. Three possible verdicts: ready,
needs_design, exit. Each verdict requires weighing what research found against the
task's stated goal. Cannot be expressed as a gate condition. Always evidence-gated by
design.

---

### 7. `setup_issue_backed`

**Classification: koto-gated** (action is partially scriptable; gate is sound)

**What needs to run:** Two actions in sequence:
1. `git checkout -b <branch-name>` (or reuse an existing feature branch)
2. `go test ./... > wip/issue_<N>_baseline.md 2>&1` (run baseline tests, save output)

**Gate (as described in the design):** `git rev-parse --abbrev-ref HEAD` is not
main/master AND `test -f wip/issue_{{ISSUE_NUMBER}}_baseline.md`

**koto capability gap:** No `on_entry` action hook. The branch and baseline file must
be created before koto evaluates the gate. The branch name is also not deterministic
— it depends on the agent's choice of name — so a fully autonomous `on_entry` would
need a naming convention (`issue-<N>` or similar) baked into the template.

**What koto would need for autonomous execution:**
- `on_entry` action hook to run the branch creation and baseline commands
- A deterministic branch naming convention (e.g., `issue-{{ISSUE_NUMBER}}`) so the
  command can be specified statically in the template
- `--var` support for `{{ISSUE_NUMBER}}` substitution in the action command

**Partial path today:** The gate validates that setup happened correctly. If a
wrapper script runs setup before invoking `koto next`, the gate auto-advances without
agent involvement. The agent is currently needed only because no wrapper script exists.

---

### 8. `setup_free_form`

**Classification: koto-gated** (same structure as `setup_issue_backed`)

**What needs to run:**
1. `git checkout -b <branch-name>` (branch name derived from task slug)
2. `go test ./... > wip/task_<slug>_baseline.md 2>&1`

**Gate:** branch is not main/master AND baseline file exists (shell expansion, no
`{{VAR}}` needed for the existence check since free-form uses slug patterns)

**koto capability gap:** same as `setup_issue_backed` — no `on_entry` action hook.
The slug must be derivable from the workflow name (already enforced by `koto init`
naming convention), so this is slightly more tractable than the issue-backed case.

**What koto would need for autonomous execution:** `on_entry` action hook. The branch
name could be derived from the workflow name (e.g., `koto init work-on-add-retry`
implies branch `add-retry`). No `--var` dependency for the file existence check.

---

### 9. `staleness_check`

**Classification: agent-judgment**

The agent runs `check-staleness.sh` and reads its YAML output to determine whether
the codebase has diverged enough from the issue's creation date to warrant
introspection. The script produces structured output, but the routing decision
(`fresh` vs. `stale_requires_introspection`) requires reading the
`introspection_recommended` field and applying judgment about whether the signal is
meaningful for this specific issue.

Theoretically, if `check-staleness.sh` output could be parsed by a gate command and
the threshold were baked in, this could become koto-gated. But the design explicitly
states: "Always evidence-gated (command gates can't inspect script output content)."
The gate condition would need to be `check-staleness.sh | jq '.introspection_recommended'
== "false"` — which is technically expressible as a shell command but requires
interpreting YAML/JSON output from an external script.

**What koto would need for partial automation:** A gate that pipes script output to a
JSON/YAML parser and checks a specific field value. This is expressible today as a
shell one-liner gate command — the blocker is not a koto capability gap but the
`--var` dependency for the issue number argument. Once `--var` ships:
```
check-staleness.sh --issue {{ISSUE_NUMBER}} | jq -e '.introspection_recommended == false'
```
would make this **koto-gated**, with the gate auto-advancing when the script says no
introspection is needed and falling back to evidence when it says introspection is
recommended.

**Current classification caveat:** The design classified this as always evidence-gated
because it predates recognizing that piped shell commands are valid gate expressions.
This is a reclassification opportunity, not a current capability.

---

### 10. `introspection`

**Classification: koto-gated** (degrades to agent-judgment until `--var` ships)

**What needs to run:** A sub-agent Task invocation that re-reads the issue against the
codebase and writes `wip/issue_<N>_introspection.md` with a structured outcome.

**Gate:** `test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`

**koto capability gap — two blockers:**
- `--var` not implemented; gate always fails.
- The sub-agent invocation itself cannot be triggered by koto; it requires the calling
  agent to spawn a Task call. This is fundamentally different from running a shell
  command — koto has no mechanism to invoke Claude sub-agents.

**What koto would need for autonomous execution:** Even with `on_entry` hooks, koto
cannot launch a sub-agent. The sub-agent work will always require the agent tier.
This state is correctly classified as koto-gated (agent does the work, koto verifies
completion via artifact existence) rather than koto-autonomous.

**Once `--var` ships:** gate evaluates `test -f wip/issue_<N>_introspection.md`. If
the artifact exists (agent already ran introspection), koto auto-advances. If not,
gate-with-evidence-fallback surfaces the expects schema.

---

### 11. `analysis`

**Classification: koto-gated** (gate is sound; work is agent-judgment)

**What needs to run:** The agent reads the issue/task, existing codebase, context
artifact, introspection artifact, and produces `wip/issue_<N>_plan.md` (or
`wip/task_<slug>_plan.md`). This is creative/interpretive work — cannot be automated.

**Gate (issue-backed):** `test -f wip/issue_{{ISSUE_NUMBER}}_plan.md` (requires `--var`)
**Gate (free-form):** `ls wip/task_*_plan.md 2>/dev/null | grep -q .` (works today)

**What koto can do now:** For free-form workflows, the gate works without `--var`. If
the agent has already produced the plan file, `koto next` auto-advances without
evidence submission. For issue-backed, this waits on `--var`.

**Autonomous ceiling:** koto can verify the plan exists but cannot write it. This
state is permanently koto-gated (not autonomous) because plan creation is
agent-judgment. The gate is the appropriate enforcement mechanism.

---

### 12. `implementation`

**Classification: koto-gated** (gate is sound; work is agent-judgment)

**What needs to run:** The agent writes code and commits. Cannot be automated.

**Gate:** Three conditions:
1. `git rev-parse --abbrev-ref HEAD` is not main/master
2. `git log main..HEAD --oneline | grep -q .` (commits beyond main exist)
3. `go test ./...` passes (exit code 0)

All three are standard shell commands that work today without `--var`. The gate is
fully evaluable by koto right now.

**What koto can do now:** If the agent has committed code and tests pass, `koto next`
auto-advances through `implementation` without evidence submission. This is the
strongest existing gate in the template — no new koto capability needed.

**Autonomous ceiling:** Code writing requires agent judgment. koto verifies completion;
it cannot perform it.

---

### 13. `finalization`

**Classification: koto-gated** (gate is sound; work is mostly scriptable)

**What needs to run:** Cleanup (remove debug code, format), write summary file, run
final test verification.

**Gate:** `test -f wip/issue_{{ISSUE_NUMBER}}_summary.md` (or task slug variant) AND
`go test ./...` passes.

**koto capability gap:**
- `--var` needed for issue-backed summary path (free-form slug variant works today
  with a glob pattern).
- No `on_entry` hook; the summary file must be written before gate evaluation.

**What koto would need for partial automation:** The cleanup steps (`gofmt`, removing
debug code) could be run by an `on_entry` hook script. Writing the summary itself
still requires the agent's judgment about what changed and what to note. This makes
finalization koto-gated rather than koto-autonomous — the agent writes the summary,
koto verifies it exists and tests pass.

---

### 14. `pr_creation`

**Classification: koto-gated** (could be koto-autonomous with `on_entry` hooks)

**What needs to run:** `gh pr create --title "..." --body "..." --base main`

**Gate:** None defined in the design. The design says "no gate can verify a PR was
created before the action happens" — this is the design's current classification, but
it's imprecise. A gate CAN verify a PR exists after creation:
```
gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number --jq '.[0].number // empty' | grep -q .
```

**koto capability gap:** Without `on_entry` hooks, koto cannot run `gh pr create`.
The agent must create the PR first. But the gate above would let koto verify it
without agent evidence submission — the design missed this verification opportunity.

**What koto would need for autonomous execution:** An `on_entry` hook running
`gh pr create` with a title derived from the branch/plan. The PR title and body
require judgment (extracting the right summary from the plan), so this is koto-gated
at best unless the title/body format is fully templated.

**Reclassification note:** The design classified this as "always evidence-gated" but
a verification gate is feasible. Once `on_entry` hooks exist, this could become
koto-gated with the gate confirming PR existence.

---

### 15. `ci_monitor`

**Classification: koto-autonomous** (with one caveat)

**Gate:**
```
gh pr checks $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number \
  --jq '.[0].number // empty') --json state \
  --jq '[.[] | select(.state != "SUCCESS")] | length == 0' | grep -q true
```

This is a complete, standalone shell command that returns exit 0 when all CI checks
pass and exit 1 otherwise. It requires no agent involvement if the PR exists and CI
is running.

**koto capability:** This gate works today (no `--var` needed). When CI passes, koto
auto-advances to `done` without any agent action. When CI is pending or failing, the
gate fails and gate-with-evidence-fallback surfaces the expects schema, allowing the
agent to report `failing_unresolvable` if needed.

**Caveat:** The `// empty` guard handles the brief window after PR creation where the
PR isn't indexed yet — during this window the gate fails and falls back to evidence.
This is acceptable behavior. Once indexed, koto auto-advances.

**No new capability needed.** This is the one state the design already correctly
identifies as capable of full auto-advancement, and it works without any Phase 1
changes (no `--var`, no `on_entry` hooks).

---

### 16. `done`

**Classification: koto-autonomous** (trivially — it's terminal)

Terminal state. koto stops here automatically. No agent work required.

---

### 17. `done_blocked`

**Classification: koto-autonomous** (trivially — it's terminal)

Terminal state. koto stops here automatically. The directive text (recovery
instructions using `koto rewind`) is read by the agent or user, but koto itself
requires no capability to reach or remain in this state.

---

## Summary Table

| # | State | Classification | What koto needs for autonomous/gated execution |
|---|-------|---------------|------------------------------------------------|
| 1 | `entry` | agent-judgment | N/A — mode selection is inherently interpretive |
| 2 | `context_injection` | koto-gated | `--var` (Phase 1) + `on_entry` action hook to run `extract-context.sh` |
| 3 | `task_validation` | agent-judgment | N/A — task clarity assessment requires judgment |
| 4 | `validation_exit` | koto-autonomous | Already works — terminal state |
| 5 | `research` | agent-judgment | N/A — codebase synthesis for arbitrary tasks requires interpretation |
| 6 | `post_research_validation` | agent-judgment | N/A — weighing research findings requires judgment |
| 7 | `setup_issue_backed` | koto-gated | `on_entry` hook for branch+baseline; `--var` for baseline path; deterministic branch naming convention |
| 8 | `setup_free_form` | koto-gated | `on_entry` hook for branch+baseline; slug derivable from workflow name (no `--var` for gate) |
| 9 | `staleness_check` | agent-judgment (reclassifiable) | `--var` (Phase 1); gate could be `check-staleness.sh --issue {{ISSUE_NUMBER}} \| jq -e '.introspection_recommended == false'` — would make this koto-gated |
| 10 | `introspection` | koto-gated | `--var` (Phase 1); sub-agent invocation always requires agent tier — gate verifies artifact existence |
| 11 | `analysis` | koto-gated | `--var` for issue-backed gate path; free-form gate works today; plan creation is always agent-judgment |
| 12 | `implementation` | koto-gated | Already works — gate (branch, commits, tests) is fully evaluable today without new capabilities |
| 13 | `finalization` | koto-gated | `--var` for issue-backed summary path; `on_entry` hook could run gofmt/cleanup; summary writing remains agent-judgment |
| 14 | `pr_creation` | koto-gated | `on_entry` hook to run `gh pr create`; PR existence gate is feasible (missed in design); PR title/body require agent judgment |
| 15 | `ci_monitor` | koto-autonomous | Already works — gate evaluates CI status without any new capabilities |
| 16 | `done` | koto-autonomous | Already works — terminal state |
| 17 | `done_blocked` | koto-autonomous | Already works — terminal state |

---

## Capability Gaps by Priority

### Gap 1: `--var` flag on `koto init` (Phase 1, already planned)

Unblocks: `context_injection`, `introspection`, `analysis` (issue-backed),
`finalization` (issue-backed), and optionally `staleness_check` (if reclassified).

This is the highest-leverage single change. It turns 4-5 gate failures from
"always fails, falls to evidence" into "auto-advances when artifact exists."

### Gap 2: `on_entry` action hook (not yet planned)

A new template field (e.g., `on_entry: <shell command>`) that koto runs when first
entering a state, before gate evaluation. The engine's evaluation order would become:
signal → chain limit → terminal → integration → **on_entry** → gates → transition.

Unblocks: `context_injection` (run `extract-context.sh`), `setup_issue_backed` and
`setup_free_form` (run branch creation + baseline), `finalization` (run gofmt/cleanup),
and `pr_creation` (run `gh pr create`).

Without this hook, all states where koto needs to perform work (not just verify it)
remain koto-gated at best — the agent or a wrapper script must trigger the work
before `koto next` is called.

### Gap 3: Verification gate for `pr_creation` (design oversight)

The design marked `pr_creation` as "always evidence-gated" because no gate can verify
a PR before it's created. This is correct, but it overlooked that a gate CAN verify
a PR exists after creation. Adding a gate:
```
gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number \
  --jq '.[0].number // empty' | grep -q .
```
would allow koto to auto-advance on resume when a PR already exists (e.g., after a
session interruption where the PR was created but evidence wasn't submitted). This
requires no new koto capability — just a gate command added to the template.

### Gap 4: `staleness_check` reclassification opportunity

With `--var` and a piped gate command, `staleness_check` could become koto-gated
rather than always evidence-gated. The gate would call `check-staleness.sh` and parse
its output. This requires no new koto capability beyond `--var` — it uses existing
gate evaluation with a more complex shell pipeline. Whether this is worth the gate
complexity depends on how deterministic `check-staleness.sh`'s output is in practice.

---

## States That Will Always Require Agent Involvement

Five states are permanently agent-judgment because no deterministic command can
substitute for the work they require:

- **`entry`**: mode routing based on invocation intent
- **`task_validation`**: description clarity and scope assessment
- **`research`**: open-ended codebase exploration and synthesis
- **`post_research_validation`**: multi-factor readiness assessment
- **`analysis`** (work, not gate): implementation planning requires creativity

The gate on `analysis` can auto-advance past the state if the plan already exists,
but writing the plan in the first place always requires the agent.
