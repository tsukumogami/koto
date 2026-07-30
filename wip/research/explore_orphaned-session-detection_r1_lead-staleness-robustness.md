# Lead: Is a path-existence check sufficient, or does staleness need to handle remount/rename?

## Findings

### How `template_source_dir` is actually recorded

`src/cli/init_child.rs:456-467` (`init_child_core`) is the only write site:

```rust
let template_source_dir = if template_path.is_absolute() {
    template_path.parent().map(|p| p.to_path_buf())
} else {
    std::fs::canonicalize(template_path)
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
};
```

Two asymmetric code paths:

- **Absolute `--template` path**: only `.parent()` is taken. No symlink resolution.
  If the path passes through a symlink (e.g. `/home/user/repo/tpl.md` where `repo`
  is a symlink to a worktree), the *symlink path* is recorded, not the resolved
  target.
- **Relative `--template` path**: `std::fs::canonicalize()` runs first, which does
  resolve symlinks (on Linux, via `realpath`-equivalent). So relative-path callers
  get a symlink-free absolute path; absolute-path callers don't.

This is stored as `Option<PathBuf>` on `StateFileHeader` (`src/engine/types.rs:258-260`),
serialized as a plain absolute-path string, `serde(default, skip_serializing_if =
"Option::is_none")` for backward compat with pre-feature state files.

### Nothing reads it back for orphan detection today, but a sibling consumer already reads it for a different purpose

`src/engine/path_resolution.rs` is the one existing reader of `template_source_dir`,
used by the batch scheduler (Decision 4/14, `docs/designs/current/DESIGN-batch-child-spawning.md`)
to resolve relative child-template paths. It already implements exactly the naive
check the lead asks about:

```rust
let base_status = template_source_dir.map(|p| p.exists());
```

`Path::exists() == false` triggers `SchedulerWarning::StaleTemplateSourceDir`, then
falls through to `submitter_cwd`. The design doc explicitly names this a "cheap
probe" assumption and documents known limitations it accepted rather than solved:
state files with no `template_source_dir` (pre-feature) loop a warning forever
until a future `koto session rehome`/`retarget` subcommand exists; absolute
child-template paths bypass the staleness check entirely; cross-machine or
cross-home-layout drift is treated as an accepted, warned-about limitation, not
something requiring a fingerprint. The design's "Alternatives considered" section
rejected a repo-relative parallel field and a session-retarget mechanism as
"real fixes but out of scope for v1."

This precedent matters because it shows the project already deliberately chose
existence-check-only for `template_source_dir` staleness — but for a **low-stakes**
consumer: worst case is a wrong warning plus an automatic, reversible fallback to
`submitter_cwd`. Nothing is deleted or reported as ground truth to a human deciding
whether to clean up state.

### Failure mode walkthrough

**(a) Directory truly deleted.** `Path::exists()` returns `false`. Correctly
classified. This is the case the naive check was built for and handles well.

**(b) Path reused by something unrelated (e.g. a reaped niwa ephemeral instance's
worktree slot gets reprovisioned at the same path for a different repo).**
`Path::exists()` returns `true` — a false negative for staleness. The session
looks "still live" even though the directory now contains unrelated content. A
plain existence check cannot distinguish this from genuinely-still-live; the check
only answers "is *a* directory there," not "is it *the same* directory." Given
this workspace's own architecture (deterministic-ish worktree paths under
`.claude/worktrees/<name>`, ephemeral niwa instances reclaimed and re-provisioned),
this isn't a hypothetical — it's the same shape of collision the original issue
describes, just recurring one layer deeper (the tool would now confidently assert
"not stale" while being wrong). Fixing this needs something beyond the path:
an inode+device fingerprint captured at init time and compared at read time
(cheap, no writes to the user's directory, but not bulletproof against inode
reuse), or a git commit/remote fingerprint when the source happens to be a git
worktree (not general — koto's template source directory is not guaranteed to be
a git repo).

**(c) Directory temporarily unavailable (remount / network mount hiccup).**
`Path::exists()` returns `false` for *any* unsuccessful stat, not just ENOENT —
permission errors, stale NFS handles, and "not yet mounted" all collapse to the
same `false`. A plain existence check cannot tell "confirmed gone" from
"transiently unreachable." This is a false positive for staleness. It matters
most for the risky candidate direction (an automatic `koto gc`/`cleanup
--orphaned` sweep that deletes or mutates state): a single failed stat during a
remount window would misclassify a live session as orphaned. It matters much
less for a read-time, human-facing check (`koto status`, an init collision
message) since a human sees the report and can re-run or investigate before
acting.

**(d) Directory renamed/moved (recorded path now dangling, but the "real" tree
still exists elsewhere).** `Path::exists()` at the old recorded path returns
`false` — same bucket as (a) truly-deleted. The check is accurate about the
recorded path but cannot distinguish "gone forever" from "moved" without
searching for the moved destination, which no existing koto mechanism attempts
(no reverse index from content/name to new location). Practically this means an
existence-check-based orphan flag is honest only if it's phrased as "the
recorded path no longer resolves," not "the source was deleted" — the two are
not the same claim, and any UI copy or `--orphaned` sweep needs to avoid
asserting deletion as the cause.

