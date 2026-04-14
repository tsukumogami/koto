# Simulation round 2, pair B3 -- path resolution edge cases under CD14

Focus: verify that Decision 4 as revised by Decision 14 holds under
realistic multi-machine, upgrade, and edge-input scenarios. Each
episode is a reset sub-scenario -- state is not shared across
episodes.

Parent template: `coord` (from `wip/walkthrough/walkthrough.md`),
`failure_policy=skip_dependents`, `default_template=impl-issue.md`.

Throughout: AGENT has no design-doc access. KOTO replies with JSON
consistent with Decision 4 + Decision 14. `[GAP: ...]` marks corners
the revised design does not explicitly speak to.

---

## Episode 1 -- Absent `template_source_dir` (pre-D4 state file)

Setup: `coord` was initialized on the old binary (pre-D4). State
header has no `template_source_dir` field. AGENT has upgraded the
binary, resumed the workflow, and now submits a batch with a relative
`template`.

### Transcript

AGENT:

```
cd /home/dan/src/tsuku/repo   # submitter_cwd
koto next coord --with-data @tasks.json
```

`tasks.json`:

```json
{
  "tasks": [
    {"name": "issue-1", "template": "impl-issue.md",
     "vars": {"ISSUE_NUMBER": "101"}}
  ]
}
```

KOTO (state file header has `template_source_dir: None`):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 1,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "working",
         "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "outcome": "pending", "state": "working"}
    ],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errored": [],
    "warnings": [
      {"kind": "missing_template_source_dir"}
    ],
    "feedback": {
      "entries": {"issue-1": {"outcome": "accepted"}},
      "orphan_candidates": []
    }
  }
}
```

Gloss (CD14 step b'): `template_source_dir` is `None`, so step (b) is
skipped entirely and the resolver falls straight through to step (c)
against `submitter_cwd = /home/dan/src/tsuku/repo`. The file exists
there, the spawn succeeds, but the scheduler records a
`missing_template_source_dir` warning so the agent can see that only
one base was tried.

### Probes

- **Warning visibility.** `scheduler.warnings` is a top-level array
  on the `scheduler` object (sibling to `errored`, `spawned_this_tick`,
  etc.), matching the walkthrough's shape and the
  `SchedulerOutcome::Scheduled.warnings: Vec<SchedulerWarning>` field
  declared in the design (§Components). Typed discriminator via
  `#[serde(tag = "kind", rename_all = "snake_case")]` produces
  `{"kind": "missing_template_source_dir"}`. Clear.
- **Does it affect spawn?** No. CD14's step (b') fires the warning
  *and* falls through to (c). `issue-1` spawns via `submitter_cwd`
  and the child's own `template_source_dir` on the newly created
  state-file header is populated from the resolved absolute path's
  parent directory, which restores the invariant for the child's own
  grandchildren.

### Gaps

- `[GAP: one-shot vs per-tick cadence]`. §Decision 14 says the warning
  fires "once per tick." That deduplicates within a single `koto next`
  invocation, but every subsequent tick on this still-pre-D4 parent
  will re-emit it until the workflow terminates or is migrated. The
  design does not describe a "remember we warned" suppression across
  ticks, and there is no header-rewrite path that would retroactively
  populate `template_source_dir`. Pre-D4 workflows therefore emit the
  warning on every single tick for the rest of their life. Noisy but
  harmless; worth flagging in the skill so agents know to suppress.

---

## Episode 2 -- Stale `template_source_dir` (cross-machine)

Setup: AGENT ran `koto init coord` on machine-A with
`--template /home/alice/project/templates/coord.md`. State file
header captured `template_source_dir: /home/alice/project/templates`.
State file syncs via cloud backend to machine-B, where the user is
`bob` and `/home/alice/...` does not exist. AGENT re-ticks on
machine-B from `/home/bob/project/repo`.

### Transcript

AGENT (on machine-B):

```
cd /home/bob/project/repo
koto next coord --with-data @tasks.json
```

