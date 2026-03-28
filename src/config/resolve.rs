use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::KotoConfig;

/// Load and merge configuration from all sources.
///
/// Precedence (highest to lowest):
/// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
/// 2. Project config (.koto/config.toml in current directory)
/// 3. User config (~/.koto/config.toml)
/// 4. Built-in defaults
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

    Ok(config)
}

/// Load a TOML config file and deserialize it.
fn load_config_file(path: &Path) -> Result<KotoConfig> {
    let content = fs::read_to_string(path)?;
    let config: KotoConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Merge source config into target. Non-default/non-empty values in source
/// overwrite target.
fn merge_config(target: &mut KotoConfig, source: &KotoConfig) {
    if !source.session.backend.is_empty() {
        target.session.backend = source.session.backend.clone();
    }
    if source.session.cloud.endpoint.is_some() {
        target.session.cloud.endpoint = source.session.cloud.endpoint.clone();
    }
    if source.session.cloud.bucket.is_some() {
        target.session.cloud.bucket = source.session.cloud.bucket.clone();
    }
    if source.session.cloud.region.is_some() {
        target.session.cloud.region = source.session.cloud.region.clone();
    }
    if source.session.cloud.access_key.is_some() {
        target.session.cloud.access_key = source.session.cloud.access_key.clone();
    }
    if source.session.cloud.secret_key.is_some() {
        target.session.cloud.secret_key = source.session.cloud.secret_key.clone();
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

    #[test]
    fn test_load_config_defaults() {
        // With no config files, we get defaults.
        // Run in a temp dir to avoid picking up real project config.
        let tmp = TempDir::new().unwrap();
        let _guard = SetCwd::new(tmp.path());

        // Clear env vars that would interfere.
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");

        let config = load_config().unwrap();
        assert_eq!(config.session.backend, "local");
        assert!(config.session.cloud.endpoint.is_none());
        assert!(config.session.cloud.access_key.is_none());
    }

    #[test]
    fn test_merge_config_overlay() {
        let mut base = KotoConfig::default();
        base.session.backend = "local".to_string();

        let overlay = KotoConfig {
            session: super::super::SessionConfig {
                backend: "cloud".to_string(),
                cloud: super::super::CloudConfig {
                    bucket: Some("my-bucket".to_string()),
                    ..Default::default()
                },
            },
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

        let overlay = KotoConfig::default();
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

        // Write a user config with credentials.
        // We can't easily write to ~/.koto/ in tests, but we can test env override logic
        // by checking that env vars override whatever was loaded.
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
