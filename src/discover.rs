use std::path::{Path, PathBuf};

use crate::engine::persistence::read_header;
use crate::engine::types::WorkflowMetadata;

const PREFIX: &str = "koto-";
const SUFFIX: &str = ".state.jsonl";

/// Return the canonical state file path for a workflow named `name` in `dir`.
///
/// Path format: `<dir>/koto-<name>.state.jsonl`
pub fn workflow_state_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}{}{}", PREFIX, name, SUFFIX))
}

/// Find all koto workflows in `dir` and return metadata from each header.
///
/// Scans for `koto-*.state.jsonl` files, reads the first line of each as a
/// `StateFileHeader`, and converts to `WorkflowMetadata`. Files whose headers
/// cannot be read or parsed are skipped with a warning on stderr.
///
/// Results are sorted by workflow name.
pub fn find_workflows_with_metadata(dir: &Path) -> anyhow::Result<Vec<WorkflowMetadata>> {
    let names = find_workflow_names(dir)?;
    let mut results = Vec::new();

    for name in &names {
        let path = workflow_state_path(dir, name);
        match read_header(&path) {
            Ok(header) => {
                results.push(WorkflowMetadata {
                    name: header.workflow.clone(),
                    created_at: header.created_at.clone(),
                    template_hash: header.template_hash.clone(),
                });
            }
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(results)
}

/// Scan `dir` for `koto-*.state.jsonl` files and return the extracted names.
///
/// Names are returned unsorted. Used by `find_workflows_with_metadata`.
fn find_workflow_names(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();

    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("failed to read directory entry: {}", e))?;

        let file_name = entry.file_name();
        let name = match file_name.to_str() {
            Some(n) => n,
            None => continue,
        };

        if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            let inner = &name[PREFIX.len()..name.len() - SUFFIX.len()];
            if !inner.is_empty() {
                names.push(inner.to_string());
            }
        }
    }

    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::StateFileHeader;
    use tempfile::TempDir;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
    }

    /// Write a valid state file with just a header line.
    fn write_header_file(dir: &Path, workflow_name: &str, template_hash: &str) {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: workflow_name.to_string(),
            template_hash: template_hash.to_string(),
            created_at: "2026-03-15T10:00:00Z".to_string(),
        };
        let content = serde_json::to_string(&header).unwrap() + "\n";
        let path = dir.join(format!("koto-{}.state.jsonl", workflow_name));
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn metadata_returns_valid_headers() {
        let dir = TempDir::new().unwrap();
        write_header_file(dir.path(), "alpha", "hash-a");
        write_header_file(dir.path(), "beta", "hash-b");

        let results = find_workflows_with_metadata(dir.path()).unwrap();
        assert_eq!(results.len(), 2);
        // Sorted by name
        assert_eq!(results[0].name, "alpha");
        assert_eq!(results[0].template_hash, "hash-a");
        assert_eq!(results[1].name, "beta");
        assert_eq!(results[1].template_hash, "hash-b");
    }

    #[test]
    fn metadata_skips_invalid_headers() {
        let dir = TempDir::new().unwrap();
        write_header_file(dir.path(), "good", "hash-good");

        // Write a file with garbage content
        let bad_path = dir.path().join("koto-bad.state.jsonl");
        std::fs::write(&bad_path, "not valid json\n").unwrap();

        let results = find_workflows_with_metadata(dir.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "good");
    }

    #[test]
    fn metadata_empty_directory() {
        let dir = TempDir::new().unwrap();
        let results = find_workflows_with_metadata(dir.path()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn metadata_mixed_files_only_matches_state_files() {
        let dir = TempDir::new().unwrap();
        write_header_file(dir.path(), "wf-one", "hash-1");

        // Non-matching files should be ignored entirely
        touch(dir.path(), "other-file.txt");
        touch(dir.path(), "koto-foo.json"); // wrong suffix
        touch(dir.path(), "koto-.state.jsonl"); // empty name

        let results = find_workflows_with_metadata(dir.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "wf-one");
    }

    #[test]
    fn metadata_skips_empty_state_file() {
        let dir = TempDir::new().unwrap();
        write_header_file(dir.path(), "valid", "hash-v");

        // Empty file -- header read will fail
        let empty_path = dir.path().join("koto-empty.state.jsonl");
        std::fs::write(&empty_path, "").unwrap();

        let results = find_workflows_with_metadata(dir.path()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "valid");
    }

    #[test]
    fn metadata_results_sorted_by_name() {
        let dir = TempDir::new().unwrap();
        write_header_file(dir.path(), "zulu", "hash-z");
        write_header_file(dir.path(), "alpha", "hash-a");
        write_header_file(dir.path(), "mike", "hash-m");

        let results = find_workflows_with_metadata(dir.path()).unwrap();
        let names: Vec<&str> = results.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mike", "zulu"]);
    }
}
