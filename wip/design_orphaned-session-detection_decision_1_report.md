<!-- decision:start id="orphaned-session-detection-signal-shape" status="assumed" -->
### Decision: Orphan-detection signal shape and wiring

**Context**

`template_source_dir` is an `Option<PathBuf>` field on `StateFileHeader`
(`src/engine/types.rs:260`) recorded once at `koto init`/child-spawn time.
Today it has exactly one consumer: the batch scheduler's path resolver,
which emits `SchedulerWarning::StaleTemplateSourceDir { path: String,
machine_id: Option<String>, falling_back_to: PathBuf }`
(`src/engine/scheduler_warning.rs`, `#[serde(tag = "kind")]` ->
`"kind": "stale_template_source_dir"`) when `Path::exists()` on that
recorded directory returns false. `koto status`, `koto init`'s
already-exists collision check, and `koto session list` never look at
this field at all -- `koto init`'s collision path doesn't even open the
colliding session's header today. `SessionInfo` (`src/session/mod.rs:
129-141`) has four fields and no orphan-flavored signal of any kind.
`koto workflows --orphaned` already means something structurally
different (a workflow whose *parent* no longer exists), and the codebase
already has a second, unrelated meaning for "orphan"
(`OrphanCandidate`/`orphan_candidates` in the batch scheduler, for
task-name drift across submissions) -- "orphan" is a taken, overloaded
word here, not an available one.

Three validators independently argued for direct reuse of
`SchedulerWarning::StaleTemplateSourceDir` (Alternative A), a new
shared-core type derived from the same computation (Alternative B), and
independent per-call-site booleans (Alternative C). After a peer-revision
round where each validator re-verified the others' claims against actual
source, the field converged substantially: all three agreed
`falling_back_to` is a scheduler-only *decision* that doesn't belong,
unmodified, in the three new surfaces, and all three agreed `machine_id`
must travel with the existence check everywhere, not just in the
scheduler (Validator C, who initially proposed dropping it, reversed
that position after confirming `current_machine_id()` costs nothing to
call).

**Assumptions**

- No interactive user was available (run in `--auto`); this decision is
  recorded as `assumed`, not `confirmed`, per the skill's status rule for
  non-interactive runs.
- `Path::exists()` is accepted as sufficient for the existence check
  (already decided upstream; not re-litigated here).
- Decision 2 (backend-specific behavior for `LocalBackend` vs.
  `CloudBackend`) will further tune what happens when a header isn't
  locally available; this decision only requires the chosen shape to
  tolerate "no data" as a valid, distinct state from "checked and fine."
- The project treats new, optional CLI-response JSON fields as additive
  and non-breaking, per direct precedent (`CHANGELOG.md` 0.10.0's
  `unassigned_children` field addition, framed explicitly as safe for
  "consumers that ignore unknown keys").

**Chosen: Alternative B (shared-core extraction), refined to a single shared computation**

Extract the existence check into one shared, synchronous helper --
`fn check_template_source_dir(header: &StateFileHeader) ->
Option<TemplateSourceStatus>`, where `TemplateSourceStatus { path:
PathBuf, exists: bool, machine_id: Option<String> }` -- placed alongside
`persistence::parse_header`/`read_header` or in `path_resolution.rs`.
This helper calls `Path::exists()` and the already-crate-visible
`current_machine_id()` (`pub(crate)`, `src/engine/path_resolution.rs:66`)
exactly once, so there is exactly one place in the codebase that computes
"does this recorded directory exist, and on whose machine."

`SchedulerWarning::StaleTemplateSourceDir`'s own construction
(`path_resolution.rs`, `batch.rs:1789`) is refactored to build *from* this
same helper's result, converting `TemplateSourceStatus.path: PathBuf` to
the enum's existing `path: String` only at that one call site, and
bolting `falling_back_to` on only there -- the scheduler's public JSON
wire format (`kind`, `path`, `machine_id`, `falling_back_to`, all
currently required per an existing hard-coded test) does not change at
all. This is the detail that resolves the sharpest disagreement in the
bakeoff: instead of two types that merely share field names by
convention (the risk B's own validator initially conceded), there is one
computation with two thin, purpose-specific projections -- the scheduler's
existing enum variant on one side, three new-surface consumers on the
other -- eliminating both the "two types can silently drift" risk and
the "three independently hand-rolled copies" risk in one move.

Wiring:
- `SessionInfo` (`src/session/mod.rs:129-141`) gains `pub
  template_source_status: Option<TemplateSourceStatus>` (additive; `None`
  when the header has no recorded `template_source_dir` *or* when no
  header is available at all -- see CloudBackend caveat below; `Some`
  otherwise, with `.exists` telling the reader stale vs. fine).
  `LocalBackend::list()` populates it from the header it already has in
  memory (zero extra I/O beyond the syscall). `CloudBackend::list()`'s
  remote-only placeholder rows get `None`, inheriting the existing
  `created_at`/`parent_workflow` placeholder-sentinel pattern -- no new
  sync round-trip, honoring this decision's scope boundary.
- `koto status` (`handle_status`, `src/cli/mod.rs:4387`) already has the
  header in scope; it calls the shared helper and, only when `exists ==
  false`, adds a conditional top-level `stale_template_source_dir` JSON
  key -- matching the existing convention of `batch`/`superseded_branches`
  appearing only when relevant, rather than always-present-but-often-null.
- `koto init`'s collision pre-check (`src/cli/mod.rs:1682-1691`, today a
  pure `backend.exists(name)` call with no header read) opens the
  colliding session's header -- new, but bounded, I/O -- runs the same
  helper, and when stale, appends a sibling JSON field / clause to the
  message rather than rewriting the existing `"already exists"` string
  (preserving the documented guarantee that callers can rely on a stable
  collision-error string).

Naming: keep the existing "stale_template_source_dir" word family
throughout (Rust field/type names, JSON keys) -- explicitly not "orphan"
anything. No new CLI flag is introduced at any of the three sites (all
three commands already run unconditionally), so there is no possibility
of colliding with `koto workflows --orphaned` or with the batch
scheduler's unrelated `OrphanCandidate`/`orphan_candidates` concept --
avoided by construction rather than by finding a clever alternate word.

**Rationale**

The bakeoff converged on two things nobody disputed by the end: (1)
`falling_back_to` is a scheduler-only decision, not a filesystem fact,
and forcing it (even as `Option<PathBuf>`) into three call sites that
have nothing to fall back to produces a single `kind` tag that
serializes as two silently different shapes depending on which command
emitted it -- exactly the ambiguity a reader of `koto status`'s output
would trip on; and (2) `machine_id` must travel with the check
everywhere, including the three new surfaces, because the scenario it
exists for (a session recorded on one machine, inspected from another
after a cross-machine cloud-sync resume) applies to `koto status`/`koto
session list` exactly as much as to a scheduler tick -- it is not
scheduler-specific, only `falling_back_to` is.

Given both of those, the only remaining question was whether the shared
"fact" (path + exists + machine_id) deserves a named, single-computation
home (B) or should be hand-copied at three sites (C). Once C conceded
`machine_id` must be included, C's proposal became structurally
identical to B's, just triplicated with no compiler- or
function-level enforcement to keep the three copies in sync -- a strictly
worse position with no remaining simplicity advantage. Refining B so the
scheduler's own `StaleTemplateSourceDir` is *constructed from* the shared
helper (rather than merely keeping field names aligned by convention)
closes B's own conceded weakness (the two-types-can-drift risk) and
directly answers the strongest argument for A: A's own validator
conceded that reusing the enum wholesale means any future scheduler-only
field added to `StaleTemplateSourceDir` will, by default, leak into
`status`/`init`/`list` output unless every future contributor remembers
those three non-scheduler consumers exist. The refined-B shape has no
such failure mode: the scheduler's type can grow scheduler-only fields
freely because the three new surfaces depend on the smaller, purpose-built
`TemplateSourceStatus`, not on `SchedulerWarning` itself.

**Consequences**

- One new, small, well-scoped type (`TemplateSourceStatus`) and one
  shared helper function enter the codebase; `SchedulerWarning`'s public
  shape and its existing hard-coded serialization test are untouched.
- `koto init`'s collision path gains new I/O (a header read) it doesn't
  have today -- a real, if bounded, behavior change that should be called
  out explicitly in the implementing PR/design doc, not treated as "just
  wiring."
- `path_resolution.rs`/`batch.rs`'s existing, tested construction of
  `StaleTemplateSourceDir` needs a small refactor to route through the
  shared helper; existing unit tests
  (`stale_base_emits_warning_with_machine_id_and_fallback` and neighbors)
  serve as the regression guard for that refactor and must keep passing
  unchanged.
- The `path: PathBuf` (new type) vs. `path: String` (existing enum)
  mismatch flagged during the bakeoff must be resolved explicitly in the
  design doc as "convert at the scheduler construction site," not left
  implicit.
- The CloudBackend `None`-means-two-things ambiguity ("no
  template_source_dir recorded" vs. "no header available") is inherited,
  unresolved, by design -- it is explicitly deferred to decision 2 in this
  design, and should be documented as a known limitation in the shipped
  code (a doc comment on the field), not silently left ambiguous.
- No new CLI flag exists to collide with `--orphaned`, and the
  "stale_template_source_dir" vocabulary is now consistent across four
  surfaces (scheduler warnings, `koto status`, `koto init`'s collision
  error, `koto session list`) without being the literal same Rust type in
  all four.

**Alternatives Considered**

- **Alternative A (Direct reuse of `SchedulerWarning::StaleTemplateSourceDir`
  as-is across all four call sites):** Cheapest to implement and gives
  perfect compiler-enforced type identity, but rejected because (1) its
  own validator conceded that any future scheduler-only field added to
  the enum will, by default, leak into non-scheduler JSON output, a
  standing coupling risk with no mitigation other than contributor
  memory; and (2) making `falling_back_to` `Option<PathBuf>` produces one
  `kind` tag that serializes as two observably different shapes depending
  on the emitting command, which undercuts the "one recognizable
  vocabulary" benefit reuse is supposed to provide.
- **Alternative C (Independent per-site `Path::exists()` booleans, no
  shared type):** Initially attractive for matching the three call
  sites' genuinely different data-availability profiles, but rejected
  because once its own validator conceded `machine_id` must be included
  (a required part of the constraint, and functionally free to add), the
  alternative collapsed into the same field set as Alternative B, just
  hand-duplicated three times with no shared function or type to prevent
  drift -- strictly dominated by B with no remaining simplicity
  advantage.
<!-- decision:end -->

---

## Consumer Rendering Notes

**Confidence:** medium-high. All three validators converged on the two
substantive design principles (`falling_back_to` stays scheduler-only;
`machine_id` travels everywhere) by the end of peer revision, and the
remaining disagreement (shared type vs. duplicated fields) was resolved
by a refinement (deriving the scheduler's own warning from the same
shared helper) that both B's and C's validators' concerns independently
point toward. The residual open items -- exact JSON key names beyond
"stale_template_source_dir," and the CloudBackend `None`-ambiguity -- are
implementation details / explicitly deferred to decision 2, not
open questions about the chosen shape itself.

**Status:** assumed (run in `--auto` mode, no interactive user
confirmation available).
