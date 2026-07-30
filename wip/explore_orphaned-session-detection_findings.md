# Exploration Findings: orphaned-session-detection

## Core Question

Koto session state already records the `template_source_dir` a session was
initialized from, but nothing reads it back to check whether that directory
still exists. What's the right shape of fix -- and what artifact should
capture it?

## Round 1

### Key Insights

- **The detection mechanism this issue asks for already exists in the
  codebase -- just wired to one narrow call site.** `SchedulerWarning::StaleTemplateSourceDir`
  (`src/engine/scheduler_warning.rs`, built by Decision 14 in
  `docs/designs/current/DESIGN-batch-child-spawning.md`) already checks
  `template_source_dir.exists()` and reports a structured warning with a
  `machine_id` and fallback path. It fires only during the batch scheduler's
  per-tick child-template-path resolution -- never on `koto init`, `koto
  status`, or `koto session list`. (leads: template-source-dir-plumbing,
  candidate-direction-fit, staleness-robustness, prior-art)

- **Two follow-on subcommands were already named and deliberately deferred
  by the team, never built:** `koto session retarget` (rewrite
  `template_source_dir` after cross-machine migration) and `koto session
  rehome <parent>` (patch a header without losing event history), both
  explicitly scoped to "v1.1 or a successor design" in
  DESIGN-batch-child-spawning.md's Non-goals section. Nothing named
  `retarget`/`rehome` exists in code today. (lead: prior-art)

- **The three candidate directions are layered, not mutually exclusive, and
  very unevenly sized.** Direction 1 (status/init messaging) needs zero new
  structs, zero new CLI verbs, and reuses header reads both call sites
  already perform -- realistically a few dozen lines. Direction 2 (`session
  list` staleness column/flag) needs a `SessionInfo` struct field addition
  (currently silently dropped after being parsed off disk) and has a real
  naming collision to resolve. Direction 3 (sweep/gc) is a genuine
  architecture decision: koto has no precedent for a whole-backend
  (root-less) destructive sweep -- the closest existing bulk-destructive verb,
  `koto workspace prune`, is root-scoped only. (lead: candidate-direction-fit)

- **"Orphaned" is already a loaded, shipped term in koto's CLI**, meaning
  "parent workflow no longer exists" (`koto workflows --orphaned`,
  documented in PRD-hierarchical-workflows.md /
  DESIGN-hierarchical-workflows.md). Reusing the same word for "template
  source directory gone" (a session can independently have neither, either,
  or both conditions) risks real operator confusion and needs a different
  name (e.g. `--stale-source`) or very careful disambiguation in output.
  (lead: candidate-direction-fit)

- **Cross-machine session resumption is a shipped, documented feature, not a
  future hazard** -- `docs/guides/cloud-sync-setup.md` + `CloudBackend` +
  `koto session resolve --keep local|remote` are real, working code. A
  session resumed via cloud sync on a different machine will legitimately
  have a `template_source_dir` that doesn't exist locally. This is the
  primary source of legitimate false positives for any orphan signal, and
  koto's own `StaleTemplateSourceDir` warning already threads a `machine_id`
  specifically to soften this ambiguity rather than assert failure. (leads:
  opt-in-posture, prior-art)

- **A plain `Path::exists()` check is adequate for read-only, human-facing
  signals (directions 1 and 2) but not safe on its own for destructive action
  (direction 3).** It can't distinguish "truly deleted" from "path reused by
  something unrelated" (false negative) or from "transiently unreachable,
  e.g. remount hiccup" (false positive). Both failure modes are informational
  noise for a status message but potential data loss for an automatic sweep.
  koto's own prior design (Decision 14) explicitly accepted this same
  limitation for its low-stakes scheduler-fallback use case and rejected
  richer fixes (a fingerprint, a retarget mechanism) as "real fixes but out
  of scope for v1." (lead: staleness-robustness)

- **Prior art across git worktree / docker / terraform converges on a
  two-layer pattern, not a single mechanism**: lazy read-time detection
  surfaced passively in normal listing output (`git worktree list`'s
  "prunable" flag, docker's dangling-volume filter), plus a separate,
  explicitly-invoked, safety-gated cleanup verb (`git worktree prune
  --dry-run --expire`, `docker volume prune` with confirmation). No strong
  analog ships only one layer and calls it done -- and the negative examples
  (systemd's half-solved stale-PID problem, VS Code's never-swept
  recent-folders list) show what happens when only one layer (or none) gets
  built. (lead: prior-art-other-tools)

- **koto has direct internal precedent for "informational default-on,
  destructive opt-in"**: `docs/designs/current/DESIGN-session-legibility.md`
  ships a read-time `Liveness` classifier that's default-on and additive
  (R12/R14: no migration, no schema break), while explicitly rejecting an
  auto-archive fork specifically because mutating state introduces
  "reversibility, races" concerns. koto's 0.10.0 changelog also shows the
  project deliberately removed automatic cleanup-on-terminal in favor of an
  explicit `koto workspace prune` verb. Both point the same direction: report
  by default, mutate only on explicit ask. (leads: opt-in-posture, prior-art)

### Tensions

- The issue's own framing ("nothing checks whether the directory still
  exists") is slightly inaccurate -- something does check, just scoped to
  the batch scheduler. A fix could go two ways: (a) generalize/extend the
  existing `StaleTemplateSourceDir` + `machine_id` mechanism to the
  init/status/list call sites, or (b) build the deferred `koto session
  retarget`/`rehome` subcommand the team already named. These aren't the
  same scope of work -- (a) is a detection/reporting fix; (b) is a repair
  mechanism. The issue as filed is asking for (a); (b) remains future work
  regardless.
- Direction 2's natural name (`--orphaned`) collides with an existing,
  differently-scoped flag (`koto workflows --orphaned`). This is a small but
  real design decision that a plan alone (no design doc) would likely miss
  or handle inconsistently across contributors.
- Whether the read-time check for direction 1 needs to be backend-aware
  (local vs cloud) wasn't fully resolved by this round -- cloud-synced
  sessions are the main legitimate false-positive source, and it's unclear
  whether the fix should suppress/soften the signal specifically when
  `session.backend == cloud`.

### Gaps

- No lead confirmed whether the *existing* `StaleTemplateSourceDir` warning
  would ever fire for the issue's actual repro path (a plain, non-batch
  `koto init`/`koto status` collision) -- it's scoped to batch scheduler
  ticks, so almost certainly no, but this wasn't independently verified by
  tracing `init_child.rs`'s collision path against a batch-parent
  precondition.
