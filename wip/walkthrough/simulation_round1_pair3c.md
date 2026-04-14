# Simulation round 1, pair 3c -- path resolution and limit boundaries

Focus: error-path ergonomics around Decision 4 (template resolution),
Decision 8 (default_template compile-time validation), the submission-
time hard-limit block, and the "..-segments permitted" paragraph.

Parent template: `coord` (from `wip/walkthrough/walkthrough.md`),
init'd with `template_source_dir=/home/alice/repo/templates`,
`default_template=impl-issue.md`, `failure_policy=skip_dependents`.

Throughout: AGENT is unpredictable and has no design-doc access. KOTO
replies with JSON consistent with the design or marks `[GAP: ...]`
where the design does not speak.

---

## Section 1: Transcript

### Scenario 1 -- per-task absolute path pointing outside the parent repo

AGENT:

```
koto next coord --with-data @tasks.json
```

`tasks.json`:

```json
{
  "tasks": [
    {"name": "issue-1", "vars": {"ISSUE_NUMBER": "101"}},
    {
      "name": "issue-cross-repo",
      "template": "/home/alice/other-repo/custom-impl.md",
      "vars": {"ISSUE_NUMBER": "999"}
    }
  ]
}
```

KOTO:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 2, "completed": 0, "pending": 2,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-cross-repo", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-1", "coord.issue-cross-repo"],
    "already": [], "blocked": [], "skipped": []
  }
}
```

Gloss: Decision 4(a) says "absolute paths pass through". The scheduler
does not interpret "outside the parent repo" -- it opens the file by
the absolute path and compiles. Spawn succeeds. `[GAP:
cross-machine portability -- the child's state file records the
absolute path in `template_source_dir` (parent dir of the child's
template), which means if this state file gets cloud-synced to a
machine where `/home/alice/other-repo/` does not exist, the child is
unusable. The design's "Cloud sync compatibility" paragraph assumes
both bases "point at repo content" but says nothing about a child
whose template lives outside the parent's repo. The child has its
OWN `template_source_dir` captured at spawn time pointing at
`/home/alice/other-repo`, which may not exist on the desktop.]`

### Scenario 2 -- `..` escape

AGENT resubmits with:

```json
{
  "tasks": [
    {
      "name": "issue-traverse",
      "template": "../../../etc/shadow",
      "vars": {"ISSUE_NUMBER": "1"}
    }
  ]
}
```

KOTO (scheduler spawn phase):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned": [],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errors": [{
      "task": "issue-traverse",
      "error": "template_resolve_failed",
      "paths_tried": [
        "/home/alice/repo/templates/../../../etc/shadow",
        "/home/alice/repo/../../../etc/shadow"
      ],
      "message": "Template did not compile: YAML frontmatter missing"
    }]
  }
}
```

Gloss: Per Decision 4's "`..`-segments are permitted" paragraph, koto
does not reject the path. The file exists and is readable, but
compilation fails (no YAML frontmatter), so the scheduler returns
`BatchError::TemplateResolveFailed`. There is no warning that the
path escaped the template dir. `[GAP: if `/etc/shadow` had been a
valid koto template, the spawn would have succeeded silently. The
design consciously accepts this (threat model: invoking user is
trusted), but there is no log, no `warning` field in the scheduler
outcome, and no surfacing to the agent that the resolution reached
outside the parent's repo. A downstream reviewer looking at the
state file has no signal that a traversal happened.]` `[GAP: the
BatchError variant enum lists `TemplateResolveFailed { task,
paths_tried }`, but the wire shape when a single task fails while
others succeed is not specified -- does the whole submission reject,
or does the scheduler report per-task errors as above? The Data
Flow text says runtime validation (R1-R7) happens before spawn
and "fail the whole submission", but R1 is "child template
compilable" which is what failed here. Ambiguous.]`

### Scenario 3 -- `template_source_dir` unset (upgrade from older koto)

AGENT:

```
koto next coord
```

State file header lacks `template_source_dir` (parent was init'd under
v0.7.0 before Decision 4 landed). After upgrade, agent submits a
batch with a relative template path.

`tasks.json`:

```json
{"tasks": [{"name": "issue-1", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "1"}}]}
```

