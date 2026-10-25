//! `koto init`'s entry flags: `--vars-file`, `--replace-terminal`,
//! `--attach-live` and `--koto-leg <req>:<leg>`.
//!
//! A skill that enters koto the same way on every invocation needs one of
//! four outcomes from a single command: a new session, an attached live
//! session, a fresh session replacing a finished one, or a refusal with
//! exit 2 and a typed error. This module is that command.
//!
//! **Check, then write.** Every check runs before anything is written:
//! variables (against the template, before the name is even looked up),
//! the live session's template identity, its origin record, its fixed
//! variables, and -- under `--koto-leg` -- every check `koto request
//! attach` makes. Writes then happen in a fixed order: create or replace
//! the session, bind the leg, re-apply the `rebind: true` variables, and
//! last record the session's dispatched-by links (`--dispatched-by`, and the
//! leg's requester under `--koto-leg`), in this session only. So a
//! refused invocation changes nothing, and a stale invocation naming a
//! leg it no longer owns can't flip a rebind variable such as `MERGE`.
//!
//! **Refusals are recorded on the leg.** Under `--koto-leg`, every
//! refusal is also written onto the named leg with `source: refused`, so
//! whoever waits on the leg learns why instead of waiting forever. The
//! store admits that write only while the leg is open and unbound, and
//! recording it never changes the invocation's own output or exit code.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cli::init_child::{self, TemplateCompileCache};
use crate::cli::request::{self, InitLegRefusal, InitLegTarget};
use crate::cli::task_spawn_error::SpawnErrorKind;
use crate::engine::request_store::AttachingSession;
use crate::engine::types::{
    Event, EventPayload, LegTemplates, SessionOrigin, StateFileHeader, TemplateIdentity,
    WorkflowResult,
};
use crate::engine::variables::{check_value, validate_rebind, VarError};
use crate::session::{Backend, SessionBackend};
use crate::template::types::CompiledTemplate;

/// Largest `--vars-file` koto reads. A variable file holds a handful of
/// short flags and slugs; anything near this size is a mistake or an
/// attack, and is refused before it is parsed.
pub(crate) const MAX_VARS_FILE_BYTES: u64 = 64 * 1024;

/// The entry flags of one `koto init` invocation.
pub(crate) struct EntryFlags {
    pub attach_live: bool,
    pub replace_terminal: bool,
    pub koto_leg: Option<InitLegTarget>,
}

// ===== Refusals =====

/// A refused invocation: the error body it prints, its exit code, and the
/// fields a refusal record on the leg carries.
struct Refusal {
    body: serde_json::Map<String, serde_json::Value>,
    exit: i32,
    reason: String,
    var: String,
    recorded: String,
    requested: String,
}

impl Refusal {
    /// A refusal with `error` text, `command: init`, and an optional code.
    fn new(code: Option<&str>, message: impl Into<String>, exit: i32) -> Self {
        let mut body = serde_json::Map::new();
        body.insert("error".into(), message.into().into());
        body.insert("command".into(), "init".into());
        if let Some(code) = code {
            body.insert("code".into(), code.into());
        }
        Self {
            body,
            exit,
            reason: code
                .map(kebab)
                .unwrap_or_else(|| "init-refused".to_string()),
            var: String::new(),
            recorded: String::new(),
            requested: String::new(),
        }
    }

    /// A refused variable. The text is the one `koto init` has always
    /// printed; the typed fields ride beside it.
    fn var(e: &VarError) -> Self {
        let mut r = Self::new(None, e.to_string(), 2);
        r.body.extend(e.fields());
        let (reason, var, recorded, requested) = match e {
            VarError::Duplicate { var } => (format!("duplicate-var:{var}"), var, "", ""),
            VarError::Unknown { var } => (format!("unknown-var:{var}"), var, "", ""),
            VarError::Missing { var } => (format!("missing-var:{var}"), var, "", ""),
            VarError::Invalid { var, value, .. } => {
                (format!("invalid-var:{var}"), var, "", value.as_str())
            }
            VarError::Mismatch {
                var,
                recorded,
                requested,
            } => (
                format!("var-mismatch:{var}"),
                var,
                recorded.as_str(),
                requested.as_str(),
            ),
            VarError::Malformed { .. } => {
                r.reason = "malformed-var".to_string();
                return r;
            }
            VarError::TerminalSession { .. } => {
                r.reason = "session-terminal".to_string();
                return r;
            }
        };
        r.reason = reason;
        r.var = var.clone();
        r.recorded = recorded.to_string();
        r.requested = requested.to_string();
        r
    }

    /// A leg check `koto request attach` would make, refused.
    fn leg(e: InitLegRefusal) -> Self {
        let code = e.code();
        let mut r = Self::new(
            Some(&code),
            e.error.message.clone(),
            e.error.code.exit_code(),
        );
        if !e.error.details.is_empty() {
            r.body.insert(
                "details".into(),
                serde_json::to_value(&e.error.details).unwrap_or_default(),
            );
        }
        if !e.var.is_empty() {
            r.body.insert("var".into(), e.var.as_str().into());
            r.body.insert("recorded".into(), e.recorded.as_str().into());
            r.body
                .insert("requested".into(), e.requested.as_str().into());
        }
        r.var = e.var;
        r.recorded = e.recorded;
        r.requested = e.requested;
        r
    }

    fn with_field(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.body.insert(key.into(), value.into());
        self
    }

    fn with_record(mut self, recorded: impl Into<String>, requested: impl Into<String>) -> Self {
        self.recorded = recorded.into();
        self.requested = requested.into();
        self
    }
}

/// `invalid_var` -> `invalid-var`.
fn kebab(code: &str) -> String {
    code.replace('_', "-")
}

