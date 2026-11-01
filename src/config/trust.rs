//! Project config trust.
//!
//! A project's `.koto/config.toml` applies only after the user trusts it
//! with `koto config trust`, run in the project's directory. The grant is
//! kept in `<koto root>/trust.toml`, outside any repository and outside any
//! config key, and binds the canonical project directory, the canonical path
//! of the file and the SHA-256 of the file's exact bytes, so any change to
//! the file needs a new grant. A grant also keeps the settings it granted,
//! only so a later `koto config trust` can say what changed; whether a file
//! is trusted is decided by the hash alone.
//!
//! [`check_project`] is the one decision every reader of the project file
//! goes through. It never parses TOML, and it opens the file only when the
//! directory has a grant; on a match it returns the bytes it hashed, so the
//! bytes parsed are the bytes verified. `koto config set` and `unset` on the
//! project file go through [`set_project_key`] and [`unset_project_key`],
//! which grant exactly the bytes they write.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::validate::{self, ProjectLayer};

/// The grant store's file name, in the koto root.
pub const TRUST_FILE: &str = "trust.toml";

/// The grant store format this build reads and writes.
const STORE_VERSION: u32 = 1;

/// The largest project config file koto reads.
pub const PROJECT_FILE_CAP: u64 = 64 * 1024;

/// The longest string `koto config trust` echoes from a project file.
const ECHO_CAP: usize = 120;

/// Path to the project config file: `.koto/config.toml`, relative to the
/// working directory. Private to the config module: every reader goes
/// through [`check_project`].
pub(super) fn project_config_path() -> PathBuf {
    PathBuf::from(".koto").join("config.toml")
}

/// One trusted project config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    /// The canonical project directory.
    pub dir: String,
    /// The canonical path of the project config file.
    pub file: String,
    /// The SHA-256 of the file's exact bytes, as hex.
    pub sha256: String,
    #[serde(default)]
    pub granted_at: String,
    /// The allowed settings the grant covered, as written, for the next
    /// grant's list of changes. Never consulted for trust.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default, rename = "grant", skip_serializing_if = "Vec::is_empty")]
    grants: Vec<Grant>,
}

/// The grants in a koto root's `trust.toml`.
#[derive(Debug, Default)]
pub struct TrustStore {
    grants: Vec<Grant>,
    /// Why the store reads as empty although a file is there: unreadable,
    /// malformed, or written by a newer koto. No grant applies then.
    pub problem: Option<String>,
}

impl TrustStore {
    /// Read `<root>/trust.toml`. A missing file is an empty store; one that
    /// can't be used is an empty store with [`TrustStore::problem`] set.
    pub fn load(root: &Path) -> TrustStore {
        let path = root.join(TRUST_FILE);
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TrustStore::default(),
            Err(_) => return TrustStore::unusable(&path, "can't be read"),
        };
        match toml::from_str::<StoreFile>(&content) {
            Ok(file) if file.version <= STORE_VERSION => TrustStore {
                grants: file.grants,
                problem: None,
            },
            Ok(_) => TrustStore::unusable(&path, "was written by a newer koto"),
            Err(_) => TrustStore::unusable(&path, "is not a valid trust store"),
        }
    }

    fn unusable(path: &Path, why: &str) -> TrustStore {
        TrustStore {
            grants: Vec::new(),
            problem: Some(format!(
                "{} {why}, so no project config is trusted",
                path.display()
            )),
        }
    }

    /// The grant for a canonical project directory, if any.
    pub fn grant_for(&self, dir: &str) -> Option<&Grant> {
        self.grants.iter().find(|g| g.dir == dir)
    }

    fn save(&self, root: &Path) -> Result<()> {
        crate::koto_home::ensure_private_root(root)?;
        let file = StoreFile {
            version: STORE_VERSION,
            grants: self.grants.clone(),
        };
        let content = toml::to_string_pretty(&file)?;
        let path = root.join(TRUST_FILE);
        let tmp = root.join(format!("{TRUST_FILE}.tmp-{}", std::process::id()));
        write_private(&tmp, content.as_bytes())?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("writing the trust store {}", path.display()))?;
        Ok(())
    }
}

