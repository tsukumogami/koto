use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::buildinfo;
use crate::cache::compile_cached;
use crate::discover::{find_workflows, workflow_state_path};
use crate::engine::persistence::{append_event, derive_machine_state, read_events};
use crate::engine::types::{now_iso8601, Event};
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
fn validate_compiled_template(path: &str) -> Result<(), String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read file: {}", e))?;
    let template: CompiledTemplate =
        serde_json::from_str(&content).map_err(|e| format!("invalid JSON: {}", e))?;
    template.validate()
}

/// Load a compiled template from a cache path.
fn load_compiled_template(path: &str) -> anyhow::Result<CompiledTemplate> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read template {}: {}", path, e))?;
    serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse template {}: {}", path, e))
}

/// Print a JSON error and exit with code 1.
fn exit_with_error(error: serde_json::Value) -> ! {
    println!("{}", serde_json::to_string(&error).unwrap_or_default());
    std::process::exit(1);
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
            let event = Event {
                event_type: "init".to_string(),
                state: initial_state.clone(),
                timestamp: now_iso8601(),
                template: Some(cache_path_str),
                template_hash: Some(hash),
            };

            if let Err(e) = append_event(&state_path, &event) {
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

            let events = match read_events(&state_path) {
                Ok(e) => e,
                Err(err) => {
                    exit_with_error(serde_json::json!({
                        "error": err.to_string(),
                        "command": "next"
                    }));
                }
            };

            if events.is_empty() {
                exit_with_error(serde_json::json!({
                    "error": "state file is empty",
                    "command": "next"
                }));
            }

            let machine_state = match derive_machine_state(&events) {
                Some(ms) => ms,
                None => {
                    exit_with_error(serde_json::json!({
                        "error": "corrupt state file",
                        "command": "next"
                    }));
                }
            };

            let compiled = match load_compiled_template(&machine_state.template_path) {
                Ok(t) => t,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": e.to_string(),
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

            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "state": current_state,
                    "directive": template_state.directive,
                    "transitions": template_state.transitions
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

            let events = match read_events(&state_path) {
                Ok(e) => e,
                Err(err) => {
                    exit_with_error(serde_json::json!({
                        "error": err.to_string(),
                        "command": "rewind"
                    }));
                }
            };

            if events.len() <= 1 {
                exit_with_error(serde_json::json!({
                    "error": "already at initial state, cannot rewind",
                    "command": "rewind"
                }));
            }

            let prev_state = events[events.len() - 2].state.clone();
            let event = Event {
                event_type: "rewind".to_string(),
                state: prev_state.clone(),
                timestamp: now_iso8601(),
                template: None,
                template_hash: None,
            };

            if let Err(e) = append_event(&state_path, &event) {
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
            let names = match find_workflows(&current_dir) {
                Ok(n) => n,
                Err(e) => {
                    exit_with_error(serde_json::json!({
                        "error": e.to_string(),
                        "command": "workflows"
                    }));
                }
            };
            println!("{}", serde_json::to_string(&names)?);
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
                        let error = serde_json::json!({"error": e.to_string(), "command": "template compile"});
                        println!("{}", serde_json::to_string(&error)?);
                        std::process::exit(1);
                    }
                }
            }
            TemplateSubcommand::Validate { path } => {
                let result = validate_compiled_template(&path);
                match result {
                    Ok(()) => Ok(()),
                    Err(msg) => {
                        let error =
                            serde_json::json!({"error": msg, "command": "template validate"});
                        println!("{}", serde_json::to_string(&error)?);
                        std::process::exit(1);
                    }
                }
            }
        },
    }
}
