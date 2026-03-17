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
        Command::Next { name } => {
            let current_dir = std::env::current_dir()?;
            let state_path = workflow_state_path(&current_dir, &name);

            if !state_path.exists() {
                exit_with_error(serde_json::json!({
                    "error": format!("workflow '{}' not found", name),
                    "command": "next"
                }));
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

            // Verify the cached template hash matches the header's template_hash.
            let template_bytes = match std::fs::read(&machine_state.template_path) {
                Ok(b) => b,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": format!("failed to read template {}: {}", machine_state.template_path, e),
                        "command": "next"
                    }));
                }
            };
            let actual_hash = sha256_hex(&template_bytes);
            if actual_hash != machine_state.template_hash {
                exit_with_error(serde_json::json!({
                    "error": format!(
                        "template hash mismatch: header says {} but cached template hashes to {}",
                        machine_state.template_hash, actual_hash
                    ),
                    "command": "next"
                }));
            }

            let compiled: CompiledTemplate = match serde_json::from_slice(&template_bytes) {
                Ok(t) => t,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": format!("failed to parse template {}: {}", machine_state.template_path, e),
                        "command": "next"
                    }));
                }
            };

            let current_state = &machine_state.current_state;
            let template_state = match compiled.states.get(current_state) {
                Some(s) => s,
                None => {
                    exit_with_error(serde_json::json!({
                        "error": format!("state '{}' not found in template", current_state),
                        "command": "next"
                    }));
                }
            };

            let transition_targets: Vec<&str> = template_state
                .transitions
                .iter()
                .map(|t| t.target.as_str())
                .collect();

            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "state": current_state,
                    "directive": template_state.directive,
                    "transitions": transition_targets
                }))?
            );
            Ok(())
        }
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
