pub mod resolve;
pub mod validate;

use serde::{Deserialize, Serialize};

/// Top-level koto configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct KotoConfig {
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub request_store: RequestStoreConfig,
    #[serde(default)]
    pub workflows: WorkflowsConfig,
}

/// Native Claude Code `/workflows` rendering configuration.
///
/// `native` controls whether koto sessions render in Claude Code's `/workflows`
/// screen. When true, a session driven inside a Claude Code session
/// self-discovers that session's workflows directory from the
/// `CLAUDE_CODE_SESSION_ID` environment variable and renders into it -- no
/// SessionStart hook or plugin required. **Defaults to true (on).** A session
/// with no discoverable Claude Code environment (fully headless) still renders
/// nothing, so the default path is untouched there. To opt out, set
/// `koto config set workflows.native false --user`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WorkflowsConfig {
    #[serde(default = "default_native")]
    pub native: bool,
}

impl Default for WorkflowsConfig {
    fn default() -> Self {
        Self {
            native: default_native(),
        }
    }
}

fn default_native() -> bool {
    true
}

/// Request-store (coordinator/operator) configuration.
///
/// Carries the eight operator-tunable dimensions from Decision 4 of
/// DESIGN-koto-request-store. Each field has a built-in default and
/// is independently overridable via TOML, env-var, or (for
/// `redelegation_cap`) a per-tick CLI flag on `koto next`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RequestStoreConfig {
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
    /// supplied under `[request_store.recursion]` are accepted by the
    /// parser but emit a warn-level log on `koto config get` and `koto
    /// next` startup so operators get a clear signal that the namespace
    /// is reserved.
    #[serde(default)]
    pub recursion: Option<toml::Value>,
}

