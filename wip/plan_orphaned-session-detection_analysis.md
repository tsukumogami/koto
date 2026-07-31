# Plan Analysis: DESIGN-orphaned-session-detection

## Source Document
Path: docs/designs/DESIGN-orphaned-session-detection.md
Status: Accepted
Input Type: design

## Scope Summary

Extend koto's existing (but scheduler-only) `template_source_dir` staleness
check to `koto init`'s collision path, `koto status`, and `koto session
list`, via a new shared `TemplateSourceStatus` core reused by both existing
scheduler construction sites, with backend-aware wording so cross-machine
cloud-sync resume isn't misreported as a dead session. No new CLI flags,
no automatic cleanup/gc (explicitly deferred).

## Components Identified

- **`src/engine/template_source_status.rs`** (new): `TemplateSourceStatus`
  struct plus `check_template_source_path(Option<&Path>)` (core) and
  `check_template_source_dir(&StateFileHeader)` (thin wrapper).
- **`src/engine/scheduler_warning.rs`** (modified): `StaleTemplateSourceDir`
  construction built from `TemplateSourceStatus` instead of computing
  `Path::exists()`/`current_machine_id()` inline; public JSON shape
  (`kind`, `path`, `machine_id`, `falling_back_to`) unchanged.
- **`src/engine/path_resolution.rs`** (modified): per-task resolver calls
  `check_template_source_path` instead of computing existence itself.
- **`src/cli/batch.rs`** (modified): `emit_template_source_dir_warnings`
  (~line 1774, called from ~line 882) -- the second, independent existing
  construction site -- switches to the same core function.
- **`src/session/mod.rs`** (modified): `SessionInfo` gains an additive
  `pub template_source_status: Option<TemplateSourceStatus>` field.
- **`src/session/local.rs`** (modified): `LocalBackend::list()` populates
  the new field from the in-memory header (no new I/O).
- **`src/session/cloud.rs`** (modified): `CloudBackend::list()` leaves the
  field `None` for remote-only placeholder rows; populates normally for
  rows already synced locally. Backend-aware message wording (checks
  `Backend::is_cloud()`) lives here or in the formatting helper it feeds.
- **`src/cli/mod.rs`** (modified): `handle_status` adds a conditional
  `stale_template_source_dir` JSON key; both `koto init` collision paths
  (pre-check ~line 1682, `SpawnErrorKind::Collision` handler ~line 1707)
  open the colliding session's header and append the same staleness
  clause to their respective existing messages.
- **`src/cli/session.rs`** (modified): `handle_list` surfaces
  `template_source_status` from each `SessionInfo` row.

## Implementation Phases (from design)

### Phase 1: Shared status module
Add `src/engine/template_source_status.rs` with `TemplateSourceStatus` and
`check_template_source_path`/`check_template_source_dir`. No behavior
change yet -- unit-tested in isolation against constructed
`StateFileHeader`/path values (present/absent, existing/missing directory).

### Phase 2: Refactor both scheduler construction sites to consume the shared module
Both existing places that construct `StaleTemplateSourceDir` move to the
shared core: `path_resolution.rs`'s per-task resolver (header-accepting
wrapper) and `batch.rs`'s `emit_template_source_dir_warnings` (core
function directly, since it only has `Option<&Path>` in scope). No
wire-format change; existing tests
(`stale_base_emits_warning_with_machine_id_and_fallback` and neighbors)
must keep passing unchanged as the regression guard.

### Phase 3: `SessionInfo` and both `list()` backends
Add `template_source_status` to `SessionInfo`. Populate in
`LocalBackend::list()` from the in-memory header. Leave `None` in
`CloudBackend::list()`'s remote-only placeholder rows; populate normally
for rows already synced locally. Add a doc comment on the new field noting
the `CloudBackend` None-means-two-things limitation.

### Phase 4: `koto status` and `koto session list` output
Wire `handle_status` to add the conditional `stale_template_source_dir`
JSON key. Wire `handle_list` to surface `template_source_status`.
Implement backend-aware wording (Decision 2) as a small shared formatting
function consulted by both.

### Phase 5: `koto init` collision messaging
Update both collision paths to open the colliding session's header, run
the shared check, and append the same staleness clause to both (their base
messages are not byte-identical today and this design does not unify
them -- only the new clause must be present on both).

## Success Metrics

**Positive** (from Consequences):
- Fixes the concrete bug in tsukumogami/koto#189: a same-named `koto init`
  colliding with a dead session gets a diagnosable message instead of a
  generic "already exists" error.
- `koto session list` gains passive staleness visibility with no new
  network cost.
- No new CLI flags, no new "orphan"-named surface.
- Batch scheduler's existing, tested behavior preserved exactly (same wire
  format, same test suite passing unmodified).

**Accepted trade-offs / known limitations** (from Consequences, not
success criteria but must not regress):
- `koto init`'s collision pre-check gains new I/O (a header read).
- `CloudBackend`'s `None` is ambiguous between "never recorded" and "no
  header available" -- documented, not resolved.
- Backend-aware wording only softens language; cannot prove cross-machine
  vs. deleted.
- `Path::exists()`'s `stat()` can block on hung mounts; this design pays
  that cost once per session on every `LocalBackend::list()` call
  (including the dashboard's ~500ms poll), not just once per scheduler
  tick -- documented as an accepted, deferred risk, not something this
  plan's issues need to fix.

## External Dependencies

- **`docs/designs/current/DESIGN-batch-child-spawning.md` Decision 14**:
  source of the `SchedulerWarning::StaleTemplateSourceDir` type,
  `current_machine_id()`, and the existing regression test suite this
  work must not break.
- **No new external crate dependencies.**
- **Existing tests as regression guard**: `stale_base_emits_warning_with_machine_id_and_fallback`
  and neighbors in the scheduler test suite.
