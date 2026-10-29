//! `koto init SUCC --template T --continues PRED --supersede [--carry
//! PREFIX]... [--expect-state STATE]`: open SUCC as the session that takes
//! PRED's work over, and fence PRED so nothing drives it again.
//!
//! The command runs five steps in order, and the successor's session id is
//! minted before anything is written:
//!
//! 1. **Check.** PRED's log is read. A fence already naming SUCC (same name
//!    and session id, or SUCC's name when SUCC doesn't exist yet: a crash
//!    between the fence and the create) makes this a re-run, which goes on
//!    to step 3. A fence naming another session, a state other than
//!    `--expect-state`, SUCC being PRED (`self`) or a supersede loop
//!    (`cycle`) refuse, exit 2, with nothing written.
//! 2. **Fence.** A `session_superseded` event naming SUCC is appended to
//!    PRED's own log under its append lock, waited for at most
//!    [`FENCE_LOCK_WAIT`]: a lock still held then exits 3
//!    `fence_unavailable`, nothing written. The log is checked again under
//!    the lock, so of two successors the first append wins.
//! 3. **Create.** SUCC is created with the minted id and its continues
//!    relation, unless an earlier run of this command already created it.
//! 4. **Carry.** Every key under each `--carry` prefix is copied from PRED
//!    into SUCC, skipping keys SUCC already holds with the same value, and
//!    then `koto/superseded-carry-complete` is written in SUCC. A re-run
//!    that finds the marker carries nothing.
//! 5. **Ring.** PRED's wake file is rung once, best-effort, so a waiter on
//!    PRED looks again and finds the fence.
//!
//! Nothing is created before the fence, so every refusal writes nothing.
//!
//! Under the cloud backend the predecessor usually lives under another
//! workspace's prefix, and steps 1 and 2 work on the bucket's copy there,
//! not on this host's: its state is pulled with its ETag (from the
//! `--continues-from` prefix, else the one its local copy records), checked,
//! and pushed back with the fence appended, conditional on that ETag. A
//! refused or failed conditional push, or a bucket that can't write
//! conditionally, exits 3 `fence_unavailable` with nothing written; there
//! is no unconditional fallback. Of two successors the first push wins; the
//! other's push is refused, it reads the fence, and it refuses as it would
//! at step 1. Step 4 reads the carried keys from the same prefix (one GET of
//! the manifest, one per key) and writes them into the successor, under
//! this directory's prefix, as one push.
//!
//! Names read from a log or the bucket (a fence's successor and session id,
//! a home prefix, a carried key) are validated before anything is built
//! from them.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::engine::persistence::{
    append_supersede_fence, derive_state_from_log, superseded_by, FenceOutcome, SupersedeFence,
    FENCE_LOCK_WAIT,
};
use crate::engine::types::{generate_session_id, now_iso8601, SupersedeActor};
use crate::session::cloud::{check_fence_names, CloudBackend, CloudFence, PulledState};
use crate::session::context::ContextStore;
use crate::session::context_log::{added_event, WRITER_KOTO};
use crate::session::links::{self, is_reserved_key, supersede_fence, MAX_SUPERSEDE_HOPS};
use crate::session::{state_file_name, Backend, SessionBackend};

/// The key `koto init --supersede` writes in the successor once the carry
/// is complete. A re-run that finds it carries nothing.
pub const CARRY_COMPLETE_KEY: &str = "koto/superseded-carry-complete";

/// The arguments of one `koto init --supersede`.
pub(crate) struct SupersedeArgs<'a> {
    pub name: &'a str,
    pub template: &'a str,
    pub vars: &'a [String],
    pub intent: Option<&'a str>,
    pub dispatched_by: Option<&'a str>,
    pub pred: &'a str,
    pub continues_from: Option<&'a str>,
    pub execution_dir: Option<&'a Path>,
    pub legacy_environment: bool,
    pub carry: &'a [String],
    pub expect_state: Option<&'a str>,
}

