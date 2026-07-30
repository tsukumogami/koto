# Lead: Should orphan-flagging be opt-in, and are there legitimate long-lived-session workflows?

## Findings

### Cross-machine sessions are a real, documented, shipped workflow

`docs/guides/cloud-sync-setup.md` documents an S3-compatible cloud session
backend whose entire purpose is "resume workflows on a different machine."
The guide shows the exact scenario this lead worries about:

```
# On machine A
koto init my-workflow --template review.md
...
# On machine B (same config + credentials)
koto next my-workflow  # downloads session from cloud, picks up where A left off
```

`README.md:169` lists this as "Cloud sync setup -- configure S3-compatible
cloud storage for cross-machine sessions." This is not aspirational or a
future direction description in a design doc -- it's a user-facing guide with
working config commands (`koto config set session.backend cloud`, etc.) and a
conflict-resolution flow (`koto session resolve <name> --keep local|remote`).

So: yes, koto has a first-class notion of a session intentionally continuing
after its originating directory is gone from the current machine, because it
never existed on this machine to begin with. `template_source_dir` recorded
on machine A is expected to be absent on machine B under this workflow.

### The codebase already has a non-fatal, informational answer to this exact case

`src/engine/scheduler_warning.rs` and `src/engine/path_resolution.rs`
(existing, shipped code, not this exploration's proposal) already define
`SchedulerWarning::StaleTemplateSourceDir` -- emitted precisely when
`template_source_dir` is recorded but doesn't exist on the current machine.
The doc comment is explicit: "typically following a cross-machine session
migration." The warning carries the stale path, a best-effort `machine_id`,
and the directory the scheduler fell back to (`submitter_cwd`). It is
non-fatal: resolution falls through and continues. There's even a forward
pointer (`path_resolution.rs:62-65`) noting a future revision may align this
`machine_id` with "the same identifier the cloud-sync layer attaches to state
files (Decision 12 Q5)."

This means: the codebase has already converged, in a shipped feature, on the
exact posture this lead is asking about for orphan detection -- treat a
missing/stale recorded directory as informational, not an error, include
diagnostic context (path, machine, fallback), and let the operation proceed.
Orphan-flagging for `template_source_dir` would be a second consumer of the
same signal (directory existence), and precedent strongly favors matching
this shape: report, don't block.

### Direct precedent for default-on, read-only, additive classification: session legibility

`docs/designs/current/DESIGN-session-legibility.md` (status: Current) is the
closest architectural precedent in the repo for "flag it and let humans
decide." It adds a `Liveness` classifier (`Active`, `Idle`,
`NeedsYouStalled`, `NeedsYouBlocked`, `NeedsYouFailed`, `Pending`, `Done`) to
the dashboard, computed **at read time** from data already on disk, with
explicit requirements:

- **R12 -- no migration**: classification must apply to sessions already on
  disk with zero schema change.
- **R14 -- additive/compatible**: new data written must be additive; no
  reader of the existing format breaks.

Critically, this classifier is **default-on** and requires no flag --
`NeedsYouStalled` (a session that went silent) is surfaced in the default
dashboard view precisely because it's informational and blocked-before-idle
protects against false "this looks dead" alarms on legitimately parked
sessions (R4: "Blocked always wins over idle... a developer might kill good
work" if misclassified). The one thing that *is* gated behind an opt-in
reveal (`--all` / a keypress) is **receding terminal/abandoned sessions from
the default view** -- and even that's a display filter, not a mutation; nothing
is deleted or archived. Fork B2 (auto-archive: physically moving session
directories) was explicitly considered and rejected/deferred specifically
because it is "a write/migration with its own correctness surface
(reversibility, races)."

`PRD-session-legibility.md` explicitly carves cross-machine visibility out of
this feature's scope: "Cross-machine / multi-host visibility (`host` /
`owner`, ... the aggregated cross-machine view, remote storage) -- owned by
the separate S3-backed dashboard work" -- confirming cross-machine session
handling is a recognized, separately-owned problem area, not an edge case
nobody has thought about.

### Existing cleanup command precedent: explicit, single-target, user-invoked -- not a sweep

