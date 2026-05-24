//! Wake-candidates pass — emits `RequesterWoken` audit events when
//! dispatched children reach terminal state.
//!
//! Runs on every `koto next` tick. Walks the coordinator's own session
//! log for `ChildDispatched` events; for each, checks whether the
//! corresponding child reached terminal AND whether a matching
//! `RequesterWoken` (carrying the child's id in its
//! `child_session_ids` array) already exists on this coord's log.
//! For each open candidate (terminal child + no matching
//! RequesterWoken): emits a `RequesterWoken` event covering the batch
//! via [`crate::engine::audit::requester_woken_fields`], runs the
//! 3-point fsync sequence from [`crate::engine::persistence::fsync_wake_preconditions`],
//! invokes the substrate's wake-delivery primitive via [`SubstrateWaker`],
//! and unlinks the claim sidecar via [`crate::engine::claim::release_sidecar`].
//!
//! The age-and-activity recovery rule (Decision 2 sub-question 4)
//! repairs a coordinator crash between the `RequesterWoken` fsync and
//! the `substrate_wake` syscall without double-waking: if
//! `now - woken_at > stale_dispatch_timeout` AND the requester's
//! session log has no entries with mtime later than `woken_at`, the
//! pass re-invokes the substrate's wake primitive (no new
//! `RequesterWoken` is emitted — the original is authoritative).
//!
//! Performance: the pass is `O(open-dispatches)`, NOT
//! `O(workspace-sessions)`. It walks the coord's own log + the
//! terminal-index lookup table for each `ChildDispatched` it finds;
//! no scan-cursor interaction (different scan path from
//! [`crate::engine::discovery::scan`]).
//!
//! See DESIGN-koto-request-store.md Decision 2 sub-question 4
//! (age-and-activity recovery), Decision 6 (audit-event family),
//! PRD R19 (wake happens-after fsync), R30 (wake candidates).

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::engine::audit::{requester_woken_fields, CHILD_DISPATCHED, REQUESTER_WOKEN};
use crate::engine::claim::{format_rfc3339_millis, release_sidecar};
use crate::engine::errors::EngineError;
use crate::engine::persistence::{fsync_wake_preconditions, read_events};
use crate::engine::terminal_index::read_terminal_index;
use crate::engine::types::{Event, EventPayload, ValidatedSessionId};
use crate::session::state_file_name;

/// Pluggable substrate wake-delivery abstraction.
///
/// Sibling to [`crate::engine::claim::SubstrateSpawner`]. The concrete
/// implementation (Claude Code agent-membership poke, bunki BK2 hosted
/// wake, etc.) lives outside this crate. Tests use a mock that records
/// every wake invocation so the dispatch contract can be asserted
/// without touching a real substrate.
///
/// `wake` is expected to be idempotent at the substrate level: the
/// age-and-activity recovery rule may re-invoke wake for the same
/// session if the original wake was lost mid-syscall. The substrate's
/// existing agent-membership primitive is fire-and-forget, so duplicate
/// invocations are benign — the requester resumes from its own koto
/// log either way.
pub trait SubstrateWaker {
    /// Signal the substrate to wake `session_id`. Returns
    /// `Err(EngineError)` on substrate-level failure; the wake-emission
    /// path treats `Err` as a soft failure (the durable `RequesterWoken`
    /// event remains on disk, so the next tick's age-and-activity
    /// recovery rule will retry).
    fn wake(&self, session_id: &ValidatedSessionId) -> Result<(), EngineError>;
}

/// Default [`SubstrateWaker`] used by `handle_next` until a concrete
/// substrate implementation (Claude Code agent-membership poke, bunki
/// BK2 hosted wake) ships. Every call emits an `eprintln!` so operators
/// can observe wake intent during the transitional period; nothing
/// else happens.
///
/// The durable `RequesterWoken` event on the coord's log is the
/// source of truth for "wake intent recorded"; the substrate primitive
/// invocation is the operational follow-through. Until that primitive
/// exists, the audit trail stays correct and the requester resumes by
/// other means (operator-triggered, polling, etc.).
pub struct LoggingWaker;

impl SubstrateWaker for LoggingWaker {
    fn wake(&self, session_id: &ValidatedSessionId) -> Result<(), EngineError> {
        eprintln!(
            "info: SubstrateWaker stub invoked for session '{}'; \
             concrete wake-delivery primitive not yet wired",
            session_id.as_str()
        );
        Ok(())
    }
}