KOTO (`Path::new("/home/alice/project/templates").exists() == false`):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [ /* as Episode 1 */ ],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "outcome": "pending", "state": "working"}
    ],
    "already": [], "blocked": [], "skipped": [],
    "errored": [],
    "warnings": [
      {"kind": "stale_template_source_dir",
       "path": "/home/alice/project/templates"}
    ],
    "feedback": {
      "entries": {"issue-1": {"outcome": "accepted"}},
      "orphan_candidates": []
    }
  },
  "sync_status": "fresh",
  "machine_id": "machine-B"
}
```

Gloss (CD14 "Present-but-stale" branch): probe `Path::exists()` on the
captured dir; it returns false, so emit
`SchedulerWarning::StaleTemplateSourceDir { path }` and fall through
to `submitter_cwd = /home/bob/project/repo`. The template resolves
there and spawns succeed.

### Probes

- **Field population.** The variant payload per §Components is
  `StaleTemplateSourceDir { path: String }` -- only the captured
  stale path is carried. The variant does NOT carry `machine_id` or
  `falling_back_to`. The agent can derive:
  - `machine_id` from the top-level cloud-mode field (`machine_id:
    "machine-B"`).
  - The fallback target from its own `cwd` (or `submitter_cwd` echoed
    in the next `koto next` it issues against a child).
- **Deduplication.** Design says "Deduplicated per
  `template_source_dir` value per tick" (§Decision 14). That applies
  *within* a tick, not across ticks. On every subsequent tick on
  machine-B this warning re-fires -- there is no "seen this stale
  path already" memory across ticks.

### Gaps

- `[GAP: enrichment]`. The warning does not carry
  `falling_back_to` or `machine_id` inline. For agents producing
  human-readable diagnostics, synthesizing "on machine-B, falling
  back from /home/alice to /home/bob" requires combining three
  sources (warning.path, top-level machine_id, external cwd
  knowledge). Either enrich the warning variant or document the
  recomposition recipe in the koto-user skill. Low risk but rough.
- `[GAP: no child-side propagation]`. When `coord.issue-1` spawns, its
  *own* header captures `template_source_dir` from the resolved
  absolute path on machine-B (under `/home/bob/...`). So the child
  inherits machine-B's layout for its own grandchildren. If the
  state then cloud-syncs back to machine-A where `/home/bob` doesn't
  exist, the child re-experiences Episode 2 at its own next tick.
  The design hints at this ("captured at init time") but doesn't
  walk through the pingpong case explicitly.
- `[GAP: repeated warnings suppress or not?]`. CD14 says "once per
  tick"; it does not say "once per workflow lifetime." For long-
  lived batch parents synced across machines, every tick from a
  machine where the captured path is absent will noisily repeat the
  warning. Agents MUST implement their own deduplication -- note in
  koto-user.

---

## Episode 3 -- Absolute template paths under cloud sync

Setup: AGENT on machine-A submits a batch where one task uses an
absolute path.

### Transcript

AGENT:

```
koto next coord --with-data @tasks.json
```

`tasks.json`:

```json
{
  "tasks": [
    {"name": "issue-custom",
     "template": "/home/alice/project/templates/custom-impl.md",
     "vars": {"ISSUE_NUMBER": "42"}}
  ]
}
```

KOTO (machine-A, absolute path exists):

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.issue-custom"],
    "materialized_children": [
      {"name": "coord.issue-custom", "outcome": "pending",
       "state": "working"}
    ],
    "already": [], "blocked": [], "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {"issue-custom": {"outcome": "accepted"}},
      "orphan_candidates": []
    }
  }
}
```

Gloss: Decision 4 step (a) -- absolute paths pass through
unmodified. No path-composition fallback applies, so neither of
Decision 14's warnings fire. The child's own header captures
`template_source_dir = /home/alice/project/templates` (the parent
directory of the absolute template path).

### Probes

