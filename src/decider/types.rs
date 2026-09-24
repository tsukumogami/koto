//! Pure decider types shared by configuration, the engine, and the
//! provider client. Nothing here does I/O.

use std::fmt;

use serde::Serialize;

/// A decider API key.
///
/// The type deliberately has no `Display` impl and its `Debug` prints
/// `<redacted>`, so the key can't reach a log line, an error message, or
/// a `{:?}` dump by accident. The raw value is reachable only through
/// [`ApiKey::expose_for_transport`], which exists for the HTTP transport
/// that puts it in the `Authorization` header and for nothing else.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap a raw key string.
    pub fn new(raw: impl Into<String>) -> Self {
        ApiKey(raw.into())
    }

    /// The raw key, for the transport's `Authorization` header only.
    ///
    /// Never interpolate the return value into a message, a warning, an
    /// error, or anything that is printed or persisted.
    pub fn expose_for_transport(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The user's global decider mode, ordered `Off < Shadow < Auto`.
///
/// The effective global mode is the minimum of the user-level mode (env
/// or user config) and the project mode, so a repository can only lower
/// it. `never` is a template-only mode and is not a valid global mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalMode {
    Off,
    Shadow,
    Auto,
}

impl GlobalMode {
    /// Parse a mode string, trimming whitespace and ignoring case.
    /// Returns `None` for anything other than `off`, `shadow`, or `auto`
    /// (including `never` and the empty string).
    pub fn parse(raw: &str) -> Option<GlobalMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Some(GlobalMode::Off),
            "shadow" => Some(GlobalMode::Shadow),
            "auto" => Some(GlobalMode::Auto),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GlobalMode::Off => "off",
            GlobalMode::Shadow => "shadow",
            GlobalMode::Auto => "auto",
        }
    }
}

/// Which configuration layer a decider setting came from.
///
/// Recorded on each consultation as `endpoint_origin`, and used by the
/// same-layer rule: a key is only sent to an endpoint from its own layer
/// or to the built-in default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingOrigin {
    /// Built-in default (only meaningful for the endpoint).
    #[default]
    Default,
    /// `~/.koto/config.toml`.
    User,
    /// A `KOTO_DECIDER*` environment variable.
    Env,
}

impl SettingOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingOrigin::Default => "default",
            SettingOrigin::User => "user",
            SettingOrigin::Env => "env",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_debug_is_redacted() {
        let key = ApiKey::new("sk-very-secret-value");
        let dbg = format!("{:?}", key);
        assert_eq!(dbg, "<redacted>");
        assert!(!dbg.contains("sk-very-secret-value"));
        let dbg_opt = format!("{:?}", Some(key.clone()));
        assert!(!dbg_opt.contains("sk-very-secret-value"));
        assert_eq!(key.expose_for_transport(), "sk-very-secret-value");
    }

    #[test]
    fn global_mode_parse_trims_and_lowercases() {
        assert_eq!(GlobalMode::parse(" Shadow "), Some(GlobalMode::Shadow));
        assert_eq!(GlobalMode::parse("AUTO"), Some(GlobalMode::Auto));
        assert_eq!(GlobalMode::parse("off"), Some(GlobalMode::Off));
        assert_eq!(GlobalMode::parse("never"), None);
        assert_eq!(GlobalMode::parse("bogus"), None);
        assert_eq!(GlobalMode::parse(""), None);
    }

    #[test]
    fn global_mode_orders_off_shadow_auto() {
        assert!(GlobalMode::Off < GlobalMode::Shadow);
        assert!(GlobalMode::Shadow < GlobalMode::Auto);
        assert_eq!(GlobalMode::Auto.min(GlobalMode::Shadow), GlobalMode::Shadow);
    }

    #[test]
    fn origin_strings() {
        assert_eq!(SettingOrigin::Default.as_str(), "default");
        assert_eq!(SettingOrigin::User.as_str(), "user");
        assert_eq!(SettingOrigin::Env.as_str(), "env");
        assert_eq!(
            serde_json::to_string(&SettingOrigin::Env).unwrap(),
            "\"env\""
        );
    }
}
