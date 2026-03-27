use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::Result;

use crate::session::context::ContextStore;

/// Read content from stdin and store it under the given key.
///
/// When `from_file` is provided, reads from that path instead of stdin.
pub fn handle_add(
    store: &dyn ContextStore,
    session: &str,
    key: &str,
    from_file: Option<&str>,
) -> Result<()> {
    let content = match from_file {
        Some(path) => {
            fs::read(path).map_err(|e| anyhow::anyhow!("failed to read file '{}': {}", path, e))?
        }
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| anyhow::anyhow!("failed to read stdin: {}", e))?;
            buf
        }
    };

    store.add(session, key, &content)
}

/// Retrieve stored content and write it to stdout.
///
/// When `to_file` is provided, writes to that path instead of stdout.
pub fn handle_get(
    store: &dyn ContextStore,
    session: &str,
    key: &str,
    to_file: Option<&str>,
) -> Result<()> {
    let content = store.get(session, key)?;

    match to_file {
        Some(path) => {
            if let Some(parent) = PathBuf::from(path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("failed to create parent directory for '{}': {}", path, e)
                    })?;
                }
            }
            fs::write(path, &content)
                .map_err(|e| anyhow::anyhow!("failed to write file '{}': {}", path, e))?;
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&content)
                .map_err(|e| anyhow::anyhow!("failed to write to stdout: {}", e))?;
        }
    }

    Ok(())
}

/// Check if a key exists. Returns Ok(true) if present, Ok(false) if not.
///
/// The caller is responsible for mapping the boolean to exit codes.
pub fn handle_exists(store: &dyn ContextStore, session: &str, key: &str) -> bool {
    store.ctx_exists(session, key)
}

/// List all keys as a JSON array, optionally filtered by prefix.
pub fn handle_list(store: &dyn ContextStore, session: &str, prefix: Option<&str>) -> Result<()> {
    let keys = store.list_keys(session, prefix)?;
    println!("{}", serde_json::to_string(&keys)?);
    Ok(())
}