/// Write `bytes` to a new or truncated file, mode 0600 on Unix.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Record `grant`, replacing any grant for the same directory. The store is
/// read again just before the write, which narrows, but doesn't close, the
/// window in which a grant another process records is lost; there is no
/// lock. A lost grant fails closed: that directory's file is ignored again.
fn record_grant(root: &Path, grant: Grant) -> Result<()> {
    let mut store = TrustStore::load(root);
    if let Some(problem) = store.problem {
        bail!("{problem}; fix or remove it before trusting a project config");
    }
    store.grants.retain(|g| g.dir != grant.dir);
    store.grants.push(grant);
    store.save(root)
}

/// The koto root whose grants apply, or why none may be used. Grants are
/// read only from a root the user's own environment names: `HOME`, or an
/// absolute `KOTO_HOME`. A relative `KOTO_HOME` resolves against the
/// working directory, so it doesn't name a fixed root, and no grant under
/// it is used.
fn grant_root() -> std::result::Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("KOTO_HOME").filter(|v| !v.is_empty()) {
        if !Path::new(&raw).is_absolute() {
            return Err(
                "KOTO_HOME is a relative path, so no trust grant under it is used".to_string(),
            );
        }
    }
    match crate::koto_home::koto_home() {
        Some(root) if root.is_absolute() => Ok(root),
        Some(root) => Err(format!("the koto root {} is not absolute", root.display())),
        None => Err("the koto root can't be determined".to_string()),
    }
}

/// The canonical working directory.
fn canonical_cwd() -> std::result::Result<PathBuf, String> {
    std::env::current_dir()
        .and_then(fs::canonicalize)
        .map_err(|_| "the working directory can't be resolved".to_string())
}

/// Whether `.koto` or `.koto/config.toml` under `dir` is a symlink.
fn symlinked(dir: &Path) -> bool {
    let is_link = |p: PathBuf| {
        fs::symlink_metadata(p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    };
    is_link(dir.join(".koto")) || is_link(dir.join(project_config_path()))
}

/// Whether the project config file and the user config file are the same
/// file, as when koto runs from the home directory.
fn same_as_user(project: &Path, user: Option<&Path>) -> bool {
    let Some(user) = user else {
        return false;
    };
    if let (Some(a), Some(b)) = (resolved(project), resolved(user)) {
        if a == b {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(project), fs::metadata(user)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

/// `path` made absolute and canonical as far as it exists: the deepest
/// existing ancestor canonicalized, the rest appended. So two spellings of
/// a file that isn't there yet still compare equal.
fn resolved(path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut rest = Vec::new();
    let mut at = absolute.as_path();
    loop {
        if let Ok(canonical) = fs::canonicalize(at) {
            let mut out = canonical;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        rest.push(at.file_name()?.to_os_string());
        at = at.parent()?;
    }
}

/// Read a project config file once: it must be a regular file no larger
/// than [`PROJECT_FILE_CAP`]. The check is made on the opened handle, and
/// the read stops one byte past the cap.
pub fn read_bounded(path: &Path) -> std::result::Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|_| "it can't be read".to_string())?;
    let meta = file
        .metadata()
        .map_err(|_| "it can't be read".to_string())?;
    if !meta.is_file() {
        return Err("it is not a regular file".to_string());
    }
    let mut bytes = Vec::new();
    file.take(PROJECT_FILE_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "it can't be read".to_string())?;
    if bytes.len() as u64 > PROJECT_FILE_CAP {
        return Err(format!("it is larger than {} KiB", PROJECT_FILE_CAP / 1024));
    }
    Ok(bytes)
}

/// The SHA-256 of `bytes`, as hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What [`check_project`] found in the working directory.
#[derive(Debug)]
pub enum ProjectCheck {
    /// No `.koto/config.toml` here.
    Absent,
    /// The project config file is the user config file.
    SameAsUser,
    /// There is a file and no grant applies to it. `reason` says why no
    /// grant could apply at all, when that is the cause.
    Untrusted { reason: Option<String> },
    /// The directory has a grant, but the file is no longer what was
    /// granted.
    Changed,
    /// The file matches its grant. `bytes` are the bytes that were hashed.
    Trusted { bytes: Vec<u8> },
}

/// Decide whether the working directory's project config applies, without
/// parsing it.
pub fn check_project(user_path: Option<&Path>) -> ProjectCheck {
    let rel = project_config_path();
    if fs::symlink_metadata(&rel).is_err() {
        return ProjectCheck::Absent;
    }
    if same_as_user(&rel, user_path) {
        return ProjectCheck::SameAsUser;
    }
    let cwd = match canonical_cwd() {
        Ok(c) => c,
        Err(reason) => {
            return ProjectCheck::Untrusted {
                reason: Some(reason),
            }
        }
    };
    let root = match grant_root() {
        Ok(r) => r,
        Err(reason) => {
            return ProjectCheck::Untrusted {
                reason: Some(reason),
            }
        }
    };
    let store = TrustStore::load(&root);
    if let Some(problem) = store.problem {
        return ProjectCheck::Untrusted {
            reason: Some(problem),
        };
    }
    let Some(grant) = store.grant_for(&cwd.to_string_lossy()) else {
        return ProjectCheck::Untrusted { reason: None };
    };
    // A grant is never made for a symlinked file, so one here means the
    // file isn't the one granted.
    if symlinked(&cwd) {
        return ProjectCheck::Changed;
    }
    let file = cwd.join(&rel);
    match read_bounded(&file) {
        Ok(bytes) if grant.file == file.to_string_lossy() && grant.sha256 == sha256_hex(&bytes) => {
            ProjectCheck::Trusted { bytes }
        }
        _ => ProjectCheck::Changed,
    }
}

// ---------------------------------------------------------------------------
// What load_config reports about the project file
// ---------------------------------------------------------------------------

/// Whether the project config applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProjectStatus {
    /// No project config file here (or it is the user config file).
    #[default]
    Absent,
    /// A file is here and isn't trusted, so it was ignored.
    Untrusted,
    /// The file changed since it was trusted, so it was ignored.
    Changed,
    /// The file is trusted and its allowed keys applied.
    Trusted,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectStatus::Absent => "absent",
            ProjectStatus::Untrusted => "untrusted",
            ProjectStatus::Changed => "changed",
            ProjectStatus::Trusted => "trusted",
        }
    }
}

