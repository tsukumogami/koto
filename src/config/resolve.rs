use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{KotoConfig, Kt1Config};

/// Override values for the `kt1` config block that come from outside
/// the layered config files. Each `None` means "no override at this
/// layer", letting `kt1_config()` evaluate the precedence cascade in
/// one place.
///
/// The fields mirror `Kt1Config` one-to-one so a future operator-
/// tunable dimension can be added by extending both structs together.
#[derive(Debug, Default, Clone)]
pub struct Kt1Overrides {
    pub stale_claim_timeout_seconds: Option<u64>,
    pub stale_dispatch_timeout_seconds: Option<u64>,
    pub redelegation_cap: Option<u32>,
    pub coord_cursor_ttl_days: Option<u32>,
    pub terminal_index_compact_lines: Option<u64>,
    pub compact_lock_timeout_seconds: Option<u64>,
    pub directive_batch_size: Option<u32>,
    pub respawn_generation_cap: Option<u32>,
}

/// Load and merge configuration from all sources.
///
/// Precedence (highest to lowest):
/// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY,
///    `KOTO_KT1_*` for KT1 dimensions)
/// 2. Project config (.koto/config.toml in current directory)
/// 3. User config (~/.koto/config.toml)
/// 4. Built-in defaults
///
/// The CLI-flag layer (the highest tier of the 5-level cascade) is
/// applied per-tick in `kt1_config()`, not here -- `load_config()` has
/// no access to per-command argv. Callers that need a `Kt1Config`
/// resolved against a CLI flag should call [`kt1_config`] with the
/// base returned here.
pub fn load_config() -> Result<KotoConfig> {
    let mut config = KotoConfig::default();
    // Apply serde default for backend since Default trait gives empty string.
    config.session.backend = "local".to_string();

    // Layer 1: user config
    if let Some(user_path) = user_config_path() {
        if user_path.exists() {
            let user_config = load_config_file(&user_path)
                .with_context(|| format!("loading user config from {}", user_path.display()))?;
            merge_config(&mut config, &user_config);
        }
    }

    // Layer 2: project config
    let project_path = project_config_path();
    if project_path.exists() {
        let project_config = load_config_file(&project_path)
            .with_context(|| format!("loading project config from {}", project_path.display()))?;
        merge_config(&mut config, &project_config);
    }

    // Layer 3: env var overrides for credentials
    if let Ok(val) = env::var("AWS_ACCESS_KEY_ID") {
        config.session.cloud.access_key = Some(val);
    }
    if let Ok(val) = env::var("AWS_SECRET_ACCESS_KEY") {
        config.session.cloud.secret_key = Some(val);
    }

    // Layer 3b: KOTO_KT1_* env-var overrides for the kt1 block.
    apply_kt1_env_overrides(&mut config.kt1);

    Ok(config)
}

/// Resolve `Kt1Config` through the full 5-level precedence cascade:
///
///   CLI flag > env-var > project config > user config > built-in default
///
/// `base` is the `Kt1Config` already produced by [`load_config`] (which
/// has merged the file layers and applied `KOTO_KT1_*` env-var
/// overrides). `cli` carries the per-tick CLI-flag overrides; on a
/// `koto next` invocation only `redelegation_cap` is settable today.
pub fn kt1_config(base: &Kt1Config, cli: &Kt1Overrides) -> Kt1Config {
    let mut out = base.clone();
    if let Some(v) = cli.stale_claim_timeout_seconds {
        out.stale_claim_timeout_seconds = v;
    }
    if let Some(v) = cli.stale_dispatch_timeout_seconds {
        out.stale_dispatch_timeout_seconds = v;
    }
    if let Some(v) = cli.redelegation_cap {
        out.redelegation_cap = v;
    }
    if let Some(v) = cli.coord_cursor_ttl_days {
        out.coord_cursor_ttl_days = v;
    }
    if let Some(v) = cli.terminal_index_compact_lines {
        out.terminal_index_compact_lines = v;
    }
    if let Some(v) = cli.compact_lock_timeout_seconds {
        out.compact_lock_timeout_seconds = v;
    }
    if let Some(v) = cli.directive_batch_size {
        out.directive_batch_size = v;
    }
    if let Some(v) = cli.respawn_generation_cap {
        out.respawn_generation_cap = v;
    }
    out
}

