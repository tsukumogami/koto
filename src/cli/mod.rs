pub mod batch_error;
pub mod context;
pub mod init_child;
pub mod next;
pub mod next_types;
pub mod overrides;
pub mod session;
pub mod task_spawn_error;
pub mod vars;

pub use init_child::{init_child_from_parent, TemplateCompileCache};
pub use task_spawn_error::{SpawnErrorKind, TaskSpawnError};

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::engine::substitute::validate_value;
use crate::template::types::VariableDecl;

use crate::buildinfo;
use crate::cache::{compile_cached, sha256_hex};
use crate::discover::find_workflows_with_metadata;
use crate::engine::errors::EngineError;
use crate::engine::persistence::{derive_decisions, derive_machine_state, derive_state_from_log};
use crate::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use crate::session::context::ContextStore;
use crate::session::local::LocalBackend;
use crate::session::{Backend, SessionBackend, SessionError};
use crate::template::types::CompiledTemplate;

/// Maximum payload size for --with-data (1 MB).
pub(super) const MAX_WITH_DATA_BYTES: usize = 1_048_576;

/// Maximum size of captured stdout/stderr from action execution (64 KB).
const MAX_ACTION_OUTPUT_BYTES: usize = 64 * 1024;

/// Exit code space:
/// - 0: success
/// - 1: transient / retryable errors (gate_blocked, integration_unavailable, engine errors)
/// - 2: caller errors (invalid_submission, precondition_failed, terminal_state, etc.)
/// - 3: infrastructure / config errors (corrupted state, template hash mismatch, parse failures)
///
/// `NextErrorCode::exit_code()` handles codes 1 and 2 for domain errors.
/// `exit_code_for_engine_error()` and this constant handle code 3.
pub(super) const EXIT_INFRASTRUCTURE: i32 = 3;