/// Print the refusal and exit, recording it on the leg first under
/// `--koto-leg`. The record is best effort and silent, so the output and
/// exit code are the same with and without the flag.
fn refuse(entry: &EntryFlags, r: Refusal) -> ! {
    if let Some(target) = &entry.koto_leg {
        request::init_record_refusal(target, &r.reason, &r.var, &r.recorded, &r.requested);
    }
    super::exit_with_error_code(serde_json::Value::Object(r.body), r.exit)
}

/// Exit with `r` without recording it on the leg: the flags themselves
/// were unusable, so there is no leg to name.
pub(crate) fn usage_error(message: impl Into<String>) -> ! {
    let r = Refusal::new(Some("invalid_usage"), message, 2);
    super::exit_with_error_code(serde_json::Value::Object(r.body), r.exit)
}

// ===== --vars-file =====

/// Refuse an unreadable or malformed `--vars-file` (`invalid_vars_file`,
/// exit 2), recording it on the leg under `--koto-leg`.
pub(crate) fn refuse_vars_file(entry: &EntryFlags, reason: String) -> ! {
    refuse(
        entry,
        Refusal::new(
            Some("invalid_vars_file"),
            format!("--vars-file: {reason}"),
            2,
        ),
    )
}

/// Read `--vars-file`: a JSON list of `[key, value]` string pairs.
///
/// Returned as `KEY=VALUE` entries in file order, so a repeated key
/// survives to be refused as `duplicate_var` by the same resolution
/// `--var` goes through. A key that is empty or contains `=` can never
/// name a variable and is refused here, which keeps that conversion
/// lossless.
///
/// The file must be a regular file, not a symlink, and at most
/// [`MAX_VARS_FILE_BYTES`].
pub(crate) fn read_vars_file(path: &str) -> std::result::Result<Vec<String>, String> {
    let p = Path::new(path);
    let meta = std::fs::symlink_metadata(p).map_err(|e| format!("cannot read {path}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{path} is a symlink; pass the file itself"));
    }
    if !meta.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if meta.len() > MAX_VARS_FILE_BYTES {
        return Err(format!(
            "{path} is {} bytes, over the {MAX_VARS_FILE_BYTES}-byte limit",
            meta.len()
        ));
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The file was checked above; refuse it if it was swapped for a
        // symlink since.
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts
        .open(p)
        .map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut buf = Vec::new();
    use std::io::Read as _;
    file.take(MAX_VARS_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    if buf.len() as u64 > MAX_VARS_FILE_BYTES {
        return Err(format!(
            "{path} is over the {MAX_VARS_FILE_BYTES}-byte limit"
        ));
    }

    parse_vars_json(&buf).map_err(|reason| format!("{path}: {reason}"))
}

/// Parse the body of a vars file. See [`read_vars_file`].
fn parse_vars_json(bytes: &[u8]) -> std::result::Result<Vec<String>, String> {
    const SHAPE: &str = "expected a JSON list of [\"KEY\", \"VALUE\"] string pairs";
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("not valid JSON ({e}); {SHAPE}"))?;
    let list = value.as_array().ok_or_else(|| SHAPE.to_string())?;
    let mut entries = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let pair = item
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(|| format!("entry {i} is not a two-element list; {SHAPE}"))?;
        let (Some(key), Some(val)) = (pair[0].as_str(), pair[1].as_str()) else {
            return Err(format!("entry {i} is not a pair of strings; {SHAPE}"));
        };
        if key.is_empty() || key.contains('=') {
            return Err(format!(
                "entry {i} has key {key:?}, which can't name a variable"
            ));
        }
        entries.push(format!("{key}={val}"));
    }
    Ok(entries)
}

// ===== Variables =====

/// Split `KEY=VALUE` entries into pairs and check each one against the
/// template: no repeated key, no undeclared key, and every value within
/// its declared constraint and the allowlist.
///
/// This is the part of variable validation that doesn't depend on
/// whether a session exists, so it runs before the name is looked up. A
/// missing required variable is checked only when a session is actually
/// created: attaching to a live session keeps its recorded values.
fn validate_pairs(
    raw: &[String],
    template: &CompiledTemplate,
) -> std::result::Result<Vec<(String, String)>, VarError> {
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(raw.len());
    for entry in raw {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(VarError::Malformed {
                entry: entry.clone(),
                reason: "expected KEY=VALUE".to_string(),
            });
        };
        if key.is_empty() {
            return Err(VarError::Malformed {
                entry: entry.clone(),
                reason: "key must not be empty".to_string(),
            });
        }
        if pairs.iter().any(|(k, _)| k == key) {
            return Err(VarError::Duplicate {
                var: key.to_string(),
            });
        }
        let Some(decl) = template.variables.get(key) else {
            return Err(VarError::Unknown {
                var: key.to_string(),
            });
        };
        check_value(key, decl, value)?;
        pairs.push((key.to_string(), value.to_string()));
    }
    Ok(pairs)
}

// ===== The existing session =====

/// What the entry flags need to know about a session that already exists.
struct Existing {
    header: StateFileHeader,
    events: Vec<Event>,
    compiled: CompiledTemplate,
    current_state: String,
    /// The terminal state's name, `"cancelled"` for a cancelled session,
    /// or `None` while it is live.
    terminal: Option<String>,
}

