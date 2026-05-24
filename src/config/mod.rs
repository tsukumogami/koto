pub mod resolve;
pub mod validate;

use serde::{Deserialize, Serialize};

/// Top-level koto configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct KotoConfig {
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub kt1: Kt1Config,
}

/// KT1 (coordinator/operator) configuration.
///
/// Carries the eight operator-tunable dimensions from Decision 4 of
/// DESIGN-koto-request-store. Each field has a built-in default and
/// is independently overridable via TOML, env-var, or (for
/// `redelegation_cap`) a per-tick CLI flag on `koto next`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Kt1Config {
    #[serde(default = "default_stale_claim_timeout_seconds")]
    pub stale_claim_timeout_seconds: u64,
    #[serde(default = "default_stale_dispatch_timeout_seconds")]
    pub stale_dispatch_timeout_seconds: u64,
    #[serde(default = "default_redelegation_cap")]
    pub redelegation_cap: u32,
    #[serde(default = "default_coord_cursor_ttl_days")]
    pub coord_cursor_ttl_days: u32,
    #[serde(default = "default_terminal_index_compact_lines")]
    pub terminal_index_compact_lines: u64,
    #[serde(default = "default_compact_lock_timeout_seconds")]
    pub compact_lock_timeout_seconds: u64,
    #[serde(default = "default_directive_batch_size")]
    pub directive_batch_size: u32,
    #[serde(default = "default_respawn_generation_cap")]
    pub respawn_generation_cap: u32,
    /// Reserved-but-ignored namespace for V1.1 recursion-cap promotion.
    /// V1 runtime caps are hard-coded in `src/engine/caps.rs`; values
    /// supplied under `[kt1.recursion]` are accepted by the parser but
    /// emit a warn-level log on `koto config get` and `koto next`
    /// startup so operators get a clear signal that the namespace is
    /// reserved.
    #[serde(default)]
    pub recursion: Option<toml::Value>,
}

impl Default for Kt1Config {
    fn default() -> Self {
        Self {
            stale_claim_timeout_seconds: default_stale_claim_timeout_seconds(),
            stale_dispatch_timeout_seconds: default_stale_dispatch_timeout_seconds(),
            redelegation_cap: default_redelegation_cap(),
            coord_cursor_ttl_days: default_coord_cursor_ttl_days(),
            terminal_index_compact_lines: default_terminal_index_compact_lines(),
            compact_lock_timeout_seconds: default_compact_lock_timeout_seconds(),
            directive_batch_size: default_directive_batch_size(),
            respawn_generation_cap: default_respawn_generation_cap(),
            recursion: None,
        }
    }
}

fn default_stale_claim_timeout_seconds() -> u64 {
    600
}
fn default_stale_dispatch_timeout_seconds() -> u64 {
    600
}
fn default_redelegation_cap() -> u32 {
    3
}
fn default_coord_cursor_ttl_days() -> u32 {
    7
}
fn default_terminal_index_compact_lines() -> u64 {
    100_000
}
fn default_compact_lock_timeout_seconds() -> u64 {
    3600
}
fn default_directive_batch_size() -> u32 {
    50
}
fn default_respawn_generation_cap() -> u32 {
    2
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
        "kt1.stale_claim_timeout_seconds" => {
            Some(config.kt1.stale_claim_timeout_seconds.to_string())
        }
        "kt1.stale_dispatch_timeout_seconds" => {
            Some(config.kt1.stale_dispatch_timeout_seconds.to_string())
        }
        "kt1.redelegation_cap" => Some(config.kt1.redelegation_cap.to_string()),
        "kt1.coord_cursor_ttl_days" => Some(config.kt1.coord_cursor_ttl_days.to_string()),
        "kt1.terminal_index_compact_lines" => {
            Some(config.kt1.terminal_index_compact_lines.to_string())
        }
        "kt1.compact_lock_timeout_seconds" => {
            Some(config.kt1.compact_lock_timeout_seconds.to_string())
        }
        "kt1.directive_batch_size" => Some(config.kt1.directive_batch_size.to_string()),
        "kt1.respawn_generation_cap" => Some(config.kt1.respawn_generation_cap.to_string()),
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
    "kt1.stale_claim_timeout_seconds",
    "kt1.stale_dispatch_timeout_seconds",
    "kt1.redelegation_cap",
    "kt1.coord_cursor_ttl_days",
    "kt1.terminal_index_compact_lines",
    "kt1.compact_lock_timeout_seconds",
    "kt1.directive_batch_size",
    "kt1.respawn_generation_cap",
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

/// Emit a warn-level log to stderr if the `[kt1.recursion]` table is
/// populated in the resolved config. Called from `koto config get` and
/// `koto next` startup. Silent when the namespace is absent.
pub fn warn_if_kt1_recursion_reserved(config: &KotoConfig) {
    if config.kt1.recursion.is_some() {
        eprintln!(
            "warning: [kt1.recursion] is a reserved-but-ignored namespace at V1; \
             values supplied here have no effect (runtime recursion caps are hard-coded). \
             The table is pre-staked for future operator-tunable promotion."
        );
    }
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
    fn test_kt1_config_defaults() {
        let kt1 = Kt1Config::default();
        assert_eq!(kt1.stale_claim_timeout_seconds, 600);
        assert_eq!(kt1.stale_dispatch_timeout_seconds, 600);
        assert_eq!(kt1.redelegation_cap, 3);
        assert_eq!(kt1.coord_cursor_ttl_days, 7);
        assert_eq!(kt1.terminal_index_compact_lines, 100_000);
        assert_eq!(kt1.compact_lock_timeout_seconds, 3600);
        assert_eq!(kt1.directive_batch_size, 50);
        assert_eq!(kt1.respawn_generation_cap, 2);
        assert!(kt1.recursion.is_none());
    }

    #[test]
    fn test_kt1_config_partial_toml_uses_defaults() {
        let toml_str = "[kt1]\nredelegation_cap = 5\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.kt1.redelegation_cap, 5);
        // Other dimensions stay at their built-in defaults.
        assert_eq!(cfg.kt1.stale_claim_timeout_seconds, 600);
        assert_eq!(cfg.kt1.directive_batch_size, 50);
    }

    #[test]
    fn test_kt1_recursion_table_parses_without_error() {
        let toml_str = "[kt1.recursion]\nmax_depth_soft = 7\nmax_depth_hard = 20\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.kt1.recursion.is_some());
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