#[derive(Parser)]
#[command(
    name = "koto",
    about = "Workflow orchestration engine for AI coding agents"
)]
pub struct App {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print version information
    Version {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Initialize a new workflow from a template
    Init {
        /// Workflow name
        name: String,

        /// Path to template file
        #[arg(long)]
        template: String,

        /// Set a template variable (repeatable)
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Name of an existing parent workflow (creates a child workflow)
        #[arg(long)]
        parent: Option<String>,
    },

    /// Get the current state directive for a workflow
    Next {
        /// Workflow name
        name: String,

        /// Submit evidence as JSON (validated against accepts schema)
        #[arg(long = "with-data")]
        with_data: Option<String>,

        /// Directed transition to a named state
        #[arg(long)]
        to: Option<String>,

        /// Skip session cleanup when reaching a terminal state (useful for debugging)
        #[arg(long)]
        no_cleanup: bool,

        /// Always include the details field in the response, regardless of visit count
        #[arg(long)]
        full: bool,
    },

    /// Cancel a workflow, preventing further advancement
    Cancel {
        /// Workflow name
        name: String,
    },

    /// Roll back the workflow to the previous state
    Rewind {
        /// Workflow name
        name: String,
    },

    /// List all active workflows in the current directory
    Workflows {
        /// Show only root workflows (no parent)
        #[arg(long, group = "filter")]
        roots: bool,

        /// Show only children of the named workflow
        #[arg(long, group = "filter", value_name = "NAME")]
        children: Option<String>,

        /// Show only orphaned workflows whose parent no longer exists
        #[arg(long, group = "filter")]
        orphaned: bool,
    },

    /// Template management subcommands
    Template {
        #[command(subcommand)]
        subcommand: TemplateSubcommand,
    },

    /// Session management subcommands
    Session {
        #[command(subcommand)]
        subcommand: SessionCommand,
    },

    /// Content context management subcommands
    Context {
        #[command(subcommand)]
        subcommand: ContextCommand,
    },

    /// Show the current status of a workflow (read-only, no state changes)
    Status {
        /// Workflow name
        name: String,
    },

    /// Decision recording and retrieval
    Decisions {
        #[command(subcommand)]
        subcommand: DecisionsSubcommand,
    },

    /// Gate override recording and retrieval
    Overrides {
        #[command(subcommand)]
        subcommand: overrides::OverridesSubcommand,
    },

    /// Configuration management subcommands
    Config {
        #[command(subcommand)]
        subcommand: ConfigCommand,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Print the value of a config key
    Get {
        /// Dotted key path (e.g., session.backend)
        key: String,
    },
    /// Set a config key to a value
    Set {
        /// Dotted key path (e.g., session.backend)
        key: String,
        /// Value to set
        value: String,
        /// Write to user config (~/.koto/config.toml) instead of project config
        #[arg(long)]
        user: bool,
    },
    /// Remove a config key
    Unset {
        /// Dotted key path (e.g., session.backend)
        key: String,
        /// Remove from user config (~/.koto/config.toml) instead of project config
        #[arg(long)]
        user: bool,
    },
    /// List resolved configuration
    List {
        /// Output as JSON instead of TOML
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SessionCommand {
    /// Print the absolute session directory path
    Dir {
        /// Session name
        name: String,
    },
    /// List all sessions as JSON
    List,
    /// Remove a session directory (idempotent)
    Cleanup {
        /// Session name
        name: String,
    },
    /// Resolve a session version conflict
    Resolve {
        /// Session name
        name: String,
        /// Which version to keep: "local" or "remote"
        #[arg(long)]
        keep: String,
    },
}

#[derive(Subcommand)]
pub enum ContextCommand {
    /// Store content under a key (reads from stdin or --from-file)
    Add {
        /// Session name
        session: String,
        /// Context key (hierarchical, e.g. "scope.md" or "research/r1/lead.md")
        key: String,
        /// Read content from file instead of stdin
        #[arg(long)]
        from_file: Option<String>,
    },
    /// Retrieve stored content (writes to stdout or --to-file)
    Get {
        /// Session name
        session: String,
        /// Context key
        key: String,
        /// Write content to file instead of stdout
        #[arg(long)]
        to_file: Option<String>,
    },
    /// Check if a key exists (exit 0 if present, 1 if not)
    Exists {
        /// Session name
        session: String,
        /// Context key
        key: String,
    },
    /// List all keys as a JSON array
    List {
        /// Session name
        session: String,
        /// Filter keys by prefix
        #[arg(long)]
        prefix: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum ExportFormat {
    Mermaid,
    Html,
}

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

    /// Verify existing file matches what would be generated
    #[arg(long)]
    pub check: bool,
}

/// Validate export flag combinations (R15).
///
/// Returns `Ok(())` if flags are compatible, or an error describing the
/// invalid combination.
fn validate_export_flags(args: &ExportArgs) -> std::result::Result<(), String> {
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

/// Resolve a source argument to a CompiledTemplate.
///
/// Accepts either a `.md` source file (compiled via `compile_cached`) or a
/// `.json` pre-compiled template (loaded directly).
fn resolve_template(source: &str) -> anyhow::Result<CompiledTemplate> {
    let path = Path::new(source);
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => load_compiled_template(source),
        _ => {
            // Treat as markdown source: compile and load.
            let (cache_path, _hash) = compile_cached(path, false)?;
            let cache_path_str = cache_path.to_string_lossy().to_string();
            load_compiled_template(&cache_path_str)
        }
    }
}

#[derive(Subcommand)]
pub enum TemplateSubcommand {
    /// Compile a YAML template source to FormatVersion=1 JSON
    Compile {
        /// Path to the YAML template source
        source: String,
        /// Allow templates with gates that have no gates.* when-clause routing.
        ///
        /// This flag is transitory and will be removed once legacy templates
        /// have migrated to structured gate routing.
        // TODO: remove once shirabe work-on template migrates to gates.* routing
        #[arg(long)]
        allow_legacy_gates: bool,
    },

    /// Validate a compiled template JSON file
    Validate {
        /// Path to the compiled template JSON
        path: String,
    },

    /// Export a template as a visual diagram
    Export(ExportArgs),
}

#[derive(Subcommand)]
pub enum DecisionsSubcommand {
    /// Record a structured decision without advancing state
    Record {
        /// Workflow name
        name: String,
        /// Decision data as JSON (must include "choice" and "rationale" fields)
        #[arg(long = "with-data")]
        with_data: String,
    },
    /// List accumulated decisions for the current state
    List {
        /// Workflow name
        name: String,
    },
}

/// Read and validate a compiled template JSON file.
fn validate_compiled_template(path: &str) -> anyhow::Result<()> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("failed to read file: {}", e))?;
    let template: CompiledTemplate =
        serde_json::from_str(&content).map_err(|e| anyhow::anyhow!("invalid JSON: {}", e))?;
    template
        .validate(true)
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Load a compiled template from a cache path.
fn load_compiled_template(path: &str) -> anyhow::Result<CompiledTemplate> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read template {}: {}", path, e))?;
    serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse template {}: {}", path, e))
}

/// Print a JSON error and exit with the given code.
fn exit_with_error(error: serde_json::Value) -> ! {
    exit_with_error_code(error, 1)
}

/// Print a JSON error and exit with a specific exit code.
pub(super) fn exit_with_error_code(error: serde_json::Value, code: i32) -> ! {
    println!("{}", serde_json::to_string(&error).unwrap_or_default());
    std::process::exit(code);
}

/// Determine the exit code for an engine error by downcasting to EngineError.
///
/// Returns exit code 3 for corrupted state files, and exit code 1 for all
/// other errors.
pub(super) fn exit_code_for_engine_error(err: &anyhow::Error) -> i32 {
    match err.downcast_ref::<EngineError>() {
        Some(EngineError::StateFileCorrupted(_)) => EXIT_INFRASTRUCTURE,
        _ => 1,
    }
}

/// Construct the session backend based on configuration.
///
/// Reads `session.backend` from the merged config and dispatches:
/// - `"local"` -> `LocalBackend` (default)
/// - `"cloud"` without the `cloud` feature -> error with install hint
/// - `"cloud"` with the `cloud` feature -> stub (CloudBackend not yet implemented)
/// - anything else -> error
///
/// When `KOTO_SESSIONS_BASE` is set, the local backend stores sessions directly
/// under that directory (bypassing repo-id hashing). This is intended for
/// integration tests that need to control the storage location.
fn build_backend() -> Result<Backend> {
    let config = crate::config::resolve::load_config()?;

    match config.session.backend.as_str() {
        "local" => Ok(Backend::Local(build_local_backend()?)),
        "cloud" => {
            let working_dir = std::env::current_dir()?;
            let cloud_backend =
                crate::session::cloud::CloudBackend::new(&working_dir, &config.session.cloud)?;
            Ok(Backend::Cloud(cloud_backend))
        }
        other => {
            anyhow::bail!("unknown backend: {other}")
        }
    }
}

/// Build the local backend, honoring `KOTO_SESSIONS_BASE` for testing.
fn build_local_backend() -> Result<LocalBackend> {
    if let Ok(base) = std::env::var("KOTO_SESSIONS_BASE") {
        Ok(LocalBackend::with_base_dir(PathBuf::from(base)))
    } else {
        let working_dir = std::env::current_dir()?;
        LocalBackend::new(&working_dir)
    }
}

/// Validate and resolve `--var KEY=VALUE` arguments against the template's
/// variable declarations. Returns a map of resolved variable bindings ready
/// for storage in the WorkflowInitialized event.
pub(crate) fn resolve_variables(
    raw_vars: &[String],
    declarations: &BTreeMap<String, VariableDecl>,
) -> std::result::Result<HashMap<String, String>, String> {
    let mut provided: HashMap<String, String> = HashMap::new();

    // 1. Parse each --var string and reject duplicates.
    for entry in raw_vars {
        let eq_pos = entry
            .find('=')
            .ok_or_else(|| format!("invalid --var format {:?}: expected KEY=VALUE", entry))?;
        let key = &entry[..eq_pos];
        let value = &entry[eq_pos + 1..];

        if key.is_empty() {
            return Err(format!(
                "invalid --var format {:?}: key must not be empty",
                entry
            ));
        }

        if provided.contains_key(key) {
            return Err(format!("duplicate --var key {:?}", key));
        }

        // Reject keys not declared in the template.
        if !declarations.contains_key(key) {
            return Err(format!(
                "unknown variable {:?}: not declared in template",
                key
            ));
        }

        provided.insert(key.to_string(), value.to_string());
    }

    // 2. Resolve all declared variables: --var value > default > error if required.
    let mut resolved: HashMap<String, String> = HashMap::new();
    for (key, decl) in declarations {
        let value = if let Some(v) = provided.get(key) {
            v.clone()
        } else if !decl.default.is_empty() {
            decl.default.clone()
        } else if decl.required {
            return Err(format!(
                "missing required variable {:?}: provide --var {}=VALUE",
                key, key
            ));
        } else {
            // Not required, no default, not provided -- skip.
            continue;
        };

        // 3. Validate the value against the allowlist.
        validate_value(key, &value).map_err(|e| e.to_string())?;

        resolved.insert(key.clone(), value);
    }

    Ok(resolved)
}

/// Truncate a string to at most `max_bytes` bytes. If truncated, appends a note.
/// Handles UTF-8 correctly by truncating at a char boundary.
fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Find the largest char boundary at or before max_bytes.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = s[..end].to_string();
    truncated.push_str("\n... [output truncated]");
    truncated
}

/// Check whether a parsed evidence JSON object contains the reserved "gates" key.
///
/// Returns `true` when the value is a JSON object with a top-level "gates" key.
/// Returns `false` for non-object values (the evidence schema validator will
/// catch those separately).
fn evidence_has_reserved_gates_key(data: &serde_json::Value) -> bool {
    data.as_object()
        .map(|o| o.contains_key("gates"))
        .unwrap_or(false)
}

/// Parse a `--with-data` JSON string and check for the reserved `"gates"` key.
///
/// Returns the parsed `Value` on success, or a `NextError` with code
/// `InvalidSubmission` when the JSON is malformed or contains the reserved key.
/// This function contains only pure logic; callers are responsible for exiting.
#[cfg(unix)]
fn validate_with_data_payload(
    data_str: &str,
) -> Result<serde_json::Value, crate::cli::next_types::NextError> {
    use crate::cli::next_types::{NextError, NextErrorCode};

    let data: serde_json::Value = serde_json::from_str(data_str).map_err(|e| NextError {
        code: NextErrorCode::InvalidSubmission,
        message: format!("invalid JSON in --with-data: {}", e),
        details: vec![],
    })?;

    if evidence_has_reserved_gates_key(&data) {
        return Err(NextError {
            code: NextErrorCode::InvalidSubmission,
            message: r#""gates" is a reserved field; agent submissions must not include this key"#
                .to_string(),
            details: vec![],
        });
    }

    Ok(data)
}

/// Execute a command with polling: run repeatedly, evaluate gates after each
/// execution, and return when all gates pass or the timeout expires.
///
/// The `gates` map and `evaluate_gates_fn` allow gate evaluation within the
/// polling loop. For non-polling actions, gates are evaluated by the advance
/// loop after the action closure returns. For polling actions, gates must be
/// checked inside the loop so we know when to stop retrying.
#[cfg(unix)]
fn execute_with_polling<G>(
    command: &str,
    working_dir: &std::path::Path,
    polling: &crate::template::types::PollingConfig,
    gates: &std::collections::BTreeMap<String, crate::template::types::Gate>,
    evaluate_gates_fn: &G,
    shutdown: &std::sync::atomic::AtomicBool,
) -> crate::action::CommandOutput
where
    G: Fn(
        &std::collections::BTreeMap<String, crate::template::types::Gate>,
    ) -> std::collections::BTreeMap<String, crate::gate::StructuredGateResult>,
{
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(u64::from(polling.timeout_secs));

    loop {
        if shutdown.load(Ordering::Relaxed) {
            return crate::action::CommandOutput {
                exit_code: -1,
                stdout: String::new(),
                stderr: "polling interrupted by signal".to_string(),
            };
        }

        let output = crate::action::run_shell_command(command, working_dir, 30);

        // Check gates after each command execution.
        if !gates.is_empty() {
            let gate_results = evaluate_gates_fn(gates);
            let all_passed = gate_results
                .values()
                .all(|r| matches!(r.outcome, crate::gate::GateOutcome::Passed));
            if all_passed {
                return output;
            }
        } else if output.exit_code == 0 {
            // No gates: succeed on exit code 0.
            return output;
        }

        if Instant::now() >= deadline {
            return crate::action::CommandOutput {
                exit_code: output.exit_code,
                stdout: output.stdout,
                stderr: format!(
                    "{}\npolling timed out after {} seconds",
                    output.stderr, polling.timeout_secs
                ),
            };
        }

        // Sleep for the interval, but check shutdown more frequently.
        let sleep_end = Instant::now() + Duration::from_secs(u64::from(polling.interval_secs));
        while Instant::now() < sleep_end {
            if shutdown.load(Ordering::Relaxed) {
                return crate::action::CommandOutput {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "polling interrupted by signal".to_string(),
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn run(app: App) -> Result<()> {
    match app.command {
        Command::Version { json } => {
            let info = buildinfo::build_info();
            if json {
                println!("{}", serde_json::to_string(&info)?);
            } else {
                println!("koto {} ({} {})", info.version, info.commit, info.built_at);
            }
            Ok(())
        }
        Command::Init {
            name,
            template,
            vars,
            parent,
        } => {
            let backend = build_backend()?;
            handle_init(&backend, &name, &template, &vars, parent.as_deref())
        }
        Command::Next {
            name,
            with_data,
            to,
            no_cleanup,
            full,
        } => {
            let backend = build_backend()?;
            let context_store: &dyn ContextStore = &backend;
            handle_next(
                &backend,
                context_store,
                name,
                with_data,
                to,
                no_cleanup,
                full,
            )
        }
        Command::Cancel { name } => {
            let backend = build_backend()?;
            handle_cancel(&backend, &name)
        }
        Command::Rewind { name } => {
            let backend = build_backend()?;
            handle_rewind(&backend, &name)
        }
        Command::Status { name } => {
            let backend = build_backend()?;
            handle_status(&backend, &name)
        }
        Command::Workflows {
            roots,
            children,
            orphaned,
        } => {
            let backend = build_backend()?;
            let metadata = match find_workflows_with_metadata(&backend) {
                Ok(m) => m,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": e.to_string(),
                        "command": "workflows"
                    }));
                }
            };
            let filtered: Vec<_> = if roots {
                metadata
                    .into_iter()
                    .filter(|wf| wf.parent_workflow.is_none())
                    .collect()
            } else if let Some(ref parent_name) = children {
                metadata
                    .into_iter()
                    .filter(|wf| wf.parent_workflow.as_deref() == Some(parent_name.as_str()))
                    .collect()
            } else if orphaned {
                metadata
                    .into_iter()
                    .filter(|wf| match &wf.parent_workflow {
                        Some(parent) => !backend.exists(parent),
                        None => false,
                    })
                    .collect()
            } else {
                metadata
            };
            println!("{}", serde_json::to_string(&filtered)?);
            Ok(())
        }
        Command::Session { subcommand } => {
            let backend = build_backend()?;
            match subcommand {
                SessionCommand::Dir { name } => session::handle_dir(&backend, &name),
                SessionCommand::List => session::handle_list(&backend),
                SessionCommand::Cleanup { name } => {
                    // Query children before cleanup so the parent is still visible.
                    let children = query_children(&backend, &name);
                    session::handle_cleanup(&backend, &name)?;
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "name": name,
                            "cleaned_up": true,
                            "children": children
                        }))?
                    );
                    Ok(())
                }
                SessionCommand::Resolve { name, keep } => {
                    session::handle_resolve(&backend, &name, &keep)
                }
            }
        }
        Command::Context { subcommand } => {
            let backend = build_backend()?;
            let store: &dyn ContextStore = &backend;
            match subcommand {
                ContextCommand::Add {
                    session,
                    key,
                    from_file,
                } => {
                    if let Err(e) = context::handle_add(store, &session, &key, from_file.as_deref())
                    {
                        exit_with_error_code(
                            serde_json::json!({
                                "error": e.to_string(),
                                "command": "context add"
                            }),
                            EXIT_INFRASTRUCTURE,
                        );
                    }
                    Ok(())
                }
                ContextCommand::Get {
                    session,
                    key,
                    to_file,
                } => {
                    if let Err(e) = context::handle_get(store, &session, &key, to_file.as_deref()) {
                        exit_with_error_code(
                            serde_json::json!({
                                "error": e.to_string(),
                                "command": "context get"
                            }),
                            EXIT_INFRASTRUCTURE,
                        );
                    }
                    Ok(())
                }
                ContextCommand::Exists { session, key } => {
                    if context::handle_exists(store, &session, &key) {
                        std::process::exit(0);
                    } else {
                        std::process::exit(1);
                    }
                }
                ContextCommand::List { session, prefix } => {
                    if let Err(e) = context::handle_list(store, &session, prefix.as_deref()) {
                        exit_with_error_code(
                            serde_json::json!({
                                "error": e.to_string(),
                                "command": "context list"
                            }),
                            EXIT_INFRASTRUCTURE,
                        );
                    }
                    Ok(())
                }
            }
        }
        Command::Template { subcommand } => match subcommand {
            TemplateSubcommand::Compile {
                source,
                allow_legacy_gates,
            } => {
                let source_path = Path::new(&source);
                let strict = !allow_legacy_gates;
                match compile_cached(source_path, strict) {
                    Ok((cache_path, _)) => {
                        println!("{}", cache_path.display());
                        Ok(())
                    }
                    Err(e) => {
                        exit_with_error(serde_json::json!({
                            "error": e.to_string(),
                            "command": "template compile"
                        }));
                    }
                }
            }
            TemplateSubcommand::Validate { path } => {
                if let Err(e) = validate_compiled_template(&path) {
                    exit_with_error(serde_json::json!({
                        "error": e.to_string(),
                        "command": "template validate"
                    }));
                }
                Ok(())
            }
            TemplateSubcommand::Export(args) => {
                if let Err(msg) = validate_export_flags(&args) {
                    eprintln!("error: {}", msg);
                    std::process::exit(2);
                }

                let compiled = match resolve_template(&args.input) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                };

                let output_bytes = match args.format {
                    ExportFormat::Mermaid => {
                        let raw = crate::export::to_mermaid(&compiled);
                        if args.output.is_some() {
                            // Wrap in fenced code block for GitHub rendering.
                            format!("```mermaid\n{}```\n", raw).into_bytes()
                        } else {
                            // Raw mermaid text for stdout composability.
                            raw.into_bytes()
                        }
                    }
                    ExportFormat::Html => crate::export::generate_html(&compiled),
                };

                if args.check {
                    // --check requires --output (enforced by validate_export_flags)
                    let output_path = args.output.as_ref().unwrap();
                    let path = std::path::Path::new(output_path);
                    match crate::export::check_freshness(&output_bytes, path) {
                        Ok(crate::export::CheckResult::Fresh) => Ok(()),
                        Ok(crate::export::CheckResult::Stale) => {
                            let format_name = match args.format {
                                ExportFormat::Mermaid => "mermaid",
                                ExportFormat::Html => "html",
                            };
                            eprintln!("error: {} is out of date", output_path);
                            eprintln!(
                                "run: koto template export {} --format {} --output {}",
                                args.input, format_name, output_path
                            );
                            std::process::exit(1);
                        }
                        Ok(crate::export::CheckResult::Missing) => {
                            let format_name = match args.format {
                                ExportFormat::Mermaid => "mermaid",
                                ExportFormat::Html => "html",
                            };
                            eprintln!("error: {} does not exist", output_path);
                            eprintln!(
                                "run: koto template export {} --format {} --output {}",
                                args.input, format_name, output_path
                            );
                            std::process::exit(1);
                        }
                        Err(e) => {
                            eprintln!("error: failed to check {}: {}", output_path, e);
                            std::process::exit(1);
                        }
                    }
                } else if let Some(ref output_path) = args.output {
                    std::fs::write(output_path, &output_bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write {}: {}", output_path, e))?;
                    println!("{}", output_path);
                    if args.open {
                        opener::open(output_path).map_err(|e| {
                            anyhow::anyhow!("failed to open {}: {}", output_path, e)
                        })?;
                    }
                    Ok(())
                } else {
                    std::io::stdout().write_all(&output_bytes)?;
                    Ok(())
                }
            }
        },
        Command::Decisions { subcommand } => {
            let backend = build_backend()?;
            match subcommand {
                DecisionsSubcommand::Record { name, with_data } => {
                    handle_decisions_record(&backend, name, with_data)
                }
                DecisionsSubcommand::List { name } => handle_decisions_list(&backend, name),
            }
        }
        Command::Overrides { subcommand } => {
            let backend = build_backend()?;
            match subcommand {
                overrides::OverridesSubcommand::Record {
                    name,
                    gate,
                    rationale,
                    with_data,
                } => overrides::handle_overrides_record(&backend, name, gate, rationale, with_data),
                overrides::OverridesSubcommand::List { name } => {
                    overrides::handle_overrides_list(&backend, name)
                }
            }
        }
        Command::Config { subcommand } => handle_config(subcommand),
    }
}

