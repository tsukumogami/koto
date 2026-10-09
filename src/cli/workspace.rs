//! `koto workspace prune` -- operator-facing workspace reclaim verb.
//!
//! Reads the root header, validates the workflow has reached a terminal
//! state (`completed` or `abandoned`), walks descendants via
//! `backend.list()` + parent filter, and reclaims after operator
//! confirmation. Symlinked roots reject via `lstat()` before any
//! directory traversal; `fs::remove_dir_all` (the underlying reclaim
//! primitive) does not follow symlinks inside the descendant tree, so
//! a symlink whose target lives outside `~/.koto/` cannot be removed
//! through this verb.
//!
//! The verb intentionally does NOT consult `coordinator_of_record`:
//! Request-store workspaces can be pruned by any operator regardless of which
//! coordinator (if any) is currently dispatching to the tree (Decision
//! 4 line 578). It also does NOT mutate
//! `~/.koto/_terminal_index.jsonl` -- Issue 9 owns terminal-index
//! compaction.
//!
//! TODO(issue-3): switch the `--root` validator to `ValidatedSessionId::new`
//! once Issue 3 lands. The current site reuses `validate_session_id()`
//! from `src/session/validate.rs` which already implements the same
//! character allowlist; the refactor is type-signature only.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use crate::engine::persistence::derive_machine_state;
use crate::engine::types::{EventPayload, StateFileHeader};
use crate::session::{validate::validate_session_id, SessionBackend, SessionInfo};
use crate::template::types::CompiledTemplate;

use super::exit_with_error_code;

/// Outcome of the terminal-state gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalStatus {
    /// Workflow reached a terminal state in the compiled template.
    Completed,
    /// Workflow was cancelled (a `WorkflowCancelled` event is in the log).
    Abandoned,
    /// Workflow has not reached a terminal state; the variant carries
    /// the derived current state name for operator-facing messaging.
    NonTerminal { current_state: String },
}

impl TerminalStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Abandoned)
    }

    fn describe(&self) -> String {
        match self {
            Self::Completed => "completed".to_string(),
            Self::Abandoned => "abandoned".to_string(),
            Self::NonTerminal { current_state } => {
                format!("not terminal (current state: {})", current_state)
            }
        }
    }
}

