# Security Review: DESIGN-local-session-storage

## Scope

Review of the full design document with focus on Decision 5 (repo-id derivation),
Decision 6 (coordinated skill migration), and the Security Considerations section.

---

## 1. Hash scheme (Decision 5)

### 1.1 Collision resistance is adequate

16 hex characters = 64 bits. Birthday-bound collision probability at 100k projects
is ~0.1%. For a single user's project count this is negligible. No issue here.

### 1.2 `to_string_lossy()` is a latent correctness risk

```rust
let hash = sha256_hex(canonical.to_string_lossy().as_bytes());
```

`to_string_lossy()` replaces invalid UTF-8 sequences with U+FFFD. Two distinct
paths containing different invalid byte sequences could map to the same lossy
string and the same hash. This is not an attack vector in practice (paths with
invalid UTF-8 are rare and would need to be attacker-controlled), but it's a
correctness defect that could cause silent session collisions on exotic filesystems.

**Recommendation (low priority):** Use `canonical.as_os_str().as_encoded_bytes()`
(stable since Rust 1.74) to hash the raw bytes. This eliminates the lossy
conversion entirely. If the minimum Rust version doesn't support it,
`canonical.to_str().ok_or("non-UTF-8 path")?` is preferable to silent
replacement -- it fails loudly instead of silently colliding.

### 1.3 Canonicalization is the right choice but has edge cases

`std::fs::canonicalize()` resolves symlinks, which means:

- Two symlinks to the same directory get the same repo-id (correct).
- A bind mount of the same directory at two paths gets two different repo-ids
  (correct -- they're distinct mount points).
- Network filesystems (NFS, SSHFS) where the canonical path includes a mount
  point: works fine, the canonical path is stable as long as the mount is stable.

No security issue here. The design correctly notes that `canonicalize()` requires
the path to exist, which is the right behavior.

### 1.4 No TOCTOU in repo-id derivation

The repo-id is computed once at `LocalBackend::new()` and stored in `base_dir`.
No re-derivation happens between check and use. Clean.

---

## 2. Path traversal and session ID validation

### 2.1 Allowlist is correct

`^[a-zA-Z0-9._-]+$` prevents path traversal (no `/`, `\`, null bytes, or `..`
as a path component since the regex requires at least one character and `.` alone
or `..` alone would only match if the full ID is `.` or `..`).

**Issue: the regex allows `.` and `..` as session IDs.** The pattern
`^[a-zA-Z0-9._-]+$` matches both `.` and `..`. While `base_dir.join("..")`
wouldn't escape `~/.koto/sessions/<repo-id>/` to a dangerous location (it would
resolve to `~/.koto/sessions/`), it could cause:

- `cleanup("..")` calling `remove_dir_all` on `~/.koto/sessions/<repo-id>/../`,
  which is `~/.koto/sessions/` -- deleting ALL sessions for ALL projects.
- `cleanup(".")` calling `remove_dir_all` on `~/.koto/sessions/<repo-id>/./`,
  which is `~/.koto/sessions/<repo-id>/` -- deleting all sessions for this project.

**Severity: HIGH.** This is a real destructive path traversal via `..` as a
session ID.

**Recommendation (must fix):** Add explicit rejection of `.` and `..` in
`validate_session_id()`:

```rust
fn validate_session_id(id: &str) -> Result<()> {
    if id == "." || id == ".." {
        return Err(anyhow!("session ID cannot be '.' or '..'"));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
        return Err(anyhow!("invalid session ID"));
    }
    Ok(())
}
```

Alternatively, tighten the regex to require the ID starts with an alphanumeric
character: `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`.

### 2.2 `session_dir()` does not validate

`session_dir()` is documented as "no I/O, just path computation" and does not
call `validate_session_id()`. Only `create()` validates. If any code path calls
`session_dir()` with untrusted input without going through `create()` first
(e.g., `koto session dir <user-input>`), the validation is bypassed.

**Recommendation:** Add validation in `session_dir()` as well, or at the CLI
argument parsing layer for all commands that accept a session name. Defense in
depth -- validate at every entry point, not just creation.

### 2.3 `cleanup()` does not validate

Same issue. `cleanup()` calls `remove_dir_all` on a path derived from the ID
but doesn't validate the ID first. Combined with 2.1, this is the actual
exploitation path.

**Recommendation:** `cleanup()` must call `validate_session_id()` before
constructing the path.

---

## 3. Filesystem permissions and symlink attacks

### 3.1 No symlink following in session directories

The design uses `fs::create_dir_all` and `fs::remove_dir_all`. If an attacker
can create a symlink at `~/.koto/sessions/<repo-id>/<name>` pointing to an
arbitrary directory, `remove_dir_all` would follow it and delete the target.

**Practical risk: LOW.** The attacker would need write access to `~/.koto/`,
which means they already have the user's privileges. This is a non-issue for
single-user systems. On shared systems with `HOME` on a shared filesystem, it
could be relevant, but that's an unusual deployment.

**Recommendation (low priority):** Before `remove_dir_all`, verify that the
target path is not a symlink: `if path.is_symlink() { fs::remove_file } else
{ fs::remove_dir_all }`.

### 3.2 Umask and directory permissions

The design says "permissions follow the user's umask." This is correct for
single-user systems. The default umask (022) creates directories readable by
others. Session artifacts may contain project-specific information (research
notes, plan contents).

**Recommendation (low priority):** Consider creating `~/.koto/` with 0700
permissions on first creation. This is what `~/.ssh/` and `~/.gnupg/` do. Not
strictly necessary since session data isn't secrets, but it's good hygiene.

---

## 4. Coordinated release strategy (Decision 6)

### 4.1 Atomic release dependency

The design states "neither ships without the other." This creates a tight
coupling between koto and skill releases. If the skill migration is incomplete
or a skill is missed, users get silent failures (skills write to `wip/` which
no longer exists, koto reads from `~/.koto/sessions/` which has no skill
artifacts).

**Security concern: LOW.** This is a reliability/correctness issue, not a
security issue. There's no privilege escalation or data exposure from a
mismatched release. The worst case is workflow failures that require manual
intervention.

### 4.2 Shell injection via `koto session dir`

Skills will call:
```bash
SESSION_DIR=$(koto session dir "$name")
```

If `koto session dir` outputs a path containing shell metacharacters, and a
skill doesn't quote `$SESSION_DIR`, there's a shell injection risk. However:

- The output path is `~/.koto/sessions/<hex-hash>/<validated-id>/`
- The hex hash contains only `[0-9a-f]`
- The validated ID contains only `[a-zA-Z0-9._-]`
- The home directory path is OS-controlled

The output path cannot contain shell metacharacters unless the user's home
directory path does (e.g., a username with spaces on macOS is possible but
unusual). **Practical risk: negligible.**

### 4.3 No rollback path

If the coordinated release has a defect, there's no way to revert koto without
also reverting skills. The design acknowledges "no backward-compatible transition
period." This is acceptable given the co-versioned assumption but means any
security fix in the session layer requires a coordinated hotfix.

**Recommendation:** Document the coordinated rollback procedure. Not a security
issue per se, but affects incident response speed.

---

## 5. Removal of `version` from session.meta.json

The design shows `session.meta.json` containing only `id` and `created_at`. No
`version` field.

### 5.1 No integrity checking impact

The current design has no integrity checking at all -- no signatures, no
checksums, no version validation. The `version` field wouldn't have provided
integrity checking; at most it would have provided format compatibility detection.

### 5.2 Forward compatibility risk

Without a version field, if the session.meta.json schema changes in a future
release, there's no way to detect that a session was created by an older version
and needs migration. This isn't a security issue but is a maintenance concern.

**Recommendation:** Add a `schema_version: 1` field to session.meta.json. This
costs nothing and prevents a class of upgrade bugs. It doesn't affect integrity
(an attacker could change the version field just as easily), but it enables
safe schema evolution.

---

## 6. Risks not covered in the design's Security Considerations

### 6.1 Race condition in `create()`

`create()` calls `validate_session_id()`, then `create_dir_all()`, then
`write_session_meta()`. If two processes call `create()` for the same session
ID simultaneously, both may succeed, and the second `write_session_meta()` call
overwrites the first. The design doesn't mention concurrency.

**Practical risk: LOW.** koto workflows are typically single-agent. But if a
user runs `koto init` twice in quick succession (e.g., from two terminal tabs),
data loss is possible.

**Recommendation:** Use atomic writes for `session.meta.json` (write to a temp
file, then rename). The codebase already uses this pattern in `src/cache.rs`
with `tempfile::Builder`.

### 6.2 `~/.koto/` as an attack surface for agent sandboxes

The design notes that users must add `~/.koto` to `sandbox.filesystem.allowRead`
and `allowWrite`. This means ANY koto session from ANY project can be read or
modified by an agent running in ANY project. A malicious template in project A
could read session artifacts (research notes, plans) from project B.

**Severity: MEDIUM.** This is inherent to the home-directory storage model and
can't be fixed without per-project sandbox rules (which agent platforms don't
currently support). The design should acknowledge this cross-project read/write
risk explicitly.

**Recommendation:** Document this in the Security Considerations section. Users
who work on sensitive projects should be aware that all projects share the
`~/.koto/` sandbox allowance.

### 6.3 Disk exhaustion

No limits on session count or total size. A runaway skill could fill
`~/.koto/sessions/` with artifacts. This is a denial-of-service against the
user's disk.

**Practical risk: LOW.** Same risk exists today with `wip/` in the repo. Not
worse, just moved.

---

## Summary of findings

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 2.1 | `.` and `..` bypass in session ID validation allows destructive path traversal | HIGH | Must fix before implementation |
| 2.2 | `session_dir()` skips validation | MEDIUM | Add validation at all entry points |
| 2.3 | `cleanup()` skips validation | MEDIUM | Add validation before `remove_dir_all` |
| 6.2 | Cross-project session access via shared sandbox allowance | MEDIUM | Document in Security Considerations |
| 1.2 | `to_string_lossy()` silent collision on non-UTF-8 paths | LOW | Use raw bytes or fail on non-UTF-8 |
| 5.2 | No schema version in session.meta.json | LOW | Add `schema_version: 1` |
| 3.2 | Default umask may leave sessions world-readable | LOW | Create `~/.koto/` with 0700 |
| 6.1 | Race condition in `create()` | LOW | Use atomic writes |
| 3.1 | Symlink following in `remove_dir_all` | LOW | Check for symlinks before removal |

No residual risk warrants escalation beyond the development team. The HIGH
finding (2.1) is straightforward to fix and should block implementation until
resolved. The MEDIUM findings are defense-in-depth improvements that should
ship with the initial implementation.