/// Handle `koto config` subcommands.
fn handle_config(subcommand: ConfigCommand) -> Result<()> {
    use crate::config;

    match subcommand {
        ConfigCommand::Get { key } => {
            let resolved = config::resolve::load_config()?;
            match config::get_value(&resolved, &key) {
                Some(value) => {
                    println!("{}", value);
                    Ok(())
                }
                None => {
                    std::process::exit(1);
                }
            }
        }
        ConfigCommand::Set { key, value, user } => {
            if user {
                config::resolve::ensure_koto_dir()?;
                let path = config::resolve::user_config_path()
                    .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
                let mut doc = config::resolve::load_toml_value(&path)?;
                config::set_value_in_toml(&mut doc, &key, &value)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                config::resolve::write_toml_value(&path, &doc)?;
            } else {
                config::validate::validate_project_key(&key)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                let path = config::resolve::project_config_path();
                let mut doc = config::resolve::load_toml_value(&path)?;
                config::set_value_in_toml(&mut doc, &key, &value)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                config::resolve::write_toml_value(&path, &doc)?;
            }
            Ok(())
        }
        ConfigCommand::Unset { key, user } => {
            if user {
                let path = config::resolve::user_config_path()
                    .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
                let mut doc = config::resolve::load_toml_value(&path)?;
                config::unset_value_in_toml(&mut doc, &key)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                config::resolve::write_toml_value(&path, &doc)?;
            } else {
                let path = config::resolve::project_config_path();
                let mut doc = config::resolve::load_toml_value(&path)?;
                config::unset_value_in_toml(&mut doc, &key)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                config::resolve::write_toml_value(&path, &doc)?;
            }
            Ok(())
        }
        ConfigCommand::List { json } => {
            let resolved = config::resolve::load_config()?;
            let redacted = config::redact(&resolved);
            if json {
                println!("{}", serde_json::to_string_pretty(&redacted)?);
            } else {
                println!("{}", toml::to_string_pretty(&redacted)?);
            }
            Ok(())
        }
    }
}