KOTO:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned": [],
    "errors": [{
      "task": "issue-1",
      "error": "template_resolve_failed",
      "paths_tried": ["<submitter_cwd>/impl-issue.md"],
      "message": "Template not found. Parent was initialized before template_source_dir tracking; only submitter_cwd fallback is available."
    }]
  }
}
```

Gloss: `[GAP: the design says the resolution order is "(b) relative
paths join against template_source_dir, (c) on ENOENT, fall back to
submitter_cwd.join(...)". When `template_source_dir` is None
(Option), does the scheduler skip step (b) entirely and go straight
to submitter_cwd? Or does it error at submission because the header
field is None? The serde comment says `default, skip_serializing_if`
for back-compat, implying None is valid and should be tolerated. But
the fallback text is silent on "absent vs ENOENT". A user-visible
warning ("parent initialized before v0.8; template resolution may
be unreliable") would reduce surprise.]`

### Scenario 4 -- same relative name in both bases, different files

Setup: `template_source_dir=/home/alice/repo/templates/` contains
`impl-issue.md`. `submitter_cwd=/home/alice/repo/` contains a
different `impl-issue.md` (perhaps an older copy the agent edited
without realizing the effective one is the one in `templates/`).

AGENT:

```
koto next coord --with-data @tasks.json
```

`tasks.json`: `{"tasks": [{"name": "issue-1", "vars": {"I": "1"}}]}`
(uses `default_template: impl-issue.md`).

KOTO:

```json
{
  "action": "gate_blocked",
  "scheduler": {
    "spawned": ["coord.issue-1"],
    "already": [], "blocked": [], "skipped": []
  }
}
```

Gloss: Resolution (b) finds the file in `template_source_dir` first;
submitter_cwd is never consulted. Child spawns using the templates/
copy. `[GAP: this is silent. The design does not surface which base
won the resolution. If the agent expected the cwd copy (because it
just edited that file), there is no signal that the other copy was
used. A debug mode or an always-on `resolved_template_path` field in
the scheduler output per spawned task would make this diagnosable.
Concrete proposal: add `resolved_template` (absolute, canonicalized)
to each entry in `scheduler.spawned` (or alongside it).]`

### Scenario 5 -- `default_template` deleted between compile and submission

Setup: parent template compiled E9-valid against
`/home/alice/repo/templates/impl-issue.md`. Between `koto template
compile` and `koto next coord --with-data @tasks.json`, the user
deletes `impl-issue.md`.

AGENT:

```
koto next coord --with-data @tasks.json
```

`tasks.json`: `{"tasks": [{"name": "issue-1", "vars": {"I": "1"}}]}`.

KOTO:

```json
{
  "action": "gate_blocked",
  "scheduler": {
    "spawned": [],
    "errors": [{
      "task": "issue-1",
      "error": "template_resolve_failed",
      "paths_tried": [
        "/home/alice/repo/templates/impl-issue.md",
        "/home/alice/repo/impl-issue.md"
      ],
      "message": "Template not found. Tried parent template_source_dir, then submitter_cwd.",
      "inherited_from_default_template": true
    }]
  }
}
```

Gloss: Compile-time E9 passed earlier but does not help at submission
time. `[GAP: `inherited_from_default_template` is NOT in the
documented BatchError variants. The design's `TemplateResolveFailed
{ task, paths_tried }` does not carry a flag saying "this task had
no explicit template, so the failure came from the default". An
agent hitting this error would see two paths but no hint that the
path came from the parent template's default, not from something
they themselves supplied. Suggest adding an optional `source:
"per_task_override" | "default_template"` discriminator to the
error.]`

### Scenario 6 -- per-task template resolves but fails to compile

Setup: `issue-special.md` exists at `template_source_dir` but has a
malformed YAML frontmatter (missing `initial_state`).

AGENT submits:

```json
{"tasks": [{"name": "issue-x", "template": "issue-special.md", "vars": {}}]}
```

KOTO:

```json
{
  "action": "gate_blocked",
  "scheduler": {
    "spawned": [],
    "errors": [{
      "task": "issue-x",
      "error": "template_resolve_failed",
      "paths_tried": ["/home/alice/repo/templates/issue-special.md"],
      "message": "Template compile failed: missing required frontmatter field `initial_state`"
    }]
  }
}
```

Gloss: `[GAP: the BatchError docs describe `TemplateResolveFailed` as
"Resolved template path doesn't exist or fails to compile." -- so
compile failure is bundled with not-found. That collapses two
distinct diagnostics into one variant with one `paths_tried` field,
which is meaningless for compile errors. Suggest splitting into
`TemplateNotFound { task, paths_tried }` and `TemplateCompileFailed
{ task, path, compile_error }`.]` `[GAP: the design's Data Flow
(step 4) says runtime validation R1 (child template compilable)
happens *before* spawn and "fail the whole submission". The "Submit"
text at line 1862 says "fail the whole submission with
BatchError::InvalidBatchDefinition" for cycles, dangling refs, and
duplicate names -- is child-template compile failure also
"InvalidBatchDefinition" (whole-batch reject), or is it
per-task `TemplateResolveFailed` (partial success)? Two places in
the design give different answers.]`

### Scenario 7 -- limit: 1000 tasks now, resubmit adding 1 more

AGENT submits 1000 tasks (exact cap). KOTO accepts, spawns the
root-of-DAG subset, responds with the usual `gate_blocked`.

Later, a running child submits `koto next coord --with-data
'{"tasks": [<1 new task>]}'`. The design says `merge_epoch_evidence`
unions new tasks with existing. Post-merge set: 1001.

KOTO:

```json
{
  "action": "error",
  "error": {
    "kind": "batch",
    "variant": "limit_exceeded",
    "which": "task_count",
    "limit": 1000,
    "actual": 1001,
    "message": "Merged task set (original 1000 + new 1) exceeds per-batch limit of 1000 tasks. Submission rejected; no tasks appended."
  }
}
```

Gloss: `[GAP: the design says "Submission-time hard limit
enforcement" rejects exceeding submissions, and the resubmission
path is documented as "the scheduler rejects the resubmission
before any new spawn happens; already-spawned children from earlier
submissions are untouched" -- but that sentence is about cycles in
merged DAGs, not about hitting the task-count cap. The semantics
match (reject the merge, keep prior state) but the design is only
explicit for cycles. Suggest stating that `LimitExceeded` on a
resubmission also rejects the whole merge without partial
acceptance.]` `[GAP: what EvidenceSubmitted event state is left
behind after this reject? If the advance loop already appended the
new `{"tasks": [...]}` evidence event before the scheduler ran its
limit check, the event log now has an EvidenceSubmitted that will be
merged on the next tick -- re-triggering the same rejection
forever. The limit check must happen BEFORE evidence append (at
validate_accepts_schema or an earlier submission-validate step).
The design says "At evidence submission, reject task lists
exceeding hard caps" which implies pre-append, but there is no
explicit code-path note tying this to advance-loop ordering.]`

### Scenario 8 -- waits_on > 10: split across submissions

AGENT submits one task with `waits_on` of 10 entries. Accepted.

AGENT then submits a resubmission that adds an 11th entry to the
same task's `waits_on`:

```json
{"tasks": [{"name": "issue-x", "waits_on": ["a","b","c","d","e","f","g","h","i","j","k"]}]}
```

KOTO:

```json
{
  "action": "error",
  "error": {
    "kind": "batch",
    "variant": "limit_exceeded",
    "which": "waits_on_per_task",
    "limit": 10,
    "actual": 11,
    "task": "issue-x"
  }
}
```

Gloss: `[GAP: the design says "no more than 10 `waits_on` entries per
task" but does not clarify whether this is enforced on the
submission payload or the post-merge task. If `merge_epoch_evidence`
produces a single union of the two waits_on lists, then the 11th
entry triggers rejection as above. But if the merge semantics
*replace* the task entry whole (last-writer-wins), then the
submission of 11 IS the submission-time content and the check is
trivially on the new payload. The design does not specify whether
`merge_epoch_evidence` on the `tasks` field is a union of task
entries (by name) or a replacement. This ambiguity propagates to
every "dynamic additions" use case.]`

### Scenario 9 -- DAG depth 51

AGENT submits a linear chain: issue-1 -> issue-2 -> ... -> issue-51
(50 edges, chain length 51).

```json
{
  "tasks": [
    {"name": "issue-1"},
    {"name": "issue-2", "waits_on": ["issue-1"]},
    ...
    {"name": "issue-51", "waits_on": ["issue-50"]}
  ]
}
```

KOTO:

```json
{
  "action": "error",
  "error": {
    "kind": "batch",
    "variant": "limit_exceeded",
    "which": "dag_depth",
    "limit": 50,
    "actual": 51
  }
}
```

Gloss: `[GAP: "DAG depth of 50" is the limit, but "depth" is not
defined. Options: (a) number of nodes in the longest root-to-leaf
path, (b) number of edges on that path (= nodes - 1), (c) length of
the longest path between any pair of nodes. For a chain of 51
nodes, (a)=51, (b)=50, (c)=50. A chain of 50 nodes gives (a)=50,
(b)=49. Pick one and document. The security-considerations text
phrases it as "DAG depth no deeper than 50" which slightly suggests
(a), but the BatchError does not distinguish. Agents cannot reason
about the boundary without this definition.]`

### Scenario 10 -- cloud sync: parent init'd on laptop, resumed on desktop

AGENT (on laptop): `koto init coord --template
/home/alice/repo/templates/coord.md`. Header gets
`template_source_dir=/home/alice/repo/templates` (canonicalized
laptop absolute path).

AGENT (on laptop) submits tasks. Scheduler spawns.

Laptop syncs state files via cloud sync. User switches to desktop
(path layout may differ, but in this common case
`/home/alice/repo/...` exists on both -- same user, synced repo).

AGENT (on desktop) runs `koto next coord`. Scheduler resumes,
classifies, spawns additional children. For each task, it resolves
`template: "impl-issue.md"` against
`template_source_dir=/home/alice/repo/templates` (the value captured
at init on the laptop).

KOTO (desktop):

```json
{
  "action": "gate_blocked",
  "scheduler": {
    "spawned": ["coord.issue-2", "coord.issue-3"],
    "already": ["coord.issue-1"],
    "blocked": [], "skipped": []
  }
}
```

Gloss: Works iff the captured absolute path exists on the desktop. If
the desktop's path layout differs (e.g., laptop was `/home/alice`,
desktop is `/Users/alice`), `template_source_dir` resolution fails,
then submitter_cwd is tried (whatever `cwd` was at submission --
also a laptop path captured in the EvidenceSubmitted event). Both
fail; scheduler errors out.

`[GAP: the design's "Cloud sync compatibility" paragraph says "both
bases point at repo content, not koto cache. Koto already assumes
repo checkouts have stable paths across machines." -- but that
assumption is not true in general. Same-user same-home-dir works;
macOS-Linux mixed work does not. The design does not provide a
recovery path beyond "the invoking user must keep paths stable".
Suggest either (a) documenting this as a known limitation, or (b)
adding a `koto session retarget --template-source-dir <new>`
command for rebasing paths after machine migration.]` `[GAP:
`submitter_cwd` captured at laptop submission is ALSO a laptop
absolute path. On the desktop, it is equally stale. The fallback
from template_source_dir -> submitter_cwd does not help across
machine boundaries; both bases are laptop paths. This weakens the
design's "common case" framing.]`

### Scenario 11 -- explicit `template: null`

AGENT submits:

```json
{
  "tasks": [
    {"name": "issue-1", "template": null, "vars": {"ISSUE_NUMBER": "1"}}
  ]
}
```

KOTO: `[GAP]` The design does not specify null vs. omitted. The
`item_schema` in Decision 8 says `"template": { "type": "string",
"required": false, "default": "impl-issue.md" }` -- "required: false"
could mean "omit" OR "any value including null". Two plausible
behaviors:

Behavior A (tolerant, substitute default):

```json
{
  "action": "gate_blocked",
  "scheduler": {"spawned": ["coord.issue-1"], "already": [], "blocked": [], "skipped": []}
}
```

Behavior B (strict, reject as schema violation):

```json
{
  "action": "error",
  "error": {
    "kind": "batch",
    "variant": "invalid_batch_definition",
    "reason": "task `issue-1` field `template` must be a string (got null). Omit the field to use default_template=impl-issue.md."
  }
}
```

Gloss: `[GAP: which behavior is canonical? Most JSON schema tooling
treats null as distinct from absent. koto's accepts validator
likely errors on a type mismatch (expected string, got null).
The design should pick Behavior B (reject null) with an actionable
error message, and the item_schema response should maybe add
"nullable: false" to foreclose the ambiguity. Alternatively,
Behavior A (coerce null to default) is friendlier but harder to
reason about. Either way, document it.]`

---

## Section 2: Findings

### Finding 1 -- Cross-repo absolute template path leaves state file unportable

- Observation: Scenario 1. An absolute path pointing outside the
  parent's repo is accepted per Decision 4(a). The child's own
  `template_source_dir` becomes that external directory. Cloud sync
  plus machine migration will break the child if the external path
  does not exist on the destination machine.
- Location in design: lines 840-865 ("Cloud sync compatibility"
  paragraph assumes both bases point at repo content).
- Severity: Medium. Silent cross-machine breakage.
- Proposed resolution: Document explicitly that absolute paths
  outside the parent's repo are a portability hazard. Consider a
  warning field in `scheduler.spawned` entries when the resolved
  template path is not a descendant of `template_source_dir`.

### Finding 2 -- `..`-traversal is silent

- Observation: Scenario 2. Permitted by design, but never surfaced
  anywhere in the response or state file. An agent or a later
  reviewer has no signal that resolution reached outside the
  declared template dir.
- Location in design: lines 867-874, plus Security Considerations
  lines 2304-2312.
- Severity: Low (by design), but low-cost to improve.
- Proposed resolution: Add a non-fatal `warnings` array to the
  scheduler outcome; emit `"template_escaped_source_dir"` with the
  canonicalized path when any task resolves via `..`.

### Finding 3 -- `template_source_dir == None` fallback undefined

- Observation: Scenario 3. Decision 4 says fallback happens "on
  ENOENT", but never specifies the "primary base absent entirely"
  case (upgrade from old koto state file).
- Location in design: lines 844-856.
- Severity: Medium. Affects upgrade users in a way that produces
  unclear errors.
- Proposed resolution: Specify that absent `template_source_dir`
  skips step (b) and goes straight to `submitter_cwd`. Surface a
  one-time warning in the response noting the state file lacks
  `template_source_dir` and suggesting re-initialization.

### Finding 4 -- Resolution winner not surfaced when both bases contain the file

- Observation: Scenario 4. Silent tie-break between
  `template_source_dir` and `submitter_cwd`. Violates principle of
  least surprise when the agent has two copies on disk.
- Location in design: line 854 (resolution order), line 2307
  ("canonicalized template_source_dir first, then against
  submitter_cwd").
- Severity: Low-Medium.
- Proposed resolution: Add `resolved_template` (canonicalized
  absolute path) to every entry in `scheduler.spawned` so agents can
  verify which file was actually used. One-line addition to
  `SchedulerOutcome::Scheduled` serialization.

### Finding 5 -- `TemplateResolveFailed` does not distinguish "inherited from default_template" vs "per-task override"

- Observation: Scenario 5. When the default template goes missing,
  the agent sees `paths_tried` but no hint that the failing
  `template` field came from the parent-declared default, not from
  the task entry itself.
- Location in design: lines 1597-1608 (BatchError enum definition).
- Severity: Low. Purely a UX improvement.
- Proposed resolution: Add `source: "per_task_override" |
  "default_template"` field to `TemplateResolveFailed`. Update the
  error message template to say "Child template inherited from
  parent's default_template".

### Finding 6 -- `TemplateResolveFailed` conflates not-found with compile-failed

- Observation: Scenario 6. One variant, one `paths_tried` field,
  for two very different failure modes. `paths_tried` is
  meaningless when the file was found but failed to compile.
- Location in design: lines 1602-1603 (BatchError docstring:
  "Resolved template path doesn't exist or fails to compile").
- Severity: Medium. Muddies error semantics.
- Proposed resolution: Split into `TemplateNotFound { task,
  paths_tried }` and `TemplateCompileFailed { task, path,
  compile_error }`. Updates ~3 error-handling sites.

### Finding 7 -- Per-task child-compile: partial spawn vs whole-batch reject is ambiguous

- Observation: Scenario 6. The Data Flow text (line 1862) says R1
  (child template compilable) is runtime validation that "fails the
  whole submission with BatchError::InvalidBatchDefinition". But
  the BatchError docstring suggests per-task `TemplateResolveFailed`.
  Two places in the design give different answers.
- Location in design: line 640 (R1 runtime check), lines 1597-1608
  (BatchError enum), lines 1862-1868 (Data Flow step 4).
- Severity: High. The two behaviors have very different
  operational semantics (do siblings spawn or not?).
- Proposed resolution: Pick one. Recommendation: per-task error,
  because whole-batch reject on a single bad template defeats the
  "dynamic additions" promise. Document the choice and make R1 a
  per-task check rather than a batch-level one.

### Finding 8 -- Limit-exceeded on resubmission: rejection atomicity not documented

- Observation: Scenario 7. Design states limits are hard, and
  describes cycle-rejection semantics for resubmission ("rejects
  before any new spawn happens; already-spawned children
  untouched"), but does not generalize that pattern to
  `LimitExceeded` on merge-driven growth.
- Location in design: lines 1864-1868, lines 2126-2132.
- Severity: Medium.
- Proposed resolution: Explicitly state that `LimitExceeded` on a
  resubmission rejects the whole submission; no partial acceptance;
  no EvidenceSubmitted event is appended. Tie this to pre-append
  validation in handle_next so the state log does not contain a
  poison event that re-triggers rejection every tick.

### Finding 9 -- `waits_on > 10`: per-submission vs post-merge is undefined

- Observation: Scenario 8. The 10-cap could apply to the payload or
  to the merged task definition. The design does not say which.
  Underlying question: does `merge_epoch_evidence` union task
  entries by name (growing waits_on) or replace them wholesale?
- Location in design: line 2288 ("no more than 10 `waits_on`
  entries per task"), plus the "merge_epoch_evidence" references
  at lines 93, 1037, 1861.
- Severity: High. This question also blocks Scenarios 7 and 11 and
  the entire "dynamic additions" mental model.
- Proposed resolution: Document the merge semantics for `tasks`
  explicitly -- either "entries merge by name, lists union" or
  "entries replace by name, last-writer-wins". Pick one and
  evaluate limits against the merged final form.

### Finding 10 -- "DAG depth of 50" is undefined

- Observation: Scenario 9. Depth has three plausible definitions.
  A chain of 51 nodes is rejected under (a) nodes-on-longest-path
  but accepted under (b) edges-on-longest-path.
- Location in design: line 2289 ("DAG depth no deeper than 50"),
  line 2128.
- Severity: Medium. Users will hit boundary surprises.
- Proposed resolution: Pick node-count (simpler to communicate),
  document explicitly, and reference the definition in the
  BatchError message: `"Longest dependency chain has 51 tasks;
  limit is 50"`.

### Finding 11 -- Cloud-sync path portability is only partially addressed

- Observation: Scenario 10. `template_source_dir` captured at
  init and `submitter_cwd` captured at submission are BOTH machine-
  specific absolute paths. On cross-home-layout migration (e.g.,
  Linux to macOS), both fallbacks fail. The design's "Cloud sync
  compatibility" paragraph assumes repo paths are stable across
  machines -- true for same-user same-layout, false in general.
- Location in design: lines 863-865, lines 2221-2225 ("Negative"
  bullet acknowledges this partially).
- Severity: Medium. Mitigated by the typical single-user setup,
  but the failure mode is confusing.
- Proposed resolution: Either (a) make this a documented known
  limitation in the user-facing guide, (b) add a `koto session
  retarget` subcommand to rewrite header paths, or (c) store the
  parent-relative template path (repo-relative) alongside the
  absolute path and prefer the repo-relative one when the absolute
  one does not resolve.

### Finding 12 -- `template: null` semantics undefined

- Observation: Scenario 11. `item_schema` says `required: false`
  with a `default`; nothing documents null vs. absent. Most accepts
  validators would reject null as "expected string, got null".
- Location in design: lines 1316-1334 (item_schema block).
- Severity: Low. Easily addressed with a one-line clarification.
- Proposed resolution: Add `"nullable": false` (or equivalent) to
  the item_schema fields that have `default`, and specify the
  accepts validator rejects null with a message naming the default:
  `"field `template` must be omitted or a string; to use default,
  omit the key"`.
