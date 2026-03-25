# Decision Report: Local Session Storage Location

**Decision ID:** local-session-storage-4
**Date:** 2026-03-25
**Status:** Recommendation ready

## Research Findings

### 1. git clean behavior with .git/ custom directories

Tested `git clean -fdx` against files stored under `.git/koto/sessions/`:

- **`git clean -fd`**: Does NOT touch `.git/` contents. Survived.
- **`git clean -fdx`**: Does NOT touch `.git/` contents. Survived.
- **`git gc --prune=now`**: Does NOT touch `.git/` contents. Survived.
- **`git status`**: `.git/koto/` is completely invisible. Never listed.
- **`git diff`**: No effect. Invisible.

Git never operates on its own `.git/` directory through clean, status, or diff commands. This is by design -- `.git/` is the repository metadata directory, and git commands that deal with the working tree ignore it entirely.

### 2. git clean behavior with excluded/gitignored directories

Tested `.koto/` excluded via both `.gitignore` and `.git/info/exclude`:

- **`git clean -fd`**: Does NOT remove ignored/excluded directories. Safe.
- **`git clean -fdx`**: **REMOVES** ignored/excluded directories. Both `.gitignore` and `info/exclude` entries get wiped. The `-x` flag means "don't use ignore rules" -- it deletes everything untracked regardless of exclusion.

This means Options D and E have a data-loss risk when users run `git clean -fdx`, which is common in CI environments and during "nuke everything" resets.

### 3. .git/info/exclude: standard support

- Created automatically by `git init` in every repository
- Pre-populated with comment block explaining its purpose
- Documented in `gitignore(5)` manpage alongside `.gitignore`
- Works identically to `.gitignore` but is local-only (never committed)
- Supported by all git versions in common use
- Less well-known than `.gitignore` among casual git users, but standard

### 4. Agent sandbox behavior with .git/

Claude Code's file tools (Read/Edit/Write) operate on the filesystem directly. Based on current behavior:

- Claude Code can read and write files anywhere the process has OS-level permissions
- No evidence of explicit `.git/` blocking in Claude Code's sandboxing
- However, future agent frameworks or stricter sandbox configurations could restrict `.git/` access as a safety measure (preventing accidental repository corruption)
- Some IDE-integrated agents restrict file operations to the working tree, explicitly excluding `.git/`

This is a real but speculative risk. The `.git/` directory is designed for git's own use, and security-conscious tools may block writes there.

## Constraint Matrix

| Constraint | A (.git/koto/) | B (symlink) | C (~/.koto/) | D (.gitignore) | E (info/exclude) |
|---|---|---|---|---|---|
| Agent file tools work | Yes* | Partial** | Partial*** | Yes | Yes |
| No git config changes | Yes | No**** | Yes | No | Depends***** |
| Invisible to git status | Yes | No | Yes | Yes | Yes |
| Invisible to git diff | Yes | Yes | Yes | Yes | Yes |
| Survives git add . | Yes | No | Yes | Yes | Yes |
| Survives git clean -fdx | **Yes** | N/A | Yes | **No** | **No** |
| Linux + macOS | Yes | Partial | Yes | Yes | Yes |

\* Some agents may block `.git/` writes in the future.
\** Symlink itself appears in git status.
\*** Sandbox may block paths outside repo.
\**** Symlink appears in git status unless gitignored.
\***** `.git/info/exclude` is a git config file, but it's local-only and never tracked.

## Evaluation

### Option A: .git/koto/sessions/ -- Recommended

Strengths:
- Satisfies all hard constraints with zero configuration
- Immune to `git clean -fdx` (unique among all options)
- Zero git configuration needed -- not even local exclude files
- Files are invisible to every git command by definition
- Precedent exists: git hooks, worktree data, and third-party tools (e.g., git-lfs, git-branchless) store data in `.git/`

Weaknesses:
- Unconventional. Developers may be surprised to find non-git data in `.git/`
- Future agent sandboxes might restrict `.git/` writes
- IDEs or git GUIs that watch `.git/` might react to changes (though most only watch specific subdirectories like `refs/` and `HEAD`)

Risk mitigation: If an agent sandbox blocks `.git/` access, koto can detect the failure and fall back to an alternative path. The storage path should be a single configurable constant, making a future migration straightforward.

### Option B: Symlink -- Rejected

The symlink itself is an untracked file that appears in `git status`, which directly violates constraint #3. This can only be fixed by gitignoring it, which violates constraint #2. Additionally, Windows symlink support is unreliable.

### Option C: ~/.koto/sessions/ -- Viable fallback

Clean separation, but breaks when agents are sandboxed to the repo tree. The global namespace collision (two repos with the same workflow) requires additional disambiguation logic (embedding repo path hash in session keys). Best suited as a fallback if Option A is blocked at runtime.

### Option D: .gitignore management -- Rejected

Violates the "no git configuration changes" constraint. While many tools do auto-modify `.gitignore`, the constraint was explicitly set for this decision. Also vulnerable to `git clean -fdx`.

### Option E: .git/info/exclude -- Second choice

Elegant compromise: modifies only a local, never-committed file. But it fails the `git clean -fdx` durability test, and whether editing `.git/info/exclude` counts as "git config change" depends on interpretation. If the constraint is relaxed to "no changes to tracked files," this becomes a strong option combined with `.koto/sessions/` in the working tree.

## Recommendation

**Primary: Option A** -- `.git/koto/sessions/<id>/`

Use `.git/koto/` as the storage root. This is the only option that satisfies every hard constraint including durability against `git clean -fdx`. The concern about it being unconventional is real but manageable -- koto owns this directory and can document its purpose clearly.

**Fallback: Option C** -- `~/.koto/sessions/<id>/` with repo-scoped disambiguation

If an agent environment blocks `.git/` writes at runtime, fall back to `~/.koto/sessions/<repo-hash>/<id>/`. Detect the failure, log a clear message, and continue.

**Implementation path:**
1. Define a `session_root()` function that returns `.git/koto/sessions/`
2. On write failure to `.git/koto/`, detect permission errors and fall back to `~/.koto/sessions/<repo-hash>/`
3. Store the resolved path choice in session metadata so reads use the same location
4. Document in koto's help output that sessions live in `.git/koto/` by default

## Decision

**Chosen option: A (.git/koto/sessions/) with C as runtime fallback.**

The `.git/` directory provides a location that is invisible to git by definition, survives all cleaning operations, requires zero configuration, and lives inside the repo tree where agent file tools can reach it. The fallback to `~/.koto/` handles edge cases where `.git/` is restricted.