impl Default for RequestStoreConfig {
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
        "request_store.stale_claim_timeout_seconds" => {
            Some(config.request_store.stale_claim_timeout_seconds.to_string())
        }
        "request_store.stale_dispatch_timeout_seconds" => Some(
            config
                .request_store
                .stale_dispatch_timeout_seconds
                .to_string(),
        ),
        "request_store.redelegation_cap" => Some(config.request_store.redelegation_cap.to_string()),
        "request_store.coord_cursor_ttl_days" => {
            Some(config.request_store.coord_cursor_ttl_days.to_string())
        }
        "request_store.terminal_index_compact_lines" => Some(
            config
                .request_store
                .terminal_index_compact_lines
                .to_string(),
        ),
        "request_store.compact_lock_timeout_seconds" => Some(
            config
                .request_store
                .compact_lock_timeout_seconds
                .to_string(),
        ),
        "request_store.directive_batch_size" => {
            Some(config.request_store.directive_batch_size.to_string())
        }
        "request_store.respawn_generation_cap" => {
            Some(config.request_store.respawn_generation_cap.to_string())
        }
        "workflows.native" => Some(config.workflows.native.to_string()),
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
        "request_store.stale_claim_timeout_seconds"
        | "request_store.stale_dispatch_timeout_seconds"
        | "request_store.coord_cursor_ttl_days"
        | "request_store.terminal_index_compact_lines"
        | "request_store.compact_lock_timeout_seconds"
        | "request_store.directive_batch_size"
        | "request_store.respawn_generation_cap"
        | "request_store.redelegation_cap" => {
            let field = key.strip_prefix("request_store.").unwrap();
            let parsed: i64 = value
                .parse()
                .map_err(|e| format!("value for {} must parse as an integer: {}", key, e))?;
            if parsed < 0 {
                return Err(format!(
                    "value for {} must be non-negative (got {})",
                    key, parsed
                ));
            }
            let rs = table
                .entry("request_store")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let rs_table = rs.as_table_mut().ok_or("request_store is not a table")?;
            rs_table.insert(field.to_string(), toml::Value::Integer(parsed));
        }
        k if k.starts_with("request_store.recursion.") => {
            // The recursion namespace is reserved-but-ignored at V1
            // (the runtime caps are hard-coded in src/engine/caps.rs).
            // We still let operators write under it so they can stage
            // intended values for the V1.1 promotion. Values are
            // parsed as integers when they look like integers, falling
            // back to a string write otherwise; the resolver emits a
            // warn-level log on read.
            let field = k.strip_prefix("request_store.recursion.").unwrap();
            if field.is_empty() || field.contains('.') {
                return Err(format!("unknown config key: {}", key));
            }
            let toml_value = match value.parse::<i64>() {
                Ok(n) => toml::Value::Integer(n),
                Err(_) => toml::Value::String(value.to_string()),
            };
            let rs = table
                .entry("request_store")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let rs_table = rs.as_table_mut().ok_or("request_store is not a table")?;
            let recursion = rs_table
                .entry("recursion")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let recursion_table = recursion
                .as_table_mut()
                .ok_or("request_store.recursion is not a table")?;
            recursion_table.insert(field.to_string(), toml_value);
        }
        "workflows.native" => {
            let parsed: bool = value
                .parse()
                .map_err(|_| format!("value for {} must be true or false", key))?;
            let workflows = table
                .entry("workflows")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            let workflows_table = workflows.as_table_mut().ok_or("workflows is not a table")?;
            workflows_table.insert("native".to_string(), toml::Value::Boolean(parsed));
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
        "request_store.stale_claim_timeout_seconds"
        | "request_store.stale_dispatch_timeout_seconds"
        | "request_store.coord_cursor_ttl_days"
        | "request_store.terminal_index_compact_lines"
        | "request_store.compact_lock_timeout_seconds"
        | "request_store.directive_batch_size"
        | "request_store.respawn_generation_cap"
        | "request_store.redelegation_cap" => {
            let field = key.strip_prefix("request_store.").unwrap();
            if let Some(rs) = table.get_mut("request_store") {
                if let Some(rs_table) = rs.as_table_mut() {
                    return Ok(rs_table.remove(field).is_some());
                }
            }
            Ok(false)
        }
        k if k.starts_with("request_store.recursion.") => {
            let field = k.strip_prefix("request_store.recursion.").unwrap();
            if field.is_empty() || field.contains('.') {
                return Err(format!("unknown config key: {}", key));
            }
            if let Some(rs) = table.get_mut("request_store") {
                if let Some(rs_table) = rs.as_table_mut() {
                    if let Some(recursion) = rs_table.get_mut("recursion") {
                        if let Some(recursion_table) = recursion.as_table_mut() {
                            return Ok(recursion_table.remove(field).is_some());
                        }
                    }
                }
            }
            Ok(false)
        }
        "workflows.native" => {
            if let Some(workflows) = table.get_mut("workflows") {
                if let Some(t) = workflows.as_table_mut() {
                    return Ok(t.remove("native").is_some());
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
    "request_store.stale_claim_timeout_seconds",
    "request_store.stale_dispatch_timeout_seconds",
    "request_store.redelegation_cap",
    "request_store.coord_cursor_ttl_days",
    "request_store.terminal_index_compact_lines",
    "request_store.compact_lock_timeout_seconds",
    "request_store.directive_batch_size",
    "request_store.respawn_generation_cap",
    "workflows.native",
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

/// Emit a warn-level log to stderr if the `[request_store.recursion]`
/// table is populated in the resolved config. Called from `koto config
/// get` and `koto next` startup. Silent when the namespace is absent.
pub fn warn_if_request_store_recursion_reserved(config: &KotoConfig) {
    if config.request_store.recursion.is_some() {
        eprintln!(
            "warning: [request_store.recursion] is a reserved-but-ignored namespace at V1; \
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
    fn test_request_store_config_defaults() {
        let rs = RequestStoreConfig::default();
        assert_eq!(rs.stale_claim_timeout_seconds, 600);
        assert_eq!(rs.stale_dispatch_timeout_seconds, 600);
        assert_eq!(rs.redelegation_cap, 3);
        assert_eq!(rs.coord_cursor_ttl_days, 7);
        assert_eq!(rs.terminal_index_compact_lines, 100_000);
        assert_eq!(rs.compact_lock_timeout_seconds, 3600);
        assert_eq!(rs.directive_batch_size, 50);
        assert_eq!(rs.respawn_generation_cap, 2);
        assert!(rs.recursion.is_none());
    }

    #[test]
    fn test_request_store_config_partial_toml_uses_defaults() {
        let toml_str = "[request_store]\nredelegation_cap = 5\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.request_store.redelegation_cap, 5);
        // Other dimensions stay at their built-in defaults.
        assert_eq!(cfg.request_store.stale_claim_timeout_seconds, 600);
        assert_eq!(cfg.request_store.directive_batch_size, 50);
    }

    #[test]
    fn test_request_store_recursion_table_parses_without_error() {
        let toml_str = "[request_store.recursion]\nmax_depth_soft = 7\nmax_depth_hard = 20\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.request_store.recursion.is_some());
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

    // -----------------------------------------------------------------------
    // request_store.* set/unset coverage (Fix 4)
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_value_request_store_redelegation_cap() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "request_store.redelegation_cap", "5").unwrap();
        let cfg: KotoConfig = doc.try_into().unwrap();
        assert_eq!(cfg.request_store.redelegation_cap, 5);
    }

    #[test]
    fn test_set_value_request_store_all_top_level_dimensions() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(
            &mut doc,
            "request_store.stale_claim_timeout_seconds",
            "1200",
        )
        .unwrap();
        set_value_in_toml(
            &mut doc,
            "request_store.stale_dispatch_timeout_seconds",
            "900",
        )
        .unwrap();
        set_value_in_toml(&mut doc, "request_store.coord_cursor_ttl_days", "14").unwrap();
        set_value_in_toml(
            &mut doc,
            "request_store.terminal_index_compact_lines",
            "50000",
        )
        .unwrap();
        set_value_in_toml(
            &mut doc,
            "request_store.compact_lock_timeout_seconds",
            "7200",
        )
        .unwrap();
        set_value_in_toml(&mut doc, "request_store.directive_batch_size", "100").unwrap();
        set_value_in_toml(&mut doc, "request_store.respawn_generation_cap", "3").unwrap();
        let cfg: KotoConfig = doc.try_into().unwrap();
        assert_eq!(cfg.request_store.stale_claim_timeout_seconds, 1200);
        assert_eq!(cfg.request_store.stale_dispatch_timeout_seconds, 900);
        assert_eq!(cfg.request_store.coord_cursor_ttl_days, 14);
        assert_eq!(cfg.request_store.terminal_index_compact_lines, 50_000);
        assert_eq!(cfg.request_store.compact_lock_timeout_seconds, 7200);
        assert_eq!(cfg.request_store.directive_batch_size, 100);
        assert_eq!(cfg.request_store.respawn_generation_cap, 3);
    }

    #[test]
    fn test_set_value_request_store_recursion_nested_namespace() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "request_store.recursion.max_depth", "10").unwrap();
        set_value_in_toml(&mut doc, "request_store.recursion.max_breadth", "20").unwrap();
        let cfg: KotoConfig = doc.try_into().unwrap();
        // The recursion table parses into Option<toml::Value> by Decision 4;
        // we assert it's present + carries the two keys.
        let recursion = cfg
            .request_store
            .recursion
            .as_ref()
            .expect("recursion table must parse");
        let recursion_table = recursion.as_table().expect("recursion is a table");
        assert_eq!(
            recursion_table
                .get("max_depth")
                .and_then(|v| v.as_integer()),
            Some(10)
        );
        assert_eq!(
            recursion_table
                .get("max_breadth")
                .and_then(|v| v.as_integer()),
            Some(20)
        );
    }

    #[test]
    fn test_set_value_request_store_rejects_non_integer() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let err = set_value_in_toml(&mut doc, "request_store.redelegation_cap", "five")
            .expect_err("string must reject");
        assert!(err.contains("must parse as an integer"));
    }

    #[test]
    fn test_set_value_request_store_rejects_negative() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let err = set_value_in_toml(&mut doc, "request_store.redelegation_cap", "-1")
            .expect_err("negative must reject");
        assert!(err.contains("non-negative"));
    }

    #[test]
    fn test_set_value_request_store_bogus_key_rejects() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let err = set_value_in_toml(&mut doc, "request_store.bogus", "5")
            .expect_err("unknown key must reject");
        assert!(err.contains("unknown config key"));
    }

