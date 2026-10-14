//! Session settings from the environment, and where every session setting
//! came from.
//!
//! [`resolve::load_config`](super::resolve::load_config) fills a
//! [`SessionEnv`] while it merges the layers: the origin of each of the
//! eight session settings, the environment values it couldn't use, and the
//! missing half of a koto credential pair. Loading never fails on any of
//! them, so `koto config list` can show them; [`check`] refuses them where a
//! backend is built. The cloud-session-config design records the rules.

use super::KotoConfig;
use crate::redact::{redact_str, Redactor};

/// `session.backend`.
pub const ENV_BACKEND: &str = "KOTO_SESSION_BACKEND";
/// `session.cloud.endpoint`.
pub const ENV_ENDPOINT: &str = "KOTO_SESSION_CLOUD_ENDPOINT";
/// `session.cloud.bucket`.
pub const ENV_BUCKET: &str = "KOTO_SESSION_CLOUD_BUCKET";
/// `session.cloud.region`.
pub const ENV_REGION: &str = "KOTO_SESSION_CLOUD_REGION";
/// `session.cloud.path_style`.
pub const ENV_PATH_STYLE: &str = "KOTO_SESSION_CLOUD_PATH_STYLE";
/// `session.cloud.access_key`, half of the koto credential pair.
pub const ENV_ACCESS_KEY: &str = "KOTO_SESSION_CLOUD_ACCESS_KEY";
/// `session.cloud.secret_key`, half of the koto credential pair.
pub const ENV_SECRET_KEY: &str = "KOTO_SESSION_CLOUD_SECRET_KEY";

/// The session settings whose origin `koto config list` reports, in the
/// order it reports them.
pub const SETTINGS: [&str; 8] = [
    "session.backend",
    "session.cloud.endpoint",
    "session.cloud.bucket",
    "session.cloud.region",
    "session.cloud.path_style",
    "session.cloud.access_key",
    "session.cloud.secret_key",
    "session.cloud.marker_retain_days",
];

/// The longest raw value an error or origin line repeats.
const ECHO_MAX_CHARS: usize = 32;

/// Where a session setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// An environment variable, named.
    Env(&'static str),
    /// `.koto/config.toml` in the working directory.
    Project,
    /// `~/.koto/config.toml`.
    User,
    /// No layer set it, and koto's built-in value applies.
    Default,
    /// No layer set it, and the setting has no built-in value.
    Unset,
}

impl Origin {
    /// The `source` word the report prints.
    pub fn source(&self) -> &'static str {
        match self {
            Origin::Env(_) => "env",
            Origin::Project => "project",
            Origin::User => "user",
            Origin::Default => "default",
            Origin::Unset => "unset",
        }
    }
}

/// An environment value koto can't use for its setting.
#[derive(Clone, PartialEq, Eq)]
pub struct InvalidEnv {
    pub setting: &'static str,
    pub variable: &'static str,
    /// The value as set; [`None`] when it isn't valid Unicode.
    pub raw: Option<String>,
}

impl std::fmt::Debug for InvalidEnv {
    // The value can be a credential pasted into the wrong variable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvalidEnv")
            .field("setting", &self.setting)
            .field("variable", &self.variable)
            .finish()
    }
}

/// What loading learned about the session settings. Never serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEnv {
    origins: [Origin; 8],
    /// Environment values koto can't use, in [`SETTINGS`] order.
    pub invalid: Vec<InvalidEnv>,
    /// The koto credential variable that is unset while the other is set.
    pub pair_missing: Option<&'static str>,
}

impl Default for SessionEnv {
    fn default() -> Self {
        let origins = SETTINGS.map(|s| match s {
            "session.backend" | "session.cloud.path_style" | "session.cloud.marker_retain_days" => {
                Origin::Default
            }
            _ => Origin::Unset,
        });
        SessionEnv {
            origins,
            invalid: Vec::new(),
            pair_missing: None,
        }
    }
}

impl SessionEnv {
    /// The origin of `setting`, one of [`SETTINGS`].
    pub fn origin(&self, setting: &str) -> Origin {
        match SETTINGS.iter().position(|s| *s == setting) {
            Some(i) => self.origins[i],
            None => Origin::Unset,
        }
    }

