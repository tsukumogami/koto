use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use std::time::Duration;

use anyhow::{Context, Result};
use url::Url;

use super::session_env::{self, Origin};
use super::validate::{check_decider_endpoint, DECIDER_TIMEOUT_DEFAULT_MS, DECIDER_TIMEOUT_MAX_MS};
use super::{DeciderConfig, KotoConfig, RequestStoreConfig};
use crate::decider::{ApiKey, GlobalMode, SettingOrigin};

/// Override values for the `request_store` config block that come from
/// outside the layered config files. Each `None` means "no override at
/// this layer", letting `request_store_config()` evaluate the
/// precedence cascade in one place.
///
/// The fields mirror `RequestStoreConfig` one-to-one so a future
/// operator-tunable dimension can be added by extending both structs
/// together.
#[derive(Debug, Default, Clone)]
pub struct RequestStoreOverrides {
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
/// The `[decider]` table follows its own layer rules (see
/// [`merge_decider`] and [`apply_decider_env`]): project config can only
/// contribute a mode, and its key, endpoint, and timeout are dropped here.
/// Problems in `[decider]` never fail the load; they're collected as
/// warnings that [`resolve_decider`] returns. This function prints
/// nothing.
///
/// Precedence (highest to lowest):
/// 1. Environment variables (`KOTO_SESSION_*` for the session backend,
///    location and credentials, `AWS_ACCESS_KEY_ID` and
///    `AWS_SECRET_ACCESS_KEY` when the koto credential pair is unset,
///    `KOTO_REQUEST_STORE_*` for request-store dimensions)
/// 2. Project config (.koto/config.toml in current directory)
/// 3. User config (~/.koto/config.toml)
/// 4. Built-in defaults
///
/// The session layer records where each session setting came from in
/// `config.session_env`, and never fails the load on a value it can't use:
/// [`session_env::check`] refuses those where a backend is built, so
/// `koto config list` can still show them.
///
/// The CLI-flag layer (the highest tier of the 5-level cascade) is
/// applied per-tick in `request_store_config()`, not here --
/// `load_config()` has no access to per-command argv. Callers that need
/// a `RequestStoreConfig` resolved against a CLI flag should call
/// [`request_store_config`] with the base returned here.
pub fn load_config() -> Result<KotoConfig> {
    let mut config = KotoConfig::default();
    // Apply serde default for backend since Default trait gives empty string.
    config.session.backend = "local".to_string();

    // Layer 1: user config
    if let Some(user_path) = user_config_path() {
        if user_path.exists() {
            let user_config = load_config_file(&user_path, "user config")?;
            record_file_secrets(&mut config.secret_sources, &user_config.config);
            merge_config(&mut config, &user_config);
            mark_file_origins(&mut config, &user_config, Origin::User, true);
            merge_run_journal(&mut config, &user_config);
            merge_marker_retention(&mut config, &user_config);
            merge_check_window(&mut config, &user_config);
            config
                .retention
                .merge_file(&user_config.config.retention, Origin::User);
            merge_decider(
                &mut config.decider,
                &user_config.config.decider,
                ConfigLayer::User,
            );
        }
    }

    // Layer 2: project config
    let project_path = project_config_path();
    if project_path.exists() {
        let project_config = load_config_file(&project_path, "project config")?;
        record_file_secrets(&mut config.secret_sources, &project_config.config);
        merge_config(&mut config, &project_config);
        mark_file_origins(&mut config, &project_config, Origin::Project, false);
        config
            .retention
            .merge_file(&project_config.config.retention, Origin::Project);
        merge_decider(
            &mut config.decider,
            &project_config.config.decider,
            ConfigLayer::Project,
        );
    }

    // Layer 3: the KOTO_SESSION_* location variables, then the
    // credentials: the koto pair as a whole, else the AWS pair as before.
    // The config-file values they replace were recorded above, so
    // redaction still searches for them.
    apply_session_env(&mut config, env_lookup);
    apply_credential_env(&mut config, env_lookup);

    // Layer 3b: KOTO_REQUEST_STORE_* env-var overrides for the
    // request_store block.
    apply_request_store_env_overrides(&mut config.request_store);

    // Layer 3c: KOTO_DECIDER* env overrides for the global decider values.
    apply_decider_env(&mut config.decider, |k| env::var(k).ok());

    // Layer 3d: KOTO_RETENTION_* over the retention settings' file values.
    config.retention.apply_env(|k| env::var(k).ok());

    Ok(config)
}

/// One environment variable as the session layer sees it: `None` when it is
/// unset, `Some(Err(()))` when its value isn't valid Unicode.
type EnvValue = Option<Result<String, ()>>;

/// [`load_config`]'s lookup: the process environment.
fn env_lookup(name: &str) -> EnvValue {
    match env::var(name) {
        Ok(v) => Some(Ok(v)),
        Err(env::VarError::NotUnicode(_)) => Some(Err(())),
        Err(env::VarError::NotPresent) => None,
    }
}

/// A `KOTO_SESSION_*` value as the session layer uses it: trimmed, and
/// `None` when unset, empty or whitespace only. A value that isn't Unicode
/// is set: `Some(Err(()))`.
fn session_var<F>(lookup: &F, name: &str) -> EnvValue
where
    F: Fn(&str) -> EnvValue,
{
    match lookup(name)? {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(Ok(v.trim().to_string())),
        Err(()) => Some(Err(())),
    }
}

/// Mark the session settings a config file names with that file's origin.
///
/// A file names a setting only when its raw TOML has the key, whatever its
/// value. A file with a `[session]` table but no `backend` key still sets
/// the backend to `local` through serde's default (and still replaces an
/// earlier layer's value, as it always has); that value is koto's default,
/// so the origin reads `default`. `marker_retain_days` and
/// `check_window_secs` merge from the user file only, so only that layer
/// can be their origin.
fn mark_file_origins(
    config: &mut KotoConfig,
    loaded: &LoadedConfig,
    origin: Origin,
    user_layer: bool,
) {
    for setting in session_env::SETTINGS {
        if USER_ONLY_SESSION_SETTINGS.contains(&setting) && !user_layer {
            continue;
        }
        let named = loaded.session_keys.iter().any(|k| k == setting);
        // An empty backend is the one value a file can name that the merge
        // ignores; the layer below keeps the setting, and its origin.
        let ignored = setting == "session.backend" && loaded.config.session.backend.is_empty();
        if named && !ignored {
            config.session_env.set_origin(setting, origin);
        } else if setting == "session.backend" && !loaded.config.session.backend.is_empty() {
            config.session_env.set_origin(setting, Origin::Default);
        }
    }
}

/// Apply the five `KOTO_SESSION_*` location variables through `lookup`.
///
/// A set value replaces the file layers' value for its setting and marks
/// its origin. A backend is kept as given, valid or not: `session_env::check`
/// refuses one other than `local` or `cloud`. A path style other than `true`
/// or `false`, and any value that isn't Unicode, is recorded in
/// `session_env.invalid` and leaves its setting with no value, so neither a
/// lower layer nor a default stands in for it.
pub fn apply_session_env<F>(config: &mut KotoConfig, lookup: F)
where
    F: Fn(&str) -> EnvValue,
{
    use session_env::{
        InvalidEnv, ENV_BACKEND, ENV_BUCKET, ENV_CHECK_WINDOW, ENV_ENDPOINT, ENV_PATH_STYLE,
        ENV_REGION,
    };
    for (setting, var) in [
        ("session.backend", ENV_BACKEND),
        ("session.cloud.endpoint", ENV_ENDPOINT),
        ("session.cloud.bucket", ENV_BUCKET),
        ("session.cloud.region", ENV_REGION),
        ("session.cloud.path_style", ENV_PATH_STYLE),
        ("session.cloud.check_window_secs", ENV_CHECK_WINDOW),
    ] {
        let Some(value) = session_var(&lookup, var) else {
            continue;
        };
        config.session_env.set_origin(setting, Origin::Env(var));
        let invalid = |raw: Option<String>| InvalidEnv {
            setting,
            variable: var,
            raw,
        };
        let cloud = &mut config.session.cloud;
        match (setting, value) {
            ("session.cloud.path_style", Ok(v)) if v == "true" || v == "false" => {
                cloud.path_style = Some(v == "true");
            }
            ("session.cloud.path_style", Ok(v)) => {
                cloud.path_style = None;
                config.session_env.invalid.push(invalid(Some(v)));
            }
            ("session.cloud.check_window_secs", Ok(v)) => match v.parse::<u64>() {
                Ok(secs) => cloud.check_window_secs = Some(secs),
                Err(_) => {
                    cloud.check_window_secs = None;
                    config.session_env.invalid.push(invalid(Some(v)));
                }
            },
            (_, Err(())) => {
                match setting {
                    "session.backend" => config.session.backend = String::new(),
                    "session.cloud.endpoint" => cloud.endpoint = None,
                    "session.cloud.bucket" => cloud.bucket = None,
                    "session.cloud.region" => cloud.region = None,
                    "session.cloud.check_window_secs" => cloud.check_window_secs = None,
                    _ => cloud.path_style = None,
                }
                config.session_env.invalid.push(invalid(None));
            }
            ("session.backend", Ok(v)) => config.session.backend = v,
            ("session.cloud.endpoint", Ok(v)) => cloud.endpoint = Some(v),
            ("session.cloud.bucket", Ok(v)) => cloud.bucket = Some(v),
            (_, Ok(v)) => cloud.region = Some(v),
        }
    }
}

/// Resolve the cloud credentials from the environment through `lookup`.
///
/// When either `KOTO_SESSION_CLOUD_ACCESS_KEY` or
/// `KOTO_SESSION_CLOUD_SECRET_KEY` is set (non-empty after trimming), both
/// credentials come from that pair and nothing else: a missing half becomes
/// no value, named in `session_env.pair_missing`, and is never filled from
/// the AWS pair or a file. With neither set, `AWS_ACCESS_KEY_ID` and
/// `AWS_SECRET_ACCESS_KEY` replace the file values exactly as they always
/// have, an empty value included.
pub fn apply_credential_env<F>(config: &mut KotoConfig, lookup: F)
where
    F: Fn(&str) -> EnvValue,
{
    use session_env::{InvalidEnv, ENV_ACCESS_KEY, ENV_SECRET_KEY};
    let access = session_var(&lookup, ENV_ACCESS_KEY);
    let secret = session_var(&lookup, ENV_SECRET_KEY);
    if access.is_some() || secret.is_some() {
        for (setting, var, value) in [
            ("session.cloud.access_key", ENV_ACCESS_KEY, access),
            ("session.cloud.secret_key", ENV_SECRET_KEY, secret),
        ] {
            let resolved = match value {
                Some(Ok(v)) => {
                    config.session_env.set_origin(setting, Origin::Env(var));
                    Some(v)
                }
                Some(Err(())) => {
                    config.session_env.set_origin(setting, Origin::Env(var));
                    config.session_env.invalid.push(InvalidEnv {
                        setting,
                        variable: var,
                        raw: None,
                    });
                    None
                }
                None => {
                    config.session_env.set_origin(setting, Origin::Unset);
                    config.session_env.pair_missing = Some(var);
                    None
                }
            };
            let sources = &mut config.secret_sources;
            if setting == "session.cloud.access_key" {
                config.session.cloud.access_key = resolved;
                sources.access_key_env = Some(var);
            } else {
                config.session.cloud.secret_key = resolved;
                sources.secret_key_env = Some(var);
            }
        }
        return;
    }
    if let Some(Ok(val)) = lookup(ENV_AWS_ACCESS_KEY_ID) {
        config.session.cloud.access_key = Some(val);
        config.secret_sources.access_key_env = Some(ENV_AWS_ACCESS_KEY_ID);
        config.session_env.set_origin(
            "session.cloud.access_key",
            Origin::Env(ENV_AWS_ACCESS_KEY_ID),
        );
    }
    if let Some(Ok(val)) = lookup(ENV_AWS_SECRET_ACCESS_KEY) {
        config.session.cloud.secret_key = Some(val);
        config.secret_sources.secret_key_env = Some(ENV_AWS_SECRET_ACCESS_KEY);
        config.session_env.set_origin(
            "session.cloud.secret_key",
            Origin::Env(ENV_AWS_SECRET_ACCESS_KEY),
        );
    }
}

/// Record every secret value one config file sets, before the merge can
/// replace or drop it: a later layer or an environment variable may
/// override it, and the merge ignores a project file's `decider.api_key`.
/// koto doesn't use those values, but a command could still print them, so
/// redaction searches for them under their setting names.
fn record_file_secrets(sources: &mut super::SecretSources, file: &KotoConfig) {
    for (setting, value) in [
        ("session.cloud.access_key", &file.session.cloud.access_key),
        ("session.cloud.secret_key", &file.session.cloud.secret_key),
        ("decider.api_key", &file.decider.api_key),
    ] {
        if let Some(v) = value {
            if !sources
                .file_values
                .iter()
                .any(|(s, x)| *s == setting && x == v)
            {
                sources.file_values.push((setting, v.clone()));
            }
        }
    }
}

/// Resolve `RequestStoreConfig` through the full 5-level precedence
/// cascade:
///
///   CLI flag > env-var > project config > user config > built-in default
///
/// `base` is the `RequestStoreConfig` already produced by
/// [`load_config`] (which has merged the file layers and applied
/// `KOTO_REQUEST_STORE_*` env-var overrides). `cli` carries the
/// per-tick CLI-flag overrides; on a `koto next` invocation only
/// `redelegation_cap` is settable today.
pub fn request_store_config(
    base: &RequestStoreConfig,
    cli: &RequestStoreOverrides,
) -> RequestStoreConfig {
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

/// Apply `KOTO_REQUEST_STORE_*` env-var overrides to a
/// `RequestStoreConfig` in place.
///
/// Env-var key spellings come from DESIGN-koto-request-store Decision 4.
/// Unset vars leave the field untouched. Malformed integer values are
/// silently ignored (matches the existing `AWS_*` env-var behavior).
fn apply_request_store_env_overrides(rs: &mut RequestStoreConfig) {
    if let Some(v) = env_parse::<u64>("KOTO_REQUEST_STORE_STALE_CLAIM_TIMEOUT_S") {
        rs.stale_claim_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_REQUEST_STORE_STALE_DISPATCH_TIMEOUT_S") {
        rs.stale_dispatch_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_REDELEGATION_CAP") {
        rs.redelegation_cap = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_COORD_CURSOR_TTL_DAYS") {
        rs.coord_cursor_ttl_days = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_REQUEST_STORE_TERMINAL_INDEX_COMPACT_LINES") {
        rs.terminal_index_compact_lines = v;
    }
    if let Some(v) = env_parse::<u64>("KOTO_REQUEST_STORE_COMPACT_LOCK_TIMEOUT_S") {
        rs.compact_lock_timeout_seconds = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_DIRECTIVE_BATCH_SIZE") {
        rs.directive_batch_size = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_RESPAWN_GENERATION_CAP") {
        rs.respawn_generation_cap = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_REQUEST_LEG_APPEND_CAP") {
        rs.request_leg_append_cap = v;
    }
    if let Some(v) = env_parse::<u32>("KOTO_REQUEST_STORE_REQUEST_LEG_CAP") {
        rs.request_leg_cap = v;
    }
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    env::var(key).ok().and_then(|v| v.parse::<T>().ok())
}

/// Load a TOML config file and deserialize it. The request_store block
/// is parsed separately from `KotoConfig` so we can distinguish "field
/// present in the source file" from "field defaulted by serde" -- the
/// merge step only overlays explicitly-set fields onto the target.
///
/// A parse failure is reported as a [`ConfigParseError`], which carries
/// only the path and position: the toml error's message and source
/// snippet can quote a line holding a key, so neither is kept.
fn load_config_file(path: &Path, label: &'static str) -> Result<LoadedConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("loading {} from {}", label, path.display()))?;
    let raw: toml::Value = content
        .parse()
        .map_err(|e| ConfigParseError::new(label, path, &content, &e, ParseProblem::Syntax))?;
    let config: KotoConfig = toml::from_str(&content)
        .map_err(|e| ConfigParseError::new(label, path, &content, &e, ParseProblem::Shape))?;
    let request_store_keys = raw
        .as_table()
        .and_then(|t| t.get("request_store"))
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter(|(_, v)| !v.is_table())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let request_store_has_recursion = raw
        .as_table()
        .and_then(|t| t.get("request_store"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("recursion"))
        .is_some();
    let run_journal_keys = raw
        .as_table()
        .and_then(|t| t.get("run_journal"))
        .and_then(|v| v.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    let workflows_native_present = raw
        .as_table()
        .and_then(|t| t.get("workflows"))
        .and_then(|v| v.as_table())
        .map(|t| t.contains_key("native"))
        .unwrap_or(false);
    let session_keys = session_keys_named(&raw);
    Ok(LoadedConfig {
        config,
        request_store_keys,
        request_store_has_recursion,
        run_journal_keys,
        workflows_native_present,
        session_keys,
    })
}

/// The session settings a file's raw TOML names, as dotted names. Both TOML
/// spellings (`[session]` with `backend = ...`, or a top-level
/// `session.backend = ...`) parse to the same table.
fn session_keys_named(raw: &toml::Value) -> Vec<String> {
    let session = raw
        .as_table()
        .and_then(|t| t.get("session"))
        .and_then(|v| v.as_table());
    let Some(session) = session else {
        return Vec::new();
    };
    let cloud = session.get("cloud").and_then(|v| v.as_table());
    session_env::SETTINGS
        .iter()
        .filter(|setting| match setting.strip_prefix("session.cloud.") {
            Some(field) => cloud.is_some_and(|c| c.contains_key(field)),
            None => session.contains_key(setting.trim_start_matches("session.")),
        })
        .map(|s| s.to_string())
        .collect()
}

/// What kind of parse failure a [`ConfigParseError`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseProblem {
    /// The file isn't valid TOML.
    Syntax,
    /// The file is valid TOML but a value has the wrong type or shape.
    Shape,
}

/// A config file that failed to parse.
///
/// Holds only fixed text, the file's label and path, and the 1-based line
/// and column of the problem. The underlying toml error is dropped on
/// purpose: its message and source snippet quote the offending line, and a
/// user config line can hold `decider.api_key`. Nothing built from this
/// error (`Display`, `Debug`, or an anyhow chain) can carry file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigParseError {
    label: &'static str,
    path: PathBuf,
    position: Option<(usize, usize)>,
    problem: ParseProblem,
}

impl ConfigParseError {
    /// Build the error from a toml error, keeping only its position.
    pub fn new(
        label: &'static str,
        path: &Path,
        content: &str,
        err: &toml::de::Error,
        problem: ParseProblem,
    ) -> Self {
        ConfigParseError {
            label,
            path: path.to_path_buf(),
            position: err.span().map(|span| line_column(content, span.start)),
            problem,
        }
    }

    /// 1-based line and column of the problem, when the parser gave one.
    pub fn position(&self) -> Option<(usize, usize)> {
        self.position
    }
}

impl std::fmt::Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.problem {
            ParseProblem::Syntax => "is not valid TOML",
            ParseProblem::Shape => "has a value of the wrong type or shape",
        };
        write!(f, "{} {} {}", self.label, self.path.display(), what)?;
        if let Some((line, column)) = self.position {
            write!(f, " at line {}, column {}", line, column)?;
        }
        write!(
            f,
            " (the file's content isn't shown because it can hold credentials)"
        )
    }
}

