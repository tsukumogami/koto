//! Child-workflow init helper used by the batch scheduler (Issue #12).
//!
//! Issue #3 extracts [`init_child_from_parent`] from `handle_init` so
//! the future batch scheduler can spawn children one-at-a-time and
//! surface per-task failures through `TaskSpawnError` instead of
//! halting the whole tick on the first bad entry.
//!
//! The helper is intentionally I/O-sequential and cheap — a single
//! scheduler tick will call it once per task, reusing a
//! [`TemplateCompileCache`] so repeated entries that all point at the
//! same `default_template` only compile once.
//!
//! # Design notes
//!
//! - **Atomic write.** The helper goes through
//!   [`SessionBackend::init_state_file`] exclusively — there is no
//!   `create` + `append_header` + `append_event` sequence. A
//!   `SessionError::Collision` from the backend becomes
//!   [`SpawnErrorKind::Collision`], preserving the race-winner
//!   semantics `handle_init` already relies on.
//!
//! - **Per-tick compile cache.** The caller passes a
//!   `&mut TemplateCompileCache`. The same template path is compiled
//!   once per tick and reused for every task that points at it. Callers
//!   that don't already hold a cache (direct CLI init, tests) allocate
//!   a throwaway one at the call site; the helper itself never
//!   allocates a cache.
//!
//! - **Per-template `resolve_variables`.** Each child template may
//!   declare a different set of variables from its parent. The helper
//!   re-runs [`resolve_variables`](crate::cli::resolve_variables) with
//!   the *child* template's declarations — crucially not the parent's
//!   — matching the design doc's insistence that `--var` bindings are
//!   template-scoped, not workflow-scoped.
//!
//! - **Typed error envelope.** Every failure returns
//!   [`TaskSpawnError`]. `handle_init` wraps the result and continues
//!   to map failures onto `exit_with_error`, so the public CLI
//!   behavior is unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::cache::compile_cached;
use crate::cli::task_spawn_error::{SpawnErrorKind, TaskSpawnError};
use crate::engine::types::{
    generate_session_id, now_iso8601, Event, EventPayload, SpawnEntrySnapshot, StateFileHeader,
};
use crate::session::{SessionBackend, SessionError};
use crate::template::types::CompiledTemplate;

/// Prefix applied to `TaskSpawnError.message` when the underlying
/// failure is a `--var` resolution error (unknown key, missing
/// required, malformed `KEY=VALUE`, etc.).
///
/// `handle_init` uses this prefix to map the error back onto the
/// CLI's existing exit code 2 (caller-error) rather than the generic
/// exit code 1 used for compile / I/O / collision failures. Keeping
/// the prefix as a named constant ensures producer and consumer stay
/// in sync — a test below asserts the prefix appears on the error
/// message when variable resolution fails.
pub(crate) const VAR_RESOLUTION_MSG_PREFIX: &str = "variable resolution failed: ";

/// Per-tick cache keyed by the *canonical* template source path.
///
/// Two tasks pointing at the same `default_template` would otherwise
/// recompile the template once each. One scheduler tick shares a
/// single cache across all child-spawn calls so the template parse
/// happens once per unique path.
///
/// The cache is intentionally simple: a scheduler tick is short-lived,
/// so there is no invalidation path. The key is the canonicalized
/// absolute path of the template source file; two relative paths that
/// resolve to the same file share a cache slot.
#[derive(Debug, Default)]
pub struct TemplateCompileCache {
    entries: HashMap<PathBuf, CachedTemplate>,
}

#[derive(Debug, Clone)]
struct CachedTemplate {
    compiled: CompiledTemplate,
    cache_path: PathBuf,
    hash: String,
    /// Canonical *source* template path (not the cache artifact).
    /// Retained so post-compile error paths (e.g. variable resolution)
    /// can forward it into `TaskSpawnError.path` for parity with
    /// `BatchError::TemplateCompileFailed.path`.
    source_path: PathBuf,
}

