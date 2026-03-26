<!-- decision:start id="repo-id-derivation" status="assumed" -->
### Decision: repo-id derivation for session storage paths

**Context**

The local session storage design places sessions at `~/.koto/sessions/<repo-id>/<name>/`, where repo-id scopes sessions to a specific project directory. The design doc describes repo-id as "a hash of the working directory path" but leaves the algorithm, canonicalization, truncation, and collision handling unspecified.

Key constraints: sha2 and hex crates are already in Cargo.toml with a `sha256_hex()` utility in `src/cache.rs`. No new dependencies are preferred. The scheme must work without git. Claude Code's `~/.claude/projects/` directory uses a slug approach (replacing `/` with `-`), not a hash.

**Assumptions**

- Path lengths in practice stay under OS limits. The longest observed slug in Claude Code's projects directory is ~70 characters. Deeply nested paths could approach filesystem limits (~255 chars for a single component), but this is rare in practice. If wrong: sessions for extremely deep paths would fail to create with a clear filesystem error.
- Hash collisions at 16 hex characters (64 bits of entropy) are negligible for per-user project counts. Birthday paradox gives ~0.1% collision probability at 100,000 projects. If wrong: two projects would silently share a session namespace, corrupting each other's state.
- Users don't routinely access session directories by hand. Repo-id is an internal implementation detail surfaced only by `koto session dir`. If wrong: the slug approach (human-readable) would be preferable despite its length issues.

**Chosen: SHA-256 of canonicalized path, truncated to 16 hex characters**

Derive repo-id as follows:

1. **Canonicalize** the working directory path using `std::fs::canonicalize()`. This resolves symlinks and removes trailing slashes, `.`, and `..` components. The result is an absolute path with no ambiguity.
2. **Hash** the canonicalized path's UTF-8 bytes with SHA-256 using the existing `sha256_hex()` function from `src/cache.rs`.
3. **Truncate** to the first 16 hex characters (64 bits). This produces a compact directory name like `a1b2c3d4e5f6g7h8` that fits easily in terminal output and filesystem paths.
4. **No collision handling**. At 64 bits, collisions are astronomically unlikely for the number of projects a single developer works on. If two projects happen to collide, sessions would share a namespace -- but this probability is roughly 1 in 10^14 for 1,000 projects. Not worth adding complexity for.

Implementation in `src/session/local.rs`:

```rust
use crate::cache::sha256_hex;
use std::path::Path;

fn repo_id(working_dir: &Path) -> std::io::Result<String> {
    let canonical = std::fs::canonicalize(working_dir)?;
    let hash = sha256_hex(canonical.to_string_lossy().as_bytes());
    Ok(hash[..16].to_string())
}
```

This reuses the existing `sha256_hex` function with zero new dependencies.

**Rationale**

SHA-256 truncated to 16 hex chars hits the sweet spot between compactness and collision resistance. The codebase already depends on sha2/hex and has a utility function ready to use, so this adds zero code to the dependency tree. Canonicalization via `std::fs::canonicalize()` is the right normalization because it handles every ambiguity at once: symlinks, trailing slashes, relative paths, and `.`/`..` components. The 16-character length matches the design doc's example (`a1b2c3d4`) in spirit while providing much stronger collision resistance (64 bits vs 32 bits from 8 chars).

The alternative of following Claude Code's slug pattern was seriously considered, but slugs have practical problems: they get long for deep paths, they expose directory structure in `~/.koto/`, and they need custom sanitization logic for special characters. Hashes are uniform length and character-safe by construction.

**Alternatives Considered**

- **Path slug (Claude Code style)**: replace `/` with `-`, producing human-readable names like `-home-user-repos-my-project`. This is what Claude Code uses for `~/.claude/projects/`. Rejected because slugs grow proportionally with path depth, can hit filesystem name length limits (255 chars) for deeply nested workspaces, and expose directory structure. They also need custom handling for characters that are valid in paths but problematic in directory names (spaces, unicode). The human-readability benefit is minimal since users interact through `koto session dir`, not by browsing `~/.koto/sessions/` directly.

- **SHA-256, 8 hex characters (32 bits)**: matches the design doc's `a1b2c3d4` example. Rejected because 32 bits gives ~50% collision probability at ~65,000 projects (birthday paradox). While most developers won't hit this, it's unnecessarily risky when 16 characters costs nothing extra.

- **SHA-256, full 64 hex characters**: maximum collision resistance. Rejected because 64 characters makes paths unwieldy in terminal output and log messages. The marginal collision resistance over 16 characters (64 bits) provides no practical benefit at human-scale project counts.

- **Blake3 or FNV hash**: faster algorithms. Rejected because speed is irrelevant (hashing a single path string), they'd add new dependencies (violating the zero-new-deps constraint), and SHA-256 is already available in the codebase.

- **Hybrid slug+hash** (e.g., `my-project-a1b2c3d4`): human-readable prefix plus hash suffix. Rejected because extracting a meaningful prefix requires heuristics (last path component? repo name?), adds complexity, and the prefix can collide (many repos named "backend"). The hash alone is sufficient since `koto session dir` provides the lookup.

**Consequences**

- The repo-id is deterministic and stable as long as the canonical path doesn't change. Renaming or moving a project directory produces a new repo-id, orphaning existing sessions. This is documented in the design doc's Negative Consequences and mitigated by `koto session cleanup`.
- `std::fs::canonicalize()` requires the path to exist at call time. If called with a non-existent directory, it returns an error. This is correct behavior -- you can't create a session for a directory that doesn't exist.
- On systems where `canonicalize()` resolves symlinks differently (e.g., a symlink to the same repo from two locations), each path resolves to the same canonical path and gets the same repo-id. This is the desired behavior.
- The 16-character hex string is safe for all filesystems (ASCII alphanumeric only, well under name length limits).
<!-- decision:end -->
