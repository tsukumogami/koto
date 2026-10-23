//! The three retention settings: `retention.keep`, `retention.idle_period`
//! and `retention.stale_after` (DESIGN-session-retention.md, Decision 3).
//!
//! Each resolves environment, then project file, then user file, then
//! koto's default, and remembers where its value came from. The values are
//! kept as written and parsed late, by
//! [`crate::session::hierarchy::RetentionSettings::resolve`], which
//! refuses a malformed one naming the setting: loading never fails on
//! them, so `koto config list` can still show what was set. They stay out
//! of the `KOTO_SESSION_*` family on purpose, since koto hands that family
//! to the commands a tick runs and a cloud-configuration scrub removes it.

use serde::{Deserialize, Deserializer, Serialize};

use super::session_env::Origin;

/// `retention.keep`.
pub const ENV_KEEP: &str = "KOTO_RETENTION_KEEP";
/// `retention.idle_period`.
pub const ENV_IDLE_PERIOD: &str = "KOTO_RETENTION_IDLE_PERIOD";
/// `retention.stale_after`.
pub const ENV_STALE_AFTER: &str = "KOTO_RETENTION_STALE_AFTER";

/// The default of `retention.keep`.
pub const DEFAULT_KEEP: &str = "7d";
/// The default of `retention.idle_period`.
pub const DEFAULT_IDLE_PERIOD: &str = "7d";
/// The default of `retention.stale_after`.
pub const DEFAULT_STALE_AFTER: &str = "30d";

/// The three settings, in the order every report lists them, with their
/// environment variable and default.
pub const SETTINGS: [(&str, &str, &str); 3] = [
    ("retention.keep", ENV_KEEP, DEFAULT_KEEP),
    (
        "retention.idle_period",
        ENV_IDLE_PERIOD,
        DEFAULT_IDLE_PERIOD,
    ),
    (
        "retention.stale_after",
        ENV_STALE_AFTER,
        DEFAULT_STALE_AFTER,
    ),
];

/// The `[retention]` table, as the layers merged it.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetentionConfig {
    #[serde(
        default,
        deserialize_with = "any_as_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub keep: Option<String>,
    #[serde(
        default,
        deserialize_with = "any_as_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_period: Option<String>,
    #[serde(
        default,
        deserialize_with = "any_as_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub stale_after: Option<String>,
    /// Where each value came from, in [`SETTINGS`] order. Filled by
    /// `resolve::load_config`; never serialized.
    #[serde(skip)]
    pub origins: RetentionOrigins,
}

/// Where each retention setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionOrigins(pub [Origin; 3]);

impl Default for RetentionOrigins {
    fn default() -> Self {
        RetentionOrigins([Origin::Default; 3])
    }
}

/// Read a value of any TOML type as text, so `keep = 7` loads (and is then
/// refused, naming the setting, where retention is judged) instead of
/// failing every command that loads the configuration.
fn any_as_string<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    let value = Option::<toml::Value>::deserialize(de)?;
    Ok(value.map(|v| match v {
        toml::Value::String(s) => s,
        other => other.to_string(),
    }))
}

fn index_of(setting: &str) -> Option<usize> {
    SETTINGS.iter().position(|(name, _, _)| *name == setting)
}

impl RetentionConfig {
    /// Whether no layer set any of the three, so `koto config list` prints
    /// no `[retention]` table, as before these settings existed.
    pub fn is_empty(&self) -> bool {
        self.keep.is_none() && self.idle_period.is_none() && self.stale_after.is_none()
    }

    fn slot(&mut self, setting: &str) -> Option<&mut Option<String>> {
        match setting {
            "retention.keep" => Some(&mut self.keep),
            "retention.idle_period" => Some(&mut self.idle_period),
            "retention.stale_after" => Some(&mut self.stale_after),
            _ => None,
        }
    }

    /// The value set for `setting`, if a layer set one.
    pub fn raw(&self, setting: &str) -> Option<&str> {
        match setting {
            "retention.keep" => self.keep.as_deref(),
            "retention.idle_period" => self.idle_period.as_deref(),
            "retention.stale_after" => self.stale_after.as_deref(),
            _ => None,
        }
    }

    /// The value `setting` resolves to: the one set, else its default.
    /// `None` for a name that isn't a retention setting.
    pub fn effective(&self, setting: &str) -> Option<&str> {
        let default = SETTINGS.iter().find(|(n, _, _)| *n == setting)?.2;
        Some(self.raw(setting).unwrap_or(default))
    }

    /// Where `setting`'s value came from.
    pub fn origin(&self, setting: &str) -> Origin {
        index_of(setting)
            .map(|i| self.origins.0[i])
            .unwrap_or(Origin::Unset)
    }

    /// Lay the values a config file set over the ones below it.
    pub fn merge_file(&mut self, file: &RetentionConfig, origin: Origin) {
        for (setting, _, _) in SETTINGS {
            if let Some(value) = file.raw(setting) {
                let value = value.to_string();
                *self.slot(setting).expect("a retention setting") = Some(value);
                self.origins.0[index_of(setting).expect("a retention setting")] = origin;
            }
        }
    }

