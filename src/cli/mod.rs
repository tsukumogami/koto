use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::buildinfo;
use crate::cache::compile_cached;
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

pub fn run(app: App) -> Result<()> {
    match app.command {
        Command::Version => {
            let info = buildinfo::build_info();
            println!("{}", serde_json::to_string(&info)?);
            Ok(())
        }
        Command::Init { .. } => {
            let error = serde_json::json!({"error": "not yet implemented", "command": "init"});
            println!("{}", serde_json::to_string(&error)?);
            std::process::exit(1);
        }
        Command::Next { .. } => {
            let error = serde_json::json!({"error": "not yet implemented", "command": "next"});
            println!("{}", serde_json::to_string(&error)?);
            std::process::exit(1);
        }
        Command::Rewind { .. } => {
            let error = serde_json::json!({"error": "not yet implemented", "command": "rewind"});
            println!("{}", serde_json::to_string(&error)?);
            std::process::exit(1);
        }
        Command::Workflows => {
            let error = serde_json::json!({"error": "not yet implemented", "command": "workflows"});
            println!("{}", serde_json::to_string(&error)?);
            std::process::exit(1);
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
