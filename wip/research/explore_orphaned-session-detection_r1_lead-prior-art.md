# Lead: Is there existing prior art for session lifecycle/staleness/remote state?

## Findings

### Docs: DESIGN-batch-child-spawning.md is the load-bearing prior art

`docs/designs/current/DESIGN-batch-child-spawning.md`, Decision 4 (child
template path resolution) refined by Decision 14 (path resolution
contradictions), already builds almost exactly the mechanism issue #189 asks
for -- but scoped to a different call site.

Decision 4/14 established:

- `template_source_dir` on the state-file header (recorded at `koto init`
  time -- see `src/cli/init_child.rs:456-475`) and `submitter_cwd` on
  `EvidenceSubmitted` events, with resolution order absolute path ->
  `template_source_dir` -> `submitter_cwd` -> error.
- Two runtime warnings, both implemented in `src/engine/path_resolution.rs`
  and `src/engine/scheduler_warning.rs`:
  - `SchedulerWarning::MissingTemplateSourceDir` -- header predates the field
    (`None`).
  - `SchedulerWarning::StaleTemplateSourceDir { path, machine_id,
    falling_back_to }` -- header has a path, but `Path::exists()` is false.
    This is exactly the "originating directory was torn down" check issue
    #189 wants, just applied to the batch scheduler's per-tick template
    resolution rather than to `koto init`/`koto status`/`koto session list`.
- `current_machine_id()` (`path_resolution.rs:66-79`) is a best-effort
  per-host identifier (`/etc/machine-id`, falls back to `$HOSTNAME`) attached
  to the stale-warning payload specifically so agents can tell "moved to a
  different machine" apart from "same machine, path just vanished."
- Decision 14's "Known limitations" and "Alternatives considered" sections
  explicitly name two subcommands as deliberately deferred, not rejected:
  - **`koto session retarget`** -- rewrite `template_source_dir` on an
    existing header after cross-machine migration. Listed again in the
    design's own "Non-goals for v1" section (~line 3999): "the mechanism fix
    is future work."
  - **`koto session rehome <parent>`** -- patch a header in place without
    losing event-log history, scoped explicitly to "v1.1 or a successor
    design."
  Neither exists in the codebase today (`grep -rn "retarget\|rehome"` finds
  only the design-doc mentions).
- Security Considerations in the same design documents cross-machine
  portability as a known limitation (different home layouts, Linux/macOS,
  different usernames, container paths) and points at the two warnings as
  the current mitigation.

### Docs: DESIGN-config-and-cloud-sync.md -- remote/cloud state is a shipped, not hypothetical, direction

`docs/designs/current/DESIGN-config-and-cloud-sync.md` is the design that
introduced non-local session state: `CloudBackend` wraps `LocalBackend` and
syncs per-key to S3-compatible storage, with a monotonic version counter and
three-way conflict detection (`local == remote == last_sync_base` /
diverged / conflict). This is not aspirational -- `src/session/cloud.rs`
exists, `koto session resolve --keep local|remote` is implemented
(`src/cli/session.rs:525-583`), and `sync_status`/`machine_id` fields are
already threaded through the CLI response under `CloudBackend` per Decision
12 Q5 of the batch-child-spawning design. So koto already has a real
multi-machine session story: sessions can live in S3 and be resolved from
any machine, each carrying a `machine_id`.

This matters directly for issue #189: koto's own architecture already
distinguishes "gone because deleted" from "gone because a different
machine/environment" via the `machine_id` + `sync_status` channel. A fix
for orphaned `template_source_dir` detection should reuse this vocabulary
(`current_machine_id()`, `StaleTemplateSourceDir`-style warning shape)
rather than inventing a second, incompatible way to say "this path doesn't
exist here."

### Docs: DESIGN-local-session-storage.md -- machine-locality is a named, load-bearing limitation

The original local-storage design states the problem plainly: "koto
sessions are machine-local. A workflow started on one machine can't be
resumed on another without manually copying `~/.koto/sessions/`." Cloud sync
(above) is the committed answer to that gap. There's no separate
lifecycle/staleness design beyond what Decision 14 already built.

