//! The environment a session's commands run with
//! (DESIGN-koto-fixed-environment.md).
//!
//! A session records three values at creation -- `PATH`, `HOME` and
//! `XDG_CONFIG_HOME` -- and a list of names whose values are read live on
//! every tick. Everything else is absent from a command's environment. This
//! module owns the lists, the name rules, the `PATH` normalization, the
//! building of a record from a process environment, the per-tick
//! [`CommandEnv`] built from a record, and the stale check. It spawns
//! nothing; the stale check is a few `stat` calls.
//!
//! Values are recorded only for the three fixed variables, which locate
//! tools and configuration and hold no secret. The names on the default
//! live list can carry credentials, so only their names are ever written.

use std::path::Path;

use crate::action::CommandEnv;
use crate::engine::types::CommandEnvironment;

/// The `PATH` a command gets when its session recorded none. Never the
/// shell's built-in default, which in upstream bash ends in `.`.
pub const UNSET_PATH: &str = "/usr/bin:/bin";

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
/// matching ordinary path text by accident. The same floor applies to the
/// values redaction searches captured output for.
const MIN_CREDENTIAL_LEN: usize = crate::redact::MIN_VALUE_LEN;

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
    let path_absent = path
        .as_deref()
        .map(|p| {
            p.split(':')
                .filter(|dir| !Path::new(dir).is_dir())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let absent = |v: &Option<String>| v.as_deref().is_some_and(|v| !Path::new(v).exists());
    let home_absent = absent(&home);
    let xdg_config_home_absent = absent(&xdg_config_home);

    let record = CommandEnvironment {
        path,
        path_absent,
        home,
        xdg_config_home,
        home_absent,
        xdg_config_home_absent,
        pass: DEFAULT_LIVE_NAMES.iter().map(|s| s.to_string()).collect(),
        legacy,
    };
    (record, report)
}

/// The recorded values a tick found missing, as (name, value) pairs; see
/// [`stale`].
pub type StaleValues = Vec<(&'static str, String)>;

/// A tick found a session with no command environment record.
///
/// Adoption gives every session a record before its commands run, so this
/// can't happen through the CLI today. If it ever does, the tick refuses
/// rather than falling back to the caller's environment: this is the boundary
/// the record exists to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingRecord;

impl std::fmt::Display for MissingRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the session has no recorded command environment, so its commands can't run; \
             nothing ran",
        )
    }
}

/// The tick's [`CommandEnv`] and stale list for a session's record, or
/// [`MissingRecord`] when there is none. Never falls back to this process's
/// environment for a missing record.
///
/// `config_keys` are koto's own configured secrets as `(source, value)`
/// pairs, from the configuration the caller already loaded (see
/// `crate::config::redaction_keys`). They join the tick's known set, built by
/// [`known_credentials`] and stored on the returned environment, so every
/// command's output is redacted against it.
///
/// This is the only public way to build a tick's environment, so a tick's
/// commands can't run without the known set.
pub fn for_tick<F>(
    record: Option<&CommandEnvironment>,
    pass_env: &[String],
    session: &str,
    sessions_base: Option<&Path>,
    config_keys: &[(String, String)],
    lookup: F,
) -> Result<(CommandEnv, StaleValues), MissingRecord>
where
    F: Fn(&str) -> Option<String>,
{
    let record = record.ok_or(MissingRecord)?;
    Ok((
        build_command_env(
            record,
            pass_env,
            session,
            sessions_base,
            config_keys,
            &lookup,
        ),
        stale(record),
    ))
}