- **Warns on absolute paths under cloud sync?** No. Neither D4 nor
  CD14 emits a warning when the absolute path exists on the
  originating machine. CD14's warnings only fire on the fallback
  path, which absolute inputs skip entirely.

### Gaps

- `[GAP: absolute-path cross-machine footgun]`. This is the same
  concern round 1 pair 3c raised (Scenario 1). When the state file
  syncs to machine-B, the child's header still carries
  `template_source_dir: /home/alice/project/templates`. On machine-B,
  that directory is absent. The child's next tick triggers
  `stale_template_source_dir` + fallthrough to machine-B's
  `submitter_cwd`. If the submitter happens to run from a directory
  where `custom-impl.md` exists, the child silently uses a *different*
  file than the parent intended on machine-A. This is the most
  dangerous cross-machine mode: no error, no warning at submission,
  only a post-hoc stale warning on the child. The design
  acknowledges this ("Cross-machine portability documented +
  runtime warning" in CD14), but Episode 3 shows that for *absolute*
  inputs the warning fires on the child's tick, not the parent's
  batch tick -- so the agent only learns something is wrong well
  after the batch has "successfully" spawned. Recommend: add a
  submission-time warning when a task entry uses an absolute path
  whose parent directory differs from the parent's
  `template_source_dir`, flagging cross-machine drift risk. Out of
  CD14's stated scope but a natural follow-up.

---

## Episode 4 -- `..` path escape to a nonexistent file

Setup: AGENT submits `template: "../../../etc/shadow"`. The resolved
file does not exist (this machine does not actually have an
accessible /etc/shadow readable by this user, or the ../ ascent goes
above `/`).

### Transcript

AGENT:

```
cd /home/dan/src/tsuku/repo/sub
koto next coord --with-data '{"tasks": [{"name": "bad",
  "template": "../../../etc/shadow", "vars": {"ISSUE_NUMBER": "0"}}]}'
```

KOTO:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 0, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 1,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.bad", "state": null, "complete": false,
         "outcome": "spawn_failed",
         "spawn_error": {
           "task": "bad",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/alice/project/templates/../../../etc/shadow",
             "/home/dan/src/tsuku/repo/sub/../../../etc/shadow"
           ],
           "message": "Template not found at any configured base"
         }}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [],
    "already": [], "blocked": [], "skipped": [],
    "errored": [
      {
        "task": "bad",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/project/templates/../../../etc/shadow",
          "/home/dan/src/tsuku/repo/sub/../../../etc/shadow"
        ],
        "message": "Template not found at any configured base"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {"bad": {"outcome": "errored",
                           "kind": "template_not_found"}},
      "orphan_candidates": []
    }
  }
}
```

Gloss: D4 "..-permitted" means no sandbox rejection. CD14 step (d)
returns `TemplateNotFound { task, paths_tried }`. Path joins are
lexical (no canonicalization), so `paths_tried` echoes the unresolved
`..` strings as joined against each base.

### Probes

- **Sensitivity of `paths_tried`.** The array echoes both the
  parent's `template_source_dir` and the `submitter_cwd`, both of
  which live under the user's home directory. §Security
  Considerations ("Error bodies echo agent-submitted content") notes
  these strings are as sensitive as the state file itself. For
  absolute-path inputs echoed verbatim, agents pasting error bodies
  into bug reports leak directory structure. Already documented in
  §Security Considerations (round 1 addition); CD14 doesn't change
  the surface.
- **Canonicalization?** §Decision 4 canonicalizes `template_source_dir`
  at `handle_init` time but does NOT specify that the scheduler
  canonicalizes the *resolved* paths it tries. `paths_tried` therefore
  contains literal `..` segments. This is a small ergonomics issue
  (hard to read) but not a correctness issue.

### Gaps

- `[GAP: canonicalized paths_tried]`. Minor: `paths_tried` containing
  `.../../../..` is harder to read than a canonicalized form. Not a
  design defect, but an implementation note for the koto-user skill
  if agents produce human summaries.

---

## Episode 5 -- File exists, fails to compile

Setup: `impl-broken.md` exists in `template_source_dir` but has a
frontmatter error (e.g., missing required `initial_state` field).

### Transcript

AGENT:

```
koto next coord --with-data '{"tasks": [{"name": "issue-bad",
  "template": "impl-broken.md", "vars": {"ISSUE_NUMBER": "7"}}]}'
