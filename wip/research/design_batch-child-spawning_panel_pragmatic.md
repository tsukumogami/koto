# Pragmatic Review: DESIGN-batch-child-spawning

Reviewer lens: YAGNI. The user's problem is "I have a plan with
GitHub issues that depend on each other, and I want koto to handle
the spawning loop instead of writing it in SKILL.md prose."

---

## 1. Line count vs value

The design is 2311 lines with 16 decisions (8 exploration, 8 design).
The walkthrough is another 593 lines. The issue is 30 lines.

That is roughly 100:1 design-to-problem ratio. Several decisions
run 80+ lines documenting alternatives that were never serious
contenders. E1 (flat vs nested) spends 40 lines rejecting an option
nobody proposed. E4 (child naming) has three alternatives, one of
which is "batch UUID plus task index" -- who asked for that?

What could be cut: every "Alternatives considered" section could be
3-5 lines per alternative instead of 8-15. The exploration decisions
(E1-E8) could be summarized in a table with one-line rationale each
-- they were already settled. Repeating them in full design-decision
format doubles the reading cost for zero new information.

Conservative estimate: the doc could deliver the same content in
~800 lines.

## 2. Decisions that could be deferred to implementation

**E4 (child naming: `<parent>.<task>`)** -- This is an obvious
implementation choice. There's exactly one sensible option. It didn't
need 50 lines of analysis.

**E6 (CLI surface: `@file.json` prefix)** -- A convenience feature.
The agent could inline JSON or the implementer could decide the
syntax at PR time. Deciding it here adds no architectural value.

**E8 (per-task `trigger_rule` is out of scope)** -- A decision to
NOT do something doesn't need its own section. "Out of scope" belongs
in a one-liner in the scope section, not a 40-line decision block.

**Decision 3 (forward-compat `deny_unknown_fields`)** -- This is a
defensive serde annotation. It protects against a niche failure mode
(running a batch template on a pre-batch binary). It could be added
by the implementer as a one-line annotation without design-level
deliberation.

**Decision 7 (how the agent knows what to do)** -- The answer is
"use the existing directive mechanism, no code changes needed." That
is not a decision. It is an observation. It consumed 100 lines.

**Decision 8 (item_schema in response)** -- A UX polish detail. The
implementer would naturally include schema information in the
response. It doesn't affect architecture.

That is 6 of 16 decisions that could have been deferred or omitted.
Only the following truly needed design-level treatment:

- E1 (flat vs nested) -- architectural
- E2 (where batch state lives) -- architectural
- E3 (where scheduler runs) -- architectural
- E5 (failure routing default) -- behavioral contract
- E7 (tasks type vs generic json) -- schema contract
- Decision 1 (materialize_children hook shape) -- the actual feature
- Decision 2 (atomic init) -- correctness
- Decision 4 (template path resolution) -- needed but over-detailed
- Decision 5 (failure/skip/retry mechanics) -- needed but scope concern (see below)
- Decision 6 (observability) -- nice-to-have, not v1

## 3. Features not in the issue

