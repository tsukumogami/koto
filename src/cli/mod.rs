use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::buildinfo;

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
            TemplateSubcommand::Compile { .. } => {
                let error = serde_json::json!({"error": "not yet implemented", "command": "template compile"});
                println!("{}", serde_json::to_string(&error)?);
                std::process::exit(1);
            }
            TemplateSubcommand::Validate { .. } => {
                let error = serde_json::json!({"error": "not yet implemented", "command": "template validate"});
                println!("{}", serde_json::to_string(&error)?);
                std::process::exit(1);
            }
        },
    }
}