/// Single open dispatch identified by the wake-candidates walk.
///
/// "Open" means: the coordinator's log carries a `ChildDispatched` for
/// `(child_session_id, dispatch_epoch)`, AND no later `RequesterWoken`
/// on this log has the same `(child_session_id, dispatch_epoch)` pair
/// in its parallel `child_session_ids` / `child_dispatch_epochs`
/// arrays. The epoch is part of the key so a session that is
/// dispatched, terminalized, woken, then re-dispatched (header epoch
/// bumps under recovery) gets a fresh wake at the new epoch instead of
/// being silently filtered by the prior wake's bare-id record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDispatch {
    /// The dispatched child's session id.
    pub child_session_id: ValidatedSessionId,
    /// Dispatch epoch from the `ChildDispatched` event's `fields` map.
    /// Part of the wake-filter key alongside `child_session_id`.
    pub dispatch_epoch: u32,
    /// The session id of the principal that originally requested the
    /// child. Read from the child's header's `requested_by` field at
    /// dispatch time. Used to populate the wake's `requested_by` field
    /// and to identify the substrate-wake target.
    pub requested_by: String,
    /// `seq` of the `ChildDispatched` event that recorded this open
    /// dispatch. Diagnostic — not load-bearing for the wake protocol.
    pub dispatch_seq: u64,
}

/// Single completed wake candidate ready for emission.
///
/// Produced by [`find_wake_candidates`] when an [`OpenDispatch`]'s
/// child is also terminal (per the terminal-index lookup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeCandidate {
    /// The dispatched child's session id.
    pub child_session_id: ValidatedSessionId,
    /// Dispatch epoch carried through from the corresponding
    /// [`OpenDispatch`]. Emitted into the `RequesterWoken` event's
    /// `child_dispatch_epochs` array so future wake passes can match
    /// (id, epoch) and not collide on bare id.
    pub dispatch_epoch: u32,
    /// Resolved requester session id from the dispatch's header.
    pub requested_by: String,
}

/// Outcome of a wake-emission attempt for a batch.
#[derive(Debug)]
pub struct WakeEmissionOutcome {
    /// Number of `RequesterWoken` events emitted on the coord's log
    /// during this pass. Each event covers one batch of
    /// children that share a `requested_by` principal.
    pub events_emitted: usize,
    /// Number of substrate-wake invocations made during this pass
    /// (including any recovery re-invocations).
    pub wakes_invoked: usize,
    /// Number of claim sidecars unlinked during this pass.
    pub sidecars_released: usize,
    /// Number of stale `RequesterWoken` events whose
    /// age-and-activity recovery rule fired (re-invoke wake; no new
    /// `RequesterWoken` event emitted).
    pub recoveries_fired: usize,
}

/// Walk a coord's session log and return the set of currently open
/// dispatches.
///
/// "Open" means: the log carries a `ChildDispatched` event for the
/// `(child_session_id, dispatch_epoch)` pair AND no later
/// `RequesterWoken` event carries that same pair in its parallel
/// `child_session_ids` / `child_dispatch_epochs` arrays. Keying on the
/// pair instead of the bare id is what lets a re-dispatched child
/// (header epoch bumped under recovery) get a fresh wake; bare-id
/// keying would silently drop the second wake against the first
/// wake's record.
///
/// Pure function over events; no I/O beyond what the caller already
/// passed in. Callers typically derive `events` from
/// [`crate::engine::persistence::read_events`].
pub fn find_open_dispatches(events: &[Event]) -> Vec<OpenDispatch> {
    let mut already_woken: HashSet<(String, u32)> = HashSet::new();
    // First pass: collect every (child_session_id, dispatch_epoch) pair
    // that appears in a later RequesterWoken event. Pre-fix-1 wakes
    // (no child_dispatch_epochs array) are treated as covering epoch 0
    // so the on-disk audit log remains backward-compatible.
    for event in events {
        if event_kind(event) != Some(REQUESTER_WOKEN) {
            continue;
        }
        let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload else {
            continue;
        };
        let Some(ids_arr) = fields.get("child_session_ids").and_then(|v| v.as_array()) else {
            continue;
        };
        let epochs_arr = fields
            .get("child_dispatch_epochs")
            .and_then(|v| v.as_array());
        for (idx, item) in ids_arr.iter().enumerate() {
            let Some(id_str) = item.as_str() else {
                continue;
            };
            let epoch = epochs_arr
                .and_then(|arr| arr.get(idx))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0);
            already_woken.insert((id_str.to_string(), epoch));
        }
    }

    // Second pass: collect every ChildDispatched whose
    // (child_session_id, dispatch_epoch) pair is NOT in the
    // already-woken set.
    let mut open: Vec<OpenDispatch> = Vec::new();
    for event in events {
        if event_kind(event) != Some(CHILD_DISPATCHED) {
            continue;
        }
        let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload else {
            continue;
        };
        let Some(child_str) = fields.get("child_session_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let dispatch_epoch = fields
            .get("dispatch_epoch")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0);
        if already_woken.contains(&(child_str.to_string(), dispatch_epoch)) {
            continue;
        }
        let Ok(child_id) = ValidatedSessionId::new(child_str) else {
            continue;
        };
        // requested_by is not on the ChildDispatched payload (per
        // Decision 6 line 803 — that event's fields are
        // child_session_id/dispatched_at/role/template). We read it
        // from the child's header at terminal-confirmation time;
        // populate as empty here and let the caller resolve.
        open.push(OpenDispatch {
            child_session_id: child_id,
            dispatch_epoch,
            requested_by: String::new(),
            dispatch_seq: event.seq,
        });
    }
    open
}