impl TemplateCompileCache {
    /// Construct an empty cache. Typically held by the scheduler for
    /// the duration of one `koto next` tick.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Return the number of distinct templates in the cache. Public
    /// primarily so tests can assert "the second spawn call didn't
    /// trigger another compile".
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache contains any compiled template.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Task-agnostic compile-failure detail.
///
/// `compile_with_cache` runs before the caller knows which task this
/// failure belongs to (in the scheduler case the caller decides as it
/// iterates entries; in the root-init case there is no task). Returning
/// this struct lets the caller construct the full [`TaskSpawnError`]
/// explicitly with the right `task`, `paths_tried`, `template_source`,
/// and (future) `compile_error` fields filled in — without smuggling a
/// sentinel `task = ""` through a field-spread `..e` update.
///
/// Keep this in sync with [`TaskSpawnError`]'s non-`task` fields if new
/// compile-context fields land.
#[derive(Debug)]
pub(crate) struct CompileErrorInfo {
    /// Discriminator for how to classify the eventual `TaskSpawnError`.
    pub kind: SpawnErrorKind,
    /// Human-readable message describing what went wrong.
    pub message: String,
    /// Source template path the caller asked about. Callers that know
    /// the resolved / canonicalized form may prefer to log that
    /// separately; this is the *input* path so the scheduler can plumb
    /// it into `paths_tried` unchanged.
    #[allow(dead_code)]
    pub path: PathBuf,
    /// Resolved (canonicalized) template path, when path resolution
    /// succeeded. `None` means the input path never resolved — for
    /// example `TemplateNotFound` (the file did not exist) or a
    /// canonicalize failure on `PermissionDenied` / generic I/O. The
    /// caller forwards this verbatim into [`TaskSpawnError::path`] so
    /// the JSON surface mirrors `BatchError::TemplateCompileFailed.path`.
    pub resolved_path: Option<PathBuf>,
}

/// Compile `template_path` once per cache, returning the cached
/// [`CompiledTemplate`], its on-disk cache path (used as the stored
/// `template_path` on the `WorkflowInitialized` event), and its hash
/// (written into the state-file header).
///
/// A hit returns the previously compiled bundle verbatim. A miss
/// canonicalizes the source path, compiles via [`compile_cached`], and
/// records the result before returning.
///
/// Errors are returned as [`CompileErrorInfo`] — a *partial* envelope
/// with no `task` field — so the caller can construct the final
/// [`TaskSpawnError`] with the right task name (or whatever root-init
/// placeholder it prefers). Previous revisions used a sentinel
/// `TaskSpawnError { task: "" }` plus `..e` field-spread to rewrite
/// `task` at the call site; that pattern would silently drop any new
/// field future issues (#5 / #8) add to [`TaskSpawnError`], so it's
/// been replaced with this explicit construction.
fn compile_with_cache(
    template_path: &Path,
    cache: &mut TemplateCompileCache,
) -> Result<CachedTemplate, CompileErrorInfo> {
    // Canonicalize *before* looking up so two relative paths that point
    // at the same file share a cache slot. `canonicalize` also fails
    // with `NotFound` when the source doesn't exist, which is exactly
    // the signal `TemplateNotFound` encodes.
    let canonical = match std::fs::canonicalize(template_path) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CompileErrorInfo {
                kind: SpawnErrorKind::TemplateNotFound,
                message: format!("template not found: {} ({})", template_path.display(), e),
                path: template_path.to_path_buf(),
                // Resolution failed — the file didn't exist. Leave the
                // resolved path `None` so agents can distinguish "never
                // resolved" from "resolved then compile-failed".
                resolved_path: None,
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(CompileErrorInfo {
                kind: SpawnErrorKind::PermissionDenied,
                message: format!(
                    "permission denied reading template {}: {}",
                    template_path.display(),
                    e
                ),
                path: template_path.to_path_buf(),
                resolved_path: None,
            });
        }
        Err(e) => {
            return Err(CompileErrorInfo {
                kind: SpawnErrorKind::IoError,
                message: format!(
                    "failed to access template {}: {}",
                    template_path.display(),
                    e
                ),
                path: template_path.to_path_buf(),
                resolved_path: None,
            });
        }
    };

    if let Some(hit) = cache.entries.get(&canonical) {
        return Ok(hit.clone());
    }

    let (cache_path, hash) = compile_cached(&canonical, false).map_err(|e| CompileErrorInfo {
        kind: SpawnErrorKind::TemplateCompileFailed,
        message: format!("failed to compile template {}: {}", canonical.display(), e),
        path: canonical.clone(),
        // Source template resolved successfully; carry the canonical
        // source path (not the cache-artifact path) so downstream
        // consumers see the same shape `BatchError::TemplateCompileFailed`
        // exposes.
        resolved_path: Some(canonical.clone()),
    })?;

    let content = std::fs::read_to_string(&cache_path).map_err(|e| CompileErrorInfo {
        kind: SpawnErrorKind::IoError,
        message: format!(
            "failed to read cached template {}: {}",
            cache_path.display(),
            e
        ),
        path: cache_path.clone(),
        resolved_path: Some(canonical.clone()),
    })?;
    let compiled: CompiledTemplate =
        serde_json::from_str(&content).map_err(|e| CompileErrorInfo {
            kind: SpawnErrorKind::TemplateCompileFailed,
            message: format!(
                "failed to parse cached template {}: {}",
                cache_path.display(),
                e
            ),
            path: cache_path.clone(),
            resolved_path: Some(canonical.clone()),
        })?;

