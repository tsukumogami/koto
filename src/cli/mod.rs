pub mod next;
pub mod next_types;

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::engine::substitute::validate_value;
use crate::template::types::VariableDecl;

use crate::buildinfo;
use crate::cache::{compile_cached, sha256_hex};
use crate::discover::{find_workflows_with_metadata, workflow_state_path};
use crate::engine::errors::EngineError;
use crate::engine::persistence::{
    append_event, append_header, derive_decisions, derive_machine_state, derive_state_from_log,
    read_events,
};
use crate::engine::types::{now_iso8601, EventPayload, StateFileHeader};
use crate::template::types::CompiledTemplate;

/// Maximum payload size for --with-data (1 MB).
const MAX_WITH_DATA_BYTES: usize = 1_048_576;

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
const EXIT_INFRASTRUCTURE: i32 = 3;

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
    Workflows,

    /// Template management subcommands
    Template {
        #[command(subcommand)]
        subcommand: TemplateSubcommand,
    },

    /// Decision recording and retrieval
    Decisions {
        #[command(subcommand)]
        subcommand: DecisionsSubcommand,
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
            let (cache_path, _hash) = compile_cached(path)?;
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
    template.validate().map_err(|e| anyhow::anyhow!("{}", e))
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
fn exit_with_error_code(error: serde_json::Value, code: i32) -> ! {
    println!("{}", serde_json::to_string(&error).unwrap_or_default());
    std::process::exit(code);
}

/// Determine the exit code for an engine error by downcasting to EngineError.
///
/// Returns exit code 3 for corrupted state files, and exit code 1 for all
/// other errors.
fn exit_code_for_engine_error(err: &anyhow::Error) -> i32 {
    match err.downcast_ref::<EngineError>() {
        Some(EngineError::StateFileCorrupted(_)) => EXIT_INFRASTRUCTURE,
        _ => 1,
    }
}

/// Validate and resolve `--var KEY=VALUE` arguments against the template's
/// variable declarations. Returns a map of resolved variable bindings ready
/// for storage in the WorkflowInitialized event.
fn resolve_variables(
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
    ) -> std::collections::BTreeMap<String, crate::gate::GateResult>,
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
                .all(|r| matches!(r, crate::gate::GateResult::Passed));
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
        } => {
            // Validate workflow name before any filesystem operation.
            if let Err(msg) = crate::discover::validate_workflow_name(&name) {
                exit_with_error_code(
                    serde_json::json!({
                        "error": msg,
                        "command": "init",
                        "allowed_pattern": "^[a-zA-Z0-9][a-zA-Z0-9._-]*$"
                    }),
                    2,
                );
            }

            let current_dir = std::env::current_dir()?;
            let state_path = workflow_state_path(&current_dir, &name);

            if state_path.exists() {
                exit_with_error(serde_json::json!({
                    "error": format!("workflow '{}' already exists", name),
                    "command": "init"
                }));
            }

            let (cache_path, hash) = match compile_cached(Path::new(&template)) {
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
            let variables = match resolve_variables(&vars, &compiled.variables) {
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

            // Write header line
            let header = StateFileHeader {
                schema_version: 1,
                workflow: name.clone(),
                template_hash: hash,
                created_at: ts.clone(),
            };
            if let Err(e) = append_header(&state_path, &header) {
                exit_with_error(serde_json::json!({
                    "error": e.to_string(),
                    "command": "init"
                }));
            }

            // Write workflow_initialized event (seq 1)
            let init_payload = EventPayload::WorkflowInitialized {
                template_path: cache_path_str,
                variables,
            };
            if let Err(e) = append_event(&state_path, &init_payload, &ts) {
                exit_with_error(serde_json::json!({
                    "error": e.to_string(),
                    "command": "init"
                }));
            }

            // Write initial transitioned event (seq 2, from: null)
            let transition_payload = EventPayload::Transitioned {
                from: None,
                to: initial_state.clone(),
                condition_type: "auto".to_string(),
            };
            if let Err(e) = append_event(&state_path, &transition_payload, &ts) {
                exit_with_error(serde_json::json!({
                    "error": e.to_string(),
                    "command": "init"
                }));
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
        Command::Next {
            name,
            with_data,
            to,
        } => handle_next(name, with_data, to),
        Command::Cancel { name } => handle_cancel(name),
        Command::Rewind { name } => {
            let current_dir = std::env::current_dir()?;
            let state_path = workflow_state_path(&current_dir, &name);

            if !state_path.exists() {
                exit_with_error(serde_json::json!({
                    "error": format!("workflow '{}' not found", name),
                    "command": "rewind"
                }));
            }

            let (_header, events) = match read_events(&state_path) {
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
            // The current state comes from the last state-changing event.
            // To rewind, we need to find the second-to-last state-changing event.
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

            if let Err(e) = append_event(&state_path, &rewind_payload, &now_iso8601()) {
                exit_with_error(serde_json::json!({
                    "error": e.to_string(),
                    "command": "rewind"
                }));
            }

            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "name": name,
                    "state": prev_state
                }))?
            );
            Ok(())
        }
        Command::Workflows => {
            let current_dir = std::env::current_dir()?;
            let metadata = match find_workflows_with_metadata(&current_dir) {
                Ok(m) => m,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": e.to_string(),
                        "command": "workflows"
                    }));
                }
            };
            println!("{}", serde_json::to_string(&metadata)?);
            Ok(())
        }
        Command::Template { subcommand } => match subcommand {
            TemplateSubcommand::Compile { source } => {
                let source_path = Path::new(&source);
                match compile_cached(source_path) {
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
                    ExportFormat::Mermaid => crate::export::to_mermaid(&compiled).into_bytes(),
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
        Command::Decisions { subcommand } => match subcommand {
            DecisionsSubcommand::Record { name, with_data } => {
                handle_decisions_record(name, with_data)
            }
            DecisionsSubcommand::List { name } => handle_decisions_list(name),
        },
    }
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
/// 6. Acquire advisory flock on state file (non-blocking)
/// 7. Register SIGTERM/SIGINT signal handlers
/// 8. Merge evidence from current epoch
/// 9. Run advancement loop (advance_until_stop)
/// 10. Map StopReason to NextResponse, serialize and exit
///
/// NOTE: This handler uses structured `NextError` for domain errors (per the
/// output contract). Other commands (init, rewind, etc.) use a flat
/// `{"error": "string", "command": "..."}` format. Do not mix the two styles.
#[cfg(unix)]
fn handle_next(name: String, with_data: Option<String>, to: Option<String>) -> Result<()> {
    use crate::cli::next::dispatch_next;
    use crate::cli::next_types::{
        BlockingCondition, ErrorDetail, ExpectsSchema, IntegrationOutput,
        IntegrationUnavailableMarker, NextError, NextErrorCode, NextResponse,
    };
    use crate::engine::advance::{
        advance_until_stop, merge_epoch_evidence, ActionResult, IntegrationError, StopReason,
    };
    use crate::engine::evidence::validate_evidence;
    use crate::engine::persistence::derive_evidence;
    use crate::engine::substitute::Variables;
    use crate::gate::{evaluate_gates, GateResult};
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
    let state_path = workflow_state_path(&current_dir, &name);

    if !state_path.exists() {
        let err = NextError {
            code: NextErrorCode::WorkflowNotInitialized,
            message: format!("workflow '{}' not found", name),
            details: vec![],
        };
        let json = serde_json::json!({"error": err});
        exit_with_error_code(json, err.code.exit_code());
    }

    let (header, events) = match read_events(&state_path) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "next"
                }),
                code,
            );
        }
    };

    if events.is_empty() {
        exit_with_error(serde_json::json!({
            "error": "state file has no events",
            "command": "next"
        }));
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
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("variable re-validation failed: {}", e),
                    "command": "next"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            exit_with_error(serde_json::json!({
                "error": "corrupt state file: cannot derive current state",
                "command": "next"
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
                    "command": "next"
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
                "command": "next"
            }),
            3,
        );
    }

    let compiled: CompiledTemplate = match serde_json::from_slice(&template_bytes) {
        Ok(t) => t,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to parse template {}: {}", machine_state.template_path, e),
                    "command": "next"
                }),
                3,
            );
        }
    };

    // 4. Handle --to (directed transition) -- single-shot, no advancement loop
    if let Some(ref target) = to {
        let current_state = &machine_state.current_state;

        // Look up the current template state to validate the target.
        let current_template_state = match compiled.states.get(current_state) {
            Some(s) => s,
            None => {
                exit_with_error_code(
                    serde_json::json!({
                        "error": format!("state '{}' not found in template", current_state),
                        "command": "next"
                    }),
                    3,
                );
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
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("target state '{}' not found in template", target),
                    "command": "next"
                }),
                3,
            );
        }

        // Append directed_transition event.
        let payload = EventPayload::DirectedTransition {
            from: current_state.clone(),
            to: target.clone(),
        };
        if let Err(e) = append_event(&state_path, &payload, &now_iso8601()) {
            exit_with_error(serde_json::json!({
                "error": e.to_string(),
                "command": "next"
            }));
        }

        // Dispatch on the new (target) state, skip gate evaluation.
        let target_template_state = compiled.states.get(target).unwrap();
        let gate_results = std::collections::BTreeMap::new();

        match dispatch_next(target, target_template_state, true, &gate_results) {
            Ok(resp) => {
                let resp = resp.with_substituted_directive(|d| variables.substitute(d));
                println!("{}", serde_json::to_string(&resp)?);
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
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("state '{}' not found in template", current_state),
                    "command": "next"
                }),
                3,
            );
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

        // Parse the JSON payload.
        let data: serde_json::Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(e) => {
                let err = NextError {
                    code: NextErrorCode::InvalidSubmission,
                    message: format!("invalid JSON in --with-data: {}", e),
                    details: vec![],
                };
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
        if let Err(e) = append_event(&state_path, &payload, &now_iso8601()) {
            exit_with_error(serde_json::json!({
                "error": e.to_string(),
                "command": "next"
            }));
        }
    }

    // 6. Acquire advisory flock on state file (non-blocking).
    // Prevents concurrent koto next calls from interleaving writes.
    let lock_file = match std::fs::File::open(&state_path) {
        Ok(f) => f,
        Err(e) => {
            exit_with_error(serde_json::json!({
                "error": format!("failed to open state file for locking: {}", e),
                "command": "next"
            }));
        }
    };
    {
        use std::os::unix::io::AsRawFd;
        let fd = lock_file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = NextError {
                code: NextErrorCode::PreconditionFailed,
                message: "another koto next is already running for this workflow".to_string(),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code());
        }
    }

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
    let (_, current_events) = match read_events(&state_path) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "next"
                }),
                code,
            );
        }
    };
    let epoch_events = derive_evidence(&current_events);
    let evidence = merge_epoch_evidence(&epoch_events.into_iter().cloned().collect::<Vec<_>>());

    // 9. Set up I/O closures and run advancement loop.
    let state_path_clone = state_path.clone();
    let mut append_closure = |payload: &EventPayload| -> Result<(), String> {
        append_event(&state_path_clone, payload, &now_iso8601())
            .map(|_| ())
            .map_err(|e| e.to_string())
    };

    let gate_closure =
        |gates: &std::collections::BTreeMap<String, crate::template::types::Gate>| {
            let substituted: std::collections::BTreeMap<String, crate::template::types::Gate> =
                gates
                    .iter()
                    .map(|(name, gate)| {
                        let mut g = gate.clone();
                        g.command = variables.substitute(&g.command);
                        (name.clone(), g)
                    })
                    .collect();
            evaluate_gates(&substituted, &current_dir)
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
                    crate::gate::evaluate_gates(gates, &current_dir)
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
        let _ = append_event(&state_path, &event_payload, &now_iso8601());

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
                    exit_with_error_code(
                        serde_json::json!({
                            "error": format!("state '{}' not found in template", final_state),
                            "command": "next"
                        }),
                        3,
                    );
                }
            };

            let expects = crate::cli::next_types::derive_expects(final_template_state);

            let resp = match advance_result.stop_reason {
                StopReason::Terminal => NextResponse::Terminal {
                    state: final_state.clone(),
                    advanced,
                },
                StopReason::GateBlocked(gate_results) => {
                    let blocking: Vec<BlockingCondition> = gate_results
                        .iter()
                        .filter_map(|(name, result)| {
                            let status = match result {
                                GateResult::Passed => return None,
                                GateResult::Failed { .. } => "failed",
                                GateResult::TimedOut => "timed_out",
                                GateResult::Error { .. } => "error",
                            };
                            Some(BlockingCondition {
                                name: name.clone(),
                                condition_type: "command".to_string(),
                                status: status.to_string(),
                                agent_actionable: false,
                            })
                        })
                        .collect();
                    NextResponse::GateBlocked {
                        state: final_state.clone(),
                        directive: final_template_state.directive.clone(),
                        advanced,
                        blocking_conditions: blocking,
                    }
                }
                StopReason::EvidenceRequired => {
                    // The engine only returns EvidenceRequired when accepts is Some,
                    // so expects is always populated here.
                    let es = expects.unwrap_or_else(|| ExpectsSchema {
                        event_type: "evidence_submitted".to_string(),
                        fields: std::collections::BTreeMap::new(),
                        options: vec![],
                    });
                    NextResponse::EvidenceRequired {
                        state: final_state.clone(),
                        directive: final_template_state.directive.clone(),
                        advanced,
                        expects: es,
                    }
                }
                StopReason::UnresolvableTransition => {
                    let err = NextError {
                        code: NextErrorCode::PreconditionFailed,
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
                    directive: final_template_state.directive.clone(),
                    advanced,
                    expects,
                    integration: IntegrationOutput { name, output },
                },
                StopReason::IntegrationUnavailable { name } => {
                    NextResponse::IntegrationUnavailable {
                        state: final_state.clone(),
                        directive: final_template_state.directive.clone(),
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
                        code: NextErrorCode::PreconditionFailed,
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
                        code: NextErrorCode::PreconditionFailed,
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
                            directive: final_template_state.directive.clone(),
                            advanced,
                            expects: es.clone(),
                        }
                    } else {
                        NextResponse::EvidenceRequired {
                            state: final_state.clone(),
                            directive: final_template_state.directive.clone(),
                            advanced,
                            expects: ExpectsSchema {
                                event_type: "evidence_submitted".to_string(),
                                fields: std::collections::BTreeMap::new(),
                                options: vec![],
                            },
                        }
                    }
                }
            };

            let resp = resp.with_substituted_directive(|d| variables.substitute(d));
            println!("{}", serde_json::to_string(&resp)?);
            std::process::exit(0);
        }
        Err(advance_err) => {
            let err = NextError {
                code: NextErrorCode::PreconditionFailed,
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
fn handle_next(name: String, _with_data: Option<String>, _to: Option<String>) -> Result<()> {
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
fn handle_decisions_record(name: String, with_data: String) -> Result<()> {
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
    let current_dir = std::env::current_dir()?;
    let state_path = workflow_state_path(&current_dir, &name);

    if !state_path.exists() {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' not found", name),
            "command": "decisions record"
        }));
    }

    let (header, events) = match read_events(&state_path) {
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
    if let Err(e) = append_event(&state_path, &payload, &now_iso8601()) {
        exit_with_error(serde_json::json!({
            "error": e.to_string(),
            "command": "decisions record"
        }));
    }

    // 6. Count decisions in current epoch
    let (_, updated_events) = match read_events(&state_path) {
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
fn handle_decisions_list(name: String) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let state_path = workflow_state_path(&current_dir, &name);

    if !state_path.exists() {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": "decisions list"
            }),
            2,
        );
    }

    let (_, events) = match read_events(&state_path) {
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

/// Handle the `koto cancel` command.
///
/// Appends a `WorkflowCancelled` event to the event log. Rejects double-cancel
/// and cancel of already-terminal workflows.
fn handle_cancel(name: String) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let state_path = workflow_state_path(&current_dir, &name);

    if !state_path.exists() {
        exit_with_error(serde_json::json!({
            "error": format!("workflow '{}' not found", name),
            "command": "cancel"
        }));
    }

    let (header, events) = match read_events(&state_path) {
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
    if let Err(e) = append_event(&state_path, &payload, &now_iso8601()) {
        exit_with_error(serde_json::json!({
            "error": e.to_string(),
            "command": "cancel"
        }));
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "state": current_state,
            "cancelled": true
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
}
