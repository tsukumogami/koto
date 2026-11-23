//! One `[pools.<name>]` table from the user's koto config.
//!
//! Pools are defined only in `<koto root>/config.toml` (PRD R5), and are read
//! each time a pooled action is reached (R8), so a resize takes effect on the
//! next tick. The table is parsed here and nowhere else: a bad value makes
//! that one pool invalid (R7) rather than failing every config load, the same
//! way retention values are kept as written and judged where they are used.

use std::path::Path;

/// The most slots a pool may have.
pub const MAX_SIZE: i64 = 1024;
/// The most a pool's `claim_secs` or `lapse_secs` may be.
pub const MAX_PERIOD_SECS: i64 = 86_400;
/// The most a pool's `wait_secs` may be.
pub const MAX_WAIT_SECS: i64 = 60;

pub const DEFAULT_CLAIM_SECS: u32 = 120;
pub const DEFAULT_LAPSE_SECS: u32 = 600;
pub const DEFAULT_WAIT_SECS: u32 = 10;

/// A defined pool's settings, every value inside its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSpec {
    pub size: u32,
    pub claim_secs: u32,
    pub lapse_secs: u32,
    pub wait_secs: u32,
}

/// What the user config says about one pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolLookup {
    /// No `[pools.<name>]` table, or no config file: the pool imposes no
    /// limit (R6).
    Undefined,
    Defined(PoolSpec),
    /// The table exists but a value is missing or out of bounds. `key` is
    /// the dotted key at fault.
    Invalid {
        key: String,
        reason: String,
    },
}

/// Look `name` up in the config file at `path`.
///
/// A file that doesn't mention `pools` at all is answered without parsing
/// it, so a tick on a machine without pools pays one read (R28).
pub fn lookup(path: &Path, name: &str) -> PoolLookup {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return PoolLookup::Undefined,
    };
    if !text.contains("pools") {
        return PoolLookup::Undefined;
    }
    lookup_in(&text, name)
}

/// [`lookup`] on the config file's text.
pub fn lookup_in(text: &str, name: &str) -> PoolLookup {
    let value: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(e) => {
            return PoolLookup::Invalid {
                key: format!("pools.{name}"),
                reason: format!("the config file does not parse: {e}"),
            }
        }
    };
    let Some(table) = value
        .get("pools")
        .and_then(|p| p.as_table())
        .and_then(|p| p.get(name))
    else {
        return PoolLookup::Undefined;
    };
    let Some(table) = table.as_table() else {
        return PoolLookup::Invalid {
            key: format!("pools.{name}"),
            reason: "must be a table".to_string(),
        };
    };
    let field = |key: &str, min: i64, max: i64, default: Option<u32>| -> Result<u32, PoolLookup> {
        let invalid = |reason: String| PoolLookup::Invalid {
            key: format!("pools.{name}.{key}"),
            reason,
        };
        match table.get(key) {
            None => default.ok_or_else(|| invalid("is required".to_string())),
            Some(toml::Value::Integer(n)) if (min..=max).contains(n) => Ok(*n as u32),
            Some(_) => Err(invalid(format!(
                "must be a whole number from {min} to {max}"
            ))),
        }
    };
    let spec = (|| {
        Ok(PoolSpec {
            size: field("size", 1, MAX_SIZE, None)?,
            claim_secs: field("claim_secs", 1, MAX_PERIOD_SECS, Some(DEFAULT_CLAIM_SECS))?,
            lapse_secs: field("lapse_secs", 1, MAX_PERIOD_SECS, Some(DEFAULT_LAPSE_SECS))?,
            wait_secs: field("wait_secs", 0, MAX_WAIT_SECS, Some(DEFAULT_WAIT_SECS))?,
        })
    })();
    match spec {
        Ok(spec) => PoolLookup::Defined(spec),
        Err(invalid) => invalid,
    }
}

/// The default `[background] max_timeout_secs` (PRD R19).
pub const DEFAULT_MAX_TIMEOUT_SECS: u32 = 7200;
/// The most `[background] max_timeout_secs` may be: a template's own
/// `timeout_secs` bound.
pub const MAX_BACKGROUND_TIMEOUT_SECS: i64 = 86_400;

/// The user config's `[background]` table: whether background actions may
/// run on this machine, and the longest any of them may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundSpec {
    pub enabled: bool,
    pub max_timeout_secs: u32,
}

impl Default for BackgroundSpec {
    fn default() -> Self {
        BackgroundSpec {
            enabled: true,
            max_timeout_secs: DEFAULT_MAX_TIMEOUT_SECS,
        }
    }
}

