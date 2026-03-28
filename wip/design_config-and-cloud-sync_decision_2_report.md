# Decision 2: Config CLI command structure

## Question

What's the command structure for `koto config get/set`?

## Chosen: Option 2 -- git-style get/set with `list`

### Command surface

| Command | Description |
|---------|-------------|
| `koto config get <key>` | Print raw value to stdout (exit 1 if unset) |
| `koto config set <key> <value>` | Write to user config; `--project` writes to project config |
| `koto config unset <key>` | Remove key; `--project` targets project config |
| `koto config list` | Dump resolved config as TOML; `--json` for machine-readable |

Keys use dotted paths matching TOML table structure (e.g., `session.backend`, `sync.bucket`).

### CLI definition (clap derive)

```rust
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Read a config value (raw, to stdout)
    Get {
        /// Dotted key path (e.g. session.backend)
        key: String,
    },
    /// Write a config value
    Set {
        /// Dotted key path
        key: String,
        /// Value to set
        value: String,
        /// Write to project config instead of user config
        #[arg(long)]
        project: bool,
    },
    /// Remove a config key
    Unset {
        /// Dotted key path
        key: String,
        /// Remove from project config instead of user config
        #[arg(long)]
        project: bool,
    },
    /// Show resolved configuration
    List {
        /// Output as JSON instead of TOML
        #[arg(long)]
        json: bool,
    },
}
```

This slots into the existing `Command` enum as:

```rust
/// Configuration management
Config {
    #[command(subcommand)]
    subcommand: ConfigCommand,
},
```

### Why this fits koto's patterns

The codebase already uses nested subcommand enums for `Session`, `Context`, `Template`, and `Decisions`. Each uses positional args for required values and `--long` flags for options. `ConfigCommand` follows the same shape: positional `key`/`value`, optional `--project` and `--json` flags.

### Output behavior

- `get`: prints the raw value with a trailing newline. No quoting, no key echo. Exit 0 on success, exit 1 if the key doesn't exist. This lets callers use `backend=$(koto config get session.backend)` directly.
- `set`/`unset`: silent on success (exit 0), error message to stderr on failure.
- `list`: prints the fully resolved config (user merged over defaults, project merged over user). Default format is TOML for human readability; `--json` for scripting.

## Rejected options

### Option 1: get/set without `list`

Functionally identical but missing `list`. Omitting `list` forces users to know every key name up front. There's no way to inspect what's configured or debug precedence issues. Adding `list` costs one small match arm and provides significant debugging value. No reason to leave it out.

### Option 3: Interactive config wizard

Incompatible with agent callers. Koto's primary consumers are AI coding agents running non-interactively. An interactive wizard would require stdin, which agents can't provide. The PRD itself specifies `get/set` as the interface. This option contradicts both the spec and koto's design philosophy.

## Assumptions

- Decision 1 (config file format) will use TOML, so dotted key paths map naturally to TOML table paths.
- Decision 3 (credential handling) may restrict which keys `set` accepts in project config. The `--project` flag provides the hook for that enforcement without changing this command structure.
- `list` resolves all layers (defaults, user, project, env overrides) so the output reflects what the engine actually sees at runtime.