/// What `load_config` did with the project config file, for the note and
/// for `koto config list` and `koto session dir`.
#[derive(Debug, Clone, Default)]
pub struct ProjectConfigState {
    pub status: ProjectStatus,
    /// Why no grant could apply, when that was the cause.
    pub reason: Option<String>,
    /// Keys a trusted file set that project config may not set.
    pub dropped: Vec<String>,
    /// How many keys a trusted file set that koto doesn't recognize.
    pub unknown: usize,
}

impl ProjectConfigState {
    /// The state for a [`ProjectCheck`] that applied nothing.
    pub fn ignored(check: &ProjectCheck) -> ProjectConfigState {
        match check {
            ProjectCheck::Untrusted { reason } => ProjectConfigState {
                status: ProjectStatus::Untrusted,
                reason: reason.clone(),
                ..Default::default()
            },
            ProjectCheck::Changed => ProjectConfigState {
                status: ProjectStatus::Changed,
                ..Default::default()
            },
            _ => ProjectConfigState::default(),
        }
    }

    /// The state for a trusted file, filtered to `layer`.
    pub fn trusted(layer: &ProjectLayer) -> ProjectConfigState {
        ProjectConfigState {
            status: ProjectStatus::Trusted,
            reason: None,
            dropped: layer.dropped.clone(),
            unknown: layer.unknown,
        }
    }

    /// The `project_config` object `koto config list --json` and
    /// `koto session dir --json` print.
    pub fn to_json(&self) -> serde_json::Value {
        let mut out = serde_json::json!({
            "path": project_config_path().display().to_string(),
            "status": self.status.as_str(),
        });
        if self.status == ProjectStatus::Trusted {
            out["dropped"] = serde_json::json!(self.dropped);
            if self.unknown > 0 {
                out["unrecognized"] = serde_json::json!(self.unknown);
            }
        }
        out
    }

    /// The `# project config:` comment lines `koto config list` prints.
    pub fn to_toml_comments(&self) -> String {
        let path = project_config_path();
        let mut out = match self.status {
            ProjectStatus::Absent => return String::new(),
            ProjectStatus::Untrusted => {
                format!("# project config: {} untrusted (ignored)\n", path.display())
            }
            ProjectStatus::Changed => format!(
                "# project config: {} changed since it was trusted (ignored)\n",
                path.display()
            ),
            ProjectStatus::Trusted => format!("# project config: {} trusted\n", path.display()),
        };
        for key in &self.dropped {
            out.push_str(&format!("# project config: {key} ignored\n"));
        }
        out
    }