/// Print a refusal, `{"error", "command": "init", "code", ...extra}`, and
/// exit with `exit`.
fn refuse(code: &str, message: String, exit: i32, extra: serde_json::Value) -> ! {
    let mut body = serde_json::json!({
        "error": message,
        "command": "init",
        "code": code,
    });
    if let (Some(obj), Some(more)) = (body.as_object_mut(), extra.as_object()) {
        obj.extend(more.clone());
    }
    super::exit_with_error_code(body, exit)
}

/// The refusal for a predecessor another session already superseded.
fn refuse_superseded_by(backend: &Backend, pred: &str, fence: &SupersedeFence) -> ! {
    let newest = links::newest_member(backend, pred)
        .map(|s| s.newest)
        .unwrap_or_else(|| fence.by.clone());
    refuse(
        "superseded_by",
        format!(
            "session '{pred}' is already superseded: superseded_by={}; nothing was written. \
             Continue in '{newest}'",
            fence.by
        ),
        super::EXIT_CALLER_ERROR,
        serde_json::json!({
            "superseded_by": fence.by,
            "newest": newest,
            "actor": fence.actor,
        }),
    )
}

/// Who is running this command, from the environment.
fn actor() -> SupersedeActor {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    SupersedeActor {
        claude_session: var("CLAUDE_CODE_SESSION_ID"),
        host: Some(crate::session::version::get_hostname()).filter(|h| h != "unknown"),
        user: var("USER").or_else(|| var("LOGNAME")),
    }
}

/// Whether making `succ` supersede `pred` would close a loop: `pred`
/// continues `succ` (directly or through its predecessors), or `succ`'s own
/// supersede chain leads to `pred`. Only an existing `succ` can be in
/// either.
fn closes_a_loop(backend: &Backend, succ: &str, pred: &str) -> bool {
    // Backward, through the continues relations PRED's chain records.
    let mut visited = HashSet::from([pred.to_string()]);
    let mut at = pred.to_string();
    for _ in 0..MAX_SUPERSEDE_HOPS {
        let Some(before) = links::continues_of(backend, &at)
            .ok()
            .and_then(|p| p.into_iter().next())
        else {
            break;
        };
        if before == succ {
            return true;
        }
        if !visited.insert(before.clone()) {
            break;
        }
        at = before;
    }
    // Forward, through the fences SUCC's chain holds.
    let mut visited = HashSet::from([succ.to_string()]);
    let mut at = succ.to_string();
    for _ in 0..MAX_SUPERSEDE_HOPS {
        let Some(fence) = supersede_fence(backend, &at) else {
            break;
        };
        if fence.by == pred {
            return true;
        }
        if !visited.insert(fence.by.clone()) {
            break;
        }
        at = fence.by;
    }
    false
}