### Docs: session-schema-hygiene, session-feed-data-contract, cli-usage

- `docs/designs/current/DESIGN-session-schema-hygiene.md:212` shows the
  actual header struct field: `pub template_source_dir: Option<PathBuf>`,
  alongside `parent_workflow`, `spawn_entry`, `submitter_cwd` -- confirming
  the field is a stable, documented part of `StateFileHeader`.
- `docs/designs/current/DESIGN-session-feed-data-contract.md:384` documents
  `template_source_dir` as a consumer-facing (if ignorable) field on the
  session feed JSON: "An internal hint used by the batch scheduler's path
  resolver. Consumers may ignore this field." This framing -- "internal
  hint for the scheduler" -- would need to change if `koto status`/`koto
  session list` start reading it for orphan detection; it stops being
  scheduler-internal.
- `docs/guides/cli-usage.md` documents `koto session cleanup <name>` (manual,
  single-session) and `koto workspace prune --root <id>` (tree-wide, gated
  on terminal state, with a `--force` escape hatch and a documented cron
  cadence). Neither checks `template_source_dir`; both operate on session
  *state* (terminal or not), not on whether the originating working tree
  still exists. There's no existing `--orphaned` flag or staleness column
  anywhere in the session/workspace CLI surface.
- CHANGELOG's 0.10.0 entry documents the *removal* of auto-cleanup on
  terminal sessions (a deliberate behavior change requiring `koto workspace
  prune`) -- relevant precedent that koto has form for deliberately shifting
  cleanup responsibility to an explicit opt-in verb rather than silent
  automatic sweeps, which bears on the "should orphan-flagging be a default
  safety check or a flag" open question.

### CHANGELOG / git log

No entry in `CHANGELOG.md` mentions cloud sync shipping as a headline
feature (the config/cloud-sync design's phases may be partially landed
without a dedicated changelog callout, since `sync_status`/`machine_id`/
`CloudBackend`/`session resolve` all exist in code). Git log shows no prior
commit or PR title referencing "orphan" other than the just-created scope
commit (`d389f14`) and one unrelated hit (hierarchical workflows, PR #128,
matched only on "orphan" as a substring elsewhere). No "stale" commits are
relevant beyond the batch-orchestration PR (#136) which is part of the
Decision 14 lineage above.

### GitHub issues

- **#189** (open, `enhancement`) is this exploration's own issue -- confirmed
  no duplicate.
- No other open or closed issue matches "orphan," "stale," or "session" in a
  way that overlaps this problem. The closest historically-related closed
  issues are #132 (`koto cancel` leaves workflow in DB; cleanup required as
  a separate step) and #134 (children-complete gate not observing cleaned-up
  sessions) -- both about cleanup/lifecycle *sequencing* bugs, not about
  detecting a torn-down originating directory. Neither is a duplicate or
  blocker.
- #185 (closed) touched "old sessions leave unreadable state files" --
  adjacent (schema/migration robustness) but not the same failure mode.

## Implications

The exploration should not treat this as greenfield. Decision 14 in
DESIGN-batch-child-spawning.md is the direct precedent: it already solved
"does `template_source_dir` still exist" for one call site (the batch
scheduler's per-tick template resolution) and explicitly deferred the
general-purpose fix -- a subcommand to inspect/repair a stale header -- as
future work under the names `koto session retarget` / `koto session
rehome`. A design for #189 should either:

1. Extend/reuse Decision 14's existing `StaleTemplateSourceDir` +
   `current_machine_id()` mechanism and apply it at `koto init`
   (already-exists error path) and `koto session list`/`koto status`, rather
   than inventing a new warning shape, or
2. If a repair/rewrite verb is in scope, implement the deferred
   `koto session retarget`/`rehome` concept Decision 14 already named,
   rather than picking a fresh command name.

Either way, the fix should be framed as "closing the gap Decision 14 left
open" and should reference DESIGN-batch-child-spawning.md (Decisions 4 and
14) as upstream, plus acknowledge DESIGN-config-and-cloud-sync.md's
`machine_id`/`sync_status` vocabulary so a local-orphan detector and a
cross-machine cloud-sync detector don't end up saying "this path is gone"
in two incompatible ways. The existing `docs/designs/current/` directory is
the right place to extend rather than write a brand-new standalone design
from scratch -- this is very plausibly a small addendum/decision inside (or
directly following on from) DESIGN-batch-child-spawning.md rather than a new
DESIGN doc.

The "opt-in vs default-on" open question (research lead #4) has a relevant
precedent too: koto already removed automatic cleanup-on-terminal in 0.10.0
in favor of an explicit `koto workspace prune` verb specifically to avoid
silent destructive automatic behavior. That argues for orphan detection
being surfaced as information (status flag, list column) by default, with
any destructive sweep (removal) staying opt-in/explicit, mirroring the
prune-vs-auto-cleanup precedent.

## Surprises

- The mechanism issue #189 asks for essentially already exists in the
  codebase, just wired to the wrong (or rather, a narrower) call site. This
  wasn't obvious from the issue text alone, which frames the problem as if
  nothing reads `template_source_dir` back at all -- true for `init`/
  `status`/`session list`, false for the batch scheduler.
- Remote/non-local session state is not a "someday" direction to avoid
  conflicting with -- it's already implemented (`CloudBackend`, `koto
  session resolve`, S3 sync). The exploration's own scope note ("Remote/
  non-local session state... only considered insofar as the chosen fix
  shouldn't foreclose it") undersells how far this has already gone; it's
  not a future hazard, it's present-tense architecture with its own
  established `machine_id` vocabulary that a good fix should slot into.
- Two future subcommands (`koto session retarget`, `koto session rehome`)
  were already named and scoped by the team as of the batch-child-spawning
  design, and neither has been built. This is a strong signal that whoever
  designs #189's fix should check whether it *is* one of these two
  subcommands rather than a third new concept.

## Open Questions

- Is `koto session retarget`/`rehome` still the intended future shape, or
  has thinking moved on since DESIGN-batch-child-spawning.md was written?
  (No newer design supersedes it as of this search.)
- Does the batch scheduler's existing `StaleTemplateSourceDir` warning get
  triggered in practice for the exact repro in the issue (ephemeral sandbox
  reap / `git worktree remove` / container teardown), or only for batch
  parents with `materialize_children` hooks? If the repro session in
  dangazineu/commuter#49 wasn't a batch parent, the existing mechanism never
  fires for it at all -- worth confirming with a second research lead
  reading `src/cli/init_child.rs` and the `koto init`/`koto status`
  collision-check code path directly.
- Whether `docs/designs/current/DESIGN-session-feed-data-contract.md`'s
  "consumers may ignore this field" language for `template_source_dir` would
  need a companion update once it stops being scheduler-internal.

## Summary

koto already built almost exactly this mechanism once: DESIGN-batch-child-spawning.md's Decision 14 added a `StaleTemplateSourceDir` warning with a best-effort `machine_id`, triggered when a recorded `template_source_dir` no longer exists on disk, and explicitly deferred a general-purpose `koto session retarget`/`rehome` subcommand as future work that was never built. This means a fix for #189 should extend or reuse that existing warning/machine-id vocabulary and check whether it's simply implementing the previously-named `retarget`/`rehome` subcommand, rather than treating the problem as new; it should also account for koto's already-shipped `CloudBackend`/`sync_status` remote-state model so local-orphan and cross-machine-stale don't get reported two different ways. The main open question left for a second lead is whether the existing `StaleTemplateSourceDir` check (which only runs in the batch scheduler's per-tick path resolution) ever fires for the issue's actual repro path (`koto init`/`koto status` collision detection), since that determines whether this is a "wire an existing check into a new call site" fix or a "build the check for the first time at this call site" fix.
