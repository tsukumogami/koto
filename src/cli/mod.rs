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
            unimplemented!("init command is implemented in Issue 4")
        }
        Command::Next { .. } => {
            unimplemented!("next command is implemented in Issue 4")
        }
        Command::Rewind { .. } => {
            unimplemented!("rewind command is implemented in Issue 4")
        }
        Command::Workflows => {
            unimplemented!("workflows command is implemented in Issue 4")
        }
        Command::Template { subcommand } => match subcommand {
            TemplateSubcommand::Compile { .. } => {
                unimplemented!("template compile is implemented in Issue 4")
            }
            TemplateSubcommand::Validate { .. } => {
                unimplemented!("template validate is implemented in Issue 4")
            }
        },
    }
}
