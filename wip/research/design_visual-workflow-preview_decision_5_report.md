# Decision 5: Unified export CLI structure

## Problem

The `koto template export` command has four flags (`--format`, `--output`, `--open`, `--check`) with conditional validity rules. `--open` only makes sense with `--format html`. `--output` is required for html but optional for mermaid. `--check` requires `--output` and conflicts with `--open`. How should the CLI enforce these constraints?

## Alternatives

### a. clap attribute validation

Use clap's derive-macro attributes to encode constraints declaratively:

```rust
#[derive(clap::Args)]
pub struct ExportArgs {
    /// Path to template source (.md) or compiled template (.json)
    pub input: String,

    /// Output format
    #[arg(long, default_value = "mermaid", value_enum)]
    pub format: ExportFormat,

    /// Output file path (required for html, optional for mermaid)
    #[arg(long)]
    pub output: Option<String>,

    /// Open in default browser (html only)
    #[arg(long, requires_if("html", "format"), conflicts_with = "check")]
    pub open: bool,

    /// Check freshness without writing
    #[arg(long, requires = "output")]
    pub check: bool,
}
```

**Pros:**
- Zero validation code in the handler; clap rejects bad combos before `run()` executes.
- Help text auto-generates constraint hints (e.g., "[requires: --output]").
- Follows clap's intended usage pattern.

**Cons:**
- clap's `requires_if` and conditional `required_if_eq` attributes don't compose well for "required when another flag equals a specific value" -- the attribute `required_if_eq("format", "html")` on `output` is the right shape, but error messages read like clap internals rather than domain language ("the following required arguments were not provided: --output" without saying *why*).
- The `--open requires --format html` constraint is awkward. `requires_if` checks "if *this* flag is set, *that* flag must have value X" but the actual constraint is the inverse: "if *that* flag doesn't have value X, *this* flag must not be set." clap has no clean `prohibited_unless_eq` attribute. You'd need a `conflicts_with` that targets a specific enum variant, which clap's derive macros don't support.
- Testing constraint logic requires spawning the CLI or manually calling `App::try_parse_from`, which is heavier than unit-testing a pure function.

**Feasibility verdict:** Partially feasible. The `--check` requires `--output` and `--check` conflicts with `--open` constraints map cleanly. The format-conditional constraints (`--output` required for html, `--open` only with html) don't fit clap's attribute model without workarounds like custom validation logic anyway, which defeats the purpose.

### b. Post-parse validation

Parse all flags permissively, then validate in the handler:

```rust
#[derive(clap::Args)]
pub struct ExportArgs {
    /// Path to template source (.md) or compiled template (.json)
    pub input: String,

    /// Output format: mermaid (default) or html
    #[arg(long, default_value = "mermaid", value_enum)]
    pub format: ExportFormat,

    /// Write output to file (required for html)
    #[arg(long)]
    pub output: Option<String>,

    /// Open generated file in browser (html only)
    #[arg(long)]
    pub open: bool,

    /// Verify output matches existing file without writing
    #[arg(long)]
    pub check: bool,
}

fn validate_export_flags(args: &ExportArgs) -> Result<(), String> {
    if args.format == ExportFormat::Html && args.output.is_none() {
        return Err("--format html requires --output <path>".into());
    }
    if args.open && args.format != ExportFormat::Html {
        return Err("--open is only valid with --format html".into());
    }
    if args.open && args.check {
        return Err("--open and --check are mutually exclusive".into());
    }
    if args.check && args.output.is_none() {
        return Err("--check requires --output <path>".into());
    }
    Ok(())
}
```

**Pros:**
- Error messages are exact domain sentences, not clap-generated phrasing.
- All four rules live in one function that's trivially unit-testable with constructed `ExportArgs` values.
- Adding a new format means adding cases to `validate_export_flags` -- straightforward and local.
- Help text stays clean: each flag's doc stands alone without constraint annotations.
- Matches the existing koto pattern. The CLI already uses post-parse validation (see `resolve_variables` in `mod.rs` lines 189-249, which validates `--var` flags after parsing).