/// Handle the `koto init` command.
fn handle_init(
    backend: &dyn SessionBackend,
    name: &str,
    template: &str,
    vars: &[String],
    parent: Option<&str>,
) -> Result<()> {
    // Validate workflow name before any filesystem operation.
    if let Err(msg) = crate::discover::validate_workflow_name(name) {
        exit_with_error_code(
            serde_json::json!({
                "error": msg,
                "command": "init",
                "allowed_pattern": "^[a-zA-Z0-9][a-zA-Z0-9._-]*$"
            }),
            2,
        );
    }

    // Validate parent workflow exists, if specified.
    if let Some(parent_name) = parent {
        if !backend.exists(parent_name) {
            exit_with_error(serde_json::json!({
                "error": format!("parent workflow '{}' not found", parent_name),
                "command": "init"
            }));
        }
    }

    // Check if session already exists (state file present). This is a
    // best-effort pre-check; the atomic `init_state_file` call below is
    // the authoritative collision detector (it handles concurrent
    // racers where two callers both see `exists() == false`).
    if backend.exists(name) {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' already exists", name),
            "command": "init"
        }));
    }

    let (cache_path, hash) = match compile_cached(Path::new(template), false) {
        Ok(result) => result,
        Err(e) => {
            exit_with_error(serde_json::json!({
                "error": e.to_string(),
                "command": "init"
            }));
        }
    };

    let cache_path_str = cache_path.to_string_lossy().to_string();
    let compiled: CompiledTemplate = match load_compiled_template(&cache_path_str) {
        Ok(t) => t,
        Err(e) => {
            exit_with_error(serde_json::json!({
                "error": e.to_string(),
                "command": "init"
            }));
        }
    };

    // Resolve --var flags against template variable declarations.
    let variables = match resolve_variables(vars, &compiled.variables) {
        Ok(v) => v,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": e,
                    "command": "init"
                }),
                2,
            );
        }
    };

    let initial_state = compiled.initial_state.clone();
    let ts = now_iso8601();

    // Build the atomic bundle: one header + two initial events. The
    // bundle shape matches what the old three-step sequence produced
    // (append_header, then append_event for WorkflowInitialized with
    // seq 1, then append_event for Transitioned with seq 2).
    let header = StateFileHeader {
        schema_version: 1,
        workflow: name.to_string(),
        template_hash: hash,
        created_at: ts.clone(),
        parent_workflow: parent.map(|s| s.to_string()),
    };

    let init_payload = EventPayload::WorkflowInitialized {
        template_path: cache_path_str,
        variables,
    };
    let transition_payload = EventPayload::Transitioned {
        from: None,
        to: initial_state.clone(),
        condition_type: "auto".to_string(),
    };
    let initial_events = vec![
        Event {
            seq: 1,
            timestamp: ts.clone(),
            event_type: init_payload.type_name().to_string(),
            payload: init_payload,
        },
        Event {
            seq: 2,
            timestamp: ts.clone(),
            event_type: transition_payload.type_name().to_string(),
            payload: transition_payload,
        },
    ];

    // One atomic create-or-fail rename commits the header and both
    // events together. A collision (another writer won the race)
    // surfaces the same "already exists" caller-facing error the
    // pre-check above would have emitted.
    match backend.init_state_file(name, header, initial_events) {
        Ok(()) => {}
        Err(SessionError::Collision) => {
            exit_with_error(serde_json::json!({
                "error": format!("workflow '{}' already exists", name),
                "command": "init"
            }));
        }
        Err(e) => {
            exit_with_error(serde_json::json!({
                "error": e.to_string(),
                "command": "init"
            }));
        }
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "state": initial_state
        }))?
    );
    Ok(())
}

/// Handle the `koto rewind` command.
fn handle_rewind(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    if !backend.exists(name) {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' not found", name),
            "command": "rewind"
        }));
    }

    let (_header, events) = match backend.read_events(name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "rewind"
                }),
                code,
            );
        }
    };

    // Find the current state and the state to rewind to.
    let state_changing: Vec<&crate::engine::types::Event> = events
        .iter()
        .filter(|e| {
            matches!(
                e.payload,
                EventPayload::Transitioned { .. }
                    | EventPayload::DirectedTransition { .. }
                    | EventPayload::Rewound { .. }
            )
        })
        .collect();

    if state_changing.len() <= 1 {
        exit_with_error(serde_json::json!({
            "error": "already at initial state, cannot rewind",
            "command": "rewind"
        }));
    }

    let current_state = derive_state_from_log(&events).unwrap_or_default();
    let prev_event = state_changing[state_changing.len() - 2];
    let prev_state = match &prev_event.payload {
        EventPayload::Transitioned { to, .. } => to.clone(),
        EventPayload::DirectedTransition { to, .. } => to.clone(),
        EventPayload::Rewound { to, .. } => to.clone(),
        _ => unreachable!(),
    };

    let rewind_payload = EventPayload::Rewound {
        from: current_state,
        to: prev_state.clone(),
    };

    if let Err(e) = backend.append_event(name, &rewind_payload, &now_iso8601()) {
        exit_with_error(serde_json::json!({
            "error": e.to_string(),
            "command": "rewind"
        }));
    }

    let children = query_children(backend, name);

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "state": prev_state,
            "children": children
        }))?
    );
    Ok(())
}

/// Decide whether a workflow state's entry into `handle_next` must be
/// serialized behind an advisory flock on the parent state file.
///
/// The "batch-scoped parent" concept lives in the batch-child-spawning
/// design (Decision 12): when a parent state materializes children, two
/// concurrent `koto next` ticks on the same parent can race the
/// scheduler's read-decide-write cycle and spawn duplicate children or
/// double-count completions. Such states must serialize ticks; all
/// other states are free to skip the lock so the happy path is
/// unchanged.
///
/// # Current detection rules
///
/// The authoritative signals planned by the design are:
///
/// 1. A `materialize_children` hook on the current state's template
///    (introduced by Issue #7).
/// 2. A `SchedulerRan` or `BatchFinalized` event in the state log
///    (introduced by Issues #16 / #17).
///
/// Neither signal exists in the codebase yet. This helper therefore
/// currently returns `false` in every case, which keeps the lock path
/// cold until the hook machinery lands. Issues #7 / #16 / #17 will
/// extend this function to inspect the relevant fields; call sites in
/// `handle_next` do not need to change when that happens.
///
/// # Parameters
///
/// - `_compiled`: the compiled template for the workflow. When hook
///   metadata lands, implementations will read
///   `_compiled.states[state_name]` to check for the hook.
/// - `_state_name`: the current state name; used as the lookup key in
///   the compiled template.
/// - `_events`: the full event log for the workflow. When batch event
///   types land, implementations will scan this for
///   `SchedulerRan` / `BatchFinalized` entries.
///
/// All parameters are prefixed with `_` today because none of them are
/// inspected yet; they are threaded through now so the helper's
/// signature does not change when the real detection logic lands.
// TODO(#7): inspect template hook metadata once materialize_children
// hooks parse onto CompiledTemplate::states.
// TODO(#16, #17): scan `_events` for SchedulerRan / BatchFinalized
// once those event payloads exist.
#[cfg(unix)]
fn state_is_batch_scoped(
    _compiled: &CompiledTemplate,
    _state_name: &str,
    _events: &[Event],
) -> bool {
    false
}

