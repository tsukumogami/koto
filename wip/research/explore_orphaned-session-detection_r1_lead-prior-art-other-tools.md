# Lead: What do comparable tools do to detect/report stale local state tied to a removed directory?

## Findings

### git worktree prune (closest analog)

Git doesn't track "does the directory exist" directly — it tracks a `.git` file inside the worktree's directory that links back to the main repo's `.git/worktrees/<name>` administrative entry. A worktree is considered prunable when that linked working directory (or the `.git` file inside it) is gone. This is exactly koto's situation: a directory-shaped pointer that can vanish out from under the tool without the tool being told.

Detection is lazy, not proactive: git does not run a background scan. The check happens (a) implicitly, whenever another worktree operation touches the admin metadata (e.g. `git worktree add` of a name that collides with a prunable entry auto-prunes it first in modern git), and (b) explicitly via `git worktree prune`, which walks `.git/worktrees/*` and tests each for a live working directory.

Reporting has three tiers of increasing intent:
- `git worktree list` marks a stale entry inline with a `prunable` annotation next to the path (no separate command needed to *see* it — visibility is free, cleanup is not).
- `git worktree prune --dry-run` reports exactly what would be removed, with no side effects — this is the safety rail.
- `git worktree prune` (no flags) actually deletes the admin data for anything currently prunable, immediately, no confirmation prompt. Safety instead comes from `--expire <time>` (e.g. `--expire 30.days.ago`), which limits pruning to entries stale for a minimum age, guarding against a worktree that's mid-creation or on a slow/disconnected filesystem being wrongly swept.

Critically, prune is opt-in-to-run but not opt-in-to-see: you always find out about staleness for free in `list` output, but nothing is deleted until you explicitly ask, and even then age-gating is available. There's also a real gotcha reported in the community (musteresel's blog, 2018): if the directory is deleted manually rather than via `git worktree remove`, the remaining `.git/worktrees/<name>` entry is invisible in normal workflows until someone runs `list` or `prune` — it just silently persists. That is effectively the exact bug koto has today.

### Docker (dangling volumes/images)

Docker's model is reference-counting rather than path-existence: a volume is "dangling" when no container (running or stopped) references it, discoverable via `docker volume ls --filter dangling=true`. This is a read-time filter on `list`, not a background sweep — the daemon doesn't proactively flag anything until you ask. Cleanup is a separate, explicitly invoked command (`docker volume prune`, `docker system prune --volumes`), and by default it's scoped conservatively (since 22.06, `volume prune` only removes anonymous volumes; named volumes need `--all`). No dry-run flag exists on prune itself, but `--filter` lets you preview via `ls` first, and prune interactively confirms ("WARNING! This will remove all volumes not used by at least one container... Are you sure?") unless `-f` is passed. So Docker's shape is: read-time flag in list output + explicit sweep command + confirmation-gated by default, force-bypassable.

### terraform (state referencing destroyed resources)

Terraform's staleness check is deliberately *not* automatic on every command — `terraform plan` triggers a refresh (pre-0.15.4 always, and still by default unless `-refresh=false`) that calls out to the provider API and detects if a previously-tracked resource no longer exists remotely. It reports this as a diff line in the plan output (resource marked for recreation, or with `-refresh-only` mode, a state-only change to reconcile). There's no separate "gc" for this — reconciliation happens as a side effect of the normal read/plan path, which is a lazy/read-time model. The dangerous edge case documented in community writeups: `terraform refresh` (or the refresh step of `plan`) will silently drop a resource from state if it's gone, which can surprise people who wanted to *recreate* it rather than forget it — so Terraform's design leans toward "surface it in the diff for a human decision" rather than "auto-heal silently," though the refresh-only variant exists specifically so you can apply *just* the reconciliation without other changes.

### systemd (stale PID files)

