# Research: Orphan-detection signal shape and wiring

## Research conducted
Read source directly: `src/engine/types.rs` (StateFileHeader), `src/engine/
scheduler_warning.rs` (full), `src/cli/batch.rs` + `src/engine/
path_resolution.rs` (StaleTemplateSourceDir construction), `src/cli/mod.rs`
(handle_status ~4387, init collision ~1682-1716), `src/cli/session.rs`
(handle_list ~504), `src/session/mod.rs` (SessionInfo, SessionBackend
trait), `src/session/local.rs` and `src/session/cloud.rs` (list()
impls), `docs/designs/current/DESIGN-batch-child-spawning.md` (Decision
14), `docs/designs/current/DESIGN-hierarchical-workflows.md` +
`docs/prds/PRD-hierarchical-workflows.md` (--orphaned), `docs/
STABILITY.md`, `CHANGELOG.md`, and grepped for "orphan" project-wide.

## Findings

### StateFileHeader (src/engine/types.rs:260)
`template_source_dir: Option<PathBuf>` with `#[serde(default,
skip_serializing_if = "Option::is_none")]`. Established convention:
additive fields are `Option<T>` + that same serde attribute pair,
documented inline as "Additive field: ...".

### SchedulerWarning (src/engine/scheduler_warning.rs, full file)
Exactly three variants: `MissingTemplateSourceDir` (unit),
`StaleTemplateSourceDir { path: String, machine_id: Option<String>,
falling_back_to: PathBuf }`, `OmittedPriorTask { task: String }`. Enum
attribute: `#[serde(tag = "kind", rename_all = "snake_case")]` (line 68) →
JSON `{"kind":"stale_template_source_dir","path":...,"machine_id":...,
"falling_back_to":...}`. `machine_id: Option<String>` with
`skip_serializing_if = "Option::is_none"` (line 86) -- omitted from JSON
when `None` (test: `stale_omits_machine_id_when_none`). Module doc
(lines 33-37) states the external-tag shape is chosen specifically so
"adding variants is additive."

### Construction trigger (path_resolution.rs, batch.rs:1774-1797)
Always a `Path::exists()` probe. `falling_back_to` = the directory the
scheduler actually used after the stale base is skipped. **Only fires
inside the batch scheduler's relative-child-template resolution path**
-- nothing in `koto status`, `koto init`, or `koto session list`
currently constructs or reads this warning.

### handle_status (src/cli/mod.rs:4387-4490)
Reads the full header via `backend.read_events(name)` but never
references `template_source_dir` (confirmed by grep -- only other hit in
`mod.rs` is a test fixture at line 5413). JSON response built via
`serde_json::json!({...})` with base fields plus conditionally-attached
`batch` and `superseded_branches` keys added only when non-trivial --
existing convention of optional/conditional top-level JSON keys.

### koto init collision (src/cli/mod.rs:1682-1691, ~1708-1716)
Only calls `backend.exists(name)`; header never opened at this
pre-check. Exact message: `"workflow '{}' already exists; run `koto
session cleanup {}` to reuse the name, or `koto cancel --cleanup {}` to
stop a running workflow first"` (JSON `{"error": ..., "command":
"init"}`). A second race-detection collision path emits a shorter
`"workflow '{}' already exists"`.

