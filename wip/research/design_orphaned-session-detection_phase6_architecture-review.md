# Architecture Review: DESIGN-orphaned-session-detection.md

## Scope

Reviewed `docs/designs/DESIGN-orphaned-session-detection.md` in full and
verified every file/line citation and code-behavior claim in "Solution
Architecture" and "Key Interfaces" against the actual koto source tree at
this worktree's HEAD.

## Citation verification

All checked citations are accurate:

- `src/cli/mod.rs:1682` -- `if backend.exists(name) {` (init pre-check),
  block runs through 1691. Confirmed exact.
- `src/cli/mod.rs:1707-1716` (doc's "~1707-1716") -- actual
  `SpawnErrorKind::Collision => {` is at 1708, error string at 1713. Close
  enough to the doc's own "~" hedge.
- `src/cli/mod.rs:4387` -- `fn handle_status(...)`. Exact match.
- `src/cli/session.rs:504` -- `pub fn handle_list(...)`. Exact match.
- `src/session/mod.rs:129-141` -- `pub struct SessionInfo { id, created_at,
  template_hash, parent_workflow }`. Exact match, all four existing fields
  confirmed, no `template_source_status` yet (correctly not-yet-added).
- `src/engine/path_resolution.rs:66` -- `pub(crate) fn current_machine_id()
  -> Option<String>` reads `/etc/machine-id` then falls back to `HOSTNAME`.
  Exact match to the doc's description.
- `src/engine/scheduler_warning.rs` -- `StaleTemplateSourceDir { path:
  String, machine_id: Option<String>, falling_back_to: PathBuf }` with
  `#[serde(tag = "kind", ...)]`. Matches doc exactly, including the
  hard-coded serialization tests the doc says must keep passing
  (`stale_base_emits_warning_with_machine_id_and_fallback` and a sibling
  `machine_id`-omitted test).
- `src/cli/batch.rs:1789` -- confirmed a `StaleTemplateSourceDir` push
  inside `emit_template_source_dir_warnings`. Doc's plain `batch.rs:1789`
  citation (no directory prefix) correctly resolves to `src/cli/batch.rs`,
  not `src/engine/`.
- `src/session/cloud.rs:559` -- `pub fn is_cloud(&self) -> bool`. Exact.
- `CloudBackend::list()` (`src/session/cloud.rs:677-697`) -- confirmed:
  local sessions get full `SessionInfo` via `self.local.list()`; remote-only
  IDs get placeholder rows with `created_at: String::new()`,
  `template_hash: String::new()`, `parent_workflow: None` -- no header
  read at all. This matches the doc's claim precisely and validates
  Decision 2's premise that placeholder rows carry no header data to
  misfire the check on.
- `LocalBackend::list()` (`src/session/local.rs:84-120`) -- confirmed it
  already calls `persistence::read_header` per session and has the header
  in memory when building `SessionInfo`, supporting the doc's "zero extra
  I/O beyond the existence syscall" claim for Phase 3.
- `koto workflows --orphaned` -- confirmed at `src/cli/mod.rs:203-205`
  (`/// Show only orphaned workflows whose parent no longer exists`) and
  used at `src/cli/mod.rs:1191`. The doc's naming-collision concern is real
  and correctly scoped.
- `OrphanCandidate`/`orphan_candidates` -- confirmed in `src/cli/batch.rs`
  (struct at line 329, builder `build_orphan_candidates` at 1203), for
  task-name-drift detection, unrelated to `template_source_dir`. Matches
  doc's characterization.
- `StateFileHeader` (`src/engine/types.rs:223`) confirmed to carry
  `template_source_dir` and no creator-machine identifier, matching
  Decision 2's finding that there's no stored value to compare
  `current_machine_id()` against.
- Second `machine_id` concept: confirmed `src/session/version.rs` has
  `get_or_create_machine_id()` / `version.json` `machine_id` used by
  `src/cli/session.rs:555`, tracking "last machine to write a version
  bump" -- exactly as the doc describes it as an unreliable substitute.

No citation was found to be wrong. This design's factual grounding is
unusually solid for a document produced without an interactive user.

## Finding: a real missing component (Phase 2 is incomplete)

`SchedulerWarning::StaleTemplateSourceDir` is constructed in **two** places
today, not one:

1. `src/engine/path_resolution.rs` (~line 175), inside the per-task
   resolver -- this is the site the doc's Phase 2 explicitly targets for
   refactoring onto the new shared helper.
2. `src/cli/batch.rs::emit_template_source_dir_warnings` (~line 1774-1794),
   the *per-tick* (not per-task) warning emitter called once at the top of
   each scheduler tick (`src/cli/batch.rs:882`). This function independently
   calls `crate::engine::path_resolution::current_machine_id()` and builds
   its own `StaleTemplateSourceDir` value from a `base_exists: Option<bool>`
   computed via a direct `Path::exists()` call at `src/cli/batch.rs:875`.

The design's Solution Architecture component list and Phase 2 deliverables
mention only `path_resolution.rs` and `scheduler_warning.rs`. They do not
mention `src/cli/batch.rs`, so as scoped, this second inline construction
site is left untouched -- it will keep calling `current_machine_id()`
directly and computing its own existence boolean, independent of the new
`check_template_source_dir` helper. This directly contradicts the design's
own stated rationale ("one computation, not four... eliminates both 'two
types can silently drift' and 'three independently hand-rolled copies'
risk") -- after implementation there would still be two hand-rolled
construction paths for the same enum variant, one of which (`batch.rs`) is
completely outside the new shared module's reach.

There's also a signature friction that likely explains the oversight: the
proposed helper's signature, `check_template_source_dir(header: &StateFileHeader)
-> Option<TemplateSourceStatus>`, takes a full header. But `batch.rs`'s
per-tick site doesn't have a header in hand at that point -- it has
`template_source_dir: Option<&Path>` and `submitter_cwd`, already extracted
via `resolution_context(backend, parent_name, events)` from the *parent's*
event log, not a freshly-read header. A header-typed helper can't be called
directly from `batch.rs`'s call site without either re-deriving a header-like
value or overloading the helper's signature (e.g. an inner function taking
`Option<&Path>` plus a pre-computed `Option<bool>`, with the header-typed
public function as a thin wrapper). This is worth resolving explicitly in
Phase 1/2 rather than discovered during implementation -- either by widening
`check_template_source_dir`'s signature or adding a second entry point that
both `path_resolution.rs` and `batch.rs` can share.

Recommend: add `src/cli/batch.rs` to the Solution Architecture's component
list and to Phase 2's deliverables, with an explicit note on how
`emit_template_source_dir_warnings` will call into the shared module given
its `Option<&Path>` + pre-computed `bool`, not `&StateFileHeader`, input
shape.

## Phase sequencing

The five phases are correctly ordered for risk and dependency reasons:
build the isolated, unit-testable core first (Phase 1); refactor the
existing, tested scheduler consumer next as a regression-guarded step
(Phase 2, though incomplete per above); extend the data model (Phase 3);
wire the two read-only surfaces (Phase 4); and land the one surface with a
real behavior change -- new I/O on `koto init`'s collision path (Phase 5)
-- last, after the shared logic has already been exercised by three other
call sites. No phase depends on a later one. The only correction needed is
closing the Phase 2 gap above before implementation starts, since Phase 2
is meant to be the regression-guarded proof that the shared helper produces
identical output to today's inline logic -- that proof is incomplete while
a second inline site exists unexamined.

## Missing interfaces / components (beyond the batch.rs gap)

- No interface is specified for how `handle_list`'s formatting helper
  (Phase 4, "shared wording-formatting helper") is shared between
  `handle_status` and `handle_list` -- the doc says both consult it but
  doesn't name a function signature or module location the way it does for
  `check_template_source_dir`. Minor, but worth a one-line signature in Key
  Interfaces for consistency with the rest of the doc's precision.
- The doc doesn't specify whether `koto init`'s new header-read (Phase 5)
  should reuse `persistence::read_header` (confirmed to exist and already
  used by `LocalBackend::list()`) or a different accessor -- almost
  certainly the former, but not stated explicitly the way other call sites
  are.

## Simpler alternatives overlooked?

No. The doc's own alternatives section (direct enum reuse vs. three
hand-duplicated booleans) already covers the natural simpler options and
gives concrete, source-grounded reasons for rejecting both -- direct reuse
would leak scheduler-only fields into non-scheduler JSON and force
`falling_back_to` to become optional (verified: `falling_back_to: PathBuf`
is indeed non-optional in the current enum), and hand-duplication was
shown to collapse to the same shape once `machine_id` is included properly.
One genuinely simpler adjustment surfaced by the batch.rs finding above:
widening `check_template_source_dir` to accept `Option<&Path>` directly
(with the header-accepting version as a thin wrapper around it) would let
both existing scheduler call sites *and* the three new ones share one
function, which is simpler than the current plan's implicit assumption
that only one scheduler site needs touching.

## Overall assessment

Yes, implementable as written, with one correction needed before Phase 2
starts: extend Phase 2's scope (and the Solution Architecture's component
list) to cover `src/cli/batch.rs`'s independent `StaleTemplateSourceDir`
construction site, and settle the helper's parameter shape (header vs.
bare path) so both scheduler call sites can route through it. Everything
else -- decisions, rationale, phase ordering, security considerations --
is well-supported by the actual source and internally consistent.
