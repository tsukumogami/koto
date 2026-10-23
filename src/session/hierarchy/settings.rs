//! [`RetentionSettings`]: the three retention settings, parsed and checked.

use std::time::Duration;

use crate::config::retention::{RetentionConfig, SETTINGS};
use crate::config::session_env::Origin;
use crate::session::duration::parse_age;

/// One retention setting as it resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// The dotted name, such as `retention.keep`.
    pub name: &'static str,
    pub value: Duration,
    /// The value as written.
    pub raw: String,
    pub origin: Origin,
}

impl Setting {
    /// Where the value came from, as a person reads it: `default`,
    /// `user config`, `project config` or the variable's name.
    pub fn origin_label(&self) -> String {
        match self.origin {
            Origin::Env(var) => var.to_string(),
            Origin::Project => "project config".to_string(),
            Origin::User => "user config".to_string(),
            Origin::Default | Origin::Unset => "default".to_string(),
        }
    }
}

/// `retention.keep`, `retention.idle_period` and `retention.stale_after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionSettings {
    /// How long a hierarchy is kept after its release instant.
    pub keep: Setting,
    /// How long an anchor with no observed marker stays open after its last
    /// activity.
    pub idle_period: Setting,
    /// How long after its last activity an observed anchor is reported
    /// stale.
    pub stale_after: Setting,
}

/// A retention setting koto refuses, with the setting named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingError(pub String);

impl std::fmt::Display for SettingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingError {}

/// The value as an error repeats it: control characters dropped, cut to
/// 32 characters.
fn shown(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| !c.is_control()).take(32).collect();
    format!("{clean:?}")
}

impl RetentionSettings {
    /// Parse and check the resolved configuration. Each value must be a
    /// whole number followed by `s`, `m`, `h` or `d`, longer than zero, and
    /// the stale threshold must be longer than the retention, so a stale
    /// report never comes before a correct close would have released
    /// anything. A refusal names the setting and where its value came from.
    pub fn resolve(config: &RetentionConfig) -> Result<Self, SettingError> {
        let one = |index: usize| -> Result<Setting, SettingError> {
            let (name, _, _) = SETTINGS[index];
            let raw = config.effective(name).unwrap_or_default().to_string();
            let origin = config.origin(name);
            let setting = Setting {
                name,
                value: Duration::ZERO,
                raw: raw.clone(),
                origin,
            };
            match parse_age(raw.trim()) {
                Some(d) if !d.is_zero() => Ok(Setting {
                    value: d,
                    ..setting
                }),
                Some(_) => Err(SettingError(format!(
                    "{name} is {} (from {}); it must be longer than zero",
                    shown(&raw),
                    setting.origin_label()
                ))),
                None => Err(SettingError(format!(
                    "{name} is {} (from {}); it must be a whole number followed by s, m, h or d, \
                     such as 7d",
                    shown(&raw),
                    setting.origin_label()
                ))),
            }
        };
        let settings = RetentionSettings {
            keep: one(0)?,
            idle_period: one(1)?,
            stale_after: one(2)?,
        };
        if settings.stale_after.value <= settings.keep.value {
            return Err(SettingError(format!(
                "retention.stale_after is {} (from {}), which must be longer than retention.keep, \
                 {} (from {}), so a stale report never comes before a release",
                shown(&settings.stale_after.raw),
                settings.stale_after.origin_label(),
                shown(&settings.keep.raw),
                settings.keep.origin_label(),
            )));
        }
        Ok(settings)
    }

    /// Load the configuration and resolve the settings.
    pub fn load() -> anyhow::Result<Self> {
        let config = crate::config::resolve::load_config()?;
        Ok(Self::resolve(&config.retention)?)
    }

    /// Settings with the given durations, each from koto's default. For
    /// tests and callers that already hold durations.
    pub fn from_durations(keep: Duration, idle_period: Duration, stale_after: Duration) -> Self {
        let setting = |name: &'static str, value: Duration| Setting {
            name,
            value,
            raw: format!("{}s", value.as_secs()),
            origin: Origin::Default,
        };
        RetentionSettings {
            keep: setting("retention.keep", keep),
            idle_period: setting("retention.idle_period", idle_period),
            stale_after: setting("retention.stale_after", stale_after),
        }
    }

    /// The three, in report order.
    pub fn all(&self) -> [&Setting; 3] {
        [&self.keep, &self.idle_period, &self.stale_after]
    }
}