/// The password in a proxy URL's userinfo (`scheme://user:password@host`),
/// in both spellings a command can print: percent-decoded, and as written in
/// the URL. Empty when there is no password; the decoded spelling is left
/// out when it doesn't decode to UTF-8, and the written one when it equals
/// the decoded one.
fn proxy_passwords(url: &str) -> Vec<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let Some((userinfo, _)) = authority.rsplit_once('@') else {
        return Vec::new();
    };
    let Some((_, password)) = userinfo.split_once(':') else {
        return Vec::new();
    };
    let bytes = password.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Only `%` and two hex digits is an escape; `from_str_radix` alone
        // would also take a sign, so `%+f` stays as written.
        let digit = |k: usize| bytes.get(k).and_then(|&b| (b as char).to_digit(16));
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (digit(i + 1), digit(i + 2)) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut spellings = Vec::new();
    if let Ok(decoded) = String::from_utf8(out) {
        spellings.push(decoded);
    }
    if !spellings.iter().any(|s| s == password) {
        spellings.push(password.to_string());
    }
    spellings
}

/// The tick's known credentials, as a [`crate::redact::Redactor`] for captured output
/// (DESIGN-koto-failure-reporting.md, Decision 5).
///
/// In priority order, the first source naming a value that several carry:
/// the live value of each credential-carrying variable, plus the password
/// of any proxy URL among them, both percent-decoded and as written; the
/// value of each template `pass_env:` name that reaches a command (the same
/// `passed_names` filter the command's environment uses); then `config_keys`;
/// then the environment variables that supply koto's own keys
/// (`crate::config::resolve::ENV_SECRET_NAMES`), read through `lookup`
/// directly, so they're searched for even when the configuration failed to
/// load and `config_keys` is empty. A legacy
/// session's commands inherit the caller's environment, which `lookup`
/// reads, so the same names are looked up there. The names on the record's
/// default list other than the carriers hold no secret and aren't searched
/// for. Values shorter than [`crate::redact::MIN_VALUE_LEN`] bytes are
/// dropped by the redactor.
pub fn known_credentials<F>(
    record: &CommandEnvironment,
    pass_env: &[String],
    config_keys: &[(String, String)],
    lookup: F,
) -> crate::redact::Redactor
where
    F: Fn(&str) -> Option<String>,
{
    let mut known: Vec<(String, String)> = Vec::new();
    for name in CREDENTIAL_CARRIERS {
        if let Some(value) = lookup(name) {
            let passwords = proxy_passwords(&value);
            known.push((name.to_string(), value));
            for password in passwords {
                known.push((name.to_string(), password));
            }
        }
    }

    for (from_record, name) in passed_names(record, pass_env) {
        // A name from the record's default list reaches the command but holds
        // no secret (the carriers among them were added above). It still goes
        // through the filter first, so a `pass_env:` duplicate of it is
        // skipped rather than searched for.
        if from_record {
            continue;
        }
        if let Some(value) = lookup(name) {
            known.push((name.to_string(), value));
        }
    }

    known.extend(config_keys.iter().cloned());
    for name in crate::config::resolve::ENV_SECRET_NAMES {
        if let Some(value) = lookup(name).filter(|v| !v.trim().is_empty()) {
            known.push((name.to_string(), value));
        }
    }
    crate::redact::Redactor::new(known)
}

/// The names whose live values reach a non-legacy tick's commands: the
/// record's list, then the template's `pass_env`, each at most once, leaving
/// out refused names, the fixed names and the names koto sets. Each comes
/// with whether it came from the record's list.
///
/// [`build_command_env`] and [`known_credentials`] both use this, so a value
/// that reaches a command is always one the known set searches for.
fn passed_names<'a>(
    record: &'a CommandEnvironment,
    pass_env: &'a [String],
) -> impl Iterator<Item = (bool, &'a str)> {
    let mut seen: Vec<&'a str> = Vec::new();
    let from_record = record.pass.iter().map(|n| (true, n.as_str()));
    let from_template = pass_env.iter().map(|n| (false, n.as_str()));
    from_record.chain(from_template).filter(move |&(_, name)| {
        if seen.contains(&name)
            || is_refused(name)
            || FIXED_NAMES.contains(&name)
            || KOTO_SET_NAMES.contains(&name)
        {
            return false;
        }
        seen.push(name);
        true
    })
}

