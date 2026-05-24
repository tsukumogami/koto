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
    let status = derive_terminal_status(&header, &events)?;

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
    //    Includes transitive descendants (BFS).
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

    // 11. Reclaim. Descendants first so a partial failure leaves the
    //     root visible in `koto workflows`.
    for id in &descendants {
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
) -> Result<TerminalStatus> {
    if events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }))
    {
        return Ok(TerminalStatus::Abandoned);
    }

    let machine_state = derive_machine_state(header, events).ok_or_else(|| {
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

/// DFS over `SessionInfo.parent_workflow` to collect every transitive
/// descendant of `root`. Removal safety depends on root-removed-last
/// (the caller in `handle_prune` removes descendants first then the
/// root), NOT on visit order — a failed removal mid-tree just leaves
/// children rooted at a still-present parent until the next prune
/// retry. The visit order is DFS as an implementation detail of
/// `Vec::pop()`; BFS would be equally safe.
fn collect_descendants(root: &str, sessions: &[SessionInfo]) -> Vec<String> {
    let mut descendants = Vec::new();
    let mut frontier: Vec<String> = vec![root.to_string()];
    while let Some(parent) = frontier.pop() {
        for s in sessions {
            if s.parent_workflow.as_deref() == Some(parent.as_str()) {
                descendants.push(s.id.clone());
                frontier.push(s.id.clone());
            }
        }
    }
    descendants
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
            match derive_terminal_status(&header, &events) {
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
    if n == 0 {
        return Ok(false); // EOF
    }
    // Trim trailing newline but keep case + body. Exact match only.
    Ok(input.trim_end_matches(['\n', '\r']) == "force-prune")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_descendants_finds_direct_and_transitive_children() {
        let sessions = vec![
            SessionInfo {
                id: "root".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
            },
            SessionInfo {
                id: "child-a".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
            },
            SessionInfo {
                id: "child-b".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
            },
            SessionInfo {
                id: "grandchild".to_string(),
                created_at: "t2".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("child-a".to_string()),
            },
            SessionInfo {
                id: "unrelated".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
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
