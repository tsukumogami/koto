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
use super::store::{self, DirUnavailable, Holder, QueueEntry, Slot};

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
}

/// What the guard decided.
#[derive(Debug)]
pub enum GuardOutcome {
    /// The pool is undefined: run the action as if `pool` were absent.
    Unpooled,
    /// A slot was taken; hold it while the action runs.
    Acquired(Slot),
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
            GuardOutcome::Unpooled | GuardOutcome::Acquired(_) => return None,
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
pub fn acquire(pool: &str, requester: &Requester) -> GuardOutcome {
    let config_path = requester.koto_root.join("config.toml");
    match config::lookup(&config_path, pool) {
        PoolLookup::Undefined => GuardOutcome::Unpooled,
        PoolLookup::Invalid { key, reason } => GuardOutcome::ConfigInvalid {
            config_path,
            key,
            reason,
        },
        PoolLookup::Defined(spec) => acquire_defined(pool, &spec, requester),
    }
}

/// [`acquire`] for a pool the config defines as `spec`.
pub fn acquire_defined(pool: &str, spec: &PoolSpec, requester: &Requester) -> GuardOutcome {
    let dir = match store::pool_dir(&requester.koto_root, pool) {
        Ok(dir) => dir,
        Err(e) => return GuardOutcome::DirUnavailable(e),
    };
    let unavailable = |e: std::io::Error| {
        GuardOutcome::DirUnavailable(DirUnavailable {
            path: dir.clone(),
            reason: e.to_string(),
        })
    };
    let started = Instant::now();
    let wait = Duration::from_secs(spec.wait_secs as u64);
    loop {
        match attempt(&dir, spec, requester) {
            Ok(Some(slot)) => return GuardOutcome::Acquired(slot),
            Ok(None) => {}
            Err(e) => return unavailable(e),
        }
        if started.elapsed() + POLL_INTERVAL > wait {
            return GuardOutcome::Queued(queued_output(&dir, pool, spec, requester));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One attempt under `queue.lock`: drop lapsed waiters, take a free slot and
/// leave the queue, or join it (or refresh this session's place in it).
/// `Ok(None)` when no slot is free, or when `queue.lock` stayed busy, which
/// counts as "not yet".
fn attempt(dir: &Path, spec: &PoolSpec, requester: &Requester) -> std::io::Result<Option<Slot>> {
    let now = store::now_secs();
    let holder = Holder {
        session: requester.session.clone(),
        workspace: requester.workspace.clone(),
        state: requester.state.clone(),
        pid: std::process::id(),
        since: now,
    };
    let outcome = store::edit_queue(dir, |queue| {
        let before = queue.entries.len();
        queue
            .entries
            .retain(|e| now.saturating_sub(e.last_seen) <= spec.lapse_secs as u64);
        let mut changed = queue.entries.len() != before;
        let mine = queue.position_of(&requester.session, &requester.workspace);
        match store::try_acquire(dir, spec.size, &holder) {
            Ok(Some(slot)) => {
                if let Some(i) = mine {
                    queue.entries.remove(i);
                    changed = true;
                }
                (Ok(Some(slot)), changed)
            }
            Ok(None) => {
                match mine {
                    Some(i) => queue.entries[i].last_seen = now,
                    None => {
                        let ticket = queue.next_ticket;
                        queue.next_ticket += 1;
                        queue.entries.push(QueueEntry {
                            ticket,
                            session: requester.session.clone(),
                            workspace: requester.workspace.clone(),
                            joined_at: now,
                            last_seen: now,
                        });
                    }
                }
                (Ok(None), true)
            }
            Err(e) => (Err(e), changed),
        }
    })?;
    outcome.unwrap_or(Ok(None))
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
        }
    }

    #[test]
    fn an_undefined_pool_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            acquire("test-suite", &requester(root.path(), "a")),
            GuardOutcome::Unpooled
        ));
        assert!(!root.path().join("pools").exists());
    }

    #[test]
    fn a_second_requester_is_queued_behind_the_holder() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[pools.test-suite]\nsize = 1\nwait_secs = 0\n",
        )
        .unwrap();
        let held = match acquire("test-suite", &requester(root.path(), "a")) {
            GuardOutcome::Acquired(slot) => slot,
            other => panic!("expected a slot, got {other:?}"),
        };
        let output = match acquire("test-suite", &requester(root.path(), "b")) {
            GuardOutcome::Queued(output) => output,
            other => panic!("expected queued, got {other:?}"),
        };
        assert_eq!(output["position"], 1);
        assert_eq!(output["waiting"], 1);
        assert_eq!(output["size"], 1);
        assert_eq!(output["retry_after_secs"], 15);
        assert_eq!(output["holders"][0]["session"], "a");
        drop(held);
        assert!(matches!(
            acquire("test-suite", &requester(root.path(), "b")),
            GuardOutcome::Acquired(_)
        ));
        // Taking the slot left the queue.
        let dir = root.path().join("pools").join("test-suite");
        assert!(store::read_queue(&dir).entries.is_empty());
    }

    #[test]
    fn an_invalid_pool_names_its_key() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.toml"), "[pools.p]\nsize = 0\n").unwrap();
        match acquire("p", &requester(root.path(), "a")) {
            GuardOutcome::ConfigInvalid { key, .. } => assert_eq!(key, "pools.p.size"),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn the_in_tick_wait_takes_a_slot_that_frees() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[pools.p]\nsize = 1\nwait_secs = 5\n",
        )
        .unwrap();
        let held = match acquire("p", &requester(root.path(), "a")) {
            GuardOutcome::Acquired(slot) => slot,
            other => panic!("expected a slot, got {other:?}"),
        };
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            drop(held);
        });
        let started = Instant::now();
        assert!(matches!(
            acquire("p", &requester(root.path(), "b")),
            GuardOutcome::Acquired(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
        releaser.join().unwrap();
    }
}