    let entry = CachedTemplate {
        compiled,
        cache_path,
        hash,
        source_path: canonical.clone(),
    };
    cache.entries.insert(canonical, entry.clone());
    Ok(entry)
}

/// Map a [`SessionError`] raised by `init_state_file` onto the
/// appropriate [`SpawnErrorKind`].
///
/// `Collision` is the hot path — it's how the atomic rename reports
/// "the child already exists on disk". `Io(PermissionDenied)` is
/// pulled out as a separate kind so operators can tell a directory
/// permission problem apart from a generic I/O failure, matching the
/// design's Decision 12 kind list.
fn classify_session_error(task: &str, err: SessionError) -> TaskSpawnError {
    match err {
        SessionError::Collision => TaskSpawnError::new(
            task,
            SpawnErrorKind::Collision,
            format!(
                "child workflow already exists: state file collision for {:?}",
                task
            ),
        ),
        SessionError::Locked { .. } => TaskSpawnError::new(
            task,
            SpawnErrorKind::BackendUnavailable,
            format!("state file locked for {:?}: {}", task, err),
        ),
        SessionError::Io(io_err) => {
            let kind = match io_err.kind() {
                std::io::ErrorKind::NotFound => SpawnErrorKind::IoError,
                std::io::ErrorKind::PermissionDenied => SpawnErrorKind::PermissionDenied,
                _ => SpawnErrorKind::IoError,
            };
            TaskSpawnError::new(task, kind, format!("I/O error for {:?}: {}", task, io_err))
        }
        SessionError::Other(e) => TaskSpawnError::new(
            task,
            SpawnErrorKind::BackendUnavailable,
            format!("backend error for {:?}: {}", task, e),
        ),
    }
}

/// Initialize a workflow on disk, optionally linked to a parent.
///
/// `parent_name` is `Some(parent)` for a child spawn (the batch
/// scheduler's hot path), threading `parent_workflow` through to the
/// state-file header. When `None`, this is a root (top-level) init:
/// the header's `parent_workflow` is `None` and the caller has
/// typically arrived through `handle_init`'s CLI path.
///
/// `child_name` is the full composed workflow name the caller wants on
/// disk (e.g., `parent.issue-1` when `parent_name` is `Some`, or just
/// `root-name` when it is `None`). The helper does not prepend the
/// parent name itself — the scheduler composes that upstream so the
/// composition rules live in one place.
///
/// `template_path` is the source template (markdown with YAML
/// frontmatter). The helper canonicalizes it, runs it through the
/// supplied [`TemplateCompileCache`], and resolves `vars` against the
/// child template's variable declarations.
///
/// `spawn_entry` is the canonical-form batch task entry (Decision 10 /
/// 2 amendment). Callers that spawn a child on behalf of a batch
/// scheduler pass `Some(..)` so later ticks can R8-compare against the
/// recorded entry. The top-level `koto init` path and direct CLI
/// callers pass `None` — no batch exists to compare against.
///
/// On success a single atomic `init_state_file` call commits the
/// header plus the `WorkflowInitialized` and initial `Transitioned`
/// events. Every failure path returns a [`TaskSpawnError`] whose
/// `task` field is `child_name`, so the scheduler can fan the error
/// straight into `SchedulerOutcome::Scheduled.errored` without extra
/// bookkeeping.
//
// clippy::result_large_err: `TaskSpawnError` is intentionally rich — its
// shape is fixed by the design doc's Key Interfaces section so the
// future batch scheduler can emit `paths_tried` (#5), `template_source`
// (#5), and a typed `compile_error` (#8) on the same envelope. Current
// size is roughly 6 * usize + 1 enum tag (~56 bytes today, ~120 bytes
// once the Option fields are populated). Boxing it would force the
// scheduler to unwrap `Box` every time it accumulates an error into
// `SchedulerOutcome::Scheduled.errored`; we accept the warning here
// rather than pushing that cost onto every caller.
#[allow(clippy::result_large_err)]
pub fn init_child_from_parent(
    backend: &dyn SessionBackend,
    parent_name: Option<&str>,
    child_name: &str,
    template_path: &Path,
    vars: &[String],
    cache: &mut TemplateCompileCache,
    spawn_entry: Option<SpawnEntrySnapshot>,
) -> Result<(), TaskSpawnError> {
    init_child_core(
        backend,
        parent_name,
        child_name,
        template_path,
        vars,
        cache,
        spawn_entry,
        None,
    )
}