    #[test]
    fn test_set_value_request_store_recursion_deep_nesting_rejects() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let err = set_value_in_toml(&mut doc, "request_store.recursion.a.b", "5")
            .expect_err("deeper nesting must reject");
        assert!(err.contains("unknown config key"));
    }

    #[test]
    fn test_get_value_request_store_after_set() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "request_store.redelegation_cap", "7").unwrap();
        let cfg: KotoConfig = doc.try_into().unwrap();
        assert_eq!(
            get_value(&cfg, "request_store.redelegation_cap"),
            Some("7".to_string())
        );
    }

    #[test]
    fn test_unset_value_request_store_round_trips() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "request_store.redelegation_cap", "5").unwrap();
        let removed = unset_value_in_toml(&mut doc, "request_store.redelegation_cap").unwrap();
        assert!(removed);
        let again = unset_value_in_toml(&mut doc, "request_store.redelegation_cap").unwrap();
        assert!(!again, "second unset returns false");
    }

    #[test]
    fn test_unset_value_request_store_recursion() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "request_store.recursion.max_depth", "10").unwrap();
        let removed = unset_value_in_toml(&mut doc, "request_store.recursion.max_depth").unwrap();
        assert!(removed);
    }

    // -----------------------------------------------------------------------
    // workflows.native opt-in coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_workflows_native_defaults_true() {
        let config = KotoConfig::default();
        assert!(config.workflows.native);
        assert_eq!(
            get_value(&config, "workflows.native"),
            Some("true".to_string())
        );
    }

    #[test]
    fn test_set_and_get_workflows_native() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "workflows.native", "true").unwrap();
        let cfg: KotoConfig = doc.try_into().unwrap();
        assert!(cfg.workflows.native);
        assert_eq!(
            get_value(&cfg, "workflows.native"),
            Some("true".to_string())
        );
    }

    #[test]
    fn test_workflows_native_partial_toml_uses_defaults() {
        // A config that only sets request_store leaves workflows.native at its
        // default (on).
        let toml_str = "[request_store]\nredelegation_cap = 5\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.workflows.native);
    }

    #[test]
    fn test_workflows_native_explicit_optout_parses() {
        // Opting out is an explicit `native = false`.
        let toml_str = "[workflows]\nnative = false\n";
        let cfg: KotoConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.workflows.native);
        assert_eq!(
            get_value(&cfg, "workflows.native"),
            Some("false".to_string())
        );
    }

    #[test]
    fn test_set_workflows_native_rejects_non_bool() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        let err = set_value_in_toml(&mut doc, "workflows.native", "yes")
            .expect_err("non-bool must reject");
        assert!(err.contains("must be true or false"));
    }

    #[test]
    fn test_unset_workflows_native_round_trips() {
        let mut doc = toml::Value::Table(toml::map::Map::new());
        set_value_in_toml(&mut doc, "workflows.native", "true").unwrap();
        assert!(unset_value_in_toml(&mut doc, "workflows.native").unwrap());
        assert!(!unset_value_in_toml(&mut doc, "workflows.native").unwrap());
    }

    #[test]
    fn test_workflows_native_in_all_keys() {
        assert!(ALL_KEYS.contains(&"workflows.native"));
    }
}