/// Resolve which open dispatches have terminal children and return
/// the populated [`WakeCandidate`] list.
///
/// Uses the workspace's terminal-index to identify terminal children;
/// stale-index entries (children whose header has since advanced past
/// the index's recorded mtime) are filtered by the index's
/// header-is-truth fallthrough — but the wake-candidates pass only
/// needs "is currently terminal", so a stale-index false-positive is
/// harmless (we'd emit a wake; the requester reads the actual state
/// from its own log).
///
/// `requested_by` is resolved by reading each candidate's header. A
/// header read failure causes that candidate to be skipped (silent
/// log, see [`crate::engine::discovery`] for the analogous pattern).
pub fn find_wake_candidates(
    koto_root: &Path,
    sessions_dir: &Path,
    open: &[OpenDispatch],
) -> Vec<WakeCandidate> {
    let terminal_by_id = read_terminal_index(koto_root);
    let mut candidates = Vec::with_capacity(open.len());
    for o in open {
        if !terminal_by_id.contains_key(o.child_session_id.as_str()) {
            continue;
        }
        // Resolve requested_by from the child's header.
        let child_state_path = sessions_dir
            .join(o.child_session_id.as_str())
            .join(state_file_name(o.child_session_id.as_str()));
        let header = match crate::engine::persistence::read_header(&child_state_path) {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "warning: skipping wake candidate {} — header read failed: {}",
                    o.child_session_id.as_str(),
                    e
                );
                continue;
            }
        };
        let requested_by = header.requested_by.clone().unwrap_or_default();
        if requested_by.is_empty() {
            // Without a requested_by we can't address the wake
            // primitive. Skip; Issue 4 guarantees the field is
            // populated on request-store requests but pre-Issue-4 headers may
            // lack it.
            continue;
        }
        candidates.push(WakeCandidate {
            child_session_id: o.child_session_id.clone(),
            dispatch_epoch: o.dispatch_epoch,
            requested_by,
        });
    }
    candidates
}

/// Emit a single `RequesterWoken` event covering the batch of
/// candidates that share `requested_by`, run the 3-point fsync
/// sequence per child, invoke the substrate's wake-delivery primitive
/// for the requester, and unlink each child's claim sidecar.
///
/// Returns the count of (events, wakes, sidecar-unlinks) for the
/// caller's accounting. Errors from the substrate are surfaced but
/// the on-disk state stays consistent (the `RequesterWoken` event is
/// durable; the next tick's recovery rule will retry).
fn emit_one_wake_batch(
    coord_state_file: &Path,
    sessions_dir: &Path,
    requested_by: &str,
    children: &[ValidatedSessionId],
    epochs: &[u32],
    waker: &dyn SubstrateWaker,
    now: SystemTime,
) -> Result<(usize, usize, usize)> {
    use crate::engine::persistence::append_event;
    if children.is_empty() {
        return Ok((0, 0, 0));
    }
    debug_assert_eq!(
        children.len(),
        epochs.len(),
        "emit_one_wake_batch: epochs.len() must equal children.len()"
    );
    let fields = requester_woken_fields(children, epochs, requested_by);
    let payload = EventPayload::EvidenceSubmitted {
        state: "request_store.wake".to_string(),
        fields,
        submitter_cwd: None,
    };
    let timestamp = format_rfc3339_millis(now);
    append_event(coord_state_file, &payload, &timestamp)
        .with_context(|| format!("append RequesterWoken to {}", coord_state_file.display()))?;

    // 3-point fsync per child (each fsync_wake_preconditions call
    // fsyncs child log + coord log twice; we run per-child so the
    // child's log is the right log per call).
    for child in children {
        let child_state_path = sessions_dir
            .join(child.as_str())
            .join(state_file_name(child.as_str()));
        fsync_wake_preconditions(&child_state_path, coord_state_file)
            .with_context(|| format!("fsync_wake_preconditions for child {}", child.as_str()))?;
    }

    // Invoke substrate wake for the requester (ONE call per batch).
    let requester_id = ValidatedSessionId::new(requested_by).map_err(|e| {
        anyhow::anyhow!(
            "requested_by '{}' is not a valid session id: {}",
            requested_by,
            e
        )
    })?;
    let mut wakes = 0usize;
    match waker.wake(&requester_id) {
        Ok(()) => wakes += 1,
        Err(e) => {
            eprintln!(
                "warning: substrate wake for '{}' returned err ({}); recovery will retry on next tick",
                requested_by, e
            );
        }
    }

    // Unlink each child's claim sidecar.
    let mut sidecars_released = 0usize;
    for child in children {
        let child_dir = sessions_dir.join(child.as_str());
        if let Err(e) = release_sidecar(&child_dir) {
            eprintln!(
                "warning: release_sidecar for child {} returned err: {}",
                child.as_str(),
                e
            );
        } else if !sidecar_still_present(&child_dir) {
            sidecars_released += 1;
        }
    }

    Ok((1, wakes, sidecars_released))
}

