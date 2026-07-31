---
status: Current
problem: |
  A koto session's state header already records the `template_source_dir`
  it was initialized from, but nothing reads that field back to check
  whether the directory still exists on `koto status`, `koto init`'s
  "already exists" collision path, or `koto session list`. A session
  created inside a working tree that's later torn down (reaped ephemeral
  sandbox, `git worktree remove`, container teardown) stays "live" from
  koto's point of view forever, and a later `koto init <same-name>` from an
  unrelated environment collides with it and fails with a generic
  "already exists" error indistinguishable from a real concurrent session
  (tsukumogami/koto#189). Koto already ships almost this exact mechanism --
  `SchedulerWarning::StaleTemplateSourceDir`, from Decision 14 in
  DESIGN-batch-child-spawning.md -- but scoped narrowly to the batch
  scheduler's per-tick child-template-path resolution, invisible to the
  three call sites where this issue actually bites. This design decides how
  to extend that existing pattern to `init`/`status`/`session list`: where
  the new signal lives, what it's named (given `koto workflows --orphaned`
  already means something unrelated), and how it behaves across the
  already-shipped local vs. cloud-sync backends.
decision: |
  Extract the existence/staleness check into one shared core --
  `TemplateSourceStatus { path, exists, machine_id }` plus
  `check_template_source_path`/`check_template_source_dir` in a new
  `src/engine/template_source_status.rs`, plus a thin `Backend::is_cloud()`
  accessor -- and route the scheduler's existing per-tick existence probe
  and its `StaleTemplateSourceDir` warning construction (`batch.rs`) through
  the shared core (the per-task resolver in `path_resolution.rs` is
  explicitly not touched -- it only ever received a pre-computed boolean,
  never an independent existence check to consolidate), alongside three
  new consumers: `SessionInfo` gains an additive
  `Option<TemplateSourceStatus>` field populated by `LocalBackend::list()`
  (left `None` for `CloudBackend`'s remote-only placeholder rows), `koto
  status` adds a conditional `stale_template_source_dir` JSON key, and
  `koto init`'s two "already exists" collision paths each open the
  colliding session's header and append the same staleness clause.
  Message wording branches on `Backend::is_cloud()` to soften language for
  cloud-synced sessions without suppressing or altering the underlying
  check. No new CLI flag is introduced anywhere, and the word "orphan" is
  never used in any new field or flag name, avoiding collision with the
  existing `koto workflows --orphaned` flag and the scheduler's unrelated
  `OrphanCandidate` concept by construction. An automatic sweep/gc/cleanup
  of orphaned sessions (the source issue's third candidate direction)
  remains explicitly out of scope.
rationale: |
  A shared core beats both direct reuse of the scheduler's existing
  `SchedulerWarning::StaleTemplateSourceDir` enum (which would let future
  scheduler-only fields leak into non-scheduler output by default) and
  independent per-call-site booleans (which collapsed into the same field
  set once `machine_id` was correctly included, just hand-duplicated with
  no drift protection) -- three independent validators converged on this
  after cross-examining each other's claims against the actual source.
  Backend-aware wording, not suppression, is the right posture for
  cross-machine cloud-sync false positives because no cheap, reliable
  per-session signal exists today to prove "deleted" versus "resumed
  elsewhere" -- the scheduler's own `machine_id` field turned out to be an
  observational label, not a stored comparison, once verified directly
  against the code. A bare `Path::exists()` check is accepted as
  sufficient because this design is read-only/informational throughout;
  the failure modes a stronger check would catch (directory reuse,
  transient remounts) only matter for destructive action, which stays out
  of scope. Phase 6 review caught a second scheduler construction site
  this design would otherwise have missed (`batch.rs`), a factual error in
  an earlier draft's collision-message-identity claim, and a stat-latency
  availability risk from computing the check on every `list()` call
  rather than once per scheduler tick -- all three are corrected or
  documented in the final design rather than left to surface during
  implementation.
---

# DESIGN: Orphaned Session Detection

## Status

Current

## Context and Problem Statement

A koto session's state header records `template_source_dir` -- the
directory a session was initialized from -- but three commands that should
care whether that directory still exists never check it:

- **`koto init`'s "already exists" collision path** (`src/cli/mod.rs:1682-1691`)
  is a pure `backend.exists(name)` filesystem-presence test; it never opens
  the colliding session's header, so it cannot tell a real concurrent
  session apart from a dead one whose originating working tree was reaped.
- **`koto status`** (`handle_status`, `src/cli/mod.rs:4387`) already reads
  the full header but never inspects `template_source_dir`.
- **`koto session list`** (`handle_list`, `src/cli/session.rs:504`) reads
  each session's header via `persistence::read_header`, then discards
  `template_source_dir` when projecting into `SessionInfo`
  (`src/session/mod.rs:129-141`) -- the field is read off disk and thrown
  away before it reaches the CLI.

The practical consequence (tsukumogami/koto#189): a session created inside
a working tree that later gets torn down (a reaped ephemeral sandbox, a
removed git worktree, a container teardown) stays "live" from koto's
perspective forever. A later `koto init <same-name>` from an unrelated
environment collides with it and fails with a generic
`"workflow '<name>' already exists"` error -- indistinguishable from a real
concurrent session. The only way to tell the two apart today is to
manually open the colliding session's raw state JSONL and check by hand
whether `template_source_dir` still resolves.

The codebase already solved a version of this problem once, just scoped
narrowly. Decision 14 of `docs/designs/current/DESIGN-batch-child-spawning.md`
added `SchedulerWarning::StaleTemplateSourceDir { path, machine_id,
falling_back_to }` (`src/engine/scheduler_warning.rs`), emitted when the
batch scheduler's per-tick child-template-path resolver
(`src/engine/path_resolution.rs`) finds a recorded `template_source_dir`
that `Path::exists()` reports as gone. That mechanism is invisible outside
scheduler ticks -- it never fires for `init`, `status`, or `session list`,
which is exactly where issue #189's repro lives.

This design decides how to extend that existing, working pattern to the
three call sites where it's missing: what shape the new signal takes and
where it lives in the codebase, what it's named (given `koto workflows
--orphaned` already means something structurally different -- a workflow
whose *parent* no longer exists, not a session whose *originating
directory* is gone), and how it should behave given that cross-machine
session resumption via `CloudBackend`'s cloud sync is a shipped, documented
feature (`docs/guides/cloud-sync-setup.md`) and the single largest source of
legitimate false positives for this exact signal.

Explicitly out of scope: any automatic sweep/gc/cleanup of orphaned
sessions. See Decisions Already Made below.

## Decision Drivers

- **Reuse over reinvention.** The batch scheduler already solved "does
  `template_source_dir` still exist" once; a second, differently-shaped
  answer to the same question would leave the codebase with two
  incompatible ways to describe the same fact.
- **No new incorrect-by-default coupling.** Whatever shape is chosen must
  not let scheduler-only concerns (like the scheduler's `submitter_cwd`
  fallback decision) leak into `status`/`init`/`session list`, which have
  nothing to fall back to.
- **Read-only for this design; destructive action is out of scope.** A
  bare `Path::exists()` check is accepted as sufficient because directions
  1 and 2 (status/init messaging, list surfacing) are informational only.
  A stronger, fingerprint-based check would only be needed for destructive
  cleanup, which this design explicitly defers.
- **Cross-machine resume must not be misreported as certain deletion.**
  `CloudBackend` sessions resumed on a different machine legitimately have
  a `template_source_dir` that only ever existed on the originating
  machine. The signal's wording must not overclaim what a plain existence
  check can prove.
- **No new CLI surface where avoidable, and no name collisions.** The
  existing `koto workflows --orphaned` flag and the batch scheduler's
  separate `OrphanCandidate`/`orphan_candidates` concept already occupy the
  word "orphan" for two different things; a third meaning would be a real
  source of operator confusion.
- **Additive, non-breaking wire changes.** New JSON fields on
  `koto status`/`koto session list` output must be additive (present only
  when relevant, or `Option`-typed), consistent with the project's existing
  precedent for evolving CLI response shapes (e.g. CHANGELOG 0.10.0's
  `unassigned_children` addition).

## Decisions Already Made

- **Direction 3 (an automatic sweep/gc/cleanup-by-staleness command) is out
  of scope for this design.** It's the only one of the issue's three
  candidate directions that requires a genuine new architecture decision
  (a whole-backend destructive sweep, which koto has no precedent for --
  the closest existing bulk-destructive verb, `koto workspace prune`, is
  root-scoped only), and it depends on this design's detection logic as a
  prerequisite. Deferred to a follow-up issue once directions 1+2 ship.
- **A bare `Path::exists()` check is sufficient for this design's scope.**
  Directions 1 and 2 are both read-only/informational (a status message, a
  list column/flag) -- koto's own existing `StaleTemplateSourceDir`
  precedent already accepts this same limitation for the same field, for
  the same reason: the failure modes a stronger check would catch
  (transient remounts, directory reuse, renames) matter for destructive
  action, which is out of scope per the decision above.
- **The fix should extend/reuse `StaleTemplateSourceDir`'s existing shape
  and `machine_id` vocabulary** (`src/engine/scheduler_warning.rs`,
  `src/engine/path_resolution.rs`) rather than invent a second,
  differently-shaped signal for the same underlying condition, so the
  codebase doesn't end up with two incompatible ways to say "this recorded
  directory doesn't exist here."
- **Whatever flag/column direction 2 adds must not be named `--orphaned`**,
  or must clearly disambiguate from the existing, differently-scoped
  `koto workflows --orphaned` flag (documented in
  PRD-hierarchical-workflows.md / DESIGN-hierarchical-workflows.md, meaning
  "parent workflow no longer exists") in both CLI help text and wire
  format.

These decisions were made during the `/explore` workflow that preceded this
design (six research leads covering current plumbing, candidate-direction
sizing, staleness-check robustness, opt-in posture, and prior art in koto's
own docs and comparable tools); their findings are folded into the Context,
Considered Options, and Solution Architecture sections below.

## Considered Options

### Decision 1: Orphan-detection signal shape and wiring

`template_source_dir` has exactly one consumer today: the batch
scheduler's path resolver, which builds `SchedulerWarning::StaleTemplateSourceDir
{ path: String, machine_id: Option<String>, falling_back_to: PathBuf }`
(`#[serde(tag = "kind")]` -> `"kind": "stale_template_source_dir"`) when
`Path::exists()` on the recorded directory returns false. `koto status`,
`koto init`'s collision check, and `koto session list` never look at this
field, and `SessionInfo` (`src/session/mod.rs:129-141`) has no
orphan-flavored signal at all. "Orphan" is already a taken word in this
codebase: `koto workflows --orphaned` means a workflow whose *parent* no
longer exists, and the batch scheduler separately has an unrelated
`OrphanCandidate`/`orphan_candidates` concept for task-name drift across
submissions.

Three alternatives were evaluated in depth, then cross-examined by
independent validators who revised their positions after verifying each
other's claims directly against the source. Two points survived that
process undisputed: `falling_back_to` is a scheduler-only *decision*
(what to fall back to when resolution fails) that has no meaning at the
three new call sites, which have nothing to fall back to; and `machine_id`
must travel with the existence check everywhere, not just the scheduler,
because the cross-machine cloud-sync scenario it exists for applies to
`koto status`/`koto session list` exactly as much as to a scheduler tick.

**Key assumptions:**
- No interactive user was available during this design (run in `--auto`
  mode); this decision is recorded as "assumed," not "confirmed."
- `Path::exists()` is accepted as sufficient (a prior, separate decision;
  see Decisions Already Made).
- The chosen shape must tolerate "no data available" as a state distinct
  from "checked and fine," since `CloudBackend`'s remote-only placeholder
  rows have no header to check at all (see Decision 2).
- New, optional CLI-response JSON fields are treated as additive and
  non-breaking, per the project's own precedent.

#### Chosen: Shared-core extraction (refined)

Extract the existence check into one shared, synchronous helper --
`fn check_template_source_dir(header: &StateFileHeader) ->
Option<TemplateSourceStatus>`, where `TemplateSourceStatus { path:
PathBuf, exists: bool, machine_id: Option<String> }` -- placed alongside
`persistence::parse_header`/`read_header` or in `path_resolution.rs`. The
helper calls `Path::exists()` and the already-crate-visible
`current_machine_id()` (`pub(crate)`, `src/engine/path_resolution.rs:66`)
exactly once, so there is exactly one place in the codebase that computes
"does this recorded directory exist, and on whose machine."

`SchedulerWarning::StaleTemplateSourceDir`'s construction in
`batch.rs::emit_template_source_dir_warnings` (`~line 1789`) is refactored
to build *from* this same helper's result -- converting
`TemplateSourceStatus.path: PathBuf` to the enum's existing `path: String`
at that call site, and bolting `falling_back_to` on there. (The *other*
existing construction site, `path_resolution.rs`'s per-task resolver, is
not refactored by this design -- see Solution Architecture's correction.)
The scheduler's public JSON wire format
(`kind`, `path`, `machine_id`, `falling_back_to`, all currently required
per an existing hard-coded serialization test) does not change at all.

Wiring at the three new call sites:
- **`SessionInfo`** (`src/session/mod.rs:129-141`) gains `pub
  template_source_status: Option<TemplateSourceStatus>` (additive; `None`
  when the header has no recorded `template_source_dir` *or* when no
  header is available at all -- see Decision 2 for the `CloudBackend`
  handling). `LocalBackend::list()` populates it from the header it
  already has in memory, at zero extra I/O beyond the existence syscall.
- **`koto status`** (`handle_status`, `src/cli/mod.rs:4387`) already has
  the header in scope; it calls the shared helper and, only when `exists
  == false`, adds a conditional top-level `stale_template_source_dir` JSON
  key -- matching the existing convention of `batch`/`superseded_branches`
  appearing only when relevant, rather than always-present-but-often-null.
- **`koto init`'s collision pre-check** (`src/cli/mod.rs:1682-1691`, today
  a pure `backend.exists(name)` call with no header read) opens the
  colliding session's header -- new, but bounded, I/O -- runs the shared
  helper, and when stale, appends a sibling field/clause to the message
  rather than rewriting the existing `"already exists"` string (preserving
  the documented guarantee that callers can rely on a stable
  collision-error string).

Naming keeps the existing `stale_template_source_dir` word family
throughout (Rust field/type names, JSON keys) -- explicitly not "orphan"
anything. No new CLI flag is introduced at any of the three sites (all
three commands already run unconditionally), so there is no possibility of
colliding with `koto workflows --orphaned` or the scheduler's
`OrphanCandidate` concept -- avoided by construction, not by finding a
clever alternate word.

This closes the sharpest disagreement in the bakeoff: instead of two types
that merely share field names by convention, there is one computation with
two thin, purpose-specific projections -- the scheduler's existing enum
variant on one side, the three new-surface consumers on the other -- which
eliminates both "two types can silently drift" and "three independently
hand-rolled copies" risk in the same move. It also directly answers the
strongest argument for direct reuse (see Alternatives below): because the
three new surfaces depend on the smaller, purpose-built
`TemplateSourceStatus` rather than on `SchedulerWarning` itself, the
scheduler's type remains free to grow scheduler-only fields without any
risk of those fields leaking into non-scheduler output by default.

#### Alternatives Considered

**Direct reuse of `SchedulerWarning::StaleTemplateSourceDir` as-is across
all four call sites**: cheapest to implement and gives perfect
compiler-enforced type identity between the scheduler's existing warning
and the three new surfaces. Rejected because reusing the enum wholesale
means any future scheduler-only field added to it leaks into non-scheduler
JSON output by default unless every future contributor remembers those
three non-scheduler consumers exist -- a standing coupling risk with no
mitigation but memory. It also forces `falling_back_to` to become
`Option<PathBuf>` so non-scheduler emitters can omit it, which produces one
`kind` tag that serializes as two observably different shapes depending on
which command emitted it, undercutting the "one recognizable vocabulary"
benefit reuse is supposed to provide.

**Independent per-call-site `Path::exists()` booleans, no shared type**:
initially attractive for matching the three call sites' genuinely
different data-availability profiles (each already has different
information in hand). Rejected because once corrected to include
`machine_id` -- a required part of the constraint, and functionally free to
call -- the alternative became structurally identical to the chosen shape,
just hand-duplicated three times with no shared function or type to
prevent the three copies from drifting apart over time. Strictly dominated
by the chosen shared-core approach with no remaining simplicity advantage.

### Decision 2: Backend differentiation for local vs. cloud-sync sessions

`CloudBackend` makes cross-machine session resumption a shipped, documented
workflow (`docs/guides/cloud-sync-setup.md`): a session created on machine
A and resumed on machine B legitimately has a `template_source_dir` that
only ever existed on A. This decision asks whether the new signal's
computation, wording, or suppression should differ between `LocalBackend`
and `CloudBackend` to avoid misreporting a healthy cross-machine resume as
a dead session, without adding new S3 round trips to `koto session list`.

Direct code research overturned part of the premise this decision started
with: `StaleTemplateSourceDir`'s `machine_id` field, initially assumed to
distinguish "deleted" from "cross-machine," does not actually perform that
comparison today -- `current_machine_id()` is a cheap, ephemeral local read
used purely as an observational label ("here's which machine noticed
this"), not a stored value compared against anything. `StateFileHeader` has
no creator-machine identifier at all. A second, unrelated `machine_id`
concept (`get_or_create_machine_id()` / `version.json.machine_id`) records
the *last machine to write a version bump*, not the machine that recorded
`template_source_dir`, and is frequently absent for read-only sessions --
not a reliable substitute. `CloudBackend::list()`'s remote-only placeholder
rows also turned out not to be where the real false positive happens (they
carry no header data to misfire on); the actual common case is a session
whose header has already been pulled to a second machine via
`sync_pull_state`, at which point the row is an indistinguishable,
fully-populated local session whose `template_source_dir` happens to
belong to another host.

**Key assumptions:**
- Decision 1's chosen `SessionInfo` field uses `Option` semantics
  (matching `parent_workflow`), not the `String::new()` sentinel idiom
  `created_at`/`template_hash` use elsewhere in the same struct -- Decision
  1 confirms this holds. If it hadn't, every `CloudBackend` remote-only
  placeholder row would misfire the check regardless of any wording layered
  on top.
- "No new round trips" is a hard constraint that also rules out inventing
  new persisted state solely to serve this decision.

#### Chosen: Backend-aware wording only, gated on a free discriminant

Keep the check's computation identical across backends: evaluate
`Path::exists()` only when `template_source_dir` is known (`Some`), never
when absent/unknown, regardless of which backend produced the row. Do not
suppress or skip the check for `CloudBackend` sessions specifically -- a
fully-downloaded session with a genuinely stale `template_source_dir` is
just as worth flagging as a local one.

The one place backend matters is message text. When `Backend::is_cloud()`
is true (a thin, zero-cost accessor added to the `Backend` enum -- see
Solution Architecture's correction on this point -- delegating to
`CloudBackend::is_cloud()` at `cloud.rs:559-561`, no I/O and no persisted
new field), soften the wording to
acknowledge the ambiguity instead of asserting deletion -- e.g. "template
source directory not found (if this session was synced from another
machine, this may be expected)" -- rather than a flat "no longer exists."
For `LocalBackend` sessions, where cross-machine resume isn't a possible
explanation (a session can't be resumed on another machine without cloud
sync), keep the more direct wording.

This is a static, backend-type-level hedge applied uniformly to every
`CloudBackend` session's warning -- not a per-session claim that the
cross-machine explanation applies to this specific session. No cheap,
reliable per-session signal exists to make that stronger claim honestly;
the backend-type discriminant is the only thing that's genuinely free.

#### Alternatives Considered

**Uniform behavior (identical check, identical wording, no backend
awareness)**: rejected because it discards a zero-cost signal already
available on every session row, producing needlessly alarming wording for
a common, shipped, healthy workflow (cross-machine resume) with no
offsetting simplicity benefit -- the backend-aware branch is a single
conditional at message-formatting time, not a structural complication.

**Machine-id cross-reference via a new persisted creator-machine-id**:
rejected because it requires inventing new persisted state and a new write
path in `StateFileHeader`/`init` -- a materially larger, separately-scoped
change than adjusting an existing signal's computation/wording/suppression
-- and because the codebase's existing `version.json.machine_id` field
demonstrates that machine-id fields drift (tracking "last touched by"
rather than "created by") unless deliberately designed to be immutable
post-init, a risk this decision doesn't need to take on to deliver most of
the benefit via wording alone.

## Decision Outcome

**Chosen: Shared-core `TemplateSourceStatus` extraction (Decision 1) +
backend-aware wording only (Decision 2)**

### Summary

A new, small type -- `TemplateSourceStatus { path: PathBuf, exists: bool,
machine_id: Option<String> }` -- and one shared helper,
`check_template_source_dir(header: &StateFileHeader) ->
Option<TemplateSourceStatus>`, become the single place in the codebase that
answers "does this session's recorded source directory still exist, and on
whose machine." The batch scheduler's existing
`SchedulerWarning::StaleTemplateSourceDir` is refactored to build from this
same helper's result instead of computing the fact inline a second way;
its own public JSON shape (`kind`, `path`, `machine_id`, `falling_back_to`)
is untouched, and its existing hard-coded serialization test keeps passing
unchanged.

Three call sites wire into the shared helper. `SessionInfo` gains an
additive `Option<TemplateSourceStatus>` field, populated by
`LocalBackend::list()` from the header it already holds in memory (no new
I/O) and left `None` by `CloudBackend::list()`'s remote-only placeholder
rows (no new sync round-trip). `koto status` calls the helper and adds a
conditional `stale_template_source_dir` JSON key only when the directory is
confirmed missing. `koto init`'s "already exists" collision check gains new
(bounded) I/O -- it now opens the colliding session's header, runs the
helper, and appends a sibling clause to the existing, stable
`"already exists"` error string rather than rewriting it.

Message wording is the one place backend type matters: sessions where
`Backend::is_cloud()` is true get softened wording acknowledging that a missing
directory may reflect a legitimate cross-machine resume, using a static,
already-in-hand discriminant, while `Backend::Local` sessions -- for which
cross-machine resume isn't possible -- keep more direct wording. The
underlying `Path::exists()` computation never differs by backend; only the
words describing the result do. No CLI flag is introduced anywhere, and the
word "orphan" is never used in any new field, message, or flag name --
the existing `stale_template_source_dir` vocabulary is reused throughout,
avoiding the `koto workflows --orphaned` and scheduler `OrphanCandidate`
collisions by construction.

Direction 3 from the source issue (an automatic sweep/gc/cleanup command
for orphaned sessions) remains explicitly out of scope; this design only
covers read-only, informational surfacing.

### Rationale

The two decisions reinforce each other cleanly. Decision 1 settles what the
signal *is* and where it lives; Decision 2 settles how its *words* should
change by context, without touching the underlying check. Because Decision
1 committed to `Option` semantics on the new `SessionInfo` field (mirroring
the existing `parent_workflow` idiom, not the `String::new()` sentinel
idiom used elsewhere in the same struct), Decision 2's central risk --
`CloudBackend` placeholder rows misfiring the check universally -- doesn't
materialize; cross-validation confirmed no conflict between the two
decisions' assumptions.

The combination accepts one real trade-off: `koto init`'s collision path
gains I/O it doesn't have today (a header read on the colliding session).
This is a deliberate, bounded cost, not an oversight -- it's the minimum
required to turn a generic "already exists" error into a diagnosable one,
which is the concrete bug this design exists to fix. The `CloudBackend`
`None`-means-two-things ambiguity ("no `template_source_dir` recorded" vs.
"no header available to check") is inherited and left unresolved by
design; it should be documented as a known limitation via a doc comment on
the field, not silently papered over.

## Solution Architecture

### Overview

One new, small module owns the fact this design is about: whether a
session's recorded `template_source_dir` still resolves, and on whose
machine. Everything else -- the batch scheduler's existing warning, and the
three new call sites -- consumes that one computation rather than
recomputing or reinventing it.

### Components

- **`src/engine/template_source_status.rs`** (new file): defines
  `TemplateSourceStatus { path: PathBuf, exists: bool, machine_id:
  Option<String> }`, two functions -- a core `fn
  check_template_source_path(path: Option<&Path>) ->
  Option<TemplateSourceStatus>` and a thin wrapper `fn
  check_template_source_dir(header: &StateFileHeader) ->
  Option<TemplateSourceStatus>` that extracts `header.template_source_dir`
  and delegates to the core -- plus `fn
  format_stale_template_source_note(is_cloud: bool) -> &'static str`, the
  shared wording helper both `koto status`/`koto session list` (below) and
  `koto init`'s collision messages consume. Placing the wording helper
  here too (rather than in `src/cli/mod.rs`, where an earlier draft put
  it) is itself a plan-review correction (Category D): `koto init`'s
  messaging depends only on this module, not on the `koto status`/`session
  list` work, so a helper defined alongside the accessor it takes as input
  is reachable from both without a cross-issue ordering hazard. The core
  takes `Option<&Path>`, not a header,
  because **there are two existing scheduler call sites, not one** (see
  below), and one of them has no header in scope at its call site --
  discovered during Phase 6 architecture review, corrected here before this
  was two functions maintained in parallel instead of one. Placed as its
  own module rather than folded into `persistence.rs` (owns serialization,
  not policy) or `path_resolution.rs` (owns the per-task resolver's
  fallback policy specifically, which this check is not part of) -- all
  consumers depend on this module without depending on each other.
- **`src/engine/scheduler_warning.rs`** (existing, modified): `SchedulerWarning::StaleTemplateSourceDir`'s
  constructor now accepts a `TemplateSourceStatus` (or is built from one)
  rather than computing `Path::exists()`/`current_machine_id()` inline,
  converting `TemplateSourceStatus.path: PathBuf` to its own `path: String`
  field and attaching `falling_back_to` at each call site. The variant's
  public shape and `#[serde(tag = "kind")]` output are unchanged.
- **`src/engine/path_resolution.rs`** (existing, **not modified** by this
  design -- see the correction below). `current_machine_id()` (line 66)
  stays exactly where it is; the shared module's core function calls it,
  not the other way around.
- **`src/cli/batch.rs`** (existing, modified, narrower scope than an
  earlier draft claimed): the scheduler's *single* existing existence
  probe (`~line 875-876`, `template_source_dir_exists =
  template_source_dir.as_deref().map(|p| p.exists())`, run once per tick)
  switches to `check_template_source_path(template_source_dir.as_deref())`,
  then derives the boolean for existing downstream consumers via
  `.map(|s| s.exists)` -- so the one real filesystem probe in this whole
  path now goes through the shared core. `emit_template_source_dir_warnings`
  (`~line 1774`, called once per tick from `~line 882`) is updated to take
  the resulting `Option<TemplateSourceStatus>` directly and build its
  `StaleTemplateSourceDir` warning from it, rather than taking a bare
  `base_exists: Option<bool>` and calling `current_machine_id()` itself.
  This is the one and only construction-site change on the scheduler side.

  **Correction from Phase 6 plan review**: an earlier draft of this design
  additionally claimed `path_resolution.rs`'s per-task resolver
  (`resolve_template_path_with_base_status`) is a *second* independent
  `StaleTemplateSourceDir` construction site that this design refactors.
  That claim doesn't survive contact with the actual call graph:
  `resolve_template_path_with_base_status` takes `base_exists:
  Option<bool>` as a parameter from its callers (`spawn_ready_task`,
  `spawn_skip_marker_task` in `batch.rs`) -- it never had a path or header
  in its own scope to call the shared module with, because the one real
  existence check already happens upstream, once per tick, at the
  `batch.rs` probe corrected above. Its own inline warning construction
  (lines 175-179) is left as-is in this design: it keeps consuming the
  existing `base_exists: Option<bool>` parameter and keeps calling
  `current_machine_id()` itself. Threading a full `TemplateSourceStatus`
  down through `resolve_template_path_with_base_status`'s multiple call
  sites, purely to avoid one extra cheap, non-filesystem `current_machine_id()`
  read per task, is not worth the touched-surface-area increase across
  `spawn_ready_task`/`spawn_skip_marker_task`/`canonical_paths_tried` --
  this is a deliberate scope boundary, not an oversight. "One computation,
  not four" refers to the filesystem existence check (now genuinely
  singular, at the `batch.rs` probe); a second, cheap, side-effect-free
  `current_machine_id()` call at `path_resolution.rs`'s existing
  warning-construction site is an accepted, harmless duplication, not a
  second instance of the risk Decision 1 was scoped to eliminate.
- **`src/session/mod.rs`** (existing, modified): `SessionInfo` gains `pub
  template_source_status: Option<TemplateSourceStatus>`. `Backend` also
  gains `pub fn is_cloud(&self) -> bool` (`Backend::Local(_) => false`,
  `Backend::Cloud(b) => b.is_cloud()` or equivalent `matches!` form) --
  see the `is_cloud()` correction under `src/session/cloud.rs` below for
  why this method needs to exist on the enum, not just the concrete
  `CloudBackend` struct.
- **`src/session/local.rs`** (existing, modified): `LocalBackend::list()`
  populates the new field from the header it already holds in memory
  (calls `check_template_source_dir` once per session, same cost class as
  today's existing header read).
- **`src/session/cloud.rs`** (existing, modified): `CloudBackend::list()`
  leaves the new field `None` for remote-only placeholder rows (no header
  available, no new sync round-trip); for rows whose header has already
  been pulled locally via `sync_pull_state`, behaves like `LocalBackend`.
  Message formatting (see below) additionally calls `Backend::is_cloud()`
  to select wording.

  **Correction, twice-revised.** A Phase 6 design-review pass first
  observed that `is_cloud()` at `cloud.rs:559-561` is an inherent method
  on the concrete `CloudBackend` struct, not on the `Backend` enum that
  `handle_status`/`handle_list` actually hold, and suggested inline
  `matches!(backend, Backend::Cloud(_))` at each call site instead. A
  later plan-review pass (Category B/C, `/plan` Phase 6) caught that this
  second fix created a *new* contradiction: three independently-generated
  plan issues (status/list, `koto init` messaging) had already, correctly,
  concluded a real `Backend::is_cloud()` accessor was needed and written
  acceptance criteria to add one -- multiple call sites across two
  commands all need this same discriminant, and a one-line delegating
  method on the enum (`pub fn is_cloud(&self) -> bool { matches!(self,
  Backend::Cloud(_)) }`) is the more maintainable answer than repeating
  the `matches!` pattern at every call site. This design now settles on
  that accessor as the final answer: `Backend::is_cloud()` is added once,
  as part of this design's foundational shared-infrastructure work (see
  `src/session/mod.rs` above), and every later call site (here, `koto
  status`, `koto session list`, `koto init`'s collision messages) calls
  it rather than re-deriving the same `matches!` inline.
- **`src/cli/mod.rs`** (existing, modified): `handle_status` calls
  `check_template_source_dir` on the header it already has and adds a
  conditional `stale_template_source_dir` JSON key when `exists == false`.
  `koto init`'s collision paths (both the pre-check at line ~1682 and the
  `SpawnErrorKind::Collision` handler at line ~1707) each open the
  colliding session's header and append the same staleness clause when
  stale -- see Implicit Decision below for why both, not just one.
- **`src/cli/session.rs`** (existing, modified): `handle_list` reads
  `template_source_status` off each `SessionInfo` row (already populated by
  the backend) and includes it in the JSON output; no new I/O at this
  layer.

### Key Interfaces

```rust
// src/engine/template_source_status.rs (new)
pub struct TemplateSourceStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub machine_id: Option<String>,
}

// Core: used directly by src/cli/batch.rs (no header in scope there).
pub fn check_template_source_path(
    path: Option<&Path>,
) -> Option<TemplateSourceStatus> {
    let path = path?.to_path_buf();
    let exists = path.exists();
    Some(TemplateSourceStatus { path, exists, machine_id: current_machine_id() })
}

// Wrapper: used by the three new call sites (status, init, session
// list), which have a StateFileHeader in hand.
pub fn check_template_source_dir(
    header: &StateFileHeader,
) -> Option<TemplateSourceStatus> {
    check_template_source_path(header.template_source_dir.as_deref())
}
```

`SessionInfo` (`src/session/mod.rs`):
```rust
pub struct SessionInfo {
    // existing fields: id, created_at, template_hash, parent_workflow
    pub template_source_status: Option<TemplateSourceStatus>, // new, additive
}
```

Wire shape addition on `koto status` (only present when stale):
```json
{
  "stale_template_source_dir": {
    "path": "/home/user/repo-that-was-deleted",
    "machine_id": "host-a",
    "note": "template source directory not found (if this session was synced from another machine, this may be expected)"
  }
}
```
The `note` field's wording branches on `Backend::is_cloud()` per
Decision 2 (softened for cloud-backed sessions, direct for local ones); the `kind`
discriminator and other field names follow the existing
`stale_template_source_dir` vocabulary from `scheduler_warning.rs` for
cross-surface consistency.

### Data Flow

1. `koto init`/child-spawn writes `template_source_dir` into
   `StateFileHeader` at session creation (unchanged, existing behavior).
2. At read time, the three new surfaces (`koto status`, `koto init`'s
   collision check, `koto session list`) each load the relevant header
   through the existing shared parser (`persistence::parse_header`), then
   pass it to `check_template_source_dir` (the header-accepting wrapper).
3. Separately, the batch scheduler's per-tick probe (`batch.rs`, ~line
   875) already holds a raw `Option<&Path>`, not a header, so it calls
   `check_template_source_path` (the core function) directly once per
   tick; the resulting `Option<TemplateSourceStatus>` is converted into
   the scheduler's existing `StaleTemplateSourceDir` variant by
   `emit_template_source_dir_warnings`. The per-task resolver
   (`path_resolution.rs`) is not touched by this design -- see Solution
   Architecture's correction for why.