/// Spawn a child directly into its terminal `skipped_marker: true`
/// state. Used by the batch scheduler when a task's upstream dependency
/// has failed and the batch `failure_policy` is `skip_dependents`
/// (Decision 9 Part 5 — see DESIGN-batch-child-spawning.md:3615-3620).
///
/// The child's real template is still compiled (so `spawn_entry` values
/// bind to the same variable space), but the initial `Transitioned`
/// event routes straight to `skipped_state_name` rather than the
/// template's `initial_state`. F5 (compile rule) guarantees every
/// batch-eligible child template declares a reachable
/// `skipped_marker: true` state; callers pass the skip state name they
/// discovered when deciding to skip.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)]
pub fn init_child_as_skip_marker_from_parent(
    backend: &dyn SessionBackend,
    parent_name: Option<&str>,
    child_name: &str,
    template_path: &Path,
    vars: &[String],
    cache: &mut TemplateCompileCache,
    spawn_entry: Option<SpawnEntrySnapshot>,
    skipped_state_name: &str,
) -> Result<(), TaskSpawnError> {
    init_child_core(
        backend,
        parent_name,
        child_name,
        template_path,
        vars,
        cache,
        spawn_entry,
        Some(skipped_state_name),
    )
}

/// Shared implementation. When `override_initial_state` is `Some`, the
/// first `Transitioned` event routes to that state instead of the
/// template's `initial_state`; this is the skip-marker spawn path.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)]
fn init_child_core(
    backend: &dyn SessionBackend,
    parent_name: Option<&str>,
    child_name: &str,
    template_path: &Path,
    vars: &[String],
    cache: &mut TemplateCompileCache,
    spawn_entry: Option<SpawnEntrySnapshot>,
    override_initial_state: Option<&str>,
) -> Result<(), TaskSpawnError> {
    let cached = compile_with_cache(template_path, cache).map_err(|info| {
        let mut err = TaskSpawnError::new(child_name, info.kind, info.message);
        // Forward the resolved template path when resolution
        // succeeded. Advisory #10: keep parity with
        // `BatchError::TemplateCompileFailed.path` so agents rendering
        // per-task errors see the same shape regardless of envelope.
        if let Some(resolved) = info.resolved_path {
            err = err.with_path(resolved);
        }
        err
        // NOTE: when Issue #5 / #8 land, this is where a richer
        // TaskSpawnError (paths_tried, template_source, compile_error)
        // would be composed from `info.path` plus caller-side context.
    })?;

    let variables =
        crate::cli::resolve_variables(vars, &cached.compiled.variables).map_err(|msg| {
            // Variable resolution runs *after* a successful compile, so
            // the cache entry carries a valid resolved path. Plumb it
            // through to keep parity with other post-resolution failure
            // paths.
            TaskSpawnError::new(
                child_name,
                SpawnErrorKind::TemplateCompileFailed,
                format!("{}{}", VAR_RESOLUTION_MSG_PREFIX, msg),
            )
            .with_path(cached.source_path.clone())
        })?;

    let initial_state = match override_initial_state {
        Some(s) => s.to_string(),
        None => cached.compiled.initial_state.clone(),
    };
    let ts = now_iso8601();
    let cache_path_str = cached.cache_path.to_string_lossy().to_string();

    // Capture the source template's parent directory so the batch
    // scheduler's path resolver (Decision 4 / 14 in
    // DESIGN-batch-child-spawning.md) can use it as the base for
    // relative child-template paths. Only meaningful when the source
    // path is absolute and its parent directory exists; relative
    // paths or odd shapes (stdin / inline) leave the field `None`,
    // and the resolver emits `MissingTemplateSourceDir` in that case.
    let template_source_dir = if template_path.is_absolute() {
        template_path.parent().map(|p| p.to_path_buf())
    } else {
        // Best-effort: resolve relative paths against the current
        // working directory before snapshotting the parent. The
        // cache path canonicalization above already proved the file
        // exists; any failure here is non-fatal — we leave the
        // header field `None` and let the resolver fall back.
        std::fs::canonicalize(template_path)
            .ok()
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
    };

    let header = StateFileHeader {
        schema_version: 1,
        workflow: child_name.to_string(),
        template_hash: cached.hash.clone(),
        created_at: ts.clone(),
        parent_workflow: parent_name.map(|s| s.to_string()),
        template_source_dir,
        session_id: generate_session_id(),
        intent: None,
        template_name: if cached.compiled.name.is_empty() {
            None
        } else {
            Some(cached.compiled.name.clone())
        },
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    };

    let init_payload = EventPayload::WorkflowInitialized {
        template_path: cache_path_str,
        variables,
        spawn_entry,
    };
    let transition_payload = EventPayload::Transitioned {
        from: None,
        to: initial_state.clone(),
        condition_type: "auto".to_string(),
        skip_if_matched: None,
    };
    let initial_events = vec![
        Event {
            seq: 1,
            timestamp: ts.clone(),
            event_type: init_payload.type_name().to_string(),
            payload: init_payload,
            idempotency_hash: None,
        },
        Event {
            seq: 2,
            timestamp: ts.clone(),
            event_type: transition_payload.type_name().to_string(),
            payload: transition_payload,
            idempotency_hash: None,
        },
    ];

    // Create the session directory before init_state_file: the atomic
    // rename inside init_state_file needs a parent directory to rename
    // *into*. `create` is a no-op if the directory already exists.
    backend.create(child_name).map_err(|e| {
        TaskSpawnError::new(
            child_name,
            SpawnErrorKind::IoError,
            format!(
                "failed to create session directory for {:?}: {}",
                child_name, e
            ),
        )
        .with_path(cached.source_path.clone())
    })?;

    backend
        .init_state_file(child_name, header, initial_events)
        .map_err(|e| classify_session_error(child_name, e).with_path(cached.source_path.clone()))?;

    Ok(())
}

