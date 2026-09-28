//! The environment a session's commands run with
//! (DESIGN-koto-fixed-environment.md).
//!
//! A session records three values at creation -- `PATH`, `HOME` and
//! `XDG_CONFIG_HOME` -- and a list of names whose values are read live on
//! every tick. Everything else is absent from a command's environment. This
//! module owns the lists, the name rules, the `PATH` normalization and the
//! building of a record from a process environment. It reads no file and
//! spawns nothing.
//!
//! Values are recorded only for the three fixed variables, which locate
//! tools and configuration and hold no secret. The names on the default
//! live list can carry credentials, so only their names are ever written.

use crate::engine::types::CommandEnvironment;

/// Variables recorded by value when a session is created.
pub const FIXED_NAMES: [&str; 3] = ["PATH", "HOME", "XDG_CONFIG_HOME"];

/// Names koto sets itself on every command. A value from the ticking
/// process or a template declaration never replaces them.
pub const KOTO_SET_NAMES: [&str; 2] = [
    crate::engine::reentrancy::TICK_SESSION_ENV,
    "KOTO_SESSIONS_BASE",
];

/// Names whose live values reach a session's commands by default.
///
/// Published as an exact list in the documentation. A session stores the
/// list it was created with, so an upgrade that changes this constant does
/// not change what an existing session's commands see.
pub const DEFAULT_LIVE_NAMES: &[&str] = &[
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_COLLATE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_MONETARY",
    "TZ",
    "TMPDIR",
    "TERM",
    "NO_COLOR",
    "CI",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "SSH_AUTH_SOCK",
    "DBUS_SESSION_BUS_ADDRESS",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GH_HOST",
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Default live names whose values can carry a credential: the tokens, and
/// the proxy URLs, which may embed a user and password. A fixed value that
/// contains one of these values is recorded unset rather than written.
const CREDENTIAL_CARRIERS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// A credential value shorter than this is too short to search for without
/// matching ordinary path text by accident.
const MIN_CREDENTIAL_LEN: usize = 8;

/// Names refused exactly: they make the shell source a file at start-up, or
/// make `git` or `gh` run a program or load configuration of the caller's
/// choosing.
const REFUSED_EXACT: &[&str] = &[
    "BASH_ENV",
    "ENV",
    "GIT_SSH_COMMAND",
    "GIT_ASKPASS",
    "GH_CONFIG_DIR",
];

/// Name prefixes refused: exported bash functions, and git's
/// environment-supplied configuration (`GIT_CONFIG`, `GIT_CONFIG_COUNT`,
/// `GIT_CONFIG_KEY_<n>`, `GIT_CONFIG_GLOBAL`, ...).
const REFUSED_PREFIXES: &[&str] = &["BASH_FUNC_", "GIT_CONFIG"];

/// Whether `name` is a well-formed environment variable name:
/// `^[A-Za-z_][A-Za-z0-9_]*$`.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` may never reach a command.
pub fn is_refused(name: &str) -> bool {
    REFUSED_EXACT.contains(&name) || REFUSED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Whether koto supplies `name` itself, so a declaration of it has no effect.
pub fn is_koto_supplied(name: &str) -> bool {
    FIXED_NAMES.contains(&name) || KOTO_SET_NAMES.contains(&name)
}

/// Check one name a template declares in `pass_env:`.
pub fn validate_declared_name(name: &str) -> Result<(), String> {
    if !is_valid_name(name) {
        return Err(format!(
            "pass_env: {:?} is not a variable name\n  \
             remedy: a name matches ^[A-Za-z_][A-Za-z0-9_]*$; list names, not patterns or values",
            name
        ));
    }
    if is_refused(name) {
        return Err(format!(
            "pass_env: {:?} can never be passed to a command\n  \
             remedy: remove it; it makes the shell, git or gh run code or load configuration \
             chosen by whoever ticks the session",
            name
        ));
    }
    Ok(())
}

/// `PATH` after dropping the entries that resolve relative to wherever a
/// command starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPath {
    /// The surviving entries joined with `:`, or `None` when none survived.
    pub path: Option<String>,
    /// Entries removed, in order. An empty entry is reported as `""`.
    pub dropped: Vec<String>,
}

/// Drop empty and relative entries from a `PATH` value.
///
/// Decided from the string alone. An empty entry, `.`, `bin`,
/// `node_modules/.bin` and `~/bin` all resolve against the directory a
/// command starts in -- the execution anchor, inside the repository under
/// review -- because neither `execvp` nor the shell's lookup expands `~`.
pub fn normalize_path(raw: &str) -> NormalizedPath {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for entry in raw.split(':') {
        if entry.starts_with('/') {
            kept.push(entry);
        } else {
            dropped.push(entry.to_string());
        }
    }
    NormalizedPath {
        path: (!kept.is_empty()).then(|| kept.join(":")),
        dropped,
    }
}

/// Why a fixed variable that was set is recorded unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsetReason {
    /// The value was not an absolute path (every `PATH` entry was dropped,
    /// or `HOME` or `XDG_CONFIG_HOME` was relative).
    NotAbsolute,
    /// The value contained the value of a credential-carrying variable.
    Credential,
}