/// Handle the `koto next` command with full output contract support.
///
/// Flow:
/// 1. Validate flag combinations (--with-data and --to are mutually exclusive)
/// 2. Enforce payload size limit on --with-data
/// 3. Load state file and template
/// 4. If --to: validate target, append directed_transition event, re-derive
///    state, dispatch on new state (single-shot, no advancement loop)
/// 5. If --with-data: validate evidence against accepts schema, append
///    evidence_submitted event
/// 6. If the current state is a batch-scoped parent, acquire an advisory
///    flock on the state file (non-blocking). Non-batch states skip this.
/// 7. Register SIGTERM/SIGINT signal handlers
/// 8. Merge evidence from current epoch
/// 9. Run advancement loop (advance_until_stop)
/// 10. Map StopReason to NextResponse, serialize and exit
///
/// NOTE: This handler uses structured `NextError` for domain errors (per the
/// output contract). Other commands (init, rewind, etc.) use a flat
/// `{"error": "string", "command": "..."}` format. Do not mix the two styles.
/// The batch-scoped lock path produces a third envelope shape --
/// `BatchError::ConcurrentTick` (see `cli::batch_error`) -- which Issue #10
/// will extend with additional variants.
#[cfg(unix)]
fn handle_next(
    backend: &dyn SessionBackend,
    context_store: &dyn ContextStore,
    name: String,
    with_data: Option<String>,
    to: Option<String>,
    no_cleanup: bool,
    full: bool,
) -> Result<()> {
    use crate::cli::next::dispatch_next;
    use crate::cli::next_types::{
        blocking_conditions_from_gates, ErrorDetail, ExpectsSchema, IntegrationOutput,
        IntegrationUnavailableMarker, NextError, NextErrorCode, NextResponse,
    };
    use crate::engine::advance::{
        advance_until_stop, merge_epoch_evidence, ActionResult, AdvanceError, IntegrationError,
        StopReason,
    };
    use crate::engine::evidence::validate_evidence;
    use crate::engine::persistence::{derive_evidence, derive_visit_counts};
    use crate::engine::substitute::Variables;
    use crate::gate::evaluate_gates;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    // 1. Mutual exclusivity check
    if with_data.is_some() && to.is_some() {
        let err = NextError {
            code: NextErrorCode::PreconditionFailed,
            message: "--with-data and --to are mutually exclusive".to_string(),
            details: vec![],
        };
        let json = serde_json::json!({"error": err});
        exit_with_error_code(json, err.code.exit_code());
    }

    // 2. Payload size limit
    if let Some(ref data_str) = with_data {
        if data_str.len() > MAX_WITH_DATA_BYTES {
            let err = NextError {
                code: NextErrorCode::InvalidSubmission,
                message: format!(
                    "--with-data payload exceeds maximum size of {} bytes",
                    MAX_WITH_DATA_BYTES
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }
    }

    // 3. Load state file and template
    let current_dir = std::env::current_dir()?;

    if !backend.exists(&name) {
        let err = NextError {
            code: NextErrorCode::WorkflowNotInitialized,
            message: format!("workflow '{}' not found", name),
            details: vec![],
        };
        let json = serde_json::json!({"error": err});
        exit_with_error_code(json, err.code.exit_code());
    }

    let (header, events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: err.to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };

    if events.is_empty() {
        let ne = NextError {
            code: NextErrorCode::PersistenceError,
            message: "state file has no events".to_string(),
            details: vec![],
        };
        let json = serde_json::json!({"error": ne});
        exit_with_error_code(json, ne.code.exit_code());
    }

    // Check for cancelled workflow before any processing.
    let is_cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
    if is_cancelled {
        let err = NextError {
            code: NextErrorCode::TerminalState,
            message: "workflow has been cancelled".to_string(),
            details: vec![],
        };
        let json = serde_json::json!({"error": err});
        exit_with_error_code(json, err.code.exit_code());
    }

    // Construct variable bindings from the WorkflowInitialized event.
    // Re-validates values as defense in depth; exits with infrastructure error on failure.
    let variables = match Variables::from_events(&events) {
        Ok(v) => v,
        Err(e) => {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: format!("variable re-validation failed: {}", e),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };

    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: "corrupt state file: cannot derive current state".to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };

    // Verify template hash
    let template_bytes = match std::fs::read(&machine_state.template_path) {
        Ok(b) => b,
        Err(e) => {
            let ne = NextError {
                code: NextErrorCode::TemplateError,
                message: format!(
                    "failed to read template {}: {}",
                    machine_state.template_path, e
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };
    let actual_hash = sha256_hex(&template_bytes);
    if actual_hash != machine_state.template_hash {
        let ne = NextError {
            code: NextErrorCode::TemplateError,
            message: format!(
                "template hash mismatch: header says {} but cached template hashes to {}",
                machine_state.template_hash, actual_hash
            ),
            details: vec![],
        };
        let json = serde_json::json!({"error": ne});
        exit_with_error_code(json, ne.code.exit_code());
    }

    let compiled: CompiledTemplate = match serde_json::from_slice(&template_bytes) {
        Ok(t) => t,
        Err(e) => {
            let ne = NextError {
                code: NextErrorCode::TemplateError,
                message: format!(
                    "failed to parse template {}: {}",
                    machine_state.template_path, e
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };

    // Check for reserved variable name collisions in the template.
    for reserved in crate::cli::vars::RESERVED_VARIABLE_NAMES {
        if compiled.variables.contains_key(*reserved) {
            let err = NextError {
                code: NextErrorCode::TemplateError,
                message: format!(
                    "template declares reserved variable '{}'; this name is injected by the runtime and cannot be redefined",
                    reserved
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }
    }

    // Build the runtime variable map for {{SESSION_DIR}} and {{SESSION_NAME}} substitution.
    let mut runtime_vars = std::collections::HashMap::new();
    runtime_vars.insert(
        "SESSION_DIR".to_string(),
        backend.session_dir(&name).to_string_lossy().to_string(),
    );
    runtime_vars.insert("SESSION_NAME".to_string(), name.clone());

    // 4. Handle --to (directed transition) -- single-shot, no advancement loop
    if let Some(ref target) = to {
        let current_state = &machine_state.current_state;

        // Look up the current template state to validate the target.
        let current_template_state = match compiled.states.get(current_state) {
            Some(s) => s,
            None => {
                let ne = NextError {
                    code: NextErrorCode::TemplateError,
                    message: format!("state '{}' not found in template", current_state),
                    details: vec![],
                };
                let json = serde_json::json!({"error": ne});
                exit_with_error_code(json, ne.code.exit_code());
            }
        };

        // Validate target is a valid transition from current state.
        let valid_targets: Vec<&str> = current_template_state
            .transitions
            .iter()
            .map(|t| t.target.as_str())
            .collect();

        if !valid_targets.contains(&target.as_str()) {
            let err = NextError {
                code: NextErrorCode::PreconditionFailed,
                message: format!(
                    "state '{}' does not have a transition to '{}'",
                    current_state, target
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }

        // Validate target state exists in template.
        if !compiled.states.contains_key(target) {
            let ne = NextError {
                code: NextErrorCode::TemplateError,
                message: format!("target state '{}' not found in template", target),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }

        // Append directed_transition event.
        let payload = EventPayload::DirectedTransition {
            from: current_state.clone(),
            to: target.clone(),
        };
        if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: e.to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }

        // Dispatch on the new (target) state, skip gate evaluation.
        let target_template_state = compiled.states.get(target).unwrap();
        let gate_results = std::collections::BTreeMap::new();

        match dispatch_next(target, target_template_state, true, &gate_results) {
            Ok(resp) => {
                let resp = resp.with_substituted_directive(|d| {
                    let d = crate::cli::vars::substitute_vars(d, &runtime_vars);
                    variables.substitute(&d)
                });
                println!("{}", serde_json::to_string(&resp)?);
                // Auto-cleanup after output when reaching a terminal state.
                if matches!(resp, next_types::NextResponse::Terminal { .. }) && !no_cleanup {
                    if let Err(e) = backend.cleanup(&name) {
                        eprintln!("warning: session cleanup failed: {}", e);
                    }
                }
                std::process::exit(0);
            }
            Err(err) => {
                let code = err.code.exit_code();
                let json = serde_json::json!({"error": err});
                exit_with_error_code(json, code);
            }
        }
    }

    // Get current state info for evidence validation.
    let current_state = &machine_state.current_state;
    let template_state = match compiled.states.get(current_state) {
        Some(s) => s,
        None => {
            let ne = NextError {
                code: NextErrorCode::TemplateError,
                message: format!("state '{}' not found in template", current_state),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };

    // 5. Handle --with-data (evidence submission)
    if let Some(ref data_str) = with_data {
        // Check that the current state is not terminal.
        if template_state.terminal {
            let err = NextError {
                code: NextErrorCode::TerminalState,
                message: format!(
                    "cannot submit evidence: state '{}' is terminal",
                    current_state
                ),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }

        // Check that the state has an accepts block.
        let accepts = match &template_state.accepts {
            Some(a) => a,
            None => {
                let err = NextError {
                    code: NextErrorCode::PreconditionFailed,
                    message: format!(
                        "state '{}' does not accept evidence (no accepts block)",
                        current_state
                    ),
                    details: vec![],
                };
                let json = serde_json::json!({"error": err});
                exit_with_error_code(json, err.code.exit_code());
            }
        };

        // Parse the JSON payload and reject the reserved "gates" key.
        let data = match validate_with_data_payload(data_str) {
            Ok(v) => v,
            Err(err) => {
                let json = serde_json::json!({"error": err});
                exit_with_error_code(json, err.code.exit_code());
            }
        };

        // Validate evidence against schema.
        if let Err(validation_err) = validate_evidence(&data, accepts) {
            let details: Vec<ErrorDetail> = validation_err
                .field_errors
                .iter()
                .map(|fe| ErrorDetail {
                    field: fe.field.clone(),
                    reason: fe.reason.clone(),
                })
                .collect();
            let err = NextError {
                code: NextErrorCode::InvalidSubmission,
                message: "evidence validation failed".to_string(),
                details,
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }

        // Append evidence_submitted event.
        let fields: HashMap<String, serde_json::Value> = data
            .as_object()
            .expect("validate_evidence guarantees object input")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let payload = EventPayload::EvidenceSubmitted {
            state: current_state.clone(),
            fields,
        };
        if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: e.to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    }

    // 6. For batch-scoped parents only: acquire an advisory flock on
    // the state file (non-blocking) and hold it for the rest of the
    // tick. The RAII guard released on drop keeps lock lifetime tied
    // to this function's scope.
    //
    // Non-batch workflows intentionally skip the lock. The parent-
    // lock requirement comes from the batch-child-spawning design
    // (Decision 12) where concurrent ticks on a parent state can
    // race the scheduler's read-decide-write cycle. Ordinary
    // workflows never run a scheduler, so no ordering is needed and
    // two concurrent ticks would at worst append-compete at the
    // state file level -- which the engine's existing single-writer
    // semantics handle.
    //
    // On lock contention we translate `SessionError::Locked` into
    // `BatchError::ConcurrentTick` rather than reusing
    // `NextErrorCode::ConcurrentAccess`. The batch envelope lives
    // alongside the existing `NextError` envelope on purpose: Issue
    // #10 will extend `BatchError` with additional batch-specific
    // variants, and agents need to discriminate batch errors from
    // per-state ones. `_batch_lock` is kept alive across the advance
    // loop; its field is intentionally unused.
    let _batch_lock: Option<crate::session::SessionLock> =
        if state_is_batch_scoped(&compiled, current_state, &events) {
            match backend.lock_state_file(&name) {
                Ok(guard) => Some(guard),
                Err(SessionError::Locked { holder_pid }) => {
                    let err = crate::cli::batch_error::BatchError::ConcurrentTick { holder_pid };
                    let code = err.exit_code();
                    exit_with_error_code(err.to_envelope(), code);
                }
                Err(e) => {
                    let ne = NextError {
                        code: NextErrorCode::PersistenceError,
                        message: format!("failed to acquire state file lock: {}", e),
                        details: vec![],
                    };
                    let json = serde_json::json!({"error": ne});
                    exit_with_error_code(json, ne.code.exit_code());
                }
            }
        } else {
            None
        };

    // 7. Register signal handlers for clean shutdown.
    let shutdown = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
    {
        eprintln!("warning: failed to register SIGTERM handler: {}", e);
    }
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
    {
        eprintln!("warning: failed to register SIGINT handler: {}", e);
    }

    // 8. Merge evidence from current epoch.
    // Re-read events to include the evidence_submitted we may have just appended.
    let (_, current_events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let ne = NextError {
                code: NextErrorCode::PersistenceError,
                message: err.to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": ne});
            exit_with_error_code(json, ne.code.exit_code());
        }
    };
    let epoch_events = derive_evidence(&current_events);
    let evidence = merge_epoch_evidence(&epoch_events.into_iter().cloned().collect::<Vec<_>>());

    // 9. Set up I/O closures and run advancement loop.
    let mut append_closure = |payload: &EventPayload| -> Result<(), String> {
        backend
            .append_event(&name, payload, &now_iso8601())
            .map_err(|e| e.to_string())
    };

    // Build the children-complete gate evaluator closure. It captures the
    // session backend to call list() and read_events() for child discovery.
    let workflow_name_for_children = name.clone();
    let children_eval =
        move |gate: &crate::template::types::Gate| -> crate::gate::StructuredGateResult {
            evaluate_children_complete(backend, &workflow_name_for_children, gate)
        };

    let vars_for_gates = runtime_vars.clone();
    let session_name = &name;
    let gate_closure =
        |gates: &std::collections::BTreeMap<String, crate::template::types::Gate>| {
            // Substitute both template and runtime variables in gate command strings.
            let substituted: std::collections::BTreeMap<String, crate::template::types::Gate> =
                gates
                    .iter()
                    .map(|(name, gate)| {
                        let mut g = gate.clone();
                        let cmd = crate::cli::vars::substitute_vars(&g.command, &vars_for_gates);
                        g.command = variables.substitute(&cmd);
                        (name.clone(), g)
                    })
                    .collect();
            evaluate_gates(
                &substituted,
                &current_dir,
                Some(context_store),
                Some(session_name),
                Some(&children_eval),
            )
        };

    let integration_closure = |_name: &str| -> Result<serde_json::Value, IntegrationError> {
        Err(IntegrationError::Unavailable)
    };

    let action_closure = |state_name: &str,
                          action: &crate::template::types::ActionDecl,
                          has_evidence: bool|
     -> ActionResult {
        // If override evidence exists, skip the action.
        if has_evidence {
            return ActionResult::Skipped;
        }

        // Substitute variables in command and working_dir.
        let command = variables.substitute(&action.command);
        let wd = if action.working_dir.is_empty() {
            current_dir.clone()
        } else {
            std::path::PathBuf::from(variables.substitute(&action.working_dir))
        };

        // Execute: polling or one-shot.
        let output = if let Some(polling) = &action.polling {
            // For polling, we need to evaluate gates inside the loop.
            // Look up the state's gates from the compiled template.
            let state_gates = compiled
                .states
                .get(state_name)
                .map(|s| {
                    // Substitute variables in gate commands.
                    s.gates
                        .iter()
                        .map(|(name, gate)| {
                            let mut g = gate.clone();
                            g.command = variables.substitute(&g.command);
                            (name.clone(), g)
                        })
                        .collect::<std::collections::BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            execute_with_polling(
                &command,
                &wd,
                polling,
                &state_gates,
                &|gates: &std::collections::BTreeMap<String, crate::template::types::Gate>| {
                    crate::gate::evaluate_gates(
                        gates,
                        &current_dir,
                        Some(context_store),
                        Some(&name),
                        None, // children-complete not needed in polling loop
                    )
                },
                &shutdown,
            )
        } else {
            crate::action::run_shell_command(&command, &wd, 30)
        };

        // Truncate output.
        let stdout = truncate_output(&output.stdout, MAX_ACTION_OUTPUT_BYTES);
        let stderr = truncate_output(&output.stderr, MAX_ACTION_OUTPUT_BYTES);

        // Append DefaultActionExecuted event.
        let event_payload = EventPayload::DefaultActionExecuted {
            state: state_name.to_string(),
            command: command.clone(),
            exit_code: output.exit_code,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
        };
        let _ = backend.append_event(&name, &event_payload, &now_iso8601());

        if action.requires_confirmation {
            ActionResult::RequiresConfirmation {
                exit_code: output.exit_code,
                stdout,
                stderr,
            }
        } else {
            ActionResult::Executed {
                exit_code: output.exit_code,
                stdout,
                stderr,
            }
        }
    };

    let result = advance_until_stop(
        current_state,
        &compiled,
        &evidence,
        &current_events,
        &mut append_closure,
        &gate_closure,
        &integration_closure,
        &action_closure,
        &shutdown,
    );

    // 8. Map AdvanceResult/AdvanceError to NextResponse/NextError and exit.
    match result {
        Ok(advance_result) => {
            let final_state = &advance_result.final_state;
            let advanced = advance_result.advanced;

            let final_template_state = match compiled.states.get(final_state) {
                Some(s) => s,
                None => {
                    let ne = NextError {
                        code: NextErrorCode::TemplateError,
                        message: format!("state '{}' not found in template", final_state),
                        details: vec![],
                    };
                    let json = serde_json::json!({"error": ne});
                    exit_with_error_code(json, ne.code.exit_code());
                }
            };

            let expects = crate::cli::next_types::derive_expects(final_template_state);

            // Use the raw directive here; with_substituted_directive applies both
            // runtime and template variable substitution before serialization.
            let directive = final_template_state.directive.clone();

            // Derive visit counts to decide whether to include details.
            // Re-read events to capture transitions appended during the advancement loop.
            let details = if final_template_state.details.is_empty() {
                None
            } else {
                let post_events = backend
                    .read_events(&name)
                    .map(|(_, evts)| evts)
                    .unwrap_or_default();
                let visit_counts = derive_visit_counts(&post_events);
                let count = visit_counts.get(final_state.as_str()).copied().unwrap_or(0);
                if full || count <= 1 {
                    Some(final_template_state.details.clone())
                } else {
                    None
                }
            };

            let resp = match advance_result.stop_reason {
                StopReason::Terminal => NextResponse::Terminal {
                    state: final_state.clone(),
                    advanced,
                },
                StopReason::GateBlocked(gate_results) => {
                    let blocking =
                        blocking_conditions_from_gates(&gate_results, &final_template_state.gates);
                    NextResponse::GateBlocked {
                        state: final_state.clone(),
                        directive: directive.clone(),
                        details: details.clone(),
                        advanced,
                        blocking_conditions: blocking,
                    }
                }
                StopReason::EvidenceRequired { failed_gates } => {
                    // The engine only returns EvidenceRequired when accepts is Some,
                    // so expects is always populated here.
                    let es = expects.unwrap_or_else(|| ExpectsSchema {
                        event_type: "evidence_submitted".to_string(),
                        fields: std::collections::BTreeMap::new(),
                        options: vec![],
                    });
                    let blocking = failed_gates
                        .as_ref()
                        .map(|fg| blocking_conditions_from_gates(fg, &final_template_state.gates))
                        .unwrap_or_default();
                    NextResponse::EvidenceRequired {
                        state: final_state.clone(),
                        directive: directive.clone(),
                        details: details.clone(),
                        advanced,
                        expects: es,
                        blocking_conditions: blocking,
                    }
                }
                StopReason::UnresolvableTransition => {
                    let err = NextError {
                        code: NextErrorCode::TemplateError,
                        message: format!(
                            "state '{}' has conditional transitions but no accepts block; \
                             the agent cannot submit evidence to resolve this",
                            final_state
                        ),
                        details: vec![],
                    };
                    let json = serde_json::json!({"error": err});
                    exit_with_error_code(json, err.code.exit_code());
                }
                StopReason::Integration { name, output } => NextResponse::Integration {
                    state: final_state.clone(),
                    directive: directive.clone(),
                    details: details.clone(),
                    advanced,
                    expects,
                    integration: IntegrationOutput { name, output },
                },
                StopReason::IntegrationUnavailable { name } => {
                    NextResponse::IntegrationUnavailable {
                        state: final_state.clone(),
                        directive: directive.clone(),
                        details: details.clone(),
                        advanced,
                        expects,
                        integration: IntegrationUnavailableMarker {
                            name,
                            available: false,
                        },
                    }
                }
                StopReason::ActionRequiresConfirmation {
                    state: action_state,
                    exit_code,
                    stdout,
                    stderr,
                } => NextResponse::ActionRequiresConfirmation {
                    state: action_state,
                    directive: final_template_state.directive.clone(),
                    details: details.clone(),
                    advanced,
                    action_output: crate::cli::next_types::ActionOutput {
                        command: final_template_state
                            .default_action
                            .as_ref()
                            .map(|a| variables.substitute(&a.command))
                            .unwrap_or_default(),
                        exit_code,
                        stdout,
                        stderr,
                    },
                    expects,
                },
                StopReason::CycleDetected { state: cycle_state } => {
                    // Cycle is a template bug; report as an error.
                    let err = NextError {
                        code: NextErrorCode::TemplateError,
                        message: format!(
                            "cycle detected: advancement loop would revisit state '{}'",
                            cycle_state
                        ),
                        details: vec![],
                    };
                    let json = serde_json::json!({"error": err});
                    exit_with_error_code(json, err.code.exit_code());
                }
                StopReason::ChainLimitReached => {
                    let err = NextError {
                        code: NextErrorCode::TemplateError,
                        message: "advancement chain limit reached (100 transitions)".to_string(),
                        details: vec![],
                    };
                    let json = serde_json::json!({"error": err});
                    exit_with_error_code(json, err.code.exit_code());
                }
                StopReason::SignalReceived => {
                    // Return the current state with advanced flag.
                    // The agent can resume from here.
                    if final_template_state.terminal {
                        NextResponse::Terminal {
                            state: final_state.clone(),
                            advanced,
                        }
                    } else if let Some(ref es) = expects {
                        NextResponse::EvidenceRequired {
                            state: final_state.clone(),
                            directive: directive.clone(),
                            details: details.clone(),
                            advanced,
                            expects: es.clone(),
                            blocking_conditions: vec![],
                        }
                    } else {
                        NextResponse::EvidenceRequired {
                            state: final_state.clone(),
                            directive: directive.clone(),
                            details: details.clone(),
                            advanced,
                            expects: ExpectsSchema {
                                event_type: "evidence_submitted".to_string(),
                                fields: std::collections::BTreeMap::new(),
                                options: vec![],
                            },
                            blocking_conditions: vec![],
                        }
                    }
                }
            };

            let resp = resp.with_substituted_directive(|d| {
                let d = crate::cli::vars::substitute_vars(d, &runtime_vars);
                variables.substitute(&d)
            });
            println!("{}", serde_json::to_string(&resp)?);
            // Auto-cleanup after output when reaching a terminal state.
            if matches!(resp, NextResponse::Terminal { .. }) && !no_cleanup {
                if let Err(e) = backend.cleanup(&name) {
                    eprintln!("warning: session cleanup failed: {}", e);
                }
            }
            std::process::exit(0);
        }
        Err(advance_err) => {
            let code = match &advance_err {
                AdvanceError::AmbiguousTransition { .. } => NextErrorCode::TemplateError,
                AdvanceError::DeadEndState { .. } => NextErrorCode::TemplateError,
                AdvanceError::UnknownState { .. } => NextErrorCode::TemplateError,
                AdvanceError::PersistenceError(_) => NextErrorCode::PersistenceError,
            };
            let err = NextError {
                code,
                message: advance_err.to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }
    }
}

/// Non-unix stub for handle_next.
#[cfg(not(unix))]
fn handle_next(
    _backend: &dyn SessionBackend,
    _context_store: &dyn ContextStore,
    name: String,
    _with_data: Option<String>,
    _to: Option<String>,
    _no_cleanup: bool,
    _full: bool,
) -> Result<()> {
    exit_with_error_code(
        serde_json::json!({
            "error": "koto next is only supported on unix platforms",
            "command": "next"
        }),
        3,
    );
}

/// Handle the `koto decisions record` command.
///
/// Appends a `DecisionRecorded` event to the state file without running
/// the advancement loop. Validates the payload against a fixed schema:
/// - "choice" (string, required)
/// - "rationale" (string, required)
/// - "alternatives_considered" (array of strings, optional)
fn handle_decisions_record(
    backend: &dyn SessionBackend,
    name: String,
    with_data: String,
) -> Result<()> {
    // 1. Payload size limit
    if with_data.len() > MAX_WITH_DATA_BYTES {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("--with-data payload exceeds maximum size of {} bytes", MAX_WITH_DATA_BYTES),
                "command": "decisions record"
            }),
            2,
        );
    }

    // 2. Load state file
    if !backend.exists(&name) {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' not found", name),
            "command": "decisions record"
        }));
    }

    let (header, events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "decisions record"
                }),
                code,
            );
        }
    };

    // Derive current state
    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            exit_with_error(serde_json::json!({
                "error": "corrupt state file: cannot derive current state",
                "command": "decisions record"
            }));
        }
    };

    // Verify template hash
    let template_bytes = match std::fs::read(&machine_state.template_path) {
        Ok(b) => b,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to read template {}: {}", machine_state.template_path, e),
                    "command": "decisions record"
                }),
                3,
            );
        }
    };
    let actual_hash = sha256_hex(&template_bytes);
    if actual_hash != machine_state.template_hash {
        exit_with_error_code(
            serde_json::json!({
                "error": format!(
                    "template hash mismatch: header says {} but cached template hashes to {}",
                    machine_state.template_hash, actual_hash
                ),
                "command": "decisions record"
            }),
            3,
        );
    }

    // 3. Parse the JSON payload
    let data: serde_json::Value = match serde_json::from_str(&with_data) {
        Ok(v) => v,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("invalid JSON in --with-data: {}", e),
                    "command": "decisions record"
                }),
                2,
            );
        }
    };

    // 4. Validate the fixed decision schema
    let obj = match data.as_object() {
        Some(o) => o,
        None => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "decision payload must be a JSON object",
                    "command": "decisions record"
                }),
                2,
            );
        }
    };

    // Validate "choice" (string, required)
    match obj.get("choice") {
        Some(v) if v.is_string() => {}
        Some(_) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "field \"choice\" must be a string",
                    "command": "decisions record"
                }),
                2,
            );
        }
        None => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "missing required field \"choice\"",
                    "command": "decisions record"
                }),
                2,
            );
        }
    }

    // Validate "rationale" (string, required)
    match obj.get("rationale") {
        Some(v) if v.is_string() => {}
        Some(_) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "field \"rationale\" must be a string",
                    "command": "decisions record"
                }),
                2,
            );
        }
        None => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "missing required field \"rationale\"",
                    "command": "decisions record"
                }),
                2,
            );
        }
    }

    // Validate "alternatives_considered" (array of strings, optional)
    if let Some(alt) = obj.get("alternatives_considered") {
        match alt.as_array() {
            Some(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    if !item.is_string() {
                        exit_with_error_code(
                            serde_json::json!({
                                "error": format!("alternatives_considered[{}] must be a string", i),
                                "command": "decisions record"
                            }),
                            2,
                        );
                    }
                }
            }
            None => {
                exit_with_error_code(
                    serde_json::json!({
                        "error": "field \"alternatives_considered\" must be an array of strings",
                        "command": "decisions record"
                    }),
                    2,
                );
            }
        }
    }

    // 5. Append DecisionRecorded event
    let current_state = machine_state.current_state.clone();
    let payload = EventPayload::DecisionRecorded {
        state: current_state.clone(),
        decision: data,
    };
    if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
        exit_with_error(serde_json::json!({
            "error": e.to_string(),
            "command": "decisions record"
        }));
    }

    // 6. Count decisions in current epoch
    let (_, updated_events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "decisions record"
                }),
                code,
            );
        }
    };
    let decision_count = derive_decisions(&updated_events).len();

    // 7. Print confirmation
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "state": current_state,
            "decisions_recorded": decision_count
        }))?
    );
    Ok(())
}

