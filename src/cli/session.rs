use anyhow::Result;

use crate::cli::ChildrenPolicy;
use crate::session::cloud::{ChildResolution, CloudBackend};
use crate::session::Backend;
use crate::session::SessionBackend;

/// Print the absolute session directory path.
pub fn handle_dir(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    let dir = backend.session_dir(name);
    println!("{}", dir.display());
    Ok(())
}

/// Print all sessions as a JSON array.
pub fn handle_list(backend: &dyn SessionBackend) -> Result<()> {
    let sessions = backend.list()?;
    println!("{}", serde_json::to_string_pretty(&sessions)?);
    Ok(())
}

/// Remove a session directory. Idempotent: succeeds even if the session doesn't exist.
pub fn handle_cleanup(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    backend.cleanup(name)?;
    Ok(())
}

/// Resolve a session version conflict by keeping either the local or
/// remote state. Under `CloudBackend`, a `--children` policy reconciles
/// the parent's direct children using the strict-prefix rule (default)
/// or an explicit side-selection.
///
/// `sync_status` and `machine_id` fields in the JSON response are
/// emitted only under `CloudBackend` per Decision 12 Q5 of the batch
/// child-spawning design — under `LocalBackend` those fields have no
/// meaningful value and are elided.
pub fn handle_resolve(
    backend: &Backend,
    name: &str,
    keep: &str,
    children: ChildrenPolicy,
) -> Result<()> {
    match keep {
        "local" | "remote" => {}
        other => anyhow::bail!(
            "invalid --keep value: '{}'. Must be 'local' or 'remote'.",
            other
        ),
    }

    let cloud = match backend {
        Backend::Cloud(c) => c,
        Backend::Local(_) => {
            anyhow::bail!("session resolve requires cloud backend (session.backend = \"cloud\")")
        }
    };

    // Push-parent-first ordering: parent reconciliation commits before
    // we touch any child. `resolve_conflict` runs the parent leg and
    // pushes to S3 first; only after that succeeds do we enumerate
    // children and apply the policy. This preserves Decision 12 Q6:
    // children never appear "ahead" of the parent log on S3.
    cloud.resolve_conflict(name, keep)?;

    let children_result = apply_children_policy(cloud, backend, name, children);

    let machine_id = crate::session::version::get_or_create_machine_id()?;
    let sync_status = parent_sync_status_label(cloud, name);

    let response = serde_json::json!({
        "name": name,
        "keep": keep,
        "children_policy": children_policy_label(children),
        "sync_status": sync_status,
        "machine_id": machine_id,
        "children": children_result,
    });

    println!("{}", serde_json::to_string_pretty(&response)?);

    // Return an error only if the `auto` policy hit at least one
    // true conflict — that's the case the design reserves for per-child
    // `koto session resolve <child>`. Errored children are reported in
    // the JSON body but do not abort (mirrors per-task spawn-error
    // accumulation elsewhere in the codebase).
    if children == ChildrenPolicy::Auto
        && children_result
            .iter()
            .any(|r| matches!(r.resolution, ChildResolution::Conflict))
    {
        anyhow::bail!(
            "one or more children are in conflict; run `koto session resolve <child>` on each \
             flagged child"
        );
    }

    Ok(())
}

/// Per-child row in the response body, pairing the child's name with
/// its reconciliation outcome. Kept a flat struct so the JSON shape is
/// `{"name": "...", "action": "...", "message": "..."}` rather than a
/// nested object.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildResolutionRow {
    /// Child session name (fully-qualified `<parent>.<task>`).
    pub name: String,
    /// Flattened `ChildResolution` so the JSON is a single object.
    #[serde(flatten)]
    pub resolution: ChildResolution,
}

/// Enumerate the parent's direct children and apply `policy` to each.
///
/// Runs under `CloudBackend` only; the `handle_resolve` caller has
/// already verified that. Errors from the per-child call surface as
/// `ChildResolution::Errored` entries so siblings continue to process.
fn apply_children_policy(
    cloud: &CloudBackend,
    backend: &Backend,
    parent: &str,
    policy: ChildrenPolicy,
) -> Vec<ChildResolutionRow> {
    let policy_str = children_policy_label(policy);

    let sessions = match backend.list() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: session resolve: failed to enumerate children: {}",
                e
            );
            return Vec::new();
        }
    };

    sessions
        .into_iter()
        .filter(|s| s.parent_workflow.as_deref() == Some(parent))
        .map(|child| {
            let resolution = cloud.reconcile_child(&child.id, policy_str);
            ChildResolutionRow {
                name: child.id,
                resolution,
            }
        })
        .collect()
}

/// Map the `ChildrenPolicy` enum to the wire string accepted by
/// `CloudBackend::reconcile_child` and echoed in the JSON response.
fn children_policy_label(p: ChildrenPolicy) -> &'static str {
    match p {
        ChildrenPolicy::Auto => "auto",
        ChildrenPolicy::Skip => "skip",
        ChildrenPolicy::AcceptRemote => "accept-remote",
        ChildrenPolicy::AcceptLocal => "accept-local",
    }
}

/// Best-effort summary of the parent's post-reconciliation sync state.
///
/// After `resolve_conflict` succeeds, local and remote converge on the
/// same version, so the expected label is `"fresh"`. We still probe
/// remote to detect the narrow case where S3 is unreachable after the
/// parent leg wrote locally; in that case we surface `"local_only"` so
/// the caller doesn't mistake an offline push-back for a clean sync.
fn parent_sync_status_label(cloud: &CloudBackend, name: &str) -> &'static str {
    if cloud.remote_state_exists(name) {
        "fresh"
    } else {
        "local_only"
    }
}
