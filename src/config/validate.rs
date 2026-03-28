/// Keys allowed in project config (.koto/config.toml).
/// Credential keys are intentionally excluded to prevent secrets from being
/// committed to version control.
const PROJECT_ALLOWLIST: &[&str] = &[
    "session.backend",
    "session.cloud.endpoint",
    "session.cloud.bucket",
    "session.cloud.region",
];

/// Validate that a key is allowed in project config.
/// Returns an error message if the key is blocked.
pub fn validate_project_key(key: &str) -> Result<(), String> {
    if PROJECT_ALLOWLIST.contains(&key) {
        Ok(())
    } else if key == "session.cloud.access_key" || key == "session.cloud.secret_key" {
        Err(format!(
            "key '{}' contains credentials and cannot be stored in project config (use user config or env vars instead)",
            key
        ))
    } else {
        Err(format!("key '{}' is not allowed in project config", key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_keys() {
        assert!(validate_project_key("session.backend").is_ok());
        assert!(validate_project_key("session.cloud.endpoint").is_ok());
        assert!(validate_project_key("session.cloud.bucket").is_ok());
        assert!(validate_project_key("session.cloud.region").is_ok());
    }

    #[test]
    fn test_blocked_credential_keys() {
        let err = validate_project_key("session.cloud.access_key").unwrap_err();
        assert!(err.contains("credentials"));
        assert!(err.contains("cannot be stored in project config"));

        let err = validate_project_key("session.cloud.secret_key").unwrap_err();
        assert!(err.contains("credentials"));
    }

    #[test]
    fn test_unknown_key_blocked() {
        let err = validate_project_key("some.random.key").unwrap_err();
        assert!(err.contains("not allowed in project config"));
    }
}