impl UnsetReason {
    pub fn as_str(self) -> &'static str {
        match self {
            UnsetReason::NotAbsolute => "not-absolute",
            UnsetReason::Credential => "credential",
        }
    }
}

/// What recording changed about the creating process's values, for the
/// `koto init` response. Carries no value of any variable except the
/// dropped `PATH` entries, which are path text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordReport {
    pub dropped_path_entries: Vec<String>,
    pub unset: Vec<(&'static str, UnsetReason)>,
}

impl RecordReport {
    pub fn is_empty(&self) -> bool {
        self.dropped_path_entries.is_empty() && self.unset.is_empty()
    }

    /// The report as the `environment` object of a `koto init` response.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "dropped_path_entries": self.dropped_path_entries,
            "unset": self.unset.iter().map(|(name, why)| serde_json::json!({
                "variable": name,
                "reason": why.as_str(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Build a record from an environment read through `lookup`.
///
/// `lookup` returns a variable's value, or `None` when it is unset or not
/// valid UTF-8. The same function serves `koto init` (the invoking process)
/// and the first tick of an older session (the ticking process).
pub fn record_from<F>(lookup: F, legacy: bool) -> (CommandEnvironment, RecordReport)
where
    F: Fn(&str) -> Option<String>,
{
    let mut report = RecordReport::default();
    let credentials: Vec<String> = CREDENTIAL_CARRIERS
        .iter()
        .filter_map(|n| lookup(n))
        .filter(|v| v.len() >= MIN_CREDENTIAL_LEN)
        .collect();
    let carries_credential = |v: &str| credentials.iter().any(|c| v.contains(c.as_str()));

    let path = lookup("PATH").and_then(|raw| {
        if carries_credential(&raw) {
            report.unset.push(("PATH", UnsetReason::Credential));
            return None;
        }
        let normalized = normalize_path(&raw);
        report.dropped_path_entries = normalized.dropped;
        if normalized.path.is_none() {
            report.unset.push(("PATH", UnsetReason::NotAbsolute));
        }
        normalized.path
    });

    let mut absolute = |name: &'static str| {
        lookup(name).and_then(|v| {
            if carries_credential(&v) {
                report.unset.push((name, UnsetReason::Credential));
                None
            } else if !v.starts_with('/') {
                report.unset.push((name, UnsetReason::NotAbsolute));
                None
            } else {
                Some(v)
            }
        })
    };
    let home = absolute("HOME");
    let xdg_config_home = absolute("XDG_CONFIG_HOME");

    let record = CommandEnvironment {
        path,
        home,
        xdg_config_home,
        pass: DEFAULT_LIVE_NAMES.iter().map(|s| s.to_string()).collect(),
        legacy,
    };
    (record, report)
}

/// Build a record from this process's environment.
pub fn record_from_process(legacy: bool) -> (CommandEnvironment, RecordReport) {
    record_from(|name| std::env::var(name).ok(), legacy)
}

/// The fixed variables whose values in a caller's environment, read through
/// `lookup`, differ from a session's record, in `FIXED_NAMES` order.
///
/// Names only: an attach reports which variables drifted, never a value,
/// the same rule as the `koto init` response. The caller's values are
/// normalized exactly as a record would be, so a `PATH` differing only in
/// entries recording drops reports no drift.
pub fn drift<F>(record: &CommandEnvironment, lookup: F) -> Vec<&'static str>
where
    F: Fn(&str) -> Option<String>,
{
    let (caller, _) = record_from(lookup, false);
    let mut drifted = Vec::new();
    if record.path != caller.path {
        drifted.push("PATH");
    }
    if record.home != caller.home {
        drifted.push("HOME");
    }
    if record.xdg_config_home != caller.xdg_config_home {
        drifted.push("XDG_CONFIG_HOME");
    }
    drifted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn names_follow_the_pattern() {
        for good in ["PATH", "_x", "GH_DB", "a1_b2"] {
            assert!(is_valid_name(good), "{good}");
        }
        for bad in ["", "1A", "A-B", "A*", "BASH_FUNC_f%%", "GH TOKEN", "A=B"] {
            assert!(!is_valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_refused_list_is_exactly_the_injection_names() {
        for refused in [
            "BASH_ENV",
            "ENV",
            "BASH_FUNC_whoami",
            "GIT_CONFIG",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_GLOBAL",
            "GIT_SSH_COMMAND",
            "GIT_ASKPASS",
            "GH_CONFIG_DIR",
        ] {
            assert!(is_refused(refused), "{refused}");
        }
        for allowed in [
            "GIT_DIR",
            "GIT_CEILING_DIRECTORIES",
            "EDITOR",
            "LD_PRELOAD",
            "KOTO_BIN",
            "GH_HOST",
            "ENVIRONMENT",
            "PATH",
        ] {
            assert!(!is_refused(allowed), "{allowed}");
        }
    }

    #[test]
    fn declared_names_are_checked() {
        assert!(validate_declared_name("GH_DB").is_ok());
        assert!(validate_declared_name("PATH").is_ok());
        assert!(validate_declared_name("BASH_ENV").is_err());
        assert!(validate_declared_name("GIT_CONFIG_COUNT").is_err());
        assert!(validate_declared_name("1BAD").is_err());
        assert!(validate_declared_name("A*").is_err());
    }

    #[test]
    fn no_default_name_is_refused_or_fixed() {
        for name in DEFAULT_LIVE_NAMES {
            assert!(is_valid_name(name), "{name}");
            assert!(!is_refused(name), "{name}");
            assert!(!is_koto_supplied(name), "{name}");
        }
    }

    #[test]
    fn every_credential_carrier_is_a_default_live_name() {
        // The check compares fixed values against these variables' live
        // values; a carrier that isn't on the default list would never be
        // passed to a command, so checking it would guard nothing.
        for name in CREDENTIAL_CARRIERS {
            assert!(DEFAULT_LIVE_NAMES.contains(name), "{name}");
        }
    }

    #[test]
    fn relative_and_empty_path_entries_are_dropped() {
        let n = normalize_path(":/usr/bin:.:bin::node_modules/.bin:~/bin:/bin:");
        assert_eq!(n.path.as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(
            n.dropped,
            vec!["", ".", "bin", "", "node_modules/.bin", "~/bin", ""]
        );
    }

    #[test]
    fn a_path_with_nothing_absolute_is_unset() {
        let n = normalize_path(".:bin");
        assert_eq!(n.path, None);
        let (rec, report) = record_from(env(&[("PATH", ".:bin")]), false);
        assert_eq!(rec.path, None);
        assert!(report.unset.contains(&("PATH", UnsetReason::NotAbsolute)));
    }

    #[test]
    fn a_record_carries_the_three_values_and_the_default_names() {
        let (rec, report) = record_from(
            env(&[
                ("PATH", "/a:/b"),
                ("HOME", "/home/u"),
                ("XDG_CONFIG_HOME", "/home/u/.config"),
                ("TMPDIR", "/tmp/x"),
            ]),
            false,
        );
        assert_eq!(rec.path.as_deref(), Some("/a:/b"));
        assert_eq!(rec.home.as_deref(), Some("/home/u"));
        assert_eq!(rec.xdg_config_home.as_deref(), Some("/home/u/.config"));
        assert_eq!(rec.pass.len(), DEFAULT_LIVE_NAMES.len());
        assert!(!rec.legacy);
        assert!(report.is_empty());
    }

    #[test]
    fn unset_stays_unset_and_relative_homes_are_unset() {
        let (rec, report) = record_from(env(&[("HOME", "relative/home")]), true);
        assert_eq!(rec.path, None);
        assert_eq!(rec.home, None);
        assert_eq!(rec.xdg_config_home, None);
        assert!(rec.legacy);
        assert_eq!(report.unset, vec![("HOME", UnsetReason::NotAbsolute)]);
    }

    #[test]
    fn a_fixed_value_carrying_a_credential_is_unset() {
        let token = "ghp_0123456789abcdef";
        let home = format!("/home/{token}");
        let path = format!("/usr/bin:/opt/{token}/bin");
        let (rec, report) = record_from(
            env(&[("GH_TOKEN", token), ("HOME", &home), ("PATH", &path)]),
            false,
        );
        assert_eq!(rec.home, None);
        assert_eq!(rec.path, None);
        assert!(report.unset.contains(&("HOME", UnsetReason::Credential)));
        assert!(report.unset.contains(&("PATH", UnsetReason::Credential)));
        let rendered = report.to_json().to_string();
        assert!(!rendered.contains(token));
    }

    #[test]
    fn drift_compares_normalized_values_and_names_only() {
        let (record, _) = record_from(
            env(&[("PATH", "/usr/bin:/bin"), ("HOME", "/home/u")]),
            false,
        );
        // Only a dropped entry differs: no drift.
        assert!(drift(
            &record,
            env(&[("PATH", ".:/usr/bin:/bin:"), ("HOME", "/home/u")])
        )
        .is_empty());
        let d = drift(
            &record,
            env(&[
                ("PATH", "/opt/new:/usr/bin:/bin"),
                ("HOME", "/home/u"),
                ("XDG_CONFIG_HOME", "/home/u/.config"),
            ]),
        );
        assert_eq!(d, vec!["PATH", "XDG_CONFIG_HOME"]);
    }

    #[test]
    fn a_short_credential_is_not_searched_for() {
        let (rec, report) = record_from(env(&[("GH_TOKEN", "bin"), ("PATH", "/usr/bin")]), false);
        assert_eq!(rec.path.as_deref(), Some("/usr/bin"));
        assert!(report.is_empty());
    }
}
