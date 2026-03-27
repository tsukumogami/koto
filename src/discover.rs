use crate::engine::types::WorkflowMetadata;
use crate::session::SessionBackend;

/// Find all koto workflows and return metadata from each session.
///
/// Delegates to `backend.list()` to discover sessions. Each session's
/// metadata (name, created_at) is extracted from the state file header.
/// Sessions whose headers cannot be read are skipped with a warning on stderr.
///
/// Results are sorted by workflow name.
pub fn find_workflows_with_metadata(
    backend: &dyn SessionBackend,
) -> anyhow::Result<Vec<WorkflowMetadata>> {
    let sessions = backend.list()?;
    let results = sessions
        .into_iter()
        .map(|info| WorkflowMetadata {
            name: info.id,
            created_at: info.created_at,
            template_hash: info.template_hash,
        })
        .collect();
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::persistence::append_header;
    use crate::engine::types::StateFileHeader;
    use crate::session::local::LocalBackend;
    use crate::session::state_file_name;
    use std::path::Path;
    use tempfile::TempDir;

    /// Write a valid state file header into a session directory.
    ///
    /// Creates `<base_dir>/<workflow_name>/koto-<workflow_name>.state.jsonl`
    /// with a single header line.
    fn write_session_state(base_dir: &Path, workflow_name: &str, template_hash: &str) {
        let session_dir = base_dir.join(workflow_name);
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(workflow_name));
        let header = StateFileHeader {
            schema_version: 1,
            workflow: workflow_name.to_string(),
            template_hash: template_hash.to_string(),
            created_at: "2026-03-15T10:00:00Z".to_string(),
        };
        append_header(&state_path, &header).unwrap();
    }

    #[test]
    fn metadata_returns_valid_headers() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "alpha", "hash-a");
        write_session_state(dir.path(), "beta", "hash-b");

        let results = find_workflows_with_metadata(&backend).unwrap();
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
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "good", "hash-good");

        // Write a file with garbage content inside a session directory
        let bad_dir = dir.path().join("bad");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join(state_file_name("bad")), "not valid json\n").unwrap();

        let results = find_workflows_with_metadata(&backend).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "good");
    }

    #[test]
    fn metadata_empty_directory() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        let results = find_workflows_with_metadata(&backend).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn metadata_mixed_files_only_matches_sessions() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "wf-one", "hash-1");

        // Non-session items should be ignored: files (not dirs), dirs without state files
        std::fs::write(dir.path().join("other-file.txt"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("orphan-dir")).unwrap();

        let results = find_workflows_with_metadata(&backend).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "wf-one");
    }

    #[test]
    fn metadata_skips_empty_state_file() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "valid", "hash-v");

        // Empty state file inside a session directory -- header read will fail
        let empty_dir = dir.path().join("empty");
        std::fs::create_dir_all(&empty_dir).unwrap();
        std::fs::write(empty_dir.join(state_file_name("empty")), "").unwrap();

        let results = find_workflows_with_metadata(&backend).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "valid");
    }

    #[test]
    fn metadata_results_sorted_by_name() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "zulu", "hash-z");
        write_session_state(dir.path(), "alpha", "hash-a");
        write_session_state(dir.path(), "mike", "hash-m");

        let results = find_workflows_with_metadata(&backend).unwrap();
        let names: Vec<&str> = results.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mike", "zulu"]);
    }
}