/// Build a record from this process's environment.
pub fn record_from_process(legacy: bool) -> (CommandEnvironment, RecordReport) {
    record_from(|name| std::env::var(name).ok(), legacy)
}

/// The fixed variables (`PATH`, `HOME`, `XDG_CONFIG_HOME`, in that order)
/// whose values in a caller's environment, read through `lookup`, differ
/// from a session's record.
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

/// The environment one tick's commands run with, built from the session's
/// record.
///
/// A legacy record gives this process's environment plus `KOTO_TICK_SESSION`,
/// which is koto's behaviour before records existed. Otherwise the result is,
/// in order: the live value of each name in the record's list and the
/// template's `pass_env` that `lookup` finds, leaving out refused names and the
/// names koto sets; the three fixed values, `PATH` becoming [`UNSET_PATH`] when
/// the record has none; then `KOTO_TICK_SESSION` and, when given,
/// `KOTO_SESSIONS_BASE`. Nothing else reaches a command.
///
/// The result always carries the tick's known set ([`known_credentials`]
/// over `config_keys` and `lookup`), so every command run under it has its
/// output redacted. Private: [`for_tick`] is the public entry.
fn build_command_env<F>(
    record: &CommandEnvironment,
    pass_env: &[String],
    session: &str,
    sessions_base: Option<&Path>,
    config_keys: &[(String, String)],
    lookup: F,
) -> CommandEnv
where
    F: Fn(&str) -> Option<String>,
{
    let redactor = known_credentials(record, pass_env, config_keys, &lookup);
    let tick = (
        crate::engine::reentrancy::TICK_SESSION_ENV.to_string(),
        session.to_string(),
    );
    if record.legacy {
        return CommandEnv::inherited(vec![tick], redactor);
    }

    let mut vars: Vec<(String, String)> = Vec::new();
    for (_, name) in passed_names(record, pass_env) {
        if let Some(value) = lookup(name) {
            vars.push((name.to_string(), value));
        }
    }

    vars.push((
        "PATH".to_string(),
        record
            .path
            .clone()
            .unwrap_or_else(|| UNSET_PATH.to_string()),
    ));
    if let Some(home) = &record.home {
        vars.push(("HOME".to_string(), home.clone()));
    }
    if let Some(xdg) = &record.xdg_config_home {
        vars.push(("XDG_CONFIG_HOME".to_string(), xdg.clone()));
    }
    vars.push(tick);
    if let Some(base) = sessions_base {
        vars.push((
            "KOTO_SESSIONS_BASE".to_string(),
            base.to_string_lossy().into_owned(),
        ));
    }
    CommandEnv::cleared(vars, redactor)
}

