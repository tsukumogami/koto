pub mod next;
pub mod next_types;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::buildinfo;
use crate::cache::{compile_cached, sha256_hex};
use crate::discover::{find_workflows_with_metadata, workflow_state_path};
use crate::engine::errors::EngineError;
use crate::engine::persistence::{
    append_event, append_header, derive_machine_state, derive_state_from_log, read_events,
};
use crate::engine::types::{now_iso8601, EventPayload, StateFileHeader};
use crate::template::types::CompiledTemplate;

/// Maximum payload size for --with-data (1 MB).
const MAX_WITH_DATA_BYTES: usize = 1_048_576;

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
    /// Print version information as JSON
    Version,

    /// Initialize a new workflow from a template
    Init {
        /// Workflow name
        name: String,

        /// Path to template file
        #[arg(long)]
        template: String,
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
        Some(EngineError::StateFileCorrupted(_)) => 3,
        _ => 1,
    }
}

pub fn run(app: App) -> Result<()> {
    match app.command {
        Command::Version => {
            let info = buildinfo::build_info();
            println!("{}", serde_json::to_string(&info)?);
            Ok(())
        }
        Command::Init { name, template } => {
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
                variables: HashMap::new(),
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
///    state, dispatch on new state (skip gate evaluation)
/// 5. If --with-data: validate evidence against accepts schema, append
///    evidence_submitted event
/// 6. Evaluate command gates (unless --to)
/// 7. Call dispatcher with pre-computed inputs
/// 8. Serialize response and exit with correct code
#[cfg(unix)]
fn handle_next(name: String, with_data: Option<String>, to: Option<String>) -> Result<()> {
    use crate::cli::next::{dispatch_next, NextFlags};
    use crate::cli::next_types::{ErrorDetail, NextError, NextErrorCode};
    use crate::engine::evidence::validate_evidence;
    use crate::gate::evaluate_gates;

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

    // Track whether we appended an event (for the `advanced` flag).
    let mut advanced = false;

    // 4. Handle --to (directed transition)
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
        advanced = true;

        // Dispatch on the new (target) state, skip gate evaluation.
        let target_template_state = compiled.states.get(target).unwrap();
        let flags = NextFlags {
            with_data: false,
            to: true,
        };
        let gate_results = std::collections::BTreeMap::new();

        match dispatch_next(
            target,
            target_template_state,
            advanced,
            &gate_results,
            &flags,
        ) {
            Ok(resp) => {
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

    // Get current state info for evidence validation and dispatch.
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
            .unwrap()
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
        advanced = true;
    }

    // 6. Evaluate command gates (skip if --to, which was handled above)
    let gate_results = if template_state.gates.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        evaluate_gates(&template_state.gates, &current_dir)
    };

    // 7. Call dispatcher
    let flags = NextFlags {
        with_data: with_data.is_some(),
        to: false,
    };

    match dispatch_next(
        current_state,
        template_state,
        advanced,
        &gate_results,
        &flags,
    ) {
        Ok(resp) => {
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
