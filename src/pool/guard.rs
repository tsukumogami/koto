//! The pool guard a tick runs before a pooled action
//! (DESIGN-host-resource-pools.md Decision 3).
//!
//! It runs before the action and before any gate. A pool the user config
//! doesn't define imposes nothing and touches nothing on disk (PRD R6). For a
//! defined pool the guard joins the queue, waits inside the tick for up to
//! `wait_secs` for a free slot (R15), and either takes one or reports the
//! session queued, so the tick stops without running the action or the gates
//! and without failing the run.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::config::{self, PoolLookup, PoolSpec};
use super::store::{self, DirUnavailable, Holder, Slot};
use crate::engine::types::{Event, EventPayload, PoolQueued, PoolReleased};

/// How long the in-tick wait sleeps between attempts.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The most `retry_after_secs` a queued report carries (PRD R16).
pub const MAX_RETRY_AFTER_SECS: u32 = 15;

/// Who is asking for a slot.
#[derive(Debug, Clone)]
pub struct Requester {
    /// The koto root the pool lives under.
    pub koto_root: PathBuf,
    pub session: String,
    /// The workspace directory's name.
    pub workspace: String,
    pub state: String,
    /// The session's log says it is waiting in this pool's queue on this
    /// visit to `state` ([`waiting_in_visit`]). The queue can't say so
    /// itself once a sweep has dropped the entry, and the log is how a
    /// lapsed waiter learns it lost its place.
    pub was_waiting: bool,
    /// What a slot taken now is held for, written into its holder record
    /// and carried by the nesting marker: the background run's id, or a
    /// fresh id for an ordinary action's tick.
    pub holder_id: String,
    /// Whether a nesting marker that verifies lets this action run on the
    /// outer slot. A background action never does: its runner would outlive
    /// the slot it borrowed, so it queues as its own session.
    pub may_nest: bool,
}

/// The nesting marker: `<pool>/<slot index>/<holder id>` of the slot the
/// enclosing action holds (DESIGN-host-resource-pools.md, "Nesting").
pub const POOL_HELD_ENV: &str = "KOTO_POOL_HELD";
/// The koto root the marker's pool lives under, on its own so a root path
/// with any character is unambiguous.
pub const POOL_ROOT_ENV: &str = "KOTO_POOL_ROOT";

/// The marker for slot `index` of `pool` under `root`, held for
/// `holder_id`, as the variables a command run on that slot gets.
pub fn marker_vars(
    pool: &str,
    index: usize,
    holder_id: &str,
    root: &Path,
) -> Vec<(String, String)> {
    vec![
        (
            POOL_HELD_ENV.to_string(),
            format!("{pool}/{index}/{holder_id}"),
        ),
        (
            POOL_ROOT_ENV.to_string(),
            root.to_string_lossy().into_owned(),
        ),
    ]
}

/// Whether two koto roots are the same directory.
fn same_root(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The slot index this process may run `pool`'s action on because an
/// enclosing action holds it: the marker (`held`, `root`) names `pool`
/// under `koto_root`, and the named slot is held by a record naming the
/// marker's holder id. A marker that doesn't verify (another pool or root,
/// a free slot, a different holder, a malformed value) grants nothing, so a
/// stale marker left in a daemon's environment is ignored once its run has
/// ended.
pub fn verify_marker(
    held: Option<&str>,
    root: Option<&str>,
    pool: &str,
    koto_root: &Path,
) -> Option<usize> {
    let (held, root) = (held?, root?);
    let mut parts = held.splitn(3, '/');
    let (name, index, id) = (parts.next()?, parts.next()?, parts.next()?);
    if name != pool || id.is_empty() || root.is_empty() {
        return None;
    }
    let index: usize = index.parse().ok()?;
    if !same_root(Path::new(root), koto_root) {
        return None;
    }
    let holder = store::held_slot(koto_root, pool, index)?;
    (holder.run_id.as_deref() == Some(id)).then_some(index)
}

/// [`verify_marker`] on this process's environment.
fn inherited_slot(pool: &str, koto_root: &Path) -> Option<usize> {
    let held = std::env::var(POOL_HELD_ENV).ok();
    let root = std::env::var(POOL_ROOT_ENV).ok();
    verify_marker(held.as_deref(), root.as_deref(), pool, koto_root)
}

/// What the guard's queue edits found, beyond its outcome, so the tick can
/// record it (PRD R26).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueNote {
    /// The session was waiting but its entry was gone: it lapsed (R12).
    pub lapsed: bool,
    /// The session's entry is in the queue as the guard returns.
    pub queued: bool,
    /// The session was waiting on a pool the config no longer defines (R8).
    pub pool_removed: bool,
}