    /// The stderr lines for this state: a note when the file was ignored,
    /// and a warning per key a trusted file may not set. The decider's own
    /// keys are left out; `merge_decider` warns about those.
    pub fn messages(&self) -> Vec<String> {
        let path = project_config_path();
        let mut out = Vec::new();
        match (self.status, &self.reason) {
            (ProjectStatus::Untrusted, Some(reason)) => {
                out.push(format!("note: {} is ignored: {reason}", path.display()))
            }
            (ProjectStatus::Untrusted, None) => out.push(format!(
                "note: {} in this directory isn't trusted, so koto ignores it; review the \
                 file, and if it should apply here, trust it with `koto config trust`",
                path.display()
            )),
            (ProjectStatus::Changed, _) => out.push(format!(
                "note: {} changed since it was trusted, so koto ignores it; review the \
                 file, and if it should apply here, trust it again with `koto config trust`",
                path.display()
            )),
            _ => {}
        }
        for key in self.dropped.iter().filter(|k| !k.starts_with("decider.")) {
            out.push(format!(
                "warning: project config: {key} is ignored in project config (set it in user \
                 config with `koto config set --user`{})",
                env_hint(key)
            ));
        }
        if self.unknown > 0 {
            out.push(format!(
                "warning: project config: {} unrecognized key(s) ignored",
                self.unknown
            ));
        }
        out
    }
}

/// The environment variable that also sets `key`, as a message suffix.
fn env_hint(key: &str) -> &'static str {
    match key {
        "session.cloud.access_key" => " or KOTO_SESSION_CLOUD_ACCESS_KEY",
        "session.cloud.secret_key" => " or KOTO_SESSION_CLOUD_SECRET_KEY",
        "session.cloud.check_window_secs" => " or KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS",
        _ => "",
    }
}

/// Print [`ProjectConfigState::messages`] on stderr, once per process.
pub fn note_once(state: &ProjectConfigState) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let lines = state.messages();
    if lines.is_empty() {
        return;
    }
    ONCE.call_once(|| {
        for line in lines {
            eprintln!("{line}");
        }
    });
}

// ---------------------------------------------------------------------------
// koto config trust / untrust
// ---------------------------------------------------------------------------

/// What `koto config trust` granted, for its report.
#[derive(Debug)]
pub struct TrustReport {
    pub dir: PathBuf,
    pub layer: ProjectLayer,
    pub settings: BTreeMap<String, String>,
    pub previous: Option<BTreeMap<String, String>>,
}

/// Trust the working directory's project config as it is now.
///
/// Refuses a symlinked `.koto` or `.koto/config.toml`, a missing file, the
/// user's own config file, a file over the size cap, and one that doesn't
/// parse; grants nothing in each case.
pub fn trust_here(user_path: Option<&Path>) -> Result<TrustReport> {
    let rel = project_config_path();
    let cwd = canonical_cwd().map_err(anyhow::Error::msg)?;
    if same_as_user(&rel, user_path) {
        bail!(
            "{} here is your user config, which always applies; there is nothing to trust",
            rel.display()
        );
    }
    if symlinked(&cwd) {
        bail!(
            "{} or its directory is a symlink; koto trusts only a regular file",
            rel.display()
        );
    }
    if fs::symlink_metadata(&rel).is_err() {
        bail!(
            "there is no project config ({}) in this directory",
            rel.display()
        );
    }
    let root = grant_root().map_err(anyhow::Error::msg)?;
    let file = cwd.join(&rel);
    let bytes = read_bounded(&file)
        .map_err(|why| anyhow::anyhow!("can't trust {}: {why}", rel.display()))?;
    let layer = parse_layer(&bytes)?;
    let settings = validate::settings_snapshot(&layer.kept);
    let dir = cwd.to_string_lossy().to_string();
    let previous = TrustStore::load(&root)
        .grant_for(&dir)
        .map(|g| g.settings.clone());
    record_grant(
        &root,
        Grant {
            dir: dir.clone(),
            file: file.to_string_lossy().to_string(),
            sha256: sha256_hex(&bytes),
            granted_at: crate::engine::types::now_iso8601(),
            settings: settings.clone(),
        },
    )?;
    Ok(TrustReport {
        dir: cwd,
        layer,
        settings,
        previous,
    })
}

/// Remove the working directory's grant. Returns whether there was one.
pub fn untrust_here() -> Result<bool> {
    let cwd = canonical_cwd().map_err(anyhow::Error::msg)?;
    let root = grant_root().map_err(anyhow::Error::msg)?;
    let mut store = TrustStore::load(&root);
    if let Some(problem) = store.problem {
        bail!("{problem}; fix or remove it first");
    }
    let dir = cwd.to_string_lossy();
    let before = store.grants.len();
    store.grants.retain(|g| g.dir != dir);
    if store.grants.len() == before {
        return Ok(false);
    }
    store.save(&root)?;
    Ok(true)
}