- Exact CLI shape for a future direction-3 sweep (bend `SessionCommand::Cleanup`
  vs. new `Command::Gc` vs. new `WorkspaceCommand` variant) was surfaced as
  three plausible options with no clear winner -- moot for this exploration
  since direction 3 is being deferred (see Decisions below), but would need
  resolving whenever that work is picked up.

### Decisions

- **Direction 3 (sweep/gc) is out of scope for the artifact this exploration
  produces.** Rationale: it's the only direction requiring a genuine new
  architecture decision (whole-backend destructive sweep, no existing
  precedent), it depends on directions 1+2's detection logic as a
  prerequisite, and the issue's own acceptance-criteria framing centers on
  the collision-error confusion, which directions 1+2 fully address.
  Deferred, not rejected -- worth a follow-up issue once 1+2 ship.
- **A bare `Path::exists()` check is sufficient for this fix's scope**
  (directions 1+2, both read-only/informational). Rationale: multiple leads
  confirm this matches koto's own existing precedent for the same field, and
  the failure modes a stronger check (fingerprinting) would catch matter
  only for destructive action, which is out of scope per the decision above.
- **The fix should extend/reuse `StaleTemplateSourceDir`'s existing shape and
  `machine_id` vocabulary** rather than invent a second, differently-named
  signal for the same underlying condition. Rationale: consistency across
  the codebase's JSON vocabulary; avoids a future reader wondering why two
  near-identical concepts exist.
- **Direction 2's flag/column must not be named `--orphaned`** (or must
  clearly disambiguate from `koto workflows --orphaned` in help text and
  wire format). Rationale: same word, different condition, real collision
  risk confirmed by reading the existing flag's documentation.

### User Focus

No live user was available to narrow interactively during this
background/dispatched exploration session (see scope file). Proceeding per
the auto-mode research-first protocol: the decisions above are the
recommended path, documented with rationale, to be confirmed or overridden
by a human reviewer on the resulting PR.

## Accumulated Understanding

Koto already built almost exactly the mechanism issue #189 is asking for --
`SchedulerWarning::StaleTemplateSourceDir`, from Decision 14 in
DESIGN-batch-child-spawning.md -- but scoped narrowly to the batch
scheduler's per-tick path resolution, invisible to `koto init`, `koto
status`, and `koto session list`. The fix is therefore best framed as
*extending an existing, working pattern to new call sites*, not designing a
new mechanism from scratch:

1. `koto status <name>` and `koto init`'s "already exists" collision message
   both gain an explicit check (reusing `StaleTemplateSourceDir`'s shape and
   `machine_id` vocabulary) -- small, mechanical, no new CLI surface, ships
   the actual bug fix.
2. `koto session list` gains a passive staleness signal (field on
   `SessionInfo`, threaded through both `LocalBackend` and `CloudBackend`) --
   medium-sized, needs one real naming decision (avoid colliding with the
   existing `workflows --orphaned` flag) and a decision about cloud-backend
   remote-only rows.
3. Any destructive sweep/gc/cleanup-by-staleness command is deferred as
   future work, consistent with the team's own already-deferred `koto
   session retarget`/`rehome` subcommands and koto's general precedent
   (0.10.0 changelog, DESIGN-session-legibility.md's rejected auto-archive
   fork) of keeping destructive state changes behind an explicit,
   separately-invoked verb rather than folding them into a detection fix.

This is a real "how should we build this" question -- there are genuine,
non-trivial decisions (naming collision, backend-parity handling, where the
new signal's home is: extend `scheduler_warning.rs`'s shape vs. a new
session-level warning type, whether local vs cloud backends need different
default behavior) that a bare issue or plan would likely leave for individual
implementers to resolve inconsistently. But it is not large enough, and not
novel enough, to need a brand-new standalone design doc -- the load-bearing
precedent (Decision 14, `StaleTemplateSourceDir`, `machine_id`) already lives
in `docs/designs/current/DESIGN-batch-child-spawning.md`, and the natural
home for these decisions is as a decision record or small design addendum
that extends that existing document's Decision 14, rather than a freestanding
DESIGN doc that would have to re-establish context Decision 14 already
covers.

## Decision: Crystallize