/// The guard's decision and what it found in the queue.
#[derive(Debug)]
pub struct Guarded {
    pub outcome: GuardOutcome,
    pub note: QueueNote,
}

impl Guarded {
    /// The events the tick appends for what happened to the session's place
    /// in the queue, before anything about a slot: `pool_released` with
    /// `pool_removed` or `lapsed` for a wait that ended without a slot, and
    /// `pool_queued` when the session is waiting now and wasn't before (a
    /// lapsed waiter queued again at the back counts as joining).
    pub fn queue_events(&self, pool: &str, requester: &Requester) -> Vec<EventPayload> {
        let released = |reason: &str| {
            EventPayload::PoolReleased(PoolReleased {
                state: requester.state.clone(),
                pool: pool.to_string(),
                reason: reason.to_string(),
                run_id: None,
            })
        };
        let mut events = Vec::new();
        if self.note.pool_removed {
            events.push(released("pool_removed"));
        }
        if self.note.lapsed {
            events.push(released("lapsed"));
        }
        if self.note.queued && (!requester.was_waiting || self.note.lapsed) {
            events.push(EventPayload::PoolQueued(PoolQueued {
                state: requester.state.clone(),
                pool: pool.to_string(),
                workspace: requester.workspace.clone(),
            }));
        }
        events
    }
}

/// The latest of `pool_queued`, `pool_acquired` and `pool_released` naming
/// `pool` in `events`.
fn latest_pool_event<'a>(events: &'a [Event], pool: &str) -> Option<&'a EventPayload> {
    events.iter().rev().map(|e| &e.payload).find(|p| match p {
        EventPayload::PoolQueued(q) => q.pool == pool,
        EventPayload::PoolAcquired(a) => a.pool == pool,
        EventPayload::PoolReleased(r) => r.pool == pool,
        _ => false,
    })
}

/// Whether the log leaves the session waiting on `pool` in its current
/// visit to `state`: the visit's latest event naming the pool is
/// `pool_queued`.
pub fn waiting_in_visit(events: &[Event], state: &str, pool: &str) -> bool {
    let visit = crate::engine::persistence::epoch_slice(events, state);
    matches!(
        latest_pool_event(visit, pool),
        Some(EventPayload::PoolQueued(q)) if q.state == state
    )
}

/// Every wait the log leaves open: for each pool, its latest event, when
/// that is `pool_queued`. A session that leaves the state such a wait was
/// recorded in leaves the queue (R12).
pub fn open_waits(events: &[Event]) -> Vec<PoolQueued> {
    let mut pools: Vec<&str> = Vec::new();
    for e in events {
        if let EventPayload::PoolQueued(q) = &e.payload {
            if !pools.contains(&q.pool.as_str()) {
                pools.push(&q.pool);
            }
        }
    }
    pools
        .into_iter()
        .filter_map(|pool| match latest_pool_event(events, pool) {
            Some(EventPayload::PoolQueued(q)) => Some(q.clone()),
            _ => None,
        })
        .collect()
}

/// What the guard decided.
#[derive(Debug)]
pub enum GuardOutcome {
    /// The pool is undefined: run the action as if `pool` were absent.
    Unpooled,
    /// A slot was taken; hold it while the action runs.
    Acquired(Slot),
    /// An enclosing action already holds slot `index` of this pool, and its
    /// marker verified: run on that slot, taking none and recording no
    /// `pool_acquired` (PRD R13). `marker` is the variables to pass on, so
    /// an action nested deeper verifies the same way.
    Nested {
        index: usize,
        marker: Vec<(String, String)>,
    },
    /// No slot came free within `wait_secs`. The value is the `__pool__`
    /// condition's `output`.
    Queued(serde_json::Value),
    /// The pool's config entry is invalid (`pool_config_invalid`).
    ConfigInvalid {
        config_path: PathBuf,
        key: String,
        reason: String,
    },
    /// The pool's directory can't be used (`pool_dir_unavailable`).
    DirUnavailable(DirUnavailable),
    /// The pool is defined, but this platform doesn't run pools
    /// (`pool_unsupported_platform`, PRD R27).
    UnsupportedPlatform,
}