`koto session cleanup <name>` already exists (`src/cli/session.rs:511`,
`handle_cleanup`) and is used today, e.g. in error messages guiding a user
whose `koto init` collided with an existing session name ("session '{}'
already exists; run `koto session cleanup {}` first"). Notably:

- It requires an explicit session name -- there is no bulk/wildcard/`--all`
  cleanup today.
- There is no `koto gc` or `--orphaned` sweep flag anywhere in the CLI
  (`src/cli/mod.rs`, `src/cli/session.rs`) -- confirmed by grep, nothing
  matches.
- `koto cancel --cleanup` is the other cleanup-adjacent surface, again
  scoped to one named session, and it stops a *running* workflow (different
  problem: live-but-unwanted, not orphaned-by-directory-loss).

So the only existing "delete/remove state" precedent in koto is opt-in by
construction (you must name the target) and never automatic. There is no
prior art in this codebase for an unattended sweep that deletes sessions
based on inferred staleness.

### No evidence of a "detach and reattach" concept beyond cloud sync

Searched for "detach" across docs/src: the only hit
(`docs/designs/current/DESIGN-koto-agent-integration.md`) is about git's
"detached HEAD" state in an unrelated context (submodule friction), not
session detach/reattach. There's no koto-specific "detach this session from
its working directory" primitive -- the *implicit* mechanism for
directory-independence is the cloud backend (session state lives in S3, not
tied to any one machine's filesystem) plus `submitter_cwd`/
`template_source_dir` fallback in path resolution. Portability is achieved by
storage location, not by an explicit detach verb.

## Implications

- **Read-only orphan flagging should be default-on**, following the Liveness
  precedent: it's a pure function over data already recorded
  (`template_source_dir` + `Path::exists()`), requires no migration, breaks
  no existing reader, and the failure mode of *not* flagging (silent
  same-name collision producing a generic "already exists" error, per the
  original issue repro) is worse than the failure mode of an occasional false
  positive on a legitimately relocated/cloud-synced session -- especially
  since a false positive here is informational, not destructive.
- **The false-positive risk from cloud-synced sessions is real but bounded
  and matches an already-solved case.** A session synced via `session.backend
  = cloud` and resumed on machine B will have a `template_source_dir` from
  machine A that doesn't exist locally. The fix should reuse the exact
  distinction `StaleTemplateSourceDir` already draws: "recorded directory is
  gone" is not inherently bad, it's either (a) a genuine orphan (originating
  sandbox/worktree was torn down, nobody will ever resume it there) or (b) a
  cross-machine resume in progress. The existing warning's own doc comments
  already acknowledge this ambiguity by attaching `machine_id` rather than
  asserting failure. Any orphan flag on `koto status`/`koto session list`
  should be worded as advisory ("template_source_dir not found on this
  machine") rather than asserting the session is dead -- exactly how
  `StaleTemplateSourceDir` is worded, and exactly how `NeedsYouStalled` is
  worded (surfaced, not alarmed).
- **Any destructive action (delete/cleanup a sweep of "orphaned" sessions)
  should require explicit opt-in**, consistent with both existing precedent
  (cleanup is always single-target and explicit today; auto-archive was
  explicitly rejected/deferred in the legibility design specifically because
  mutating state introduces "reversibility, races" concerns) and with the
  cloud-sync false-positive risk (auto-deleting a session whose directory is
  merely missing *locally* could destroy a session mid cross-machine resume).
  A `koto session cleanup --orphaned` or `koto gc` sweep, if built, should
  default to dry-run/report mode and require an explicit confirmation flag
  (e.g. `--yes` or a second explicit flag) to actually delete -- it should not
  reuse the bar of the informational flag.
- This argues for the issue's option 1 and 2 (status/init-error check;
  `session list` staleness column or `--orphaned` filter) being safe as
  default-on, informational features, while option 3 (a cleanup/gc sweep)
  is the piece that needs the opt-in/confirmation posture -- these are not a
  single decision, they're two different postures for two different classes
  of feature within the same issue.

## Surprises

- The codebase already ships a mechanism (`StaleTemplateSourceDir`) that
  solves a closely adjacent problem (stale `template_source_dir` after
  cross-machine migration) using exactly the read-only/informational/
  fallback pattern this lead was asked to evaluate as hypothetical. This
  isn't a green field decision -- there's an established, working pattern to
  extend rather than invent.
- `koto session cleanup <name>` already exists and is already referenced in
  user-facing error messages as the answer to "session already exists" --
  meaning part of the friction described in the exploration's core question
  (generic "already exists" error) already has a documented manual escape
  hatch; the gap is discoverability/diagnosis (telling the user *why* it
  collided), not the remediation command itself.
- Cross-machine session portability is a shipped, documented feature (not a
  future roadmap item) -- this changes the risk calculus more than a "maybe
  someday" direction would.

## Open Questions

- Does the cloud-sync backend's session header record anything (a
  `sync_status`, a "last synced from machine X" marker) that a
  `template_source_dir`-based orphan check could cross-reference to
  suppress false positives automatically, rather than relying purely on
  wording the flag as advisory? `path_resolution.rs`'s TODO about aligning
  `machine_id` with "the cloud-sync layer" (Decision 12 Q5) suggests this
  linkage doesn't fully exist yet -- worth checking
  `DESIGN-config-and-cloud-sync.md` in more depth for the header schema.
- Should the orphan flag distinguish "backend is `local`" (where
  `template_source_dir` gone almost certainly means a torn-down sandbox) from
  "backend is `cloud`" (where it's ambiguous/expected)? This wasn't fully
  explored here and could simplify the false-positive story considerably --
  possibly the flag is only meaningful/default-on for local-backend sessions.
- What does `koto session list`'s actual current command surface look like
  (flags, columns)? This investigation found the `--once` dashboard path and
  `handle_cleanup`, but didn't locate a dedicated `session list` subcommand
  implementation to confirm exactly where a staleness column would plug in.

## Summary
Koto already ships a documented cross-machine "resume elsewhere" workflow (S3 cloud sync) and already has a shipped, non-fatal mechanism (`StaleTemplateSourceDir`) for exactly this ambiguity, so orphan-flagging should follow that same read-only, informational, default-on pattern rather than inventing new opt-in gating. Any actual cleanup/sweep/delete action, by contrast, should require explicit opt-in/confirmation, matching the existing `koto session cleanup <name>` precedent (always single-target, never automatic) and the legibility design's explicit rejection of auto-archive due to reversibility/race concerns. The biggest open question is whether the orphan check should be scoped or worded differently depending on `session.backend` (local vs cloud), since cloud-synced sessions are the main source of legitimate false positives and that linkage isn't fully wired up yet.
