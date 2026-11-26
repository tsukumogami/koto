//! The machine-name rule every source and reader of a session shares.
//! The rest of koto's `session::version` (version tracking and machine
//! identity) writes, and stays in koto.

/// The longest machine name koto records.
pub const MACHINE_NAME_MAX: usize = 64;

/// The rule a machine name follows, as people read it in a message.
pub const MACHINE_NAME_RULE: &str =
    "a machine name is 1 to 64 characters of A-Z, a-z, 0-9, '.', '_' or '-', and is not 'unknown'";

/// Check a machine name against the one rule every source and reader
/// shares: 1 to [`MACHINE_NAME_MAX`] characters of `[A-Za-z0-9._-]`, never
/// `unknown` (reserved for sessions that record no machine). The error names
/// the rule, never the value.
pub fn validate_machine_name(name: &str) -> std::result::Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= MACHINE_NAME_MAX
        && name != "unknown"
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(MACHINE_NAME_RULE.to_string())
    }
}
