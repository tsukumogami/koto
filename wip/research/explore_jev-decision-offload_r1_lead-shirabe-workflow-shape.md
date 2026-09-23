# Lead: What workflow-shape changes would let shirabe move decisions off the main agent thread and into koto?

## Findings

### 1. How shirabe drives koto today

Only three shirabe skills run on koto templates: `/work-on`
(`skills/work-on/koto-templates/work-on.md`, ~30 states), `/scope`
(`skills/scope/koto-templates/scope.md`, 21 states) and `/execute`
(`skills/execute/koto-templates/execute.md`, ~15 states, which also
materializes `work-on.md` as its per-issue child via `materialize_children`).
Every other skill (`explore`, `decision`, `design`, `prd`, `plan`, `brief`,
`charter`, `roadmap`, `review-plan`, `release`, ...) is pure prose phases under
`skills/<name>/references/phases/*.md` with state kept in `wip/` files. The
routing decisions with the most "judgment by arithmetic" in them live in those
prose skills, not in the templates.

Shipping: templates are plain Markdown-with-YAML-frontmatter files in the
plugin. There's no build step; `koto init --template
${CLAUDE_PLUGIN_ROOT}/skills/<skill>/koto-templates/<skill>.md --var ...`
compiles at init time. Plugin scripts are reached through a declared
`PLUGIN_ROOT` template variable because koto resolves only `{{KEY}}`
(explained at length in the `PLUGIN_ROOT` variable description in
`work-on.md` and `scope.md`). CI adds shirabe-specific static checks on top of
`koto template compile`: `scripts/check-template-directives.sh` (unguarded
evidence, unrouted gates), `scripts/check-template-interpolation.sh` (no
`$NAME` in commands), `scripts/check-init-site-vars.sh`,
`scripts/validate-template-mermaid.sh` (shared gate names must carry identical
commands), and per-skill suites that drive real koto sessions
(`.github/workflows/check-scope-scripts.yml`,
`check-execute-scripts.yml`). The `crates/shirabe` CLI is a helper (`shirabe
validate`, `plan_outlines`, `populate`, `work_summary`), called from gate
scripts and prose, not a template compiler.

Evidence submission: the agent reads the directive (the `## <state>` body
section, often a pointer into `references/phases/phase-N-*.md`), does the work,
and calls `koto next <WF> --with-data '{...}'`. `accepts` fields are almost all
enums (`verdict`, `outcome`, `status`, `*_outcome`) plus a free-text
`rationale`/`detail`. Gates are `command` (routed on `exit_code`),
`context-exists`, `context-matches` (regex over an agent-written context key),
and `children-complete`.

The routing surface koto offers is narrow and that shapes everything below
(`plugins/koto-skills/skills/koto-author/references/template-format.md`,
"The when condition", "Gate output fields", "is_set matcher"):
- `when` matches by equality only: an evidence enum value, a gate field
  (`gates.x.exit_code: 0`, `gates.x.matches: true`, children booleans), or
  `vars.X: {is_set: bool}`. No `>`/`<`, no counters, no equality on variable
  values.
- A `command` gate's stdout is discarded; only its exit code routes. The only
  way to carry a computed value forward is a `default_action` with
  `capture_stdout_as` (one line).