/// Apply `KOTO_KT1_*` env-var overrides to a `Kt1Config` in place.
///
/// Env-var key spellings come from DESIGN-koto-request-store Decision 4.
/// Unset vars leave the field untouched. Malformed integer values are
/// silently ignored (matches the existing `AWS_*` env-var behavior).
fn apply_kt1_env_overrides(kt1: &mut Kt1Config) {
    if let Some(v) = env_parse::<u64>("KOTO_KT1_STALE_CLAIM_TIMEOUT_S") {
        kt1.stale_claim_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_KT1_STALE_DISPATCH_TIMEOUT_S") {
        kt1.stale_dispatch_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_KT1_REDELEGATION_CAP") {
        kt1.redelegation_cap = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_KT1_COORD_CURSOR_TTL_DAYS") {
        kt1.coord_cursor_ttl_days = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_KT1_TERMINAL_INDEX_COMPACT_LINES") {
        kt1.terminal_index_compact_lines = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_KT1_COMPACT_LOCK_TIMEOUT_S") {
        kt1.compact_lock_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_KT1_DIRECTIVE_BATCH_SIZE") {
        kt1.directive_batch_size = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_KT1_RESPAWN_GENERATION_CAP") {
        kt1.respawn_generation_cap = v;
    }
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    env::var(key).ok().and_then(|v| v.parse::<T>().ok())
}

/// Load a TOML config file and deserialize it. The kt1 block is parsed
/// separately from `KotoConfig` so we can distinguish "field present in
/// the source file" from "field defaulted by serde" -- the merge step
/// only overlays explicitly-set fields onto the target.
fn load_config_file(path: &Path) -> Result<LoadedConfig> {
    let content = fs::read_to_string(path)?;
    let config: KotoConfig = toml::from_str(&content)?;
    let raw: toml::Value = content.parse()?;
    let kt1_keys = raw
        .as_table()
        .and_then(|t| t.get("kt1"))
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter(|(_, v)| !v.is_table())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let kt1_has_recursion = raw
        .as_table()
        .and_then(|t| t.get("kt1"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("recursion"))
        .is_some();
    Ok(LoadedConfig {
        config,
        kt1_keys,
        kt1_has_recursion,
    })
}

/// A loaded config plus metadata about which kt1 fields the source file
/// actually set. Drives the layered merge step.
struct LoadedConfig {
    config: KotoConfig,
    kt1_keys: Vec<String>,
    kt1_has_recursion: bool,
}

/// Merge source config into target. Non-default/non-empty values in source
/// overwrite target. For `kt1`, only fields that were explicitly present
/// in the source file are overlaid (serde defaults are not "values").
fn merge_config(target: &mut KotoConfig, source: &LoadedConfig) {
    if !source.config.session.backend.is_empty() {
        target.session.backend = source.config.session.backend.clone();
    }
    if source.config.session.cloud.endpoint.is_some() {
        target.session.cloud.endpoint = source.config.session.cloud.endpoint.clone();
    }
    if source.config.session.cloud.bucket.is_some() {
        target.session.cloud.bucket = source.config.session.cloud.bucket.clone();
    }
    if source.config.session.cloud.region.is_some() {
        target.session.cloud.region = source.config.session.cloud.region.clone();
    }
    if source.config.session.cloud.access_key.is_some() {
        target.session.cloud.access_key = source.config.session.cloud.access_key.clone();
    }
    if source.config.session.cloud.secret_key.is_some() {
        target.session.cloud.secret_key = source.config.session.cloud.secret_key.clone();
    }

    for key in &source.kt1_keys {
        match key.as_str() {
            "stale_claim_timeout_seconds" => {
                target.kt1.stale_claim_timeout_seconds =
                    source.config.kt1.stale_claim_timeout_seconds;
            }
            "stale_dispatch_timeout_seconds" => {
                target.kt1.stale_dispatch_timeout_seconds =
                    source.config.kt1.stale_dispatch_timeout_seconds;
            }
            "redelegation_cap" => {
                target.kt1.redelegation_cap = source.config.kt1.redelegation_cap;
            }
            "coord_cursor_ttl_days" => {
                target.kt1.coord_cursor_ttl_days = source.config.kt1.coord_cursor_ttl_days;
            }
            "terminal_index_compact_lines" => {
                target.kt1.terminal_index_compact_lines =
                    source.config.kt1.terminal_index_compact_lines;
            }
            "compact_lock_timeout_seconds" => {
                target.kt1.compact_lock_timeout_seconds =
                    source.config.kt1.compact_lock_timeout_seconds;
            }
            "directive_batch_size" => {
                target.kt1.directive_batch_size = source.config.kt1.directive_batch_size;
            }
            "respawn_generation_cap" => {
                target.kt1.respawn_generation_cap = source.config.kt1.respawn_generation_cap;
            }
            _ => {}
        }
    }
    if source.kt1_has_recursion {
        target.kt1.recursion = source.config.kt1.recursion.clone();
    }
}

/// Path to the user config file: ~/.koto/config.toml
pub fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".koto").join("config.toml"))
}

