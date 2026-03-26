# Pragmatic review: Local session storage design

## Findings

### 1. SessionBackend trait is justified but `state_file_path` is not
**Blocking.** The trait has one implementation today, but the cost is near-zero (one trait, one struct) and the second backend (git) is planned with clear demand (the entire PRD motivation is "wip/ in git looks bad" -- some users will want to keep it). The trait itself passes the bar. However, `state_file_path()` is just `session_dir(id).join(format!("koto-{}.state.jsonl", id))` -- a naming convention, not a backend concern. Every backend will produce the same path relative to session_dir. Inline it into the one caller or make it a free function.
**Fix:** Remove `state_file_path` from the trait. Add a free function `state_file_path(session_dir: &Path, id: &str) -> PathBuf` in the session module.

### 2. session.meta.json is ceremony
**Blocking.** The file stores `schema_version`, `id`, and `created_at`. The `id` is already the directory name. The `schema_version` is 1 and there's no migration logic. The `created_at` is used only by `koto session list` and can be derived from directory mtime via `fs::metadata().modified()`. The file's real job is to distinguish "valid session" from "random directory" in `exists()` and `list()` -- but koto owns `~/.koto/sessions/<repo-id>/`, so every subdirectory there is a session by definition. Nobody else writes to that path. The meta file adds a write on create, a read on list, a parse on exists, and an orphan-detection concept that only exists because the file might be missing.
**Fix:** Drop `session.meta.json`. Use directory existence for `exists()`. Use directory mtime for `list()`. If you need schema versioning later, add it then.

### 3. Features 3 and 4 on the roadmap are speculative
**Advisory.** The git backend (Feature 3) preserves backward compatibility for a workflow convention (`wip/`) that the PRD explicitly says has zero external users. There's nobody to be backward-compatible with. Cloud sync (Feature 4) requires tokio, aws-sdk, version counters, conflict resolution, and implicit sync -- it's a second product. The roadmap acknowledges Feature 4 is "most complex" but doesn't question whether it belongs. Both features are driving design decisions today (the trait shape "must accommodate cloud sync," session.meta.json has a version field for cloud conflict detection). Dead futures are shaping live code.
**Fix:** Move Features 3 and 4 to a "Potential future work" section. Remove any trait design accommodations for cloud sync (sync_down/sync_up mentions in the design doc). If cloud sync is ever needed, the trait can be extended -- the doc already acknowledges koto controls all implementations.

### 4. substitute_vars HashMap is over-built for one variable
**Advisory.** The design builds a `HashMap<String, String>` and iterates it with `str::replace` in a loop. There is exactly one variable: `SESSION_DIR`. A `str::replace("{{SESSION_DIR}}", &path)` call is simpler, shorter, and obviously correct. The HashMap exists to "provide a clean extension point for future --var support" -- that's speculative generality. When `--var` ships, it can introduce the HashMap.
**Fix:** Replace `substitute_vars(input, vars_map)` with `input.replace("{{SESSION_DIR}}", &session_dir)` at the two call sites. Delete vars.rs. Introduce the generalized version when issue #67 lands.

### 5. validate_session_id in session_dir() and state_file_path() is gold-plated
**Advisory.** The design says validation runs "in all public SessionBackend methods that accept an ID." But `session_dir()` is documented as "no I/O, just path computation" and returns a PathBuf -- it can't return an error. Validating in a method that returns PathBuf (not Result) means either panicking or silently ignoring invalid input. Validate in `create()` (the entry point that establishes the session). Callers of `session_dir()` are internal code that already passed through `create()` or `handle_init`. Trust code you control.
**Fix:** Validate session ID in `create()` only. Other methods assume the ID is valid (it came from a state file that was created through `create()`).

### 6. `koto session list` as JSON is scope creep for Feature 1
**Advisory.** The PRD says list shows "workflow name and last-modified time." Feature 1's job is to move state out of git. A list command with JSON serialization, orphan detection, and meta file parsing is session management UX -- useful, but not required to unblock the core storage move. `koto session dir` is the essential command; list and cleanup can ship in a follow-up.
**Fix:** Ship Feature 1 with `koto session dir` only. Add list/cleanup as a fast follow or fold into Feature 2 alongside the config system.

## No findings

- The repo-id derivation (SHA-256 of canonicalized path, 16 hex chars) is the simplest correct approach. Reuses existing `sha256_hex`, no new deps.
- `koto session dir` as the sole discovery mechanism is clean. No competing env var or convention.
- Runtime substitution (not compile-time at init) is the correct choice for `{{SESSION_DIR}}`.
- Threading `&dyn SessionBackend` through `run()` is straightforward dependency injection with no framework overhead.
- The security analysis is grounded and proportionate -- no threat theater.
