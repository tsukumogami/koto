# Architecture Review: DESIGN-local-session-storage

**Reviewer**: Architect
**Date**: 2026-03-26
**Status**: Review complete

---

## 1. Is the architecture clear enough to implement from the design doc alone?

**Mostly yes, with three gaps.**

The design specifies module locations, trait shape, data flow, and CLI integration clearly enough for an implementer to start work. The code examples are close to real Rust and reference actual crate functions. Three areas need clarification before implementation:

### Gap A: `run()` signature change is underspecified

The design says "command handlers receive `&dyn SessionBackend`" but `run()` currently takes `App` (the parsed CLI struct) and dispatches internally. The design shows `run()` constructing the backend and passing it to handlers, but the actual signature is `pub fn run(app: App) -> Result<()>`. The design must specify whether:

- `run()` continues to take `App` and constructs the backend internally (most likely, and consistent with the code example), or
- The backend is constructed in `main.rs` and passed to `run()`.

The first option matches the design's code example but contradicts the "backend constructed in run(), passed to command handlers" framing slightly, since `handle_next` is a free function that currently receives individual fields, not `&dyn SessionBackend`. The refactoring scope for `handle_next` (which takes `name: String, with_data: Option<String>, to: Option<String>`) needs to be explicit: it must gain a `&dyn SessionBackend` parameter and stop calling `workflow_state_path` directly.

**Severity: Advisory.** The intent is clear; the gap is in mechanical detail that an implementer will resolve.

### Gap B: `find_workflows_with_metadata` migration path

The design says Phase 2 updates `find_workflows_with_metadata()` to "scan `~/.koto/sessions/<repo-id>/` instead of the working directory." Currently this function scans for `koto-*.state.jsonl` files in a directory (discover.rs:50-73). After the change, it needs to:

1. Enumerate subdirectories of `~/.koto/sessions/<repo-id>/`
2. Look for `koto-<name>.state.jsonl` inside each subdirectory
3. Or read `session.meta.json` to get the session list, then read state file headers

The design doesn't specify which approach. Option 3 (use `SessionBackend::list()` then read headers) would be the clean architectural path since it routes through the backend abstraction. Option 2 duplicates the backend's knowledge of directory layout in discover.rs. This matters because it determines whether `find_workflows_with_metadata` stays in `discover.rs` or moves to use the backend.

**Severity: Advisory.** Both approaches work, but the design should pick one to avoid an implementer making an inconsistent choice.

### Gap C: `handle_init` creates the state file, but where exactly?

The design says `handle_init` calls `backend.create(name)` which returns a `PathBuf` (the session directory), then "write koto-my-workflow.state.jsonl into session dir." Currently `handle_init` calls `append_header(&state_path, &header)` where `state_path = workflow_state_path(&current_dir, &name)`. After the change, the state path must be computed as `backend.session_dir(&name).join(format!("koto-{}.state.jsonl", name))`.

This means `workflow_state_path()` is called twice for different purposes:
1. By `LocalBackend::session_dir()` internally (the design says this, line 186-187)
2. By `handle_init` to compute the state file path within the session directory

Wait -- `workflow_state_path(dir, name)` produces `<dir>/koto-<name>.state.jsonl`. If the session dir is `~/.koto/sessions/<repo-id>/<name>/`, then `workflow_state_path(&session_dir, &name)` would produce `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`. This works, but the design says "`workflow_state_path()` still exists but is called by `LocalBackend::session_dir()` internally" -- that's wrong. `session_dir()` returns the *directory*, not the state file path. `workflow_state_path` should be called to compute the state file path *within* the session directory, not by `session_dir()`.

**Severity: Blocking (minor).** The design contains a factual error about which function calls `workflow_state_path`. The state file path derivation needs a clear owner. Recommend adding a `session_state_path(&self, id: &str) -> PathBuf` method to the trait (or a helper on `LocalBackend`) that returns `self.session_dir(id).join(format!("koto-{}.state.jsonl", id))`. This avoids every command handler reimplementing the path join.

---

## 2. Are there missing components or interfaces?

### Missing: state file path accessor