/// A `[background]` value koto can't use: `key` is the dotted key at
/// fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundInvalid {
    pub key: String,
    pub reason: String,
}

/// Read `[background]` from the user config file at `path`. Like a pool,
/// it is user config only and judged where it is used, so a bad value
/// stops background actions alone rather than every koto command. A file
/// that never mentions `background` is answered without parsing it.
pub fn background(path: &Path) -> Result<BackgroundSpec, BackgroundInvalid> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.contains("background") => background_in(&text),
        _ => Ok(BackgroundSpec::default()),
    }
}

/// [`background`] on the config file's text.
pub fn background_in(text: &str) -> Result<BackgroundSpec, BackgroundInvalid> {
    let invalid = |key: &str, reason: String| BackgroundInvalid {
        key: key.to_string(),
        reason,
    };
    let value: toml::Value = text
        .parse()
        .map_err(|e| invalid("background", format!("the config file does not parse: {e}")))?;
    let Some(table) = value.get("background") else {
        return Ok(BackgroundSpec::default());
    };
    let Some(table) = table.as_table() else {
        return Err(invalid("background", "must be a table".to_string()));
    };
    let mut spec = BackgroundSpec::default();
    match table.get("enabled") {
        None => {}
        Some(toml::Value::Boolean(b)) => spec.enabled = *b,
        Some(_) => {
            return Err(invalid(
                "background.enabled",
                "must be true or false".to_string(),
            ))
        }
    }
    match table.get("max_timeout_secs") {
        None => {}
        Some(toml::Value::Integer(n)) if (1..=MAX_BACKGROUND_TIMEOUT_SECS).contains(n) => {
            spec.max_timeout_secs = *n as u32
        }
        Some(_) => {
            return Err(invalid(
                "background.max_timeout_secs",
                format!("must be a whole number from 1 to {MAX_BACKGROUND_TIMEOUT_SECS}"),
            ))
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_defaults_and_values() {
        assert_eq!(background_in(""), Ok(BackgroundSpec::default()));
        assert_eq!(
            background_in("[background]\nenabled = false\nmax_timeout_secs = 10\n"),
            Ok(BackgroundSpec {
                enabled: false,
                max_timeout_secs: 10,
            })
        );
        for (text, key) in [
            ("[background]\nenabled = \"no\"\n", "background.enabled"),
            (
                "[background]\nmax_timeout_secs = 0\n",
                "background.max_timeout_secs",
            ),
            (
                "[background]\nmax_timeout_secs = 86401\n",
                "background.max_timeout_secs",
            ),
            ("background = 3\n", "background"),
        ] {
            assert_eq!(background_in(text).unwrap_err().key, key, "{text}");
        }
    }

    #[test]
    fn an_absent_table_is_undefined() {
        assert_eq!(lookup_in("", "test-suite"), PoolLookup::Undefined);
        assert_eq!(
            lookup_in("[pools.other]\nsize = 2\n", "test-suite"),
            PoolLookup::Undefined
        );
    }

    #[test]
    fn defaults_fill_the_optional_keys() {
        assert_eq!(
            lookup_in("[pools.test-suite]\nsize = 2\n", "test-suite"),
            PoolLookup::Defined(PoolSpec {
                size: 2,
                claim_secs: 120,
                lapse_secs: 600,
                wait_secs: 10,
            })
        );
    }

    #[test]
    fn a_missing_or_out_of_range_value_names_its_key() {
        for (text, key) in [
            ("[pools.p]\nwait_secs = 1\n", "pools.p.size"),
            ("[pools.p]\nsize = 0\n", "pools.p.size"),
            ("[pools.p]\nsize = 1025\n", "pools.p.size"),
            ("[pools.p]\nsize = \"2\"\n", "pools.p.size"),
            ("[pools.p]\nsize = 1\nwait_secs = 61\n", "pools.p.wait_secs"),
            (
                "[pools.p]\nsize = 1\nclaim_secs = 0\n",
                "pools.p.claim_secs",
            ),
            (
                "[pools.p]\nsize = 1\nlapse_secs = 86401\n",
                "pools.p.lapse_secs",
            ),
        ] {
            match lookup_in(text, "p") {
                PoolLookup::Invalid { key: k, .. } => assert_eq!(k, key, "{text}"),
                other => panic!("{text}: expected invalid, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_file_without_pools_is_not_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Not TOML at all: answered without parsing.
        std::fs::write(&path, "this is [not toml").unwrap();
        assert_eq!(lookup(&path, "p"), PoolLookup::Undefined);
        assert_eq!(
            lookup(&dir.path().join("missing"), "p"),
            PoolLookup::Undefined
        );
    }
}
