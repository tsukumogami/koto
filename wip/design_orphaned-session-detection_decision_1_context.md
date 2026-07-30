# Decision Context: Orphan-detection signal shape and wiring

## Question
What shape and home should the new orphan-detection signal take, and how
should it wire into `koto status`'s response, `koto init`'s already-exists
collision message, and `koto session list`'s per-session output --
reusing vs. extending `SchedulerWarning::StaleTemplateSourceDir`, and what
should it be named given `koto workflows --orphaned` already means
something different (parent workflow gone, not template-source-dir gone)?

## Complexity
critical (changes CLI/JSON output contract of shipping commands)

## Constraints
- Direction 3 (automatic sweep/gc/cleanup) is out of scope. Read-only /
  informational signals only.
- A bare `Path::exists()` check is accepted as sufficient (already decided,
  do not re-litigate).
- Must reuse/extend `StaleTemplateSourceDir`'s existing shape and
  `machine_id` vocabulary rather than invent an incompatible second signal
  for the same underlying condition.
- New name/flag must not collide with existing, differently-scoped
  `koto workflows --orphaned` (parent-workflow-gone, not
  template-source-dir-gone).
- `SessionInfo` (`src/session/mod.rs:129-141`) is additive-only for new
  fields: adding a field is safe, changing existing method signatures is
  not.
- Must work across both `LocalBackend` and `CloudBackend` implementations
  of `SessionBackend::list()`. A second, separate decision (decision 2 in
  the same design) tunes backend-specific behavioral differences -- this
  decision only needs a shape both backends can implement.

## Known Options
(none pre-identified by the parent -- to be generated in Phase 2)

## Background
`template_source_dir` is an `Option<PathBuf>` field on `StateFileHeader`
(`src/engine/types.rs:260`), written once at `koto init`/child-spawn time
(`src/cli/init_child.rs:456-475`), read back via a single shared parser
(`persistence::parse_header`, `src/engine/persistence.rs`). Today it's
consumed by exactly one caller: the batch scheduler's relative-child-
template-path resolver (`src/cli/batch.rs`, `src/engine/path_resolution.rs`),
which emits `SchedulerWarning::StaleTemplateSourceDir { path, machine_id,
falling_back_to }` (`src/engine/scheduler_warning.rs`) when
`template_source_dir.exists()` is false.

`koto status` (`handle_status`, `src/cli/mod.rs:4387`) already reads the
full header but never looks at this field. `koto session list`
(`handle_list`, `src/cli/session.rs:504`, backed by `SessionInfo` in
`src/session/mod.rs:129-141`) reads the header then discards the field
when projecting into `SessionInfo`. `koto init`'s "already exists"
collision check (`src/cli/mod.rs:1682-1691`, `backend.exists(name)`) is a
pure filesystem-presence test that never opens the colliding session's
header at all.

Existing precedent: `docs/designs/current/DESIGN-batch-child-spawning.md`
Decision 14 established `StaleTemplateSourceDir` with JSON wire shape
`kind: "stale_template_source_dir"`. `docs/designs/current/DESIGN-
hierarchical-workflows.md` / `docs/prds/PRD-hierarchical-workflows.md`
document `koto workflows --orphaned` as detecting workflows whose parent
workflow no longer exists -- a structurally different condition
(parent-gone vs. source-dir-gone) that happens to share the word
"orphan".