As noted in Gap C, no interface returns the state file path within a session. Every command handler (init, next, cancel, rewind) needs the state file path. Without a method like `state_file_path(&self, id: &str) -> PathBuf`, each handler will independently compute `backend.session_dir(id).join(format!("koto-{}.state.jsonl", id))`. This is the same "hardcoded path construction" pattern the design is trying to eliminate.

**Recommendation**: Add `fn state_file_path(&self, id: &str) -> PathBuf` to `SessionBackend`, or as a default method that delegates to `session_dir()` + the state file name convention.

### Missing: existence check semantics for `koto init`

Currently `handle_init` checks `state_path.exists()` to prevent double-init. After the change, should it check `backend.exists(id)` (which checks for `session.meta.json`) or check for the state file directly? The design's `exists()` checks for `session.meta.json`, but a session directory could have the meta file without a state file (if init crashed between `backend.create()` and `append_header()`). The design should specify that `handle_init` uses `backend.exists()` and that `create()` is idempotent or returns an error if the session already exists.

**Severity: Advisory.** Edge case, but the design should have an opinion.

### Not missing: `dirs` crate

The design correctly identifies that `dirs` needs to be added to Cargo.toml. The existing `cache_dir()` function in `src/cache.rs` uses `$HOME` directly (lines 12-20) rather than `dirs`. This creates a minor inconsistency: cache uses `$XDG_CACHE_HOME`/`$HOME`, while sessions would use `dirs::home_dir()`. Not a structural problem, but worth noting for future alignment.

---

## 3. Are the implementation phases correctly sequenced?

**Yes, with one dependency clarification.**

- **Phase 1** (session module) has no dependencies on existing code changes. It only adds new files and the `dirs` crate. Correct starting point.
- **Phase 2** (CLI refactoring) depends on Phase 1. Correct.
- **Phase 3** (session subcommands) depends on Phase 2. Correct.

The constraint that "Phase 3 must ship in the same release as Phases 1-2" (line 539) is correctly identified.

**One sequencing concern**: Phase 2 says "verify all existing tests pass with state files in the new location." The existing integration tests (in `tests/`) likely create workflows in temp directories and look for state files there. These tests will break when state files move to `~/.koto/sessions/`. The design should note that Phase 2 requires either:
- Test infrastructure that overrides the backend's base directory (e.g., `LocalBackend::with_base_dir(path)` for testing), or
- A `KOTO_HOME` env var that redirects `~/.koto` (but the design explicitly rejects env var overrides for skills -- should clarify that a test-only override is different from a user-facing env var).

Without this, Phase 2 tests will write to the developer's real `~/.koto/sessions/` directory.

**Severity: Blocking.** Test isolation must be specified. The `LocalBackend::new()` constructor should accept a configurable base path for testing, or `LocalBackend` should support a `with_base_dir()` constructor.

---

## 4. Are there simpler alternatives we overlooked?

### Considered and correctly rejected

The design evaluates the right alternatives. In particular:
- "No trait, just LocalBackend struct" is correctly identified as too rigid.
- XDG vs `~/.koto/` is a reasonable call given cross-platform needs.
- The coordinated release (Decision 6) is the right call for a team that controls all consumers.

### One simplification worth considering

The `session.meta.json` file adds a second source of truth alongside the state file header. The state file header already contains `workflow` (name) and `created_at`. The meta file duplicates both. The design uses `session.meta.json` as the existence marker for `exists()` and `list()`, but the state file itself could serve that purpose.

If `exists()` checked for the state file instead of `session.meta.json`, and `list()` scanned for state files, the meta file becomes unnecessary for Feature 1. It could be introduced later if cloud sync (Feature 5) needs metadata that doesn't belong in the state file.

Removing `session.meta.json` from Feature 1 would:
- Eliminate the crash window between `create()` writing meta and `handle_init` writing the state file
- Remove one file format to maintain
- Simplify `create()` to just `mkdir`

**Severity: Advisory.** The meta file isn't harmful, but it adds complexity without a current consumer beyond `exists()`/`list()`, which could use the state file instead.

---

## 5. Do decisions 5 and 6 integrate cleanly with decisions 1-4?

### Decision 5 (repo-id derivation): Clean integration