/// Parse a project file's bytes and filter them to what project config may
/// set. The error names the file, never its content.
fn parse_layer(bytes: &[u8]) -> Result<ProjectLayer> {
    let raw = parse_raw(bytes)?;
    Ok(validate::filter_project_layer(&raw))
}

/// Parse a project file's bytes as a TOML table.
pub(crate) fn parse_raw(bytes: &[u8]) -> Result<toml::Value> {
    let path = project_config_path();
    let content = std::str::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(
            "project config {} is not valid UTF-8 (the file's content isn't shown because it can \
             hold credentials)",
            path.display()
        )
    })?;
    let raw: toml::Value = content.parse().map_err(|e| {
        super::resolve::ConfigParseError::new(
            "project config",
            &path,
            content,
            &e,
            super::resolve::ParseProblem::Syntax,
        )
    })?;
    Ok(raw)
}

/// `s` with control, line-separator, bidirectional, zero-width,
/// interlinear-annotation and tag characters removed and its length
/// capped, for echoing text a project file supplied.
pub fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200B}'..='\u{200F}'
                        | '\u{2028}'..='\u{202E}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{061C}'
                        | '\u{FEFF}'
                        | '\u{FFF9}'..='\u{FFFB}'
                        | '\u{E0000}'..='\u{E007F}'
                )
        })
        .collect();
    if cleaned.chars().count() > ECHO_CAP {
        let cut: String = cleaned.chars().take(ECHO_CAP).collect();
        format!("{cut}...")
    } else {
        cleaned
    }
}

/// What `koto config trust` says about an endpoint: its host, and anything
/// about it worth a second look.
pub fn endpoint_notes(raw: &str) -> (String, Vec<&'static str>) {
    let Ok(url) = url::Url::parse(raw) else {
        return (sanitize(raw), vec!["is not a valid URL"]);
    };
    let host = url.host_str().unwrap_or("").to_string();
    let mut notes = Vec::new();
    if url.scheme() == "http" && !validate::is_loopback_url(&url) {
        notes.push("uses plain http to a host that isn't this machine");
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            if ip.is_private() || ip.is_link_local() {
                notes.push("is a private or link-local address");
            }
        }
        Some(url::Host::Ipv6(ip)) => {
            let first = ip.segments()[0];
            if (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 {
                notes.push("is a private or link-local address");
            }
        }
        _ => {}
    }
    if url.query().is_some() {
        notes.push("carries a query string");
    }
    if !raw.is_ascii() || host.split('.').any(|label| label.starts_with("xn--")) {
        notes.push("has non-ASCII characters in its host");
    }
    (sanitize(&host), notes)
}

/// The lines `koto config trust` prints for `report`.
pub fn render_trust_report(report: &TrustReport) -> String {
    let path = project_config_path();
    let mut out = format!(
        "Trusted {} in {}.\n",
        path.display(),
        sanitize(&report.dir.to_string_lossy())
    );
    let destination = [
        "session.backend",
        "session.cloud.endpoint",
        "session.cloud.bucket",
    ];
    let mut keys: Vec<&String> = report.settings.keys().collect();
    keys.sort_by_key(|k| (!destination.contains(&k.as_str()), k.to_string()));
    if keys.is_empty() {
        out.push_str("It sets nothing koto applies from project config.\n");
    } else {
        out.push_str("It applies here:\n");
        for key in keys {
            let value = &report.settings[key];
            if key == "session.cloud.endpoint" {
                let (host, notes) = endpoint_notes(value);
                out.push_str(&format!("  {key} = {} (host {host})\n", sanitize(value)));
                for note in notes {
                    out.push_str(&format!("    warning: this endpoint {note}\n"));
                }
            } else {
                out.push_str(&format!("  {key} = {}\n", sanitize(value)));
            }
        }
    }
    let cloud = report.settings.get("session.backend").map(String::as_str) == Some("cloud")
        || report.settings.contains_key("session.cloud.endpoint");
    if cloud {
        out.push_str(
            "Session state for sessions run here is stored at that destination, and any storage \
             keys in your environment or user config sign its requests.\n",
        );
    }
    if !report.layer.dropped.is_empty() || report.layer.unknown > 0 {
        let mut ignored: Vec<String> = report.layer.dropped.clone();
        if report.layer.unknown > 0 {
            ignored.push(format!("{} unrecognized key(s)", report.layer.unknown));
        }
        out.push_str(&format!(
            "Ignored, since project config can't set them: {}\n",
            ignored.join(", ")
        ));
    }
    if let Some(previous) = &report.previous {
        let mut changes = Vec::new();
        let all: std::collections::BTreeSet<&String> =
            previous.keys().chain(report.settings.keys()).collect();
        for key in all {
            match (previous.get(key), report.settings.get(key)) {
                (Some(a), Some(b)) if a != b => {
                    changes.push(format!("  {key}: {} -> {}", sanitize(a), sanitize(b)))
                }
                (Some(a), None) => changes.push(format!("  {key}: {} -> (unset)", sanitize(a))),
                (None, Some(b)) => changes.push(format!("  {key}: (unset) -> {}", sanitize(b))),
                _ => {}
            }
        }
        if !changes.is_empty() {
            out.push_str("Changed since it was last trusted:\n");
            for change in changes {
                out.push_str(&change);
                out.push('\n');
            }
        }
    }
    out.push_str("Run `koto config untrust` here to stop applying it.\n");
    out
}

