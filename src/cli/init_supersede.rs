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
//! at step 1. Step 4 reads the predecessor's manifest from the same prefix,
//! skips the keys this host's copy of the successor already holds with the
//! manifest's hash, reads the rest eight at a time, each checked against
//! that hash, and writes them into the successor, under this directory's
//! prefix, as one push with the marker last. A carry that stops pushes what
//! it wrote, without the marker, so a re-run reads only what is left, and
//! refuses `carry_incomplete` with the failure's class and the counts: exit
//! 1 when running it again can finish the carry, else 3.
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
use crate::engine::types::{generate_session_id, now_iso8601, Event, EventPayload, SupersedeActor};
use crate::session::cloud::{
    check_fence_names, BucketReadFailed, CarryEntry, CarrySourceError, CloudBackend, CloudFence,
    PulledState, FLUSH_PARALLELISM,
};
use crate::session::context::{is_host_local_key, ContextStore};
use crate::session::context_log::{added_event, WRITER_KOTO};
use crate::session::links::{self, is_reserved_key, supersede_fence, MAX_SUPERSEDE_HOPS};
use crate::session::remote_feed::FailureClass;
use crate::session::{state_file_name, Backend, SessionBackend};

/// The key `koto init --supersede` writes in the successor once the carry
/// is complete. A re-run that finds it carries nothing.
pub const CARRY_COMPLETE_KEY: &str = "koto/superseded-carry-complete";

/// A cloud carry that stopped after the fence and the create, before its
/// marker reached the bucket: how far it got and what stopped it. It renders
/// as fixed wording and has no [`std::error::Error::source`], so neither
/// `{e}` nor `{e:#}` prints the cause, which may hold the bucket's own text.
/// [`run`] refuses it as `carry_incomplete`, classed by [`CarryFailure`].
#[derive(Debug)]
pub(crate) struct CarryIncomplete {
    /// How many keys the predecessor's manifest names for the carry; `None`
    /// when the manifest itself couldn't be read or trusted.
    pub keys: Option<usize>,
    /// How many of those this host's copy of the successor holds with the
    /// predecessor's value: the ones it already held, plus `carried`.
    pub held: usize,
    /// How many this run wrote.
    pub carried: usize,
    /// The key whose read or write stopped the carry, taken from the plan
    /// entry, so it has passed `validate_context_key`; `None` when the
    /// manifest read or the push stopped it. It is the only name from the
    /// bucket the refusal shows.
    pub key: Option<String>,
    /// What stopped it: the cloud backend's `BucketReadFailed` or
    /// `CarrySourceError`, or an error marked [`CarryWriteFailed`] or
    /// [`CarryPushFailed`]. It is never rendered.
    pub cause: anyhow::Error,
}

/// The `failure` class of a [`CarryIncomplete`], from its cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarryFailure {
    /// A bucket read failed with no usable answer: a refused connection, a
    /// host that didn't resolve, a 5xx other than 503.
    Unreachable,
    /// A bucket read timed out, or ran the full request timeout.
    Timeout,
    /// A bucket read was answered 429 or 503 through every retry.
    Throttled,
    /// A bucket read was answered 401 or 403.
    Denied,
    /// What the bucket holds for the predecessor doesn't check out.
    InvalidSource,
    /// A write into the successor's local store failed.
    WriteFailed,
    /// Every key and the marker were written, and the push failed.
    PushFailed,
}

impl CarryFailure {
    /// The class of `cause`. The markers [`carry_from_bucket`] attaches are
    /// looked for first. A cause that is none of the known ones can only be
    /// the predecessor's prefix or id failing its check before a key is
    /// built, so it is `invalid_source`, which a retry can't fix.
    fn of(cause: &anyhow::Error) -> Self {
        if cause.downcast_ref::<CarryPushFailed>().is_some() {
            return Self::PushFailed;
        }
        if cause.downcast_ref::<CarryWriteFailed>().is_some() {
            return Self::WriteFailed;
        }
        match cause.downcast_ref::<BucketReadFailed>().map(|r| r.class()) {
            Some(FailureClass::Unreachable) => Self::Unreachable,
            Some(FailureClass::Timeout) => Self::Timeout,
            Some(FailureClass::Throttled) => Self::Throttled,
            Some(FailureClass::Denied) => Self::Denied,
            None => Self::InvalidSource,
        }
    }

    /// The class as the refusal's `failure` names it.
    fn name(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::Throttled => "throttled",
            Self::Denied => "denied",
            Self::InvalidSource => "invalid_source",
            Self::WriteFailed => "write_failed",
            Self::PushFailed => "push_failed",
        }
    }

    /// Whether running the command again after a backoff can finish it.
    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Unreachable | Self::Timeout | Self::Throttled | Self::PushFailed
        )
    }
}

