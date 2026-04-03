# Phase 2 Research: Maintainer Perspective

## Lead A: Eval case structure and content

### Findings

**How eval.sh works**

Each eval case is a directory under `plugins/koto-skills/evals/`. The harness reads three files:

- `prompt.txt` — the user message sent as the `user` role. Can be any natural-language question or task description.
- `skill_path.txt` — path (relative to repo root) to a `SKILL.md` file. The file contents become the `system` message. Alternative: `skill.txt` for inline skill content.
- `patterns.txt` — one Perl-compatible regex per line (blank lines and `#` comments skipped). All patterns must match the response via `grep -qP`. The check is case-sensitive by default unless the pattern includes `(?i)`.

The harness calls `claude-sonnet-4-20250514` with `max_tokens=1024`, prints the truncated response, and reports pass/fail per pattern. Exit code 1 if any case fails.

The model gets the skill as its system prompt and the `prompt.txt` as a user message. There is no prior conversation history. The model is effectively being asked: "given this skill, how should I respond to this user question?" Pattern matching confirms the response contains expected command syntax, terminology, or flow.

**Behaviors most likely to regress if skills drift**

These are behaviors where the model might fall back on training knowledge to compensate for a stale or absent skill:

1. **`koto overrides record` — exact subcommand name and `--gate`/`--rationale` flags**
   Training data is unlikely to contain this command (it's project-specific). If the skill omits it or gets the syntax wrong, the model will have no fallback. A good regression target.

2. **Override two-step flow: `overrides record` then `koto next`**
   The protocol is: record override first, then call `koto next` again (the override is consumed by the engine on the next call). An agent that records but doesn't re-call `next` (or calls `next --with-data` instead) is broken. This is non-obvious and easy to lose if the skill drifts.

3. **`agent_actionable` field: means override is available**
   When `blocking_conditions[].agent_actionable == true`, the agent can record an override. If the skill fails to mention this field (it was added in Feature 2), the agent will never know overrides are possible and will just loop forever.

4. **`evidence_required` three sub-cases**
   When `action == evidence_required`, the agent must check whether `blocking_conditions` is empty or not:
   - Empty `blocking_conditions` → no gates blocking; agent should submit evidence per `expects`
   - Non-empty `blocking_conditions` → gates failed on a state that also accepts evidence; agent must decide whether to submit evidence or record overrides
   - `expects` present vs. absent
   
   The model's training knowledge of "evidence required means submit evidence" is correct only for the simple case. The presence of `blocking_conditions` in an `evidence_required` response is a koto-specific nuance the skill must teach.

5. **`koto next --to` skips gate evaluation**
   The `--to` flag does a directed transition without running gates on the target state. An agent that uses `--to` expecting gates to run will get a misleading response. The model's intuition about "going to a state" won't include the skip behavior.

6. **`koto next --full` vs. default `details` behavior**
   On first visit, `details` is included. On repeat visits, it's omitted unless `--full` is passed. An agent that never uses `--full` will miss extended guidance on re-entry. A stale skill might omit `--full` entirely.

7. **`koto workflows` (not `koto status`)**
   koto-author SKILL.md references `koto status` which does not exist. The correct command for listing active workflows is `koto workflows`. An agent following a stale skill will get a command-not-found error. A regression case here catches any future rename or new status command.

8. **`koto next --with-data` rejects a `"gates"` top-level key**
   The engine reserves the `"gates"` key; submitting `{"gates": {...}}` returns `invalid_submission`. An agent that tries to echo gate output back in evidence will fail. This is invisible without explicit skill coverage.

**Proposed eval cases**

---

**Case: `override-record-syntax`**

Scenario: Agent is shown an `evidence_required` response with a blocking condition marked `agent_actionable: true`. Asks what command to run.

```
# prompt.txt
I ran `koto next my-workflow` and received this response:
{
  "action": "evidence_required",
  "state": "ci_check",
  "directive": "CI must pass before advancing.",
  "blocking_conditions": [
    {
      "name": "ci",
      "type": "command",
      "status": "failed",
      "agent_actionable": true,
      "output": {"exit_code": 1, "error": "tests failed"}
    }
  ],
  "expects": null
}
The CI system is unavailable but I have human approval to proceed. What command do I run?
```

```
# patterns.txt
# Must use overrides record with the correct flags
koto overrides record
--gate\s+ci
--rationale
```

---

**Case: `override-two-step-flow`**

Scenario: Agent has just recorded an override. Asks what to do next.

```
# prompt.txt
I ran `koto overrides record my-workflow --gate ci --rationale "CI offline, approved by team"` and it returned `{"status": "recorded"}`. What do I do next?
```

```
# patterns.txt
# Must call koto next again (not submit evidence directly)
koto next
# Must NOT suggest submitting --with-data as the next step
```

Note: the second pattern would need to be a negative assertion — the harness only supports positive `grep -qP` matches. Consider splitting into separate cases or noting this limitation: the harness cannot assert absence. For negative checks, document as an open question (see below).

---

**Case: `gate-blocked-no-override`**

Scenario: `gate_blocked` with `agent_actionable: false`.

```
# prompt.txt
koto next returned:
{
  "action": "gate_blocked",
  "state": "review",
  "directive": "All gates must pass.",
  "blocking_conditions": [
    {"name": "lint", "type": "command", "status": "failed", "agent_actionable": false, "output": {"exit_code": 1, "error": "style errors"}}
  ]
}
What should I do?
```

```
# patterns.txt
# Agent should fix the underlying issue and re-run koto next
koto next
# Should NOT suggest overrides record (gate is not agent_actionable)
# (positive proxy: response must mention the lint/fix the issue)
(?i)fix|(?i)lint|(?i)style
```

---

**Case: `directed-transition-gate-skip`**

Scenario: Agent wants to jump to a specific state.

```
# prompt.txt
I need to skip ahead to state "deploy" without going through the normal gates. How do I do that with koto?
```

```
# patterns.txt
koto next.*--to
--to\s+deploy
# Must indicate gates are skipped / not evaluated
(?i)skip|(?i)bypass|(?i)gate
```

---

**Case: `details-full-flag`**

Scenario: Agent has visited a state before and wants extended guidance again.

```
# prompt.txt
I visited the `implementation` state before and now I'm back. The `koto next` response doesn't include the `details` field anymore. How do I get the extended guidance?
```

```
# patterns.txt
--full
koto next.*--full
```

---

**Case: `correct-list-command`**

Scenario: Agent wants to check which workflows are active (catches the `koto status` vs `koto workflows` bug and any future drift).

```
# prompt.txt
How do I see all active workflows in my current directory?
```

```
# patterns.txt
koto workflows
# Must NOT suggest koto status (which does not exist)
```

Note: again requires a negative assertion which the harness can't do. Positive proxy: response must include `koto workflows`.

---

**Case: `evidence-required-with-blocking-conditions`**

Scenario: Agent gets `evidence_required` with non-empty `blocking_conditions` — the compound case.

```
# prompt.txt
koto next returned action: "evidence_required" but blocking_conditions is not empty. There's a failing gate with agent_actionable: true. What are my options?
```

```
# patterns.txt
# Agent should present both paths: submit evidence OR record override
(?i)override|koto overrides record
(?i)submit|(?i)evidence|koto next.*--with-data
```

---

**Case: `gates-key-reserved`**

Scenario: Agent tries to pass gate output back in evidence.

```
# prompt.txt
I want to submit evidence that includes gate outputs. Can I include a "gates" key in my --with-data JSON?
```

```
# patterns.txt
# Must warn that "gates" key is reserved / will be rejected
(?i)reserved|(?i)invalid|(?i)reject|(?i)cannot
```

### Implications for Requirements

- The PRD should specify at minimum 6 eval cases covering: override record syntax, override two-step flow, gate-blocked flow, `--to` gate-skip behavior, `--full` flag, and the `evidence_required` compound case.
- The harness only supports positive pattern matching (all patterns must match). Negative assertions (ensuring the model does NOT recommend a nonexistent command like `koto status`) cannot be expressed directly. The PRD should note this limitation and either: (a) accept positive-only coverage, or (b) require a harness change to support negative patterns (out of current scope).
- `skill_path.txt` is the correct mechanism — eval cases should point to the actual `SKILL.md` being tested, not inline copies, so they stay in sync automatically.
- Pattern complexity should be kept low. The harness uses `grep -qP` with no partial-line anchoring, so patterns like `koto overrides record` and `--gate\s+\w+` are reliable. Avoid patterns that depend on sentence structure.

### Open Questions

1. Can the harness support negative patterns (e.g., `!koto status`)? Requires a harness change. Should the PRD require this as a requirement for the eval harness, or defer?
2. Should cases test the combined koto-user + koto-author skill (system prompt = both files concatenated) or only one skill at a time? Concatenating tests integration but makes failure diagnosis harder.
3. How should the eval case for `koto status` absence be expressed if negative patterns aren't supported? One option: test that `koto workflows` appears in the response and accept that the model might also hallucinate `koto status` without being caught.

---

## Lead B: Eval trigger scope

### Findings

**Current trigger configuration**

The `eval-plugins.yml` workflow fires on pull requests to `main` when any file under `plugins/**` changes. Source changes under `src/**` do not trigger evals unless the plugin is also touched.

**Source-only PR frequency**

Analyzing 76 non-merge commits from the past three months:

- Commits touching `src/` with no `plugins/` changes: **17** (22% of commits)
- Commits touching `plugins/` only: **2** (version bumps during releases)
- Commits touching both: **3** (overlapping source and plugin work)
- Commits touching neither: **54** (docs, CI, releases, chores)

The 17 source-only commits cover behavioral changes including: structured gate output (#120), gate override mechanism (#122), gate contract validation (#123), gate backward compatibility (#125), `koto next` output contract redesign (#109). These are exactly the kind of changes that would break skill correctness if the skill was not updated, and none of them would trigger the eval today.

**Cost of expanding to `src/**`**

At ~17 source-only commits per three-month window, that's roughly 5-6 per month. With evals running per PR (not per commit), and PRs typically containing multiple commits, the real PR count touching `src/` without `plugins/` is lower — estimated 10-12 source-only PRs over 3 months, or 3-4 per month.

At $0.05-0.15 per run with 5-8 eval cases, that's $0.15-0.60 per month of additional API cost if `src/**` is added to the path filter. This is negligible.

The more significant cost is false signal: a source-only PR that changes behavior but doesn't update the skill will cause evals to fail, blocking merge. This is the intended behavior if evals are meant to enforce freshness. However, a PR that changes behavior but the skill doesn't need updating (e.g., internal refactor with no user-visible change) would also fail, requiring the author to either update the skill unnecessarily or explain why evals are failing.

**Alternative: manual-only trigger**

The current design (plugin-only trigger) relies on authors to remember to update skills when source changes. The gap analysis shows this doesn't happen: PRs #120, #122, #123, #125 all changed behavior without touching plugins. An opt-in workflow dispatch would have the same reliability problem.

### Implications for Requirements

- The PRD should specify whether evals are a correctness gate (block merge on failure) or a monitoring signal (report but don't block). The current workflow exits 1 on failure, which blocks merge.
- If evals are a correctness gate, the path filter should include `src/**` to catch behavioral changes without plugin updates. The financial cost is negligible; the process cost is that authors of pure source PRs must verify skill accuracy or update skills alongside source changes.
- A practical middle path: add `src/**` to the path filter but make the `eval-plugins` job advisory (non-blocking) for source-only PRs using a `continue-on-error` flag or a separate non-required check. This surfaces regressions without blocking unrelated source work.
- The PRD must decide: does a failing eval on a source-only PR mean "don't merge" or "update the skill before merging"? Both are defensible; the PRD should pick one.

### Open Questions

1. Should the `eval-plugins` check be required (blocking) or advisory (non-blocking) in GitHub branch protection rules? Currently the workflow fails hard, but whether it blocks merge depends on the branch protection configuration.
2. If `src/**` is added to the path filter, should there be a label or path-based exception for pure refactors that don't change user-visible behavior? How would that be enforced?
3. Should evals run on push to `main` (post-merge) in addition to PR? This would catch squash-merged changes that weren't gated pre-merge.

---

## Summary

The eval harness expects three files per case — `prompt.txt`, `skill_path.txt` (or `skill.txt`), and `patterns.txt` — where all Perl regex patterns must match the model response. Eight specific behaviors are high-value regression targets: override record syntax (`koto overrides record --gate --rationale`), the two-step override flow, the `agent_actionable` signal, `evidence_required` with blocking conditions, `--to` gate-skip, `--full` for repeated details, the correct list command (`koto workflows` not `koto status`), and the reserved `"gates"` key. The harness's positive-only pattern matching limits negative assertions, which the PRD should acknowledge. On trigger scope, 17 of 76 recent non-merge commits touched `src/` without touching `plugins/`, including every significant behavioral change in the gate-transition contract work — the current plugin-only filter would have missed all of them; expanding to `src/**` adds negligible API cost and surfaces real drift, but the PRD must decide whether failing evals block source-only PRs or serve as an advisory signal.