    /// Lay the `KOTO_RETENTION_*` variables over the file layers. A value
    /// that is empty or only whitespace counts as unset.
    pub fn apply_env<F: Fn(&str) -> Option<String>>(&mut self, lookup: F) {
        for (setting, var, _) in SETTINGS {
            let Some(value) = lookup(var) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            *self.slot(setting).expect("a retention setting") = Some(value.to_string());
            self.origins.0[index_of(setting).expect("a retention setting")] = Origin::Env(var);
        }
    }

    /// The config as `koto config list` shows it: every setting with the
    /// value it resolves to, defaults included.
    pub fn resolved_for_display(&self) -> RetentionConfig {
        let mut shown = self.clone();
        for (setting, _, default) in SETTINGS {
            let slot = shown.slot(setting).expect("a retention setting");
            if slot.is_none() {
                *slot = Some(default.to_string());
            }
        }
        shown
    }
}

/// Check a value `koto config set` is asked to write: a whole number
/// followed by `s`, `m`, `h` or `d`, and not zero.
pub fn check_value(setting: &str, value: &str) -> Result<(), String> {
    match crate::session::duration::parse_age(value.trim()) {
        Some(d) if !d.is_zero() => Ok(()),
        Some(_) => Err(format!("value for {setting} must be longer than zero")),
        None => Err(format!(
            "value for {setting} must be a whole number followed by s, m, h or d (such as 7d)"
        )),
    }
}

/// The `origins` entries `koto config list --json` adds for the three.
pub fn origins_json(
    config: &RetentionConfig,
    map: &mut serde_json::Map<String, serde_json::Value>,
) {
    for (setting, _, _) in SETTINGS {
        let origin = config.origin(setting);
        let mut entry = serde_json::Map::new();
        entry.insert("source".into(), origin.source().into());
        if let Origin::Env(var) = origin {
            entry.insert("variable".into(), var.into());
        }
        map.insert(setting.to_string(), serde_json::Value::Object(entry));
    }
}

/// The comment lines `koto config list` appends for the three.
pub fn origins_toml(config: &RetentionConfig) -> String {
    let mut out = String::new();
    for (setting, _, _) in SETTINGS {
        let origin = config.origin(setting);
        out.push_str("# origin: ");
        out.push_str(setting);
        out.push_str(" = ");
        out.push_str(origin.source());
        if let Origin::Env(var) = origin {
            out.push(' ');
            out.push_str(var);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_resolve_env_over_project_over_user_over_default() {
        let mut config = RetentionConfig::default();
        assert_eq!(config.effective("retention.keep"), Some("7d"));
        assert_eq!(config.effective("retention.idle_period"), Some("7d"));
        assert_eq!(config.effective("retention.stale_after"), Some("30d"));
        assert_eq!(config.origin("retention.keep"), Origin::Default);

        let user = RetentionConfig {
            keep: Some("3d".into()),
            idle_period: Some("2d".into()),
            ..Default::default()
        };
        config.merge_file(&user, Origin::User);
        let project = RetentionConfig {
            keep: Some("4d".into()),
            ..Default::default()
        };
        config.merge_file(&project, Origin::Project);
        config.apply_env(|var| (var == ENV_IDLE_PERIOD).then(|| " 9h ".to_string()));

        assert_eq!(config.effective("retention.keep"), Some("4d"));
        assert_eq!(config.origin("retention.keep"), Origin::Project);
        assert_eq!(config.effective("retention.idle_period"), Some("9h"));
        assert_eq!(
            config.origin("retention.idle_period"),
            Origin::Env(ENV_IDLE_PERIOD)
        );
        assert_eq!(config.effective("retention.stale_after"), Some("30d"));
        assert_eq!(config.origin("retention.stale_after"), Origin::Default);
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let mut config = RetentionConfig::default();
        config.apply_env(|_| Some("  ".to_string()));
        assert!(config.is_empty());
        assert_eq!(config.origin("retention.keep"), Origin::Default);
    }

    #[test]
    fn a_value_of_any_toml_type_loads_as_text() {
        let config: super::super::KotoConfig =
            toml::from_str("[retention]\nkeep = 7\nidle_period = \"2h\"\n").unwrap();
        assert_eq!(config.retention.keep.as_deref(), Some("7"));
        assert_eq!(config.retention.idle_period.as_deref(), Some("2h"));
    }

    #[test]
    fn set_refuses_a_malformed_or_zero_value() {
        assert!(check_value("retention.keep", "7d").is_ok());
        assert!(check_value("retention.keep", "90s").is_ok());
        let e = check_value("retention.keep", "7x").unwrap_err();
        assert!(e.contains("retention.keep"), "{e}");
        assert!(check_value("retention.idle_period", "0s").is_err());
        assert!(check_value("retention.stale_after", "").is_err());
    }
}
