# Alternatives: Orphan-detection signal shape and wiring

## Alternative A: Direct reuse -- all three call sites construct/consume `SchedulerWarning::StaleTemplateSourceDir` itself

Instead of creating any new type, relocate `SchedulerWarning` (or at least
make it constructible) from a place reachable by `koto status`,
`koto init`, and `koto session list`, and have each of those three call
sites literally build and expose the existing
`SchedulerWarning::StaleTemplateSourceDir { path, machine_id,
falling_back_to }` variant -- same Rust type, same `#[serde(tag = "kind")]`
JSON shape (`"kind": "stale_template_source_dir"`), same `machine_id:
Option<String>` vocabulary already established by Decision 14.

- `SessionInfo` gains `pub warnings: Vec<SchedulerWarning>` (additive,
  empty `Vec` by default, `skip_serializing_if` when empty).
- `koto status` gains a `warnings` array top-level key (following the
  existing convention of `batch`/`superseded_branches` being present only
  when relevant).
- `koto init`'s collision path opens the colliding session's header (new
  I/O it doesn't do today) and, if stale, folds
  `SchedulerWarning::StaleTemplateSourceDir` into the collision error's
  JSON/text.
- `falling_back_to: PathBuf` has no natural value outside the scheduler
  (there's nothing to "fall back to" when just listing or checking a
  collision) -- would need to become `Option<PathBuf>`, which changes the
  wire shape scheduler consumers rely on, or be populated with a
  fabricated placeholder (e.g. the session's own directory), which is
  misleading.
- Renaming the enum away from "SchedulerWarning" (since it's no longer
  scheduler-only) touches every existing scheduler call site
  (`batch.rs`, tests), a larger blast radius than this decision's scope.

Source: existing knowledge + research artifact.

## Alternative B: Shared-core extraction -- one existence-check helper, `SchedulerWarning` stays scheduler-only, new sites get a purpose-built additive type

Extract the actual "does `template_source_dir` exist, and what's the
`machine_id`" computation into one shared helper (e.g.
`fn check_template_source_dir(header: &StateFileHeader) ->
Option<TemplateSourceStatus>` living next to `persistence::parse_header`
or in `path_resolution.rs`), where `TemplateSourceStatus { path: PathBuf,
exists: bool, machine_id: Option<String> }` carries exactly the
vocabulary `StaleTemplateSourceDir` already established (same field
names/types for `path` and `machine_id`), minus the scheduler-specific
`falling_back_to`.

- `SchedulerWarning::StaleTemplateSourceDir` is refactored to be
  constructed *from* this shared helper's result, with `falling_back_to`
  bolted on only inside the scheduler caller (`batch.rs`). One code path
  computes existence; the scheduler-only field stays scheduler-only.
- `SessionInfo` gains `pub template_source_status:
  Option<TemplateSourceStatus>` (additive; `None` when the header has no
  `template_source_dir` at all, `Some(status)` -- with `status.exists`
  covering both present-and-fine and present-but-gone -- when it does).
- `koto status` gains a conditional top-level JSON key (e.g.
  `stale_template_source_dir`, present only when `exists == false`,
  mirroring the `kind: "stale_template_source_dir"` vocabulary from
  Decision 14 so the same string means the same thing everywhere).
- `koto init`'s collision path opens the colliding session's header
  (same new but cheap I/O as Alternative A -- the header is already
  parsed on disk, one extra read) and calls the same shared helper; if
  stale, the collision error gains a distinguishing field/sentence
  ("the existing workflow's source directory no longer exists") instead
  of the generic message.
- No enum rename, no scheduler-specific field leaking into non-scheduler
  call sites, but there are now two Rust types (`TemplateSourceStatus`
  and `SchedulerWarning::StaleTemplateSourceDir`) that must be kept
  conceptually (not literally) in sync.

Source: existing knowledge + research artifact.

## Alternative C: Independent per-site booleans -- no shared type, each call site does its own `Path::exists()` check

Each of the three call sites independently computes
`header.template_source_dir.as_ref().map(|p| !p.exists())` inline and
exposes a locally-named boolean/field with no shared struct and no
`machine_id` at all (machine_id is treated as scheduler-specific
metadata not worth threading through three separate ad hoc checks).

- `SessionInfo` gains `pub template_source_dir_missing: Option<bool>`.
- `koto status` gains a similarly-shaped ad hoc boolean field.
- `koto init`'s collision path does the same inline check independently.
- Cheapest to implement (three small, independent diffs, no shared
  module, no refactor of `SchedulerWarning`).
- Drops the `machine_id` vocabulary the constraints explicitly require
  reusing -- `machine_id` is what lets a reader distinguish "this
  directory is gone because it was on a different machine that hasn't
  synced" (the legitimate cloud-sync false positive decision 2 is about)
  from "this directory is actually gone." Without it, decision 2's
  backend-specific tuning has nothing to key off of.
- Three independently-named/shaped fields across three surfaces means
  there's no single recognizable vocabulary an agent or script can learn
  once and reuse across `status`/`init`/`list` output.

Source: existing knowledge + research artifact.

## Comparison

| | A: Direct reuse | B: Shared-core extraction | C: Independent booleans |
|---|---|---|---|
| Reuses `machine_id` vocabulary | Yes (literal) | Yes (same fields) | No |
| Blast radius | High (enum rename/reshape touches scheduler) | Medium (one new small type + refactor) | Low (three independent diffs) |
| `falling_back_to` handling | Forced into non-scheduler contexts (awkward) | Stays scheduler-only (clean) | N/A (dropped entirely) |
| Single vocabulary across 3 surfaces | Yes | Yes | No |
| New I/O at `koto init` collision | Yes (open header) | Yes (open header) | Yes (open header) |
| Naming collision with `--orphaned` | None of the three introduce a new "orphan"-named flag; all reuse "stale_template_source_dir" wording. |

## Recommendation

Alternative B is the strongest fit: it satisfies the constraint to reuse
`StaleTemplateSourceDir`'s shape and `machine_id` vocabulary, avoids
forcing a scheduler-specific field (`falling_back_to`) into three
call sites that have nothing to "fall back" to, and avoids the larger
blast radius of renaming/relocating `SchedulerWarning` itself.
Naming: keep the "stale_template_source_dir" wording (not "orphan") for
the JSON `kind`/field name, and introduce no new CLI flag named
`--orphaned`-adjacent -- `koto status` and `koto init`'s collision
message are unconditional outputs already, and `koto session list` can
surface the field unconditionally in JSON (with a human-readable
annotation) without needing a flag at all, so the naming collision with
`koto workflows --orphaned` is avoided by construction rather than by
picking a clever alternate word.