/// Handle `koto workspace prune --root <id> [--dry-run] [--yes] [--force]`.
///
/// Returns on success; on caller errors (invalid root, non-terminal
/// without `--force`, symlinked root, declined confirmation) calls
/// `exit_with_error_code` and never returns.
pub fn handle_prune(
    backend: &dyn SessionBackend,
    root: String,
    dry_run: bool,
    yes: bool,
    force: bool,
) -> Result<()> {
    // 1. Parse-time validation. Reject injection attempts before any
    //    filesystem operation.
    if let Err(e) = validate_session_id(&root) {
        exit_with_error_code(
            json!({
                "error": format!("invalid --root: {}", e),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 2. Symlink refusal: `lstat()` the root session directory BEFORE
    //    opening anything. A symlinked root is a workspace-escape
    //    vector and must be rejected categorically.
    let root_dir = backend.session_dir(&root);
    reject_if_symlink(&root_dir);

    // 3. Existence check. Operator may have typoed the id.
    if !backend.exists(&root) {
        exit_with_error_code(
            json!({
                "error": format!("session '{}' not found", root),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 4. Read header + events; derive terminal status.
    let (header, events) = backend
        .read_events(&root)
        .map_err(|e| anyhow::anyhow!("failed to read state file for '{}': {}", root, e))?;
    let status = derive_terminal_status(&header, &events, &root_dir)?;

    // 5. Terminal-state gate.
    if !status.is_terminal() && !force {
        exit_with_error_code(
            json!({
                "error": format!(
                    "session '{}' is {}; use --force to prune anyway",
                    root,
                    status.describe()
                ),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 6. Enumerate descendants via backend.list() + parent filter.
    //    Includes transitive descendants, each listed after its parent.
    let all_sessions = backend
        .list()
        .with_context(|| "failed to list sessions for descendant walk")?;
    let descendants = collect_descendants(&root, &all_sessions);

    // 7. Compute non-terminal sessions in the to-be-pruned set. Operator
    //    visibility before any confirmation prompt.
    let non_terminal_in_set = non_terminal_sessions(
        backend,
        std::iter::once(root.as_str()).chain(descendants.iter().map(String::as_str)),
    );

    // 8. Print preview (descendant set + non-terminal warnings).
    print_preview(&root, &descendants, &non_terminal_in_set, &status);

    // 9. Dry-run exits 0 here without reclaiming.
    if dry_run {
        return Ok(());
    }

    // 10. Confirmation prompt. `--yes` skips. Issue 18 will plumb
    //     `KOTO_REQUEST_STORE_PRUNE_CONFIRM=1` as another bypass through this
    //     same `prompt_required` parameter.
    let prompt_required = !yes;
    if !confirm_prune(prompt_required)? {
        exit_with_error_code(
            json!({
                "error": "prune aborted by operator",
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 10b. Second confirmation when --force is combined with --yes.
    //      --yes alone covers the normal --terminal path; --force
    //      bypasses the terminal-state gate and is destructive enough
    //      to warrant a second explicit gate even when the operator
    //      pre-consented. Requires typing the literal string
    //      "force-prune" — exact match, no fuzziness, no case
    //      insensitivity. EOF on stdin is treated as negative consent.
    if yes && force && !confirm_force_prune()? {
        exit_with_error_code(
            json!({
                "error": "force-prune aborted: confirmation phrase not entered",
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 11. Reclaim. Descendants first, deepest first, so a partial failure
    //     leaves every remaining session under a parent that still exists
    //     and the root visible in `koto workflows`.
    for id in descendants.iter().rev() {
        backend
            .cleanup(id)
            .with_context(|| format!("failed to remove descendant session '{}'", id))?;
    }
    backend
        .cleanup(&root)
        .with_context(|| format!("failed to remove root session '{}'", root))?;

    // 12. Issue 7: invoke the cursor GC walk so stale coordinator
    //     cursors are reclaimed as part of prune. A GC failure is
    //     non-fatal — the prune already succeeded, we surface the
    //     count as 0 on error and let `koto next` startup retry.
    let cursors_gc = match (|| -> anyhow::Result<usize> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        let rs = crate::config::resolve::load_config()
            .unwrap_or_default()
            .request_store;
        crate::engine::discovery::gc_stale_cursors(&home.join(".koto"), &rs)
    })() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("warning: cursor GC failed during workspace prune: {}", e);
            0
        }
    };

    println!(
        "{}",
        json!({
            "name": root,
            "pruned": true,
            "descendants_removed": descendants.len(),
            "cursors_gc": cursors_gc,
        })
    );

    Ok(())
}

/// `lstat()` the candidate path; if it is a symlink, reject with a
/// clear error. This catches both an attacker-crafted root pointing
/// outside `~/.koto/` and the legitimate-but-disallowed case of an
/// operator symlinking a session directory into the workspace.
fn reject_if_symlink(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            exit_with_error_code(
                json!({
                    "error": format!(
                        "symlink not permitted: {}",
                        path.display()
                    ),
                    "command": "workspace prune",
                }),
                2,
            );
        }
        // Path doesn't exist yet: that's caller-error, surfaced below
        // by the `backend.exists()` check. Other I/O errors are surfaced
        // there too.
        _ => {}
    }
}

/// Walk events + header to determine whether the root has reached a
/// terminal state. `WorkflowCancelled` events take precedence (the
/// workflow was explicitly aborted); otherwise compare the derived
/// current state against the compiled template's `terminal` flag.
fn derive_terminal_status(
    header: &StateFileHeader,
    events: &[crate::engine::types::Event],
    session_dir: &Path,
) -> Result<TerminalStatus> {
    if events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }))
    {
        return Ok(TerminalStatus::Abandoned);
    }

    let machine_state = derive_machine_state(header, events, session_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "corrupt state file: cannot derive current state for header.workflow={}",
            header.workflow
        )
    })?;

    let template_bytes = std::fs::read(&machine_state.template_path)
        .with_context(|| format!("failed to read template at {}", machine_state.template_path))?;
    let compiled: CompiledTemplate =
        serde_json::from_slice(&template_bytes).with_context(|| {
            format!(
                "failed to parse template at {}",
                machine_state.template_path
            )
        })?;

    let is_terminal = compiled
        .states
        .get(&machine_state.current_state)
        .is_some_and(|s| s.terminal);
    if is_terminal {
        Ok(TerminalStatus::Completed)
    } else {
        Ok(TerminalStatus::NonTerminal {
            current_state: machine_state.current_state,
        })
    }
}

/// Walk `SessionInfo.parent_workflow` to collect every transitive
/// descendant of `root`. Each session is listed after its parent, so the
/// caller in `handle_prune` removes the list in reverse, deepest first, and
/// removes the root last: a failed removal then leaves every remaining
/// session under a parent that still exists, reachable by the next prune
/// from the same root.
fn collect_descendants(root: &str, sessions: &[SessionInfo]) -> Vec<String> {
    let mut descendants = Vec::new();
    // A `parent_workflow` cycle (A -> B -> A) is constructible by removing a
    // parent and re-creating it under its own former child; the visited set
    // is what ends the walk instead of looping forever.
    let mut visited: HashSet<String> = HashSet::from([root.to_string()]);
    let mut frontier: Vec<String> = vec![root.to_string()];
    while let Some(parent) = frontier.pop() {
        for s in sessions {
            if s.parent_workflow.as_deref() == Some(parent.as_str()) && visited.insert(s.id.clone())
            {
                descendants.push(s.id.clone());
                frontier.push(s.id.clone());
            }
        }
    }
    descendants
}

// ===========================================================
// Removing a parent's retained descendants with it
// ===========================================================

/// Remove `name`'s terminal descendants, if `name` can have children at all.
///
/// Called just before koto removes a session at its own terminal, so a child
/// kept on disk (a failure terminal, or `--no-cleanup`) doesn't outlive the
/// parent that koto removed (koto issue 240). A session can have children
/// when its template declares a `materialize_children` hook, which is how
/// koto recognises a coordinator, or when its own log holds a
/// `ChildCompleted` from a child created with `koto init --parent`. A leaf
/// session, which is most terminal ticks, never lists sessions.
///
/// One case falls outside both triggers: a parent without a batch hook on
/// whose log no `ChildCompleted` landed (every child's notice failed to
/// write). That child is left
/// behind, visible in `koto workflows --orphaned`.
pub(crate) fn sweep_if_parent(
    backend: &dyn SessionBackend,
    name: &str,
    compiled: &CompiledTemplate,
) {
    let declares_children = compiled
        .states
        .values()
        .any(|s| s.materialize_children.is_some());
    let had_children = declares_children
        || backend.read_events(name).is_ok_and(|(_, events)| {
            events
                .iter()
                .any(|e| matches!(e.payload, EventPayload::ChildCompleted { .. }))
        });
    if had_children {
        sweep_terminal_descendants(backend, name);
    }
}

/// Remove every descendant of `parent` that stands in a terminal state and
/// has nothing live under it, deepest first.
///
/// The removal is implicit -- nobody named these sessions -- so every rule
/// fails toward keeping:
///
/// - a descendant that isn't terminal is live work: it is not removed and
///   the walk does not descend into it;
/// - a descendant is removed only after every session under it was;
/// - a descendant whose log or template can't be read or classified is left
///   alone with its subtree;
/// - a descendant bound to a request leg that is still open is left alone:
///   its result hasn't reached the leg (typically a failed promotion waiting
///   for a retry), and removing it would leave the leg open for good;
/// - a `parent_workflow` cycle ends the walk (visited set, depth cap);
/// - each session's status is read again just before its removal, narrowing
///   the window in which a concurrent `koto rewind` could bring it back;
/// - a failed removal warns and the walk continues; the caller removes the
///   parent either way, and a leftover shows in `koto workflows --orphaned`.
pub(crate) fn sweep_terminal_descendants(backend: &dyn SessionBackend, parent: &str) {
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("warning: could not list sessions to remove {parent}'s kept children: {e}");
            return;
        }
    };
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &sessions {
        if let Some(p) = s.parent_workflow.as_deref() {
            children.entry(p).or_default().push(s.id.as_str());
        }
    }
    let mut visited: HashSet<String> = HashSet::from([parent.to_string()]);
    sweep_children(backend, parent, &children, &mut visited, 0);
}

/// Deeper than any real fan-out; only a corrupted header graph reaches it.
const MAX_SWEEP_DEPTH: usize = 1000;

/// Sweep the children of `id`. Returns true when every child was removed.
fn sweep_children(
    backend: &dyn SessionBackend,
    id: &str,
    children: &HashMap<&str, Vec<&str>>,
    visited: &mut HashSet<String>,
    depth: usize,
) -> bool {
    let Some(kids) = children.get(id) else {
        return true;
    };
    if depth >= MAX_SWEEP_DEPTH {
        return false;
    }
    let mut all_removed = true;
    for &child in kids {
        if !visited.insert(child.to_string()) {
            // A cycle back into the part of the tree already walked.
            all_removed = false;
            continue;
        }
        if !is_terminal_session(backend, child) || awaits_leg_promotion(backend, child) {
            all_removed = false;
            continue;
        }
        if !sweep_children(backend, child, children, visited, depth + 1) {
            all_removed = false;
            continue;
        }
        // Read it again: something may have rewound it since the walk began.
        if !is_terminal_session(backend, child) {
            all_removed = false;
            continue;
        }
        match backend.cleanup(child) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("warning: could not remove kept session {child}: {e}");
                all_removed = false;
            }
        }
    }
    all_removed
}

/// True when `id` is bound to a request leg that is still open, so its
/// terminal result has not reached the leg yet -- typically because the
/// promotion failed and the session's next tick would retry it. The rules
/// follow `promote_leg_result`'s own: a request that is gone, closed,
/// unreadable, or whose leg is resolved or abandoned is not pending (the
/// tick would give up too); an I/O error, which the tick would retry, keeps
/// the session.
fn awaits_leg_promotion(backend: &dyn SessionBackend, id: &str) -> bool {
    use crate::engine::request_store::{self, RequestStoreError, ValidatedRequestId};
    use crate::engine::types::{LegDisposition, RequestState};

    let Some(pointer) =
        crate::engine::leg_pointer::read_pointer_best_effort(&backend.session_dir(id))
    else {
        return false;
    };
    let (Some(home), Ok(request_id)) = (
        dirs::home_dir(),
        ValidatedRequestId::new(&pointer.request_id),
    ) else {
        return false;
    };
    match request_store::read_view(&home.join(".koto"), &request_id) {
        Ok(view) => {
            view.request_state == RequestState::Open
                && view
                    .leg(&pointer.leg_name)
                    .is_ok_and(|leg| leg.disposition == LegDisposition::Open)
        }
        Err(RequestStoreError::Io { .. }) => true,
        Err(_) => false,
    }
}

/// True only when `id` reads and classifies as terminal (completed or
/// abandoned). Any read or classification error is "not terminal", so the
/// sweep leaves the session alone. This is deliberately the opposite of
/// `non_terminal_sessions`, which only warns before an operator-confirmed
/// prune and so treats an unreadable session as nothing to warn about.
fn is_terminal_session(backend: &dyn SessionBackend, id: &str) -> bool {
    let Ok((header, events)) = backend.read_events(id) else {
        return false;
    };
    derive_terminal_status(&header, &events, &backend.session_dir(id))
        .is_ok_and(|status| status.is_terminal())
}

/// Filter the candidate set to sessions whose terminal status is
/// non-terminal. Returns a sorted vector for stable operator-facing
/// output. Errors during inspection are skipped silently -- a session
/// we cannot read is a session we cannot warn about, but the reclaim
/// step will surface the underlying issue.
fn non_terminal_sessions<'a, I>(backend: &dyn SessionBackend, candidates: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: Vec<String> = candidates
        .into_iter()
        .filter(|id| {
            let Ok((header, events)) = backend.read_events(id) else {
                return false;
            };
            match derive_terminal_status(&header, &events, &backend.session_dir(id)) {
                Ok(status) => !status.is_terminal(),
                Err(_) => false,
            }
        })
        .map(|s| s.to_string())
        .collect();
    out.sort();
    out
}

/// Print operator-facing preview of what's about to be reclaimed.
///
/// Always emits to stdout (the verb's primary output channel). The
/// preview lists the root + descendant count, and explicitly names
/// any non-terminal sessions in the to-be-pruned set so the operator
/// can abort if `--force` is masking a live session.
fn print_preview(
    root: &str,
    descendants: &[String],
    non_terminal_in_set: &[String],
    status: &TerminalStatus,
) {
    println!("root: {} ({})", root, status.describe());
    if descendants.is_empty() {
        println!("descendants: (none)");
    } else {
        println!("descendants ({}):", descendants.len());
        for d in descendants {
            println!("  {}", d);
        }
    }
    if !non_terminal_in_set.is_empty() {
        println!();
        println!("WARNING: the following sessions in the prune set are non-terminal:");
        for id in non_terminal_in_set {
            println!("  {}", id);
        }
    }
}

/// Prompt the operator for confirmation, returning whether to proceed.
///
/// `prompt_required = false` means the caller has already gathered
/// consent (e.g. `--yes` on the CLI, or Issue 18's
/// `KOTO_REQUEST_STORE_PRUNE_CONFIRM=1` env-var bypass) and the prompt is
/// skipped. With `prompt_required = true`, the function writes the
/// prompt to stdout, reads one line from stdin, and returns true on
/// `y`/`yes` (case-insensitive, trimmed). EOF on stdin counts as
/// negative consent.
fn confirm_prune(prompt_required: bool) -> io::Result<bool> {
    if !prompt_required {
        return Ok(true);
    }
    print!("Proceed with prune? [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    let n = io::stdin().read_line(&mut input)?;
    if n == 0 {
        return Ok(false); // EOF
    }
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

/// Second-tier confirmation gate for `--yes --force`. Fires AFTER
/// [`confirm_prune`] returns true and requires the operator to type
/// the literal string `force-prune` (exact match, no case folding)
/// before the destructive prune executes.
///
/// The terminal-state safety gate (refusing to prune a non-terminal
/// root without `--force`) is the first line of defense. `--yes` lets
/// cron skip the standard y/N prompt. The combination of `--yes` AND
/// `--force` removes both gates; this helper is the manual override
/// that ensures the operator INTENDS to force-prune and isn't running
/// a templated cron with `--force` baked in by accident.
fn confirm_force_prune() -> io::Result<bool> {
    println!();
    println!("WARNING: --force bypasses the terminal-state safety gate.");
    println!(
        "         A force-prune of a live tree corrupts any coordinator still holding a claim."
    );
    print!("Type 'force-prune' to confirm: ");
    io::stdout().flush()?;
    let mut input = String::new();
    let n = io::stdin().read_line(&mut input)?;
    // Print a newline so subsequent stdout (the success JSON or an
    // error payload) lands on a fresh line. Without this, piped stdin
    // (no terminal echo) leaves the prompt and the next println on the
    // same physical line, mangling downstream JSON parsers.
    println!();
    if n == 0 {
        return Ok(false); // EOF
    }
    // Trim trailing newline but keep case + body. Exact match only.
    Ok(input.trim_end_matches(['\n', '\r']) == "force-prune")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- sweep_if_parent: a leaf tick never lists sessions -----

    /// A backend that delegates to a local one and counts `list()` calls.
    struct CountingBackend {
        inner: crate::session::local::LocalBackend,
        lists: std::sync::atomic::AtomicUsize,
    }

    impl SessionBackend for CountingBackend {
        fn create(&self, id: &str) -> anyhow::Result<std::path::PathBuf> {
            self.inner.create(id)
        }
        fn session_dir(&self, id: &str) -> std::path::PathBuf {
            self.inner.session_dir(id)
        }
        fn exists(&self, id: &str) -> bool {
            self.inner.exists(id)
        }
        fn cleanup(&self, id: &str) -> anyhow::Result<()> {
            self.inner.cleanup(id)
        }
        fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
            self.lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.list()
        }
        fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
            self.inner.append_header(id, header)
        }
        fn append_event(
            &self,
            id: &str,
            payload: &EventPayload,
            timestamp: &str,
        ) -> anyhow::Result<()> {
            self.inner.append_event(id, payload, timestamp)
        }
        fn init_state_file(
            &self,
            id: &str,
            header: StateFileHeader,
            initial_events: Vec<crate::engine::types::Event>,
        ) -> Result<(), crate::session::SessionError> {
            self.inner.init_state_file(id, header, initial_events)
        }
        fn read_events(
            &self,
            id: &str,
        ) -> anyhow::Result<(StateFileHeader, Vec<crate::engine::types::Event>)> {
            self.inner.read_events(id)
        }
        fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
            self.inner.read_header(id)
        }
        fn ensure_pushed(&self, id: &str) -> Result<(), crate::session::SessionError> {
            self.inner.ensure_pushed(id)
        }
        fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
            self.inner.relocate(from, to)
        }
        fn lock_state_file(
            &self,
            id: &str,
        ) -> Result<crate::session::SessionLock, crate::session::SessionError> {
            self.inner.lock_state_file(id)
        }
    }

    fn header(name: &str) -> StateFileHeader {
        StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: name.to_string(),
            template_hash: "h".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
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
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        }
    }

    fn leaf_template() -> CompiledTemplate {
        CompiledTemplate {
            pass_env: Vec::new(),
            format_version: 1,
            name: "leaf".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "work".to_string(),
            variables: Default::default(),
            states: Default::default(),
        }
    }

    #[test]
    fn sweep_lists_sessions_only_for_a_session_that_had_children() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = CountingBackend {
            inner: crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf()),
            lists: Default::default(),
        };
        backend
            .init_state_file("leaf", header("leaf"), vec![])
            .unwrap();

        sweep_if_parent(&backend, "leaf", &leaf_template());
        assert_eq!(
            backend.lists.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a session with no batch hook and no reported child is not swept"
        );

        // A session a child reported to is swept, which lists once.
        backend
            .append_event(
                "leaf",
                &EventPayload::ChildCompleted {
                    child_name: "leaf.c".to_string(),
                    task_name: "c".to_string(),
                    outcome: crate::engine::types::TerminalOutcome::Success,
                    final_state: "done".to_string(),
                    result: None,
                    failure_reason: None,
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();
        sweep_if_parent(&backend, "leaf", &leaf_template());
        assert_eq!(backend.lists.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Prune removes descendants in reverse, deepest first, which is only
    /// correct if every session is listed after its parent.
    #[test]
    fn collect_descendants_lists_each_session_after_its_parent() {
        let info = |id: &str, parent: Option<&str>| SessionInfo {
            id: id.to_string(),
            created_at: "t".to_string(),
            template_hash: "h".to_string(),
            parent_workflow: parent.map(str::to_string),
            template_source_status: None,
        };
        // Listed out of order on purpose.
        let sessions = vec![
            info("gc", Some("c")),
            info("c", Some("root")),
            info("root", None),
            info("c2", Some("root")),
        ];
        let order = collect_descendants("root", &sessions);
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("c") < pos("gc"), "{order:?}");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn collect_descendants_finds_direct_and_transitive_children() {
        let sessions = vec![
            SessionInfo {
                id: "root".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
                template_source_status: None,
            },
            SessionInfo {
                id: "child-a".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
                template_source_status: None,
            },
            SessionInfo {
                id: "child-b".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
                template_source_status: None,
            },
            SessionInfo {
                id: "grandchild".to_string(),
                created_at: "t2".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("child-a".to_string()),
                template_source_status: None,
            },
            SessionInfo {
                id: "unrelated".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
                template_source_status: None,
            },
        ];

        let descendants = collect_descendants("root", &sessions);
        assert_eq!(descendants.len(), 3);
        assert!(descendants.contains(&"child-a".to_string()));
        assert!(descendants.contains(&"child-b".to_string()));
        assert!(descendants.contains(&"grandchild".to_string()));
        assert!(!descendants.contains(&"unrelated".to_string()));
        assert!(!descendants.contains(&"root".to_string()));
    }

    #[test]
    fn collect_descendants_empty_when_no_children() {
        let sessions = vec![SessionInfo {
            id: "lonely".to_string(),
            created_at: "t0".to_string(),
            template_hash: "h0".to_string(),
            parent_workflow: None,
            template_source_status: None,
        }];
        let descendants = collect_descendants("lonely", &sessions);
        assert!(descendants.is_empty());
    }

    #[test]
    fn terminal_status_describes_each_variant() {
        assert_eq!(TerminalStatus::Completed.describe(), "completed");
        assert_eq!(TerminalStatus::Abandoned.describe(), "abandoned");
        assert_eq!(
            TerminalStatus::NonTerminal {
                current_state: "review".to_string()
            }
            .describe(),
            "not terminal (current state: review)"
        );
    }

    #[test]
    fn terminal_status_is_terminal() {
        assert!(TerminalStatus::Completed.is_terminal());
        assert!(TerminalStatus::Abandoned.is_terminal());
        assert!(!TerminalStatus::NonTerminal {
            current_state: "s".to_string()
        }
        .is_terminal());
    }
}