impl GuardOutcome {
    /// The reserved `__pool__` condition a tick stops on for this outcome,
    /// or `None` when the action may run (unpooled or acquired).
    ///
    /// A queued session is a wait: the outcome is `Pending`, which renders
    /// as a temporal, non-actionable condition. An invalid config or an
    /// unusable directory is an `Error` naming its code, so the action runs
    /// nothing until the operator fixes it (PRD R7, R14).
    pub fn blocking_condition(
        &self,
        pool: &str,
    ) -> Option<std::collections::BTreeMap<String, crate::gate::StructuredGateResult>> {
        use crate::gate::{GateOutcome, StructuredGateResult};
        let (outcome, output) = match self {
            GuardOutcome::Unpooled | GuardOutcome::Acquired(_) | GuardOutcome::Nested { .. } => {
                return None
            }
            GuardOutcome::Queued(output) => (GateOutcome::Pending, output.clone()),
            GuardOutcome::ConfigInvalid {
                config_path,
                key,
                reason,
            } => (
                GateOutcome::Error,
                serde_json::json!({
                    "status": "invalid",
                    "error_code": "pool_config_invalid",
                    "pool": pool,
                    "config_file": config_path.display().to_string(),
                    "key": key,
                    "reason": reason,
                }),
            ),
            GuardOutcome::DirUnavailable(e) => (
                GateOutcome::Error,
                serde_json::json!({
                    "status": "unavailable",
                    "error_code": "pool_dir_unavailable",
                    "pool": pool,
                    "path": e.path.display().to_string(),
                    "reason": e.reason,
                }),
            ),
            GuardOutcome::UnsupportedPlatform => (
                GateOutcome::Error,
                serde_json::json!({
                    "status": "unsupported",
                    "error_code": "pool_unsupported_platform",
                    "pool": pool,
                    "platform": std::env::consts::OS,
                    "reason": "host pools run on Linux and macOS only",
                }),
            ),
        };
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            crate::template::types::POOL_CONDITION_NAME.to_string(),
            StructuredGateResult {
                outcome,
                output,
                ..Default::default()
            },
        );
        Some(map)
    }
}

/// Resolve `pool` from the user config under `requester.koto_root` and take
/// a slot, wait for one, or report the requester queued.
pub fn acquire(pool: &str, requester: &Requester) -> Guarded {
    let config_path = requester.koto_root.join("config.toml");
    let unnoted = |outcome| Guarded {
        outcome,
        note: QueueNote::default(),
    };
    match config::lookup(&config_path, pool) {
        PoolLookup::Undefined => {
            // A pool removed while this session waited: it runs unthrottled
            // and leaves the queue it was in (R8). The directory is left
            // alone if it is already gone, so an undefined pool still
            // creates nothing (R6).
            let mut note = QueueNote::default();
            if requester.was_waiting {
                note.pool_removed = true;
                let _ = store::leave_queue(
                    &requester.koto_root,
                    pool,
                    &requester.session,
                    &requester.workspace,
                );
            }
            Guarded {
                outcome: GuardOutcome::Unpooled,
                note,
            }
        }
        PoolLookup::Invalid { key, reason } => unnoted(GuardOutcome::ConfigInvalid {
            config_path,
            key,
            reason,
        }),
        // Checked only for a defined pool: an undefined one imposes nothing
        // on any platform, so a template naming a pool still runs where
        // pools don't (R6, R27).
        PoolLookup::Defined(_) if !super::platform::supported() => {
            unnoted(GuardOutcome::UnsupportedPlatform)
        }
        PoolLookup::Defined(spec) => {
            if requester.may_nest {
                if let Some(index) = inherited_slot(pool, &requester.koto_root) {
                    let held = std::env::var(POOL_HELD_ENV).unwrap_or_default();
                    let root = std::env::var(POOL_ROOT_ENV).unwrap_or_default();
                    return unnoted(GuardOutcome::Nested {
                        index,
                        marker: vec![
                            (POOL_HELD_ENV.to_string(), held),
                            (POOL_ROOT_ENV.to_string(), root),
                        ],
                    });
                }
            }
            acquire_defined(pool, &spec, requester)
        }
    }
}