// ---------------------------------------------------------------------------
// koto config set / unset on the project file
// ---------------------------------------------------------------------------

/// Where a `koto config set` or `unset` on the project file starts from.
enum EditBase {
    /// The project file is the user config file (koto runs from the home
    /// directory): it's edited as the user file, and needs no grant.
    User { path: PathBuf },
    /// No file yet.
    Create { cwd: PathBuf, root: PathBuf },
    /// A trusted file, with the bytes that were verified.
    Existing {
        cwd: PathBuf,
        root: PathBuf,
        bytes: Vec<u8>,
    },
}

/// Decide whether the project file may be edited, refusing a symlink and a
/// file that isn't trusted as it stands.
fn edit_base(user_path: Option<&Path>) -> Result<EditBase> {
    let rel = project_config_path();
    if let Some(user) = user_path.filter(|u| same_as_user(&rel, Some(u))) {
        return Ok(EditBase::User {
            path: user.to_path_buf(),
        });
    }
    let cwd = canonical_cwd().map_err(anyhow::Error::msg)?;
    if symlinked(&cwd) {
        bail!(
            "{} or its directory is a symlink; koto writes only a regular file",
            rel.display()
        );
    }
    match check_project(user_path) {
        ProjectCheck::Absent => {
            let root = writable_root()?;
            Ok(EditBase::Create { cwd, root })
        }
        // Handled by the same-file check above; reached only if the file
        // appeared in between, and then it is the user file all the same.
        ProjectCheck::SameAsUser => Ok(EditBase::User {
            path: user_path.map(Path::to_path_buf).unwrap_or(rel),
        }),
        ProjectCheck::Untrusted { reason } => bail!(
            "{} isn't trusted{}; review it, trust it with `koto config trust`, then change it",
            rel.display(),
            reason.map(|r| format!(" ({r})")).unwrap_or_default()
        ),
        ProjectCheck::Changed => bail!(
            "{} changed since it was trusted; review it, trust it again with \
             `koto config trust`, then change it",
            rel.display()
        ),
        ProjectCheck::Trusted { bytes } => {
            let root = writable_root()?;
            Ok(EditBase::Existing { cwd, root, bytes })
        }
    }
}

/// The koto root a write's grant goes to, checked before the project file
/// is written, so an unusable trust store refuses the edit instead of
/// leaving a file koto then can't trust.
fn writable_root() -> Result<PathBuf> {
    let root = grant_root().map_err(anyhow::Error::msg)?;
    if let Some(problem) = TrustStore::load(&root).problem {
        bail!("{problem}; fix or remove it before changing the project config");
    }
    Ok(root)
}

/// The document an edit starts from, or `None` when there is no file.
fn edit_doc(base: &EditBase) -> Result<Option<toml::Value>> {
    match base {
        EditBase::User { path } => super::resolve::load_toml_value(path).map(Some),
        EditBase::Create { .. } => Ok(None),
        EditBase::Existing { bytes, .. } => parse_raw(bytes).map(Some),
    }
}