/// The recorded values that no longer exist on disk: each `PATH` directory
/// that existed when it was recorded, `HOME` and `XDG_CONFIG_HOME`, as (name,
/// value) pairs in that order. A value that was already missing then is never
/// reported: a `PATH` entry is ordinary (a directory a tool manager would
/// create), and a `HOME` or `XDG_CONFIG_HOME` a new session would record just
/// the same, so the remedy the report names wouldn't help.
///
/// A legacy record is never stale; its commands use the caller's values.
pub fn stale(record: &CommandEnvironment) -> StaleValues {
    let mut missing = Vec::new();
    if record.legacy {
        return missing;
    }
    if let Some(path) = &record.path {
        for dir in path.split(':') {
            if !record.path_absent.iter().any(|a| a == dir) && !Path::new(dir).is_dir() {
                missing.push(("PATH", dir.to_string()));
            }
        }
    }
    for (name, value, absent_at_creation) in [
        ("HOME", &record.home, record.home_absent),
        (
            "XDG_CONFIG_HOME",
            &record.xdg_config_home,
            record.xdg_config_home_absent,
        ),
    ] {
        if let Some(value) = value {
            if !absent_at_creation && !Path::new(value).exists() {
                missing.push((name, value.clone()));
            }
        }
    }
    missing
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

    fn record(path: Option<&str>, pass: &[&str]) -> CommandEnvironment {
        CommandEnvironment {
            path: path.map(str::to_string),
            path_absent: Vec::new(),
            home_absent: false,
            xdg_config_home_absent: false,
            home: Some("/home/someone".to_string()),
            xdg_config_home: None,
            pass: pass.iter().map(|n| n.to_string()).collect(),
            legacy: false,
        }
    }

    #[test]
    fn a_command_env_holds_listed_names_then_fixed_values_then_koto_s() {
        let rec = record(Some("/opt/bin:/usr/bin"), &["LANG", "BASH_ENV", "HOME"]);
        let lookup = env(&[
            ("LANG", "C.UTF-8"),
            ("BASH_ENV", "/tmp/evil"),
            ("GH_DB", "live"),
            ("UNLISTED", "x"),
            ("HOME", "/somewhere/else"),
            ("KOTO_TICK_SESSION", "spoofed"),
            ("BASH_FUNC_probe%%", "() {  true\n}"),
            ("GIT_CONFIG_COUNT", "1"),
        ]);
        // A refused name can't compile into `pass_env`, but the builder filters
        // it again rather than trusting that.
        let pass_env = vec![
            "GH_DB".to_string(),
            "KOTO_TICK_SESSION".to_string(),
            "BASH_FUNC_probe%%".to_string(),
            "GIT_CONFIG_COUNT".to_string(),
        ];
        let built = build_command_env(&rec, &pass_env, "wf", Some(Path::new("/s")), &[], lookup);
        assert!(!built.inherits());
        assert_eq!(built.get("LANG"), Some("C.UTF-8"));
        assert_eq!(built.get("GH_DB"), Some("live"));
        assert_eq!(built.get("BASH_ENV"), None);
        assert_eq!(built.get("BASH_FUNC_probe%%"), None);
        assert_eq!(built.get("GIT_CONFIG_COUNT"), None);
        assert_eq!(built.get("UNLISTED"), None);
        assert_eq!(built.get("PATH"), Some("/opt/bin:/usr/bin"));
        assert_eq!(built.get("HOME"), Some("/home/someone"));
        assert_eq!(built.get("XDG_CONFIG_HOME"), None);
        assert_eq!(built.get("KOTO_TICK_SESSION"), Some("wf"));
        assert_eq!(built.get("KOTO_SESSIONS_BASE"), Some("/s"));
    }

    #[test]
    fn an_unset_path_becomes_the_system_directories() {
        let built = build_command_env(&record(None, &[]), &[], "wf", None, &[], env(&[]));
        assert_eq!(built.get("PATH"), Some(UNSET_PATH));
        assert_eq!(built.get("KOTO_SESSIONS_BASE"), None);
    }

    #[test]
    fn a_legacy_record_inherits_and_adds_the_tick_session() {
        let mut rec = record(Some("/usr/bin"), &[]);
        rec.legacy = true;
        let built = build_command_env(&rec, &[], "wf", Some(Path::new("/s")), &[], env(&[]));
        assert!(built.inherits());
        assert_eq!(built.get("KOTO_TICK_SESSION"), Some("wf"));
        assert_eq!(built.get("PATH"), None);
    }

    #[test]
    fn stale_names_missing_path_dirs_and_home() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().to_string_lossy().into_owned();
        let gone = dir.path().join("gone").to_string_lossy().into_owned();
        let mut rec = record(Some(&format!("{present}:{gone}")), &[]);
        rec.home = Some(gone.clone());
        rec.xdg_config_home = Some(present.clone());
        assert_eq!(
            stale(&rec),
            vec![("PATH", gone.clone()), ("HOME", gone.clone())]
        );
        // An entry that was already missing when recorded isn't stale.
        rec.path_absent = vec![gone.clone()];
        assert_eq!(stale(&rec), vec![("HOME", gone)]);
        rec.legacy = true;
        assert!(stale(&rec).is_empty());
    }

    #[test]
    fn a_tick_without_a_record_is_refused_not_given_the_callers_environment() {
        let refused = for_tick(None, &[], "wf", None, &[], env(&[("PATH", "/usr/bin")]));
        assert_eq!(refused.err(), Some(MissingRecord));
        let rec = record(Some("/usr/bin"), &[]);
        let (built, _) = for_tick(Some(&rec), &[], "wf", None, &[], env(&[])).unwrap();
        assert!(!built.inherits());
    }

    #[test]
    fn a_home_or_xdg_missing_at_recording_is_flagged_and_never_stale() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("never").to_string_lossy().into_owned();
        let present = dir.path().to_string_lossy().into_owned();
        let (rec, _) = record_from(
            env(&[
                ("HOME", gone.as_str()),
                ("XDG_CONFIG_HOME", present.as_str()),
            ]),
            false,
        );
        assert!(rec.home_absent);
        assert!(!rec.xdg_config_home_absent);
        assert!(
            stale(&rec).is_empty(),
            "a HOME missing at recording isn't stale"
        );

        let (rec, _) = record_from(env(&[("XDG_CONFIG_HOME", gone.as_str())]), false);
        assert!(rec.xdg_config_home_absent);
        assert!(stale(&rec).is_empty());
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

    fn redacted(r: &crate::redact::Redactor, text: &str) -> String {
        crate::redact::redact_str(text, r).into_string()
    }

    #[test]
    fn the_known_set_holds_carriers_proxy_passwords_pass_env_and_config_keys() {
        let rec = record(Some("/usr/bin"), &["LANG", "GH_TOKEN", "HTTPS_PROXY"]);
        let lookup = env(&[
            ("GH_TOKEN", "ghp_tokenvalue01"),
            ("HTTPS_PROXY", "http://me:p%40ssw0rd-long@proxy:3128"),
            ("LANG", "C.UTF-8-longish"),
            ("GH_DB", "db-secret-value"),
            ("BASH_ENV", "/tmp/evil-file"),
            ("PATH", "/usr/bin:/opt/secretish"),
        ]);
        let pass_env: Vec<String> = ["GH_DB", "BASH_ENV", "PATH", "LANG"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let config = vec![("decider.api_key".to_string(), "sk-config-value".to_string())];
        let r = known_credentials(&rec, &pass_env, &config, &lookup);
        assert_eq!(
            redacted(
                &r,
                "ghp_tokenvalue01 p@ssw0rd-long db-secret-value sk-config-value"
            ),
            "[REDACTED:GH_TOKEN] [REDACTED:HTTPS_PROXY] [REDACTED:GH_DB] \
             [REDACTED:decider.api_key]"
        );
        // Default live names other than the carriers, refused names, fixed
        // names and a pass_env name the record already passes aren't
        // searched for.
        for plain in [
            "C.UTF-8-longish",
            "/tmp/evil-file",
            "/usr/bin:/opt/secretish",
        ] {
            assert_eq!(redacted(&r, plain), plain);
        }
        assert!(!format!("{:?}", r).contains("tokenvalue"));
    }

    #[test]
    fn a_tick_env_carries_the_known_set_and_a_legacy_one_reads_the_caller() {
        let mut rec = record(Some("/usr/bin"), &[]);
        rec.legacy = true;
        let lookup = env(&[
            ("GITHUB_TOKEN", "ghp_legacyvalue1"),
            ("GH_DB", "legacy-db-value"),
        ]);
        let (built, _) =
            for_tick(Some(&rec), &["GH_DB".to_string()], "wf", None, &[], lookup).unwrap();
        assert!(built.inherits());
        assert_eq!(
            redacted(built.redactor(), "ghp_legacyvalue1 legacy-db-value"),
            "[REDACTED:GITHUB_TOKEN] [REDACTED:GH_DB]"
        );
        assert!(crate::action::CommandEnv::inherit().redactor().is_empty());
    }

    #[test]
    fn every_tick_env_is_built_with_the_known_set() {
        // A tick's environment must never reach a command without the known
        // set: each value a command can see, and koto's own keys, come out of
        // its redactor as markers, whichever way the environment is built.
        let rec = record(Some("/usr/bin"), &["GH_TOKEN"]);
        let lookup = env(&[
            ("GH_TOKEN", "ghp_tokenvalue01"),
            ("GH_DB", "db-secret-value"),
            ("KOTO_DECIDER_API_KEY", "sk-env-decider-key"),
        ]);
        let pass_env = vec!["GH_DB".to_string()];
        let config = vec![("decider.api_key".to_string(), "sk-config-value".to_string())];
        let text = "ghp_tokenvalue01 db-secret-value sk-config-value sk-env-decider-key";
        let want = "[REDACTED:GH_TOKEN] [REDACTED:GH_DB] [REDACTED:decider.api_key] \
                    [REDACTED:KOTO_DECIDER_API_KEY]";

        let (ticked, _) = for_tick(Some(&rec), &pass_env, "wf", None, &config, &lookup).unwrap();
        assert!(!ticked.inherits());
        assert_eq!(redacted(ticked.redactor(), text), want);

        let built = build_command_env(&rec, &pass_env, "wf", None, &config, &lookup);
        assert_eq!(redacted(built.redactor(), text), want);
    }

    #[test]
    fn proxy_passwords_are_searched_decoded_and_as_written() {
        assert_eq!(
            proxy_passwords("http://u:a%2Fb%3Ac@h:1/x"),
            vec!["a/b:c".to_string(), "a%2Fb%3Ac".to_string()]
        );
        assert_eq!(proxy_passwords("u:pw@h"), vec!["pw".to_string()]);
        assert!(proxy_passwords("http://u@h").is_empty());
        assert!(proxy_passwords("http://h:8080").is_empty());
        // Only `%` and two hex digits is an escape.
        assert_eq!(
            proxy_passwords("http://u:a%+fb%4@h"),
            vec!["a%+fb%4".to_string()]
        );
        // A password that isn't UTF-8 once decoded is still searched as written.
        assert_eq!(
            proxy_passwords("http://u:ab%FFcdefgh@h"),
            vec!["ab%FFcdefgh".to_string()]
        );

        // Printed without the rest of the URL, either spelling is replaced.
        let rec = record(Some("/usr/bin"), &["HTTPS_PROXY"]);
        let lookup = env(&[("HTTPS_PROXY", "http://me:p%40ssw0rd-long@proxy:3128")]);
        let r = known_credentials(&rec, &[], &[], &lookup);
        assert_eq!(
            redacted(&r, "user p%40ssw0rd-long / p@ssw0rd-long"),
            "user [REDACTED:HTTPS_PROXY] / [REDACTED:HTTPS_PROXY]"
        );
    }

    #[test]
    fn koto_s_own_env_keys_are_searched_without_config_keys() {
        // As when the configuration failed to load: no config keys, but the
        // environment still supplies koto's keys to a legacy session.
        let mut rec = record(Some("/usr/bin"), &[]);
        rec.legacy = true;
        let lookup = env(&[
            ("KOTO_DECIDER_API_KEY", "sk-env-decider-value"),
            ("AWS_ACCESS_KEY_ID", "AKIAENVACCESSKEY01"),
            ("AWS_SECRET_ACCESS_KEY", "env-secret-access-value"),
        ]);
        let (built, _) = for_tick(Some(&rec), &[], "wf", None, &[], lookup).unwrap();
        assert_eq!(
            redacted(
                built.redactor(),
                "sk-env-decider-value AKIAENVACCESSKEY01 env-secret-access-value"
            ),
            "[REDACTED:KOTO_DECIDER_API_KEY] [REDACTED:AWS_ACCESS_KEY_ID] \
             [REDACTED:AWS_SECRET_ACCESS_KEY]"
        );
    }
}