```

KOTO (file found at
`/home/alice/project/templates/impl-broken.md`, compile fails):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 0, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 1,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-bad", "state": null, "complete": false,
         "outcome": "spawn_failed",
         "spawn_error": {
           "task": "issue-bad",
           "kind": "template_compile_failed",
           "path": "/home/alice/project/templates/impl-broken.md",
           "compile_error": "missing required field: initial_state (at line 3)",
           "message": "Template compilation failed"
         }}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [],
    "already": [], "blocked": [], "skipped": [],
    "errored": [
      {
        "task": "issue-bad",
        "kind": "template_compile_failed",
        "path": "/home/alice/project/templates/impl-broken.md",
        "compile_error": "missing required field: initial_state (at line 3)",
        "message": "Template compilation failed"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {"issue-bad": {"outcome": "errored",
                                 "kind": "template_compile_failed"}},
      "orphan_candidates": []
    }
  }
}
```

Gloss: CD14 split achieves its goal -- `kind` discriminator carries
`template_compile_failed` with a `path` (single, resolved) and a
`compile_error` payload. `paths_tried` is absent (it's `Option<...>`
on `TaskSpawnError`), correctly because the file WAS found.

### Probes

- **Variant distinction clear?** Yes. `errored[i].kind` is a tagged
  snake_case enum: `template_not_found` vs `template_compile_failed`
  are visually distinct and the payload shape differs
  (`paths_tried` vs `path`+`compile_error`). The gate-row
  `spawn_error.kind` mirrors the same discriminator. An agent can
  pattern-match without introspection.
- **feedback.entries.kind is a string not an enum variant.** Minor
  observation: the walkthrough writes `{"outcome": "errored", "kind":
  "template_not_found"}`, i.e., `kind` is flat on the entry. For
  compile failures the same flat `kind` applies; the
  `path`/`compile_error` detail lives on `scheduler.errored[]` not on
  `feedback.entries`. Agents must cross-reference the two arrays by
  task name to get full detail. Design is consistent with walkthrough
  §1087-1112 -- no new gap here.

### Gaps

- None for CD14's split itself. The design handles this case cleanly.

---

## Episode 6 -- DAG depth boundary

### 6a: Linear chain of 50 succeeds

```
tasks: issue-1 -> issue-2 -> ... -> issue-50
(issue-N.waits_on = ["issue-(N-1)"] for N >= 2)
```

KOTO: accepts. Depth (node count along longest root-to-leaf) = 50.
Limit is 50. OK.

### 6b: Linear chain of 51 rejects