/// Handle the `koto decisions list` command.
///
/// Returns accumulated decisions for the current state's epoch as a
/// standalone JSON response.
fn handle_decisions_list(backend: &dyn SessionBackend, name: String) -> Result<()> {
    if !backend.exists(&name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": "decisions list"
            }),
            2,
        );
    }

    let (_, events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "decisions list"
                }),
                code,
            );
        }
    };

    let current_state = derive_state_from_log(&events).unwrap_or_default();
    let decision_events = derive_decisions(&events);

    let items: Vec<serde_json::Value> = decision_events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::DecisionRecorded { decision, .. } => Some(decision.clone()),
            _ => None,
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "state": current_state,
            "decisions": {
                "count": items.len(),
                "items": items
            }
        }))?
    );
    Ok(())
}

/// Handle the `koto status` command.
///
/// Returns a JSON object with the workflow's current state, template info,
/// and terminal status. Read-only: does not evaluate gates, run actions,
/// or modify the state file.
fn handle_status(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    if !backend.exists(name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("workflow '{}' not found", name),
                "command": "status"
            }),
            2,
        );
    }

    let (header, events) = match backend.read_events(name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "status"
                }),
                code,
            );
        }
    };

    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            exit_with_error_code(
                serde_json::json!({
                    "error": "corrupt state file: cannot derive current state",
                    "command": "status"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    // Load the compiled template to determine terminal status.
    let template_bytes = match std::fs::read(&machine_state.template_path) {
        Ok(b) => b,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to read template: {}", e),
                    "command": "status"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };
    let compiled: CompiledTemplate = match serde_json::from_slice(&template_bytes) {
        Ok(t) => t,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to parse template: {}", e),
                    "command": "status"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    let is_terminal = compiled
        .states
        .get(&machine_state.current_state)
        .is_some_and(|s| s.terminal);

    let response = serde_json::json!({
        "name": name,
        "current_state": machine_state.current_state,
        "template_path": machine_state.template_path,
        "template_hash": machine_state.template_hash,
        "is_terminal": is_terminal,
    });

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

/// Query child workflows for a given parent name.
///
/// Returns a JSON array where each entry has `name` and `state` fields.
/// Uses `backend.list()` filtered by `parent_workflow` to discover children,
/// then reads each child's event log to derive its current state.
/// Evaluate a `children-complete` gate by querying the session backend for
/// child workflows and checking their completion status.
///
/// Discovery: calls `backend.list()`, filters by `parent_workflow == workflow_name`.
/// If `gate.name_filter` is set, further filters by name prefix.
///
/// Completion: for now only `"terminal"` is supported. Each child's events are
/// read, state derived, and checked against the child's compiled template for
/// terminal status.
///
/// Returns `Failed` when zero children match (no vacuous pass) or any child is
/// not yet complete. Returns `Passed` when all children are complete.
fn evaluate_children_complete(
    backend: &dyn SessionBackend,
    workflow_name: &str,
    gate: &crate::template::types::Gate,
) -> crate::gate::StructuredGateResult {
    use crate::gate::{GateOutcome, StructuredGateResult};

    let sessions = match backend.list() {
        Ok(s) => s,
        Err(e) => {
            return StructuredGateResult {
                outcome: GateOutcome::Error,
                output: serde_json::json!({
                    "total": 0,
                    "completed": 0,
                    "pending": 0,
                    "all_complete": false,
                    "children": [],
                    "error": format!("failed to list sessions: {}", e)
                }),
            };
        }
    };

    // Filter to direct children of this workflow.
    let mut children: Vec<_> = sessions
        .into_iter()
        .filter(|s| s.parent_workflow.as_deref() == Some(workflow_name))
        .collect();

    // Apply optional name_filter prefix.
    if let Some(ref prefix) = gate.name_filter {
        children.retain(|s| s.id.starts_with(prefix.as_str()));
    }

    // Zero children: fail (no vacuous pass).
    if children.is_empty() {
        return StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({
                "total": 0,
                "completed": 0,
                "pending": 0,
                "all_complete": false,
                "children": [],
                "error": "no matching children found"
            }),
        };
    }

    let total = children.len();
    let mut completed = 0usize;
    let mut child_details = Vec::new();

    for child in &children {
        let (child_header, child_events) = match backend.read_events(&child.id) {
            Ok(result) => result,
            Err(_) => {
                child_details.push(serde_json::json!({
                    "name": child.id,
                    "state": "",
                    "complete": false
                }));
                continue;
            }
        };

        let machine_state = derive_machine_state(&child_header, &child_events);
        let current_state = machine_state
            .as_ref()
            .map(|ms| ms.current_state.as_str())
            .unwrap_or("");

        // Determine if the child is complete (terminal state).
        let is_complete = if let Some(ref ms) = machine_state {
            // Load the child's compiled template to check terminal status.
            std::fs::read(&ms.template_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<CompiledTemplate>(&bytes).ok())
                .and_then(|tmpl| tmpl.states.get(&ms.current_state).map(|s| s.terminal))
                .unwrap_or(false)
        } else {
            false
        };

        if is_complete {
            completed += 1;
        }

        child_details.push(serde_json::json!({
            "name": child.id,
            "state": current_state,
            "complete": is_complete
        }));
    }

    let pending = total - completed;
    let all_complete = pending == 0;
    let outcome = if all_complete {
        GateOutcome::Passed
    } else {
        GateOutcome::Failed
    };

    StructuredGateResult {
        outcome,
        output: serde_json::json!({
            "total": total,
            "completed": completed,
            "pending": pending,
            "all_complete": all_complete,
            "children": child_details,
            "error": ""
        }),
    }
}

fn query_children(backend: &dyn SessionBackend, parent_name: &str) -> Vec<serde_json::Value> {
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    sessions
        .into_iter()
        .filter(|s| s.parent_workflow.as_deref() == Some(parent_name))
        .map(|child| {
            let state = backend
                .read_events(&child.id)
                .ok()
                .and_then(|(_, events)| derive_state_from_log(&events))
                .unwrap_or_default();
            serde_json::json!({
                "name": child.id,
                "state": state
            })
        })
        .collect()
}

/// Handle the `koto cancel` command.
///
/// Appends a `WorkflowCancelled` event to the event log. Rejects double-cancel
/// and cancel of already-terminal workflows.
fn handle_cancel(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    if !backend.exists(name) {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' not found", name),
            "command": "cancel"
        }));
    }

    let (header, events) = match backend.read_events(name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "cancel"
                }),
                code,
            );
        }
    };

    // Check for double-cancel.
    let already_cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
    if already_cancelled {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("workflow '{}' is already cancelled", name),
                "command": "cancel"
            }),
            2,
        );
    }

    // Derive current state and check if terminal.
    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            exit_with_error(serde_json::json!({
                "error": "corrupt state file: cannot derive current state",
                "command": "cancel"
            }));
        }
    };

    // Load template to check terminal status.
    let template_bytes = match std::fs::read(&machine_state.template_path) {
        Ok(b) => b,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to read template: {}", e),
                    "command": "cancel"
                }),
                3,
            );
        }
    };
    let compiled: CompiledTemplate = match serde_json::from_slice(&template_bytes) {
        Ok(t) => t,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to parse template: {}", e),
                    "command": "cancel"
                }),
                3,
            );
        }
    };

    let current_state = &machine_state.current_state;
    if let Some(template_state) = compiled.states.get(current_state) {
        if template_state.terminal {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("workflow '{}' is already in terminal state '{}'", name, current_state),
                    "command": "cancel"
                }),
                2,
            );
        }
    }

    // Append the WorkflowCancelled event.
    let payload = EventPayload::WorkflowCancelled {
        state: current_state.clone(),
        reason: "cancelled by user".to_string(),
    };
    if let Err(e) = backend.append_event(name, &payload, &now_iso8601()) {
        exit_with_error(serde_json::json!({
            "error": e.to_string(),
            "command": "cancel"
        }));
    }

    let children = query_children(backend, name);

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "state": current_state,
            "cancelled": true,
            "children": children
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export_args(
        format: ExportFormat,
        output: Option<&str>,
        open: bool,
        check: bool,
    ) -> ExportArgs {
        ExportArgs {
            input: "template.md".to_string(),
            format,
            output: output.map(String::from),
            open,
            check,
        }
    }

    #[test]
    fn validate_mermaid_stdout_ok() {
        let args = export_args(ExportFormat::Mermaid, None, false, false);
        assert!(validate_export_flags(&args).is_ok());
    }

    #[test]
    fn validate_mermaid_with_output_ok() {
        let args = export_args(ExportFormat::Mermaid, Some("out.md"), false, false);
        assert!(validate_export_flags(&args).is_ok());
    }

    #[test]
    fn validate_html_without_output_fails() {
        let args = export_args(ExportFormat::Html, None, false, false);
        let err = validate_export_flags(&args).unwrap_err();
        assert!(
            err.contains("--format html requires --output"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_html_with_output_ok() {
        let args = export_args(ExportFormat::Html, Some("out.html"), false, false);
        assert!(validate_export_flags(&args).is_ok());
    }

    #[test]
    fn validate_open_without_html_fails() {
        let args = export_args(ExportFormat::Mermaid, Some("out.md"), true, false);
        let err = validate_export_flags(&args).unwrap_err();
        assert!(
            err.contains("--open is only valid with --format html"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_open_with_html_ok() {
        let args = export_args(ExportFormat::Html, Some("out.html"), true, false);
        assert!(validate_export_flags(&args).is_ok());
    }

    #[test]
    fn validate_open_and_check_fails() {
        let args = export_args(ExportFormat::Html, Some("out.html"), true, true);
        let err = validate_export_flags(&args).unwrap_err();
        assert!(
            err.contains("--open and --check are mutually exclusive"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_check_without_output_fails() {
        let args = export_args(ExportFormat::Mermaid, None, false, true);
        let err = validate_export_flags(&args).unwrap_err();
        assert!(err.contains("--check requires --output"), "got: {}", err);
    }

    #[test]
    fn validate_check_with_output_ok() {
        let args = export_args(ExportFormat::Mermaid, Some("out.md"), false, true);
        assert!(validate_export_flags(&args).is_ok());
    }

    // ---------------------------------------------------------------------------
    // evidence_has_reserved_gates_key
    // ---------------------------------------------------------------------------

    #[test]
    fn gates_key_present_is_reserved() {
        let data = serde_json::json!({"gates": {"ci_check": {"exit_code": 0}}, "other": "value"});
        assert!(evidence_has_reserved_gates_key(&data));
    }

    #[test]
    fn gates_key_absent_is_not_reserved() {
        let data = serde_json::json!({"decision": "proceed", "notes": "looks good"});
        assert!(!evidence_has_reserved_gates_key(&data));
    }

    #[test]
    fn non_object_value_is_not_reserved() {
        // Non-objects (malformed payloads) return false; schema validation catches them later.
        assert!(!evidence_has_reserved_gates_key(&serde_json::json!(
            "string"
        )));
        assert!(!evidence_has_reserved_gates_key(&serde_json::json!(42)));
        assert!(!evidence_has_reserved_gates_key(&serde_json::json!(null)));
    }

    #[test]
    fn empty_object_is_not_reserved() {
        assert!(!evidence_has_reserved_gates_key(&serde_json::json!({})));
    }

    // ---------------------------------------------------------------------------
    // validate_with_data_payload — reserved "gates" key returns InvalidSubmission
    // ---------------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn handle_next_gates_key_returns_invalid_submission() {
        use crate::cli::next_types::NextErrorCode;

        // A payload with a top-level "gates" key must be rejected with
        // NextErrorCode::InvalidSubmission before schema validation runs.
        let payload = r#"{"gates": {"some": "data"}}"#;
        let result = validate_with_data_payload(payload);
        assert!(result.is_err(), "expected Err for reserved 'gates' key");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            NextErrorCode::InvalidSubmission,
            "expected InvalidSubmission error code, got: {:?}",
            err.code
        );
        assert!(
            err.message.contains("reserved"),
            "error message should mention 'reserved': {}",
            err.message
        );
    }
}
