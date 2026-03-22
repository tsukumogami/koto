# Design Critique: shirabe work-on koto template
## Panel: Workflow Practitioner

**Role**: A practitioner who runs these workflows daily and cares about agent UX. Reviewing whether this will actually work well for agents running the workflow.

---

## 1. Resume UX: Does the directive alone reorient an agent?

### What the design proposes

Resume is two commands: `koto workflows` to find the active workflow name, then `koto next <name>` to get the current directive. The design says this is sufficient and treats resume as "free" — the event log handles it.

### What the real /work-on skill actually does on resume

The real /work-on skill builds resume context from *artifacts*, not from a state machine. It checks for files and commits in a specific order:

```
if wip/IMPLEMENTATION_CONTEXT.md exists → Resume at Phase 1
if cleanup commit exists → Resume at Phase 6
if summary commit exists → Resume at Phase 6
if implementation commits exist after plan → Resume at Phase 4 or 5
if plan commit exists → Resume at Phase 4
...
```

This gives the agent a layered picture: not just "you're at state X" but "here's what you already accomplished, here's what the branch looks like, here's what artifacts exist." The agent can read the plan file, the baseline, and the introspection output before continuing.

### What the design loses

When `koto next` returns, for example, `{"state": "implementation", "directive": "Write code and commit..."}`, an agent resuming after 24 hours knows it's in the implementation state. It does NOT know:
- Which commits already exist on this branch
- Whether tests were passing when it stopped
- What approach it was taking (the plan file is on disk, but nothing in the directive points to it)
- Whether it stopped mid-commit or after a clean checkpoint

The directive for `implementation` will say something generic about implementing the plan. But the agent needs to re-read `wip/issue_<N>_plan.md`, check `git log`, run `git status`, and reconstruct its working context before it can continue meaningfully. The design doesn't instruct the agent to do this on resume.

### The gap

The directive alone is not enough for reorientation. The actual /work-on skill's resume logic is effectively a contextual briefing: "here's what exists, here's what's done, here's where to pick up." The koto design assumes the directive carries this orientation. It doesn't — a directive like "Write code and commit" in the `implementation` state gives no orientation about what's already written.

**Fix needed**: Each state's directive text should include a "on resume" section that explicitly tells the agent to re-read the relevant wip artifacts and check git state before continuing. Alternatively, the skill wrapper (not the template) should inject this context whenever it detects a resume (i.e., when `koto workflows` returns an active workflow that it didn't just create).

---

## 2. Error Recovery: Are self-loops and done_blocked sufficient?

### What the design provides

- `implementation` self-loops on `partial_tests_failing`
- `pr_creation` self-loops on `creation_failed`
- `done_blocked` is a terminal state reachable from `analysis`, `implementation`, and `ci_monitor`
- `ci_monitor` self-loops via evidence fallback when CI hasn't been indexed yet

### What agents actually need when things go wrong

The real /work-on skill's phase instructions are much more specific about recovery paths:

**Phase 2 (Introspection)** has four outcomes (Proceed / Clarify / Amend / Re-plan), with explicit user interaction steps for Clarify and Amend. The koto template collapses this to `introspection_outcome: enum[approach_unchanged, approach_updated, issue_superseded]`. What happens on `issue_superseded`? The design routes it to... nothing explicit. The state definition says the evidence schema and implies advancing to `analysis`, but `issue_superseded` arguably means the agent should stop and consult the user, not proceed to analysis.

**Phase 4 (Implementation)** self-loops on `partial_tests_failing`. But the real phase instructions would guide the agent through: checking which tests fail, reading the error output, deciding whether the failure is related to the change or pre-existing, and escalating to the user if stuck in a loop. The template has no mechanism to detect that an agent has looped on `partial_tests_failing` three times — it will keep self-looping forever. There's no loop detection, no "after N attempts, escalate" path.

**done_blocked** is a terminal state with no return path. In practice, human intervention means: fix the issue, resume the workflow. But the workflow is now terminated — the agent would need to start a new one. This is especially bad for `ci_monitor` blocking: if the CI fails with a flaky test, the developer fixes it, and now has to re-run the entire workflow from scratch rather than resuming at `ci_monitor`.

### Specific gaps

1. **No loop count in self-loops**: `partial_tests_failing` self-loops indefinitely. An agent stuck in this loop will keep submitting the same evidence schema and koto will keep returning the `implementation` directive, forever. The design needs either a loop count gate ("if you've been in this state for more than N transitions, suggest stopping") or a distinction between `partial_tests_failing_retry` and `partial_tests_failing_escalate`.

2. **done_blocked is terminal with no recovery**: This design choice means any blocking condition — even a recoverable one like a flaky CI test — permanently terminates the workflow. The workflow must be re-initialized. A `blocked` state that allows resuming (e.g., the user fixes CI, then the agent can re-enter `ci_monitor`) would be more useful.

