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
//! - **Per-tick compile cache.** The caller passes an optional
//!   `&mut TemplateCompileCache`. When present, the same template path
//!   is compiled once per tick and reused for every task that points at
//!   it. When absent (direct callers, tests), the helper allocates a
//!   throwaway cache for the one call so the public surface stays
//!   identical.
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

use crate::cache::compile_cached;
use crate::cli::task_spawn_error::{SpawnErrorKind, TaskSpawnError};
use crate::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use crate::session::{SessionBackend, SessionError};
use crate::template::types::CompiledTemplate;

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

/// Compile `template_path` once per cache, returning the cached
/// [`CompiledTemplate`], its on-disk cache path (used as the stored
/// `template_path` on the `WorkflowInitialized` event), and its hash
/// (written into the state-file header).
///
/// A hit returns the previously compiled bundle verbatim. A miss
/// canonicalizes the source path, compiles via [`compile_cached`], and
/// records the result before returning.
//
// clippy::result_large_err: `TaskSpawnError` is intentionally rich — its
// shape is fixed by the design doc's Key Interfaces section so the
// future batch scheduler can emit `paths_tried`, `template_source`, and
// a typed `compile_error` on the same envelope. Boxing it here would
// force the scheduler to unwrap Box every time it accumulates an
// error into `SchedulerOutcome::Scheduled.errored`. The struct still
// fits comfortably on the stack for the call depths this helper sees.
#[allow(clippy::result_large_err)]
fn compile_with_cache(
    template_path: &Path,
    cache: &mut TemplateCompileCache,
) -> Result<CachedTemplate, TaskSpawnError> {
    // Canonicalize *before* looking up so two relative paths that point
    // at the same file share a cache slot. `canonicalize` also fails
    // with `NotFound` when the source doesn't exist, which is exactly
    // the signal `TemplateNotFound` encodes.
    let canonical = match std::fs::canonicalize(template_path) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(TaskSpawnError::new(
                "",
                SpawnErrorKind::TemplateNotFound,
                format!("template not found: {} ({})", template_path.display(), e),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(TaskSpawnError::new(
                "",
                SpawnErrorKind::PermissionDenied,
                format!(
                    "permission denied reading template {}: {}",
                    template_path.display(),
                    e
                ),
            ));
        }
        Err(e) => {
            return Err(TaskSpawnError::new(
                "",
                SpawnErrorKind::IoError,
                format!(
                    "failed to access template {}: {}",
                    template_path.display(),
                    e
                ),
            ));
        }
    };

    if let Some(hit) = cache.entries.get(&canonical) {
        return Ok(hit.clone());
    }

    let (cache_path, hash) = compile_cached(&canonical, false).map_err(|e| {
        TaskSpawnError::new(
            "",
            SpawnErrorKind::TemplateCompileFailed,
            format!("failed to compile template {}: {}", canonical.display(), e),
        )
    })?;

    let content = std::fs::read_to_string(&cache_path).map_err(|e| {
        TaskSpawnError::new(
            "",
            SpawnErrorKind::IoError,
            format!(
                "failed to read cached template {}: {}",
                cache_path.display(),
                e
            ),
        )
    })?;
    let compiled: CompiledTemplate = serde_json::from_str(&content).map_err(|e| {
        TaskSpawnError::new(
            "",
            SpawnErrorKind::TemplateCompileFailed,
            format!(
                "failed to parse cached template {}: {}",
                cache_path.display(),
                e
            ),
        )
    })?;

    let entry = CachedTemplate {
        compiled,
        cache_path,
        hash,
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

/// Initialize a child workflow underneath `parent_name`.
///
/// `child_name` is the full composed workflow name the caller wants on
/// disk (e.g., `parent.issue-1`). The helper does not prepend the
/// parent name itself — the scheduler composes that upstream so the
/// composition rules live in one place.
///
/// `template_path` is the source template (markdown with YAML
/// frontmatter). The helper canonicalizes it, runs it through the
/// supplied [`TemplateCompileCache`] (or a one-shot cache when `None`)
/// and resolves `vars` against the child template's variable
/// declarations.
///
/// On success a single atomic `init_state_file` call commits the
/// header plus the `WorkflowInitialized` and initial `Transitioned`
/// events. Every failure path returns a [`TaskSpawnError`] whose
/// `task` field is `child_name`, so the scheduler can fan the error
/// straight into `SchedulerOutcome::Scheduled.errored` without extra
/// bookkeeping.
//
// clippy::result_large_err: see the note on `compile_with_cache` above —
// the error shape is dictated by the design doc and is intentionally
// rich; boxing would only shift the cost to every caller in
// `SchedulerOutcome::Scheduled.errored`.
#[allow(clippy::result_large_err)]
pub fn init_child_from_parent(
    backend: &dyn SessionBackend,
    child_name: &str,
    parent_name: &str,
    template_path: &Path,
    vars: &[String],
    cache: Option<&mut TemplateCompileCache>,
) -> Result<(), TaskSpawnError> {
    // Use the caller-supplied cache when available so multiple spawns
    // in one tick share compile work; otherwise allocate a throwaway
    // cache with a single entry so the rest of this function stays
    // uniform.
    let mut local_cache = TemplateCompileCache::new();
    let cache_ref: &mut TemplateCompileCache = match cache {
        Some(c) => c,
        None => &mut local_cache,
    };

    let cached = compile_with_cache(template_path, cache_ref).map_err(|e| TaskSpawnError {
        task: child_name.to_string(),
        ..e
    })?;

    let variables =
        crate::cli::resolve_variables(vars, &cached.compiled.variables).map_err(|msg| {
            TaskSpawnError::new(
                child_name,
                SpawnErrorKind::TemplateCompileFailed,
                format!("variable resolution failed: {}", msg),
            )
        })?;

    let initial_state = cached.compiled.initial_state.clone();
    let ts = now_iso8601();
    let cache_path_str = cached.cache_path.to_string_lossy().to_string();

    let header = StateFileHeader {
        schema_version: 1,
        workflow: child_name.to_string(),
        template_hash: cached.hash.clone(),
        created_at: ts.clone(),
        parent_workflow: Some(parent_name.to_string()),
    };

    let init_payload = EventPayload::WorkflowInitialized {
        template_path: cache_path_str,
        variables,
    };
    let transition_payload = EventPayload::Transitioned {
        from: None,
        to: initial_state.clone(),
        condition_type: "auto".to_string(),
    };
    let initial_events = vec![
        Event {
            seq: 1,
            timestamp: ts.clone(),
            event_type: init_payload.type_name().to_string(),
            payload: init_payload,
        },
        Event {
            seq: 2,
            timestamp: ts.clone(),
            event_type: transition_payload.type_name().to_string(),
            payload: transition_payload,
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
    })?;

    backend
        .init_state_file(child_name, header, initial_events)
        .map_err(|e| classify_session_error(child_name, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        };
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/tmp/unused.json".to_string(),
                variables: HashMap::new(),
            },
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

        init_child_from_parent(
            &backend,
            "parent.child-1",
            "parent",
            &template,
            &["TASK_ID=42".to_string()],
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
            "parent.child-a",
            "parent",
            &template,
            &["TASK_ID=1".to_string()],
            Some(&mut cache),
        )
        .expect("first spawn");

        init_child_from_parent(
            &backend,
            "parent.child-b",
            "parent",
            &template,
            &["TASK_ID=2".to_string()],
            Some(&mut cache),
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

        init_child_from_parent(
            &backend,
            "parent.child-dup",
            "parent",
            &template,
            &["TASK_ID=1".to_string()],
            None,
        )
        .expect("first spawn");

        let err = init_child_from_parent(
            &backend,
            "parent.child-dup",
            "parent",
            &template,
            &["TASK_ID=1".to_string()],
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

        let err = init_child_from_parent(&backend, "parent.child-x", "parent", &missing, &[], None)
            .expect_err("missing template must error");

        assert_eq!(err.kind, SpawnErrorKind::TemplateNotFound, "err={:?}", err);
        assert_eq!(err.task, "parent.child-x");
    }
}