    /// Record the origin of `setting`; a name outside [`SETTINGS`] is
    /// ignored.
    pub fn set_origin(&mut self, setting: &str, origin: Origin) {
        if let Some(i) = SETTINGS.iter().position(|s| *s == setting) {
            self.origins[i] = origin;
        }
    }

    /// The unusable environment value recorded for `setting`, if any.
    pub fn invalid_for(&self, setting: &str) -> Option<&InvalidEnv> {
        self.invalid.iter().find(|i| i.setting == setting)
    }

    /// Whether the koto credential pair supplied the credentials.
    pub fn pair_in_use(&self) -> bool {
        matches!(
            self.origin("session.cloud.access_key"),
            Origin::Env(ENV_ACCESS_KEY)
        ) || matches!(
            self.origin("session.cloud.secret_key"),
            Origin::Env(ENV_SECRET_KEY)
        ) || self.pair_missing.is_some()
    }
}

/// `raw` as an error or origin line may repeat it: known credentials
/// redacted, cut to [`ECHO_MAX_CHARS`] characters, and escaped.
pub fn echo(raw: Option<&str>, config: &KotoConfig) -> String {
    let Some(raw) = raw else {
        return "<not valid unicode>".to_string();
    };
    let redactor = Redactor::new(super::redaction_keys(config));
    let (cut, more) = redact_str(raw, &redactor).cut_chars(ECHO_MAX_CHARS);
    let mut shown = format!("{:?}", cut.as_str());
    if more {
        shown.push_str("...");
    }
    shown
}

/// Refuse a session configuration koto must not build a backend from.
///
/// An unusable `KOTO_SESSION_*` value, and a backend from the environment
/// other than `local` or `cloud`, refuse whatever the backend. On the cloud
/// backend a half-set koto credential pair refuses, naming the missing
/// variable, and so does an endpoint or bucket from the project file when
/// the koto pair supplies the credentials: a committed file never chooses
/// where they're sent. A backend value from a config file is left to the
/// caller's own `unknown backend` error, as before.
pub fn check(config: &KotoConfig) -> anyhow::Result<()> {
    let env = &config.session_env;
    if let Some(bad) = env.invalid.first() {
        let wants = match bad.setting {
            "session.backend" => "local or cloud",
            "session.cloud.path_style" => "true or false",
            _ => "a Unicode string",
        };
        anyhow::bail!(
            "{} is {}; set it to {}, or unset it",
            bad.variable,
            echo(bad.raw.as_deref(), config),
            wants
        );
    }
    let backend = config.session.backend.as_str();
    if let Origin::Env(var) = env.origin("session.backend") {
        if backend != "local" && backend != "cloud" {
            anyhow::bail!(
                "{} is {}; set it to local or cloud, or unset it",
                var,
                echo(Some(backend), config)
            );
        }
    }
    if backend != "cloud" {
        return Ok(());
    }
    if let Some(missing) = env.pair_missing {
        let set = if missing == ENV_ACCESS_KEY {
            ENV_SECRET_KEY
        } else {
            ENV_ACCESS_KEY
        };
        anyhow::bail!("{} is set but {} is not; set both or neither", set, missing);
    }
    if env.pair_in_use() {
        for (setting, var) in [
            ("session.cloud.endpoint", ENV_ENDPOINT),
            ("session.cloud.bucket", ENV_BUCKET),
        ] {
            if env.origin(setting) == Origin::Project {
                anyhow::bail!(
                    "{} comes from the project file, which can't choose where \
                     KOTO_SESSION_CLOUD_* credentials are sent; set {} or the user file instead",
                    setting,
                    var
                );
            }
        }
    }
    Ok(())
}

