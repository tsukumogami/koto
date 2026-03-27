/// Validate a session ID against the allowlist pattern.
///
/// Valid IDs must start with a letter and contain only alphanumeric
/// characters, dots, underscores, and hyphens: `^[a-zA-Z][a-zA-Z0-9._-]*$`.
///
/// This rejects `.` and `..` (path traversal) without a separate check
/// since those don't start with a letter.
pub fn validate_session_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() {
        anyhow::bail!("session ID must not be empty");
    }

    let mut chars = id.chars();

    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() {
        anyhow::bail!("session ID must start with a letter, got '{}'", first);
    }

    for ch in chars {
        if !ch.is_ascii_alphanumeric() && ch != '.' && ch != '_' && ch != '-' {
            anyhow::bail!(
                "session ID contains invalid character '{}'; allowed: letters, digits, '.', '_', '-'",
                ch
            );
        }
    }

    Ok(())
}

/// Validate a context key against the hierarchical key format.
///
/// Valid keys use `/` as a namespace separator with these rules:
/// - Allowed characters: `[a-zA-Z0-9._-/]`
/// - Must not start or end with `/`
/// - No consecutive slashes (`//`)
/// - No `.` or `..` path components
/// - Each component must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`
/// - Maximum total length: 255 characters
/// - Empty string rejected
pub fn validate_context_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() {
        anyhow::bail!("context key must not be empty");
    }

    if key.len() > 255 {
        anyhow::bail!(
            "context key exceeds maximum length of 255 characters (got {})",
            key.len()
        );
    }

    if key.starts_with('/') {
        anyhow::bail!("context key must not start with '/'");
    }

    if key.ends_with('/') {
        anyhow::bail!("context key must not end with '/'");
    }

    if key.contains("//") {
        anyhow::bail!("context key must not contain consecutive slashes");
    }

    for component in key.split('/') {
        if component == "." || component == ".." {
            anyhow::bail!("context key must not contain '.' or '..' path components");
        }

        if component.is_empty() {
            // Shouldn't happen given the checks above, but be defensive.
            anyhow::bail!("context key contains empty component");
        }

        let first = component.chars().next().unwrap();
        if !first.is_ascii_alphanumeric() {
            anyhow::bail!(
                "each component must start with a letter or digit, got '{}' in component '{}'",
                first,
                component
            );
        }

        for ch in component.chars().skip(1) {
            if !ch.is_ascii_alphanumeric() && ch != '.' && ch != '_' && ch != '-' {
                anyhow::bail!(
                    "context key contains invalid character '{}' in component '{}'; \
                     allowed: letters, digits, '.', '_', '-'",
                    ch,
                    component
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_name() {
        assert!(validate_session_id("myworkflow").is_ok());
    }

    #[test]
    fn accepts_name_with_hyphens() {
        assert!(validate_session_id("my-workflow").is_ok());
    }

    #[test]
    fn accepts_name_with_dots() {
        assert!(validate_session_id("my.workflow").is_ok());
    }

    #[test]
    fn accepts_name_with_underscores() {
        assert!(validate_session_id("my_workflow").is_ok());
    }

    #[test]
    fn accepts_mixed_case_and_digits() {
        assert!(validate_session_id("MyWorkflow2").is_ok());
    }

    #[test]
    fn accepts_single_letter() {
        assert!(validate_session_id("a").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_session_id("").is_err());
    }

    #[test]
    fn rejects_starting_with_digit() {
        assert!(validate_session_id("1abc").is_err());
    }

    #[test]
    fn rejects_starting_with_dot() {
        assert!(validate_session_id(".hidden").is_err());
    }

    #[test]
    fn rejects_starting_with_hyphen() {
        assert!(validate_session_id("-flag").is_err());
    }

    #[test]
    fn rejects_starting_with_underscore() {
        assert!(validate_session_id("_private").is_err());
    }

    #[test]
    fn rejects_dot_dot() {
        assert!(validate_session_id("..").is_err());
    }

    #[test]
    fn rejects_single_dot() {
        assert!(validate_session_id(".").is_err());
    }

    #[test]
    fn rejects_slash() {
        assert!(validate_session_id("a/b").is_err());
    }

    #[test]
    fn rejects_space() {
        assert!(validate_session_id("a b").is_err());
    }

    #[test]
    fn rejects_null_byte() {
        assert!(validate_session_id("a\0b").is_err());
    }

    // -- context key validation --

    #[test]
    fn ctx_key_accepts_flat_key() {
        assert!(validate_context_key("scope.md").is_ok());
    }

    #[test]
    fn ctx_key_accepts_hierarchical_key() {
        assert!(validate_context_key("research/r1/lead-cli-ux.md").is_ok());
    }

    #[test]
    fn ctx_key_accepts_alphanumeric_start() {
        assert!(validate_context_key("1file.txt").is_ok());
        assert!(validate_context_key("a").is_ok());
    }

    #[test]
    fn ctx_key_accepts_dots_hyphens_underscores() {
        assert!(validate_context_key("my_file.v2-final.md").is_ok());
    }

    #[test]
    fn ctx_key_rejects_empty() {
        assert!(validate_context_key("").is_err());
    }

    #[test]
    fn ctx_key_rejects_leading_slash() {
        assert!(validate_context_key("/scope.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_trailing_slash() {
        assert!(validate_context_key("scope.md/").is_err());
    }

    #[test]
    fn ctx_key_rejects_consecutive_slashes() {
        assert!(validate_context_key("research//r1.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_dot_component() {
        assert!(validate_context_key("research/./r1.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_dotdot_component() {
        assert!(validate_context_key("research/../secret.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_dotdot_standalone() {
        assert!(validate_context_key("..").is_err());
    }

    #[test]
    fn ctx_key_rejects_dot_standalone() {
        assert!(validate_context_key(".").is_err());
    }

    #[test]
    fn ctx_key_rejects_component_starting_with_dot() {
        assert!(validate_context_key(".hidden/file.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_component_starting_with_hyphen() {
        assert!(validate_context_key("-flag/file.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_space() {
        assert!(validate_context_key("my file.md").is_err());
    }

    #[test]
    fn ctx_key_rejects_over_255_chars() {
        let long_key = "a".repeat(256);
        assert!(validate_context_key(&long_key).is_err());
    }

    #[test]
    fn ctx_key_accepts_exactly_255_chars() {
        let key = "a".repeat(255);
        assert!(validate_context_key(&key).is_ok());
    }

    #[test]
    fn ctx_key_rejects_special_characters() {
        assert!(validate_context_key("file@name.md").is_err());
        assert!(validate_context_key("file name.md").is_err());
        assert!(validate_context_key("file\tname.md").is_err());
    }
}
