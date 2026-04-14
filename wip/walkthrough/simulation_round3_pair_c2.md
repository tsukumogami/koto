# Simulation round 3, pair C2 -- path-resolution polish verification

Focus: verify Round 3's polish fixes against Decision 4 + Decision 14.
Round 2 Pair B3 left five follow-ups; Round 3 fixed four of them and
deferred one. This simulation exercises the revised envelopes across
the same kinds of scenarios B3 exposed.

Round 3 changes under test:

- `TaskSpawnError.template_source: Option<TemplateSource>` with
  `{Override, Default}` variants.
- `SchedulerWarning::StaleTemplateSourceDir { path, machine_id,
  falling_back_to }` -- enriched with two new fields.
- `TaskSpawnError.paths_tried` is canonicalized (no `..` segments).
- `TaskSpawnError.compile_error: Option<CompileError>` is a typed
  struct `{kind, message, location: Option<{line, column}>}` rather
  than a free string.
- Pre-D4 warning noise: documented as known limitation; `koto session
  rehome` deferred to v1.1.
- `BatchError::LimitExceeded { which, limit, actual, task }` is
  top-level, not nested inside `InvalidBatchReason`.

Parent template: `coord` from `wip/walkthrough/walkthrough.md`,
`failure_policy=skip_dependents`, `default_template=impl-issue.md`.

Throughout: AGENT has no design-doc access. KOTO responds with JSON
consistent with the revised design. `[GAP: ...]` marks genuine
ambiguities. `[OK]` flags items Round 3 explicitly fixed.

---

## Episode 1 -- Pre-D4 state file, repeated warnings

Setup: `coord` was initialized against the pre-D4 binary. After
upgrade, the state header has `template_source_dir: None`. AGENT
ticks the parent twice in a row.

### Transcript

AGENT (tick 1):

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

KOTO (tick 1):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 1,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "working",
         "complete": false, "outcome": "pending",
         "ready_to_drive": true}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "outcome": "pending", "state": "working"}
    ],
    "already": [], "blocked": [], "skipped": [],
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

AGENT (tick 2, a few minutes later, driving the parent again after
`issue-1` completed):

```
koto next coord
```

KOTO (tick 2 -- same header, no new batch evidence, but scheduler
still runs and re-probes the absent header):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": false,
  "blocking_conditions": [...],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.issue-1", "outcome": "success", "state": "done"}
    ],
    "already": [], "blocked": [], "skipped": [], "errored": [],
    "warnings": [
      {"kind": "missing_template_source_dir"}
    ],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

Gloss: CD14 "Known limitations" commits to this behavior: there is
no header-rewrite primitive in v1, so every scheduler tick on a
pre-D4 parent emits `missing_template_source_dir`. CD14 names the
future fix (`koto session rehome`) but defers it.

### Probes

- **Warning payload shape.** `{"kind": "missing_template_source_dir"}`
  -- the variant is a unit variant in `SchedulerWarning`. No
  payload, consistent with design line 3067. [OK]
- **In-place mitigation?** No. Design §Decision 14 "Known
  limitations" says "There is no header-rewrite primitive in v1."
  The warning re-fires every tick for the lifetime of the parent.
- **Does the warning tell the agent what to do?** The variant is
  discriminator-only -- no `hint` or `remediation` field. The agent
  has to consult the skill docs to learn that this warning is
  informational-only on pre-D4 parents. `[GAP: remediation
  discoverability]` -- a `hint: "pre_d4_state_file"` string on the
  variant would tell agents to suppress locally without consulting
  out-of-band docs. Low priority since the koto-user skill can
  document the cadence.

### Gaps

- `[GAP: per-workflow suppression]`. Design's dedup is "once per
  tick" (design line 2629). That doesn't help a long-lived parent
  that ticks hundreds of times -- each tick re-emits the warning,
  clogging the transcript. Design defers the real fix to v1.1.
  Agents must suppress locally (keep a set of seen-warning signatures
  keyed by parent name).