/// Check whether the claim sidecar still exists on disk at the time
/// of the call. Returns `true` when the file is present, `false` when
/// `release_sidecar` succeeded in unlinking it (or it was already
/// absent). The caller in `emit_one_wake_batch` negates the return to
/// derive the `sidecars_released` count: a successful release leaves
/// the sidecar absent, which is the `!sidecar_still_present` branch.
fn sidecar_still_present(child_dir: &Path) -> bool {
    crate::engine::claim::sidecar_path(child_dir).exists()
}

/// Fire the age-and-activity recovery rule for one stale
/// `RequesterWoken` event.
///
/// The rule (Decision 2 sub-question 4): if `now - woken_at` exceeds
/// `stale_dispatch_timeout` AND the requester's session log has no
/// entries with mtime later than `woken_at`, re-invoke the substrate
/// wake primitive. NO additional `RequesterWoken` is emitted — the
/// original event is authoritative.
///
/// Returns `Some(())` when the recovery fires (substrate wake
/// re-invoked); `None` when the rule does not fire (timeout not
/// exceeded, OR requester showed post-woken_at activity).
fn maybe_recover_stale_wake(
    sessions_dir: &Path,
    requested_by: &str,
    woken_at: SystemTime,
    now: SystemTime,
    stale_timeout: Duration,
    waker: &dyn SubstrateWaker,
) -> Option<()> {
    // Timeout check.
    let elapsed = now.duration_since(woken_at).unwrap_or(Duration::ZERO);
    if elapsed < stale_timeout {
        return None;
    }
    // Activity check: requester's session log mtime > woken_at →
    // requester resumed, recovery does NOT fire.
    let requester_state_path = sessions_dir
        .join(requested_by)
        .join(state_file_name(requested_by));
    let log_mtime = match fs::metadata(&requester_state_path).and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return None,
    };
    if log_mtime > woken_at {
        return None;
    }
    // Recovery fires.
    let requester_id = match ValidatedSessionId::new(requested_by) {
        Ok(id) => id,
        Err(_) => return None,
    };
    if let Err(e) = waker.wake(&requester_id) {
        eprintln!(
            "warning: recovery substrate wake for '{}' returned err: {}",
            requested_by, e
        );
    }
    Some(())
}