- Uses existing `sha256_hex()` from `src/cache.rs` -- no new dependencies.
- The code example (`fn repo_id(working_dir: &Path)`) is correct and uses `std::fs::canonicalize()` which is stdlib-only.
- 16-character truncation is a reasonable tradeoff.
- The `to_string_lossy()` call on line 281 is technically lossy on non-UTF-8 paths (rare on modern systems, but present on some Linux configurations). `canonicalize()` returns an `OsString`, and `to_string_lossy()` replaces invalid UTF-8 with U+FFFD. Two different non-UTF-8 paths could map to the same lossy string and collide. This is astronomically unlikely in practice but worth a code comment.

### Decision 6 (migration strategy): Clean integration with a coordination risk

The "coordinated release" framing is architecturally sound -- it avoids compatibility layers, env var overrides, and transition-only code in koto. The ~150 path references in shirabe are a mechanical replacement.

One structural concern: the design says "session directory location" (Decision 4) means zero repo footprint. But `koto init` currently stores the template cache path in the state file's `WorkflowInitialized` event (`template_path: cache_path_str` on line 206 of cli/mod.rs). This absolute path to `~/.cache/koto/<hash>.json` is already outside the repo. The session state file moving to `~/.koto/sessions/` is consistent with this precedent.

No integration issues between D5/D6 and D1-D4.

---

## 6. Are the code examples in the doc consistent with the actual codebase?

### Consistent

- `workflow_state_path(dir, name)` signature matches discover.rs:12 (`pub fn workflow_state_path(dir: &Path, name: &str) -> PathBuf`).
- `sha256_hex(data: &[u8]) -> String` signature matches cache.rs:23.
- `sha2` and `hex` crates are in Cargo.toml (lines 19-20).
- `dirs` is NOT in Cargo.toml (correctly identified as a new dependency).
- `find_workflows_with_metadata(dir: &Path)` signature matches discover.rs:23.

### Inconsistent

1. **Design line 186-187**: "workflow_state_path() still exists but is called by LocalBackend::session_dir() internally." This is wrong. `session_dir()` returns a directory path; `workflow_state_path()` appends the state file name. `session_dir()` doesn't need `workflow_state_path()` -- it's `self.base_dir.join(id)`. The state file path needs a separate computation: `session_dir(id).join(workflow_state_path_filename(id))` or similar.

2. **Design line 219**: The example shows `a1b2c3d4e5f6g7h8` as a "first 16 hex chars" but includes `g` which is not a hex digit. This is a typo in the example directory names.

3. **Design line 169**: "The run() function in src/cli/mod.rs (the CLI entry point)" -- `run()` is not the CLI entry point. `main()` in `src/main.rs` is. `run()` is the dispatch function called by `main()`. Minor but could confuse an implementer looking for where to construct the backend.

4. **`run()` currently takes `App`**: The design's code example shows `run()` doing `let working_dir = std::env::current_dir()?; let backend = LocalBackend::new(&working_dir)?; match cli.command { ... }`. But `run()` currently takes `app: App` and matches on `app.command`. The variable name difference (`cli.command` vs `app.command`) is trivial but the design should match the actual parameter name to avoid confusion.

---

## Summary of findings

| # | Finding | Severity | Recommendation |
|---|---------|----------|----------------|
| 1 | No `state_file_path()` method on trait; handlers will reimplement path join | Blocking | Add `state_file_path(&self, id: &str) -> PathBuf` to the trait or as a helper |
| 2 | Test isolation unspecified; Phase 2 tests will write to real `~/.koto/` | Blocking | Add `LocalBackend::with_base_dir()` constructor for testing |
| 3 | Design says `workflow_state_path()` is called by `session_dir()` -- it isn't | Blocking (minor) | Correct the design text; specify who computes the state file path |
| 4 | `session.meta.json` duplicates state file header fields | Advisory | Consider deferring meta file to a later feature |
| 5 | `find_workflows_with_metadata` migration approach unspecified | Advisory | Specify it routes through `SessionBackend::list()` |
| 6 | Hex typo in directory example (`g7h8` is not hex) | Advisory | Fix example |
| 7 | `run()` code example uses `cli.command` not `app.command` | Advisory | Align with actual code |
