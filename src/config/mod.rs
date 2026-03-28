pub mod resolve;
pub mod validate;

use serde::{Deserialize, Serialize};

/// Top-level koto configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct KotoConfig {
    #[serde(default)]
    pub session: SessionConfig,
}

/// Session-related configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct SessionConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default)]
    pub cloud: CloudConfig,
}

/// Cloud storage configuration for session sync.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct CloudConfig {
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
}

fn default_backend() -> String {
    "local".to_string()
}

/// Dotted key paths supported by the config system.
/// Returns the value at a dotted key path from a resolved config.
pub fn get_value(config: &KotoConfig, key: &str) -> Option<String> {
    match key {
        "session.backend" => Some(config.session.backend.clone()),
        "session.cloud.endpoint" => config.session.cloud.endpoint.clone(),
        "session.cloud.bucket" => config.session.cloud.bucket.clone(),
        "session.cloud.region" => config.session.cloud.region.clone(),
        "session.cloud.access_key" => config.session.cloud.access_key.clone(),
        "session.cloud.secret_key" => config.session.cloud.secret_key.clone(),
        _ => None,
    }
}

/// Set a value at a dotted key path in a TOML table.
/// Returns an error if the key is not recognized.
pub fn set_value_in_toml(doc: &mut toml::Value, key: &str, value: &str) -> Result<(), String> {
    let table = doc.as_table_mut().ok_or("config is not a TOML table")?;

    match key {
        "session.backend" => {
            let session = table
                .entry("session")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let session_table = session.as_table_mut().ok_or("session is not a table")?;
            session_table.insert(
                "backend".to_string(),
                toml::Value::String(value.to_string()),
            );
        }
        "session.cloud.endpoint"
        | "session.cloud.bucket"
        | "session.cloud.region"
        | "session.cloud.access_key"
        | "session.cloud.secret_key" => {
            let field = key.strip_prefix("session.cloud.").unwrap();
            let session = table
                .entry("session")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let session_table = session.as_table_mut().ok_or("session is not a table")?;
            let cloud = session_table
                .entry("cloud")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let cloud_table = cloud.as_table_mut().ok_or("cloud is not a table")?;
            cloud_table.insert(field.to_string(), toml::Value::String(value.to_string()));
        }
        _ => return Err(format!("unknown config key: {}", key)),
    }
    Ok(())
}

/// Remove a value at a dotted key path from a TOML table.
/// Returns true if the key was present and removed.
pub fn unset_value_in_toml(doc: &mut toml::Value, key: &str) -> Result<bool, String> {
    let table = doc.as_table_mut().ok_or("config is not a TOML table")?;

    match key {
        "session.backend" => {
            if let Some(session) = table.get_mut("session") {
                if let Some(t) = session.as_table_mut() {
                    return Ok(t.remove("backend").is_some());
                }
            }
            Ok(false)
        }
        "session.cloud.endpoint"
        | "session.cloud.bucket"
        | "session.cloud.region"
        | "session.cloud.access_key"
        | "session.cloud.secret_key" => {
            let field = key.strip_prefix("session.cloud.").unwrap();
            if let Some(session) = table.get_mut("session") {
                if let Some(st) = session.as_table_mut() {
                    if let Some(cloud) = st.get_mut("cloud") {
                        if let Some(ct) = cloud.as_table_mut() {
                            return Ok(ct.remove(field).is_some());
                        }
                    }
                }
            }
            Ok(false)
        }
        _ => Err(format!("unknown config key: {}", key)),
    }
}

/// All valid config key paths.
pub const ALL_KEYS: &[&str] = &[
    "session.backend",
    "session.cloud.endpoint",
    "session.cloud.bucket",
    "session.cloud.region",
    "session.cloud.access_key",
    "session.cloud.secret_key",
];

/// Produce a redacted copy of the config for display.
/// Credential values are replaced with "<set>" if present.
pub fn redact(config: &KotoConfig) -> KotoConfig {
    let mut redacted = config.clone();
    if redacted.session.cloud.access_key.is_some() {
        redacted.session.cloud.access_key = Some("<set>".to_string());
    }
    if redacted.session.cloud.secret_key.is_some() {
        redacted.session.cloud.secret_key = Some("<set>".to_string());
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KotoConfig::default();
        // Default backend is empty string from Default trait (not the serde default).
        // The serde default only applies during deserialization.
        assert_eq!(config.session.backend, "");
        assert!(config.session.cloud.endpoint.is_none());
    }

    #[test]
    fn test_get_value() {
        let mut config = KotoConfig::default();
        config.session.backend = "cloud".to_string();
        config.session.cloud.bucket = Some("my-bucket".to_string());

        assert_eq!(
            get_value(&config, "session.backend"),
            Some("cloud".to_string())
        );
        assert_eq!(
            get_value(&config, "session.cloud.bucket"),
            Some("my-bucket".to_string())
        );
        assert_eq!(get_value(&config, "session.cloud.endpoint"), None);
        assert_eq!(get_value(&config, "nonexistent"), None);
    }

    #[test]
    fn test_set_value_in_toml() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "session.backend", "cloud").unwrap();
        set_value_in_toml(&mut doc, "session.cloud.bucket", "my-bucket").unwrap();

        let config: KotoConfig = doc.try_into().unwrap();
        assert_eq!(config.session.backend, "cloud");
        assert_eq!(config.session.cloud.bucket, Some("my-bucket".to_string()));
    }

    #[test]
    fn test_set_value_unknown_key() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let result = set_value_in_toml(&mut doc, "nonexistent.key", "value");
        assert!(result.is_err());
    }

    #[test]
    fn test_unset_value_in_toml() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "session.backend", "cloud").unwrap();
        let removed = unset_value_in_toml(&mut doc, "session.backend").unwrap();
        assert!(removed);

        // Removing again returns false.
        let removed = unset_value_in_toml(&mut doc, "session.backend").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_redact() {
        let mut config = KotoConfig::default();
        config.session.cloud.access_key = Some("AKIAIOSFODNN7EXAMPLE".to_string());
        config.session.cloud.secret_key = Some("secret123".to_string());

        let redacted = redact(&config);
        assert_eq!(redacted.session.cloud.access_key, Some("<set>".to_string()));
        assert_eq!(redacted.session.cloud.secret_key, Some("<set>".to_string()));
    }

    #[test]
    fn test_redact_unset_credentials() {
        let config = KotoConfig::default();
        let redacted = redact(&config);
        assert!(redacted.session.cloud.access_key.is_none());
        assert!(redacted.session.cloud.secret_key.is_none());
    }
}