/// Run the wake-candidates pass for one coordinator tick.
///
/// Reads `coord_state_file` once, finds open dispatches, intersects
/// with terminal children via the terminal-index, batches by
/// `requested_by`, and emits a `RequesterWoken` per batch with the
/// full fsync-then-wake-then-release-sidecar discipline. Also fires
/// the age-and-activity recovery rule for any stale `RequesterWoken`
/// events on the coord's log.
///
/// `coord_state_file` is the coordinator's own session log path.
/// `sessions_dir` is `<koto_root>/sessions/`. `koto_root` is needed
/// for the terminal-index path. `stale_dispatch_timeout` is the
/// recovery rule's grace period (operator-tunable; default 600s).
///
/// Returns a [`WakeEmissionOutcome`] tally.
pub fn wake_candidates_pass(
    koto_root: &Path,
    sessions_dir: &Path,
    coord_state_file: &Path,
    waker: &dyn SubstrateWaker,
    stale_dispatch_timeout: Duration,
    now: SystemTime,
) -> Result<WakeEmissionOutcome> {
    let (_, events) = match read_events(coord_state_file) {
        Ok(r) => r,
        Err(e) => {
            // Fresh coord with no log yet → no candidates and no
            // recoveries. Not an error.
            eprintln!(
                "info: wake_candidates_pass found no readable coord log at {} ({}); skipping",
                coord_state_file.display(),
                e
            );
            return Ok(WakeEmissionOutcome {
                events_emitted: 0,
                wakes_invoked: 0,
                sidecars_released: 0,
                recoveries_fired: 0,
            });
        }
    };

    // ----- Recovery pass first -----
    let mut recoveries_fired = 0usize;
    let mut recovery_wakes = 0usize;
    for event in &events {
        if event_kind(event) != Some(REQUESTER_WOKEN) {
            continue;
        }
        let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload else {
            continue;
        };
        let Some(requested_by) = fields.get("requested_by").and_then(|v| v.as_str()) else {
            continue;
        };
        // Parse woken_at from the event's timestamp envelope field.
        let woken_at = match parse_rfc3339_millis(&event.timestamp) {
            Some(t) => t,
            None => continue,
        };
        if maybe_recover_stale_wake(
            sessions_dir,
            requested_by,
            woken_at,
            now,
            stale_dispatch_timeout,
            waker,
        )
        .is_some()
        {
            recoveries_fired += 1;
            recovery_wakes += 1;
        }
    }

    // ----- New-candidates pass -----
    let open = find_open_dispatches(&events);
    let candidates = find_wake_candidates(koto_root, sessions_dir, &open);

    // Group candidates by requested_by, threading dispatch_epoch in
    // parallel so the emitted RequesterWoken carries the matching
    // (id, epoch) pairs.
    let mut by_requester: std::collections::BTreeMap<String, (Vec<ValidatedSessionId>, Vec<u32>)> =
        std::collections::BTreeMap::new();
    for c in candidates {
        let entry = by_requester.entry(c.requested_by).or_default();
        entry.0.push(c.child_session_id);
        entry.1.push(c.dispatch_epoch);
    }

    let mut events_emitted = 0usize;
    let mut wakes_invoked = recovery_wakes;
    let mut sidecars_released = 0usize;
    for (requested_by, (kids, epochs)) in by_requester {
        let (e, w, s) = emit_one_wake_batch(
            coord_state_file,
            sessions_dir,
            &requested_by,
            &kids,
            &epochs,
            waker,
            now,
        )?;
        events_emitted += e;
        wakes_invoked += w;
        sidecars_released += s;
    }

    Ok(WakeEmissionOutcome {
        events_emitted,
        wakes_invoked,
        sidecars_released,
        recoveries_fired,
    })
}

/// Extract the `kind` value from an [`EvidenceSubmitted`] event's
/// `fields` map. Returns `None` for non-`EvidenceSubmitted` events.
fn event_kind(event: &Event) -> Option<&str> {
    if let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload {
        fields.get("kind").and_then(|v| v.as_str())
    } else {
        None
    }
}

/// Parse the RFC 3339 millisecond timestamp produced by
/// [`crate::engine::claim::format_rfc3339_millis`] back into a
/// `SystemTime`. Returns `None` on parse failure.
///
/// The format is `YYYY-MM-DDTHH:MM:SS.sssZ`. We parse it back via a
/// tiny inline parser rather than pulling chrono so the wake module
/// stays minimal.
fn parse_rfc3339_millis(ts: &str) -> Option<SystemTime> {
    // Expected format: YYYY-MM-DDTHH:MM:SS.sssZ or YYYY-MM-DDTHH:MM:SSZ
    let bytes = ts.as_bytes();
    if bytes.len() < 20 || bytes[bytes.len() - 1] != b'Z' {
        return None;
    }
    let parse_n = |start: usize, end: usize| -> Option<i64> {
        std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()
    };
    let year = parse_n(0, 4)?;
    let month = parse_n(5, 7)?;
    let day = parse_n(8, 10)?;
    let hour = parse_n(11, 13)?;
    let minute = parse_n(14, 16)?;
    let second = parse_n(17, 19)?;
    // Optional .sss
    let millis = if bytes.len() >= 24 && bytes[19] == b'.' {
        parse_n(20, 23).unwrap_or(0)
    } else {
        0
    };

    // Naive UTC → unix seconds via days-since-epoch arithmetic.
    let secs = unix_seconds_from_ymdhms(year, month, day, hour, minute, second)?;
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64) + Duration::from_millis(millis as u64))
}

