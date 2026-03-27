# Decision 9: Content CLI Interface

## Question
What's the command structure and flag design for koto's context commands?

## Decision: Option 2 -- Positional key argument

Use `koto context <subcommand> <session> <key>` with positional arguments for both session and key, matching the existing pattern where the primary entity is always the first positional arg.

### Chosen Interface

```
koto context add <session> <key> [--from-file <path>]   # reads stdin if no --from-file
koto context get <session> <key> [--to-file <path>]     # writes stdout if no --to-file
koto context exists <session> <key>                     # exit 0 if present, exit 1 if not
koto context list <session>                             # JSON array of keys to stdout
```

## Rationale

**Positional key fits existing patterns.** Every koto subcommand that operates on a named entity uses a positional argument: `koto next <name>`, `koto cancel <name>`, `koto session dir <name>`, `koto template compile <source>`. Making `<key>` a `--key` flag would break this convention. The context key is the primary target of the operation, not a modifier -- it belongs in positional position.

**Subcommand group is the right nesting level.** `koto session` and `koto template` both use subcommand groups for related operations. `koto context` follows the same pattern. The clap enum structure (`ContextCommand` with `Add`, `Get`, `Exists`, `List` variants) maps directly.

**Session-implicit (Option 3) rejected for multi-agent.** Inferring session from the active workflow works for `koto next` because there's typically one workflow per working directory. But the PRD explicitly calls out multi-agent concurrent submission -- agents in different processes may target different sessions. Requiring the session name keeps commands unambiguous and stateless. If single-session convenience matters later, a `--session` flag with env-var default (`KOTO_SESSION`) can be added without breaking the positional interface.

**Option 1's `--key` flag is verbose without benefit.** Typing `koto context get my-workflow plan --to-file plan.md` is clearer and shorter than `koto context get my-workflow --key plan --to-file plan.md`. The key is always required, never optional -- flags should represent optional modifiers, not mandatory targets.

### stdin/stdout Default Behavior

- `koto context add`: reads value from stdin by default. `--from-file <path>` reads from a file instead. This supports pipe-based workflows (`echo '{}' | koto context add sess plan`) and file-based submission equally.
- `koto context get`: writes to stdout by default. `--to-file <path>` writes to a file instead. Supports both `koto context get sess plan > plan.md` and `koto context get sess plan --to-file plan.md`.
- No `--from-stdin` flag needed -- stdin is the default when `--from-file` is absent.

### Exit Codes

- `koto context exists`: exit 0 if key exists, exit 1 if not. No stdout output. This follows standard Unix convention (like `test -f`).
- `koto context list`: exit 0 with JSON array on stdout. Empty array `[]` if no keys.
- Error conditions (session not found, backend failure) use exit code 3 (infrastructure) with JSON error on stdout, consistent with existing error handling.

### Clap Structure

```rust
#[derive(Subcommand)]
pub enum ContextCommand {
    Add {
        /// Session name
        session: String,
        /// Content key
        key: String,
        /// Read content from file instead of stdin
        #[arg(long)]
        from_file: Option<String>,
    },
    Get {
        /// Session name
        session: String,
        /// Content key
        key: String,
        /// Write content to file instead of stdout
        #[arg(long)]
        to_file: Option<String>,
    },
    Exists {
        /// Session name
        session: String,
        /// Content key
        key: String,
    },
    List {
        /// Session name
        session: String,
    },
}
```

## Rejected Options

### Option 1: `--key` as flag
Adds verbosity without benefit. The key is mandatory for `add`, `get`, and `exists` -- mandatory values belong as positional args in koto's existing pattern. Would be the only koto command where the primary target is a flag rather than a positional arg.

### Option 3: Session-implicit
Breaks multi-agent scenarios where different agents target different sessions from the same working directory. Would require either ambient state (a "current session" concept that doesn't exist today) or env-var coupling. The explicit session arg keeps commands pure and testable. Can be layered on later as syntactic sugar without changing the underlying command structure.

## Assumptions

- Content keys are simple strings (format validated per Decision 12, not this decision).
- The session name always matches the workflow name passed to `koto init`.
- stdin reading for `add` uses read-to-EOF, so callers must close the pipe. A size limit (matching the existing 1 MB `MAX_WITH_DATA_BYTES`) applies regardless of input source.

## Confidence
High. The positional pattern is well-established across all existing koto commands, and the subcommand group matches `session` and `template` precedent exactly.
