/// Validate a session ID against the allowlist pattern.
///
/// Valid IDs must start with a letter and contain only alphanumeric
/// characters, dots, underscores, hyphens, and tildes:
/// `^[a-zA-Z][a-zA-Z0-9._~-]*$`.
///
/// The tilde (`~`) is reserved for internal epoch-branch naming
/// (e.g., `parent~1.task-a` after a batch rewind). User-facing
/// workflow names are validated separately by
/// [`crate::discover::validate_workflow_name`], which rejects `~`.
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
        if !ch.is_ascii_alphanumeric() && ch != '.' && ch != '_' && ch != '-' && ch != '~' {
            anyhow::bail!(
                "session ID contains invalid character '{}'; allowed: letters, digits, '.', '_', '-', '~'",
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

/// The reason a context key is unusable, or `None` when it is usable.
///
/// Two callers ask this question about the same key and used to answer it in
/// different words -- a context gate before it hands the key to the store, and
/// `koto context exists` before it does the same. One wording is the whole point
/// of this function: the two surfaces print the same string for the same key, so
/// an operator learns what is wrong rather than which surface they are on, and a
/// third caller gets the answer by calling rather than by paraphrasing.
///
/// The message is therefore surface-neutral. It says nothing about gates,
/// because it is not always about a gate; [`validate_context_key`]'s own error
/// already names the offending character and the component it sits in, so the
/// reason carries that verbatim and adds only the part the caller cannot know:
/// that the two character sets do not line up, and that a `{{KEY}}` reference is
/// the usual way a key ends up holding something a key may not hold.
///
/// The asymmetry it describes is deliberate on both sides. A variable value is
/// content -- it reaches a directive, a command argument, a pattern -- and it
/// admits a space, a `:` and an `@` so it can hold a title or a filter
/// expression. A context key is an address: it becomes a path component on disk,
/// a key in the store's manifest, and an argument in the `koto context add` and
/// `koto context get` commands templates run. Widening the key grammar to close
/// the gap would legalize keys that word-split at that third use, so the two
/// stay apart and this function is how the boundary gets reported. The design
/// doc filed under Issue #227 records the reasoning and the alternatives.
pub fn unusable_context_key_reason(key: &str) -> Option<String> {
    let err = validate_context_key(key).err()?;
    Some(format!(
        "context key {:?} is not usable: {}\n  \
         remedy: a variable value may hold a space, ':' or '@'; a context key \
         may not. Where the key comes from a {{{{KEY}}}} reference, check what \
         that reference resolved to -- an unset optional variable leaves nothing \
         behind",
        key, err
    ))
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

    #[test]
    fn accepts_tilde_for_epoch_branches() {
        assert!(validate_session_id("parent~1.task-a").is_ok());
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

    // -- unusable_context_key_reason --
    //
    // The three characters below are the whole of the gap between what a
    // variable value may hold and what a context key may hold, so each gets its
    // own case: a reason that stopped naming one of them would leave an operator
    // with the same silence the function exists to end.

    #[test]
    fn reason_is_none_for_a_usable_key() {
        assert_eq!(unusable_context_key_reason("research/r1/lead.md"), None);
    }

    #[test]
    fn reason_names_a_space_and_the_component_it_sits_in() {
        let reason = unusable_context_key_reason("Weekly Planning-note")
            .expect("a space is not a legal context key character");
        assert!(
            reason.contains("' '"),
            "the reason should quote the offending character; got {reason}"
        );
        assert!(
            reason.contains("Weekly Planning-note"),
            "the reason should name the component; got {reason}"
        );
    }

    #[test]
    fn reason_names_a_colon() {
        let reason = unusable_context_key_reason("newer_than:90d-note")
            .expect("a colon is not a legal context key character");
        assert!(
            reason.contains("':'"),
            "the reason should quote the colon; got {reason}"
        );
    }

    #[test]
    fn reason_names_an_at_sign() {
        let reason = unusable_context_key_reason("user@example.com-note")
            .expect("an at-sign is not a legal context key character");
        assert!(
            reason.contains("'@'"),
            "the reason should quote the at-sign; got {reason}"
        );
    }

    /// A key one character away from a working one: a component must start with
    /// an alphanumeric, so `{{PREFIX}}-note` with nothing in `PREFIX` leaves
    /// `-note` while `{{PREFIX}}note` leaves `note` and works.
    #[test]
    fn reason_names_a_leading_hyphen() {
        let reason =
            unusable_context_key_reason("-note").expect("a component may not start with a hyphen");
        assert!(
            reason.contains("letter"),
            "the reason should say a component starts with a letter or digit; got {reason}"
        );
    }

    #[test]
    fn reason_says_a_key_that_resolved_to_nothing_is_empty() {
        let reason = unusable_context_key_reason("").expect("an empty key is not usable");
        assert!(
            reason.contains("empty"),
            "the reason should say the key is empty; got {reason}"
        );
    }

    /// Every reason ends with the same remedy, because the caller that prints it
    /// has no idea whether a reference produced the key and cannot add one.
    #[test]
    fn every_reason_carries_the_remedy() {
        for key in ["Weekly Planning-note", "a:b", "a@b", "-note", ""] {
            let reason = unusable_context_key_reason(key)
                .unwrap_or_else(|| panic!("{key:?} should be unusable"));
            assert!(
                reason.contains("remedy:"),
                "the reason for {key:?} should say what to change; got {reason}"
            );
            assert!(
                reason.contains("a context key may not"),
                "the reason for {key:?} should name the two grammars; got {reason}"
            );
        }
    }
}