fn read_existing(backend: &Backend, name: &str) -> std::result::Result<Existing, String> {
    let (header, events) = backend
        .read_events(name)
        .map_err(|e| format!("could not read session '{name}': {e}"))?;
    let session_dir = backend.session_dir(name);
    let machine = crate::engine::persistence::derive_machine_state(&header, &events, &session_dir)
        .ok_or_else(|| format!("session '{name}' has no readable state or template"))?;
    let compiled = super::load_compiled_template(&machine.template_path)
        .map_err(|e| format!("session '{name}': {e}"))?;
    let cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
    let terminal = if cancelled {
        Some("cancelled".to_string())
    } else if compiled
        .states
        .get(&machine.current_state)
        .map(|s| s.terminal)
        .unwrap_or(false)
    {
        Some(machine.current_state.clone())
    } else {
        None
    };
    Ok(Existing {
        header,
        events,
        compiled,
        current_state: machine.current_state,
        terminal,
    })
}

/// The workflow result of a session about to be replaced: the one it
/// recorded, else the one its terminal state resolves to. `None` for a
/// cancelled session that recorded none.
fn replaced_result(backend: &Backend, name: &str, existing: &Existing) -> Option<WorkflowResult> {
    if let Some(recorded) =
        crate::engine::terminal_result::recorded_result_for_current_arrival(&existing.events)
    {
        return Some(recorded);
    }
    if existing.terminal.as_deref() == Some("cancelled") {
        return None;
    }
    #[cfg(unix)]
    {
        Some(
            super::terminal_record(
                backend,
                backend,
                name,
                &existing.compiled,
                &existing.current_state,
            )
            .result,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (backend, name);
        None
    }
}

// ===== Origin =====

/// The caller's own origin record: the anchor a session created by this
/// invocation would get, and this backend's store identity.
fn caller_origin(backend: &Backend, execution_dir: Option<&Path>) -> Option<SessionOrigin> {
    let anchor: PathBuf = match execution_dir {
        Some(dir) => init_child::canonical_or_verbatim(dir),
        None => init_child::canonical_or_verbatim(&std::env::current_dir().ok()?),
    };
    init_child::origin_record(backend, Some(&anchor))
}

fn describe_origin(origin: &SessionOrigin) -> String {
    let bucket = origin
        .store
        .bucket
        .as_ref()
        .map(|b| format!(", bucket {b}"))
        .unwrap_or_default();
    format!(
        "{} ({} store {}{bucket})",
        origin.anchor.display(),
        origin.store.kind,
        origin.store.base.display()
    )
}

/// How a session's recorded origin relates to the caller's, when they
/// don't refuse.
#[derive(Debug, PartialEq, Eq)]
enum OriginMatch {
    /// The caller's own origin.
    Same,
    /// Started by the local store in the sessions directory the caller's
    /// cloud backend keeps its local copy in, from the same anchor: the
    /// same session, which the host's switch to the cloud backend left in
    /// a store kind its record doesn't name. Attaching uploads it and
    /// records the caller's origin.
    LocalIntoCloud,
}

/// Whether `recorded` is `caller`'s origin. A recorded cloud origin with
/// no bucket predates koto naming it, and matches the caller's bucket.
fn origin_match(recorded: &SessionOrigin, caller: &SessionOrigin) -> Option<OriginMatch> {
    if recorded.anchor != caller.anchor || recorded.store.base != caller.store.base {
        return None;
    }
    match (recorded.store.kind.as_str(), caller.store.kind.as_str()) {
        (r, c) if r == c => (recorded.store.bucket.is_none()
            || recorded.store.bucket == caller.store.bucket)
            .then_some(OriginMatch::Same),
        ("local", "cloud") => Some(OriginMatch::LocalIntoCloud),
        _ => None,
    }
}

/// Refuse a session whose origin record is missing or isn't the caller's.
fn check_origin(
    name: &str,
    recorded: Option<&SessionOrigin>,
    caller: Option<&SessionOrigin>,
) -> std::result::Result<OriginMatch, Box<Refusal>> {
    let Some(recorded) = recorded else {
        return Err(Box::new(
            Refusal::new(
                Some("origin_mismatch"),
                format!(
                    "session '{name}' has no origin record: it was created before koto recorded \
                 where a session was started, so it can't be confirmed as this caller's; finish \
                 it with the koto version that started it, or use another session name"
                ),
                2,
            )
            .with_field("recorded", serde_json::Value::Null)
            .with_record("", caller.map(describe_origin).unwrap_or_default()),
        ));
    };
    if let Some(found) = caller.and_then(|caller| origin_match(recorded, caller)) {
        return Ok(found);
    }
    let requested = caller
        .map(describe_origin)
        .unwrap_or_else(|| "(unknown)".to_string());
    // The way forward removes nothing: the caller goes to where the session
    // belongs. Same anchor means only the store differs, so name the store.
    let belongs = if caller.is_some_and(|caller| caller.anchor == recorded.anchor) {
        "run the command against the session store it was started in".to_string()
    } else {
        format!(
            "run the command from {}, where the session belongs",
            recorded.anchor.display()
        )
    };
    Err(Box::new(
        Refusal::new(
            Some("origin_mismatch"),
            format!(
                "session '{name}' belongs to another worktree or session store: it was started in \
             {}, and this invocation runs in {requested}; use another session name, or {belongs}",
                describe_origin(recorded)
            ),
            2,
        )
        .with_field("recorded", describe_origin(recorded))
        .with_field("requested", requested.clone())
        .with_record(describe_origin(recorded), requested),
    ))
}

// ===== The command =====

/// The template identity a session created from `template` would carry.
fn caller_identity(template: &Path, compiled: &CompiledTemplate, hash: &str) -> TemplateIdentity {
    TemplateIdentity {
        name: (!compiled.name.is_empty()).then(|| compiled.name.clone()),
        hash: hash.to_string(),
        source: template
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// Everything one invocation carries.
pub(crate) struct InitArgs<'a> {
    pub name: &'a str,
    pub template: &'a str,
    pub vars: &'a [String],
    pub intent: Option<&'a str>,
    /// `--dispatched-by`, already validated.
    pub dispatched_by: Option<&'a str>,
    /// `--continues`, already checked.
    pub continues: Option<&'a crate::cli::session::ContinuesTarget>,
    pub execution_dir: Option<&'a Path>,
    pub legacy_environment: bool,
}

/// Run `koto init` with `--vars-file` or any entry flag.
pub(crate) fn run(backend: &Backend, args: &InitArgs<'_>, entry: &EntryFlags) -> Result<()> {
    let name = args.name;
    if let Err(msg) = crate::discover::validate_workflow_name(name) {
        refuse(
            entry,
            Refusal::new(None, msg, 2)
                .with_field("allowed_pattern", "^[a-zA-Z0-9][a-zA-Z0-9._-]*$"),
        );
    }

    let template_path = Path::new(args.template);
    let mut cache = TemplateCompileCache::new();
    let (compiled, hash) = match init_child::compile_for_init(template_path, &mut cache) {
        Ok(c) => c,
        Err(info) => {
            let code = spawn_kind_code(&info.kind);
            refuse(entry, Refusal::new(Some(&code), info.message, 1));
        }
    };

    // Variables first: a bad or repeated value is reported as itself, never
    // as "already exists", and before anything is created or changed.
    let pairs = match validate_pairs(args.vars, &compiled) {
        Ok(p) => p,
        Err(e) => refuse(entry, Refusal::var(&e)),
    };

    if !backend.exists(name) {
        return create(backend, args, entry, &mut cache, &compiled, &hash, None);
    }

    if !entry.attach_live && !entry.replace_terminal {
        let base = format!(
            "workflow '{}' already exists; run `koto session cleanup {}` to reuse the name, \
             or `koto cancel --cleanup {}` to stop a running workflow first",
            name, name, name
        );
        let error = match super::stale_template_source_dir_clause(backend, name) {
            Some(clause) => format!("{}{}", base, clause),
            None => base,
        };
        let mut r = Refusal::new(None, error, 1);
        r.reason = "already-exists".to_string();
        refuse(entry, r);
    }

    let existing = match read_existing(backend, name) {
        Ok(e) => e,
        Err(msg) => refuse(entry, Refusal::new(Some("session_unreadable"), msg, 1)),
    };

    match (
        &existing.terminal,
        entry.attach_live,
        entry.replace_terminal,
    ) {
        (Some(_), _, true) => create(
            backend,
            args,
            entry,
            &mut cache,
            &compiled,
            &hash,
            Some(existing),
        ),
        (Some(state), _, false) => refuse(
            entry,
            Refusal::new(
                Some("session_terminal"),
                format!(
                    "session '{name}' is finished (state {state:?}), so there is nothing to \
                     attach to; pass --replace-terminal to start a fresh session under the name"
                ),
                2,
            )
            .with_field("state", state.as_str()),
        ),
        (None, true, _) => attach(backend, args, entry, &compiled, &hash, &pairs, existing),
        (None, false, _) => refuse(
            entry,
            Refusal::new(
                Some("session_live"),
                format!(
                    "session '{name}' is still running (state {:?}); --replace-terminal \
                     replaces only a finished session, and --attach-live joins a running one",
                    existing.current_state
                ),
                2,
            )
            .with_field("state", existing.current_state.as_str()),
        ),
    }
}

fn spawn_kind_code(kind: &SpawnErrorKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "init_failed".to_string())
}

/// What a replace does with the finished session it finds.
enum Disposition {
    /// Its hierarchy is released: it is removed as it always was.
    Release,
    /// Its hierarchy is kept, for `reason`: it moves aside.
    Keep { reason: String },
    /// Its own pin keeps it, whatever its hierarchy: it moves aside, and the
    /// answer carries the pin's `fields`.
    Pinned {
        reason: String,
        fields: serde_json::Map<String, serde_json::Value>,
    },
    /// A later session continuing it has standing: it moves aside with its
    /// session id, which its successors' relations resolve by, and the
    /// answer carries the relation's `fields`.
    Related {
        reason: String,
        fields: serde_json::Map<String, serde_json::Value>,
    },
}

impl Disposition {
    fn pinned(guard: &crate::session::pin::Guard) -> Self {
        Disposition::Pinned {
            reason: guard.text().unwrap_or_default(),
            fields: guard.fields(),
        }
    }
}

/// Judge the finished session `name` by the retention rule, over one scan
/// of the local sessions directory. Anything that stops the judgment (a
/// malformed retention setting, a sessions directory that can't be read, a
/// judgment that fails) counts as keep, with a warning, so the old run is
/// moved aside rather than removed.
fn predecessor_disposition(backend: &Backend, name: &str) -> Disposition {
    use crate::session::hierarchy::{reason_text, Graph, Judge, RetentionSettings, Scope};
    // A pin, read from disk now, keeps the old run before anything else.
    let guard = crate::session::pin::guard_removal(backend, name);
    if guard.keeps() {
        return Disposition::pinned(&guard);
    }
    let keep = |why: String| {
        eprintln!("warning: {why}; the finished session '{name}' is moved aside, not removed");
        Disposition::Keep { reason: why }
    };
    let settings = match RetentionSettings::load() {
        Ok(s) => s,
        Err(e) => return keep(format!("{e:#}")),
    };
    let graph = match Graph::scan(backend.local_base_dir()) {
        Ok(g) => g,
        Err(e) => return keep(format!("the sessions directory can't be read: {e:#}")),
    };
    let mut judge = Judge::new(&graph, settings, std::time::SystemTime::now());
    match judge.judge(name, &Scope::Whole) {
        Ok(j) if j.verdict.is_kept() => Disposition::Keep {
            reason: reason_text(&j.verdict),
        },
        // A later session continuing it has standing: it moves aside with
        // its session id, so the relation, which resolves by that id, still
        // names the moved session and keeps it.
        Ok(j) => {
            let related = crate::session::continues_bucket::relation_at_removal(backend, &j, name);
            if related.keeps() {
                Disposition::Related {
                    reason: crate::session::hierarchy::relation_text(&related).join("; "),
                    fields: crate::session::hierarchy::relation_json(&related),
                }
            } else {
                Disposition::Release
            }
        }
        Err(e) => keep(format!("its hierarchy can't be judged: {e:#}")),
    }
}

/// Create a session, or replace a terminal one, then bind the leg.
fn create(
    backend: &Backend,
    args: &InitArgs<'_>,
    entry: &EntryFlags,
    cache: &mut TemplateCompileCache,
    compiled: &CompiledTemplate,
    hash: &str,
    replacing: Option<Existing>,
) -> Result<()> {
    let name = args.name;
    let template_path = Path::new(args.template);

    // Full resolution, including required variables, before any write.
    let resolved = match super::resolve_variables(args.vars, &compiled.variables) {
        Ok(v) => v,
        Err(e) => refuse(entry, Refusal::var(&e)),
    };

    let requested_by = match &entry.koto_leg {
        Some(target) => {
            let facts = AttachingSession {
                session_id: name.to_string(),
                template: Some(caller_identity(template_path, compiled, hash)),
                variables: compiled.variables.clone(),
                bindings: resolved.clone(),
                terminal_state: None,
                pointer: None,
            };
            match request::init_precheck_leg(target, &facts) {
                Ok(precheck) => Some(precheck.requested_by),
                Err(e) => refuse(entry, Refusal::leg(*e)),
            }
        }
        None => None,
    };

    // ---- writes from here on ----

    let replaced = replacing.map(|old| {
        let result = replaced_result(backend, name, &old);
        (old.current_state.clone(), old.terminal.clone(), result)
    });
    // The old run is judged by the retention rule. Released, it goes as it
    // always did; kept, it and its subtree move aside (Decision 4).
    let mut moved: Option<(
        super::move_aside::Moved,
        String,
        serde_json::Map<String, serde_json::Value>,
    )> = None;
    // The pinned descendants a released old run's sweep kept.
    let mut kept_pinned: Vec<super::next_types::KeptChild> = Vec::new();
    if replaced.is_some() {
        let mut disposition = predecessor_disposition(backend, name);
        if let Disposition::Release = disposition {
            // A new run under this name must not inherit the old run's
            // kept children; they go with the session they belonged to,
            // released with it, except a child its own pin keeps. Unlike
            // the terminal tick, this sweeps without checking for a batch
            // hook: the template in scope here is the new one, not the one
            // the old session ran. If the removal below then fails, the
            // replace is refused with the old session in place and its kept
            // children already gone; they were terminal, so nothing live is
            // lost. A pin set on the old run since it was judged keeps it,
            // and it moves aside instead.
            kept_pinned = super::workspace::sweep_terminal_descendants(backend, backend, name);
            match crate::session::pin::remove_guarded(backend, name) {
                Ok(crate::session::pin::Removal::Removed) => {}
                Ok(crate::session::pin::Removal::Kept(guard)) => {
                    disposition = Disposition::pinned(&guard);
                }
                Err(e) => refuse(
                    entry,
                    Refusal::new(
                        Some("replace_failed"),
                        format!("could not remove finished session '{name}': {e}"),
                        1,
                    ),
                ),
            }
        }
        let (reason, kept_by, fields) = match disposition {
            Disposition::Release => (None, "", Default::default()),
            Disposition::Keep { reason } => (Some(reason), "by its hierarchy", Default::default()),
            Disposition::Pinned { reason, fields } => (Some(reason), "by its pin", fields),
            Disposition::Related { reason, fields } => {
                (Some(reason), "by the continues relation", fields)
            }
        };
        if let Some(reason) = reason {
            match super::move_aside::move_aside(backend, name) {
                Ok(m) => {
                    for (old, why) in &m.undeleted {
                        eprintln!(
                            "warning: '{old}' moved, but its old remote prefix still holds \
                             objects: {why}"
                        );
                    }
                    moved = Some((m, reason, fields));
                }
                Err(super::move_aside::MoveRefusal::HeldLive { live }) => refuse(
                    entry,
                    Refusal::new(
                        Some("replace_held_live"),
                        format!(
                            "session '{name}' is kept {kept_by} ({reason}), so it would \
                             move aside, but its subtree holds live work that can't move: {}; \
                             wait for it to finish, or cancel it, then replace",
                            live.join(", ")
                        ),
                        2,
                    )
                    .with_field("live", live),
                ),
                Err(super::move_aside::MoveRefusal::Failed { message, stranded }) => {
                    let mut r = Refusal::new(
                        Some("replace_failed"),
                        format!(
                            "could not move finished session '{name}' aside: {message}; \
                             nothing was replaced"
                        ),
                        1,
                    );
                    if !stranded.is_empty() {
                        r = r.with_field(
                            "stranded",
                            stranded
                                .iter()
                                .map(|(o, n)| serde_json::json!({"from": o, "to": n}))
                                .collect::<Vec<_>>(),
                        );
                    }
                    refuse(entry, r)
                }
            }
        }
    }

    let environment_report = match init_child::init_child_from_parent_at(
        backend,
        None,
        name,
        template_path,
        args.vars,
        cache,
        None,
        args.execution_dir,
        args.legacy_environment,
    ) {
        Ok(value) => value,
        Err(err) => {
            let r = match err.kind {
                SpawnErrorKind::Collision => {
                    let mut r =
                        Refusal::new(None, format!("workflow '{}' already exists", name), 1);
                    r.reason = "already-exists".to_string();
                    r
                }
                _ => match &err.var_error {
                    Some(var_error) => Refusal::var(var_error),
                    None => Refusal::new(Some(&spawn_kind_code(&err.kind)), err.message.clone(), 1),
                },
            };
            refuse(entry, r);
        }
    };

    if let Some(intent) = args.intent {
        if let Err(e) = crate::cli::session::handle_update(backend, name, intent) {
            eprintln!("warning: failed to record intent: {}", e);
        }
    } else {
        super::record_default_intent(backend, name);
    }

    let leg = entry
        .koto_leg
        .as_ref()
        .map(|target| match bind_created(backend, name, target) {
            Ok(written) => (target, written),
            Err(e) => {
                // The leg went to someone else between the check and the
                // bind. The session was made for this leg; don't leave it.
                let _ = backend.cleanup(name);
                refuse(entry, Refusal::leg(*e));
            }
        });

    // The links come last, into this session only: `--dispatched-by`, and a
    // leg-bound session's requester once the leg is its own.
    if let Some(target) = args.continues {
        crate::cli::session::record_continues_at_init(backend, name, target);
    }
    if let Some(dispatcher) = args.dispatched_by {
        crate::cli::session::record_dispatched_by(backend, name, dispatcher);
    }
    if let (Some(_), Some(requested_by)) = (&leg, &requested_by) {
        crate::cli::session::record_requester_link(backend, name, requested_by);
    }

    let (_, events) = backend
        .read_events(name)
        .map_err(|e| anyhow::anyhow!("failed to read newly initialized workflow: {}", e))?;
    let state = crate::engine::persistence::derive_state_from_log(&events).unwrap_or_default();

    let mut out = serde_json::json!({
        "name": name,
        "state": state,
        "outcome": if replaced.is_some() { "replaced" } else { "created" },
    });
    super::add_environment_report(&mut out, environment_report.as_ref());
    if let Some((old_state, terminal, result)) = replaced {
        out["replaced_state"] = terminal.unwrap_or(old_state).into();
        out["replaced_result"] = serde_json::to_value(result)?;
    }
    if let Some((m, reason, pin)) = moved {
        out["moved_to"] = m.moved_to.into();
        out["moved_reason"] = reason.into();
        // The moved run's pin, when its pin is what kept it, or the
        // relation's report, when a later session continuing it did.
        for (k, v) in pin {
            out[k] = v;
        }
        out["moved"] = m
            .renamed
            .iter()
            .map(|(o, n)| serde_json::json!({"from": o, "to": n}))
            .collect::<Vec<_>>()
            .into();
        if !m.undeleted.is_empty() {
            out["undeleted_remote_prefixes"] = m
                .undeleted
                .iter()
                .map(|(o, why)| serde_json::json!({"name": o, "error": why}))
                .collect::<Vec<_>>()
                .into();
        }
    }
    if !kept_pinned.is_empty() {
        out["kept_children"] = serde_json::to_value(&kept_pinned)?;
    }
    if let Some((target, written)) = leg {
        out["leg"] = leg_json(target, written);
    }
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// Bind a just-created session to its leg, reading its facts back from
/// disk so the bind event records exactly what was written.
fn bind_created(
    backend: &Backend,
    name: &str,
    target: &InitLegTarget,
) -> std::result::Result<bool, Box<InitLegRefusal>> {
    let session_dir = backend.session_dir(name);
    let header = backend.read_header(name).map_err(|e| {
        InitLegRefusal::persistence(format!("could not read session '{name}': {e}"))
    })?;
    let facts = request::attaching_session(name, &session_dir, &header)
        .map_err(InitLegRefusal::from_request_error)?;
    request::init_attach_leg(target, facts, &session_dir)
}

/// Move a session the local store started into the caller's cloud store:
/// upload the whole local copy, then record the caller's origin, so the
/// adoption happens once and later attaches compare cloud to cloud. The
/// origin is rewritten only after the bucket holds the session; a failed
/// upload refuses and leaves the record local.
fn adopt_into_cloud(
    backend: &Backend,
    entry: &EntryFlags,
    name: &str,
    caller: Option<SessionOrigin>,
) {
    let fail = |e: anyhow::Error| -> ! {
        refuse(
            entry,
            Refusal::new(
                Some("persistence_error"),
                format!(
                    "session '{name}' was started by the local store and couldn't be moved \
                     into the cloud store: {e:#}"
                ),
                3,
            ),
        )
    };
    let Backend::Cloud(cloud) = backend else {
        unreachable!("only a cloud caller adopts a local session")
    };
    if let Err(e) = cloud.adopt_local_copy(name) {
        fail(e);
    }
    if let Err(e) = backend.rewrite_header(name, &|mut h| {
        h.origin = caller.clone();
        h
    }) {
        fail(e);
    }
}

/// Attach to a live session: template, origin and fixed variables
/// checked, then the leg bound, then the rebind variables re-applied.
fn attach(
    backend: &Backend,
    args: &InitArgs<'_>,
    entry: &EntryFlags,
    compiled: &CompiledTemplate,
    hash: &str,
    pairs: &[(String, String)],
    existing: Existing,
) -> Result<()> {
    let name = args.name;
    let session_dir = backend.session_dir(name);

    // A leg pointer is read only when a leg is being attached; plain
    // --attach-live never touches it.
    let pointer = match &entry.koto_leg {
        Some(_) => match crate::engine::leg_pointer::read_pointer(&session_dir) {
            Ok(p) => p,
            Err(e) => refuse(
                entry,
                Refusal::new(
                    Some("persistence_error"),
                    format!("could not read session '{name}'s leg pointer: {e}"),
                    3,
                ),
            ),
        },
        None => None,
    };
    let facts = AttachingSession::from_session(
        name,
        &existing.header,
        &existing.events,
        &existing.compiled,
        pointer,
    );

    // Template identity, by the rule a request leg uses: the source file
    // name, compared exactly.
    let mine = caller_identity(Path::new(args.template), compiled, hash);
    if !LegTemplates::One(mine.source.clone()).admits(facts.template.as_ref()) {
        let theirs = facts
            .template
            .as_ref()
            .map(|t| t.source.clone())
            .unwrap_or_default();
        refuse(
            entry,
            Refusal::new(
                Some("template_mismatch"),
                format!(
                    "session '{name}' was built from {}, not {}; it can't be attached from \
                     another template",
                    if theirs.is_empty() {
                        "no template file".to_string()
                    } else {
                        format!("template {theirs}")
                    },
                    mine.source
                ),
                2,
            )
            .with_field("recorded", theirs.as_str())
            .with_field("requested", mine.source.as_str())
            .with_record(theirs.clone(), mine.source.clone()),
        );
    }

    let caller = caller_origin(backend, args.execution_dir);
    let origin = match check_origin(name, existing.header.origin.as_ref(), caller.as_ref()) {
        Ok(found) => found,
        Err(r) => refuse(entry, *r),
    };

    // Drift in the fixed environment values is reported, never refused
    // (DESIGN-koto-fixed-environment.md, R14): attach writes no record, and
    // the session's commands run with the recorded values whoever ticks it.
    // Names only, never values. A session with no record isn't compared; its
    // next tick adopts one. A legacy session isn't compared either: its
    // commands do run with the caller's environment, so there's nothing to
    // warn about.
    let environment_drift = existing
        .header
        .command_environment
        .as_ref()
        .filter(|record| !record.legacy)
        .map(|record| crate::engine::command_env::drift(record, |n| std::env::var(n).ok()))
        .unwrap_or_default();

    let plan = match validate_rebind(
        &existing.compiled,
        &facts.bindings,
        &existing.current_state,
        pairs,
    ) {
        Ok(p) => p,
        Err(e) => refuse(entry, Refusal::var(&e)),
    };

    let requested_by = match &entry.koto_leg {
        Some(target) => match request::init_precheck_leg(target, &facts) {
            Ok(precheck) => Some(precheck.requested_by),
            Err(e) => refuse(entry, Refusal::leg(*e)),
        },
        None => None,
    };

    // ---- writes from here on: adoption, leg bind, then rebind, then links ----

    if origin == OriginMatch::LocalIntoCloud {
        adopt_into_cloud(backend, entry, name, caller);
    }

    let leg = match &entry.koto_leg {
        Some(target) => match request::init_attach_leg(target, facts, &session_dir) {
            Ok(written) => Some((target, written)),
            // Lost a race after the checks: nothing was rebound.
            Err(e) => refuse(entry, Refusal::leg(*e)),
        },
        None => None,
    };

    if let Err(e) = crate::engine::variables::apply_rebind(backend, name, &plan) {
        super::exit_with_error_code(
            serde_json::json!({
                "error": format!("could not re-apply rebind variables on '{name}': {e}"),
                "command": "init",
                "code": "persistence_error",
            }),
            3,
        );
    }

    if let Some(target) = args.continues {
        crate::cli::session::record_continues_at_init(backend, name, target);
    }
    if let Some(dispatcher) = args.dispatched_by {
        crate::cli::session::record_dispatched_by(backend, name, dispatcher);
    }
    if let (Some(_), Some(requested_by)) = (&leg, &requested_by) {
        crate::cli::session::record_requester_link(backend, name, requested_by);
    }

    let mut out = serde_json::json!({
        "name": name,
        "state": existing.current_state,
        "outcome": "attached",
        "rebound": plan.changes,
    });
    if !environment_drift.is_empty() {
        eprintln!(
            "warning: session '{}' recorded a different {} than this shell has; its commands \
             run with the recorded values, not this shell's",
            name,
            environment_drift.join(", ")
        );
        out["environment_drift"] = serde_json::json!(environment_drift);
    }
    if let Some((target, written)) = leg {
        out["leg"] = leg_json(target, written);
    }
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

fn leg_json(target: &InitLegTarget, written: bool) -> serde_json::Value {
    serde_json::json!({
        "request_id": target.id.as_str(),
        "leg": target.leg,
        "written": written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vars_file_keeps_repeated_keys_in_order() {
        let entries = parse_vars_json(br#"[["A","1"],["B","x y"],["A","2"]]"#).unwrap();
        assert_eq!(entries, vec!["A=1", "B=x y", "A=2"]);
    }

    #[test]
    fn a_value_may_carry_an_equals_sign() {
        let entries = parse_vars_json(br#"[["A","k=v"]]"#).unwrap();
        assert_eq!(entries, vec!["A=k=v"]);
    }

    #[test]
    fn a_vars_file_of_the_wrong_shape_is_refused() {
        for body in [
            &b"not json"[..],
            br#"{"A":"1"}"#,
            br#"[["A"]]"#,
            br#"[["A","1","2"]]"#,
            br#"[["A",1]]"#,
            br#"[["","1"]]"#,
            br#"[["A=B","1"]]"#,
            br#"["A=1"]"#,
        ] {
            assert!(
                parse_vars_json(body).is_err(),
                "{:?} should be refused",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn an_empty_list_is_no_variables() {
        assert!(parse_vars_json(b"[]").unwrap().is_empty());
    }

    #[test]
    fn refusal_reasons_are_kebab_codes_with_the_variable() {
        let r = Refusal::var(&VarError::Invalid {
            var: "INTENT_FLAG".into(),
            value: "maybe".into(),
            constraint: "values:[continue, stop]".into(),
        });
        assert_eq!(r.reason, "invalid-var:INTENT_FLAG");
        assert_eq!(r.requested, "maybe");
        let r = Refusal::var(&VarError::Mismatch {
            var: "INTENT_FLAG".into(),
            recorded: "stop".into(),
            requested: "continue".into(),
        });
        assert_eq!(r.reason, "var-mismatch:INTENT_FLAG");
        assert_eq!(
            (r.recorded.as_str(), r.requested.as_str()),
            ("stop", "continue")
        );
        let r = Refusal::new(Some("template_mismatch"), "x", 2);
        assert_eq!(r.reason, "template-mismatch");
    }

    #[test]
    fn a_missing_origin_record_is_named_as_such() {
        let caller = SessionOrigin {
            anchor: "/w".into(),
            store: crate::engine::types::SessionStoreIdentity {
                kind: "local".into(),
                base: "/s".into(),
                bucket: None,
                prefix: None,
            },
            anchor_epoch: 0,
            retired_anchors: Vec::new(),
        };
        let r = check_origin("s", None, Some(&caller)).err().unwrap();
        let msg = r.body["error"].as_str().unwrap();
        assert!(msg.contains("no origin record"), "{msg}");
        assert!(msg.contains("koto version that started it"), "{msg}");
        assert!(msg.contains("another session name"), "{msg}");
        assert_names_no_removal(msg);
        assert_eq!(r.reason, "origin-mismatch");

        let mut other = caller.clone();
        other.anchor = "/elsewhere".into();
        let r = check_origin("s", Some(&other), Some(&caller))
            .err()
            .unwrap();
        let msg = r.body["error"].as_str().unwrap();
        assert!(!msg.contains("no origin record"), "{msg}");
        assert!(msg.contains("another session name"), "{msg}");
        assert!(
            msg.contains("run the command from /elsewhere, where the session belongs"),
            "{msg}"
        );
        assert_names_no_removal(msg);

        // Only the store differs: the anchor is the caller's own, so the
        // message names the store instead of a directory to move to.
        let mut other = caller.clone();
        other.store.base = "/other-sessions".into();
        let r = check_origin("s", Some(&other), Some(&caller))
            .err()
            .unwrap();
        let msg = r.body["error"].as_str().unwrap();
        assert!(msg.contains("/other-sessions"), "{msg}");
        assert!(msg.contains("the session store it was started in"), "{msg}");
        assert_names_no_removal(msg);
        assert_eq!(
            check_origin("s", Some(&caller), Some(&caller)).ok(),
            Some(OriginMatch::Same)
        );
    }

    /// Neither `origin_mismatch` message names a removal as the way forward.
    fn assert_names_no_removal(msg: &str) {
        for removal in [
            "session cleanup",
            "--cleanup",
            "session prune",
            "workspace prune",
        ] {
            assert!(!msg.contains(removal), "names {removal}: {msg}");
        }
    }

    fn origin(kind: &str, anchor: &str, base: &str, bucket: Option<&str>) -> SessionOrigin {
        SessionOrigin {
            anchor: anchor.into(),
            store: crate::engine::types::SessionStoreIdentity {
                kind: kind.into(),
                base: base.into(),
                bucket: bucket.map(str::to_string),
                prefix: None,
            },
            anchor_epoch: 0,
            retired_anchors: Vec::new(),
        }
    }

    #[test]
    fn a_local_session_is_adopted_by_a_cloud_caller_over_the_same_directory() {
        let local = origin("local", "/w", "/s", None);
        let cloud = origin("cloud", "/w", "/s", Some("b"));
        assert_eq!(
            origin_match(&local, &cloud),
            Some(OriginMatch::LocalIntoCloud)
        );
        // Another anchor or another sessions directory is another session.
        assert_eq!(
            origin_match(&local, &origin("cloud", "/x", "/s", Some("b"))),
            None
        );
        assert_eq!(
            origin_match(&local, &origin("cloud", "/w", "/t", Some("b"))),
            None
        );
        // Never the other way: a cloud session's newest copy may be only in
        // the bucket.
        assert_eq!(origin_match(&cloud, &local), None);
    }

    #[test]
    fn a_cloud_session_matches_only_its_own_bucket() {
        let recorded = origin("cloud", "/w", "/s", Some("b"));
        assert_eq!(
            origin_match(&recorded, &origin("cloud", "/w", "/s", Some("b"))),
            Some(OriginMatch::Same)
        );
        assert_eq!(
            origin_match(&recorded, &origin("cloud", "/w", "/s", Some("other"))),
            None
        );
        // Recorded before koto named the bucket: kind and base decide.
        assert_eq!(
            origin_match(
                &origin("cloud", "/w", "/s", None),
                &origin("cloud", "/w", "/s", Some("other"))
            ),
            Some(OriginMatch::Same)
        );
    }

    #[test]
    fn a_bucket_is_named_in_the_mismatch() {
        let recorded = origin("cloud", "/w", "/s", Some("b"));
        let caller = origin("cloud", "/w", "/s", Some("other"));
        let r = check_origin("s", Some(&recorded), Some(&caller))
            .err()
            .unwrap();
        let msg = r.body["error"].as_str().unwrap();
        assert!(msg.contains("bucket b"), "{msg}");
        assert!(msg.contains("bucket other"), "{msg}");
    }
}