4. In both flows, the resulting `Option<TemplateSourceStatus>` is either
   projected directly into a JSON response field (`status`, `list`) or
   converted into the scheduler's `StaleTemplateSourceDir` variant.
5. Message wording for the three new surfaces branches on
   `Backend::is_cloud()` at formatting time -- a pure function of
   already-known data, no extra I/O.

### Implicit Decision: both `koto init` collision paths get the staleness clause

**Correction from Phase 6 review:** an earlier draft of this section
claimed the pre-check's error text (`src/cli/mod.rs:1682-1691`) and the
atomic `SpawnErrorKind::Collision` handler's error text
(`src/cli/mod.rs:1707-1716`) are byte-identical today. They are not: the
pre-check's message includes cleanup guidance ("run `koto session cleanup
{}` to reuse the name, or `koto cancel --cleanup {}` to stop a running
workflow first") that the collision handler's shorter message
(`"workflow '{}' already exists"`) lacks. The collision handler's own
comment states the intent is to keep callers able to "rely on a stable
'already exists' string regardless of which detector fired" -- that intent
holds for the shared `"workflow '{}' already exists"` prefix both messages
contain, but the full strings diverge today; this design's earlier framing
overstated the existing guarantee.

That correction doesn't change this design's conclusion, only its
reasoning: both paths should still get the staleness clause, for the
reason the original comment gives (this design should not introduce a
*new* inconsistency where the same underlying condition is diagnosable via
one detector but not the other, race-timing-dependent). Concretely: both
messages append the same staleness clause, built from the same shared
helper call, on top of whichever base message each path already emits --
this design does not need to (and does not) unify the two base messages
themselves, only ensure the new clause isn't selectively present on one. This
is recorded here as an implicit decision (no viable alternative was
seriously considered once the existing pattern was examined) rather than a
full Considered Options entry, per the design skill's guidance for
architecture-stage decisions with an obvious, low-controversy answer; no
interactive user was available to confirm it (`--auto` mode), so it's
recorded as assumed.

## Implementation Approach

### Phase 1: Shared status module

Add `src/engine/template_source_status.rs` with `TemplateSourceStatus`,
`check_template_source_path`, and `check_template_source_dir`. Also add
the thin `Backend::is_cloud()` accessor to `src/session/mod.rs` (`pub fn
is_cloud(&self) -> bool`, delegating to `CloudBackend::is_cloud()` for the
`Cloud` variant) -- grouped into this phase because it is foundational,
zero-risk shared infrastructure that Phases 4 and 5 both need, same as the
status module itself. No behavior change yet -- this phase only introduces
the type, functions, and accessor in isolation, unit-tested directly
against constructed `StateFileHeader` values covering present-and-existing,
present-and-missing, and absent `template_source_dir`, plus at least one
edge case (a dangling symlink) and one error/invalid-input case (a
`template_source_dir` that resolves to a regular file rather than a
directory) -- not just the three happy-path cases.

Deliverables:
- `src/engine/template_source_status.rs` (new)
- `Backend::is_cloud()` accessor (`src/session/mod.rs`)
- Unit tests for `check_template_source_path`/`check_template_source_dir`,
  including edge-case and error/invalid-input coverage, not just
  happy-path cases

### Phase 2: Route the scheduler's per-tick probe and warning through the shared module

`batch.rs`'s existing per-tick existence probe (`~line 875-876`, currently
`template_source_dir.as_deref().map(|p| p.exists())`) switches to calling
`check_template_source_path`, deriving the same `Option<bool>` its existing
downstream consumers (`spawn_ready_task`, `spawn_skip_marker_task`,
`canonical_paths_tried`, `resolve_template_path_with_base_status`) already
expect via `.map(|s| s.exists)` -- their signatures do not change.
`emit_template_source_dir_warnings` (`~line 1774`, called once per tick
from `~line 882`) is updated to accept the resulting
`Option<TemplateSourceStatus>` directly and build its
`StaleTemplateSourceDir` warning from it, instead of taking a bare
`base_exists: Option<bool>` and calling `current_machine_id()` itself.

`path_resolution.rs`'s per-task resolver (`resolve_template_path_with_base_status`)
is explicitly **not** touched in this phase: it only ever receives a
pre-computed `base_exists: Option<bool>` from its callers and has no path
or header of its own to check, so there is nothing here for it to route
through the shared module. It keeps its existing inline warning
construction and its own `current_machine_id()` call -- seeing this design
doesn't remove that field's now-redundant-sounding "one computation, not
four" framing; the framing is about the filesystem existence check
(genuinely singular after this phase), not about a second,
side-effect-free `current_machine_id()` read, which is an accepted,
harmless duplication.

No wire-format change. Existing tests
(`stale_base_emits_warning_with_machine_id_and_fallback` and neighbors)
must keep passing unchanged -- they are the regression guard for this
refactor.

Deliverables:
- Refactored `batch.rs` per-tick probe (calls `check_template_source_path`)
- Refactored `batch.rs::emit_template_source_dir_warnings` construction site
- Existing scheduler tests passing unmodified
- `path_resolution.rs` left unmodified (explicitly out of scope, per the
  correction above)

### Phase 3: `SessionInfo` and both `list()` backends

Add `template_source_status` to `SessionInfo`. Populate it in
`LocalBackend::list()` from the in-memory header. Leave it `None` in
`CloudBackend::list()`'s remote-only placeholder rows; populate it
normally for rows whose header has already synced locally.

Deliverables:
- `src/session/mod.rs` struct change
- `src/session/local.rs` populated field
- `src/session/cloud.rs` placeholder-row handling
- Doc comment on the new field noting the `CloudBackend` `None`-means-two-things
  limitation (no recorded dir vs. no header available)

### Phase 4: `koto status` and `koto session list` output

Wire `handle_status` to add the conditional `stale_template_source_dir`
JSON key. Wire `handle_list` to surface `template_source_status` from each
`SessionInfo` row. Both call Phase 1's `format_stale_template_source_note`,
gated on Phase 1's `Backend::is_cloud()` accessor -- this phase does not
define a new formatting helper (an earlier draft placed the helper here;
plan-review Category D found that left Phase 5's consumer hedging on
whether this phase had landed first, since Phase 5 depends only on Phase
1 -- moving the helper itself into Phase 1 alongside the accessor closes
that gap for both).

Deliverables:
- `handle_status` change (`src/cli/mod.rs`)
- `handle_list` change (`src/cli/session.rs`)

### Phase 5: `koto init` collision messaging

Update both collision paths (pre-check and `SpawnErrorKind::Collision`
handler) to open the colliding session's header, run the shared check, and
append the same staleness clause to both (wording gated on the same
`Backend::is_cloud()` accessor from Phase 1) -- see the Implicit Decision
above for why both paths need it despite their base messages not being
byte-identical today.

Deliverables:
- `src/cli/mod.rs` collision-path changes (both sites)
- Test confirming both paths produce the same staleness clause for the
  same underlying condition

## Security Considerations

This design introduces no new external inputs, dependencies, or network
surfaces -- it only reads local session state that koto itself already
wrote, and formats already-stored values (a path, a machine identifier)
into CLI/JSON output at three additional call sites.

**New I/O is bounded and same-trust-boundary.** `koto init`'s collision
path now opens the colliding session's header (previously a pure
existence check). Both the checking process and the checked session live
under the same OS user's `~/.koto/sessions/` tree, which is
`0o700`/`0o600`-permissioned (`src/session/local.rs`); this read does not
cross any privilege or user boundary that wasn't already crossable by
directly opening the raw state JSONL, which is the documented status quo
workaround this design replaces. Because the read is keyed to the single
colliding name (not an enumeration), it is also not a viable
denial-of-service amplification vector: cost is O(1) per `koto init` call,
same class as existing header reads in `koto status`/`session list`. (The
`0o700` permission on `~/.koto` is applied by `ensure_koto_root` only at
first creation, not re-enforced on every call -- this trust-boundary claim
is guaranteed for fresh installs and merely assumed, not verified, for
directories that predate this behavior or were manually re-permissioned;
not a risk this design introduces, but worth naming precisely.)

**Surfaced values are not new disclosures.** Both `template_source_dir`
(a local path) and `machine_id` (a non-secret value from `/etc/machine-id`
or the `HOSTNAME` env var, via the existing `current_machine_id()`) are
already stored in the session header or already exposed via
`SchedulerWarning::StaleTemplateSourceDir`. This design broadens *where*
those values surface (three more CLI commands); it does not broaden *who*
can see them, since koto is a single-user local tool with no cross-user or
network session-store access. The doc comment already planned for the
`SessionInfo.template_source_status` field's `CloudBackend`
None-means-two-things ambiguity (see Consequences/Mitigations) is the
right place to also note that this field surfaces a locally-recorded path
and should not be treated as safe to forward verbatim into any future
shared/multi-user or telemetry surface without re-evaluating this
single-user assumption.

**The `Path::exists()` check is read-only and gates messages, not
actions,** so no TOCTOU mitigation is required for this design's scope; if
a future Direction 3 (destructive sweep/gc) is built on top of this
signal, that design will need to revisit staleness-check robustness
(fingerprinting, not just existence) as it already anticipates.

**Stat-latency / hung-mount availability risk (added at Phase 6 review).**
`Path::exists()` is a `stat()` syscall, and `stat()` on an unreachable
mount (a hung NFS/FUSE mount, a not-yet-remounted network path) can block
indefinitely rather than fail fast -- exactly the class of torn-down
environment this feature targets (reaped sandboxes, removed worktrees).
This design multiplies exposure to that risk beyond the scheduler's
existing once-per-tick probe: `SessionInfo.template_source_status` is
computed for every session on every `LocalBackend::list()` call, and
`list()` is invoked on every dashboard refresh (`src/cli/dashboard.rs`,
`src/cli/dashboard_data.rs:290`, default 500ms poll interval). A single
session with a hung-mount `template_source_dir` could stall the
dashboard's synchronous refresh loop, not just delay one `koto` CLI
invocation -- a materially larger availability surface than "one extra
syscall per `koto init` collision" alone would suggest. This is a known,
accepted limitation for this design, not a blocker: it's the same
trade-off the batch scheduler already made when it shipped the original
`Path::exists()` probe (Decision 14 explicitly called it a "cheap probe"
assumption), now extended to more call sites. A bounded-timeout wrapper
around the existence check is a reasonable follow-up if this proves real
in practice, but is deferred rather than built speculatively here.

## Consequences

### Positive
- Fixes the concrete bug in tsukumogami/koto#189: a same-named `koto init`
  colliding with a dead session now gets a diagnosable message instead of
  a generic "already exists" error.
- `koto session list` gains passive staleness visibility with no new
  network cost, letting operators discover garbaged sessions without
  hitting the collision case at all.
- No new CLI flags, no new "orphan"-named surface -- avoids the
  `koto workflows --orphaned` and scheduler `OrphanCandidate` naming
  collisions by construction rather than by careful wording alone.
- The batch scheduler's existing, tested behavior is preserved exactly
  (same wire format, same test suite passing unmodified) while sharing its
  core logic with the new surfaces -- one computation, not four.

### Negative
- `koto init`'s collision pre-check gains I/O it doesn't have today (a
  header read on the colliding session), a real behavior change to a path
  that was previously a zero-read filesystem check.
- `CloudBackend`'s `None` on the new field is ambiguous between "no
  `template_source_dir` was ever recorded" and "no header available to
  check" -- a real limitation, not fully resolved by this design.
- Backend-aware wording only *softens* language for cloud sessions; it
  cannot actually distinguish a torn-down cloud-synced session from a
  genuinely deleted local one, because no cheap, reliable per-session
  signal for that distinction exists today.
- `Path::exists()`'s underlying `stat()` syscall can block on a hung/
  unreachable mount rather than fail fast, and this design pays that cost
  once per session on every `LocalBackend::list()` call (including the
  dashboard's ~500ms refresh poll), not just once per scheduler tick as
  today -- see Security Considerations.

### Mitigations
- The new I/O in `koto init`'s collision path is bounded (one header read,
  only on the already-slow-path collision case, not the common
  non-colliding case) and documented explicitly in this design rather than
  discovered later as a surprise.
- The `CloudBackend` ambiguity is documented via a doc comment on the new
  field at implementation time, and flagged here as a known limitation for
  a future design to resolve (e.g. distinguishing the two `None` cases with
  a tri-state enum) rather than silently accepted.
- The wording-only limitation is itself the reason Decision 2 rejected a
  stronger, certainty-claiming alternative (a new persisted
  creator-machine-id) as a separately-scoped, larger change -- this design
  explicitly does not claim more confidence than the underlying check
  supports.