### SessionInfo / backend list() (src/session/mod.rs, local.rs, cloud.rs)
No separate `LocalBackend`/`CloudBackend` trait-object split at the CLI
layer -- single `SessionBackend` trait implemented by an enum
`Backend { Local(LocalBackend), Cloud(CloudBackend) }`. `SessionInfo`
(src/session/mod.rs:129-141) currently has exactly four fields: `id:
String`, `created_at: String`, `template_hash: String`,
`parent_workflow: Option<String>` -- **no `template_source_dir` field
exists today**; it is never read into `SessionInfo` at all (not "read
then dropped" as originally summarized).

- `LocalBackend::list()` (src/session/local.rs:84-137): calls
  `persistence::read_header()` per session dir; full header **is**
  available in memory, so a `template_source_dir` existence check adds
  zero extra I/O beyond one syscall per session.
- `CloudBackend::list()` (src/session/cloud.rs:677-699): calls
  `self.local.list()` first (full local headers available, same as
  above), then merges in S3-only session IDs with **placeholder**
  `SessionInfo` values (comment: "We can't extract full metadata without
  downloading the state file"). For locally-present sessions (including
  under CloudBackend) the check is free; for remote-only sessions
  surfaced only via S3 key listing, no header is available and an
  existence check is impossible without a full state-file download.

### Decision 14 precedent (DESIGN-batch-child-spawning.md:2579-2734)
Verbatim JSON shape matches scheduler_warning.rs doc comment:
```json
{"kind": "stale_template_source_dir", "path": "/host-a/work",
 "machine_id": "host-b", "falling_back_to": "/host-b/cwd"}
```
Rationale (lines 2635-2639): "Present-but-stale `template_source_dir`
emits a warning. When `Path::new(template_source_dir).exists()` is false
at scheduler start, emit `SchedulerWarning::StaleTemplateSourceDir` and
fall through to `submitter_cwd`. Deduplicated per `template_source_dir`
value per tick." The design doc never discusses `koto status`/`init`/
`session list` in connection with this variant -- scoped entirely to the
batch scheduler.

### koto workflows --orphaned (PRD/DESIGN-hierarchical-workflows)
PRD-hierarchical-workflows.md:486-487 (R2): "`koto workflows --orphaned`
returns workflows whose parent no longer exists." This is unambiguously
the **parent-session-gone** condition -- categorically different from
**template_source_dir-gone**.

### JSON stability precedent
`docs/STABILITY.md` covers on-disk wire format stability (StateFileHeader
additive fields, EventPayload additive variants, four frozen
SessionBackend methods) but doesn't explicitly discuss CLI response JSON
stability. `CHANGELOG.md:138-141` gives direct on-point precedent:
`NextResponse::Terminal`/`Error` gained `unassigned_children:
Vec<UnassignedChild>` -- "Adds a new key in the JSON output; consumers
that ignore unknown keys continue to work." Shipped in a minor release,
not treated as breaking. Direct precedent that adding a new optional key
to `status`/`session list` JSON is the established, safe pattern here.

### Existing "orphan" terminology (already overloaded)
Two other distinct existing uses beyond `--orphaned` itself:
- `koto workflows --orphaned` (src/cli/mod.rs:203-205, 1167, 1191) --
  parent-gone.
- `OrphanCandidate` / `SchedulerFeedback::orphan_candidates`
  (src/cli/batch.rs:303-336, 1201-1229) -- a **third, unrelated**
  meaning: children on disk whose short task name is NOT in the current
  batch submission (renamed/dropped task name detection, Issue #16).
- Scattered orphan-flavored comments elsewhere (sidecar recovery in
  claim.rs, orphaned manifest content in local.rs, orphan template-graph
  nodes in template/compile.rs) unrelated to sessions.

"Orphan" is already overloaded with two semantically distinct,
user-facing meanings. A third orphan-flavored name for
template-source-dir-gone would compound confusion. Strong argument for a
name in the "stale"/"missing source" family already established by
`SchedulerWarning::StaleTemplateSourceDir` / `MissingTemplateSourceDir`,
not an "orphan"-based term.

## Assumptions made
None required -- all critical unknowns were resolved by direct source
inspection. One clarification vs. the original prompt summary:
`SessionInfo` does not currently read-then-discard `template_source_dir`;
it never touches the field at all. This doesn't change the constraints,
only sharpens the "additive-only" framing (there's no existing behavior
to preserve here, only a new field to add).

## Critical unknowns and their resolution
1. **Can CloudBackend's list() cheaply check template_source_dir
   existence?** -- Yes for locally-materialized sessions (full header
   already in memory via self.local.list()), no for remote-only
   placeholder rows (no header at all without a download). This is
   exactly the split flagged for decision 2; for this decision it means
   the chosen shape must tolerate "unknown" as a valid state, not just
   true/false.
2. **Does StaleTemplateSourceDir's shape generalize to non-scheduler
   callers?** -- Yes structurally (path + machine_id + a fallback/context
   field), but its current field semantics (`falling_back_to`) are
   scheduler-specific (what the scheduler substituted). A reused/extended
   signal for status/init/list needs either a renamed generalization of
   that field or an omission of it for non-scheduler contexts.
3. **Is CLI JSON additive change safe here?** -- Yes, confirmed by direct
   CHANGELOG precedent (0.10.0, unassigned_children field addition).