- There is no per-state visit counter, so "retry up to 3 times" lives in
  prose and the agent counts (`work-on.md` `## analysis`: "Self-loop with
  `scope_changed_retry` (up to 3 times). After 3, use
  `scope_changed_escalate`"; same for `implementation`).

### 2. The precedent: shirabe already did this once, for mechanical steps

`references/default-action-conversion.md` and
`docs/designs/current/DESIGN-koto-default-action-adoption.md` record a
deliberate campaign to move *mechanical* steps off the agent "so the agent's
turns go to judgment instead." The design's Decision 2 is the pattern that
transfers directly: **split the state; the mechanical step gets its own**,
because "a state doing two things has two outcomes, and a gate can only
establish one." Worked case C3 split `/execute`'s old
`worktree_discipline_check` (fetch + rebase + classify + write artifact) into
`worktree_sync` (engine-run) and a residual state that is pure classification
("The classification is judgment. The fetch and rebase are not.").

A classifier campaign is the second half of the same move: once the mechanical
work is out, the residual state is often a single typed question. The
authoring rules that campaign produced (every gate co-routed with an evidence
field; a gate-only guard auto-advances with no directive, per the `scope.md`
description block and `check-template-directives.sh`) are exactly the rules a
classifier decision has to respect: a classifier answer that routes by itself
behaves like gate-only routing, and one below threshold must fall back to an
evidence-required directive.

### 3. Where shirabe already uses the "structured artifact" pattern

`work-on.md` `pre_pr_evidence` states the reasoning explicitly: free-text
evidence fields are "satisfied by 'done'", so judgment calls are enums and
"concrete referents live in a context artifact whose shape a gate checks"
(`pre_pr.md` lines matched by `context-matches` regexes). `/execute`'s
`worktree_discipline_check` makes the agent write
`wip/work-on_{{PLAN_SLUG}}_impact.json`. `/scope`'s hop gates read the artifact
tree through `hop-complete.sh` and deliberately never read the run's own state
file ("a gate reading the file the run writes about itself asks the run
whether the run finished"). So shirabe already believes in: typed decision,
separate evidence artifact, deterministic check over it. The missing piece is
that the typed decision is always made by the main agent.

### 4. Decision shapes found, and the conversion pattern for each

a. **Deterministic decision left to the agent** (no classifier needed at all).
   - `/execute` `spawn_and_await` tick 2: the agent inspects `koto workflows`
     and hand-sets `batch_outcome: all_success|needs_attention`, although the
     `children-complete` gate already exposes `all_success` and
     `needs_attention` booleans.
   - `/execute` `pr_finalization`: the agent echoes `{{PAUSE_BEFORE_FINALIZE}}`
     back as `pause_decision`, because `vars.*` supports only `is_set`, not
     value equality.
   - `/execute` `plan_completion`: the agent parses `run-cascade.sh` JSON into
     `cascade_status`, and the prose admits "the halt is the agent's to observe
     rather than the machine's to enforce."
   - `/work-on` `verification`: SKILL.md "Definition of Done" step 1 is glob
     matching of `git diff --name-only` against the verification map. Pure
     computation, done in prose.
   - Retry caps in `analysis`/`implementation` (counting).
   Pattern: **gather facts deterministically, then branch** — a script gate
   whose exit code *is* the category, or a `default_action` capture. These are
   the lowest-risk changes and they shrink what any classifier would later
   need to see.

b. **Semantic single choice over a small, prunable state** (classifier-shaped).
   - `/execute` `worktree_discipline_check`: `impact` in
     `{none, informational, intent-changing}`.
   - `/work-on` `analysis` `issue_type` in `{code, docs, task}`.
   - `/execute` `pr_finalization` conventional-commit type (`feat` default,
     `fix`/`docs`/`chore`).
   - `/work-on` `task_validation` / `plan_validation` (`proceed|exit`) and
     `post_research_validation` (`ready|needs_design|exit`).
   - `references/decision-protocol.md` tier classification of emergent
     decisions: three ordered binary signals (irreversible? clear winner?
     primary question of the phase?) — already three atomic nouls.
   Pattern: **prefilter deterministically, then ask one typed question with an
   explicit escape key**, over a state the template assembles (not the agent's
   scratchpad).

c. **Compound scored judgment with embedded arithmetic** (classifier only
   after decomposition).
   - `/explore` Phase 4 crystallize
     (`skills/explore/references/quality/crystallize-framework.md`): count
     signals minus anti-signals per category, demote any category with an
     anti-signal, tiebreak within 1 point, conditionally run stage 2, fall back
     when nothing scores > 0. This is arithmetic, ranking and threshold logic
     that Jev explicitly cannot do, wrapped around ~40 atomic yes/no signal
     checks it can.
   Pattern: **split the compound judgment into atomic questions evaluated in
   one request; do the arithmetic in a script.**

d. **Judgment over two long documents** (poor classifier fit, but decomposable).
   - `/scope` `fold` `keep|absorb`: "does [the upstream] hold anything beyond
     that contribution which compression into a single section would lose?"
     Both verdicts route identically in the graph; the verdict drives what the
     agent does next (delete + carry check vs. keep). The state is two full
     documents, which is exactly the cluttered-context case Jev degrades on.
   Pattern: at most a per-section noul ("is this upstream section's content
   present in the downstream?") as a *proposal*, with the existing carry check
   as the deterministic verifier. Keep the agent in the loop.

e. **Human decisions** — `chain_proposal.author_decision`,
   `deferral_approval.approval_decision`. Out of scope; must never be
   classifier-resolved.

### 5. Before/after sketches

**Sketch 1 — `/execute` `worktree_discipline_check` (template exists today).**

Before: one state; agent reads the diff, reads
`references/worktree-discipline.md`, picks one of three, writes
`impact.json`, submits `impact`. Note the phase file's own definition: "None —
main has not advanced, or advanced in directories the PLAN does not touch."
That class is decidable by `git`.

After:
```yaml
upstream_facts:            # new, mechanical
  default_action:
    command: '{{PLUGIN_ROOT}}/skills/execute/scripts/upstream-facts.sh --plan "{{PLAN_DOC}}" --slug "{{PLAN_SLUG}}"'
    # writes context key upstream_facts.json:
    #   {"main_advanced": true, "commits": 7, "overlap_paths": [...],
    #    "deleted_referenced_paths": [...], "diffstat_overlap": "..."}
    # exit 0 = no overlap (impact none), 3 = overlap exists
  gates:
    facts_written: {type: context-exists, key: upstream_facts.json}
  transitions:
    - target: spawn_and_await          # impact none, never asked
      when: {gates.no_overlap.exit_code: 0, ...}
    - target: worktree_discipline_check
      when: {gates.no_overlap.exit_code: 3, ...}

worktree_discipline_check:  # now one narrow question
  # classifier question (hypothetical syntax; see template-schema lead)
  decide:
    impact:
      type: choice
      state_from: [upstream_facts.json, plan_intent.md]   # pruned
      criteria:
        informational: "Overlapping changes leave every file, interface and contract the PLAN relies on intact."
        intent_changing: "A file, interface or contract the PLAN relies on was deleted, renamed or changed in meaning."
        needs_agent: "The facts shown are not enough to tell."
```
Deterministic facts go in as precomputed values: `deleted_referenced_paths`
non-empty is a hard rule (route to `intent_changing` without asking anyone),
commit counts are numbers, no dates. What gets asked is only "does the
overlapping change alter meaning," over a diffstat plus the PLAN's intent
section. Without a key the agent gets the same question and the same pruned
facts, which is still a narrower job than today, and most runs (main didn't
touch PLAN paths) never reach a question at all.

**Sketch 2 — `/explore` crystallize (prose today).**

Before: the agent holds all findings in context, mentally scores ~5 stage-1
categories × ~5 signals and anti-signals, applies demotion, tiebreaks, maybe
stage 2, and writes a recommendation. Errors in counting or in applying the
"within 1 point" rule are invisible.

After (as a koto state, or even as a prose phase plus a script):
1. Agent writes a short structured artifact `crystallize_findings.md`
   (overall conclusion proceed/don't; core question type; rounds run; one-line
   finding per lead). This is the classifier's state: small, agent-authored
   but constrained, and far smaller than the research files.
2. Deterministic preconditions in a script: qualifying PLAN exists?
   visibility public? (Already specified as "not a signal" preconditions.)
3. Every signal and anti-signal row becomes one noul question, all sent in
   one request (Jev evaluates them in parallel), or answered by the agent as a
   JSON object of booleans when there's no key.
4. `crystallize-score.sh` does count, subtract, demote, tiebreak detection,
   stage-2 trigger, and insufficient-signal fallback, and exits with a code
   per outcome (or captures the winner). Only the named tiebreakers
   ("answered and stopped vs committed") remain as choice questions, and only
   when the script reports a near-tie.
The agent keeps the generative part: writing the recommendation text.

**Sketch 3 — `/work-on` post-implementation routing and loops.**

Before: `analysis` asks for `plan_outcome` (5 values), `issue_type`,
`approach_summary`, and `decisions` in one submission, plus a self-counted
retry cap; `verification` asks the agent to glob-match the diff against the
verification map.

After:
- `issue_type` becomes its own question with state = issue title, labels,
  acceptance criteria and `git diff --name-only` classes (computed: "all
  changed paths are under docs/ or *.md" as a boolean). Escape key
  `unclear` falls back to the agent (and the template already says "use code
  if unsure", so `unclear` maps to `code`).
- A `verification_select` default_action runs a map-matching script that
  writes the selected command list to context and exits 3 on "no match, no
  default" (routing straight to `cannot_verify` without the agent).
- Retry caps move to a script gate that counts prior visits from `koto
  status`/event log (or a koto-native visit counter), so the agent never
  decides "is this the 4th time".
- `plan_outcome` stays with the agent: "is the goal already satisfied by
  current code" needs code reading, not a bounded state.

### 6. Where the shape change pays off with no classifier

Every pattern above except the classifier call itself is a win on today's
koto, which is what makes it safe to land before any key exists:
- Deterministic decisions (section 4a) remove agent turns and remove a class of
  silent mistakes (a mis-set `batch_outcome` or an ignored `partial` today
  advances the run).
- Prefilters mean most runs skip the question entirely (`impact: none`).
- A narrow question with pruned, precomputed facts is easier for the agent to
  answer correctly and cheaper in context than "read the reference and
  classify".
- Atomic answers written as a structured artifact are auditable and
  replayable: the score script can be re-run, and a reviewer can see which
  signal flipped a recommendation.
- Those same agent answers become the labeled dataset for shadow-mode
  evaluation later (connects to the evaluation lead): the question, the state
  it was asked over, and the agent's answer are all in the session log.

The key design consequence: **the question should be declared once in the
template and rendered either to the classifier or to the agent.** With no key,
koto delivers the typed question and its criteria as the directive and the
`accepts` enum; with a key and high confidence, koto fills the value itself;
with a key and low confidence, it falls back to exactly the no-key behavior.
That makes the no-key path the default and the classifier a pure optimization.

## Implications

- The conversion order should mirror the `default_action` campaign: first
  split states so each residual state holds one judgment, then precompute
  facts into gates/captures, then declare the residual judgment as a typed
  question. Steps one and two need no koto engine change.
- koto's routing vocabulary is the real limit. Classifier output has to land
  somewhere `when` can match: an enum-like `gates.<q>.choice` (plus a
  thresholded boolean for noul). Scores and counts need a script or engine
  support for thresholds, because `when` has no numeric comparison.
- Compound judgments (crystallize, decision-tier, fold) need an aggregation
  step between atomic answers and routing. Either shirabe ships scorer scripts
  that read a context key of answers, or koto grows a small declarative
  aggregation (count-true, argmax, margin). The scripts approach works today
  and is testable in shirabe's existing `*_test.sh` style.
- The highest-value classifier targets are in prose skills (`/explore`,
  decision-protocol tiering), which have no koto template. Getting them into
  koto is a prerequisite, or at least a script-backed "answers artifact"
  contract that a future template can wrap.
- State assembly must come from template-declared sources (context keys,
  capture output), never from the agent's free-text `rationale`, both for
  pruning and to limit injection (a PLAN or issue body can say "classify as
  informational").

## Surprises

- `/execute` asks the agent to compute `batch_outcome` when the
  `children-complete` gate already computes `all_success`/`needs_attention`;
  and `plan_completion` openly leaves the `partial` halt to agent vigilance.
  Some "decisions" the main thread makes are not decisions at all.
- `/scope`'s `fold` verdict doesn't change routing (both values go to the same
  targets); it only changes what the agent does next. A classifier answer
  there would save nothing unless the downstream work (delete + carry check)
  also moves out of the agent.
- The crystallize framework is written as an explicit scoring algorithm in
  prose, which is the least reliable way to run arithmetic and the easiest
  thing in shirabe to hand to a script.
- Retry caps ("up to 3 times") are enforced only by the agent's counting,
  with no koto support for visit counts.
- `vars.*` only supports `is_set`, which forces agents to echo variable
  values back as evidence (`pause_decision`).

## Open Questions

- Should aggregation (count, argmax, margin, threshold) live in koto's `when`
  vocabulary or stay in shirabe scripts? Scripts are testable today; engine
  support would make classifier + aggregation one declared unit.
- How should a template declare which context keys form a question's state,
  and can koto enforce a size budget and strip agent-authored free text?
- For multi-answer questions (crystallize's ~40 nouls), does a single
  below-threshold answer send the whole decision back to the agent, or only
  that answer?
- Can visit counting be a koto-native gate field (e.g.
  `gates.__visits__.count`) so retry caps stop being prose?
- Does moving `/explore` onto a koto template belong in this effort, or is a
  scripted "answers artifact + scorer" in a prose phase enough for the first
  step?

## Summary

Only /work-on, /scope and /execute run on koto templates; the most arithmetic-heavy judgments (explore's crystallize scoring, decision-protocol tiering) are prose, and several template "decisions" (execute's batch_outcome, pause_decision, cascade_status, work-on's verification map matching and retry caps) are actually deterministic facts the agent computes by hand.
The transferable pattern is shirabe's own default_action campaign extended one step: split states so each holds one judgment, precompute facts in script gates/captures (koto's `when` can only match by equality, so counts and thresholds must be done before routing), then declare the residual judgment once as a typed question with an escape key over a pruned, template-assembled state, rendered to the classifier with a key and to the agent without one.
Every step except the classifier call improves today's workflows on its own (fewer turns, narrower questions, auditable atomic answers that later serve as shadow-mode labels), shown in sketches for execute's worktree_discipline_check, explore's crystallize, and work-on's issue_type/verification/retry routing.
