//! The shared name grammar for addressable members.
//!
//! Two things in koto let a caller name a member of a container: batch
//! task names and request leg names. Both use the grammar defined here.
//!
//! # Why this module is neutral
//!
//! The grammar lived in [`crate::engine::batch_validation`] first, but
//! that module imports its error vocabulary from the CLI layer, so an
//! engine-side consumer reaching for it would end up depending on CLI
//! types — an inversion. It would also silently inherit the batch
//! scheduler's reserved action words as forbidden names, which has no
//! justification outside the batch. So the grammar lives here, with no
//! dependencies beyond `regex`, and each caller layers its own
//! additional rules on top.
//!
//! # Why the grammar is this narrow
//!
//! A member name can become a **path component**: a batch task names a
//! child session directory. The character class admits no `/`, no `.`,
//! and no NUL, so neither `..` nor an absolute path can escape the
//! directory a caller-supplied name is joined to. Callers must run this
//! before using a name in a path.
//!
//! A leading hyphen is rejected separately. Member names are passed as
//! positional shell arguments, and a name like `--rationale` is an
//! argument-parsing hazard for the shell wrappers that drive koto.
//! koto's session-id validator rejects a leading hyphen for the same
//! reason.

use regex::Regex;

/// Maximum member-name length in bytes.
pub const MAX_NAME_LEN: usize = 64;

/// Why a member name failed the grammar.
///
/// Deliberately narrow: only the grammar failures, with no
/// container-specific notion of reserved words, so a caller does not
/// inherit another container's semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberNameError {
    /// Empty, or longer than [`MAX_NAME_LEN`]. Carries the observed length.
    LengthOutOfRange(usize),
    /// Contains a character outside `[A-Za-z0-9_-]`.
    RegexMismatch,
    /// Starts with `-`, which is ambiguous with a CLI flag.
    LeadingHyphen,
}

/// Build the member-name regex.
///
/// Compiled per call; the pattern is tiny and every call site validates
/// a bounded batch of names, so caching buys nothing measurable.
pub fn name_regex() -> Regex {
    Regex::new(r"^[A-Za-z0-9_-]+$")
        .expect("the member-name pattern is a constant and always parses")
}

/// Check one member name against the shared grammar.
///
/// A name must be 1..=[`MAX_NAME_LEN`] bytes, match `^[A-Za-z0-9_-]+$`,
/// and not begin with `-`.
///
/// Checks run in that order so an empty string reports a length failure
/// rather than a pattern failure, which is the more useful diagnostic.
pub fn validate_member_name(name: &str, name_re: &Regex) -> Result<(), MemberNameError> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(MemberNameError::LengthOutOfRange(name.len()));
    }
    if !name_re.is_match(name) {
        return Err(MemberNameError::RegexMismatch);
    }
    if name.starts_with('-') {
        return Err(MemberNameError::LeadingHyphen);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        let re = name_regex();
        for name in ["a", "reviewer-a", "reviewer_a", "Task1", "A1_b-2"] {
            assert!(
                validate_member_name(name, &re).is_ok(),
                "{name} should be accepted"
            );
        }
    }

    #[test]
    fn rejects_empty_and_overlong() {
        let re = name_regex();
        assert_eq!(
            validate_member_name("", &re),
            Err(MemberNameError::LengthOutOfRange(0))
        );
        let long = "a".repeat(MAX_NAME_LEN + 1);
        assert_eq!(
            validate_member_name(&long, &re),
            Err(MemberNameError::LengthOutOfRange(MAX_NAME_LEN + 1))
        );
        assert!(validate_member_name(&"a".repeat(MAX_NAME_LEN), &re).is_ok());
    }

    #[test]
    fn rejects_every_path_traversal_shape() {
        // This is the security-relevant case: a member name can become a
        // path component, so nothing that could escape a directory may
        // pass. Each of these must fail on the pattern, not by accident.
        let re = name_regex();
        for name in [
            "..",
            ".",
            "../etc/passwd",
            "a/b",
            "/absolute",
            "a\\b",
            "a\0b",
            "a.b",
            "a b",
            "a:b",
            "café",
            "\u{202e}gpj.exe",
        ] {
            assert!(
                validate_member_name(name, &re).is_err(),
                "{name:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_leading_hyphen_because_it_reads_as_a_flag() {
        let re = name_regex();
        for name in ["-", "-rf", "--rationale"] {
            assert_eq!(
                validate_member_name(name, &re),
                Err(MemberNameError::LeadingHyphen),
                "{name} must be rejected as flag-ambiguous"
            );
        }
        // A hyphen elsewhere is fine.
        assert!(validate_member_name("reviewer-a", &re).is_ok());
    }
}