3. **issue_superseded has no explicit terminal path**: The evidence fallback on `introspection` includes `issue_superseded` as a possible outcome, but the state machine doesn't define what happens next. Does it route to `done_blocked`? To a new `issue_superseded_exit` terminal? The design doesn't say.

4. **Creation_failed self-loop is insufficient for PR errors**: PR creation failures come in several kinds: authentication failure, network error, branch doesn't exist on remote, no commits ahead of main, draft PR not supported. A single self-loop gives the agent no guidance. The real PR creation phase gives specific error resolution instructions.

---

## 3. Mode Confusion at Entry: Is the round-trip to koto necessary?

### The design's approach

The `entry` state always requires evidence submission: `mode: enum[issue-backed, free-form]` plus either an issue number or a task description. The agent submits this, koto routes to the appropriate first state.

### The agent's actual perspective

When the /work-on skill is invoked, the agent already knows the mode:

- The user called `/work-on 71` → issue-backed, issue number is 71
- The user called `/work-on "add retry logic"` → free-form, task description is known

This information is available before `koto init`. The agent doesn't discover the mode by consulting koto — it brings this information to koto. The `entry` state round-trip adds two steps (get directive, submit evidence) that carry no new information and make no decision. It's pure ceremony.

### The comparison

The real /work-on skill resolves input at the very start before any phases run. There's no "entry" state — input resolution is handled in the skill's opening section, and then the appropriate workflow phase is entered directly.

### The actual cost

In practice this means every single invocation, whether issue-backed or free-form, requires:
1. `koto next <name>` → receive entry directive
2. Formulate the evidence JSON
3. `koto next <name> --with-data '{"mode": "issue-backed", "issue_number": "71"}'`
4. Wait for routing
5. Now get the actual first useful directive

For what gain? The mode was known at step 0. The `entry` state records the mode in the event log, which is the only real benefit. But you can get that by just including `mode` in the workflow name (`work-on-issue-71` vs `work-on-free-add-retry-logic`) or in the `--var` flags at `koto init` time.

### The alternative

Initialize with `--var MODE=issue-backed --var ISSUE_NUMBER=71` for issue-backed, and skip `entry` entirely. The first state becomes `context_injection` (issue-backed) or `task_validation` (free-form), determined at init time by which template path is compiled. Or use a single template where the first state is determined by which `--var` values are present. Either way, the agent doesn't spend a round-trip telling koto what it already knows.

The cost of keeping `entry` as-is: agents must be carefully instructed not to pause after `koto init` expecting the entry state to be informative. An agent that treats `entry` as a real "decide what to do" state will waste context trying to reason about a decision already made.

---

## 4. Staleness Check in Free-Form Mode: Is skipping always correct?

### What the design does

Free-form mode skips staleness entirely. After `task_validation` and `research`, it goes straight to `setup` and then `analysis`. No staleness check.

### When this is wrong

Staleness as defined in the design is "assess codebase freshness since the issue was opened." For free-form tasks with no issue, there's no "since the issue was opened" baseline. That's why the design skips it.

But the *purpose* of the staleness check is broader than that: it's to detect if the context that motivated the task has changed, and to ensure the approach described in analysis still makes sense given the current codebase state. For free-form tasks, this concern doesn't disappear — it just has a different reference point.

### Concrete scenario

A user invokes `/work-on "update the cache eviction policy to use LRU"`. They described this task three weeks ago in a separate conversation. Since then, the caching module was completely rewritten. The `research` state gathers "sufficient" context about the current codebase and finds the new cache module, but it doesn't specifically check whether the LRU change is still the right approach given the new architecture.

The design's `task_validation` checks if the task is "clear and appropriately scoped" at the time the task description was provided — it assesses the description itself, not the current codebase state. Research gathers context but the template doesn't ask the agent to explicitly validate the task against what it found. There's no "given what research revealed, does this task still make sense?" gate.

### What the real just-do-it skill does

The real just-do-it skill has a `Validation Jury` (Phase 3) after research — it explicitly checks whether the task is still appropriate *after* seeing what research revealed. This is effectively a lightweight post-research validation that catches "the research showed this is harder/different than expected."

### The design gap

The koto template has `research → setup → analysis` with no validation step after research. The `task_validation` state runs before research, not after. An agent could find in research that the task is fundamentally misconceived, but the template has no mechanism to exit cleanly at that point — it would have to route to `done_blocked`, which is a dead end, not a "reconsider the task" state.

**Suggested fix**: A `post_research_validation` state (or rename `task_validation` to run after research) that can either proceed to `setup` or route to a `validation_exit` terminal with a human-readable explanation. This is the lighter-weight staleness check that free-form mode actually needs.