The issue (#129) asks for:

- Submit a task list as evidence
- Each task has name, template, vars, dependencies
- Koto spawns respecting dependency order
- Children don't start until deps are terminal
- Resume works (no re-spawn of completed children)
- children-complete still works
- Failure is surfaced for routing
- Template declares which field contains the task list

Here is what the design adds beyond the issue:

| Feature | In issue? | Essential for v1? | Verdict |
|---------|-----------|-------------------|---------|
| `failure_policy: skip_dependents` with skip-marker synthesis | "Failure surfaced for routing" | Partially. Surfacing failure = yes. Synthesizing skipped-marker state files = no. | **Scope creep.** v1 could just not spawn dependents of failed tasks and report them as "blocked" in the gate output. No synthetic state files needed. |
| `retry_failed` evidence action | No | No | **Scope creep.** Retry is a recovery feature. Ship the happy path and manual-failure path first. Retry can come in v2 when someone actually asks for it. |
| Batch observability on `koto status` and `koto workflows --children` | No | No | **Scope creep.** The scheduler outcome on `koto next` already tells the agent everything. `koto status` extensions are polish. |
| Atomic `init_state_file` | No (pre-existing bug) | Independently valuable but not batch-specific. | **Separate concern.** Ship it as its own fix. Don't bundle it with the batch feature to inflate the "must ship together" surface. |
| `deny_unknown_fields` on SourceState | No | No | **Gold plating.** |
| Template path resolution (Decision 4) | Implied by "template path" in task entry | Sort of. But the two-fallback chain with `template_source_dir` and `submitter_cwd` is over-engineered. | **Over-designed.** Resolve relative to the parent template's directory. Done. One rule, not a fallback chain. |
| `trigger_rule` field reservation | No | No | **Premature.** Don't reserve fields for features you haven't designed. |
| Per-child `outcome` enum with 5 values | "Failure surfaced" | Partially. success/failure/pending covers it. | **Over-specified.** `skipped` and `blocked` as distinct outcomes require the skip-marker machinery that is itself scope creep. |
| `default_template` on hook with compile-time validation | Implied | Nice but not essential. Template could be required per-task in v1. | **Acceptable polish.** |

## 4. Could a simpler `koto batch-spawn` command solve this?

The proposal: `koto batch-spawn <parent> @tasks.json` reads the
task list, spawns children whose deps are met (via existing
`koto init --parent`), and prints what it spawned. The agent calls
it in a loop. No new template fields, no scheduler in handle_next,
no `type: tasks`.

**What this gets right:**

- Zero template schema changes. Existing templates work.
- The agent's loop is trivial: call batch-spawn, drive spawned
  children, call batch-spawn again. Three lines of skill prose.
- Resume is free -- `koto init --parent` already checks `exists()`.
- `children-complete` already works for waiting.
- Failure surfacing already exists in gate output.

**What this gets wrong:**

- The template doesn't declare that it expects a batch. The contract
  between "what this template needs" and "what the agent does" is
  entirely in prose. This is the exact problem the user complained
  about: "the consumer has to own the scheduling."
- No compile-time validation that the template is batch-aware.
- The task list is not in the event log, so resume after a crash
  that destroys the tasks.json file loses the batch definition.

**Verdict:** The batch-spawn command solves 70% of the problem with
5% of the implementation cost. It doesn't solve the declarative
contract problem (the template should say "I expect a batch"), and
it has a fragile resume story. But it's a defensible v0 that could
ship in a week while the full design marinates.

The design's core insight -- that the template should declare
`materialize_children` and koto should own the scheduler -- is
correct. But the design wraps that correct insight in 2000 lines of
machinery for failure routing, retry, observability, and edge cases
that nobody has hit yet.

## 5. Minimum viable batch feature

Keep:

1. **`type: tasks` in accepts** (E7, Decision 1 partial) -- the
   template must be able to declare "I accept a task list." Without
   this, the contract is prose-only.

2. **`materialize_children` hook** (Decision 1 partial) -- just
   `from_field` and `default_template`. Drop `failure_policy`.

3. **Scheduler tick in handle_next** (E3) -- the core spawning
   logic. Classify tasks as ready/blocked/spawned/terminal based on
   disk state. Spawn ready tasks via `koto init --parent`.

4. **Deterministic `<parent>.<task>` naming** (E4) -- free
   idempotency.

5. **`@file.json` on --with-data** (E6) -- practically necessary
   for usability, trivial to implement.

6. **Extend children-complete output** -- add `total` count from
   the batch definition so the agent knows how many tasks exist even
   before they're all spawned. Add per-child entries for un-spawned
   tasks showing them as blocked. Don't need `skipped` outcome or
   `failure_mode` flag.

7. **`scheduler` field on the response** -- spawned/blocked/already
   lists so the agent knows what happened on this tick.

Cut:

- `failure_policy` and skip-dependents machinery (just don't spawn
  dependents of failed children -- report them as "blocked")
- `retry_failed` evidence action (manual rewind per-child works)
- Synthetic skipped-marker state files (no state file = not spawned)
- `failure: bool` and `skipped_marker: bool` on TemplateState
- Batch observability on `koto status` and `koto workflows`
- `deny_unknown_fields`
- `template_source_dir` and `submitter_cwd` (resolve relative to
  parent template directory, period)
- `trigger_rule` reservation
- `item_schema` auto-generation in response (document the schema;
  don't auto-generate it)
- Decision 2 atomic init (ship separately as a bug fix)

This leaves roughly 4 changes:

1. `type: tasks` + `materialize_children` on TemplateState + compiler
2. Scheduler tick in handle_next (spawn ready, skip blocked)
3. `@file.json` prefix on --with-data
4. Extended children-complete output with batch awareness

That is maybe 1500 lines of Rust across 2-3 files, not 2300 lines
of design doc.

---

## Top 3 over-engineering concerns

**1. retry_failed is a feature nobody asked for.** The issue says
"failure of an individual child is surfaced to the parent for routing
decisions." It does not say "and then the parent retries the failed
children automatically." The retry machinery (transitive closure
computation, rewind-vs-delete branching for skipped children,
null-clearing evidence idiom, half-initialized-children repair
pre-pass) is the single largest chunk of novel complexity in this
design, and it solves a problem that has zero users. Ship failure
surfacing. Let someone ask for retry before building it.

**2. Synthetic skipped-marker state files are a consequence of a
failure policy nobody needs yet.** The entire `skipped_marker: bool`
field, the synthetic state file synthesis path, the
`skipped_due_to_dep_failure` state convention, and the corresponding
`outcome: skipped` enum value exist solely to support
`failure_policy: skip_dependents`. Without skip-dependents, a failed
child's dependents simply stay un-spawned (reported as "blocked").
The agent can see the failure in the gate output and decide what to
do. This is simpler, requires zero new state file types, and matches
what the user actually asked for ("failure is surfaced for routing
decisions"). If skip-dependents turns out to be needed, it can be
added later -- it's additive.

**3. The design doc itself is over-engineered.** 2311 lines, 16
decisions, 5 exploration research files, 3 design-phase research
files, a 593-line walkthrough. For a feature that adds a scheduler
tick to handle_next and a new field type to the template schema.
The ratio of analysis to implementation is inverted. Half the
decisions document the absence of a feature (E8, Decision 7) or
settle an implementation detail that doesn't affect the architecture
(E4, E6, Decision 3, Decision 8). The design would be stronger at
a third of its length.
