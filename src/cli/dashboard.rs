//! Entry point for the `koto dashboard` command.
//!
//! Dispatches to the data layer and renderer. Full implementation in Issue 5.

use anyhow::Result;

use crate::cli::DashboardArgs;
use crate::session::SessionBackend;

/// Entry point called from the CLI dispatch in `src/cli/mod.rs`.
///
/// For now, prints a stub message and returns successfully so that
/// `koto dashboard --help` and basic invocation work before the full
/// implementation lands in Issue 5.
pub fn run(_args: DashboardArgs, _backend: &dyn SessionBackend) -> Result<()> {
    println!("dashboard not yet implemented");
    Ok(())
}