This is the weakest/most cautionary analog, but instructive: classic init-script PID files are just a number on disk, and systemd's fix wasn't to teach the reaper to be smarter about detecting staleness — it made the whole problem obsolete for units that adopt `Type=notify`/cgroup-based tracking, or `RuntimeDirectory=` (auto-created/cleaned `/run` subdirectory tied to unit lifecycle so there's no orphan to detect in the first place). Where stale PID files still matter (legacy `Type=forking` units), the community pattern is validate-on-read: check whether the PID in the file corresponds to a live process before trusting it, and treat a dead PID as an implicit signal the file is stale — no separate sweep command exists; the check is inlined into the next start/restart. The systemd bug tracker explicitly floated "could automatically detect and clean up a stale pid file" as a *feature request* that was never fully generalized — a sign this class of problem is easy to under-invest in and leave half-solved.

### VS Code (workspace / recent-folder list) — negative example

Worth including because it shows what happens when nobody solves this: VS Code's "recently opened" list, the git extension's `closedRepositories` state, and worktree entries in the GitHub PR sidebar all accumulate stale entries pointing at deleted folders, and multiple long-open GitHub issues (microsoft/vscode #319230, #313551, #62411, #166380; microsoft/vscode-pull-request-github #8525) confirm there is no built-in reconciliation — entries persist until a full window reload or a third-party cleanup script. This is the "do nothing" end of the spectrum and it visibly frustrates users (community scripts exist specifically to patch the gap).

### direnv (adjacent, not a strong match)

direnv's core staleness concept is about `.envrc` *content* changing (hash mismatch triggers "blocked" state, requiring `direnv allow` again), not about the directory disappearing — so it's a weaker analog than initially hoped. It does have a `direnv prune` command for its allow-list cache, but per the docs it only works forward from newer versions since older cache entries don't store enough to check against; it's a maintenance sweep, not a lazily-triggered check.

### tmux (weak/inconclusive)

Sessions in tmux are decoupled from filesystem existence of their cwd almost entirely — a pane's shell keeps running with a now-deleted cwd (typical POSIX behavior: the process holds the inode open), and tmux itself has no concept of "flag this session because its start directory vanished." This is really a non-solution / out-of-scope precedent: tmux simply doesn't attempt this class of detection at all, which is itself a data point (some tools punt entirely and let the OS-level shell weirdness be the only symptom).

## Implications

The pattern that recurs across the strongest analogs (git worktree, docker volumes, terraform) is a two-layer design, not a single mechanism:

1. **Read-time, lazy detection surfaced in normal listing output.** Nobody runs a background daemon that watches the filesystem for deletions. Instead, the *next* time a human or the tool itself looks at the state (`git worktree list`, `docker volume ls --filter dangling=true`, `terraform plan`'s refresh), the check runs then and is reported inline — a flag/column/diff-line, not a separate alert.
2. **A distinct, explicitly-invoked cleanup action, gated by a safety rail.** Detection and remediation are different verbs. `git worktree prune` (with `--dry-run` and `--expire`), `docker volume prune` (confirmation prompt, conservative default scope), and `terraform state rm`/`-refresh-only` (diff surfaced before any destructive action) all separate "tell me what's stale" from "actually get rid of it," and default toward showing before deleting.

Mapped onto koto's three candidate directions (read-time check / `list --orphaned` flag / explicit sweep-gc command), the converged answer across every strong analog is **combine the first two, and layer the third on top** — not one or the other:
- The read-time check has to exist regardless, because it's what prevents the actual bug in the issue: at *collision* time (a same-named re-init hitting a session whose dir is gone), koto needs to check liveness right then and give a specific message ("this session's original directory no longer exists, safe to reclaim") instead of a generic "already exists" error. This is non-optional and cheapest to ship first — it directly fixes the reported symptom.
- `list` should surface staleness passively (a flag/column, à la `git worktree list`'s "prunable" annotation or docker's dangling filter) so a user can discover garbaged sessions without hitting the collision case at all.
- An explicit sweep/gc command is the right place for *destructive* cleanup, and every strong analog gates it either with a dry-run, an age-based expiry, or a confirmation prompt (or more than one). Given koto's session directories can be ephemeral sandboxes or worktrees that get torn down deliberately, an `--expire`-style age gate (matching git's) is probably more apt than a confirmation prompt, since these deletions are batch/scripted more often than interactive.

The read-time check alone (no sweep) leaves stale entries accumulating forever with no way to reclaim disk/state short of hitting the exact collision case — that's the VS Code failure mode. A sweep command alone, with no read-time check, leaves the originally-reported bug (collision error) unfixed until someone thinks to run gc. Both are needed; they solve different halves of the problem.

## Surprises

- The systemd and VS Code angles turned out to be the most useful negative examples rather than positive prior art — they illustrate the two failure modes to avoid (half-generalized point-fix, or no fix at all with community scripts filling the gap) rather than a mechanism to copy.
- direnv and tmux, despite being suggested leads, don't actually solve "directory disappeared out from under tracked state" in any deliberate way — direnv's staleness model is about content hashes, and tmux doesn't attempt directory-liveness tracking at all. Worth noting so the exploration doesn't over-index on them.
- Git's own gotcha (musteresel's blog) is a near-exact restatement of koto's bug: manually deleting a worktree directory (bypassing `git worktree remove`) leaves an invisible stale admin entry that only `list` or `prune` reveals — this is strong direct precedent, not just an analogy.
- Terraform's refresh-and-diff model (rather than a separate gc verb) suggests a third pattern worth naming explicitly: staleness-as-diff, where the "is this dir gone" check is folded into whatever operation already needs to read the session record, and reported as part of that operation's normal output rather than needing any new surface at all. This may be relevant for koto if there's an existing "read a session" or "attach to a session" path that could carry the check for free.

## Open Questions

- Does koto have (or plan) a `koto session list` type command already, and if so, is adding a `prunable`/`orphaned` column there a low-cost addition, or does it require a distinct read path?
- What's the actual on-disk cost of a leaked session record — is this purely a UX/collision problem (as described) or is there also unbounded state-file growth that would argue for age-based auto-expiry rather than requiring an explicit sweep?
- Should the liveness check be "directory doesn't exist" only, or should it also consider "directory exists but is a different git repo/worktree now" (a rename/reuse case) — git's model keys off the `.git` file's identity, not just path existence, which might matter if koto session directories get reused by unrelated inits at the same path faster than by full deletion+recreation.
- Is there an existing koto issue or design doc number for this bug that the other exploration lanes are working from, to make sure terminology here ("template_source_dir", "already exists" error) lines up with what's actually in the codebase?

## Summary
The closest and most direct precedent is `git worktree prune`: git already answers "does the directory this record points to still exist," reports it passively as a "prunable" flag in `list` output, and only deletes admin state via a separate, explicitly-invoked `prune` command that defaults to safe (`--dry-run`, `--expire <age>`) rather than silent deletion — docker's dangling-volume filter and terraform's refresh-and-diff model reinforce the same two-layer shape (lazy read-time surfacing + separate gated sweep). For koto this argues against picking just one of the three candidate directions: fix the immediate collision bug with a read-time liveness check at collision time, add passive staleness surfacing to whatever session-listing path exists, and layer an explicit age-gated sweep/gc command on top for actual reclamation — while systemd's half-solved PID-staleness problem and VS Code's un-swept recent-folders list stand as warnings for what happens if only one of these three layers gets built. The main open question is where in koto's existing command surface (a `list`, an `attach`, or neither yet) the lazy check most cheaply attaches, which requires looking at koto's actual session code rather than external prior art.