/// Path to the project config file: .koto/config.toml (relative to cwd)
pub fn project_config_path() -> PathBuf {
    PathBuf::from(".koto").join("config.toml")
}

/// Ensure ~/.koto/ exists with 0700 permissions.
/// This is independent of the session module's ensure_koto_root.
pub fn ensure_koto_dir() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    let koto_dir = home.join(".koto");
    let needs_create = !koto_dir.exists();
    fs::create_dir_all(&koto_dir)?;

    if needs_create {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&koto_dir, fs::Permissions::from_mode(0o700))?;
        }
    }

    Ok(koto_dir)
}

/// Load a TOML file as a raw toml::Value for editing.
/// Returns an empty table if the file does not exist.
pub fn load_toml_value(path: &Path) -> Result<toml::Value> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let content = fs::read_to_string(path)?;
    let value: toml::Value = content.parse()?;
    Ok(value)
}

/// Write a toml::Value to a file, creating parent directories as needed.
pub fn write_toml_value(path: &Path, value: &toml::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(value)?;
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// `KOTO_KT1_*` env keys recognized by `apply_kt1_env_overrides`.
    /// Listed here so tests can clear them between runs (env vars are
    /// process-global; cargo's parallel test runner can leak between
    /// unrelated tests).
    const KT1_ENV_KEYS: &[&str] = &[
        "KOTO_KT1_STALE_CLAIM_TIMEOUT_S",
        "KOTO_KT1_STALE_DISPATCH_TIMEOUT_S",
        "KOTO_KT1_REDELEGATION_CAP",
        "KOTO_KT1_COORD_CURSOR_TTL_DAYS",
        "KOTO_KT1_TERMINAL_INDEX_COMPACT_LINES",
        "KOTO_KT1_COMPACT_LOCK_TIMEOUT_S",
        "KOTO_KT1_DIRECTIVE_BATCH_SIZE",
        "KOTO_KT1_RESPAWN_GENERATION_CAP",
    ];

    fn clear_kt1_env() {
        for k in KT1_ENV_KEYS {
            env::remove_var(k);
        }
    }

    #[test]
    fn test_load_config_defaults() {
        // With no config files, we get defaults.
        // Run in a temp dir and override HOME to avoid picking up real user/project config.
        let tmp = TempDir::new().unwrap();
        let _guard = SetCwd::new(tmp.path());
        let _home_guard = SetEnv::new("HOME", tmp.path().to_str().unwrap());

        // Clear env vars that would interfere.
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        clear_kt1_env();

        let config = load_config().unwrap();
        assert_eq!(config.session.backend, "local");
        assert!(config.session.cloud.endpoint.is_none());
        assert!(config.session.cloud.access_key.is_none());
        // Kt1Config defaults match Decision 4's table.
        assert_eq!(config.kt1.redelegation_cap, 3);
        assert_eq!(config.kt1.stale_claim_timeout_seconds, 600);
        assert_eq!(config.kt1.terminal_index_compact_lines, 100_000);
    }

    #[test]
    fn test_merge_config_overlay() {
        let mut base = KotoConfig::default();
        base.session.backend = "local".to_string();

        let overlay = LoadedConfig {
            config: KotoConfig {
                session: super::super::SessionConfig {
                    backend: "cloud".to_string(),
                    cloud: super::super::CloudConfig {
                        bucket: Some("my-bucket".to_string()),
                        ..Default::default()
                    },
                },
                kt1: Kt1Config::default(),
            },
            kt1_keys: vec![],
            kt1_has_recursion: false,
        };

        merge_config(&mut base, &overlay);
        assert_eq!(base.session.backend, "cloud");
        assert_eq!(base.session.cloud.bucket, Some("my-bucket".to_string()));
    }

    #[test]
    fn test_merge_preserves_existing_when_source_empty() {
        let mut base = KotoConfig::default();
        base.session.backend = "cloud".to_string();
        base.session.cloud.bucket = Some("existing".to_string());

        let overlay = LoadedConfig {
            config: KotoConfig::default(),
            kt1_keys: vec![],
            kt1_has_recursion: false,
        };
        merge_config(&mut base, &overlay);

        // backend stays "cloud" because overlay backend is empty string (Default)
        // but serde deserialization would give "local" — here we're using Default directly.
        // The merge only overwrites if source backend is non-empty.
        assert_eq!(base.session.backend, "cloud");
        assert_eq!(base.session.cloud.bucket, Some("existing".to_string()));
    }

    #[test]
    fn test_env_var_override() {
        let tmp = TempDir::new().unwrap();
        let _guard = SetCwd::new(tmp.path());
        let _home_guard = SetEnv::new("HOME", tmp.path().to_str().unwrap());

        // Test that env vars override whatever was loaded.
        env::set_var("AWS_ACCESS_KEY_ID", "env-key-id");
        env::set_var("AWS_SECRET_ACCESS_KEY", "env-secret-key");

        let config = load_config().unwrap();
        assert_eq!(
            config.session.cloud.access_key,
            Some("env-key-id".to_string())
        );
        assert_eq!(
            config.session.cloud.secret_key,
            Some("env-secret-key".to_string())
        );

        // Clean up.
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
    }

    #[test]
    fn test_project_config_overrides_user() {
        let tmp = TempDir::new().unwrap();
        let _guard = SetCwd::new(tmp.path());
        let _home_guard = SetEnv::new("HOME", tmp.path().to_str().unwrap());
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");

        // Write a project config.
        let project_dir = tmp.path().join(".koto");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("config.toml"),
            "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nbucket = \"proj-bucket\"\n",
        )
        .unwrap();

        let config = load_config().unwrap();
        assert_eq!(config.session.backend, "cloud");
        assert_eq!(config.session.cloud.bucket, Some("proj-bucket".to_string()));
    }

    #[test]
    fn test_load_toml_value_nonexistent() {
        let val = load_toml_value(Path::new("/tmp/nonexistent_koto_config.toml")).unwrap();
        assert!(val.is_table());
        assert!(val.as_table().unwrap().is_empty());
    }

    #[test]
    fn test_write_and_load_toml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.toml");

        let mut val = toml::Value::Table(toml::map::Map::new());
        crate::config::set_value_in_toml(&mut val, "session.backend", "cloud").unwrap();

        write_toml_value(&path, &val).unwrap();

        let loaded = load_toml_value(&path).unwrap();
        let config: KotoConfig = loaded.try_into().unwrap();
        assert_eq!(config.session.backend, "cloud");
    }

    /// RAII guard that sets an env var and restores the previous value on drop.
    struct SetEnv {
        key: String,
        prev: Option<String>,
    }

    impl SetEnv {
        fn new(key: &str, val: &str) -> Self {
            let prev = env::var(key).ok();
            env::set_var(key, val);
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for SetEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(val) => env::set_var(&self.key, val),
                None => env::remove_var(&self.key),
            }
        }
    }

    /// RAII guard that changes cwd and restores it on drop.
    struct SetCwd {
        prev: PathBuf,
    }

    impl SetCwd {
        fn new(path: &Path) -> Self {
            let prev = env::current_dir().unwrap();
            env::set_current_dir(path).unwrap();
            Self { prev }
        }
    }

    impl Drop for SetCwd {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.prev);
        }
    }
}