/// The exit of a refusal a retry after a backoff can clear, as koto's exit
/// classes define it.
const EXIT_RETRYABLE: i32 = 1;

/// Refuse `incomplete` as `carry_incomplete`, as [`carry_incomplete`]
/// words it.
fn refuse_carry_incomplete(pred: &str, name: &str, incomplete: &CarryIncomplete) -> ! {
    let (message, exit, extra) = carry_incomplete(pred, name, incomplete);
    refuse("carry_incomplete", message, exit, extra)
}

/// The `carry_incomplete` refusal of `incomplete`: its `error` text, its
/// exit (1 when its class is retryable, else 3) and the body's other
/// fields. The text is fixed wording, the two session names, the counts and
/// at most the validated key; the cause is used only to pick the class.
fn carry_incomplete(
    pred: &str,
    name: &str,
    incomplete: &CarryIncomplete,
) -> (String, i32, serde_json::Value) {
    let class = CarryFailure::of(&incomplete.cause);
    let read_of = match &incomplete.key {
        Some(key) => format!("a bucket read of '{key}'"),
        None => "the bucket read of the predecessor's context manifest".to_string(),
    };
    let what = match class {
        CarryFailure::Unreachable => format!("{read_of} failed"),
        CarryFailure::Timeout => format!("{read_of} timed out"),
        CarryFailure::Throttled => format!("{read_of} was throttled"),
        CarryFailure::Denied => format!("{read_of} was denied"),
        CarryFailure::InvalidSource => {
            let why = match incomplete.cause.downcast_ref::<CarrySourceError>() {
                Some(e) => e.to_string(),
                None => {
                    "the predecessor's workspace prefix or session id can't be trusted".to_string()
                }
            };
            match &incomplete.key {
                Some(key) => format!("{why}, for key '{key}'"),
                None => why,
            }
        }
        CarryFailure::WriteFailed => match &incomplete.key {
            Some(key) => format!("'{key}' couldn't be written into this host's copy of '{name}'"),
            None => format!("a key couldn't be written into this host's copy of '{name}'"),
        },
        CarryFailure::PushFailed => {
            "every key and the marker are written on this host, but the push to the bucket failed"
                .to_string()
        }
    };
    let counts = match incomplete.keys {
        Some(keys) => format!(
            "{} of {keys} keys carried, {} remain",
            incomplete.held,
            keys.saturating_sub(incomplete.held)
        ),
        None => "no key was carried".to_string(),
    };
    let next = match class {
        CarryFailure::Denied => format!(
            "Check that this host's bucket credentials can read '{pred}''s workspace prefix, then \
             run the same command again"
        ),
        CarryFailure::InvalidSource => format!(
            "Retrying at once reads the same data; run the same command again once nothing else \
             writes '{pred}'"
        ),
        CarryFailure::WriteFailed => {
            "Run the same command again once this host's koto store can be written".to_string()
        }
        _ => "Run the same command again to finish it".to_string(),
    };
    let mut extra = serde_json::json!({
        "superseded": pred,
        "name": name,
        "failure": class.name(),
        "retryable": class.retryable(),
    });
    if let (Some(obj), Some(keys)) = (extra.as_object_mut(), incomplete.keys) {
        obj.insert("keys".into(), keys.into());
        obj.insert("held".into(), incomplete.held.into());
        obj.insert(
            "remaining".into(),
            keys.saturating_sub(incomplete.held).into(),
        );
    }
    if let Some(obj) = extra.as_object_mut() {
        obj.insert("carried".into(), incomplete.carried.into());
    }
    let exit = if class.retryable() {
        EXIT_RETRYABLE
    } else {
        super::EXIT_INFRASTRUCTURE
    };
    (
        format!(
            "--supersede: '{pred}' is fenced and '{name}' exists, but the carry stopped: {what}; \
             {counts}. {next}"
        ),
        exit,
        extra,
    )
}

impl std::fmt::Display for CarryIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carry stopped before it finished")
    }
}

impl std::error::Error for CarryIncomplete {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// Marks a [`CarryIncomplete`] cause as a write into the successor's local
/// store that failed.
#[derive(Debug)]
pub(crate) struct CarryWriteFailed;

impl std::fmt::Display for CarryWriteFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a carried key couldn't be written into the successor")
    }
}

/// Marks a [`CarryIncomplete`] cause as the push that failed after every
/// key and the marker were written.
#[derive(Debug)]
pub(crate) struct CarryPushFailed;