impl std::error::Error for ConfigParseError {}

/// 1-based line and column (in characters) of byte offset `offset`.
fn line_column(content: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(content.len());
    while !content.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &content[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

/// A loaded config plus metadata about which request_store fields the
/// source file actually set. Drives the layered merge step.
struct LoadedConfig {
    config: KotoConfig,
    request_store_keys: Vec<String>,
    request_store_has_recursion: bool,
    run_journal_keys: Vec<String>,
    workflows_native_present: bool,
    /// The session settings (dotted names from `session_env::SETTINGS`)
    /// the file's raw TOML names, whatever their values.
    session_keys: Vec<String>,
}

/// Merge source config into target. Non-default/non-empty values in
/// source overwrite target. For `request_store`, only fields that were
/// explicitly present in the source file are overlaid (serde defaults
/// are not "values").
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
    if source.config.session.cloud.path_style.is_some() {
        target.session.cloud.path_style = source.config.session.cloud.path_style;
    }

    for key in &source.request_store_keys {
        match key.as_str() {
            "stale_claim_timeout_seconds" => {
                target.request_store.stale_claim_timeout_seconds =
                    source.config.request_store.stale_claim_timeout_seconds;
            }
            "stale_dispatch_timeout_seconds" => {
                target.request_store.stale_dispatch_timeout_seconds =
                    source.config.request_store.stale_dispatch_timeout_seconds;
            }
            "redelegation_cap" => {
                target.request_store.redelegation_cap =
                    source.config.request_store.redelegation_cap;
            }
            "coord_cursor_ttl_days" => {
                target.request_store.coord_cursor_ttl_days =
                    source.config.request_store.coord_cursor_ttl_days;
            }
            "terminal_index_compact_lines" => {
                target.request_store.terminal_index_compact_lines =
                    source.config.request_store.terminal_index_compact_lines;
            }
            "compact_lock_timeout_seconds" => {
                target.request_store.compact_lock_timeout_seconds =
                    source.config.request_store.compact_lock_timeout_seconds;
            }
            "directive_batch_size" => {
                target.request_store.directive_batch_size =
                    source.config.request_store.directive_batch_size;
            }
            "respawn_generation_cap" => {
                target.request_store.respawn_generation_cap =
                    source.config.request_store.respawn_generation_cap;
            }
            "request_leg_append_cap" => {
                target.request_store.request_leg_append_cap =
                    source.config.request_store.request_leg_append_cap;
            }
            "request_leg_cap" => {
                target.request_store.request_leg_cap = source.config.request_store.request_leg_cap;
            }
            _ => {}
        }
    }
    if source.request_store_has_recursion {
        target.request_store.recursion = source.config.request_store.recursion.clone();
    }
    if source.workflows_native_present {
        target.workflows.native = source.config.workflows.native;
    }
}

/// Apply `session.cloud.marker_retain_days` from a config file. Called for
/// the user layer only: the bound guards a deletion from the bucket, so a
/// project file in a cloned repository must not shorten it (`koto config
/// set` refuses the key without `--user`).
fn merge_marker_retention(target: &mut KotoConfig, source: &LoadedConfig) {
    if source.config.session.cloud.marker_retain_days.is_some() {
        target.session.cloud.marker_retain_days = source.config.session.cloud.marker_retain_days;
    }
}

/// The session settings only the user file (or, for some, the environment)
/// may set: a project file in a cloned repository must not change them.
const USER_ONLY_SESSION_SETTINGS: [&str; 2] = [
    "session.cloud.marker_retain_days",
    "session.cloud.check_window_secs",
];

/// Apply `session.cloud.check_window_secs` from a config file. Called for
/// the user layer only: the window is how long a read may miss another
/// host's writes, so a project file in a cloned repository must not widen it
/// (`koto config set` refuses the key without `--user`).
fn merge_check_window(target: &mut KotoConfig, source: &LoadedConfig) {
    if source.config.session.cloud.check_window_secs.is_some() {
        target.session.cloud.check_window_secs = source.config.session.cloud.check_window_secs;
    }
}

/// Apply the `[run_journal]` keys a config file sets. Called for the user
/// layer only: the journal lives beside the user's session store, so a
/// project file in a cloned repository must not turn rotation on or shorten
/// retention for it (`koto config set` refuses these keys without `--user`).
fn merge_run_journal(target: &mut KotoConfig, source: &LoadedConfig) {
    for key in &source.run_journal_keys {
        match key.as_str() {
            "rotate_bytes" => {
                target.run_journal.rotate_bytes = source.config.run_journal.rotate_bytes;
            }
            "retain_days" => {
                target.run_journal.retain_days = source.config.run_journal.retain_days;
            }
            "reader_stale_days" => {
                target.run_journal.reader_stale_days = source.config.run_journal.reader_stale_days;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Decider configuration
// ---------------------------------------------------------------------------

/// The decision endpoint used when neither `KOTO_DECIDER_ENDPOINT` nor
/// user `decider.endpoint` is set. This is the full URL of the decision
/// call (a `POST`), not a base URL. `KOTO_DECIDER_ENDPOINT` or user
/// `decider.endpoint` overrides it.
///
/// Jev's decision endpoint, per <https://docs.typesafe.ai/introduction/quickstart>
/// and <https://docs.typesafe.ai/api.md>.
pub const DEFAULT_DECIDER_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// Env var that sets the global decider mode.
pub const ENV_DECIDER_MODE: &str = "KOTO_DECIDER";
/// Env var that supplies the decider API key.
pub const ENV_DECIDER_API_KEY: &str = "KOTO_DECIDER_API_KEY";
/// Env vars that supply koto's own secrets: the decider key and the cloud
/// access and secret keys. Redaction reads them from the environment
/// directly as well as through the loaded configuration, so they're
/// searched for even when a config file fails to load.
///
/// The koto credential pair comes before the AWS pair: a value both hold is
/// labeled with the koto name, the one koto used.
pub const ENV_SECRET_NAMES: [&str; 5] = [
    ENV_DECIDER_API_KEY,
    session_env::ENV_ACCESS_KEY,
    session_env::ENV_SECRET_KEY,
    ENV_AWS_ACCESS_KEY_ID,
    ENV_AWS_SECRET_ACCESS_KEY,
];
/// Env var that overrides `session.cloud.access_key`. Also a redaction
/// marker's source name, so it's visible in captured output.
pub const ENV_AWS_ACCESS_KEY_ID: &str = "AWS_ACCESS_KEY_ID";
/// Env var that overrides `session.cloud.secret_key`. Also a redaction
/// marker's source name, so it's visible in captured output.
pub const ENV_AWS_SECRET_ACCESS_KEY: &str = "AWS_SECRET_ACCESS_KEY";
/// Env var that sets the decider endpoint.
pub const ENV_DECIDER_ENDPOINT: &str = "KOTO_DECIDER_ENDPOINT";

/// Which config file a `[decider]` table was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLayer {
    /// `~/.koto/config.toml`.
    User,
    /// `.koto/config.toml` in the working directory.
    Project,
}

/// Merge one file's `[decider]` table into the accumulated config.
///
/// From user config every field is taken and tagged with the `User`
/// origin. From project config only `mode` is taken, into
/// `project_mode`; `api_key`, `endpoint`, and `timeout_ms` are dropped at
/// load time, so a checked-in `.koto/config.toml` can never supply a key,
/// point a user's key at another server, or stretch the timeout. Dropped
/// values produce a warning that names the key but not its value.
pub fn merge_decider(target: &mut DeciderConfig, source: &DeciderConfig, layer: ConfigLayer) {
    let label = match layer {
        ConfigLayer::User => "user config",
        ConfigLayer::Project => "project config",
    };
    for w in &source.warnings {
        target.warnings.push(format!("{}: {}", label, w));
    }
    match layer {
        ConfigLayer::User => {
            if let Some(v) = &source.mode {
                target.mode = Some(v.clone());
                target.mode_origin = SettingOrigin::User;
            }
            if let Some(v) = &source.api_key {
                target.api_key = Some(v.clone());
                target.api_key_origin = SettingOrigin::User;
            }
            if let Some(v) = &source.endpoint {
                target.endpoint = Some(v.clone());
                target.endpoint_origin = SettingOrigin::User;
            }
            if let Some(v) = source.timeout_ms {
                target.timeout_ms = Some(v);
            }
        }
        ConfigLayer::Project => {
            if let Some(v) = &source.mode {
                target.project_mode = Some(v.clone());
            }
            if source.api_key.is_some() {
                target.warnings.push(format!(
                    "{}: decider.api_key is ignored in project config (set it in user config or {})",
                    label, ENV_DECIDER_API_KEY
                ));
            }
            if source.endpoint.is_some() {
                target.warnings.push(format!(
                    "{}: decider.endpoint is ignored in project config (set it in user config or {})",
                    label, ENV_DECIDER_ENDPOINT
                ));
            }
            if source.timeout_ms.is_some() {
                target.warnings.push(format!(
                    "{}: decider.timeout_ms is ignored in project config (set it in user config)",
                    label
                ));
            }
        }
    }
}

/// Apply the `KOTO_DECIDER*` env overrides through `lookup`.
///
/// A non-empty `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, or
/// `KOTO_DECIDER_ENDPOINT` replaces the global mode, key, or endpoint from
/// user config and tags it with the `Env` origin. An empty (or
/// whitespace-only) value counts as unset. [`load_config`] passes
/// `std::env::var`; tests pass an explicit map instead of mutating the
/// process environment.
pub fn apply_decider_env<F>(cfg: &mut DeciderConfig, lookup: F)
where
    F: Fn(&str) -> Option<String>,
{
    let non_empty = |k: &str| lookup(k).filter(|v| !v.trim().is_empty());
    if let Some(v) = non_empty(ENV_DECIDER_MODE) {
        cfg.mode = Some(v);
        cfg.mode_origin = SettingOrigin::Env;
    }
    if let Some(v) = non_empty(ENV_DECIDER_API_KEY) {
        cfg.api_key = Some(v);
        cfg.api_key_origin = SettingOrigin::Env;
    }
    if let Some(v) = non_empty(ENV_DECIDER_ENDPOINT) {
        cfg.endpoint = Some(v);
        cfg.endpoint_origin = SettingOrigin::Env;
    }
}

/// Resolved decider settings: the one place opt-in is decided.
///
/// Built only by [`resolve_decider`]. Fields are private so a caller
/// can't assemble settings that skip the endpoint or same-layer rules.
#[derive(Debug, Clone)]
pub struct DeciderSettings {
    mode: GlobalMode,
    user_mode: GlobalMode,
    project_mode: Option<GlobalMode>,
    api_key: Option<ApiKey>,
    api_key_origin: SettingOrigin,
    endpoint: Option<Url>,
    endpoint_origin: SettingOrigin,
    timeout: Duration,
}

impl DeciderSettings {
    /// Whether the user is opted in to decider consultations.
    ///
    /// True only when the effective global mode (after the project
    /// minimum) is `shadow` or `auto`, a key is present, the endpoint
    /// parsed and passed the scheme, loopback, and userinfo rules, and the
    /// endpoint comes from the key's own layer or is the built-in default.
    /// Reads only fields on `self`: no env or file access.
    pub fn opted_in(&self) -> bool {
        if self.mode == GlobalMode::Off {
            return false;
        }
        if self.api_key.is_none() {
            return false;
        }
        let endpoint_ok = match &self.endpoint {
            Some(url) => check_decider_endpoint(url.as_str()).is_ok(),
            None => false,
        };
        endpoint_ok && same_layer(self.api_key_origin, self.endpoint_origin)
    }

    /// Effective global mode: the minimum of the user-level and project
    /// modes.
    pub fn mode(&self) -> GlobalMode {
        self.mode
    }

    /// The user-level mode (`KOTO_DECIDER` or user `decider.mode`), before
    /// the project minimum. An unrecognized value is `off`.
    pub fn user_mode(&self) -> GlobalMode {
        self.user_mode
    }

    /// The project mode, or `None` when project config sets none. An
    /// unrecognized value is `Some(Off)`.
    pub fn project_mode(&self) -> Option<GlobalMode> {
        self.project_mode
    }

    /// The API key, if one is configured.
    pub fn api_key(&self) -> Option<&ApiKey> {
        self.api_key.as_ref()
    }

    /// Layer the key came from (`User` or `Env`; `Default` when absent).
    pub fn api_key_origin(&self) -> SettingOrigin {
        self.api_key_origin
    }

    /// The decision endpoint, or `None` when the configured value was
    /// refused (unparseable, bad scheme, or userinfo).
    pub fn endpoint(&self) -> Option<&Url> {
        self.endpoint.as_ref()
    }

    /// Layer the endpoint came from: `default`, `user`, or `env`.
    pub fn endpoint_origin(&self) -> SettingOrigin {
        self.endpoint_origin
    }

    /// Per-consultation timeout, bounded to 1..=10000 ms.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// The same-layer rule: a key is sent only to the built-in default
/// endpoint or to an endpoint from the key's own layer.
fn same_layer(key: SettingOrigin, endpoint: SettingOrigin) -> bool {
    matches!(
        (key, endpoint),
        (_, SettingOrigin::Default)
            | (SettingOrigin::User, SettingOrigin::User)
            | (SettingOrigin::Env, SettingOrigin::Env)
    )
}

/// A value present with no recorded origin came from a config file.
fn file_origin_if_unset(origin: SettingOrigin) -> SettingOrigin {
    match origin {
        SettingOrigin::Default => SettingOrigin::User,
        o => o,
    }
}

fn key_source_label(origin: SettingOrigin) -> &'static str {
    match origin {
        SettingOrigin::Env => ENV_DECIDER_API_KEY,
        _ => "user config",
    }
}

fn endpoint_source_label(origin: SettingOrigin) -> &'static str {
    match origin {
        SettingOrigin::Env => ENV_DECIDER_ENDPOINT,
        SettingOrigin::User => "user config",
        SettingOrigin::Default => "the default",
    }
}

fn mode_warning(source: &str, raw: &str) -> String {
    if raw.trim().eq_ignore_ascii_case("never") {
        format!(
            "{}: decider mode 'never' is template-only; the decider is off",
            source
        )
    } else {
        format!(
            "{}: decider mode is not one of off, shadow, auto; the decider is off",
            source
        )
    }
}

/// Resolve the effective decider settings from a loaded `DeciderConfig`.
///
/// Pure: no env reads, no file reads, no printing. Returns the settings
/// and every warning (load-time ones included) for the caller to print.
/// No warning contains the key or endpoint userinfo.
///
/// - The global mode is `KOTO_DECIDER` or user `decider.mode`, default
///   `off`; an unrecognized value is `off` with a warning.
/// - The effective mode is the minimum of the global and project modes;
///   an unrecognized project mode is `off` with a warning.
/// - The timeout defaults to 2000 ms, is capped at 10000 ms, and falls
///   back to 2000 ms on zero or a negative value, each with a warning.
/// - Endpoint and same-layer problems warn only when they're what stands
///   between the user and opt-in (mode not `off` and a key present).
pub fn resolve_decider(cfg: &DeciderConfig) -> (DeciderSettings, Vec<String>) {
    let mut warnings = cfg.warnings.clone();

    // Global mode.
    let global = match &cfg.mode {
        None => GlobalMode::Off,
        Some(raw) => GlobalMode::parse(raw).unwrap_or_else(|| {
            let source = match cfg.mode_origin {
                SettingOrigin::Env => ENV_DECIDER_MODE,
                _ => "user config",
            };
            warnings.push(mode_warning(source, raw));
            GlobalMode::Off
        }),
    };

    // Project minimum: can only lower. The rule itself lives in
    // `engine::decider::effective_mode`.
    let project_mode = cfg.project_mode.as_ref().map(|raw| {
        GlobalMode::parse(raw).unwrap_or_else(|| {
            warnings.push(mode_warning("project config", raw));
            GlobalMode::Off
        })
    });
    let mode = crate::engine::decider::effective_global_mode(global, project_mode);

    // Timeout.
    let timeout_ms: u64 = match cfg.timeout_ms {
        None => DECIDER_TIMEOUT_DEFAULT_MS,
        Some(n) if n <= 0 => {
            warnings.push(format!(
                "user config: decider.timeout_ms must be at least 1; using {}",
                DECIDER_TIMEOUT_DEFAULT_MS
            ));
            DECIDER_TIMEOUT_DEFAULT_MS
        }
        Some(n) if n as u64 > DECIDER_TIMEOUT_MAX_MS => {
            warnings.push(format!(
                "user config: decider.timeout_ms is above the {} ms cap; using {}",
                DECIDER_TIMEOUT_MAX_MS, DECIDER_TIMEOUT_MAX_MS
            ));
            DECIDER_TIMEOUT_MAX_MS
        }
        Some(n) => n as u64,
    };

    // Key. One that can't travel in an HTTP header (a control or
    // non-ASCII character) counts as absent, so it never opts the user in
    // and never reaches the transport. The warning names only the source.
    let api_key = match cfg.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        Some(k) if ApiKey::is_header_safe(k) => Some(ApiKey::new(k)),
        Some(_) => {
            warnings.push(format!(
                "{}: the decider API key contains a control or non-ASCII character \
                 and can't be sent; ignoring it, so the decider is off",
                key_source_label(file_origin_if_unset(cfg.api_key_origin))
            ));
            None
        }
        None => None,
    };
    let api_key_origin = if api_key.is_some() {
        file_origin_if_unset(cfg.api_key_origin)
    } else {
        SettingOrigin::Default
    };

    // Endpoint.
    let (endpoint, endpoint_origin, endpoint_err) = match &cfg.endpoint {
        None => (
            Some(Url::parse(DEFAULT_DECIDER_ENDPOINT).expect("default endpoint parses")),
            SettingOrigin::Default,
            None,
        ),
        Some(raw) => {
            let origin = file_origin_if_unset(cfg.endpoint_origin);
            match check_decider_endpoint(raw) {
                Ok(url) => (Some(url), origin, None),
                Err(e) => (None, origin, Some(e)),
            }
        }
    };

    let wants_consult = mode != GlobalMode::Off && api_key.is_some();
    if wants_consult {
        if let Some(e) = &endpoint_err {
            warnings.push(format!(
                "{}: {}; the decider is off",
                endpoint_source_label(endpoint_origin),
                e
            ));
        } else if !same_layer(api_key_origin, endpoint_origin) {
            warnings.push(format!(
                "the decider API key comes from {} but the endpoint comes from {}; \
                 a key is only sent to an endpoint set in the same place or to the default, \
                 so the decider is off",
                key_source_label(api_key_origin),
                endpoint_source_label(endpoint_origin)
            ));
        }
    }

    (
        DeciderSettings {
            mode,
            user_mode: global,
            project_mode,
            api_key,
            api_key_origin,
            endpoint,
            endpoint_origin,
            timeout: Duration::from_millis(timeout_ms),
        },
        warnings,
    )
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
    let content = fs::read_to_string(path)
        .with_context(|| format!("loading config file {}", path.display()))?;
    let value: toml::Value = content.parse().map_err(|e| {
        ConfigParseError::new("config file", path, &content, &e, ParseProblem::Syntax)
    })?;
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

/// Write the user config file, which can hold credentials.
///
/// On Unix the file is created with mode 0600, and an existing file is
/// tightened to 0600 before its new content is written, so a key never
/// sits in a group- or world-readable file.
pub fn write_user_toml_value(path: &Path, value: &toml::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(value)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        // `mode` above applies only on creation and is subject to the
        // umask; set it explicitly so the result is always 0600.
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// `KOTO_REQUEST_STORE_*` env keys recognized by
    /// `apply_request_store_env_overrides`. Listed here so tests can
    /// clear them between runs (env vars are process-global; cargo's
    /// parallel test runner can leak between unrelated tests).
    const REQUEST_STORE_ENV_KEYS: &[&str] = &[
        "KOTO_REQUEST_STORE_STALE_CLAIM_TIMEOUT_S",
        "KOTO_REQUEST_STORE_STALE_DISPATCH_TIMEOUT_S",
        "KOTO_REQUEST_STORE_REDELEGATION_CAP",
        "KOTO_REQUEST_STORE_COORD_CURSOR_TTL_DAYS",
        "KOTO_REQUEST_STORE_TERMINAL_INDEX_COMPACT_LINES",
        "KOTO_REQUEST_STORE_COMPACT_LOCK_TIMEOUT_S",
        "KOTO_REQUEST_STORE_DIRECTIVE_BATCH_SIZE",
        "KOTO_REQUEST_STORE_RESPAWN_GENERATION_CAP",
    ];

    fn clear_request_store_env() {
        for k in REQUEST_STORE_ENV_KEYS {
            env::remove_var(k);
        }
    }

    #[test]
    fn test_load_config_defaults() {
        // With no config files, we get defaults.
        // Run in a temp dir and override HOME to avoid picking up real user/project config.
        let tmp = TempDir::new().unwrap();
        let _lock = process_env_lock();
        let _guard = SetCwd::new(tmp.path());
        let _home_guard = SetEnv::new("HOME", tmp.path().to_str().unwrap());

        // Clear env vars that would interfere.
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        clear_request_store_env();

        let config = load_config().unwrap();
        assert_eq!(config.session.backend, "local");
        assert!(config.session.cloud.endpoint.is_none());
        assert!(config.session.cloud.access_key.is_none());
        // RequestStoreConfig defaults match Decision 4's table.
        assert_eq!(config.request_store.redelegation_cap, 3);
        assert_eq!(config.request_store.stale_claim_timeout_seconds, 600);
        assert_eq!(config.request_store.terminal_index_compact_lines, 100_000);
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
                request_store: RequestStoreConfig::default(),
                workflows: Default::default(),
                run_journal: Default::default(),
                retention: Default::default(),
                decider: Default::default(),
                secret_sources: Default::default(),
                session_env: Default::default(),
            },
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: false,
            session_keys: vec![],
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
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: false,
            session_keys: vec![],
        };
        merge_config(&mut base, &overlay);

        // backend stays "cloud" because overlay backend is empty string (Default)
        // but serde deserialization would give "local" — here we're using Default directly.
        // The merge only overwrites if source backend is non-empty.
        assert_eq!(base.session.backend, "cloud");
        assert_eq!(base.session.cloud.bucket, Some("existing".to_string()));
    }

    #[test]
    fn test_merge_path_style_overrides_only_when_set() {
        let mut base = KotoConfig::default();
        base.session.cloud.path_style = Some(true);

        // A layer that doesn't set the key leaves the lower layer's value.
        let unset = LoadedConfig {
            config: KotoConfig::default(),
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: false,
            session_keys: vec![],
        };
        merge_config(&mut base, &unset);
        assert_eq!(base.session.cloud.path_style, Some(true));

        // A layer that sets it, even to false, wins.
        let mut off = KotoConfig::default();
        off.session.cloud.path_style = Some(false);
        let overlay = LoadedConfig {
            config: off,
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: false,
            session_keys: vec![],
        };
        merge_config(&mut base, &overlay);
        assert_eq!(base.session.cloud.path_style, Some(false));
    }

    #[test]
    fn test_path_style_parsed_from_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[session.cloud]\npath_style = true\n").unwrap();
        let loaded = load_config_file(&path, "project config").unwrap();
        assert_eq!(loaded.config.session.cloud.path_style, Some(true));
    }

    #[test]
    fn test_workflows_native_parsed_and_tracked_from_file() {
        // Race-free: load_config_file reads a specific path, touching no
        // process-global cwd/env.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[workflows]\nnative = true\n").unwrap();

        let loaded = load_config_file(&path, "user config").unwrap();
        assert!(loaded.config.workflows.native);
        assert!(loaded.workflows_native_present);
    }

    #[test]
    fn test_run_journal_keys_apply_from_the_user_layer_only() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.path().join("user.toml");
        fs::write(
            &user,
            "[run_journal]\nrotate_bytes = 4096\nretain_days = 3\n",
        )
        .unwrap();
        let project = tmp.path().join("project.toml");
        fs::write(&project, "[run_journal]\nretain_days = 5\n").unwrap();

        let mut config = KotoConfig::default();
        let user_layer = load_config_file(&user, "user config").unwrap();
        merge_config(&mut config, &user_layer);
        merge_run_journal(&mut config, &user_layer);
        // The project layer goes through merge_config only, which leaves
        // [run_journal] alone.
        merge_config(
            &mut config,
            &load_config_file(&project, "project config").unwrap(),
        );
        assert_eq!(config.run_journal.rotate_bytes, 4096);
        assert_eq!(config.run_journal.retain_days, 3);
        assert_eq!(config.run_journal.reader_stale_days, 90);
    }

    #[test]
    fn test_workflows_native_absent_not_tracked() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[session]\nbackend = \"local\"\n").unwrap();

        let loaded = load_config_file(&path, "user config").unwrap();
        // Absent from the file: not tracked for merge, and left at the default (on).
        assert!(!loaded.workflows_native_present);
        assert!(loaded.config.workflows.native);
    }

    #[test]
    fn test_workflows_native_explicit_optout_tracked() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "[workflows]\nnative = false\n").unwrap();

        let loaded = load_config_file(&path, "user config").unwrap();
        assert!(loaded.workflows_native_present);
        assert!(!loaded.config.workflows.native);
    }

    #[test]
    fn test_merge_applies_workflows_native_only_when_present() {
        // An explicit opt-out (present, native=false) overrides the default-on base.
        let mut base = KotoConfig::default();
        assert!(base.workflows.native, "default is on");
        let mut off = KotoConfig::default();
        off.workflows.native = false;
        let optout = LoadedConfig {
            config: off,
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: true,
            session_keys: vec![],
        };
        merge_config(&mut base, &optout);
        assert!(
            !base.workflows.native,
            "explicit opt-out overrides the default"
        );

        // An absent key (not present in the layer) leaves the base untouched.
        let mut base_off = KotoConfig::default();
        base_off.workflows.native = false;
        let absent = LoadedConfig {
            config: KotoConfig::default(),
            request_store_keys: vec![],
            request_store_has_recursion: false,
            run_journal_keys: vec![],
            workflows_native_present: false,
            session_keys: vec![],
        };
        merge_config(&mut base_off, &absent);
        assert!(!base_off.workflows.native, "absent key must not clobber");
    }

    #[test]
    fn test_env_var_override() {
        let tmp = TempDir::new().unwrap();
        let _lock = process_env_lock();
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
        let _lock = process_env_lock();
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
    fn secrets_from_every_config_layer_are_redaction_keys() {
        let tmp = TempDir::new().unwrap();
        let _lock = process_env_lock();
        let _guard = SetCwd::new(tmp.path());
        // The user layer gets its own HOME, apart from the project directory.
        let user_home = tmp.path().join("user-home");
        fs::create_dir_all(user_home.join(".koto")).unwrap();
        let _home_guard = SetEnv::new("HOME", user_home.to_str().unwrap());
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::remove_var(ENV_DECIDER_API_KEY);

        // The user file's cloud key is replaced by the project file's, and
        // the project file's decider key is ignored by the merge.
        let koto_dir = tmp.path().join(".koto");
        fs::create_dir_all(&koto_dir).unwrap();
        fs::write(
            user_home.join(".koto").join("config.toml"),
            "[session.cloud]\naccess_key = \"user-file-access-key\"\n\
             secret_key = \"user-file-secret-key\"\n",
        )
        .unwrap();
        fs::write(
            koto_dir.join("config.toml"),
            "[session.cloud]\naccess_key = \"project-file-access-key\"\n\n\
             [decider]\napi_key = \"project-file-decider-key\"\n",
        )
        .unwrap();

        let config = load_config().unwrap();
        assert_eq!(
            config.session.cloud.access_key.as_deref(),
            Some("project-file-access-key")
        );
        assert!(config.decider.api_key.is_none());
        let keys = crate::config::redaction_keys(&config);
        for (source, value) in [
            ("session.cloud.access_key", "project-file-access-key"),
            ("session.cloud.access_key", "user-file-access-key"),
            ("session.cloud.secret_key", "user-file-secret-key"),
            ("decider.api_key", "project-file-decider-key"),
        ] {
            assert!(
                keys.iter().any(|(s, v)| s == source && v == value),
                "{source} missing"
            );
        }
        // The Debug form names settings only.
        assert!(!format!("{:?}", config.secret_sources).contains("file-"));
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

    #[test]
    fn parse_errors_carry_only_path_and_position() {
        const SECRET: &str = "sk-PARSE-SECRET-91";
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let cases = [
            (format!("[decider]\napi_key = \"{SECRET}\n"), (2, 30)),
            (format!("# é\n[session]\ncloud = \"{SECRET}\"\n"), (3, 9)),
        ];
        for (body, pos) in cases {
            std::fs::write(&path, &body).unwrap();
            let errs = [
                load_config_file(&path, "user config").err().unwrap(),
                // load_toml_value only parses TOML; the shape case is valid.
                match load_toml_value(&path) {
                    Err(e) => e,
                    Ok(_) => continue,
                },
            ];
            for e in errs {
                for text in [
                    format!("{}", e),
                    format!("{:#}", e),
                    format!("{:?}", e),
                    format!("{:#?}", e),
                ] {
                    assert!(!text.contains(SECRET), "{}", text);
                    assert!(!text.contains("api_key"), "{}", text);
                    assert!(!text.contains("cloud"), "{}", text);
                }
                let shown = format!("{}", e);
                assert!(shown.contains(&path.display().to_string()), "{}", shown);
                let parse = e.downcast_ref::<ConfigParseError>().unwrap();
                assert_eq!(parse.position(), Some(pos), "{}", shown);
                assert!(e.source().is_none(), "no toml error kept: {}", shown);
            }
        }
    }

    #[test]
    fn line_column_counts_chars_and_clamps() {
        assert_eq!(line_column("", 0), (1, 1));
        assert_eq!(line_column("ab\ncd", 4), (2, 2));
        assert_eq!(line_column("é=x", 2), (1, 2));
        // Inside a multi-byte char and past the end both clamp.
        assert_eq!(line_column("é", 1), (1, 1));
        assert_eq!(line_column("a\n", 99), (2, 1));
    }

    // -----------------------------------------------------------------------
    // Decider: merge, env, resolve, opt-in
    // -----------------------------------------------------------------------

    mod decider {
        use super::super::*;
        use std::collections::HashMap;
        use tempfile::TempDir;

        const KEY: &str = "sk-test-DISTINCTIVE-9f8e7d";

        fn parse(body: &str) -> DeciderConfig {
            let cfg: KotoConfig = toml::from_str(body).expect("file parses");
            cfg.decider
        }

        /// Build a `DeciderConfig` the way `load_config` does, from explicit
        /// user and project file bodies and an explicit env map.
        fn build(user: &str, project: &str, env: &[(&str, &str)]) -> DeciderConfig {
            let mut out = DeciderConfig::default();
            merge_decider(&mut out, &parse(user), ConfigLayer::User);
            merge_decider(&mut out, &parse(project), ConfigLayer::Project);
            let map: HashMap<String, String> = env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            apply_decider_env(&mut out, |k| map.get(k).cloned());
            out
        }

        fn resolve(
            user: &str,
            project: &str,
            env: &[(&str, &str)],
        ) -> (DeciderSettings, Vec<String>) {
            resolve_decider(&build(user, project, env))
        }

        fn user_key(mode: &str) -> String {
            format!("[decider]\nmode = \"{}\"\napi_key = \"{}\"\n", mode, KEY)
        }

        fn assert_no_secret(warnings: &[String]) {
            for w in warnings {
                assert!(!w.contains(KEY), "warning leaks the key: {w}");
            }
        }

        // ---- merge ---------------------------------------------------------

        #[test]
        fn project_key_endpoint_timeout_are_dropped_at_load() {
            let project = "[decider]\napi_key = \"proj-key\"\nendpoint = \"https://evil.example/d\"\ntimeout_ms = 9000\n";
            let cfg = build("", project, &[]);
            assert!(cfg.api_key.is_none());
            assert!(cfg.endpoint.is_none());
            assert!(cfg.timeout_ms.is_none());
            assert_eq!(cfg.warnings.len(), 3, "{:?}", cfg.warnings);
            for w in &cfg.warnings {
                assert!(!w.contains("proj-key"), "{w}");
                assert!(!w.contains("evil.example"), "{w}");
            }
            let (s, _) = resolve_decider(&cfg);
            assert!(s.api_key().is_none());
            assert_eq!(s.endpoint_origin(), SettingOrigin::Default);
            assert_eq!(s.endpoint().unwrap().as_str(), DEFAULT_DECIDER_ENDPOINT);
            assert_eq!(s.timeout(), Duration::from_millis(2000));
        }

        #[test]
        fn project_endpoint_never_replaces_user_endpoint() {
            let user = "[decider]\nendpoint = \"https://a.example/decide\"\n";
            let project = "[decider]\nendpoint = \"https://b.example/decide\"\n";
            let (s, _) = resolve(user, project, &[]);
            assert_eq!(s.endpoint().unwrap().as_str(), "https://a.example/decide");
            assert_eq!(s.endpoint_origin(), SettingOrigin::User);
            assert!(!format!("{:?}", s).contains("b.example"));

            let (s, _) = resolve("", project, &[]);
            assert_eq!(s.endpoint().unwrap().as_str(), DEFAULT_DECIDER_ENDPOINT);
            assert_eq!(s.endpoint_origin(), SettingOrigin::Default);
            assert!(!format!("{:?}", s).contains("b.example"));
        }

        #[test]
        fn project_mode_lands_only_in_project_mode() {
            let cfg = build(
                "[decider]\nmode = \"shadow\"\n",
                "[decider]\nmode = \"auto\"\n",
                &[],
            );
            assert_eq!(cfg.mode.as_deref(), Some("shadow"));
            assert_eq!(cfg.project_mode.as_deref(), Some("auto"));

            let cfg = build("", "[decider]\nmode = \"auto\"\n", &[]);
            assert!(cfg.mode.is_none());
            assert_eq!(cfg.project_mode.as_deref(), Some("auto"));
        }

        #[test]
        fn merge_config_leaves_decider_alone() {
            let mut base = KotoConfig::default();
            let loaded = LoadedConfig {
                config: toml::from_str("[decider]\nmode = \"auto\"\napi_key = \"x\"\n").unwrap(),
                request_store_keys: vec![],
                request_store_has_recursion: false,
                run_journal_keys: vec![],
                workflows_native_present: false,
                session_keys: vec![],
            };
            merge_config(&mut base, &loaded);
            assert!(base.decider.is_empty());
            assert!(base.decider.project_mode.is_none());
        }

        // ---- lenient parsing ----------------------------------------------

        #[test]
        fn wrong_types_and_unknown_keys_load_with_warnings() {
            let body = "[session]\nbackend = \"cloud\"\n\n[decider]\nmode = 3\ntimeout_ms = \"fast\"\nbogus = true\nendpoint = \"https://ok.example/d\"\n";
            let cfg: KotoConfig = toml::from_str(body).expect("still parses");
            assert_eq!(cfg.session.backend, "cloud");
            assert!(cfg.decider.mode.is_none());
            assert!(cfg.decider.timeout_ms.is_none());
            assert_eq!(
                cfg.decider.endpoint.as_deref(),
                Some("https://ok.example/d")
            );
            assert_eq!(cfg.decider.warnings.len(), 3, "{:?}", cfg.decider.warnings);
        }

        #[test]
        fn non_table_decider_loads_with_warning() {
            let cfg: KotoConfig = toml::from_str("decider = 3\n").expect("still parses");
            assert!(cfg.decider.is_empty());
            assert_eq!(cfg.decider.warnings.len(), 1);
        }

        #[test]
        fn load_config_file_is_lenient_for_both_layers() {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("config.toml");
            fs::write(
                &path,
                "[session]\nbackend = \"cloud\"\n[decider]\nmode = 3\ntimeout_ms = \"fast\"\n",
            )
            .unwrap();
            let loaded = load_config_file(&path, "user config").expect("loads");
            assert_eq!(loaded.config.session.backend, "cloud");

            let mut out = DeciderConfig::default();
            merge_decider(&mut out, &loaded.config.decider, ConfigLayer::Project);
            let (s, w) = resolve_decider(&out);
            assert_eq!(s.mode(), GlobalMode::Off);
            assert_eq!(w.len(), 2, "{w:?}");
            assert!(w.iter().all(|w| w.starts_with("project config: ")), "{w:?}");
        }

        // ---- env ------------------------------------------------------------

        #[test]
        fn env_replaces_user_values() {
            let user = "[decider]\nmode = \"off\"\napi_key = \"user-key\"\nendpoint = \"https://u.example/d\"\n";
            let cfg = build(
                user,
                "",
                &[
                    ("KOTO_DECIDER", "auto"),
                    ("KOTO_DECIDER_API_KEY", "env-key"),
                    ("KOTO_DECIDER_ENDPOINT", "https://e.example/d"),
                ],
            );
            assert_eq!(cfg.mode.as_deref(), Some("auto"));
            assert_eq!(cfg.mode_origin, SettingOrigin::Env);
            assert_eq!(cfg.api_key.as_deref(), Some("env-key"));
            assert_eq!(cfg.api_key_origin, SettingOrigin::Env);
            assert_eq!(cfg.endpoint.as_deref(), Some("https://e.example/d"));
            assert_eq!(cfg.endpoint_origin, SettingOrigin::Env);
        }

        #[test]
        fn empty_env_counts_as_unset() {
            let user = "[decider]\nmode = \"shadow\"\napi_key = \"user-key\"\nendpoint = \"https://u.example/d\"\n";
            let cfg = build(
                user,
                "",
                &[
                    ("KOTO_DECIDER", ""),
                    ("KOTO_DECIDER_API_KEY", ""),
                    ("KOTO_DECIDER_ENDPOINT", "  "),
                ],
            );
            assert_eq!(cfg.mode.as_deref(), Some("shadow"));
            assert_eq!(cfg.mode_origin, SettingOrigin::User);
            assert_eq!(cfg.api_key_origin, SettingOrigin::User);
            assert_eq!(cfg.endpoint_origin, SettingOrigin::User);
        }

        // ---- mode -----------------------------------------------------------

        #[test]
        fn mode_defaults_off_so_a_key_alone_does_not_opt_in() {
            let (s, w) = resolve(&format!("[decider]\napi_key = \"{}\"\n", KEY), "", &[]);
            assert_eq!(s.mode(), GlobalMode::Off);
            assert!(!s.opted_in());
            assert!(w.is_empty(), "{w:?}");
            let (s, _) = resolve("", "", &[("KOTO_DECIDER_API_KEY", KEY)]);
            assert!(!s.opted_in());
        }

        #[test]
        fn mode_parsing_trims_and_lowercases() {
            let (s, w) = resolve("[decider]\nmode = \" Shadow \"\n", "", &[]);
            assert_eq!(s.mode(), GlobalMode::Shadow);
            assert!(w.is_empty());
        }

        #[test]
        fn env_never_and_bogus_resolve_off_with_one_warning() {
            for bad in ["never", "bogus"] {
                let (s, w) = resolve("", "", &[("KOTO_DECIDER", bad)]);
                assert_eq!(s.mode(), GlobalMode::Off, "{bad}");
                assert_eq!(w.len(), 1, "{bad}: {w:?}");
                assert!(w[0].contains("KOTO_DECIDER"), "{bad}: {w:?}");
            }
        }

        #[test]
        fn user_never_and_bogus_resolve_off_with_one_warning() {
            for bad in ["never", "bogus"] {
                let (s, w) = resolve(&format!("[decider]\nmode = \"{bad}\"\n"), "", &[]);
                assert_eq!(s.mode(), GlobalMode::Off, "{bad}");
                assert_eq!(w.len(), 1, "{bad}: {w:?}");
                assert!(w[0].contains("user config"), "{bad}: {w:?}");
            }
        }

        #[test]
        fn bogus_project_mode_resolves_off_with_warning() {
            let (s, w) = resolve(&user_key("auto"), "[decider]\nmode = \"turbo\"\n", &[]);
            assert_eq!(s.mode(), GlobalMode::Off);
            assert!(!s.opted_in());
            assert_eq!(w.len(), 1, "{w:?}");
            assert!(w[0].contains("project config"), "{w:?}");
        }

        #[test]
        fn resolved_mode_is_minimum_of_global_and_project() {
            let cases: &[(&str, Option<&str>, GlobalMode)] = &[
                ("shadow", Some("auto"), GlobalMode::Shadow),
                ("auto", Some("shadow"), GlobalMode::Shadow),
                ("auto", Some("off"), GlobalMode::Off),
                ("auto", None, GlobalMode::Auto),
                ("off", Some("auto"), GlobalMode::Off),
            ];
            for (user, project, want) in cases {
                let project_body = project
                    .map(|p| format!("[decider]\nmode = \"{p}\"\n"))
                    .unwrap_or_default();
                let (s, _) = resolve(
                    &format!("[decider]\nmode = \"{user}\"\n"),
                    &project_body,
                    &[],
                );
                assert_eq!(s.mode(), *want, "user {user} project {project:?}");
            }
            let (s, _) = resolve(
                "",
                "[decider]\nmode = \"shadow\"\n",
                &[("KOTO_DECIDER", "auto")],
            );
            assert_eq!(s.mode(), GlobalMode::Shadow);
        }

        #[test]
        fn a_key_that_cannot_be_a_header_is_absent_with_a_warning() {
            let bad = [
                format!("{KEY}\nX"),
                format!("{KEY}\rX"),
                format!("{KEY}\tX"),
                format!("{KEY}\u{7f}"),
                format!("{KEY}\u{e9}"),
            ];
            for raw in &bad {
                // From the environment.
                let (s, w) = resolve(
                    "",
                    "",
                    &[("KOTO_DECIDER", "auto"), (ENV_DECIDER_API_KEY, raw)],
                );
                assert!(s.api_key().is_none(), "{raw:?}");
                assert!(!s.opted_in(), "{raw:?}");
                assert_eq!(s.api_key_origin(), SettingOrigin::Default);
                assert_no_secret(&w);
                assert_eq!(w.len(), 1, "{w:?}");
                assert!(w[0].starts_with(ENV_DECIDER_API_KEY), "{w:?}");
                assert!(w[0].contains("control or non-ASCII"), "{w:?}");

                // From user config (TOML escapes carry the character).
                let mut cfg = build("[decider]\nmode = \"auto\"\n", "", &[]);
                cfg.api_key = Some(raw.clone());
                cfg.api_key_origin = SettingOrigin::User;
                let (s, w) = resolve_decider(&cfg);
                assert!(s.api_key().is_none() && !s.opted_in(), "{raw:?}");
                assert_no_secret(&w);
                assert!(w.iter().any(|m| m.starts_with("user config")), "{w:?}");
            }
            // An ordinary key is untouched and still opts in.
            let (s, w) = resolve(&user_key("auto"), "", &[]);
            assert!(s.opted_in(), "{w:?}");
        }

        #[test]
        fn project_can_never_raise_mode() {
            let modes = ["off", "shadow", "auto", "never", "bogus", ""];
            for g in modes {
                for p in modes {
                    let (s, _) = resolve(
                        &format!("[decider]\nmode = \"{g}\"\n"),
                        &format!("[decider]\nmode = \"{p}\"\n"),
                        &[],
                    );
                    let global = GlobalMode::parse(g).unwrap_or(GlobalMode::Off);
                    assert!(s.mode() <= global, "global {g} project {p}");
                }
            }
        }

        // ---- opted_in -------------------------------------------------------

        fn baseline_env() -> Vec<(&'static str, &'static str)> {
            vec![
                ("KOTO_DECIDER", "auto"),
                ("KOTO_DECIDER_API_KEY", KEY),
                ("KOTO_DECIDER_ENDPOINT", "https://env.example/decide"),
            ]
        }

        #[test]
        fn opted_in_baseline() {
            let (s, w) = resolve("", "", &baseline_env());
            assert!(s.opted_in());
            assert!(w.is_empty(), "{w:?}");
            let (s, _) = resolve(&user_key("shadow"), "", &[]);
            assert!(s.opted_in());
        }

        #[test]
        fn opted_in_false_when_global_mode_off() {
            let mut env = baseline_env();
            env[0] = ("KOTO_DECIDER", "off");
            assert!(!resolve("", "", &env).0.opted_in());
        }

        #[test]
        fn opted_in_false_when_project_mode_off() {
            let (s, _) = resolve(&user_key("auto"), "[decider]\nmode = \"off\"\n", &[]);
            assert!(!s.opted_in());
        }

        #[test]
        fn opted_in_false_without_key_and_no_warning() {
            let env: Vec<_> = baseline_env()
                .into_iter()
                .filter(|(k, _)| *k != "KOTO_DECIDER_API_KEY")
                .collect();
            let (s, w) = resolve("", "", &env);
            assert!(!s.opted_in());
            assert!(w.is_empty(), "auto without a key must not warn: {w:?}");
        }

        #[test]
        fn opted_in_false_for_unparseable_endpoint() {
            let mut env = baseline_env();
            env[2] = ("KOTO_DECIDER_ENDPOINT", "not a url");
            let (s, w) = resolve("", "", &env);
            assert!(!s.opted_in());
            assert!(s.endpoint().is_none());
            assert_eq!(w.len(), 1, "{w:?}");
        }

        #[test]
        fn opted_in_false_for_plain_http_non_loopback() {
            for bad in [
                "http://example.com/decide",
                "http://localhost.example.com/decide",
                "http://10.0.0.1/decide",
                "ftp://example.com/decide",
            ] {
                let mut env = baseline_env();
                env[2] = ("KOTO_DECIDER_ENDPOINT", bad);
                let (s, w) = resolve("", "", &env);
                assert!(!s.opted_in(), "{bad}");
                assert_eq!(w.len(), 1, "{bad}: {w:?}");
            }
        }

        #[test]
        fn plain_http_loopback_is_accepted() {
            for ok in [
                "http://127.0.0.1:4000/decide",
                "http://[::1]:4000/decide",
                "http://localhost:4000/decide",
            ] {
                let mut env = baseline_env();
                env[2] = ("KOTO_DECIDER_ENDPOINT", ok);
                let (s, w) = resolve("", "", &env);
                assert!(s.opted_in(), "{ok}: {w:?}");
            }
        }

        #[test]
        fn opted_in_false_for_userinfo_and_warning_hides_it() {
            for bad in [
                "https://user:pass@host.example/decide",
                "https://token@host.example/decide",
            ] {
                let mut env = baseline_env();
                env[2] = ("KOTO_DECIDER_ENDPOINT", bad);
                let (s, w) = resolve("", "", &env);
                assert!(!s.opted_in(), "{bad}");
                assert_eq!(w.len(), 1, "{bad}: {w:?}");
                for word in ["user", "pass", "token"] {
                    assert!(!w[0].contains(word), "{bad}: {:?}", w[0]);
                }
                assert_no_secret(&w);
            }
        }

        #[test]
        fn endpoint_warning_prints_only_scheme_host_path() {
            let mut env = baseline_env();
            env[2] = (
                "KOTO_DECIDER_ENDPOINT",
                "http://example.com:81/decide?q=secretq#fragf",
            );
            let (_, w) = resolve("", "", &env);
            assert_eq!(w.len(), 1);
            assert!(w[0].contains("http://example.com/decide"), "{:?}", w[0]);
            for bad in ["secretq", "fragf", ":81"] {
                assert!(!w[0].contains(bad), "{:?}", w[0]);
            }
        }

        #[test]
        fn same_layer_rule_all_combinations() {
            let user_ep = "[decider]\nendpoint = \"https://u.example/d\"\n";
            // env key + env endpoint: ok
            let (s, _) = resolve("", "", &baseline_env());
            assert!(s.opted_in());
            // env key + default endpoint: ok
            let (s, _) = resolve("", "", &baseline_env()[..2]);
            assert!(s.opted_in());
            assert_eq!(s.endpoint_origin(), SettingOrigin::Default);
            // user key + user endpoint: ok
            let body = format!("{}endpoint = \"https://u.example/d\"\n", user_key("auto"));
            let (s, _) = resolve(&body, "", &[]);
            assert!(s.opted_in());
            assert_eq!(s.endpoint_origin(), SettingOrigin::User);
            // user key + default endpoint: ok
            let (s, _) = resolve(&user_key("auto"), "", &[]);
            assert!(s.opted_in());
            assert_eq!(s.endpoint_origin(), SettingOrigin::Default);
            // env key + user endpoint: refused with a warning
            let (s, w) = resolve(user_ep, "", &baseline_env()[..2]);
            assert!(!s.opted_in());
            assert_eq!(w.len(), 1, "{w:?}");
            assert_no_secret(&w);
            // user key + env endpoint (the injected-endpoint case): refused
            let (s, w) = resolve(
                &user_key("auto"),
                "",
                &[("KOTO_DECIDER_ENDPOINT", "https://attacker.example/grab")],
            );
            assert!(!s.opted_in());
            assert_eq!(s.api_key_origin(), SettingOrigin::User);
            assert_eq!(s.endpoint_origin(), SettingOrigin::Env);
            assert_eq!(w.len(), 1, "{w:?}");
            assert_no_secret(&w);
        }

        #[test]
        fn opted_in_is_stable_across_env_changes() {
            let (s, _) = resolve("", "", &baseline_env());
            // opted_in reads only fields; the real process env is irrelevant.
            assert!(s.opted_in());
            assert!(s.opted_in());
        }

        // ---- timeout --------------------------------------------------------

        #[test]
        fn timeout_default_cap_and_fallback() {
            let (s, w) = resolve("", "", &[]);
            assert_eq!(s.timeout(), Duration::from_millis(2000));
            assert!(w.is_empty());

            let (s, w) = resolve("[decider]\ntimeout_ms = 500\n", "", &[]);
            assert_eq!(s.timeout(), Duration::from_millis(500));
            assert!(w.is_empty());

            let (s, w) = resolve("[decider]\ntimeout_ms = 60000\n", "", &[]);
            assert_eq!(s.timeout(), Duration::from_millis(10_000));
            assert_eq!(w.len(), 1);

            for bad in ["0", "-3", "\"fast\""] {
                let (s, w) = resolve(&format!("[decider]\ntimeout_ms = {bad}\n"), "", &[]);
                assert_eq!(s.timeout(), Duration::from_millis(2000), "{bad}");
                assert_eq!(w.len(), 1, "{bad}: {w:?}");
            }
        }

        // ---- key handling ---------------------------------------------------

        #[test]
        fn debug_never_contains_the_key() {
            let cfg = build(&user_key("auto"), "", &[]);
            assert!(!format!("{:?}", cfg).contains(KEY));
            let (s, _) = resolve_decider(&cfg);
            assert!(!format!("{:?}", s).contains(KEY));
            assert!(!format!("{:?}", s.api_key()).contains(KEY));
            assert_eq!(s.api_key().unwrap().expose_for_transport(), KEY);
        }

        #[test]
        fn default_endpoint_is_a_full_https_url() {
            assert_eq!(
                DEFAULT_DECIDER_ENDPOINT,
                "https://api.typesafe.ai/v1/systemone"
            );
            let u = Url::parse(DEFAULT_DECIDER_ENDPOINT).unwrap();
            assert_eq!(u.scheme(), "https");
            assert!(u.path().len() > 1, "a full decision URL, not a base");
            assert!(
                crate::config::validate::check_decider_endpoint(DEFAULT_DECIDER_ENDPOINT).is_ok()
            );
        }
    }

    // -----------------------------------------------------------------------
    // User config file permissions
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn write_user_toml_value_creates_and_tightens_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".koto").join("config.toml");
        let mut val = toml::Value::Table(toml::map::Map::new());
        crate::config::set_value_in_toml(&mut val, "decider.mode", "shadow").unwrap();

        write_user_toml_value(&path, &val).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_user_toml_value(&path, &val).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// Serializes the tests that change the process-wide cwd and env.
    /// Without it they race each other under the parallel test runner:
    /// one test's `SetCwd` restore can land while another is mid-load.
    fn process_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
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

/// The `KOTO_SESSION_*` layer, exercised through its lookup closure so no
/// test touches the process environment.
#[cfg(test)]
mod session_env_tests {
    use super::*;
    use crate::config::session_env::{
        Origin, ENV_ACCESS_KEY, ENV_BACKEND, ENV_BUCKET, ENV_ENDPOINT, ENV_PATH_STYLE, ENV_REGION,
        ENV_SECRET_KEY,
    };
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> EnvValue {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned().map(Ok)
    }

    /// A config as the two file layers leave it, from TOML bodies.
    fn from_files(user: &str, project: &str) -> KotoConfig {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = KotoConfig::default();
        config.session.backend = "local".to_string();
        for (body, origin, user_layer) in [
            (user, Origin::User, true),
            (project, Origin::Project, false),
        ] {
            if body.is_empty() {
                continue;
            }
            let path = tmp.path().join(if user_layer {
                "user.toml"
            } else {
                "project.toml"
            });
            fs::write(&path, body).unwrap();
            let loaded = load_config_file(&path, "test config").unwrap();
            merge_config(&mut config, &loaded);
            if user_layer {
                merge_marker_retention(&mut config, &loaded);
                merge_check_window(&mut config, &loaded);
            }
            mark_file_origins(&mut config, &loaded, origin, user_layer);
        }
        config
    }

    fn resolve(user: &str, project: &str, env: &[(&str, &str)]) -> KotoConfig {
        let mut config = from_files(user, project);
        apply_session_env(&mut config, lookup(env));
        apply_credential_env(&mut config, lookup(env));
        config
    }

    const BOTH_FILES_USER: &str = "[session]\nbackend = \"local\"\n[session.cloud]\nendpoint = \"https://user.example\"\nbucket = \"user-bucket\"\nregion = \"user-region\"\npath_style = false\n";
    const BOTH_FILES_PROJECT: &str = "[session]\nbackend = \"local\"\n[session.cloud]\nendpoint = \"https://project.example\"\nbucket = \"project-bucket\"\nregion = \"project-region\"\npath_style = false\n";

    #[test]
    fn each_variable_wins_over_both_files() {
        let c = resolve(
            BOTH_FILES_USER,
            BOTH_FILES_PROJECT,
            &[
                (ENV_BACKEND, "cloud"),
                (ENV_ENDPOINT, "https://env.example"),
                (ENV_BUCKET, "env-bucket"),
                (ENV_REGION, "env-region"),
                (ENV_PATH_STYLE, "true"),
            ],
        );
        assert_eq!(c.session.backend, "cloud");
        assert_eq!(
            c.session.cloud.endpoint.as_deref(),
            Some("https://env.example")
        );
        assert_eq!(c.session.cloud.bucket.as_deref(), Some("env-bucket"));
        assert_eq!(c.session.cloud.region.as_deref(), Some("env-region"));
        assert_eq!(c.session.cloud.path_style, Some(true));
        for (setting, var) in [
            ("session.backend", ENV_BACKEND),
            ("session.cloud.endpoint", ENV_ENDPOINT),
            ("session.cloud.bucket", ENV_BUCKET),
            ("session.cloud.region", ENV_REGION),
            ("session.cloud.path_style", ENV_PATH_STYLE),
        ] {
            assert_eq!(c.session_env.origin(setting), Origin::Env(var), "{setting}");
        }
    }

    #[test]
    fn project_beats_user_and_user_beats_default_without_variables() {
        let c = resolve(BOTH_FILES_USER, BOTH_FILES_PROJECT, &[]);
        assert_eq!(c.session.cloud.bucket.as_deref(), Some("project-bucket"));
        assert_eq!(
            c.session_env.origin("session.cloud.bucket"),
            Origin::Project
        );
        let c = resolve(BOTH_FILES_USER, "", &[]);
        assert_eq!(c.session.cloud.bucket.as_deref(), Some("user-bucket"));
        assert_eq!(c.session_env.origin("session.cloud.bucket"), Origin::User);
        let c = resolve("", "", &[]);
        assert_eq!(c.session.backend, "local");
        assert!(c.session.cloud.bucket.is_none());
        assert_eq!(c.session_env.origin("session.backend"), Origin::Default);
        assert_eq!(c.session_env.origin("session.cloud.bucket"), Origin::Unset);
    }

    #[test]
    fn empty_and_whitespace_values_fall_through() {
        for value in ["", "  ", "\t"] {
            let c = resolve(
                "",
                "[session]\nbackend = \"local\"\n",
                &[(ENV_BACKEND, value), (ENV_BUCKET, value)],
            );
            assert_eq!(c.session.backend, "local", "{value:?}");
            assert_eq!(c.session_env.origin("session.backend"), Origin::Project);
            assert!(c.session.cloud.bucket.is_none());
            assert!(c.session_env.invalid.is_empty());
        }
    }

    #[test]
    fn values_are_trimmed_and_case_sensitive() {
        let c = resolve("", "", &[(ENV_BACKEND, " cloud ")]);
        assert_eq!(c.session.backend, "cloud");
        assert!(crate::config::session_env::check(&c).is_ok());
        let c = resolve("", "", &[(ENV_BACKEND, "Cloud")]);
        let err = crate::config::session_env::check(&c)
            .unwrap_err()
            .to_string();
        assert!(err.contains(ENV_BACKEND) && err.contains("Cloud"), "{err}");
    }

    #[test]
    fn a_mixed_run_reports_three_origins() {
        let c = resolve(
            "[session.cloud]\nregion = \"user-region\"\n",
            "[session.cloud]\nbucket = \"project-bucket\"\n",
            &[(ENV_BACKEND, "cloud")],
        );
        assert_eq!(
            c.session_env.origin("session.backend"),
            Origin::Env(ENV_BACKEND)
        );
        assert_eq!(
            c.session_env.origin("session.cloud.bucket"),
            Origin::Project
        );
        assert_eq!(c.session_env.origin("session.cloud.region"), Origin::User);
    }

    #[test]
    fn an_invalid_value_refuses_and_never_falls_back() {
        let c = resolve(BOTH_FILES_USER, "", &[(ENV_PATH_STYLE, "yes")]);
        assert_eq!(
            c.session.cloud.path_style, None,
            "the user file's value must not show through"
        );
        assert_eq!(
            c.session_env.origin("session.cloud.path_style"),
            Origin::Env(ENV_PATH_STYLE)
        );
        let err = crate::config::session_env::check(&c)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(ENV_PATH_STYLE) && err.contains("\"yes\""),
            "{err}"
        );

        let c = resolve("", "", &[(ENV_BACKEND, "s3")]);
        assert_eq!(c.session.backend, "s3");
        let err = crate::config::session_env::check(&c)
            .unwrap_err()
            .to_string();
        assert!(err.contains(ENV_BACKEND) && err.contains("s3"), "{err}");
    }

    #[test]
    fn a_non_unicode_value_counts_as_set_and_invalid() {
        let mut c = from_files(BOTH_FILES_USER, "");
        apply_session_env(&mut c, |k: &str| (k == ENV_BUCKET).then_some(Err(())));
        assert!(c.session.cloud.bucket.is_none());
        assert_eq!(
            c.session_env.origin("session.cloud.bucket"),
            Origin::Env(ENV_BUCKET)
        );
        assert!(crate::config::session_env::check(&c).is_err());

        let mut c = from_files("", "");
        apply_credential_env(&mut c, |k: &str| match k {
            ENV_ACCESS_KEY => Some(Err(())),
            "AWS_ACCESS_KEY_ID" | "AWS_SECRET_ACCESS_KEY" => Some(Ok("aws".to_string())),
            _ => None,
        });
        assert!(c.session.cloud.access_key.is_none(), "never the AWS key");
        assert!(crate::config::session_env::check(&c).is_err());
    }

    #[test]
    fn marker_retain_days_has_no_variable() {
        let c = resolve(
            "[session.cloud]\nmarker_retain_days = 7\n",
            "",
            &[("KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS", "1")],
        );
        assert_eq!(c.session.cloud.marker_retain_days(), 7);
        assert_eq!(
            c.session_env.origin("session.cloud.marker_retain_days"),
            Origin::User
        );
    }

    #[test]
    fn check_window_comes_from_the_user_file_or_its_variable_never_the_project() {
        use super::super::session_env::ENV_CHECK_WINDOW;
        let setting = "session.cloud.check_window_secs";
        let unset = resolve("", "", &[]);
        assert_eq!(unset.session.cloud.check_window_secs(), 5);
        assert_eq!(unset.session_env.origin(setting), Origin::Default);

        let project = resolve("", "[session.cloud]\ncheck_window_secs = 600\n", &[]);
        assert_eq!(project.session.cloud.check_window_secs(), 5);
        assert_eq!(project.session_env.origin(setting), Origin::Default);

        let user = resolve("[session.cloud]\ncheck_window_secs = 2\n", "", &[]);
        assert_eq!(user.session.cloud.check_window_secs(), 2);
        assert_eq!(user.session_env.origin(setting), Origin::User);

        let env = resolve(
            "[session.cloud]\ncheck_window_secs = 2\n",
            "",
            &[(ENV_CHECK_WINDOW, "0")],
        );
        assert_eq!(env.session.cloud.check_window_secs(), 0);
        assert_eq!(
            env.session_env.origin(setting),
            Origin::Env(ENV_CHECK_WINDOW)
        );

        let bad = resolve("", "", &[(ENV_CHECK_WINDOW, "five")]);
        assert_eq!(bad.session.cloud.check_window_secs, None);
        let err = session_env::check(&bad).unwrap_err().to_string();
        assert!(
            err.contains(ENV_CHECK_WINDOW) && err.contains("\"five\""),
            "{err}"
        );
    }

    const AWS: [(&str, &str); 2] = [
        ("AWS_ACCESS_KEY_ID", "aws-access"),
        ("AWS_SECRET_ACCESS_KEY", "aws-secret"),
    ];

    #[test]
    fn the_koto_pair_replaces_the_aws_pair_and_the_file_as_a_whole() {
        let user = "[session.cloud]\naccess_key = \"file-access\"\nsecret_key = \"file-secret\"\n";
        let mut env = AWS.to_vec();
        env.extend([
            (ENV_ACCESS_KEY, "koto-access"),
            (ENV_SECRET_KEY, "koto-secret"),
        ]);
        let c = resolve(user, "", &env);
        assert_eq!(c.session.cloud.access_key.as_deref(), Some("koto-access"));
        assert_eq!(c.session.cloud.secret_key.as_deref(), Some("koto-secret"));
        assert_eq!(
            c.session_env.origin("session.cloud.access_key"),
            Origin::Env(ENV_ACCESS_KEY)
        );
        assert_eq!(c.secret_sources.access_key_env, Some(ENV_ACCESS_KEY));
        assert!(c.session_env.pair_missing.is_none());
    }

    #[test]
    fn a_half_pair_refuses_on_cloud_naming_the_missing_half() {
        for (set, missing) in [
            (ENV_ACCESS_KEY, ENV_SECRET_KEY),
            (ENV_SECRET_KEY, ENV_ACCESS_KEY),
        ] {
            let user =
                "[session.cloud]\naccess_key = \"file-access\"\nsecret_key = \"file-secret\"\n";
            let mut env = AWS.to_vec();
            env.push((set, "koto-half"));
            env.push((ENV_BACKEND, "cloud"));
            let c = resolve(user, "", &env);
            assert_eq!(c.session_env.pair_missing, Some(missing));
            let neither_aws_nor_file = |v: &Option<String>| {
                !matches!(
                    v.as_deref(),
                    Some("aws-access" | "aws-secret" | "file-access" | "file-secret")
                )
            };
            assert!(neither_aws_nor_file(&c.session.cloud.access_key));
            assert!(neither_aws_nor_file(&c.session.cloud.secret_key));
            let err = crate::config::session_env::check(&c)
                .unwrap_err()
                .to_string();
            assert!(err.contains(missing), "{err}");

            let mut env = env.clone();
            env.retain(|(k, _)| *k != ENV_BACKEND);
            let c = resolve(user, "", &env);
            assert!(
                crate::config::session_env::check(&c).is_ok(),
                "local backend is unaffected"
            );
        }
    }

    #[test]
    fn empty_koto_credentials_fall_through_to_the_aws_pair() {
        let mut env = AWS.to_vec();
        env.extend([(ENV_ACCESS_KEY, ""), (ENV_SECRET_KEY, "  ")]);
        let c = resolve("", "", &env);
        assert_eq!(c.session.cloud.access_key.as_deref(), Some("aws-access"));
        assert_eq!(
            c.session_env.origin("session.cloud.access_key"),
            Origin::Env("AWS_ACCESS_KEY_ID")
        );
        assert!(c.session_env.pair_missing.is_none());
    }

    #[test]
    fn the_aws_pair_keeps_its_empty_value_handling() {
        let user = "[session.cloud]\naccess_key = \"file-access\"\n";
        let c = resolve(user, "", &[("AWS_ACCESS_KEY_ID", "")]);
        assert_eq!(c.session.cloud.access_key.as_deref(), Some(""));
    }

    #[test]
    fn the_pair_never_goes_to_a_project_file_destination() {
        let env = [
            (ENV_BACKEND, "cloud"),
            (ENV_ACCESS_KEY, "koto-access"),
            (ENV_SECRET_KEY, "koto-secret"),
        ];
        for project in [
            "[session.cloud]\nendpoint = \"https://evil.example\"\n",
            "[session.cloud]\nbucket = \"evil\"\n",
        ] {
            let c = resolve("", project, &env);
            let err = crate::config::session_env::check(&c)
                .unwrap_err()
                .to_string();
            assert!(err.contains("project file"), "{err}");
        }
        let mut with_endpoint = env.to_vec();
        with_endpoint.push((ENV_ENDPOINT, "https://mine.example"));
        let c = resolve(
            "",
            "[session.cloud]\nendpoint = \"https://evil.example\"\n",
            &with_endpoint,
        );
        assert!(crate::config::session_env::check(&c).is_ok());
        // Without the koto pair the project file keeps today's reach.
        let c = resolve(
            "",
            "[session.cloud]\nendpoint = \"https://elsewhere.example\"\n",
            &[(ENV_BACKEND, "cloud"), AWS[0], AWS[1]],
        );
        assert!(crate::config::session_env::check(&c).is_ok());
    }

    #[test]
    fn a_session_table_without_backend_reports_default() {
        for (user, project) in [
            ("[session.cloud]\nbucket = \"b\"\n[session]\n", ""),
            ("", "[session]\n"),
        ] {
            let c = resolve(user, project, &[]);
            assert_eq!(c.session_env.origin("session.backend"), Origin::Default);
        }
        let c = resolve("", "session.backend = \"local\"\n", &[]);
        assert_eq!(c.session_env.origin("session.backend"), Origin::Project);
    }

    #[test]
    fn an_empty_backend_in_a_file_leaves_the_lower_layer_its_origin() {
        // The merge ignores an empty backend, so the user file's value and
        // origin stand.
        let c = resolve(
            "[session]\nbackend = \"cloud\"\n",
            "[session]\nbackend = \"\"\n",
            &[],
        );
        assert_eq!(c.session.backend, "cloud");
        assert_eq!(c.session_env.origin("session.backend"), Origin::User);
    }

    #[test]
    fn command_vars_carry_only_what_the_environment_supplied() {
        let c = resolve(
            "[session.cloud]\nregion = \"user-region\"\n",
            "",
            &[
                (ENV_BACKEND, "cloud"),
                (ENV_BUCKET, " env-bucket "),
                (ENV_ACCESS_KEY, "koto-access"),
                (ENV_SECRET_KEY, "koto-secret"),
            ],
        );
        let vars = crate::config::session_env::command_vars(&c);
        assert_eq!(
            vars,
            vec![
                (ENV_BACKEND.to_string(), "cloud".to_string()),
                (ENV_BUCKET.to_string(), "env-bucket".to_string()),
                (ENV_ACCESS_KEY.to_string(), "koto-access".to_string()),
                (ENV_SECRET_KEY.to_string(), "koto-secret".to_string()),
            ]
        );
        // AWS-pair credentials stay where they were: not added.
        let c = resolve("", "", &[(ENV_BACKEND, "cloud"), AWS[0], AWS[1]]);
        assert_eq!(
            crate::config::session_env::command_vars(&c),
            vec![(ENV_BACKEND.to_string(), "cloud".to_string())]
        );
    }

    #[test]
    fn redaction_labels_the_koto_variable() {
        let c = resolve(
            "",
            "",
            &[
                (ENV_ACCESS_KEY, "koto-access-value"),
                (ENV_SECRET_KEY, "koto-secret-value"),
            ],
        );
        let keys = crate::config::redaction_keys(&c);
        assert!(keys.contains(&(ENV_ACCESS_KEY.to_string(), "koto-access-value".to_string())));
        assert!(keys.contains(&(ENV_SECRET_KEY.to_string(), "koto-secret-value".to_string())));
        assert_eq!(&ENV_SECRET_NAMES[1..3], &[ENV_ACCESS_KEY, ENV_SECRET_KEY]);
    }
}