/// Write the edited document and grant exactly the bytes written.
fn write_and_grant(base: EditBase, doc: &toml::Value) -> Result<()> {
    let content = toml::to_string_pretty(doc)?;
    let bytes = content.as_bytes();
    let (cwd, root) = match base {
        EditBase::User { path } => return super::resolve::write_user_toml_value(&path, doc),
        EditBase::Create { cwd, root } => {
            let dir = cwd.join(".koto");
            if fs::symlink_metadata(&dir).is_err() {
                fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            let path = cwd.join(project_config_path());
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| format!("creating {}", path.display()))?;
            file.write_all(bytes)?;
            (cwd, root)
        }
        EditBase::Existing { cwd, root, .. } => {
            let path = cwd.join(project_config_path());
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .with_context(|| format!("writing {}", path.display()))?;
            file.write_all(bytes)?;
            (cwd, root)
        }
    };
    let layer = validate::filter_project_layer(doc);
    record_grant(
        &root,
        Grant {
            dir: cwd.to_string_lossy().to_string(),
            file: cwd
                .join(project_config_path())
                .to_string_lossy()
                .to_string(),
            sha256: sha256_hex(bytes),
            granted_at: crate::engine::types::now_iso8601(),
            settings: validate::settings_snapshot(&layer.kept),
        },
    )
}

/// `koto config set <key> <value>` on the project file: create it, or edit
/// it when it is trusted, and grant the bytes written.
pub fn set_project_key(user_path: Option<&Path>, key: &str, value: &str) -> Result<()> {
    validate::validate_project_key(key).map_err(anyhow::Error::msg)?;
    validate::validate_project_value(key, value).map_err(anyhow::Error::msg)?;
    let base = edit_base(user_path)?;
    let mut doc = edit_doc(&base)?.unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    super::set_value_in_toml(&mut doc, key, value).map_err(anyhow::Error::msg)?;
    write_and_grant(base, &doc)
}

