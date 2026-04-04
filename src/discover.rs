use std::path::{Path, PathBuf};

use crate::engine::types::WorkflowMetadata;
use crate::session::SessionBackend;

const PREFIX: &str = "koto-";
const SUFFIX: &str = ".state.jsonl";

/// Maximum allowed workflow name length.
const MAX_NAME_LENGTH: usize = 255;

/// Validate a workflow name against a strict pattern.
///
/// Security: workflow names are interpolated into state file paths
/// (`koto-<name>.state.jsonl`). An unvalidated name could write files
/// outside the intended directory via path traversal (e.g., `../../etc/passwd`).
/// This function rejects any name that doesn't match the safe pattern.
///
/// Rules:
/// - Must start with an alphanumeric character
/// - May contain alphanumeric characters, hyphens, dots, and underscores
/// - Must not be empty or exceed 255 characters
/// - Must not contain path separators, null bytes, or leading dots
pub fn validate_workflow_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("workflow name must not be empty".to_string());
    }

    if name.len() > MAX_NAME_LENGTH {
        return Err(format!(
            "workflow name must not exceed {} characters",
            MAX_NAME_LENGTH
        ));
    }

    // Check for null bytes (not caught by the regex)
    if name.bytes().any(|b| b == 0) {
        return Err("workflow name must not contain null bytes".to_string());
    }

    // Validate against strict pattern: starts with alphanumeric, then
    // alphanumeric, hyphens, dots, or underscores.
    let valid = name.chars().enumerate().all(|(i, c)| {
        if i == 0 {
            c.is_ascii_alphanumeric()
        } else {
            c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'
        }
    });

    if !valid {
        return Err(format!(
            "workflow name '{}' contains invalid characters; allowed pattern: \
             starts with [a-zA-Z0-9], followed by [a-zA-Z0-9._-]",
            // Truncate long names in error messages to avoid terminal issues
            if name.len() > 50 { &name[..50] } else { name }
        ));
    }

    Ok(())
}

/// Return the canonical state file path for a workflow named `name` in `dir`.
///
/// Path format: `<dir>/koto-<name>.state.jsonl`
///
/// Callers should validate `name` with `validate_workflow_name()` before
/// calling this function to prevent path traversal.
pub fn workflow_state_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}{}{}", PREFIX, name, SUFFIX))
}

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
            parent_workflow: info.parent_workflow,
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
            parent_workflow: None,
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

    // -------------------------------------------------------------------
    // Workflow name validation
    // -------------------------------------------------------------------

    #[test]
    fn valid_workflow_names() {
        for name in &["my-workflow", "work-on-42", "a", "A1.b_c", "test.name", "Z"] {
            assert!(
                validate_workflow_name(name).is_ok(),
                "'{}' should be valid",
                name
            );
        }
    }

    #[test]
    fn rejects_empty_name() {
        assert!(validate_workflow_name("").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        for name in &["../etc", "foo/bar", "..\\windows", "foo\\bar"] {
            assert!(
                validate_workflow_name(name).is_err(),
                "'{}' should be rejected (path traversal)",
                name
            );
        }
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(validate_workflow_name(".hidden").is_err());
    }

    #[test]
    fn rejects_leading_dash() {
        assert!(validate_workflow_name("-leading-dash").is_err());
    }

    #[test]
    fn rejects_spaces() {
        assert!(validate_workflow_name("has space").is_err());
    }

    #[test]
    fn rejects_shell_metacharacters() {
        for name in &["meta$char", "back`tick", "semi;colon", "pipe|char", "amp&"] {
            assert!(
                validate_workflow_name(name).is_err(),
                "'{}' should be rejected (metacharacter)",
                name
            );
        }
    }

    #[test]
    fn rejects_exceeding_max_length() {
        let long_name = "a".repeat(256);
        assert!(validate_workflow_name(&long_name).is_err());
        // 255 should be fine
        let max_name = "a".repeat(255);
        assert!(validate_workflow_name(&max_name).is_ok());
    }

    #[test]
    fn rejects_null_bytes() {
        assert!(validate_workflow_name("foo\0bar").is_err());
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

    /// Write a state file with a parent_workflow set.
    fn write_session_state_with_parent(
        base_dir: &Path,
        workflow_name: &str,
        template_hash: &str,
        parent: &str,
    ) {
        let session_dir = base_dir.join(workflow_name);
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(workflow_name));
        let header = StateFileHeader {
            schema_version: 1,
            workflow: workflow_name.to_string(),
            template_hash: template_hash.to_string(),
            created_at: "2026-03-15T10:00:00Z".to_string(),
            parent_workflow: Some(parent.to_string()),
        };
        append_header(&state_path, &header).unwrap();
    }

    #[test]
    fn metadata_includes_parent_workflow() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        write_session_state(dir.path(), "parent-wf", "hash-p");
        write_session_state_with_parent(dir.path(), "child-wf", "hash-c", "parent-wf");

        let results = find_workflows_with_metadata(&backend).unwrap();
        assert_eq!(results.len(), 2);

        let child = results.iter().find(|m| m.name == "child-wf").unwrap();
        assert_eq!(
            child.parent_workflow,
            Some("parent-wf".to_string()),
            "child should carry parent_workflow"
        );

        let parent = results.iter().find(|m| m.name == "parent-wf").unwrap();
        assert_eq!(
            parent.parent_workflow, None,
            "parent should have no parent_workflow"
        );
    }
}
