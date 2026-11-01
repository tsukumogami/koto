//! Trust a project config file a test wrote by hand.
//!
//! koto applies a project's `.koto/config.toml` only after the user trusts
//! it with `koto config trust`. A test that writes the file itself and
//! means it to apply records the same grant here: one entry in
//! `<koto root>/trust.toml` binding the project directory, the file and the
//! SHA-256 of its bytes. Call it after every write of the file, since any
//! change lapses the grant. `tests/project_config_trust.rs` covers the
//! command itself.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/project_trust.rs"]
//! mod project_trust;
//! ```

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::path::Path;

/// Grant the project config in `project_dir` as it is now, in the koto root
/// `koto_root` (`<HOME>/.koto` for a test that sets `HOME`). Replaces any
/// grant for the same directory.
pub fn trust(koto_root: &Path, project_dir: &Path) {
    let dir = std::fs::canonicalize(project_dir).unwrap();
    let file = dir.join(".koto").join("config.toml");
    let bytes = std::fs::read(&file).unwrap();
    let sha = hex::encode(Sha256::digest(&bytes));
    std::fs::create_dir_all(koto_root).unwrap();
    let store = koto_root.join("trust.toml");
    let mut doc: toml::Value = std::fs::read_to_string(&store)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));
    let table = doc.as_table_mut().unwrap();
    table.insert("version".into(), toml::Value::Integer(1));
    let grants = table
        .entry("grant")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .unwrap();
    let dir_text = dir.to_string_lossy().to_string();
    grants.retain(|g| g.get("dir").and_then(|d| d.as_str()) != Some(dir_text.as_str()));
    let mut grant = toml::map::Map::new();
    grant.insert("dir".into(), toml::Value::String(dir_text));
    grant.insert(
        "file".into(),
        toml::Value::String(file.to_string_lossy().to_string()),
    );
    grant.insert("sha256".into(), toml::Value::String(sha));
    grants.push(toml::Value::Table(grant));
    std::fs::write(&store, toml::to_string(&doc).unwrap()).unwrap();
}
