use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use std::time::Duration;

use anyhow::{Context, Result};
use url::Url;

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
/// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY,
///    `KOTO_REQUEST_STORE_*` for request-store dimensions)
/// 2. Project config (.koto/config.toml in current directory)
/// 3. User config (~/.koto/config.toml)
/// 4. Built-in defaults
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
            merge_config(&mut config, &user_config);
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
        merge_config(&mut config, &project_config);
        merge_decider(
            &mut config.decider,
            &project_config.config.decider,
            ConfigLayer::Project,
        );
    }

    // Layer 3: env var overrides for credentials
    if let Ok(val) = env::var("AWS_ACCESS_KEY_ID") {
        config.session.cloud.access_key = Some(val);
    }
    if let Ok(val) = env::var("AWS_SECRET_ACCESS_KEY") {
        config.session.cloud.secret_key = Some(val);
    }

    // Layer 3b: KOTO_REQUEST_STORE_* env-var overrides for the
    // request_store block.
    apply_request_store_env_overrides(&mut config.request_store);

    // Layer 3c: KOTO_DECIDER* env overrides for the global decider values.
    apply_decider_env(&mut config.decider, |k| env::var(k).ok());

    Ok(config)
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
    let workflows_native_present = raw
        .as_table()
        .and_then(|t| t.get("workflows"))
        .and_then(|v| v.as_table())
        .map(|t| t.contains_key("native"))
        .unwrap_or(false);
    Ok(LoadedConfig {
        config,
        request_store_keys,
        request_store_has_recursion,
        workflows_native_present,
    })
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
    workflows_native_present: bool,
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

    // Key.
    let api_key = cfg
        .api_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
        .map(ApiKey::new);
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
                decider: Default::default(),
            },
            request_store_keys: vec![],
            request_store_has_recursion: false,
            workflows_native_present: false,
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
            workflows_native_present: false,
        };
        merge_config(&mut base, &overlay);

        // backend stays "cloud" because overlay backend is empty string (Default)
        // but serde deserialization would give "local" — here we're using Default directly.
        // The merge only overwrites if source backend is non-empty.
        assert_eq!(base.session.backend, "cloud");
        assert_eq!(base.session.cloud.bucket, Some("existing".to_string()));
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
            workflows_native_present: true,
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
            workflows_native_present: false,
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
                workflows_native_present: false,
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