/// The `KOTO_SESSION_*` variables that supplied a session setting, with the
/// values koto resolved, for the environment of the commands a tick runs:
/// a koto that a gate or action calls then resolves the same backend,
/// location and credentials. Settings from a config file aren't included;
/// a nested koto reads the same files.
pub fn command_vars(config: &KotoConfig) -> Vec<(String, String)> {
    let env = &config.session_env;
    let cloud = &config.session.cloud;
    let mut vars = Vec::new();
    for setting in SETTINGS {
        let Origin::Env(var) = env.origin(setting) else {
            continue;
        };
        if !var.starts_with("KOTO_SESSION_") {
            continue;
        }
        let value = match setting {
            "session.backend" => Some(config.session.backend.clone()),
            "session.cloud.endpoint" => cloud.endpoint.clone(),
            "session.cloud.bucket" => cloud.bucket.clone(),
            "session.cloud.region" => cloud.region.clone(),
            "session.cloud.path_style" => match env.invalid_for(setting) {
                Some(bad) => bad.raw.clone(),
                None => cloud.path_style.map(|b| b.to_string()),
            },
            "session.cloud.access_key" => cloud.access_key.clone(),
            "session.cloud.secret_key" => cloud.secret_key.clone(),
            _ => None,
        };
        if let Some(v) = value {
            vars.push((var.to_string(), v));
        }
    }
    vars
}

/// The `origins` object `koto config list --json` adds.
pub fn origins_json(config: &KotoConfig) -> serde_json::Value {
    let env = &config.session_env;
    let mut map = serde_json::Map::new();
    for setting in SETTINGS {
        let origin = env.origin(setting);
        let mut entry = serde_json::Map::new();
        entry.insert("source".into(), origin.source().into());
        if let Origin::Env(var) = origin {
            entry.insert("variable".into(), var.into());
        }
        if let Some(bad) = env.invalid_for(setting) {
            entry.insert("invalid".into(), echo(bad.raw.as_deref(), config).into());
        }
        map.insert(setting.to_string(), serde_json::Value::Object(entry));
    }
    serde_json::Value::Object(map)
}

/// The comment lines `koto config list` appends to its TOML, one per
/// setting. Comments, so the output parses to the same tables as before.
pub fn origins_toml(config: &KotoConfig) -> String {
    let env = &config.session_env;
    let mut out = String::new();
    for setting in SETTINGS {
        let origin = env.origin(setting);
        out.push_str("# origin: ");
        out.push_str(setting);
        out.push_str(" = ");
        out.push_str(origin.source());
        if let Origin::Env(var) = origin {
            out.push(' ');
            out.push_str(var);
        }
        if let Some(bad) = env.invalid_for(setting) {
            out.push_str(&format!(" (invalid: {})", echo(bad.raw.as_deref(), config)));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_name_the_three_settings_with_a_built_in_value() {
        let env = SessionEnv::default();
        assert_eq!(env.origin("session.backend"), Origin::Default);
        assert_eq!(env.origin("session.cloud.path_style"), Origin::Default);
        assert_eq!(
            env.origin("session.cloud.marker_retain_days"),
            Origin::Default
        );
        for s in [
            "session.cloud.endpoint",
            "session.cloud.bucket",
            "session.cloud.region",
            "session.cloud.access_key",
            "session.cloud.secret_key",
        ] {
            assert_eq!(env.origin(s), Origin::Unset, "{s}");
        }
    }

    #[test]
    fn echo_cuts_escapes_and_redacts() {
        let mut config = KotoConfig::default();
        config.session.cloud.secret_key = Some("supersecretvalue".to_string());
        config.secret_sources.secret_key_env = Some(ENV_SECRET_KEY);
        let shown = echo(Some("supersecretvalue"), &config);
        assert!(!shown.contains("supersecretvalue"), "{shown}");
        let long = "x".repeat(100);
        let shown = echo(Some(&long), &config);
        assert!(shown.ends_with("..."), "{shown}");
        assert!(shown.len() < 50, "{shown}");
        assert_eq!(echo(Some("a\nb"), &config), "\"a\\nb\"");
        assert_eq!(echo(None, &config), "<not valid unicode>");
    }

    #[test]
    fn invalid_env_debug_names_no_value() {
        let bad = InvalidEnv {
            setting: "session.backend",
            variable: ENV_BACKEND,
            raw: Some("hunter2".to_string()),
        };
        assert!(!format!("{bad:?}").contains("hunter2"));
    }
}