impl std::fmt::Display for CarryPushFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the successor's carried keys couldn't be pushed to the bucket")
    }
}

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
    let from_bucket = pulled.as_ref().map(|p| BucketSource {
        prefix: &p.prefix,
        log_names_carried: logs_a_carried_key(&p.events, args.carry),
    });
    let carried = carry(backend, pred, name, args.carry, from_bucket).unwrap_or_else(|e| {
        if let Some(incomplete) = e.downcast_ref::<CarryIncomplete>() {
            refuse_carry_incomplete(pred, name, incomplete);
        }
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

/// Where a cloud carry reads the predecessor.
#[derive(Clone, Copy)]
struct BucketSource<'a> {
    /// The workspace prefix of the predecessor's fenced copy.
    prefix: &'a str,
    /// Whether the predecessor's pulled log records a key the carry takes.
    log_names_carried: bool,
}

/// Whether `events` record a `context_added` for a key the carry would
/// take: one under a `--carry` prefix that is neither koto's own nor
/// host-local. A predecessor whose bucket holds no manifest is refused when
/// its log records one, since its manifest can't have been empty.
fn logs_a_carried_key(events: &[Event], prefixes: &[String]) -> bool {
    events.iter().any(|e| match &e.payload {
        EventPayload::ContextAdded { key, .. } => {
            !is_reserved_key(key)
                && !is_host_local_key(key)
                && prefixes.iter().any(|p| key.starts_with(p.as_str()))
        }
        _ => false,
    })
}

/// Copy every key under each of `prefixes` from `pred` into `succ`, then
/// write [`CARRY_COMPLETE_KEY`] in `succ`. Returns how many keys were
/// written. Skips everything when the marker is already there, reserved
/// keys always, and a key `succ` already holds with the same value. Reads
/// `pred` through the store directly, so the carry logs no read in it;
/// each write in `succ` is logged as a `context_added` by writer `koto`.
///
/// Under the cloud backend `pred` is read from the bucket under its fenced
/// copy's prefix, as [`carry_from_bucket`] describes.
fn carry(
    backend: &Backend,
    pred: &str,
    succ: &str,
    prefixes: &[String],
    from_bucket: Option<BucketSource<'_>>,
) -> Result<usize> {
    if let (Some(cloud), Some(source)) = (backend.as_cloud(), from_bucket) {
        return carry_from_bucket(backend, cloud, pred, succ, prefixes, source);
    }
    let store: &dyn ContextStore = backend;
    if store.ctx_exists(succ, CARRY_COMPLETE_KEY) {
        return Ok(0);
    }
    let mut keys = BTreeSet::new();
    for prefix in prefixes {
        keys.extend(
            store
                .list_keys(pred, Some(prefix))?
                .into_iter()
                .filter(|k| !is_reserved_key(k)),
        );
    }
    let source = keys
        .into_iter()
        .map(|key| store.get(pred, &key).map(|value| (key, value)))
        .collect::<Result<Vec<_>>>()?;
    let mut carried = 0;
    for (key, value) in &source {
        if store.ctx_exists(succ, key) && store.get(succ, key).ok().as_ref() == Some(value) {
            continue;
        }
        write_koto_key(backend, succ, key, value)?;
        carried += 1;
    }
    write_koto_key(
        backend,
        succ,
        CARRY_COMPLETE_KEY,
        &carry_marker(pred, prefixes, source.len())?,
    )?;
    Ok(carried)
}

/// [`carry`] under the cloud backend. A batch an interrupted carry left
/// open is pushed first, so a re-run starts from what the bucket holds, and
/// a marker already there carries nothing. Then `succ`'s writes are held
/// for one push while the predecessor's manifest is read
/// ([`CloudBackend::carry_plan`]); a key this host's copy of `succ` already
/// holds with the manifest's hash is counted as held with no GET, and the
/// rest are read [`FLUSH_PARALLELISM`] at a time and written, in key order,
/// as each chunk lands.
///
/// A read that fails stops the carry after the chunk's other values are
/// written; a write that fails stops it at once. Either way the writes so
/// far are pushed once, best-effort, without the marker, and the error is a
/// [`CarryIncomplete`]. On success
/// the marker is written last and everything goes up in one push; a push
/// that fails is a [`CarryIncomplete`] too, its cause marked
/// [`CarryPushFailed`], and a re-run pushes the open batch first.
fn carry_from_bucket(
    backend: &Backend,
    cloud: &CloudBackend,
    pred: &str,
    succ: &str,
    prefixes: &[String],
    source: BucketSource<'_>,
) -> Result<usize> {
    let store: &dyn ContextStore = backend;
    cloud.flush_interrupted_batch(succ)?;
    if store.ctx_exists(succ, CARRY_COMPLETE_KEY) {
        return Ok(0);
    }
    let guard = cloud.defer_writes_to(succ);
    let plan = match cloud.carry_plan(source.prefix, pred, prefixes, source.log_names_carried) {
        Ok(plan) => plan,
        Err(cause) => {
            drop(guard);
            return Err(CarryIncomplete {
                keys: None,
                held: 0,
                carried: 0,
                key: None,
                cause,
            }
            .into());
        }
    };
    let held = cloud.held_locally(succ, &plan);
    let wanted: Vec<CarryEntry> = plan
        .iter()
        .filter(|e| !held.contains(e.key()))
        .cloned()
        .collect();
    let skipped = plan.len() - wanted.len();

    let mut carried = 0;
    // The key that stopped the carry, when one did, and why.
    let mut stopped: Option<(Option<String>, anyhow::Error)> = None;
    'chunks: for chunk in wanted.chunks(FLUSH_PARALLELISM) {
        let mut failed = None;
        for (entry, fetched) in chunk
            .iter()
            .zip(cloud.fetch_carried(source.prefix, pred, chunk))
        {
            match fetched {
                Ok(value) => {
                    if let Err(e) = write_koto_key(backend, succ, entry.key(), &value) {
                        stopped =
                            Some((Some(entry.key().to_string()), e.context(CarryWriteFailed)));
                        break 'chunks;
                    }
                    carried += 1;
                }
                Err(e) => {
                    failed.get_or_insert((Some(entry.key().to_string()), e));
                }
            }
        }
        if failed.is_some() {
            stopped = failed;
            break;
        }
    }
    if stopped.is_none() {
        let marker = carry_marker(pred, prefixes, plan.len())?;
        if let Err(e) = write_koto_key(backend, succ, CARRY_COMPLETE_KEY, &marker) {
            stopped = Some((
                Some(CARRY_COMPLETE_KEY.to_string()),
                e.context(CarryWriteFailed),
            ));
        }
    }
    drop(guard);
    let incomplete = |key, cause| CarryIncomplete {
        keys: Some(plan.len()),
        held: skipped + carried,
        carried,
        key,
        cause,
    };
    if let Some((key, cause)) = stopped {
        // The same failing network is likely to fail this push too, and a
        // batch it leaves open goes up with the next command on this host.
        // Its failure is dropped, so the class stays the one that stopped
        // the carry.
        let _ = cloud.flush_window(succ);
        return Err(incomplete(key, cause).into());
    }
    if let Err(e) = cloud.flush_window(succ) {
        return Err(incomplete(None, e.context(CarryPushFailed)).into());
    }
    Ok(carried)
}