/// `koto config unset <key>` on the project file: edit it when it is
/// trusted and grant the bytes written. With no file there is nothing to
/// remove, and nothing is written.
pub fn unset_project_key(user_path: Option<&Path>, key: &str) -> Result<()> {
    let base = edit_base(user_path)?;
    let Some(mut doc) = edit_doc(&base)? else {
        return Ok(());
    };
    super::unset_value_in_toml(&mut doc, key).map_err(anyhow::Error::msg)?;
    write_and_grant(base, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn grant(dir: &str, sha: &str) -> Grant {
        Grant {
            dir: dir.to_string(),
            file: format!("{dir}/.koto/config.toml"),
            sha256: sha.to_string(),
            granted_at: String::new(),
            settings: BTreeMap::new(),
        }
    }

    #[test]
    fn a_missing_store_is_empty() {
        let root = TempDir::new().unwrap();
        let store = TrustStore::load(root.path());
        assert!(store.problem.is_none());
        assert!(store.grant_for("/p").is_none());
    }

    #[test]
    fn grants_round_trip_and_replace_by_directory() {
        let root = TempDir::new().unwrap();
        record_grant(root.path(), grant("/p", "aa")).unwrap();
        record_grant(root.path(), grant("/q", "bb")).unwrap();
        record_grant(root.path(), grant("/p", "cc")).unwrap();
        let store = TrustStore::load(root.path());
        assert_eq!(store.grant_for("/p").unwrap().sha256, "cc");
        assert_eq!(store.grant_for("/q").unwrap().sha256, "bb");
        assert_eq!(store.grants.len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.path().join(TRUST_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_unusable_store_grants_nothing_and_is_not_overwritten() {
        let root = TempDir::new().unwrap();
        let path = root.path().join(TRUST_FILE);
        fs::write(&path, "not [ a store").unwrap();
        assert!(TrustStore::load(root.path()).problem.is_some());
        assert!(record_grant(root.path(), grant("/p", "aa")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not [ a store");

        fs::write(&path, "version = 99\n").unwrap();
        let store = TrustStore::load(root.path());
        assert!(store.problem.unwrap().contains("newer koto"));
    }

    #[test]
    fn read_bounded_takes_only_a_small_regular_file() {
        let dir = TempDir::new().unwrap();
        let small = dir.path().join("small.toml");
        fs::write(&small, "a = 1\n").unwrap();
        assert_eq!(read_bounded(&small).unwrap(), b"a = 1\n");

        let big = dir.path().join("big.toml");
        fs::write(&big, vec![b'#'; PROJECT_FILE_CAP as usize + 1]).unwrap();
        assert!(read_bounded(&big).unwrap_err().contains("larger"));

        let exact = dir.path().join("exact.toml");
        fs::write(&exact, vec![b'#'; PROJECT_FILE_CAP as usize]).unwrap();
        assert!(read_bounded(&exact).is_ok());

        assert!(read_bounded(dir.path()).is_err());
    }

    #[test]
    fn sanitize_strips_invisible_characters_and_caps_length() {
        assert_eq!(sanitize("a\u{202E}b\u{200B}c\n\u{1b}[31m"), "abc[31m");
        assert_eq!(sanitize("x\u{2028}y\u{E0041}\u{E0042}z"), "xyz");
        let long = "x".repeat(500);
        let out = sanitize(&long);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), ECHO_CAP + 3);
    }

    #[test]
    fn endpoint_notes_flag_what_deserves_a_second_look() {
        let (host, notes) = endpoint_notes("https://store.example/path");
        assert_eq!(host, "store.example");
        assert!(notes.is_empty());

        let (_, notes) = endpoint_notes("http://store.example");
        assert!(notes.iter().any(|n| n.contains("plain http")));
        assert!(endpoint_notes("http://127.0.0.1:9000").1.is_empty());

        let (_, notes) = endpoint_notes("https://169.254.169.254/");
        assert!(notes.iter().any(|n| n.contains("link-local")));
        let (_, notes) = endpoint_notes("https://10.0.0.5/");
        assert!(notes.iter().any(|n| n.contains("private")));
        let (_, notes) = endpoint_notes("https://store.example/?token=x");
        assert!(notes.iter().any(|n| n.contains("query")));
        let (_, notes) = endpoint_notes("https://st\u{f6}re.example/");
        assert!(notes.iter().any(|n| n.contains("non-ASCII")));
    }

    #[test]
    fn the_trust_report_leads_with_the_destination_and_lists_changes() {
        let raw: toml::Value = "[retention]\nkeep = \"7d\"\n[session]\nbackend = \"cloud\"\n\
             [session.cloud]\nbucket = \"b2\"\nendpoint = \"http://store.example\"\n\
             access_key = \"k\"\n[made_up]\nx = 1\n"
            .parse()
            .unwrap();
        let layer = validate::filter_project_layer(&raw);
        let settings = validate::settings_snapshot(&layer.kept);
        let mut previous = settings.clone();
        previous.insert("session.cloud.bucket".to_string(), "b1".to_string());
        let report = TrustReport {
            dir: PathBuf::from("/p"),
            layer,
            settings,
            previous: Some(previous),
        };
        let text = render_trust_report(&report);
        let backend = text.find("session.backend").unwrap();
        let retention = text.find("retention.keep").unwrap();
        assert!(backend < retention, "{text}");
        assert!(text.contains("(host store.example)"), "{text}");
        assert!(text.contains("plain http"), "{text}");
        assert!(text.contains("storage keys in your environment"), "{text}");
        assert!(text.contains("session.cloud.access_key"), "{text}");
        assert!(text.contains("1 unrecognized key(s)"), "{text}");
        assert!(!text.contains("made_up"), "{text}");
        assert!(text.contains("session.cloud.bucket: b1 -> b2"), "{text}");
        assert!(
            !text.contains("= k\n"),
            "a credential value was echoed: {text}"
        );
    }

    #[test]
    fn state_messages_name_the_file_and_never_its_content() {
        let untrusted = ProjectConfigState::ignored(&ProjectCheck::Untrusted { reason: None });
        let lines = untrusted.messages();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("note: .koto/config.toml"));
        assert!(lines[0].contains("koto config trust"));
        assert_eq!(untrusted.to_json()["status"], "untrusted");

        let changed = ProjectConfigState::ignored(&ProjectCheck::Changed);
        assert!(changed.messages()[0].contains("changed since it was trusted"));

        let raw: toml::Value =
            "[session.cloud]\nsecret_key = \"s3cr3t\"\n[decider]\napi_key = \"d\"\n"
                .parse()
                .unwrap();
        let trusted = ProjectConfigState::trusted(&validate::filter_project_layer(&raw));
        let lines = trusted.messages();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("session.cloud.secret_key"));
        assert!(!lines[0].contains("s3cr3t"));
        let json = trusted.to_json();
        assert_eq!(json["status"], "trusted");
        let mut dropped: Vec<&str> = json["dropped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        dropped.sort_unstable();
        assert_eq!(dropped, ["decider.api_key", "session.cloud.secret_key"]);

        assert!(ProjectConfigState::default().messages().is_empty());
        assert_eq!(ProjectConfigState::default().to_toml_comments(), "");
    }
}
