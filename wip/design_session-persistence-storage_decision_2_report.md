# Decision 2: Config system implementation approach

## Context

koto needs a configuration system to support the session persistence storage feature (PRD R8). The system must handle:

- `koto config get <key>` and `koto config set <key> <value>` CLI commands
- Two config files: user (`~/.koto/config.toml`) and project (`.koto/config.toml`)
- Merge precedence: project > user > defaults
- Env var overrides for credentials (`AWS_ACCESS_KEY_ID`, etc.)
- Dot-notation keys mapping to nested TOML tables

## Current codebase state

Dependencies in Cargo.toml: `clap` (derive), `serde`/`serde_json`/`serde_yml`, `sha2`, `hex`, `thiserror`, `anyhow`, `tempfile`. No TOML crate today.

The CLI dispatches via a `Command` enum with `clap` derive macros in `src/cli/mod.rs`. Adding a `Config` subcommand with `ConfigSubcommand` (Get/Set) follows the existing `Template`/`TemplateSubcommand` pattern exactly.

## Options evaluated

### Option A: Simple TOML with `toml` crate

Add `toml = "0.8"` to dependencies. Define a `KotoConfig` struct with serde derives. On `config get`: read both files, deserialize each into `toml::Value`, deep-merge project over user over defaults, then navigate the merged tree using dot-notation key splitting. On `config set`: read the target file (user or project), parse to `toml::Value`, insert the nested key, serialize back.

**Pros:**
- `toml` is the standard Rust TOML crate (used by Cargo itself). Mature, well-maintained, zero transitive surprises.
- Full control over merge logic. The merge function is ~30 lines of recursive table overlay.
- Env var override is a few lines: check `std::env::var` for known credential keys before returning config values.
- Fits the existing pattern: koto already uses `serde_json` and `serde_yml` for other formats with hand-written logic around them.
- The `toml` crate supports both serialization and deserialization, so round-tripping config files preserves formatting reasonably well.

**Cons:**
- Must write the merge logic (~30 lines) and dot-notation key navigation (~20 lines).
- No built-in env var layering -- but we only need it for a handful of credential keys.

**Estimated effort:** ~150-200 lines of library code, ~80 lines of CLI dispatch.

### Option B: `config-rs` crate

Use the `config` crate which provides layered configuration from multiple sources (files, env vars, defaults).

**Pros:**
- Built-in source layering and env var prefix mapping.
- Handles file format detection automatically.

**Cons:**
- Adds a dependency with its own transitive tree (`config` pulls in `nom`, `async-trait`, and others depending on features).
- koto's config needs are narrow: two TOML files, a few env vars, and dot-key access. `config-rs` is designed for applications with many config sources, which is more than we need.
- `config-rs` is read-only by design. It doesn't support `config set` (writing back to files). We'd still need `toml` crate for the write path, meaning two config mechanisms.
- The crate's env var mapping uses prefix conventions (`KOTO_SESSION_BACKEND`) which don't match our requirement to honor standard AWS env vars (`AWS_ACCESS_KEY_ID`).
- Last major release (0.14) had breaking API changes; the crate's stability track record is mixed.

**Estimated effort:** ~100 lines for read path, plus ~100 lines for write path using `toml` crate anyway. Net savings minimal, added complexity from two systems.

### Option C: Hand-rolled parser, no crate

Parse TOML manually without any dependency.

**Pros:**
- Zero new dependencies.

**Cons:**
- TOML parsing is not trivial. Even the subset we need (strings, integers, booleans, nested tables, arrays) is several hundred lines of parser code.
- koto already has 7 dependencies. Adding one well-known crate (`toml`) is a smaller maintenance cost than maintaining a hand-rolled parser.
- Bug surface area: a custom parser will have edge cases that the `toml` crate has already handled (escape sequences, multiline strings, inline tables).
- Writing config back out while preserving formatting is much harder without a proper serializer.

**Estimated effort:** 400+ lines of parser code, ongoing maintenance burden.

## Recommendation: Option A

Option A is the clear choice. Here's why:

1. **Minimal new dependency.** The `toml` crate is a single, stable, widely-used dependency with a small footprint. It's the de facto standard for TOML in Rust.

2. **Full read+write support.** Unlike `config-rs`, the `toml` crate handles both directions natively. `config set` works naturally.

3. **Matches existing patterns.** koto already uses `serde_json` for JSON and `serde_yml` for YAML with hand-written logic on top. Using `toml` with hand-written merge logic follows the same approach.

4. **The merge logic is straightforward.** Two TOML tables, recursive overlay, with env var checks for credential keys. This isn't complex enough to justify a framework.

5. **Env var handling stays explicit.** We need to honor `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` -- standard env var names that don't follow any config crate's prefix convention. Explicit `std::env::var` checks are clearer and more correct.

## Implementation sketch

### Config struct

```rust
// src/config.rs

use std::path::{Path, PathBuf};

/// Resolve the user config path: ~/.koto/config.toml
pub fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".koto").join("config.toml"))
}

/// Resolve the project config path: .koto/config.toml relative to repo root
pub fn project_config_path(project_root: &Path) -> PathBuf {
    project_root.join(".koto").join("config.toml")
}

/// Load and merge config: defaults <- user <- project <- env overrides
pub fn load_merged_config(project_root: &Path) -> toml::Value { ... }

/// Get a value by dot-notation key from merged config
pub fn get_key(config: &toml::Value, key: &str) -> Option<String> { ... }

/// Set a value by dot-notation key in the specified config file
pub fn set_key(path: &Path, key: &str, value: &str) -> anyhow::Result<()> { ... }
```

### CLI addition

```rust
// In Command enum:
Config {
    #[command(subcommand)]
    subcommand: ConfigSubcommand,
},

// New enum:
enum ConfigSubcommand {
    Get { key: String },
    Set {
        key: String,
        value: String,
        #[arg(long)]
        project: bool,
    },
}
```

### Merge precedence

1. Start with hardcoded defaults (`session.backend = "local"`)
2. Deep-merge user config on top
3. Deep-merge project config on top
4. For specific credential keys, check env vars last

### Env var override mapping

| Config key | Env var |
|---|---|
| `session.cloud.access_key` | `AWS_ACCESS_KEY_ID` |
| `session.cloud.secret_key` | `AWS_SECRET_ACCESS_KEY` |
| `session.cloud.region` | `AWS_REGION` or `AWS_DEFAULT_REGION` |
| `session.cloud.endpoint` | `AWS_ENDPOINT_URL` |

This is a fixed mapping, not a generic prefix system. Explicit is better here -- we don't want arbitrary config keys overridable via env vars.

### Cargo.toml change

```toml
toml = "0.8"
```

One new dependency. No feature flags needed.