/// The value of [`CARRY_COMPLETE_KEY`]: where the carry read from, under
/// which prefixes, and how many keys it covered.
fn carry_marker(pred: &str, prefixes: &[String], keys: usize) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "from": pred,
        "prefixes": prefixes,
        "keys": keys,
        "at": now_iso8601(),
    }))?)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn added(key: &str) -> Event {
        let payload = added_event(key, b"v", WRITER_KOTO);
        Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    #[test]
    fn a_log_names_a_carried_key_only_under_a_carry_prefix_and_outside_koto_s_own() {
        let prefixes = vec!["record/".to_string(), "koto".to_string()];
        assert!(logs_a_carried_key(&[added("record/a")], &prefixes));
        assert!(!logs_a_carried_key(&[added("coord/x")], &prefixes));
        assert!(!logs_a_carried_key(
            &[added("koto/superseded-carry-complete")],
            &prefixes
        ));
        assert!(!logs_a_carried_key(&[], &prefixes));
    }

    #[test]
    fn an_incomplete_carry_never_renders_its_cause() {
        let e: anyhow::Error = CarryIncomplete {
            keys: Some(3),
            held: 1,
            carried: 1,
            key: None,
            cause: anyhow::anyhow!("endpoint http://secret.example request id 42")
                .context(CarryPushFailed),
        }
        .into();
        // anyhow's `{:?}` prints the message and the source chain.
        for shown in [format!("{e}"), format!("{e:#}"), format!("{e:?}")] {
            assert!(!shown.contains("secret"), "{shown}");
        }
        assert_eq!(format!("{e:#}"), "the carry stopped before it finished");
        let incomplete = e.downcast_ref::<CarryIncomplete>().unwrap();
        assert!(incomplete.cause.downcast_ref::<CarryPushFailed>().is_some());
    }

    const SECRET: &str = "endpoint http://secret.example request id 42";

    fn read_failed(status: u16) -> anyhow::Error {
        BucketReadFailed {
            key: "7e57000000000000/p/ctx/record/a".to_string(),
            source: s3::error::S3Error::HttpFailWithBody(status, SECRET.to_string()),
            took: std::time::Duration::ZERO,
        }
        .into()
    }

    fn stopped(keys: Option<usize>, key: Option<&str>, cause: anyhow::Error) -> CarryIncomplete {
        CarryIncomplete {
            keys,
            held: if keys.is_some() { 4 } else { 0 },
            carried: if keys.is_some() { 3 } else { 0 },
            key: key.map(str::to_string),
            cause,
        }
    }

    #[test]
    fn an_incomplete_carry_is_classed_by_its_cause() {
        let cases: Vec<(anyhow::Error, CarryFailure)> = vec![
            (read_failed(500), CarryFailure::Unreachable),
            (read_failed(503), CarryFailure::Throttled),
            (read_failed(429), CarryFailure::Throttled),
            (read_failed(403), CarryFailure::Denied),
            (read_failed(401), CarryFailure::Denied),
            (
                CarrySourceError::HashMismatch.into(),
                CarryFailure::InvalidSource,
            ),
            (
                anyhow::anyhow!("'x' isn't a workspace prefix"),
                CarryFailure::InvalidSource,
            ),
            (
                anyhow::anyhow!("disk full").context(CarryWriteFailed),
                CarryFailure::WriteFailed,
            ),
            // A push's cause may hold anything; the marker decides.
            (
                read_failed(503).context(CarryPushFailed),
                CarryFailure::PushFailed,
            ),
        ];
        for (cause, class) in cases {
            assert_eq!(CarryFailure::of(&cause), class, "{cause:#}");
        }
        let retryable: Vec<&str> = [
            CarryFailure::Unreachable,
            CarryFailure::Timeout,
            CarryFailure::Throttled,
            CarryFailure::Denied,
            CarryFailure::InvalidSource,
            CarryFailure::WriteFailed,
            CarryFailure::PushFailed,
        ]
        .into_iter()
        .filter(|c| c.retryable())
        .map(CarryFailure::name)
        .collect();
        assert_eq!(
            retryable,
            ["unreachable", "timeout", "throttled", "push_failed"]
        );
    }

    #[test]
    fn a_retryable_carry_failure_exits_1_with_its_counts() {
        let (message, exit, body) = carry_incomplete(
            "p",
            "s",
            &stopped(Some(5), Some("record/a"), read_failed(503)),
        );
        assert_eq!(exit, 1);
        assert_eq!(
            body,
            serde_json::json!({
                "superseded": "p", "name": "s", "failure": "throttled", "retryable": true,
                "keys": 5, "held": 4, "remaining": 1, "carried": 3,
            })
        );
        assert_eq!(
            message,
            "--supersede: 'p' is fenced and 's' exists, but the carry stopped: a bucket read of \
             'record/a' was throttled; 4 of 5 keys carried, 1 remain. Run the same command \
             again to finish it"
        );
    }

    #[test]
    fn a_carry_whose_plan_failed_leaves_its_counts_out_and_exits_3() {
        let (message, exit, body) = carry_incomplete(
            "p",
            "s",
            &stopped(None, None, CarrySourceError::BadKey.into()),
        );
        assert_eq!(exit, 3);
        assert_eq!(
            body,
            serde_json::json!({
                "superseded": "p", "name": "s", "failure": "invalid_source",
                "retryable": false, "carried": 0,
            })
        );
        assert!(
            message.contains(&CarrySourceError::BadKey.to_string()),
            "{message}"
        );
        assert!(message.contains("no key was carried"), "{message}");
    }

    /// Whatever the class, the text holds none of the cause's own text.
    #[test]
    fn a_carry_incomplete_refusal_never_shows_its_cause() {
        for keys in [None, Some(5)] {
            let causes = [
                read_failed(500),
                read_failed(403),
                anyhow::anyhow!(SECRET),
                anyhow::anyhow!(SECRET).context(CarryWriteFailed),
                anyhow::anyhow!(SECRET).context(CarryPushFailed),
            ];
            for cause in causes {
                let key = keys.map(|_| "record/a");
                let (message, _, body) = carry_incomplete("p", "s", &stopped(keys, key, cause));
                assert!(!message.contains("secret"), "{message}");
                assert!(!body.to_string().contains("secret"), "{body}");
            }
        }
    }
}
