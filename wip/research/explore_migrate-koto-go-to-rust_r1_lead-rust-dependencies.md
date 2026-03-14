# Lead: Rust dependency choices

## Findings

### Current Go dependencies

From go.mod, the only external production dependency is `gopkg.in/yaml.v3`. Everything else (JSON, file I/O, process execution, flag parsing) uses the Go standard library. This means the Rust dependency surface is small and well-scoped.

### CLI parsing: clap v4 (derive macro)

The current Go implementation uses a hand-rolled flag parser. clap v4 with the derive macro is the correct Rust replacement:
- Derive macro generates type-safe subcommand/flag structs at compile time
- Produces `--help` output, error messages, and shell completions automatically
- Builder API only needed for runtime-generated CLIs (not applicable here)

```toml
clap = { version = "4", features = ["derive"] }
```

### Serialization: serde + serde_json + serde_yml

- `serde` with derive features for all structs
- `serde_json` for state file read/write (direct replacement for Go's `encoding/json`)
- `serde_yml` (actively maintained fork of serde_yaml) for YAML frontmatter parsing — `serde_yaml` still works but is unmaintained; `serde_yml` is the safer long-term choice

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yml = "0.7"
```

### Async vs. sync: synchronous only

koto has no networking, no concurrency, and no long-running background tasks. File I/O and process execution are all sequential within a single command invocation. **tokio is not needed.** Pup (datadog-labs/pup) uses tokio because it makes HTTP requests via reqwest; koto does not. Staying synchronous reduces complexity significantly.

Use `std::fs` for file operations and `std::process::Command` for command gates.

### Atomic file writes

The Go implementation does: `os.CreateTemp` → write → `file.Sync()` (fsync) → `os.Rename()`. Rust equivalent using standard library:

```rust
let tmp = NamedTempFile::new_in(state_dir)?;  // tempfile crate
tmp.as_file().write_all(&data)?;
tmp.as_file().sync_all()?;                     // fsync
tmp.persist(final_path)?;                      // atomic rename
```

The `tempfile` crate's `NamedTempFile` + `persist()` is the idiomatic Rust pattern. Note: full durability on Linux also requires `fsync` on the parent directory after rename — the Go implementation skips this; the Rust implementation should match current behavior for now.

```toml
tempfile = "3"
```

### Error handling: thiserror + anyhow

Use both together, mirroring the Go layering:
- `thiserror` for typed engine errors (`TransitionError`, `PersistenceError`, etc.) — gives structured variants that commands can match on
- `anyhow` at the CLI layer for error reporting — collects all errors for JSON output

```toml
thiserror = "1"
anyhow = "1"
```

### Command gate execution: std::process + wait-timeout

The Go implementation uses `exec.CommandContext` with timeout and process group kill. Rust:
- `std::process::Command` for subprocess execution
- `wait-timeout` crate for timeout enforcement (stdlib has no built-in process timeout)
- Unix process group via `std::os::unix::process::CommandExt::process_group(0)` for clean kill

```toml
[target.'cfg(unix)'.dependencies]
wait-timeout = "0.2"
```

### Testing

- `#[test]` for unit tests (engine logic, template parsing)
- `assert_cmd` for CLI integration tests — wraps `std::process::Command`, captures stdout/stderr, checks exit codes; replicates Go's integration_test.go pattern
- `assert_fs` or `tempfile` for filesystem test fixtures

```toml
[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
tempfile = "3"
```

### Full Cargo.toml dependencies

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yml = "0.7"
thiserror = "1"
anyhow = "1"
tempfile = "3"

[target.'cfg(unix)'.dependencies]
wait-timeout = "0.2"

[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
```

## Implications

The dependency surface is small and well-understood. No async runtime, no HTTP, no external service calls. The biggest non-obvious decision is `serde_yml` over `serde_yaml` (maintenance); everything else is the Rust standard for its problem category. The `wait-timeout` crate is Unix-only — if Windows support is ever needed, a different approach is required, but koto is Linux/macOS only.

## Surprises

- `serde_yaml` is unmaintained but still the most-installed crate in its category; `serde_yml` is the actively maintained fork but has lower adoption. Either works; `serde_yml` is safer long-term.
- pup uses tokio despite being a CLI tool — but only because it needs async HTTP. koto's synchronous model is simpler and correct for its use case.
- `std::process::Command` has no built-in timeout; `wait-timeout` crate is needed even for a simple 30-second gate timeout.
- Full atomic durability requires directory fsync after rename on Linux — the Go implementation doesn't do this; match current behavior for now.

## Open Questions

- The event-sourced refactor will use JSONL append (not full-file rewrite) — does this change the atomicity model enough that `tempfile` + rename is no longer the right primitive? (Likely not for the initial JSONL implementation, but worth noting in the design.)
- Windows support: `wait-timeout` is Unix-only. Not a concern now but worth noting.

## Summary

Clap v4 derive, serde_json, serde_yml, thiserror+anyhow, tempfile, and std::process cover all of koto's dependency needs without async or networking. Shell command gates require `wait-timeout` since stdlib has no process timeout. The dependency surface is minimal — far smaller than typical Rust CLI projects — because koto's Go implementation already used almost no external libraries.