/// Run `koto init --supersede`. Prints one JSON object on success; every
/// refusal prints its error and exits.
pub(crate) fn run(backend: &Backend, args: &SupersedeArgs<'_>) -> Result<()> {
    let SupersedeArgs { name, pred, .. } = *args;
    if let Err(msg) = crate::discover::validate_workflow_name(name) {
        refuse(
            "invalid_usage",
            msg,
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    if name == pred {
        refuse(
            "self",
            format!("--supersede: session '{name}' can't supersede itself; nothing was written"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    if let Some(prefix) = args.carry.iter().find(|p| is_reserved_key(p)) {
        refuse(
            "invalid_usage",
            format!(
                "--carry {prefix}: keys under '{}' are koto's own and are never carried",
                links::RESERVED_PREFIX
            ),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    // The predecessor's name, existence and kind, checked before anything
    // is written, as plain `--continues` checks them.
    let target = super::session::resolve_continues(backend, name, pred, args.continues_from)
        .unwrap_or_else(|e| {
            super::exit_with_error_code(
                serde_json::json!({"error": e.message, "command": "init"}),
                e.exit,
            )
        });

    // 1. Check: the bucket's copy under the cloud backend, else the log.
    let pred_log = backend.session_dir(pred).join(state_file_name(pred));
    let pulled = backend
        .as_cloud()
        .map(|cloud| pull_pred(cloud, pred, args.continues_from, &target));
    let pred_events = match &pulled {
        Some(pulled) => pulled.events.clone(),
        None => {
            backend
                .read_events_local(pred)
                .unwrap_or_else(|e| {
                    refuse(
                        "persistence_error",
                        format!(
                            "--supersede: could not read '{pred}''s log: {e:#}; nothing was \
                             written"
                        ),
                        super::EXIT_INFRASTRUCTURE,
                        serde_json::Value::Null,
                    )
                })
                .1
        }
    };
    let succ_exists = backend.exists(name);
    let succ_session_id = || {
        backend
            .read_header(name)
            .ok()
            .map(|h| h.session_id)
            .unwrap_or_default()
    };
    // `Some(id)` when an earlier run of this command already fenced PRED
    // for this successor: the id the successor has, or is created with.
    let rerun_id = match superseded_by(&pred_events).map(|f| trusted(pred, f)) {
        Some(fence) if fence.by == name => {
            if succ_exists && succ_session_id() != fence.by_session_id {
                // A session of the same name, but not the one the fence
                // names: another session, not this command's earlier run.
                refuse_superseded_by(backend, pred, &fence);
            }
            Some(fence.by_session_id)
        }
        Some(fence) => refuse_superseded_by(backend, pred, &fence),
        None => None,
    };

    let fenced_now = rerun_id.is_none();
    let session_id = match rerun_id {
        Some(id) => id,
        None => {
            if let Some(expected) = args.expect_state {
                let actual = derive_state_from_log(&pred_events).unwrap_or_default();
                if actual != expected {
                    refuse(
                        "stale_state",
                        format!(
                            "--expect-state: session '{pred}' is in state '{actual}', not \
                             '{expected}'; nothing was written"
                        ),
                        super::EXIT_CALLER_ERROR,
                        serde_json::json!({"expected": expected, "actual": actual}),
                    );
                }
            }
            if succ_exists {
                if closes_a_loop(backend, name, pred) {
                    refuse(
                        "cycle",
                        format!(
                            "--supersede: '{name}' superseding '{pred}' would close a loop: \
                             '{pred}' already descends from '{name}'; nothing was written"
                        ),
                        super::EXIT_CALLER_ERROR,
                        serde_json::Value::Null,
                    );
                }
                refuse(
                    "session_exists",
                    format!(
                        "--supersede: session '{name}' already exists, and no earlier run of \
                         this command opened it; pick a new name. Nothing was written"
                    ),
                    super::EXIT_CALLER_ERROR,
                    serde_json::Value::Null,
                );
            }
            // 2. Fence, with the successor's id minted first.
            let id = generate_session_id();
            let fence = SupersedeFence {
                by: name.to_string(),
                by_session_id: id.clone(),
                actor: actor(),
                at: now_iso8601(),
            };
            match &pulled {
                Some(pulled) => fence_in_bucket(backend, pred, pulled, &fence, id),
                None => fence_locally(backend, pred, &pred_log, &fence, id),
            }
        }
    };

    // 3. Create, unless an earlier run already did.
    let mut environment_report = None;
    if !backend.exists(name) {
        let (_, report) = super::create_session(
            backend,
            name,
            args.template,
            args.vars,
            None,
            args.intent,
            args.execution_dir,
            args.legacy_environment,
            args.dispatched_by,
            Some(&target),
            Some(&session_id),
        )?;
        environment_report = report;
    } else {
        // A crash between the create and the relation leaves it owed; a
        // relation already held changes nothing.
        super::session::record_continues_at_init(backend, name, &target);
        if let Some(dispatcher) = args.dispatched_by {
            super::session::record_dispatched_by(backend, name, dispatcher);
        }
    }

    // 4. Carry.
    let pred_prefix = pulled.as_ref().map(|p| p.prefix.as_str());
    let carried = carry(backend, pred, name, args.carry, pred_prefix).unwrap_or_else(|e| {
        refuse(
            "persistence_error",
            format!(
                "--supersede: '{pred}' is fenced and '{name}' exists, but the carry failed: \
                 {e:#}. Run the same command again to finish it"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        )
    });

    // 5. Ring, once, when this run wrote the fence.
    if fenced_now {
        if let Some(root) = crate::koto_home::koto_home() {
            let _ = crate::engine::wake_signal::ring(&root, pred);
        }
    }

    let (_, events) = backend
        .read_events(name)
        .map_err(|e| anyhow::anyhow!("failed to read '{name}': {e}"))?;
    let mut out = serde_json::json!({
        "name": name,
        "state": derive_state_from_log(&events).unwrap_or_default(),
        "outcome": if fenced_now { "superseded" } else { "unchanged" },
        "superseded": pred,
        "carried": carried,
        "forward": name,
    });
    super::add_environment_report(&mut out, environment_report.as_ref());
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// Step 2 on the local backend: append the fence to `pred`'s own log under
/// its append lock. Returns the successor's session id, or refuses.
fn fence_locally(
    backend: &Backend,
    pred: &str,
    pred_log: &Path,
    fence: &SupersedeFence,
    id: String,
) -> String {
    match append_supersede_fence(pred_log, fence, &fence.at, FENCE_LOCK_WAIT) {
        Ok(FenceOutcome::Fenced { .. }) | Ok(FenceOutcome::AlreadyOurs) => id,
        Ok(FenceOutcome::SupersededByOther(other)) => refuse_superseded_by(backend, pred, &other),
        Ok(FenceOutcome::Unavailable) => refuse(
            "fence_unavailable",
            format!(
                "--supersede: '{pred}''s log stayed locked by another writer for {} \
                 seconds; nothing was written. Run the command again",
                FENCE_LOCK_WAIT.as_secs()
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
        Err(e) => refuse(
            "persistence_error",
            format!("--supersede: could not fence '{pred}': {e:#}; nothing was written"),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
    }
}

/// Step 2 on the cloud backend: push the fence into the pulled copy,
/// conditional on its ETag. Returns the successor's session id, or refuses:
/// a rival's fence as `superseded_by`, and anything that isn't a confirmed
/// conditional push exits 3 `fence_unavailable`, nothing written.
fn fence_in_bucket(
    backend: &Backend,
    pred: &str,
    pulled: &PulledState,
    fence: &SupersedeFence,
    id: String,
) -> String {
    // `pulled` is only ever set under the cloud backend.
    let Some(cloud) = backend.as_cloud() else {
        unreachable!("a pulled copy is the cloud backend's");
    };
    match cloud.push_fence(pulled, fence) {
        CloudFence::Fenced => id,
        CloudFence::Taken(other)
            if other.by == fence.by && other.by_session_id == fence.by_session_id =>
        {
            id
        }
        CloudFence::Taken(other) => refuse_superseded_by(backend, pred, &trusted(pred, other)),
        CloudFence::Unavailable(reason) => refuse(
            "fence_unavailable",
            format!(
                "--supersede: '{pred}' couldn't be fenced in the bucket: {reason}; nothing was \
                 written. Run the command again"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
    }
}

/// Copy every key under each of `prefixes` from `pred` into `succ`, then
/// write [`CARRY_COMPLETE_KEY`] in `succ`. Returns how many keys were
/// written. Skips everything when the marker is already there, reserved
/// keys always, and a key `succ` already holds with the same value. Reads
/// `pred` through the store directly, so the carry logs no read in it;
/// each write in `succ` is logged as a `context_added` by writer `koto`.
///
/// Under the cloud backend `pred` is read from the bucket under
/// `pred_prefix`, its fenced copy's prefix, and `succ`'s writes are held
/// and pushed together once the marker is written. A batch an interrupted
/// carry left open is pushed first, so a re-run sees what it wrote.
fn carry(
    backend: &Backend,
    pred: &str,
    succ: &str,
    prefixes: &[String],
    pred_prefix: Option<&str>,
) -> Result<usize> {
    let store: &dyn ContextStore = backend;
    let cloud = backend.as_cloud();
    if let Some(cloud) = cloud {
        cloud.flush_interrupted_batch(succ)?;
    }
    if store.ctx_exists(succ, CARRY_COMPLETE_KEY) {
        return Ok(0);
    }
    let source = match (cloud, pred_prefix) {
        (Some(cloud), Some(prefix)) => cloud.carry_source(prefix, pred, prefixes)?,
        _ => {
            let mut keys = BTreeSet::new();
            for prefix in prefixes {
                keys.extend(
                    store
                        .list_keys(pred, Some(prefix))?
                        .into_iter()
                        .filter(|k| !is_reserved_key(k)),
                );
            }
            keys.into_iter()
                .map(|key| store.get(pred, &key).map(|value| (key, value)))
                .collect::<Result<Vec<_>>>()?
        }
    };
    let held = cloud.map(|cloud| cloud.defer_writes_to(succ));
    let mut carried = 0;
    for (key, value) in &source {
        if store.ctx_exists(succ, key) && store.get(succ, key).ok().as_ref() == Some(value) {
            continue;
        }
        write_koto_key(backend, succ, key, value)?;
        carried += 1;
    }
    let marker = serde_json::to_vec(&serde_json::json!({
        "from": pred,
        "prefixes": prefixes,
        "keys": source.len(),
        "at": now_iso8601(),
    }))?;
    write_koto_key(backend, succ, CARRY_COMPLETE_KEY, &marker)?;
    drop(held);
    if let Some(cloud) = cloud {
        cloud.flush_window(succ)?;
    }
    Ok(carried)
}

/// `fence`, read from `pred`'s log or its bucket copy, once its names check
/// out. One naming an invalid successor or session id is refused, exit 2
/// `invalid_fence`, with nothing written, before any path or key is built
/// from it.
fn trusted(pred: &str, fence: SupersedeFence) -> SupersedeFence {
    if let Err(e) = check_fence_names(&fence) {
        refuse(
            "invalid_fence",
            format!("--supersede: '{pred}''s log can't be trusted: {e:#}; nothing was written"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    fence
}

/// Pull `pred`'s state from the bucket for the fence: from the
/// `--continues-from` workspace when one was named, else from the prefix
/// this host's copy records as its home, else from the prefix it was looked
/// up under. A copy that can't be read exits 3, and a prefix holding none
/// exits 3 `fence_unavailable`; either way nothing is written.
fn pull_pred(
    cloud: &CloudBackend,
    pred: &str,
    continues_from: Option<&str>,
    target: &super::session::ContinuesTarget,
) -> PulledState {
    let prefix = match (continues_from, target.prefix()) {
        (Some(_), Some(prefix)) => prefix.to_string(),
        (_, looked_up) => cloud
            .home_prefix(pred)
            .map(|home| home.prefix)
            .ok()
            .or_else(|| looked_up.map(str::to_string))
            .unwrap_or_else(|| cloud.workspace_prefix().to_string()),
    };
    match cloud.pull_for_fence(&prefix, pred) {
        Ok(Some(pulled)) => pulled,
        Ok(None) => refuse(
            "fence_unavailable",
            format!(
                "--supersede: the bucket holds no state of '{pred}' under its workspace prefix, \
                 so it can't be fenced there; nothing was written"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
        Err(e) => refuse(
            "persistence_error",
            format!(
                "--supersede: could not pull '{pred}''s state from the bucket: {e:#}; nothing \
                 was written"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
    }
}

/// Write `key` in `session` with writer `koto` and log it there.
fn write_koto_key(backend: &Backend, session: &str, key: &str, value: &[u8]) -> Result<()> {
    let store: &dyn ContextStore = backend;
    store.add_with_writer(session, key, value, WRITER_KOTO)?;
    backend.append_event(
        session,
        &added_event(key, value, WRITER_KOTO),
        &now_iso8601(),
    )
}
