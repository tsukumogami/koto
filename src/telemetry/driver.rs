//! The Claude Code session driving this koto process.

use crate::host_env::{host_env, CLAUDE_SESSION_ID_ENV};

/// The driving Claude Code session's id, from `CLAUDE_CODE_SESSION_ID`.
///
/// `None` when the variable is unset, or when its value is not id-shaped
/// (1 to 128 characters from `A-Z a-z 0-9 . _ : -`): a malformed value
/// counts as no driver, so nothing outside that shape is ever recorded.
/// The value is read through the shared [`host_env`] seam, the same one the
/// `/workflows` renderer uses.
pub(crate) fn driver() -> Option<String> {
    host_env(CLAUDE_SESSION_ID_ENV).filter(|v| super::is_id(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_env::set_host_env_for_test;

    fn with(value: Option<&str>) -> Option<String> {
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, value);
        let d = driver();
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, None);
        d
    }

    #[test]
    fn an_id_shaped_value_is_the_driver() {
        assert_eq!(
            with(Some("3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60718")).as_deref(),
            Some("3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60718")
        );
        let max = "a".repeat(128);
        assert_eq!(with(Some(&max)), Some(max.clone()));
    }

    #[test]
    fn unset_empty_and_malformed_values_are_no_driver() {
        assert_eq!(with(None), None);
        assert_eq!(with(Some("")), None);
        assert_eq!(with(Some("abc\ndef")), None);
        assert_eq!(with(Some(" abc")), None);
        assert_eq!(with(Some("a/b")), None);
        assert_eq!(with(Some(&"a".repeat(129))), None);
        assert_eq!(with(Some(&"a".repeat(200))), None);
    }
}