/// Convert (year, month, day, hour, minute, second) UTC to unix seconds.
/// Returns `None` for nonsense inputs (year < 1970, month/day out of range).
fn unix_seconds_from_ymdhms(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if y < 1970
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&mi)
        || !(0..=60).contains(&s)
    {
        return None;
    }
    // Days from 1970-01-01 to year-month-day.
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = |yy: i64| -> bool { (yy % 4 == 0 && yy % 100 != 0) || (yy % 400 == 0) };
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if is_leap(yy) { 366 } else { 365 };
    }
    for m in 1..mo {
        days += days_in_month[(m - 1) as usize];
        if m == 2 && is_leap(y) {
            days += 1;
        }
    }
    days += d - 1;
    Some(days * 86_400 + h * 3_600 + mi * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[allow(dead_code)]
    fn sid(s: &str) -> ValidatedSessionId {
        ValidatedSessionId::new(s).expect("test id valid")
    }

    /// Mock SubstrateWaker for tests; records every wake invocation.
    #[derive(Default)]
    struct RecordingWaker {
        wakes: Mutex<Vec<String>>,
        fail_on: Mutex<HashSet<String>>,
    }
    impl RecordingWaker {
        fn count(&self) -> usize {
            self.wakes.lock().unwrap().len()
        }
        #[allow(dead_code)]
        fn calls(&self) -> Vec<String> {
            self.wakes.lock().unwrap().clone()
        }
    }
    impl SubstrateWaker for RecordingWaker {
        fn wake(&self, session_id: &ValidatedSessionId) -> Result<(), EngineError> {
            let id = session_id.as_str().to_string();
            if self.fail_on.lock().unwrap().contains(&id) {
                return Err(EngineError::StateNotFound(id));
            }
            self.wakes.lock().unwrap().push(id);
            Ok(())
        }
    }

    fn build_child_dispatched_event(seq: u64, child_id: &str, timestamp: &str) -> Event {
        build_child_dispatched_event_with_epoch(seq, child_id, 0, timestamp)
    }

    fn build_child_dispatched_event_with_epoch(
        seq: u64,
        child_id: &str,
        dispatch_epoch: u32,
        timestamp: &str,
    ) -> Event {
        let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
        fields.insert("kind".into(), serde_json::json!("ChildDispatched"));
        fields.insert("child_session_id".into(), serde_json::json!(child_id));
        fields.insert("coord_id".into(), serde_json::json!("coord-1"));
        fields.insert("dispatch_epoch".into(), serde_json::json!(dispatch_epoch));
        Event {
            seq,
            timestamp: timestamp.into(),
            event_type: "evidence_submitted".into(),
            payload: EventPayload::EvidenceSubmitted {
                state: "request_store.dispatch".into(),
                fields,
                submitter_cwd: None,
            },
            idempotency_hash: None,
        }
    }

    fn build_requester_woken_event(
        seq: u64,
        children: &[&str],
        requested_by: &str,
        timestamp: &str,
    ) -> Event {
        // Default to all-zero epochs for the legacy convenience builder;
        // tests covering re-dispatch supply epochs via the explicit
        // builder below.
        let epochs: Vec<u32> = vec![0; children.len()];
        build_requester_woken_event_with_epochs(seq, children, &epochs, requested_by, timestamp)
    }

    fn build_requester_woken_event_with_epochs(
        seq: u64,
        children: &[&str],
        epochs: &[u32],
        requested_by: &str,
        timestamp: &str,
    ) -> Event {
        let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
        fields.insert("kind".into(), serde_json::json!("RequesterWoken"));
        fields.insert("child_session_ids".into(), serde_json::json!(children));
        fields.insert("child_dispatch_epochs".into(), serde_json::json!(epochs));
        fields.insert("requested_by".into(), serde_json::json!(requested_by));
        fields.insert("child_count".into(), serde_json::json!(children.len()));
        fields.insert(
            "summary".into(),
            serde_json::json!(format!("{} children completed", children.len())),
        );
        Event {
            seq,
            timestamp: timestamp.into(),
            event_type: "evidence_submitted".into(),
            payload: EventPayload::EvidenceSubmitted {
                state: "request_store.wake".into(),
                fields,
                submitter_cwd: None,
            },
            idempotency_hash: None,
        }
    }

    #[test]
    fn find_open_dispatches_returns_empty_when_no_dispatches() {
        let events: Vec<Event> = vec![];
        let open = find_open_dispatches(&events);
        assert!(open.is_empty());
    }

    #[test]
    fn find_open_dispatches_returns_all_unwoken_dispatches() {
        let events = vec![
            build_child_dispatched_event(1, "parent.task-a", "2026-05-24T00:00:01.000Z"),
            build_child_dispatched_event(2, "parent.task-b", "2026-05-24T00:00:02.000Z"),
        ];
        let open = find_open_dispatches(&events);
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].child_session_id.as_str(), "parent.task-a");
        assert_eq!(open[1].child_session_id.as_str(), "parent.task-b");
    }

    #[test]
    fn find_open_dispatches_excludes_already_woken_children() {
        let events = vec![
            build_child_dispatched_event(1, "parent.task-a", "2026-05-24T00:00:01.000Z"),
            build_child_dispatched_event(2, "parent.task-b", "2026-05-24T00:00:02.000Z"),
            build_requester_woken_event(
                3,
                &["parent.task-a"],
                "parent",
                "2026-05-24T00:00:03.000Z",
            ),
        ];
        let open = find_open_dispatches(&events);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].child_session_id.as_str(), "parent.task-b");
    }

    #[test]
    fn find_open_dispatches_handles_batched_wake() {
        let events = vec![
            build_child_dispatched_event(1, "parent.task-a", "2026-05-24T00:00:01.000Z"),
            build_child_dispatched_event(2, "parent.task-b", "2026-05-24T00:00:02.000Z"),
            build_child_dispatched_event(3, "parent.task-c", "2026-05-24T00:00:03.000Z"),
            build_requester_woken_event(
                4,
                &["parent.task-a", "parent.task-b"],
                "parent",
                "2026-05-24T00:00:04.000Z",
            ),
        ];
        let open = find_open_dispatches(&events);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].child_session_id.as_str(), "parent.task-c");
    }

    /// Regression test for the wake-filter (id, epoch) keying fix.
    ///
    /// Scenario: dispatch child A at epoch 0 → A terminal → wake fires
    /// → epoch bumps via header rewrite → re-dispatch A at epoch 1 →
    /// A terminal → a SECOND wake must fire (the prior wake recorded
    /// `(A, 0)`, not bare `A`).
    ///
    /// Pre-fix bug: `already_woken` was keyed on bare `child_session_id`,
    /// so the second `ChildDispatched(A, epoch=1)` was filtered against
    /// the first wake's record and the second wake was silently
    /// dropped. Post-fix the (id, epoch) pair survives the filter.
    #[test]
    fn find_open_dispatches_re_dispatch_at_higher_epoch_is_not_filtered_by_prior_wake() {
        let events = vec![
            // Epoch 0 dispatch + wake.
            build_child_dispatched_event_with_epoch(
                1,
                "parent.task-a",
                0,
                "2026-05-24T00:00:01.000Z",
            ),
            build_requester_woken_event_with_epochs(
                2,
                &["parent.task-a"],
                &[0],
                "parent",
                "2026-05-24T00:00:02.000Z",
            ),
            // Epoch 1 re-dispatch (no second wake yet).
            build_child_dispatched_event_with_epoch(
                3,
                "parent.task-a",
                1,
                "2026-05-24T00:00:03.000Z",
            ),
        ];
        let open = find_open_dispatches(&events);
        assert_eq!(open.len(), 1, "second dispatch must surface as open");
        assert_eq!(open[0].child_session_id.as_str(), "parent.task-a");
        assert_eq!(open[0].dispatch_epoch, 1);
    }

    /// Companion to the regression test: after the SECOND wake fires
    /// (covering the epoch=1 record), both (A, 0) and (A, 1) are
    /// filtered and no further opens surface.
    #[test]
    fn find_open_dispatches_both_epoch_wakes_filter_completely() {
        let events = vec![
            build_child_dispatched_event_with_epoch(
                1,
                "parent.task-a",
                0,
                "2026-05-24T00:00:01.000Z",
            ),
            build_requester_woken_event_with_epochs(
                2,
                &["parent.task-a"],
                &[0],
                "parent",
                "2026-05-24T00:00:02.000Z",
            ),
            build_child_dispatched_event_with_epoch(
                3,
                "parent.task-a",
                1,
                "2026-05-24T00:00:03.000Z",
            ),
            build_requester_woken_event_with_epochs(
                4,
                &["parent.task-a"],
                &[1],
                "parent",
                "2026-05-24T00:00:04.000Z",
            ),
        ];
        let open = find_open_dispatches(&events);
        assert!(open.is_empty(), "both wakes should cover both dispatches");
    }

    /// Backward-compat: legacy `RequesterWoken` events written before
    /// this fix do NOT carry a `child_dispatch_epochs` array. Pre-fix
    /// wake records cover epoch 0 (the only epoch that existed on
    /// disk pre-fix), so a legacy wake-of-A still filters
    /// `ChildDispatched(A, epoch=0)` and DOES NOT filter
    /// `ChildDispatched(A, epoch=1)`.
    #[test]
    fn find_open_dispatches_legacy_wake_without_epochs_array_covers_epoch_zero() {
        // Hand-build a legacy wake event with no child_dispatch_epochs
        // field, simulating an on-disk record from before this fix.
        let mut legacy_wake_fields: HashMap<String, serde_json::Value> = HashMap::new();
        legacy_wake_fields.insert("kind".into(), serde_json::json!("RequesterWoken"));
        legacy_wake_fields.insert(
            "child_session_ids".into(),
            serde_json::json!(["parent.task-a"]),
        );
        legacy_wake_fields.insert("requested_by".into(), serde_json::json!("parent"));
        legacy_wake_fields.insert("child_count".into(), serde_json::json!(1));
        legacy_wake_fields.insert("summary".into(), serde_json::json!("1 children completed"));
        let legacy_wake = Event {
            seq: 2,
            timestamp: "2026-05-24T00:00:02.000Z".into(),
            event_type: "evidence_submitted".into(),
            payload: EventPayload::EvidenceSubmitted {
                state: "request_store.wake".into(),
                fields: legacy_wake_fields,
                submitter_cwd: None,
            },
            idempotency_hash: None,
        };
        let events = vec![
            build_child_dispatched_event_with_epoch(
                1,
                "parent.task-a",
                0,
                "2026-05-24T00:00:01.000Z",
            ),
            legacy_wake,
            build_child_dispatched_event_with_epoch(
                3,
                "parent.task-a",
                1,
                "2026-05-24T00:00:03.000Z",
            ),
        ];
        let open = find_open_dispatches(&events);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].child_session_id.as_str(), "parent.task-a");
        assert_eq!(open[0].dispatch_epoch, 1);
    }

    #[test]
    fn event_kind_extracts_from_evidence_submitted() {
        let event = build_child_dispatched_event(1, "parent.task-a", "2026-05-24T00:00:01.000Z");
        assert_eq!(event_kind(&event), Some("ChildDispatched"));
    }

    #[test]
    fn parse_rfc3339_millis_round_trip() {
        let t = SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_716_580_000)
            + Duration::from_millis(500);
        let s = format_rfc3339_millis(t);
        let parsed = parse_rfc3339_millis(&s).expect("parse");
        // Allow a small rounding window.
        let dur = parsed
            .duration_since(t)
            .or_else(|_| t.duration_since(parsed))
            .unwrap();
        assert!(dur < Duration::from_millis(2), "drift {:?}", dur);
    }

    #[test]
    fn parse_rfc3339_millis_rejects_malformed() {
        assert!(parse_rfc3339_millis("not-a-timestamp").is_none());
        assert!(parse_rfc3339_millis("2026-05-24T00:00:01.000").is_none()); // missing Z
        assert!(parse_rfc3339_millis("").is_none());
    }

    #[test]
    fn maybe_recover_does_not_fire_within_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let waker = RecordingWaker::default();
        let woken_at = SystemTime::now() - Duration::from_secs(10);
        let result = maybe_recover_stale_wake(
            tmp.path(),
            "parent",
            woken_at,
            SystemTime::now(),
            Duration::from_secs(600),
            &waker,
        );
        assert!(result.is_none());
        assert_eq!(waker.count(), 0);
    }

    #[test]
    fn maybe_recover_does_not_fire_when_requester_active() {
        let tmp = tempfile::tempdir().unwrap();
        // Create requester's session dir + log; touch its mtime to a
        // time AFTER woken_at (requester has progressed).
        let requester_dir = tmp.path().join("parent");
        std::fs::create_dir_all(&requester_dir).unwrap();
        let req_path = requester_dir.join(state_file_name("parent"));
        std::fs::write(&req_path, b"").unwrap();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(700);
        // Bump the requester's log mtime to "now" (after woken_at).
        let recent_ft = filetime::FileTime::from_system_time(now);
        filetime::set_file_mtime(&req_path, recent_ft).unwrap();
        let waker = RecordingWaker::default();
        let result = maybe_recover_stale_wake(
            tmp.path(),
            "parent",
            woken_at,
            now,
            Duration::from_secs(600),
            &waker,
        );
        assert!(result.is_none());
        assert_eq!(waker.count(), 0);
    }
}