### Symlink wrinkle grounded in the write-site asymmetry

Because absolute `--template` paths skip canonicalization, a session initialized
through a symlinked path is vulnerable to a variant of failure mode (b): the
symlink itself keeps existing (`Path::exists()` on the recorded path follows
whatever the symlink currently points to), but if the symlink's target is later
swapped, the "directory" still reads as present while pointing at different
content. This is the same underlying hazard as directory-path reuse, just
reached through symlink retargeting instead of delete+recreate, and it's asymmetric
with the relative-path case (which is canonicalized and therefore symlink-stable
at record time, though still vulnerable to the target itself moving later).

## Implications

The right answer depends on which of the three candidate directions consumes the
check, not on a single yes/no for the whole feature:

- **Direction 1 (`koto status <name>` / init "already exists" check explains
  staleness)** and **direction 2 (`session list` staleness column)** are read-time,
  informational, human-in-the-loop. A plain `Path::exists()` check is a
  legitimate v1 fix here, consistent with the precedent already set by
  `path_resolution.rs` for the same field. False negatives (b) leave today's
  status quo unchanged (tool already always says "looks live"); false positives
  (c) just prompt a human to double-check, no harm done.
- **Direction 3 (`koto session cleanup --orphaned` / `koto gc` automatic sweep)**
  is where a bare existence check is materially riskier: it can lead to deleting
  or reporting-as-safe-to-delete state for a session whose directory is only
  transiently unavailable (c), or asserting "orphaned" for a merely-renamed
  directory (d) whose owner would be surprised to find their session state gone.
  If direction 3 is pursued, it needs one or more of: a retry/backoff before
  declaring orphaned (to filter out (c)), a confirmation prompt or dry-run mode
  by default (mitigates both (c) and (d) by keeping a human in the loop before
  anything destructive happens), and — as a v1.1+ enhancement, not a blocker —
  an inode/device fingerprint recorded alongside the path to catch (b).
- Any UI/report copy should say "the recorded source directory no longer
  resolves" rather than "was deleted" — the exploration should recommend
  phrasing that stays honest about what an existence check can and cannot prove,
  given (b) and (d) above.

This also means the "3 candidate directions" from the issue are not equally
cheap: 1 and 2 are safe to ship with a bare existence check now; 3 needs either
a narrower scope (report-only, no auto-delete) or the added machinery above
before it's safe to make destructive.

## Surprises

The most useful discovery wasn't abstract reasoning about staleness — it's that
koto already has a fully-built, designed, and documented answer to almost this
exact question for the *same field*, just wired to a different consumer
(`src/engine/path_resolution.rs`, Decision 4/14 in
`docs/designs/current/DESIGN-batch-child-spawning.md`). That prior design
explicitly weighed "richer" fixes (repo-relative field, a `retarget`/`rehome`
subcommand) and rejected them for v1 as "real fixes but out of scope," while
accepting `Path::exists()` as good enough for its (reversible, warning-only) use
case. The orphaned-session-detection exploration can lean on that precedent for
directions 1/2, but should not silently inherit it for direction 3, since the
blast radius of being wrong is categorically different (informational message vs.
data loss).

The absolute-vs-relative canonicalization asymmetry in `init_child.rs` was not
something the lead anticipated but is a concrete, code-confirmed edge case worth
carrying into whatever design doc follows.

## Open Questions

- Does any candidate direction actually intend to delete session state
  automatically, or would "orphaned" always mean "flagged for human review"? This
  changes how much staleness-robustness machinery is actually required for v1.
- Is an inode+device fingerprint (platform-conditional, Unix-only via
  `std::os::unix::fs::MetadataExt`) an acceptable v1-adjacent addition, or does
  it need its own design decision given koto's stated aim of eventual
  cross-machine/non-local session state (which would make local-inode fingerprints
  meaningless)?
- Should the fix distinguish `ENOENT` from other stat failure kinds (permission
  denied, stale NFS handle) explicitly, rather than collapsing everything into
  `Path::exists() == false`, given how cheaply `std::fs::metadata()` plus error-kind
  inspection could do this?

## Summary

`template_source_dir` is recorded once, at `koto init`, as an absolute path
(canonicalized only when the original `--template` argument was relative — 
absolute-path inputs keep any symlink unresolved), and today's only reader
(`src/engine/path_resolution.rs`, built for the unrelated batch-scheduler
path-resolution feature) already treats a bare `Path::exists()` as sufficient,
explicitly accepting cross-machine/rename drift as a documented limitation
rather than solving it. That precedent is safe to reuse for read-time,
human-facing reporting (directions 1 and 2), but not safely sufficient on its
own for an automatic cleanup/gc sweep (direction 3), where directory reuse,
transient remounts, and renames can misclassify a live session as orphaned; the
biggest open question is whether any candidate direction actually intends
destructive action, since that's what determines whether the naive check needs
reinforcement at all.