/// [`acquire`] for a pool the config defines as `spec`.
pub fn acquire_defined(pool: &str, spec: &PoolSpec, requester: &Requester) -> Guarded {
    let mut note = QueueNote::default();
    let dir = match store::pool_dir(&requester.koto_root, pool) {
        Ok(dir) => dir,
        Err(e) => {
            return Guarded {
                outcome: GuardOutcome::DirUnavailable(e),
                note,
            }
        }
    };
    let started = Instant::now();
    let wait = Duration::from_secs(spec.wait_secs as u64);
    // Whether the session is in the queue as far as this call knows: the
    // log's answer until the first edit, then what the edits did.
    let mut waiting = requester.was_waiting;
    loop {
        match attempt(&dir, spec, requester, waiting) {
            Ok(Some(turn)) => {
                note.lapsed |= turn.lapsed;
                note.queued = turn.slot.is_none();
                if let Some(slot) = turn.slot {
                    return Guarded {
                        outcome: GuardOutcome::Acquired(slot),
                        note,
                    };
                }
                waiting = true;
            }
            // `queue.lock` stayed busy: not yet.
            Ok(None) => {}
            Err(e) => {
                return Guarded {
                    outcome: GuardOutcome::DirUnavailable(DirUnavailable {
                        path: dir.clone(),
                        reason: e.to_string(),
                    }),
                    note,
                }
            }
        }
        if started.elapsed() + POLL_INTERVAL > wait {
            return Guarded {
                outcome: GuardOutcome::Queued(queued_output(&dir, pool, spec, requester)),
                note,
            };
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// What one queue edit did for the requester.
struct Turn {
    /// The slot taken, or `None` when the requester is left waiting.
    slot: Option<Slot>,
    /// The requester was waiting but found its entry gone.
    lapsed: bool,
}

/// One attempt under `queue.lock` (DESIGN "Taking a slot: the queue and the
/// claim"): drop lapsed waiters, find or add this session's entry and mark
/// it seen, bring the claim up to date, and take a free slot only when the
/// queue says it is this session's turn. A session arriving while others
/// wait joins behind them even when a slot is free (R11), and a slot is
/// only ever taken here, under the lock, so a race for the last slot
/// produces one holder (R9). `Ok(None)` when `queue.lock` stayed busy,
/// which counts as "not yet".
fn attempt(
    dir: &Path,
    spec: &PoolSpec,
    requester: &Requester,
    waiting: bool,
) -> std::io::Result<Option<Turn>> {
    let now = store::now_secs();
    let holder = Holder {
        session: requester.session.clone(),
        workspace: requester.workspace.clone(),
        state: requester.state.clone(),
        pid: std::process::id(),
        since: now,
        run_id: Some(requester.holder_id.clone()),
    };
    let edited = store::edit_queue(dir, |queue| {
        queue.sweep_lapsed(now, spec.lapse_secs);
        let lapsed = waiting
            && queue
                .position_of(&requester.session, &requester.workspace)
                .is_none();
        let (mine, _) = queue.join_or_refresh(&requester.session, &requester.workspace, now);
        let free = store::free_slots(dir, spec.size);
        queue.settle_claim(free, spec.claim_secs, now);
        let mut slot = Ok(None);
        if queue.may_take(mine, free) {
            slot = store::try_acquire(dir, spec.size, &holder);
            if matches!(slot, Ok(Some(_))) {
                queue.remove_at(mine);
            }
        }
        // Marking the entry seen always changes the queue, so it is always
        // written back.
        (slot.map(|slot| Turn { slot, lapsed }), true)
    })?;
    edited.transpose()
}

/// The `__pool__` condition's output for a requester left waiting (PRD R16).
fn queued_output(
    dir: &Path,
    pool: &str,
    spec: &PoolSpec,
    requester: &Requester,
) -> serde_json::Value {
    let now = store::now_secs();
    let queue = store::read_queue(dir);
    let (position, waited_secs) = match queue.position_of(&requester.session, &requester.workspace)
    {
        Some(i) => (i + 1, now.saturating_sub(queue.entries[i].joined_at)),
        // The queue write was refused or raced; this session is still the
        // last to arrive.
        None => (queue.entries.len() + 1, 0),
    };
    let waiting = queue.entries.len().max(position);
    let holders: Vec<serde_json::Value> = store::holders(dir)
        .into_iter()
        .map(|(_, holder)| match holder {
            Some(h) => serde_json::json!({
                "session": store::sanitize(&h.session),
                "workspace": store::sanitize(&h.workspace),
                "state": store::sanitize(&h.state),
                "held_secs": now.saturating_sub(h.since),
            }),
            // Held, but its record is being written or was cut.
            None => serde_json::json!({
                "session": "",
                "workspace": "",
                "state": "",
                "held_secs": 0,
            }),
        })
        .collect();
    serde_json::json!({
        "status": "queued",
        "pool": pool,
        "size": spec.size,
        "position": position,
        "waiting": waiting,
        "waited_secs": waited_secs,
        "retry_after_secs": spec.claim_secs.min(MAX_RETRY_AFTER_SECS),
        "holders": holders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requester(root: &Path, session: &str) -> Requester {
        Requester {
            koto_root: root.to_path_buf(),
            session: session.to_string(),
            workspace: format!("ws-{session}"),
            state: "test".to_string(),
            was_waiting: false,
            holder_id: format!("tick-{session}"),
            may_nest: true,
        }
    }

    fn waiting(root: &Path, session: &str) -> Requester {
        Requester {
            was_waiting: true,
            ..requester(root, session)
        }
    }

    fn config(root: &Path, text: &str) {
        std::fs::write(root.join("config.toml"), text).unwrap();
    }

    fn slot(guarded: Guarded) -> Slot {
        match guarded.outcome {
            GuardOutcome::Acquired(slot) => slot,
            other => panic!("expected a slot, got {other:?}"),
        }
    }

    fn queued(guarded: &Guarded) -> &serde_json::Value {
        match &guarded.outcome {
            GuardOutcome::Queued(output) => output,
            other => panic!("expected queued, got {other:?}"),
        }
    }

    fn event_types(events: &[EventPayload]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                EventPayload::PoolReleased(r) => format!("released:{}", r.reason),
                other => other.type_name().to_string(),
            })
            .collect()
    }

    #[test]
    fn an_undefined_pool_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let guarded = acquire("test-suite", &requester(root.path(), "a"));
        assert!(matches!(guarded.outcome, GuardOutcome::Unpooled));
        assert_eq!(guarded.note, QueueNote::default());
        // A waiter whose pool went away creates nothing either.
        let guarded = acquire("test-suite", &waiting(root.path(), "a"));
        assert!(guarded.note.pool_removed);
        assert!(!root.path().join("pools").exists());
    }

    #[test]
    fn a_second_requester_is_queued_behind_the_holder() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
        let held = slot(acquire("test-suite", &requester(root.path(), "a")));
        let b = requester(root.path(), "b");
        let guarded = acquire("test-suite", &b);
        let output = queued(&guarded);
        assert_eq!(output["position"], 1);
        assert_eq!(output["waiting"], 1);
        assert_eq!(output["size"], 1);
        assert_eq!(output["retry_after_secs"], 15);
        assert_eq!(output["holders"][0]["session"], "a");
        assert_eq!(
            event_types(&guarded.queue_events("test-suite", &b)),
            ["pool_queued"]
        );
        drop(held);
        let b = waiting(root.path(), "b");
        let guarded = acquire("test-suite", &b);
        assert!(guarded.queue_events("test-suite", &b).is_empty());
        let _slot = slot(guarded);
        // Taking the slot left the queue.
        let dir = root.path().join("pools").join("test-suite");
        assert!(store::read_queue(&dir).entries.is_empty());
    }

    #[test]
    fn an_arrival_queues_behind_a_waiter_even_with_a_free_slot() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 1\nwait_secs = 0\n");
        let held = slot(acquire("p", &requester(root.path(), "h")));
        let _ = queued(&acquire("p", &requester(root.path(), "w1")));
        drop(held);
        let c = acquire("p", &requester(root.path(), "c"));
        assert_eq!(queued(&c)["position"], 2);
        // The slot is the head waiter's.
        let _w1 = slot(acquire("p", &waiting(root.path(), "w1")));
    }

    #[test]
    fn a_waiter_whose_entry_is_gone_is_told_it_lapsed_and_queues_again() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 1\nwait_secs = 0\n");
        let _held = slot(acquire("p", &requester(root.path(), "h")));
        let _ = queued(&acquire("p", &requester(root.path(), "w2")));
        // `w1`'s log says it waits, but the queue has no entry for it.
        let w1 = waiting(root.path(), "w1");
        let guarded = acquire("p", &w1);
        assert_eq!(queued(&guarded)["position"], 2);
        assert!(guarded.note.lapsed);
        assert_eq!(
            event_types(&guarded.queue_events("p", &w1)),
            ["released:lapsed", "pool_queued"]
        );
    }

    #[test]
    fn a_marker_verifies_only_against_its_own_held_slot() {
        let root = tempfile::tempdir().unwrap();
        config(
            root.path(),
            "[pools.p]\nsize = 1\nwait_secs = 0\n[pools.q]\nsize = 1\n",
        );
        let r = root.path().to_str().unwrap();
        let held = slot(acquire("p", &requester(root.path(), "a")));
        let verify = |held: &str, pool: &str| verify_marker(Some(held), Some(r), pool, root.path());
        assert_eq!(verify("p/0/tick-a", "p"), Some(0));
        // Another pool, another holder, another slot, another root, junk.
        assert_eq!(verify("p/0/tick-a", "q"), None);
        assert_eq!(verify("p/0/tick-b", "p"), None);
        assert_eq!(verify("p/1/tick-a", "p"), None);
        assert_eq!(verify("p/x/tick-a", "p"), None);
        assert_eq!(verify("p/0/", "p"), None);
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            verify_marker(Some("p/0/tick-a"), Some(r), "p", other.path()),
            None
        );
        assert_eq!(
            verify_marker(Some("p/0/tick-a"), None, "p", root.path()),
            None
        );
        // Once the slot is free the marker is stale and grants nothing.
        drop(held);
        assert_eq!(verify("p/0/tick-a", "p"), None);
    }

    #[test]
    fn an_unsupported_platform_stops_a_defined_pool_and_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 1\n");
        let _forced = super::super::platform::force_for_test(false);
        let guarded = acquire("p", &requester(root.path(), "a"));
        assert!(matches!(guarded.outcome, GuardOutcome::UnsupportedPlatform));
        let conditions = guarded.outcome.blocking_condition("p").unwrap();
        let condition = &conditions[crate::template::types::POOL_CONDITION_NAME];
        assert_eq!(condition.outcome, crate::gate::GateOutcome::Error);
        assert_eq!(
            condition.output["error_code"],
            serde_json::json!("pool_unsupported_platform")
        );
        assert_eq!(condition.output["pool"], serde_json::json!("p"));
        assert!(
            !root.path().join("pools").exists(),
            "an unsupported platform touches no pool directory"
        );
        // An undefined pool still runs unthrottled there.
        assert!(matches!(
            acquire("other", &requester(root.path(), "a")).outcome,
            GuardOutcome::Unpooled
        ));
    }

    #[test]
    fn a_supported_platform_takes_a_slot_of_a_defined_pool() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 1\n");
        let _forced = super::super::platform::force_for_test(true);
        let guarded = acquire("p", &requester(root.path(), "a"));
        assert!(
            matches!(guarded.outcome, GuardOutcome::Acquired(_)),
            "got {:?}",
            guarded.outcome
        );
    }

    #[test]
    fn an_invalid_pool_names_its_key() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 0\n");
        match acquire("p", &requester(root.path(), "a")).outcome {
            GuardOutcome::ConfigInvalid { key, .. } => assert_eq!(key, "pools.p.size"),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn the_in_tick_wait_takes_a_slot_that_frees() {
        let root = tempfile::tempdir().unwrap();
        config(root.path(), "[pools.p]\nsize = 1\nwait_secs = 5\n");
        let held = slot(acquire("p", &requester(root.path(), "a")));
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            drop(held);
        });
        let started = Instant::now();
        let b = requester(root.path(), "b");
        let guarded = acquire("p", &b);
        // Joined and left within the tick: nothing to record but the slot.
        assert!(guarded.queue_events("p", &b).is_empty());
        let _slot = slot(guarded);
        assert!(started.elapsed() < Duration::from_secs(4));
        releaser.join().unwrap();
    }

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: String::new(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn queued_event(state: &str, pool: &str) -> EventPayload {
        EventPayload::PoolQueued(PoolQueued {
            state: state.to_string(),
            pool: pool.to_string(),
            workspace: "ws".to_string(),
        })
    }

    #[test]
    fn open_waits_are_pools_whose_latest_event_is_a_join() {
        let released = EventPayload::PoolReleased(PoolReleased {
            state: "a".to_string(),
            pool: "p".to_string(),
            reason: "lapsed".to_string(),
            run_id: None,
        });
        let events = vec![
            event(1, queued_event("a", "p")),
            event(2, queued_event("a", "q")),
            event(3, released),
        ];
        let open = open_waits(&events);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].pool, "q");
    }
}