---

## 5. koto-skills Plugin Gaps: What would an agent need to run this workflow?

### What AGENTS.md currently teaches

The AGENTS.md is written around the hello-koto example, which is a two-state workflow with one command gate and one transition. The hello-koto loop is:
- `koto init --template ... --name ... --var ...`
- `koto next`
- Do work
- `koto transition eternal`
- `koto next` (confirm done)

### What the work-on template actually requires

The work-on template uses a fundamentally different CLI contract from what AGENTS.md describes:

**1. `koto next` vs `koto transition`**

AGENTS.md teaches `koto transition <state>` as the advancement command. The work-on design uses `koto next <name>` (bare or with `--with-data`) for both getting directives and advancing state. An agent reading AGENTS.md would try to call `koto transition <next-state>`, but wouldn't know what state name to transition to — the template determines routing from evidence, not from agent-supplied state names. These are different CLI contracts and the agent would be confused.

**2. Evidence submission is undocumented**

AGENTS.md has no mention of `--with-data`, evidence schemas, `expects` fields, or the NeedsEvidence response type. An agent resuming from AGENTS.md knows "run `koto next` to get a directive," but when that response comes back with `"action": "needs_evidence"` and an `expects` schema, the agent has no instructions for what to do. It would need to infer the `--with-data` syntax from the response shape alone.

**3. GateBlocked with evidence fallback is undocumented**

The new behavior introduced in Phase 1 (gate failure routes to NeedsEvidence rather than hard-stopping) is completely absent from AGENTS.md. When a gate fails on a state with an `accepts` block, the agent receives a `GateBlocked`-with-`expects` response. AGENTS.md only documents gate failure as "fix the issue and retry `koto transition`." An agent following AGENTS.md instructions when a gate fails would try to fix the gate condition, not submit evidence. This would cause confusing loops.

**4. Positional name argument is inconsistently documented**

AGENTS.md shows `koto next` (no argument) in its examples. The design uses `koto next work-on-71` (with the workflow name as a positional argument). The design acknowledges this inconsistency in Phase 4 (Documentation): "Update AGENTS.md to reflect the actual CLI signatures: positional `name` argument (not `--name` flag)." The inconsistency is known but currently unfixed. An agent following AGENTS.md would run `koto next` without a name — which may work if there's only one active workflow, but fails if multiple are active.

**5. Stop hook is insufficient for work-on**

The hooks.json Stop hook fires when any koto workflow is active and prints "Run `koto next <name>` to resume." For a work-on workflow, this generic message loses critical context: what phase was active, what the last evidence submitted was, what artifacts are on disk. The design notes in Phase 3 that the Stop hook should "mention work-on specifically when a `koto-work-on-*` workflow is active," but the current hook has no such differentiation. An agent seeing "Active koto workflow detected. Run `koto next <name>` to resume." in a new session would resume correctly mechanically, but without the contextual briefing described in question 1 above.

**6. No worked example for evidence-gated workflows**

AGENTS.md's single example uses `koto transition` with a command gate — the simplest case. There's no example of what the evidence submission loop looks like for a real workflow: receiving `expects`, constructing the JSON, submitting with `--with-data`, handling `invalid_submission` errors, and retrying. An agent implementing this correctly from AGENTS.md alone would be reverse-engineering the protocol from response shapes rather than following documented instructions.

### Summary of what would need to change

AGENTS.md needs a complete rewrite for the work-on context. Specifically:
- Replace the `koto transition` loop with the `koto next` loop (directive → execute → `koto next --with-data`)
- Document the `expects` field and evidence submission pattern with a concrete example
- Document GateBlocked-with-fallback: "when you receive GateBlocked with an `expects` field, submit evidence rather than fixing the gate condition"
- Add a positional name argument to all `koto next` examples
- Add a worked example that walks through at least 3-4 evidence-gated state transitions
- Clarify the resume sequence: `koto workflows` → read workflow name → re-read relevant wip artifacts → `koto next <name>` → continue

---

## Overall Assessment

The design makes sound architectural choices on the hard questions (gate-with-evidence-fallback, single template with routing entry). The problems are concentrated in the UX layer — what agents actually experience when running the workflow. The directive text is treated as sufficient context, when in practice agents need richer re-orientation. The self-loop failure model is theoretically clean but will trap agents in loops with no escalation path. The AGENTS.md documentation doesn't describe the protocol that work-on requires, which means every agent that reads AGENTS.md before running work-on will try the wrong CLI contract.

These aren't fatal — they're fixable in Phase 3 (skill integration) and Phase 4 (documentation). But they need to be explicitly scoped as work items, not assumed to follow from the design.