AGENT submits 51-task linear chain. KOTO:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Longest dependency chain has 51 tasks; limit is 50.",
    "details": [{"field": "tasks", "reason": "limit_exceeded_depth"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "limit_exceeded_depth",
      "limit": 50,
      "actual": 51
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: §Components defines `InvalidBatchReason::LimitExceededDepth
{ limit, actual }`. `LimitKind::Depth` is the sibling enum variant
used when surfacing as `BatchError::LimitExceeded`, but the
whole-submission rejection path routes through
`InvalidBatchReason::LimitExceededDepth` -- a subtle but documented
distinction (§2907, §2925).

### 6c: Chain of 3 plus unrelated singleton

```
A -> B -> C    (depth 3)
D              (depth 1, no deps)
```

Total nodes = 4. Longest root-to-leaf path nodes = 3. KOTO: accepts.
The check is NOT total node count.

### Probes

- **Verification.** CD14 explicitly says "Depth is the node count
  along the longest root-to-leaf path." Scenario 6c confirms by
  construction: 4 total tasks, depth 3, under the 50 limit. Pass.
- **Off-by-one.** 6a/6b confirm the design's "node count" intuition --
  "I wrote 51 tasks in a chain and got rejected" matches the error
  message "51 tasks; limit is 50," no off-by-one confusion.
- **Tie-breaking.** A DAG may have multiple longest paths (e.g.,
  diamond with both arms equal length). CD14 does not specify
  tie-breaking because depth is a length, not a path selection.
  Correct.

### Gaps

- `[GAP: pathological shapes]`. A wide-but-shallow DAG (e.g.,
  1000 tasks, all depending on one root) has depth 2, nodes 1001.
  `LimitKind::Tasks` (limit 1000) catches that case. A deep-but-narrow
  DAG (50 linear + 950 unrelated singletons) has depth 50, 1000 tasks
  -- accepted, but the scheduler's per-tick `backend.list()` +
  re-classification cost grows with `Tasks` not `Depth`. Out of scope
  for CD14 (hard limits already handle it), but worth noting that
  depth alone doesn't bound scheduler cost -- `Tasks` does.

---

## Episode 7 -- `default_template` deleted between compile and spawn

Setup: parent `coord.md` declares `default_template: "impl-issue.md"`.
At `koto init` / template compile time `impl-issue.md` existed and
passed Decision 8's compile-time validation. Between that compile
and the batch submission, someone `rm`'d the file. AGENT now
submits a batch that relies on the default (no per-task `template`
override).

### Transcript

AGENT:

```
koto next coord --with-data '{"tasks": [
  {"name": "issue-1", "vars": {"ISSUE_NUMBER": "1"}},
  {"name": "issue-2", "template": "impl-override.md",
   "vars": {"ISSUE_NUMBER": "2"}}
]}'
```

KOTO (`impl-issue.md` is gone; `impl-override.md` exists):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 2, "completed": 0, "pending": 1,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "spawn_failed": 1,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": null, "complete": false,
         "outcome": "spawn_failed",
         "spawn_error": {
           "task": "issue-1",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/alice/project/templates/impl-issue.md",
             "/home/dan/src/tsuku/repo/impl-issue.md"
           ],
           "message": "Template not found at any configured base"
         }},
        {"name": "coord.issue-2", "state": "working",
         "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-2"],
    "materialized_children": [
      {"name": "coord.issue-2", "outcome": "pending", "state": "working"}
    ],
    "already": [], "blocked": [], "skipped": [],
    "errored": [
      {
        "task": "issue-1",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/project/templates/impl-issue.md",
          "/home/dan/src/tsuku/repo/impl-issue.md"
        ],
        "message": "Template not found at any configured base"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "issue-1": {"outcome": "errored", "kind": "template_not_found"},
        "issue-2": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: Per CD14, this is per-task `TemplateNotFound`, not a whole-
submission halt. `issue-2` (which carried its own override) spawns
successfully. `issue-1` (which fell through to the default) errors.
Sibling independence per CD14 R1 holds.

### Probes

- **Does the error payload distinguish "template came from the
  default"?** No. `TaskSpawnError` carries `task`, `kind`,
  `paths_tried`, `message`. There is no `template_source` field
  distinguishing "task-supplied" vs "default_template". The agent can
  infer it by cross-referencing the original submission -- the task
  entry in the submitted payload had no `template`, so the default
  was used. The `paths_tried` will contain whatever was resolved from
  the default name (`impl-issue.md`), which gives an indirect signal.

### Gaps

- `[GAP: default vs override attribution]`. The design does not tag
  the error with "this path came from `default_template`." Agents
  rendering recovery suggestions ("did you mean to override
  `impl-issue.md`?" vs "the parent template's default is wrong")
  cannot tell without consulting the original submission and the
  compiled parent template. Minor -- agents usually have the
  submission in scope -- but a `resolved_from: "default_template" |
  "task.template"` field on `TaskSpawnError` would be a
  straightforward addition and would land the diagnostic clarity
  CD14 aspired to for the not-found vs compile-failed split.

---

## Section 2: Cross-episode findings

### Shape consistency: SchedulerWarning vs TaskSpawnError

The two envelopes are *structurally different* and that is correct:

| Envelope | Where it lives | Shape |
|---|---|---|
| `SchedulerWarning` | `scheduler.warnings[]` | `{kind, ...payload}` (tagged enum, some variants carry a single `path` field). Non-fatal. Does not stop spawn. |
| `TaskSpawnError` | `scheduler.errored[]` AND `children[].spawn_error` | `{task, kind, paths_tried?, path?, compile_error?, message}`. Per-task, fatal to that task only. |

Episodes 1-2 confirm warnings do not gate spawn. Episodes 4, 5, 7
confirm errors are per-task and siblings keep spawning. CD14's
commitment holds.

### Discriminator clarity

`SpawnErrorKind` -- `template_not_found`, `template_compile_failed`,
`collision`, `backend_unavailable`, `permission_denied`, `io_error` --
is a flat snake_case enum. Agents can match on `kind` alone. The
variant-specific fields (`paths_tried` for not-found, `path` +
`compile_error` for compile-failed) use `Option<...>` on the outer
`TaskSpawnError` so the JSON shape adapts without requiring a nested
tagged union. Clean.

### Cross-machine surface -- the biggest remaining risk

CD14 added the `StaleTemplateSourceDir` warning, which handles
*relative* paths cleanly (Episode 2). But absolute paths (Episode 3)
have a subtler failure mode: the parent batch succeeds on the
originating machine, the child's own header captures an originating-
machine-layout `template_source_dir`, and the staleness only
manifests when the *child* ticks on the receiving machine. By then
the batch response has long since been consumed. CD14 doesn't address
this, and it is not a CD14 contradiction -- it is a *new* cross-
machine surface that emerges from the interaction of D4 (absolute
pass-through) and the child's own header capture. Worth a follow-up.

### Pre-D4 state files: permanent warning emission

Episode 1 also surfaces a lifecycle gap: a workflow initialized pre-
D4 has no header field, and nothing in the design rewrites the header
on the first post-upgrade tick. So every subsequent tick emits
`missing_template_source_dir` forever. CD14 mentioned `koto session
retarget` as a future extension; that or an in-place header patch on
first tick would resolve this. Skill docs should warn agents to
expect repeated warnings and treat them as informational, not
actionable, after the first tick.

### Default-template attribution

Episode 7 shows CD14's not-found/compile-failed split doesn't
capture *whether* the resolved path came from the task override or
the parent's `default_template`. Not a breaking gap but a clarity
improvement opportunity aligned with CD14's stated goal ("agents
programmatically distinguish 'my path is wrong' from 'my template
file is broken'" -- extend to "'my override is wrong' vs 'the
default is wrong'").

### Summary of gaps to surface in round-2 synthesis

1. **Repeated-warning noise.** Pre-D4 + stale-dir warnings re-fire
   every tick. Document expected cadence in koto-user skill, or add
   cross-tick dedup / header patching.
2. **Absolute-path cross-machine drift.** Episode 3 -- absolute
   template inputs on machine-A produce no submission-time warning
   even when they reference paths absent on sync peers. Consider
   a submission-time warning when a task's absolute `template` lies
   outside the parent's `template_source_dir`.
3. **Default-template attribution on errors.** Episode 7 -- add
   `resolved_from` to `TaskSpawnError` so agents can render
   "override failed" vs "default failed" diagnostics.
4. **SchedulerWarning enrichment for stale-dir.** Episode 2 -- the
   stale warning carries only `path`; `machine_id` and
   `falling_back_to` must be recomposed by the agent. Minor UX.
5. **`paths_tried` canonicalization.** Episode 4 -- consider
   canonicalizing the strings to remove `..` segments before
   echoing them back.

None of the above are CD14 contradictions. CD14's decisions hold up
under every episode tested. The gaps are follow-up opportunities --
either small enrichments or cross-decision interactions D4+CD14
don't fully speak to.