**Cons:**
- `--help` doesn't surface constraint relationships. Users discover them by getting errors. This is mitigated by the error messages being actionable.
- Slightly more code than clap attributes (a 15-line function), but the code is readable and testable.

### c. Format-specific subcommands

Split by format: `koto template export mermaid` and `koto template export html`:

```rust
#[derive(Subcommand)]
pub enum ExportSubcommand {
    /// Export as Mermaid stateDiagram-v2
    Mermaid {
        input: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        check: bool,
    },
    /// Export as interactive HTML
    Html {
        input: String,
        #[arg(long)]
        output: String,  // required, not Option
        #[arg(long)]
        open: bool,
        #[arg(long, conflicts_with = "open")]
        check: bool,
    },
}
```

**Pros:**
- Eliminates cross-flag validation entirely. `--output` is required on `Html` by type. `--open` doesn't exist on `Mermaid`. The only remaining constraint (`--check` conflicts with `--open`) maps to a single clap attribute.
- Help text per subcommand is focused: `koto template export html --help` shows only html-relevant flags.
- Adding a new format is adding a new variant with its own flag set.

**Cons:**
- Deeper nesting: `koto template export html --output foo.html` vs `koto template export --format html --output foo.html`. The subcommand form is 1 token longer and less conventional for format selection (tools like `pandoc`, `ffmpeg`, `cargo fmt` use `--format` or `--emit`, not subcommands).
- The PRD explicitly says "single export command with format selection via --format flag" (R1). Subcommands would deviate from the spec.
- `--check` logic and input handling are identical across subcommands -- you'd either duplicate them or extract shared args into a separate struct, adding structural overhead.
- The `TemplateSubcommand` enum gains a nested subcommand (`Export` containing `ExportSubcommand`), making the command tree three levels deep: `koto -> template -> export -> html`. The existing codebase has at most two levels of subcommands.

## Recommendation

**Option (b): Post-parse validation.**

The deciding factors:

1. **Consistency with existing patterns.** The codebase already validates flag combinations after parsing (`resolve_variables` validates `--var` flags, the `next` handler validates `--to` and `--with-data` interactions). Post-parse validation is the established pattern.

2. **Error message quality.** The PRD explicitly requires "clear errors" for invalid combinations (R15) and "actionable error messages" (R11). Post-parse validation lets us write exact sentences like `--format html requires --output <path>`. clap-generated messages are correct but generic.

3. **PRD alignment.** The PRD specifies `--format mermaid|html` as a flag, not subcommands. Option (c) diverges from the spec for marginal structural benefit.

4. **Testability.** A pure `validate_export_flags(&ExportArgs) -> Result<(), String>` function is trivially unit-tested with four or five constructed inputs. No need to spawn processes or parse command lines.

Option (a) could handle two of the four constraints but needs custom validation for the other two anyway, resulting in split validation logic that's harder to reason about than a single function.

## Key interfaces (Rust types/signatures)

```rust
/// Output format for template export.
#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum ExportFormat {
    Mermaid,
    Html,
}

/// Arguments for `koto template export`.
#[derive(clap::Args)]
pub struct ExportArgs {
    /// Path to template source (.md) or compiled template (.json)
    pub input: String,

    /// Output format
    #[arg(long, default_value = "mermaid", value_enum)]
    pub format: ExportFormat,

    /// Write output to file path (required for html format)
    #[arg(long)]
    pub output: Option<String>,

    /// Open generated file in default browser (html format only)
    #[arg(long)]
    pub open: bool,

    /// Verify existing file matches what would be generated, without writing
    #[arg(long)]
    pub check: bool,
}

/// Validate flag combinations for the export command.
/// Returns Ok(()) or an error message describing the invalid combination.
fn validate_export_flags(args: &ExportArgs) -> Result<(), String>;

// TemplateSubcommand gains a new variant:
#[derive(Subcommand)]
pub enum TemplateSubcommand {
    Compile { source: String },
    Validate { path: String },
    /// Export a template as a visual diagram
    Export(ExportArgs),
}
```

The handler in `run()` would call `validate_export_flags(&args)` early and exit with a clear error (exit code 2, caller error) before doing any template loading or generation work.
