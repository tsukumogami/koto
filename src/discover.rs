use std::path::Path;

const PREFIX: &str = "koto-";
const SUFFIX: &str = ".state.jsonl";

/// Find all koto workflows in `dir` by globbing `koto-*.state.jsonl`.
///
/// Returns workflow names with the `koto-` prefix and `.state.jsonl` suffix stripped.
pub fn find_workflows(dir: &Path) -> anyhow::Result<Vec<String>> {
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

    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn find_workflows_returns_correct_names() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "koto-my-workflow.state.jsonl");
        touch(dir.path(), "koto-another.state.jsonl");
        touch(dir.path(), "koto-third.state.jsonl");
        // Should not be included:
        touch(dir.path(), "other-file.txt");
        touch(dir.path(), "koto-.state.jsonl"); // empty name

        let mut names = find_workflows(dir.path()).unwrap();
        names.sort();

        assert_eq!(names, vec!["another", "my-workflow", "third"]);
    }

    #[test]
    fn find_workflows_empty_dir() {
        let dir = TempDir::new().unwrap();
        let names = find_workflows(dir.path()).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn find_workflows_ignores_non_matching_files() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "state.jsonl");
        touch(dir.path(), "koto-foo.json");
        touch(dir.path(), "prefix-koto-foo.state.jsonl");

        let names = find_workflows(dir.path()).unwrap();
        assert!(names.is_empty());
    }
}
