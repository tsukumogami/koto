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
}