- `[GAP: no onward migration path]`. A parent upgraded mid-workflow
  has no header-rewrite primitive. Every descendant spawned under
  this parent inherits a correctly populated header (the child's
  init captures `template_source_dir` fresh), but the parent itself
  warns forever. Acceptable per CD14's "Alternatives considered" --
  error-at-submission would break backward compat.

---

## Episode 2 -- Cross-machine stale `template_source_dir`

Setup: AGENT ran `koto init coord` on machine-A with `--template
/home/alice/proj/templates/coord.md`. State header captured
`template_source_dir: /home/alice/proj/templates`. State synced via
cloud backend to machine-B where the user is `bob` and
`/home/alice/...` does not exist. AGENT re-ticks on machine-B from
`/home/bob/proj`.

### Transcript

AGENT (on machine-B):

```
cd /home/bob/proj
koto next coord --with-data @tasks.json
```

KOTO (`Path::new("/home/alice/proj/templates").exists() == false`):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 1, "completed": 0, "pending": 1,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.issue-1", "state": "working",
         "complete": false, "outcome": "pending",
         "ready_to_drive": true}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-1"],
    "materialized_children": [
      {"name": "coord.issue-1", "outcome": "pending", "state": "working"}
    ],
    "already": [], "blocked": [], "skipped": [], "errored": [],
    "warnings": [
      {"kind": "stale_template_source_dir",
       "path": "/home/alice/proj/templates",
       "machine_id": "machine-B",
       "falling_back_to": "/home/bob/proj"}
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

Gloss: CD14 "Present-but-stale" branch, enriched payload. All three
fields agent needs (`path`, `machine_id`, `falling_back_to`) are
carried inline. The top-level `machine_id` field (Decision 12 Q5)
still appears; the warning's `machine_id` duplicates it so agents
processing `scheduler.warnings` in isolation have enough context.

### Probes

- **Field population.** Design line 3071-3076 declares the variant
  shape. All three fields present and useful. [OK]
- **Redundant `machine_id`?** Yes -- it's in both the warning and the
  top-level response. Intentional per the design comment on line
  3068-3070 ("so agents don't need to recompose the context from
  other fields"). The warning becomes self-contained and routable
  through logs/alerting without the envelope.
- **`falling_back_to` type.** `PathBuf` in Rust, serializes as a
  string in JSON. No `Option<...>` wrapper because there's always a
  fallback target -- if `submitter_cwd` were also absent we'd fall
  through to `TemplateNotFound`, not emit this warning.
- **Non-cloud machines.** Outside `CloudBackend`, `machine_id` is
  `None` and omitted via `serde(skip_serializing_if)`. The warning
  shape in a local-only tick would carry only `path` and
  `falling_back_to`. Design line 3073-3074 confirms.

### Gaps

- `[GAP: per-tick dedup survives]`. The "once per tick" rule still
  holds. If the parent ticks 100 times across weeks with a persistent
  stale header, each tick re-emits the warning. Design §Decision 14
  does not specify cross-tick memoization. Agents must dedupe
  locally, same footgun as Episode 1.
- `[GAP: no remediation hint on the warning]`. Similar to Episode 1:
  no inline pointer at "future `koto session rehome`" or "re-init on
  this machine." The agent must know to consult the koto-user skill.

---

## Episode 3 -- `TaskSpawnError.template_source` attribution

Setup: the parent declares `default_template: impl-issue.md`. AGENT
submits two tasks. One explicitly overrides to `override.md`. The
other relies on the default. Both template files are missing.

### Transcript

AGENT:

```
cd /home/dan/src/tsuku/repo
koto next coord --with-data '{"tasks": [
  {"name": "with-override", "template": "override.md",
   "vars": {"ISSUE_NUMBER": "1"}},
  {"name": "inherits-default", "vars": {"ISSUE_NUMBER": "2"}}
]}'
```

KOTO (neither template resolves; header
`template_source_dir: /home/alice/proj/templates`):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": true,
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 2, "completed": 0, "pending": 0, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 2,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": true,
      "needs_attention": false,
      "children": [
        {"name": "coord.with-override", "state": null,
         "complete": false, "outcome": "spawn_failed",
         "ready_to_drive": false,
         "spawn_error": {
           "task": "with-override",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/alice/proj/templates/override.md",
             "/home/dan/src/tsuku/repo/override.md"
           ],
           "template_source": "override",
           "message": "Template not found at any configured base"
         }},
        {"name": "coord.inherits-default", "state": null,
         "complete": false, "outcome": "spawn_failed",
         "ready_to_drive": false,
         "spawn_error": {
           "task": "inherits-default",
           "kind": "template_not_found",
           "paths_tried": [
             "/home/alice/proj/templates/impl-issue.md",
             "/home/dan/src/tsuku/repo/impl-issue.md"
           ],
           "template_source": "default",
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
        "task": "with-override",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/proj/templates/override.md",
          "/home/dan/src/tsuku/repo/override.md"
        ],
        "template_source": "override",
        "message": "Template not found at any configured base"
      },
      {
        "task": "inherits-default",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/proj/templates/impl-issue.md",
          "/home/dan/src/tsuku/repo/impl-issue.md"
        ],
        "template_source": "default",
        "message": "Template not found at any configured base"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {
        "with-override": {"outcome": "errored", "kind": "template_not_found"},
        "inherits-default": {"outcome": "errored", "kind": "template_not_found"}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: CD14 R1 per-task failures. Both tasks produce independent
`TaskSpawnError` entries. The new `template_source` field is the
Round 3 fix: `"override"` for the task that supplied `template`
explicitly; `"default"` for the task that inherited from the hook's
`default_template`. Payload snake_case per serde line 3103. [OK]

### Probes

- **Both payloads distinguishable?** Yes. The `template_source`
  discriminator is a sibling of `kind`. An agent can pattern-match
  `(kind=template_not_found, template_source=default)` to render
  "the hook's default_template `impl-issue.md` is missing -- fix the
  parent template or override in each task" vs
  `(kind=template_not_found, template_source=override)` to render
  "your `override.md` isn't present -- check the filename." [OK]
- **Could both fail with different sources in one batch?** Yes --
  exactly this episode. The flat enum + independent per-task error
  accumulation handles mixed cases cleanly.
- **Absence of `template_source`.** `Option<TemplateSource>` --
  serde skips serialization when None. When could it be None? The
  design doesn't explicitly say, but a reasonable interpretation:
  when the error is not template-related (`BackendUnavailable`,
  `IoError`, `Collision` -- none of which know which way the path
  was chosen), the field is simply absent.

### Gaps

- `[GAP: `default_template` ambiguity at compile time]`. Design line
  3092-3095 says "came from the agent's `template` field or was
  inherited from the hook's `default_template`." But what if the
  hook has no `default_template` and the task omits `template`? D4
  doesn't allow that -- compile-time validation (Decision 3/E series)
  rejects it. Consistent with the field being binary.
- `[GAP: absolute-path case]`. When the task's `template` is absolute
  (`/abs/path/foo.md`), `template_source` should presumably be
  `override` -- the task supplied it. Design doesn't explicitly say
  but the interpretation is unambiguous.

---

## Episode 4 -- Canonicalized `paths_tried`

Setup: AGENT runs from `/home/dan/src/tsuku/repo/sub` with
`template_source_dir = /home/alice/proj/templates`. Submits
`template: "../helper/impl.md"`. Neither base resolves.

### Transcript

AGENT:

```
cd /home/dan/src/tsuku/repo/sub
koto next coord --with-data '{"tasks": [
  {"name": "bad", "template": "../helper/impl.md",
   "vars": {"ISSUE_NUMBER": "0"}}
]}'
```

KOTO (both resolutions fail; canonicalization strips `..`):

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [],
    "already": [], "blocked": [], "skipped": [],
    "errored": [
      {
        "task": "bad",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/proj/helper/impl.md",
          "/home/dan/src/tsuku/repo/helper/impl.md"
        ],
        "template_source": "override",
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

Gloss: Round 3 spec (design lines 2694-2698 and line 3087-3089)
explicitly canonicalizes before serialization. The `..` ascent
resolves lexically, dropping the last segment of each base and
adding `helper/impl.md`. [OK]

### Probes

- **Readability.** Both bases appear as canonical absolute paths.
  `/home/alice/proj/helper/impl.md` instead of
  `/home/alice/proj/templates/../helper/impl.md`. [OK]
- **Which paths appear?** Both fallbacks tried: step (b)
  against `template_source_dir` and step (c) against `submitter_cwd`,
  matching §Decision 14's order. Exactly two entries unless
  `template_source_dir` is absent (Episode 1 would produce a
  single-entry `paths_tried`).
- **Canonicalization mode.** The design says "canonicalized (`..`
  segments resolved) before serialization." This is lexical
  normalization, NOT filesystem canonicalization via `canonicalize()`
  -- crucial because `canonicalize()` fails when the target doesn't
  exist, and `TemplateNotFound` fires precisely because it doesn't
  exist. Implementation must use a lexical normalizer
  (`std::path::Path::components()` walk or a helper like
  `path_clean::clean`). `[GAP: canonicalization kind]` -- design
  doesn't explicitly say lexical vs filesystem, but filesystem is
  impossible for this path.

### Gaps

- `[GAP: symlink resolution]`. Lexical canonicalization does NOT
  resolve symlinks. If `/home/alice/proj/templates` is itself a
  symlink, `paths_tried` shows the unresolved form. Probably
  desirable (preserve user's mental model) but worth noting.
- `[GAP: escape to `/`]`. `template: "../../../../foo.md"` from a
  shallow base can lexically resolve to a path above `/`. Unix path
  normalization caps at `/`, so the result is `/foo.md`. Not a bug,
  just a visual surprise.

---

## Episode 5 -- Absolute-path cross-machine drift

Setup: AGENT on machine-A submits a task with an absolute template
path. State file syncs to machine-B where the path doesn't exist.
Machine-B's scheduler attempts the spawn.

### Transcript

AGENT (on machine-A at submission time):

```
koto next coord --with-data '{"tasks": [
  {"name": "issue-custom",
   "template": "/home/alice/proj/templates/custom-impl.md",
   "vars": {"ISSUE_NUMBER": "42"}}
]}'
```

KOTO (machine-A, absolute path exists): spawns cleanly. No warning.
The task's `template` rides the evidence log unchanged.

Then state syncs to machine-B. AGENT (now on machine-B) runs `koto
next coord` again, which attempts to respawn the child (hypothetical
retry scenario, or the original spawn lost the race). The scheduler
revisits the absolute path:

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [],
    "already": [], "blocked": [], "skipped": [],
    "errored": [
      {
        "task": "issue-custom",
        "kind": "template_not_found",
        "paths_tried": [
          "/home/alice/proj/templates/custom-impl.md"
        ],
        "template_source": "override",
        "message": "Template not found at any configured base"
      }
    ],
    "warnings": [],
    "feedback": {
      "entries": {"issue-custom": {"outcome": "errored",
                                    "kind": "template_not_found"}},
      "orphan_candidates": []
    }
  },
  "sync_status": "fresh",
  "machine_id": "machine-B"
}
```

Gloss: design line 2685-2692 "Absolute child-template paths bypass
the path-resolution warnings at submission time." `paths_tried`
carries only the absolute path (no fallbacks are attempted because
D4 step (a) passes absolute through unmodified). `template_source:
"override"` correctly identifies that the agent supplied the path.

### Probes

- **Submission-time signal?** None. Per design line 2685, "Known
  limitation in v1: cross-machine portability checks cover
  header-based relative resolution only." Confirmed: the parent's
  batch submission on machine-A succeeds without warning.
  `StaleTemplateSourceDir` does NOT fire here because it only fires
  when the path probe is on `template_source_dir` at scheduler
  start, not on individual task entries.
- **Agent learns late.** The scheduler tick on machine-B produces
  per-task `TemplateNotFound`. The agent's recovery reads
  `template_source: "override"` + the single-element `paths_tried`
  and can render "your absolute path `/home/alice/...` doesn't
  exist on this machine."
- **No submission-time cross-machine lint.** CD14 "Alternatives
  considered" rejected adding a submission-time warning when a
  task's absolute template differs from the parent's
  `template_source_dir` (noted as "Real fix but out of Decision 14's
  scope"). So Round 3 confirms the scope boundary: submission
  accepts, runtime scheduler surfaces the failure. [OK as known
  limitation]

### Gaps

- `[GAP: absolute-path cross-machine footgun persists]`. Round 3
  did NOT add a submission-time warning for cross-machine-fragile
  absolute paths. Decision documents this as a known limitation
  (design line 4284-4295). The footgun is bounded: the scheduler
  tick produces a clean `TemplateNotFound` with `template_source:
  "override"` on the receiving machine, so agents can render
  targeted recovery. Not silently wrong -- just late.

---

## Episode 6 -- `TemplateCompileFailed` with typed `CompileError`

Setup: `impl-broken.md` resolves successfully under
`template_source_dir`, but its YAML frontmatter is malformed (e.g.,
a tab character where only spaces are allowed, line 3 column 5).

### Transcript

AGENT:

```
koto next coord --with-data '{"tasks": [
  {"name": "issue-bad", "template": "impl-broken.md",
   "vars": {"ISSUE_NUMBER": "7"}}
]}'
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
      "any_spawn_failed": true, "needs_attention": false,
      "children": [
        {"name": "coord.issue-bad", "state": null,
         "complete": false, "outcome": "spawn_failed",
         "ready_to_drive": false,
         "spawn_error": {
           "task": "issue-bad",
           "kind": "template_compile_failed",
           "template_source": "override",
           "compile_error": {
             "kind": "yaml_parse",
             "message": "found character that cannot start any token",
             "location": {"line": 3, "column": 5}
           },
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
        "template_source": "override",
        "compile_error": {
          "kind": "yaml_parse",
          "message": "found character that cannot start any token",
          "location": {"line": 3, "column": 5}
        },
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

Gloss: Round 3's typed `CompileError` struct (design lines
3214-3228) replaces the free string. Three fields: `kind` (short
machine-parseable discriminator), `message`, optional `location`.
The shape matches exactly between
`BatchError::TemplateCompileFailed.compile_error` and
`TaskSpawnError.compile_error` (design comment line 3193-3195
"shared with per-task `TaskSpawnError` so agents render one shape
for compile failures regardless of surface"). [OK]

### Probes

- **Shape.** `{kind, message, location: Option<{line, column}>}`.
  `paths_tried` is absent (it's `Option<Vec<String>>` in
  `TaskSpawnError`, serde skips when None). `path` is present in
  the `BatchError::TemplateCompileFailed` variant but not on
  `TaskSpawnError` -- but `TaskSpawnError` has no `path` field per
  design line 3084-3101. `[GAP: `path` missing on
  `TaskSpawnError`]` -- when a compile fails, the agent knows which
  task but doesn't know which concrete file the scheduler resolved
  to. For `Default` inheritance, that's annoying: "`impl-issue.md`
  compiled how? Against `/home/alice/proj/templates/impl-issue.md`
  or some other resolution?" The `BatchError` variant carries
  `path` (design line 3196); parity would add it to
  `TaskSpawnError` too. Worth flagging for Round 4.
- **Agent-parseable `kind`.** `"yaml_parse"`, `"missing_field"`,
  `"state_reference"` listed as examples (design line 3215-3217).
  Agents can route by `kind`: YAML errors -> "your frontmatter is
  malformed"; missing_field -> "add field X to the frontmatter";
  state_reference -> "state name X doesn't exist in this template."

### Gaps

- `[GAP: `path` asymmetry]`. `BatchError::TemplateCompileFailed`
  carries `path: String`, but `TaskSpawnError` does NOT (design
  line 3084-3101 has no `path` field). When the compile-failed
  case surfaces via `TaskSpawnError` (i.e., via
  `SchedulerOutcome::errored`), agents can't tell which file the
  scheduler read. For `template_source: "override"` the task's
  `template` gives them the answer; for `template_source:
  "default"` they have to infer from the hook declaration. Not
  catastrophic but a parity miss.
- `[GAP: `CompileError.kind` vocabulary]`. Design gives three
  example values (`yaml_parse`, `missing_field`, `state_reference`)
  but no exhaustive list. If new kinds arrive with future compiler
  changes, agents pattern-matching on specific values break
  silently. Recommend: document stable-vs-evolving kinds, or make
  the enum explicit.

---

## Episode 7 -- DAG depth: diamond, linear, boundary

### 7a: Diamond, depth 3

Tasks: A, B (waits_on A), C (waits_on A), D (waits_on B, C).
Longest root-to-leaf path: A -> B -> D (or A -> C -> D), 3 nodes.
Total nodes: 4. Accepted.

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.A"],
    ...
  }
}
```

Only A spawns this tick (B and C are blocked on A; D is blocked on
both).

### 7b: Linear, depth 5

Tasks: A -> B -> C -> D -> E. Longest path: 5 nodes. Accepted
(limit 50).

### 7c: Linear 51, rejected

Tasks: 51-element linear chain. Longest path: 51 nodes. Rejected.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Longest dependency chain has 51 tasks; limit is 50.",
    "details": [{"field": "tasks", "reason": "limit_exceeded"}],
    "batch": {
      "kind": "limit_exceeded",
      "which": "depth",
      "limit": 50,
      "actual": 51,
      "task": null
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: Round 3 flattens `LimitExceeded` to top level. Design line
3199-3202 `BatchError::LimitExceeded { which: LimitKind, limit,
actual, task: Option<String> }` lives as a sibling of
`InvalidBatchDefinition`, not nested inside
`InvalidBatchReason`. Design comment line 3243-3246 explicitly
says "Limits surface via the sibling `BatchError::LimitExceeded`
... variant with `which` carrying the typed `LimitKind`, not via
nested variants on this enum." The JSON `error.batch.kind` reads
`"limit_exceeded"` (the `BatchError` tag). [OK]

### Probes

- **`task` field for depth.** `task: null` -- depth is a graph
  property, not per-task. For `WaitsOn` (per-task limit, e.g., a
  task depending on 11+ siblings when limit is 10), `task` would
  name the offending entry.
- **`which` discriminator.** `"depth"` -- snake_case, typed via
  `LimitKind` (design line 3256-3262). Other values:
  `"tasks"`, `"waits_on"`, `"payload_bytes"`.
- **Off-by-one confirmation.** Message says "Longest dependency
  chain has 51 tasks; limit is 50." Matches agent's intuition
  exactly. [OK]
- **Hoisted to top-level `kind`.** The `error.batch.kind` is
  `"limit_exceeded"` (BatchError variant name snake_cased), not
  nested under `"invalid_batch_definition"`. Agents branch on
  `error.batch.kind` to tell submission-validity from limit
  violations.

### Gaps

- `[GAP: `LimitKind` growth]`. Design shows four variants (`Tasks`,
  `WaitsOn`, `Depth`, `PayloadBytes`). If a fifth arrives,
  pattern-matching agents fall through. Not CD14's problem --
  standard enum-extension concern.
- `[GAP: diamond runtime semantics]`. CD14 doesn't discuss whether
  the scheduler de-duplicates A->D transitive edges (`trigger_rule:
  all_success` on D means both B and C must succeed). Out of scope
  for this pair but noted.

---

## Section 2: Cross-episode findings

### Warning vs error coexistence (Probe)

Confirmed: `SchedulerWarning` entries live in `scheduler.warnings[]`
(non-fatal). `TaskSpawnError` entries live in `scheduler.errored[]`
(per-task fatal). The two arrays are siblings on the `scheduler`
object. Both can populate in the same response: consider a tick
where the header carries a stale `template_source_dir` (Episode 2
triggers warning) AND one task's override path is missing (Episode
3-style error). The response would carry one warning and one
errored entry. Episodes 1-2 confirm warnings do not abort spawn;
Episodes 3-4 confirm errors are per-task. Clean.

### Agent-side filtering (Probe)

An agent's predicate "drive workers for children where
`ready_to_drive == true AND outcome != 'spawn_failed'`" works
cleanly against `blocking_conditions[0].output.children[]`. Each
entry carries `ready_to_drive` (walkthrough's existing field) and
`outcome`. `spawn_failed` children explicitly carry
`ready_to_drive: false` (they have no state file, so there's
nothing to drive). Agents filter by a single JSONPath traversal.

Additionally, for recovery, agents cross-reference
`scheduler.errored[]` by task name to pull detail. The per-child
`spawn_error` sub-object mirrors the same data, so a single pass
over `children[]` is sufficient -- `scheduler.errored[]` is the
redundant-by-design summary.

### Pre-D4 warning dedup (Probe)

Every tick re-emits `missing_template_source_dir` on a pre-D4
parent (Episode 1 tick 2). Design explicitly defers the fix:
"There is no header-rewrite primitive in v1 ... A future `koto
session rehome <parent>` subcommand -- scope for v1.1 or a
successor design" (design line 2676-2683). Agents must suppress
locally. koto-user skill should document this expectation.

### Round 3 polish item verification

| Polish item | Verified in | Status |
|---|---|---|
| `TaskSpawnError.template_source` | Episode 3 | [OK] distinguishes `override` vs `default` |
| `StaleTemplateSourceDir.machine_id` + `falling_back_to` | Episode 2 | [OK] all three fields present and useful |
| `paths_tried` canonicalization | Episode 4 | [OK] lexical normalization, `..` stripped |
| `CompileError` typed struct | Episode 6 | [OK] shape `{kind, message, location}` |
| Pre-D4 warning as known limitation | Episode 1 | [OK] rehome deferred to v1.1 |
| `LimitExceeded` hoisted to top-level | Episode 7c | [OK] sibling of `InvalidBatchDefinition` |

### Remaining gaps (new or carried forward)

1. **`TaskSpawnError.path` asymmetry with `BatchError` variant.**
   Episode 6. `BatchError::TemplateCompileFailed` carries `path`;
   `TaskSpawnError` does not. Parity would help agents consuming
   runtime compile errors identify which file was read, especially
   for `template_source: "default"` where the agent didn't
   originally supply a path.
2. **Cross-tick warning suppression.** Episodes 1, 2. Design's
   "once per tick" dedup only works within a single `koto next`
   invocation. Long-lived parents accumulate warnings across
   ticks. Agents must suppress locally. Future `koto session
   rehome` is the real fix.
3. **Absolute-path cross-machine drift (known limitation).**
   Episode 5. Round 3 explicitly declines to add submission-time
   cross-machine warnings for absolute paths. Runtime per-task
   `TemplateNotFound` with `template_source: "override"` is the
   only signal. Acceptable scope boundary.
4. **`CompileError.kind` vocabulary.** Episode 6. Three example
   values listed, no exhaustive enum. Pattern-matching agents may
   break if new kinds arrive silently.
5. **Warning remediation hints.** Episodes 1, 2. Neither
   `MissingTemplateSourceDir` nor `StaleTemplateSourceDir` carries
   an inline hint pointing at the documented mitigation. Agents
   must consult skill docs out-of-band. Low priority.

### Summary

Round 3 polish holds under every episode tested. The four fixes
(template_source attribution, StaleTemplateSourceDir enrichment,
paths_tried canonicalization, CompileError struct) land as
described. The deferred items (pre-D4 header rewrite,
submission-time absolute-path lint) are documented as known
limitations with clear v1.1 landing paths. The remaining gaps are
small parity and discoverability improvements, not design
contradictions.

No Round 3 contradictions. Design is consistent.