/// Fixed filename for the human-readable source persisted by the inline
/// (`koto init --from-stdin`) path. The original extension is metadata,
/// never a path component — a recorded extension is never concatenated
/// into a filesystem path, so a hostile `<name>` or extension cannot
/// encode traversal (see the design's Security Considerations). Issue 1
/// wires up the compiled-artifact storage and session-relative resolution;
/// the source-write (`koto init --from-stdin`) lands in Issue 2 and reuses
/// this constant.
#[allow(dead_code)]
pub(crate) const INLINE_SOURCE_FILENAME: &str = "source";

/// Initialize a top-level workflow whose compiled artifact is stored
/// **inside the session directory** and recorded as a **session-relative**
/// `template_path`.
///
/// This is the engine seam for the inline (`koto init --from-stdin`) path.
/// Unlike [`init_child_from_parent`], which records the absolute
/// `~/.cache/koto/<sha>.json` cache path, this helper:
///
/// 1. creates the session directory (so `ensure_koto_root` sets 0700),
/// 2. compiles `source_path` *into* the session directory via
///    [`compile_cached_into`] in **strict** mode, and
/// 3. records only the artifact's filename (`<sha256>.json`) as the
///    `WorkflowInitialized.template_path`.
///
/// Because the recorded path is relative, [`derive_machine_state`] resolves
/// it against the session directory at read time, so the compiled artifact
/// survives `~/.cache` eviction and survives session `relocate` (which
/// renames the whole directory but does not rewrite the event log). The
/// content-addressed `template_hash` is identical to what the cache path
/// would produce — only the recorded path string differs.
///
/// `source_path` must already exist on disk; the stdin→source-file write is
/// the CLI surface added in Issue 2. The persisted human-readable source
/// (under [`INLINE_SOURCE_FILENAME`]) is likewise Issue 2's concern.
///
/// [`derive_machine_state`]: crate::engine::persistence::derive_machine_state
/// [`compile_cached_into`]: crate::cache::compile_cached_into
pub fn init_inline_into_session(
    backend: &dyn SessionBackend,
    name: &str,
    source_path: &Path,
    vars: &[String],
) -> anyhow::Result<()> {
    // Create the session directory first: compile_cached_into would
    // create it too, but going through the backend also applies the
    // 0700 koto-root permissions and the session-id validation that
    // guards `<name>` against traversal.
    let session_dir = backend.create(name)?;

    // Compile strictly into the session directory. The artifact lands at
    // <session_dir>/<sha256>.json and the hash is content-addressed over
    // the compiled JSON bytes (identical to the global-cache path).
    let (artifact_path, hash) = crate::cache::compile_cached_into(source_path, &session_dir, true)?;

    // Read the freshly written artifact back so we can resolve variables
    // against the compiled template's declarations.
    let content = std::fs::read_to_string(&artifact_path).with_context(|| {
        format!(
            "failed to read compiled artifact {}",
            artifact_path.display()
        )
    })?;
    let compiled: CompiledTemplate = serde_json::from_str(&content).with_context(|| {
        format!(
            "failed to parse compiled artifact {}",
            artifact_path.display()
        )
    })?;

    let variables = crate::cli::resolve_variables(vars, &compiled.variables)
        .map_err(|msg| anyhow::anyhow!("{}{}", VAR_RESOLUTION_MSG_PREFIX, msg))?;

    // Record the artifact as a SESSION-RELATIVE path: just the filename.
    // derive_machine_state resolves it against the session dir at read
    // time. file_name() is safe — compile_cached_into always names the
    // artifact `<sha256>.json` directly under `session_dir`.
    let relative_template_path = artifact_path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "compiled artifact path has no file name: {}",
                artifact_path.display()
            )
        })?;

    let ts = now_iso8601();
    let initial_state = compiled.initial_state.clone();

    let header = StateFileHeader {
        schema_version: 1,
        workflow: name.to_string(),
        template_hash: hash,
        created_at: ts.clone(),
        parent_workflow: None,
        // The compiled artifact lives in the session dir, not a source
        // tree, so there is no parent source dir for the batch resolver.
        template_source_dir: None,
        session_id: generate_session_id(),
        intent: None,
        template_name: if compiled.name.is_empty() {
            None
        } else {
            Some(compiled.name.clone())
        },
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    };

    let init_payload = EventPayload::WorkflowInitialized {
        template_path: relative_template_path,
        variables,
        spawn_entry: None,
    };
    let transition_payload = EventPayload::Transitioned {
        from: None,
        to: initial_state.clone(),
        condition_type: "auto".to_string(),
        skip_if_matched: None,
    };
    let initial_events = vec![
        Event {
            seq: 1,
            timestamp: ts.clone(),
            event_type: init_payload.type_name().to_string(),
            payload: init_payload,
            idempotency_hash: None,
        },
        Event {
            seq: 2,
            timestamp: ts.clone(),
            event_type: transition_payload.type_name().to_string(),
            payload: transition_payload,
            idempotency_hash: None,
        },
    ];

    backend
        .init_state_file(name, header, initial_events)
        .map_err(|e| anyhow::anyhow!("failed to initialize inline session {:?}: {}", name, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::sha256_hex;
    use crate::session::local::LocalBackend;
    use std::io::Write;
    use tempfile::TempDir;

    const SIMPLE_TEMPLATE: &str = r#"---
name: child-template
version: "1.0"
initial_state: only
variables:
  TASK_ID:
    required: true
states:
  only:
    accepts:
      marker:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## only

Do the thing.

## done

Done.
"#;

    fn write_template(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create template");
        f.write_all(body.as_bytes()).expect("write template");
        path
    }

    /// Isolate the XDG cache dir so tests don't share a cache with the
    /// user's real koto cache (and don't race each other).
    struct CacheGuard {
        _tmp: TempDir,
        prev: Option<std::ffi::OsString>,
    }

    impl CacheGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tmp cache");
            let prev = std::env::var_os("XDG_CACHE_HOME");
            std::env::set_var("XDG_CACHE_HOME", tmp.path());
            Self { _tmp: tmp, prev }
        }
    }

    impl Drop for CacheGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
    }

    fn backend_in(dir: &std::path::Path) -> LocalBackend {
        LocalBackend::with_base_dir(dir.to_path_buf())
    }

    /// Parent is pre-created so `init_child_from_parent` has something
    /// to point `parent_workflow` at. The helper itself doesn't verify
    /// the parent — that's `handle_init`'s job — but tests mirror the
    /// realistic shape.
    fn seed_parent(backend: &LocalBackend, parent: &str) {
        backend.create(parent).expect("create parent dir");
        let header = StateFileHeader {
            schema_version: 1,
            workflow: parent.to_string(),
            template_hash: "0".repeat(64),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            respawn_generation: None,
        };
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/tmp/unused.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];
        backend
            .init_state_file(parent, header, events)
            .expect("seed parent");
    }

    #[test]
    fn atomic_init_leaves_no_tempfile_remnants() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());
        seed_parent(&backend, "parent");

        let template = write_template(tpl_dir.path(), "child.md", SIMPLE_TEMPLATE);

        let mut cache = TemplateCompileCache::new();
        init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-1",
            &template,
            &["TASK_ID=42".to_string()],
            &mut cache,
            None,
        )
        .expect("init child");

        // The atomic `init_state_file` lands the final state file via
        // tempfile::persist; no `.tmp*` remnants should remain alongside
        // it. A failing rename would leave either a temp file or no
        // state file; both are caught here.
        let child_dir = sessions.path().join("parent.child-1");
        let state_path = child_dir.join(crate::session::state_file_name("parent.child-1"));
        assert!(
            state_path.exists(),
            "child state file must exist at {}",
            state_path.display()
        );

        let mut leftover_tempfiles = 0usize;
        for entry in std::fs::read_dir(&child_dir).expect("read child dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains(".tmp") {
                leftover_tempfiles += 1;
            }
        }
        assert_eq!(
            leftover_tempfiles, 0,
            "init_state_file must leave no temp artifacts"
        );
    }

    #[test]
    fn shared_cache_compiles_each_template_once() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());
        seed_parent(&backend, "parent");

        let template = write_template(tpl_dir.path(), "child.md", SIMPLE_TEMPLATE);

        let mut cache = TemplateCompileCache::new();
        assert!(cache.is_empty());

        init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-a",
            &template,
            &["TASK_ID=1".to_string()],
            &mut cache,
            None,
        )
        .expect("first spawn");

        init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-b",
            &template,
            &["TASK_ID=2".to_string()],
            &mut cache,
            None,
        )
        .expect("second spawn");

        // Two spawns against the same template path must share one
        // cache entry. If the helper were recompiling per call the
        // cache would be empty (bypassed) or hold duplicate entries
        // keyed on distinct `PathBuf`s.
        assert_eq!(cache.len(), 1, "one template path => one cache entry");
    }

    #[test]
    fn collision_maps_to_spawn_error_kind_collision() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());
        seed_parent(&backend, "parent");

        let template = write_template(tpl_dir.path(), "child.md", SIMPLE_TEMPLATE);

        let mut cache = TemplateCompileCache::new();
        init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-dup",
            &template,
            &["TASK_ID=1".to_string()],
            &mut cache,
            None,
        )
        .expect("first spawn");

        let mut cache2 = TemplateCompileCache::new();
        let err = init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-dup",
            &template,
            &["TASK_ID=1".to_string()],
            &mut cache2,
            None,
        )
        .expect_err("second spawn must collide");

        assert_eq!(err.kind, SpawnErrorKind::Collision, "err={:?}", err);
        assert_eq!(err.task, "parent.child-dup");
    }

    #[test]
    fn missing_template_maps_to_template_not_found() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let backend = backend_in(sessions.path());
        seed_parent(&backend, "parent");

        let missing = sessions.path().join("does-not-exist.md");

        let mut cache = TemplateCompileCache::new();
        let err = init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-x",
            &missing,
            &[],
            &mut cache,
            None,
        )
        .expect_err("missing template must error");

        assert_eq!(err.kind, SpawnErrorKind::TemplateNotFound, "err={:?}", err);
        assert_eq!(err.task, "parent.child-x");
    }

    /// `parent_name = None` is the root-init path `handle_init` takes.
    /// The resulting state-file header must record `parent_workflow =
    /// None` so it stays a top-level workflow on disk, not a child.
    #[test]
    fn root_init_leaves_parent_workflow_none() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());

        let template = write_template(tpl_dir.path(), "root.md", SIMPLE_TEMPLATE);

        let mut cache = TemplateCompileCache::new();
        init_child_from_parent(
            &backend,
            None,
            "root-workflow",
            &template,
            &["TASK_ID=1".to_string()],
            &mut cache,
            None,
        )
        .expect("root init");

        let (header, events) = backend
            .read_events("root-workflow")
            .expect("read root events");
        assert_eq!(
            header.parent_workflow, None,
            "root init must leave parent_workflow unset"
        );
        assert_eq!(header.workflow, "root-workflow");

        // Top-level init (parent_name=None) must leave spawn_entry None
        // on the WorkflowInitialized event — the snapshot is meaningful
        // only when a batch scheduler populates it.
        match &events[0].payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => assert!(
                spawn_entry.is_none(),
                "top-level koto init must leave spawn_entry None"
            ),
            other => panic!("expected WorkflowInitialized, got {:?}", other),
        }
    }

    /// When the caller supplies a `spawn_entry` (the batch-scheduler
    /// hot path), the helper must record it verbatim on the child's
    /// `WorkflowInitialized` event so later ticks can R8-compare
    /// against the snapshot.
    #[test]
    fn spawn_entry_is_persisted_on_workflow_initialized_event() {
        use std::collections::BTreeMap;

        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());
        seed_parent(&backend, "parent");

        let template = write_template(tpl_dir.path(), "child.md", SIMPLE_TEMPLATE);

        let mut vars_map = BTreeMap::new();
        vars_map.insert(
            "TASK_ID".to_string(),
            serde_json::Value::String("42".to_string()),
        );
        let snapshot = SpawnEntrySnapshot::new(
            "child.md".to_string(),
            vars_map,
            vec!["sibling-a".to_string()],
        );

        let mut cache = TemplateCompileCache::new();
        init_child_from_parent(
            &backend,
            Some("parent"),
            "parent.child-snap",
            &template,
            &["TASK_ID=42".to_string()],
            &mut cache,
            Some(snapshot.clone()),
        )
        .expect("child init with snapshot");

        let (_header, events) = backend
            .read_events("parent.child-snap")
            .expect("read child events");
        match &events[0].payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => {
                assert_eq!(
                    spawn_entry.as_ref(),
                    Some(&snapshot),
                    "spawn_entry must round-trip through init_state_file"
                );
            }
            other => panic!("expected WorkflowInitialized, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Inline session-local storage (Issue 1)
    // -----------------------------------------------------------------------

    const INLINE_TEMPLATE: &str = r#"---
name: inline-wf
version: "1.0"
initial_state: gather
states:
  gather:
    transitions:
      - target: done
  done:
    terminal: true
---

## gather

Gather things.

## done

Done.
"#;

    /// The compiled artifact is written under the session directory and the
    /// `WorkflowInitialized.template_path` is recorded as a session-relative
    /// filename (no directory component, no absolute cache path).
    #[test]
    fn inline_init_stores_artifact_in_session_dir_with_relative_path() {
        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());

        let source = write_template(tpl_dir.path(), "inline.md", INLINE_TEMPLATE);

        init_inline_into_session(&backend, "inline-wf", &source, &[]).expect("inline init");

        let (header, events) = backend.read_events("inline-wf").expect("read events");

        // The recorded template_path must be RELATIVE (the bare filename),
        // not the absolute cache path the --template path would record.
        let recorded = match &events[0].payload {
            EventPayload::WorkflowInitialized { template_path, .. } => template_path.clone(),
            other => panic!("expected WorkflowInitialized, got {:?}", other),
        };
        assert!(
            !Path::new(&recorded).is_absolute(),
            "inline template_path must be session-relative, got {:?}",
            recorded
        );
        assert!(
            !recorded.contains('/'),
            "inline template_path must be a bare filename, got {:?}",
            recorded
        );
        assert!(
            recorded.ends_with(".json"),
            "artifact must be the compiled JSON, got {:?}",
            recorded
        );

        // The artifact must physically live inside the session directory.
        let session_dir = backend.session_dir("inline-wf");
        let artifact = session_dir.join(&recorded);
        assert!(
            artifact.exists(),
            "compiled artifact must exist under the session dir at {}",
            artifact.display()
        );

        // The header hash must be the SHA-256 of the on-disk artifact bytes.
        let bytes = std::fs::read(&artifact).expect("read artifact");
        assert_eq!(
            header.template_hash,
            sha256_hex(&bytes),
            "template_hash must be content-addressed over the artifact bytes"
        );
    }

    /// `derive_machine_state` resolves the session-relative path against the
    /// session directory, yielding a directly-readable absolute path whose
    /// bytes hash to the recorded `template_hash` — and that holds across
    /// repeated reads with the global cache emptied between each, proving the
    /// run does not depend on `~/.cache`.
    #[test]
    fn inline_artifact_survives_cache_eviction_across_repeated_reads() {
        use crate::engine::persistence::derive_machine_state;

        let cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());

        let source = write_template(tpl_dir.path(), "inline.md", INLINE_TEMPLATE);
        init_inline_into_session(&backend, "inline-wf", &source, &[]).expect("inline init");

        let (header, events) = backend.read_events("inline-wf").expect("read events");
        let session_dir = backend.session_dir("inline-wf");

        for tick in 0..3 {
            // Empty the global cache between ticks: a cache-dependent run
            // would break here. The session-local artifact must not.
            let cache_dir = cache._tmp.path().join("koto");
            if cache_dir.exists() {
                std::fs::remove_dir_all(&cache_dir).expect("evict cache");
            }
            assert!(
                !cache_dir.exists(),
                "precondition: cache must be empty on tick {}",
                tick
            );

            let ms =
                derive_machine_state(&header, &events, &session_dir).expect("derive machine state");

            // The resolved path is absolute (session dir is absolute) and
            // reads back to bytes that hash to the recorded template_hash.
            let bytes = std::fs::read(&ms.template_path).unwrap_or_else(|e| {
                panic!(
                    "tick {}: failed to read resolved template {}: {}",
                    tick, ms.template_path, e
                )
            });
            assert_eq!(
                sha256_hex(&bytes),
                ms.template_hash,
                "tick {}: hash verification must pass against the session-local artifact",
                tick
            );
        }
    }

    /// After a session `relocate` (which renames the whole directory and
    /// rewrites only the header, not the event-log `template_path`), the
    /// session-relative path still resolves against the new directory and
    /// still hash-verifies.
    #[test]
    fn inline_artifact_resolves_after_relocate() {
        use crate::engine::persistence::derive_machine_state;

        let _cache = CacheGuard::new();
        let sessions = TempDir::new().expect("sessions dir");
        let tpl_dir = TempDir::new().expect("templates dir");
        let backend = backend_in(sessions.path());

        let source = write_template(tpl_dir.path(), "inline.md", INLINE_TEMPLATE);
        init_inline_into_session(&backend, "inline-old", &source, &[]).expect("inline init");

        backend
            .relocate("inline-old", "inline-new")
            .expect("relocate");

        let (header, events) = backend
            .read_events("inline-new")
            .expect("read relocated events");
        let session_dir = backend.session_dir("inline-new");

        let ms =
            derive_machine_state(&header, &events, &session_dir).expect("derive machine state");

        // The resolved path points into the NEW session dir, and the bytes
        // still hash-verify — the relocation gap is closed because the
        // recorded path is relative, not the (now-dangling) absolute path.
        assert!(
            ms.template_path.contains("inline-new"),
            "resolved path must point into the relocated dir, got {}",
            ms.template_path
        );
        let bytes = std::fs::read(&ms.template_path).expect("read relocated artifact");
        assert_eq!(
            sha256_hex(&bytes),
            ms.template_hash,
            "hash verification must pass after relocate"
        );
    }
}
