use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::engine::errors::EngineError;
use crate::engine::types::{
    Event, EventPayload, MachineState, StateFileHeader, CURRENT_SCHEMA_VERSION,
};

/// Write the header line to a new state file.
///
/// Creates the file with mode 0600 on unix. The header has no seq field.
pub fn append_header(path: &Path, header: &StateFileHeader) -> anyhow::Result<()> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(false);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let line = serde_json::to_string(header)
        .map_err(|e| anyhow::anyhow!("failed to serialize header: {}", e))?;

    writeln!(file, "{}", line)
        .map_err(|e| anyhow::anyhow!("failed to write header to {}: {}", path.display(), e))?;

    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;

    Ok(())
}

/// Append one event to the state file, auto-assigning its seq number.
///
/// Reads the file to find the last event's seq (or 0 if no events yet),
/// then appends with `max_seq + 1`. Creates the file with mode 0600 on
/// unix if it doesn't exist. Calls `sync_data()` after every write.
pub fn append_event(path: &Path, payload: &EventPayload, timestamp: &str) -> anyhow::Result<u64> {
    debug_assert!(
        !matches!(payload, EventPayload::Unknown { .. }),
        "Unknown events must not be passed to append_event"
    );
    // Determine the next seq by reading the current last seq.
    let next_seq = if path.exists() {
        read_last_seq(path)? + 1
    } else {
        1
    };

    let event = Event {
        seq: next_seq,
        timestamp: timestamp.to_string(),
        event_type: payload.type_name().to_string(),
        payload: payload.clone(),
        idempotency_hash: None,
    };

    let mut opts = OpenOptions::new();
    opts.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let line = serde_json::to_string(&event)
        .map_err(|e| anyhow::anyhow!("failed to serialize event: {}", e))?;

    writeln!(file, "{}", line)
        .map_err(|e| anyhow::anyhow!("failed to write event to {}: {}", path.display(), e))?;

    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;

    Ok(next_seq)
}

/// Outcome of an idempotent append.
///
/// Distinguishes "this call wrote a new event" from "this call observed
/// a prior identical event and short-circuited". Both return the
/// authoritative seq number the caller should reference; only
/// [`AppendOutcome::Written`] increments the on-disk event count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// A new event was appended at the returned seq. The file gained
    /// one line and one fsync.
    Written { seq: u64 },
    /// An identical prior event was found via the idempotency hash;
    /// no write or fsync occurred. The returned seq points at the
    /// prior event so the caller can reference it.
    Idempotent { seq: u64 },
}

impl AppendOutcome {
    /// Return the seq number regardless of whether a write occurred.
    pub fn seq(&self) -> u64 {
        match self {
            AppendOutcome::Written { seq } | AppendOutcome::Idempotent { seq } => *seq,
        }
    }
}

/// Compute the SHA-256 hex digest of canonical-JSON serialization of
/// `(state_name, payload)`.
///
/// "Canonical" means: serde-serialize the tuple, parse the result as a
/// [`serde_json::Value`], recursively sort all object keys, then
/// re-serialize without whitespace. Two payloads that differ only in
/// key order or whitespace produce identical hashes, matching the R17
/// idempotency contract.
///
/// `state_name` is included alongside the payload so identical payloads
/// at different states do not short-circuit each other. For payload
/// variants whose serialized form already embeds a `state` field, this
/// is redundant-but-defensive; for payloads with no `state` (e.g.
/// `WorkflowInitialized`, `IntentUpdated`), the explicit `state_name`
/// preserves the hash-domain contract.
pub fn idempotency_hash(state_name: &str, payload: &EventPayload) -> String {
    let payload_value = serde_json::to_value(payload).expect("EventPayload is always serializable");
    let domain = serde_json::json!({
        "state_name": state_name,
        "payload": payload_value,
    });
    let canonical = canonicalize_json(domain);
    let serialized = serde_json::to_string(&canonical).expect("canonical JSON is serializable");
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hex::encode(hasher.finalize())
}

/// Recursively sort object keys in a [`serde_json::Value`] so the
/// re-serialized form is deterministic regardless of input key order.
///
/// Arrays preserve their order (semantic). Objects are rebuilt with
/// sorted keys (canonicalization).
fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, canonicalize_json(v));
            }
            // BTreeMap iterates in sorted key order; build a
            // serde_json::Map from the sorted entries.
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonicalize_json).collect())
        }
        other => other,
    }
}

/// Append an event with idempotency-hash short-circuit semantics.
///
/// Behavior depends on `hash`:
///
/// - `None`: identical to [`append_event`]. The event is appended
///   unconditionally; no hash field is written.
/// - `Some(h)`: scan the on-disk log for an event whose stored
///   `idempotency_hash` equals `h`.
///   - If found AND its payload bytes-equal the new payload, return
///     [`AppendOutcome::Idempotent`] with the prior seq; no write, no
///     fsync (PRD R17).
///   - If found AND its payload differs, return
///     [`EngineError::ConcurrentSubmissionConflict`] with the supplied
///     `state_name`; no write, no fsync (PRD OQ8).
///   - If not found, append a new event with `idempotency_hash: Some(h)`
///     and return [`AppendOutcome::Written`].
///
/// The hash-vs-payload bytes-equal check is belt-and-suspenders: SHA-256
/// preimage collisions are cryptographically negligible, but if a future
/// schema change makes the hash domain narrower than the payload (an
/// unintended regression) the bytes-equal check catches it.
///
/// Concurrent identical retries are serialized via [`acquire_state_flock`]
/// so the race-condition AC (N=32 concurrent identical retries → 1
/// event on disk) holds. The flock guards the read-then-write window;
/// readers outside this path are unaffected.
pub fn append_event_idempotent(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
    state_name: &str,
    hash: Option<&str>,
) -> anyhow::Result<AppendOutcome> {
    debug_assert!(
        !matches!(payload, EventPayload::Unknown { .. }),
        "Unknown events must not be passed to append_event_idempotent"
    );

    // No hash → fall back to the non-idempotent path.
    let Some(h) = hash else {
        let seq = append_event(path, payload, timestamp)?;
        return Ok(AppendOutcome::Written { seq });
    };

    // Take an exclusive lock on the state file for the duration of the
    // read-then-write window. The lock file is the same as the state
    // file; concurrent readers via `read_events` do NOT take a lock and
    // are unaffected (advisory).
    let _guard = acquire_state_flock(path)?;

    // Scan for a prior event with the same hash. Read line-by-line so
    // a malformed final line doesn't break the scan (mirrors
    // `read_events` tolerance).
    if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read state file {}: {}", path.display(), e))?;
        // Skip the header line; events start at line 2 (1-indexed).
        for line in content.lines().skip(1) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let prior_hash = parsed.get("idempotency_hash").and_then(|v| v.as_str());
            if prior_hash != Some(h) {
                continue;
            }
            let prior_seq = parsed
                .get("seq")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| anyhow::anyhow!("prior event missing seq"))?;
            // Bytes-equal payload check (belt-and-suspenders).
            let prior_payload = parsed
                .get("payload")
                .ok_or_else(|| anyhow::anyhow!("prior event missing payload"))?;
            let new_payload_value =
                serde_json::to_value(payload).expect("EventPayload is always serializable");
            if prior_payload == &new_payload_value {
                return Ok(AppendOutcome::Idempotent { seq: prior_seq });
            } else {
                return Err(EngineError::ConcurrentSubmissionConflict {
                    session_id: extract_session_id_from_path(path),
                    state_name: state_name.to_string(),
                }
                .into());
            }
        }
    }

    // No prior hash hit → append a new event with the hash stored.
    let next_seq = if path.exists() {
        read_last_seq(path)? + 1
    } else {
        1
    };
    let event = Event {
        seq: next_seq,
        timestamp: timestamp.to_string(),
        event_type: payload.type_name().to_string(),
        payload: payload.clone(),
        idempotency_hash: Some(h.to_string()),
    };

    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;
    let line = serde_json::to_string(&event)
        .map_err(|e| anyhow::anyhow!("failed to serialize event: {}", e))?;
    writeln!(file, "{}", line)
        .map_err(|e| anyhow::anyhow!("failed to write event to {}: {}", path.display(), e))?;
    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;

    Ok(AppendOutcome::Written { seq: next_seq })
}

/// Acquire an exclusive `flock(LOCK_EX)` on the state file. The lock
/// is released when the returned `File` is dropped.
///
/// Used by [`append_event_idempotent`] to serialize the read-then-write
/// window so concurrent identical retries collapse to a single write
/// rather than racing past the hash scan.
#[cfg(unix)]
fn acquire_state_flock(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    // Open or create the state file (we need a valid fd to flock; the
    // append_event_idempotent caller may be writing the very first
    // event so we must tolerate non-existent paths).
    let mut opts = OpenOptions::new();
    opts.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path).map_err(|e| {
        anyhow::anyhow!(
            "failed to open state file for lock {}: {}",
            path.display(),
            e
        )
    })?;
    let fd = file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if ret != 0 {
        return Err(anyhow::anyhow!(
            "failed to acquire flock on {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn acquire_state_flock(_path: &Path) -> anyhow::Result<std::fs::File> {
    // Non-unix targets: idempotency check is best-effort without a
    // cross-process lock. Concurrent retries on Windows are extremely
    // unlikely in koto's single-coordinator model; falling through
    // produces correct semantics in the no-contention case.
    Err(anyhow::anyhow!("flock not available on this platform"))
}

/// Extract the session id (workflow name) from a state file path.
///
/// State files live at `<sessions_root>/<session_id>/koto-<session_id>.state.jsonl`.
/// The session id is the file's parent directory's basename. Used by
/// [`append_event_idempotent`] to populate the
/// [`EngineError::ConcurrentSubmissionConflict`] envelope; falls back to
/// `"<unknown>"` if the path doesn't follow the convention.
fn extract_session_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// fsync the three log files that must be durable BEFORE the
/// coordinator's `substrate_wake()` call (R19 / Issue 12 Decision 2
/// sub-question 2):
///
/// 1. The child's terminal-evidence log
/// 2. The coordinator's log (which carries the appended `ChildDispatched` event)
/// 3. The coordinator's log (which carries the appended `RequesterWoken` event)
///
/// Points 2 and 3 are the same file; the helper fsyncs it twice to
/// reflect the design's three-point discipline. The fsync of an
/// already-fsync'd file is a benign no-op on Linux/macOS.
///
/// `child_log_path` and `coord_log_path` are caller-supplied so the
/// helper can be unit-tested without a full backend; the wake-emission
/// path (Issue 15) resolves them from the backend's `session_dir(id)`.
///
/// Returns an error if either fsync fails; the wake-delivery primitive
/// must NOT be invoked when this helper returns `Err`.
pub fn fsync_wake_preconditions(
    child_log_path: &Path,
    coord_log_path: &Path,
) -> anyhow::Result<()> {
    fsync_log_file(child_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: child log {} fsync failed: {}",
            child_log_path.display(),
            e
        )
    })?;
    fsync_log_file(coord_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: coord log {} (post-ChildDispatched) fsync failed: {}",
            coord_log_path.display(),
            e
        )
    })?;
    fsync_log_file(coord_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: coord log {} (post-RequesterWoken) fsync failed: {}",
            coord_log_path.display(),
            e
        )
    })?;
    Ok(())
}

/// Open `path` and call `sync_all`. Missing files are NOT tolerated —
/// the caller must guarantee the log has been initialized before
/// invoking `fsync_wake_preconditions`.
fn fsync_log_file(path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    file.sync_all()
}

/// Read the last event's seq from the file. Returns 0 if no events exist.
///
/// Reads only the last non-empty non-header line's seq, since in a valid
/// file seq equals line index. This avoids silently masking corruption
/// where events appear out of order.
fn read_last_seq(path: &Path) -> anyhow::Result<u64> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read state file {}: {}", path.display(), e))?;

    // Find the last non-empty line after the header.
    let event_lines: Vec<&str> = content.lines().skip(1).collect();
    let last_line = event_lines.iter().rev().find(|l| !l.trim().is_empty());

    match last_line {
        None => Ok(0),
        Some(line) => {
            let val: serde_json::Value = serde_json::from_str(line.trim())
                .map_err(|e| anyhow::anyhow!("failed to parse last event line: {}", e))?;
            Ok(val.get("seq").and_then(|s| s.as_u64()).unwrap_or(0))
        }
    }
}

/// Read the header line from a state file.
///
/// Parses the first line as a `StateFileHeader`. If it doesn't parse,
/// the file is treated as corrupted.
pub fn read_header(path: &Path) -> anyhow::Result<StateFileHeader> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let reader = BufReader::new(file);
    let first_line = reader
        .lines()
        .next()
        .ok_or_else(|| EngineError::StateFileCorrupted("state file is empty".to_string()))?
        .map_err(|e| anyhow::anyhow!("failed to read first line: {}", e))?;

    let trimmed = first_line.trim();
    if trimmed.is_empty() {
        return Err(
            EngineError::StateFileCorrupted("state file has empty first line".to_string()).into(),
        );
    }

    parse_header(trimmed)
}

/// Parse a header line as a `StateFileHeader`.
fn parse_header(first_line: &str) -> anyhow::Result<StateFileHeader> {
    let header = serde_json::from_str::<StateFileHeader>(first_line)
        .map_err(|e| EngineError::StateFileCorrupted(format!("failed to parse header: {}", e)))?;
    if header.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(EngineError::IncompatibleSchemaVersion {
            found: header.schema_version,
            max_supported: CURRENT_SCHEMA_VERSION,
        }
        .into());
    }
    Ok(header)
}

/// Read all events from the JSONL state file at `path`.
///
/// Parses the header line first, then reads events with seq validation.
/// Returns `(header, events)`.
///
/// Validation rules:
/// - Each event's seq must be exactly `prev_seq + 1` (monotonic by 1)
/// - A gap in a non-final line produces a `state_file_corrupted` error
/// - A malformed final line is recovered: events up to last valid are returned
///   with a warning to stderr
pub fn read_events(path: &Path) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let mut lines: Vec<&str> = content.lines().collect();
    // Remove trailing empty lines
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        return Err(EngineError::StateFileCorrupted("state file is empty".to_string()).into());
    }

    // Parse header (first line)
    let header = parse_header(lines[0].trim())?;

    // Parse events (remaining lines)
    let event_lines = &lines[1..];
    let mut events = Vec::new();
    let mut expected_seq: u64 = 1;

    for (i, line) in event_lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_last_line = i == event_lines.len() - 1;

        match serde_json::from_str::<Event>(trimmed) {
            Ok(event) => {
                if event.seq != expected_seq {
                    return Err(EngineError::StateFileCorrupted(format!(
                        "sequence gap at line {}: expected seq {}, got {}",
                        i + 2, // +2 because line 1 is header, line numbers are 1-indexed
                        expected_seq,
                        event.seq
                    ))
                    .into());
                }
                expected_seq = event.seq + 1;
                events.push(event);
            }
            Err(e) => {
                if is_last_line {
                    // Truncated final line: recoverable. Warn and return what we have.
                    eprintln!(
                        "warning: truncated final line in {}, recovering: {}",
                        path.display(),
                        e
                    );
                    break;
                } else {
                    // Non-final malformed line: corruption.
                    return Err(EngineError::StateFileCorrupted(format!(
                        "malformed event on line {}: {}",
                        i + 2,
                        e
                    ))
                    .into());
                }
            }
        }
    }

    Ok((header, events))
}

/// Derive the current state from an event log by replay.
///
/// Returns the `to` field of the last event whose type is `transitioned`,
/// `directed_transition`, or `rewound`. Returns `None` if no such event
/// exists.
pub fn derive_state_from_log(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::Transitioned { to, .. } => Some(to.clone()),
        EventPayload::DirectedTransition { to, .. } => Some(to.clone()),
        EventPayload::Rewound { to, .. } => Some(to.clone()),
        _ => None,
    })
}

/// Derive evidence for the current state from the event log.
///
/// Returns `evidence_submitted` events occurring after the most recent
/// state-changing event whose `to` field matches the current state.
/// State-changing events are: `transitioned`, `directed_transition`, `rewound`.
pub fn derive_evidence(events: &[Event]) -> Vec<&Event> {
    let current_state = match derive_state_from_log(events) {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find the most recent state-changing event whose `to` matches current state.
    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    });

    let start = match epoch_start_idx {
        Some(idx) => idx + 1,
        None => return Vec::new(),
    };

    events[start..]
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::EvidenceSubmitted { state, .. } if state == &current_state))
        .collect()
}

/// Derive decisions for the current state from the event log.
///
/// Returns `DecisionRecorded` events occurring after the most recent
/// state-changing event whose `to` field matches the current state.
/// State-changing events are: `transitioned`, `directed_transition`, `rewound`.
pub fn derive_decisions(events: &[Event]) -> Vec<&Event> {
    let current_state = match derive_state_from_log(events) {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find the most recent state-changing event whose `to` matches current state.
    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    });

    let start = match epoch_start_idx {
        Some(idx) => idx + 1,
        None => return Vec::new(),
    };

    events[start..]
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::DecisionRecorded { state, .. } if state == &current_state))
        .collect()
}

/// Derive gate overrides for the current state from the event log.
///
/// Returns `GateOverrideRecorded` events occurring after the most recent
/// state-changing event whose `to` field matches the current state.
/// State-changing events are: `transitioned`, `directed_transition`, `rewound`.
pub fn derive_overrides(events: &[Event]) -> Vec<&Event> {
    let current_state = match derive_state_from_log(events) {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find the most recent state-changing event whose `to` matches current state.
    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    });

    let start = match epoch_start_idx {
        Some(idx) => idx + 1,
        None => return Vec::new(),
    };

    events[start..]
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::GateOverrideRecorded { state, .. } if state == &current_state))
        .collect()
}

/// Derive all gate overrides from the event log, regardless of epoch.
///
/// Returns every `GateOverrideRecorded` event in the log.
pub fn derive_overrides_all(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::GateOverrideRecorded { .. }))
        .collect()
}

/// Derive the most recent gate evaluation output for the named gate within the
/// current epoch.
///
/// Returns the `output` field from the most recent `GateEvaluated` event for
/// `gate` that falls after the epoch boundary for the current state. Returns
/// `None` if no such event exists.
pub fn derive_last_gate_evaluated(events: &[Event], gate: &str) -> Option<serde_json::Value> {
    let current_state = derive_state_from_log(events)?;

    // Find the most recent state-changing event whose `to` matches current state.
    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    })?;

    let start = epoch_start_idx + 1;

    events[start..].iter().rev().find_map(|e| {
        if let EventPayload::GateEvaluated {
            gate: g, output, ..
        } = &e.payload
        {
            if g == gate {
                return Some(output.clone());
            }
        }
        None
    })
}

/// Derive full machine state from header and event log.
///
/// Uses `derive_state_from_log` for the current state, and extracts
/// template info from the header and workflow_initialized event.
pub fn derive_machine_state(header: &StateFileHeader, events: &[Event]) -> Option<MachineState> {
    let current_state = derive_state_from_log(events)?;

    // Find the template_path from the workflow_initialized event.
    let template_path = events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
        _ => None,
    })?;

    Some(MachineState {
        current_state,
        template_path,
        template_hash: header.template_hash.clone(),
    })
}

/// Derive per-state visit counts from the event log.
///
/// Counts the number of times each state has been entered via Transitioned,
/// DirectedTransition, or Rewound events. Returns a map from state name to
/// visit count.
pub fn derive_visit_counts(events: &[Event]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for event in events {
        let target = match &event.payload {
            EventPayload::Transitioned { to, .. } => Some(to.as_str()),
            EventPayload::DirectedTransition { to, .. } => Some(to.as_str()),
            EventPayload::Rewound { to, .. } => Some(to.as_str()),
            _ => None,
        };
        if let Some(state_name) = target {
            *counts.entry(state_name.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, StateFileHeader};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_header() -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: "test-wf".to_string(),
            template_hash: "deadbeef".to_string(),
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
            respawn_generation: None,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
        }
    }

    fn make_event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn write_state_file(
        dir: &Path,
        name: &str,
        header: &StateFileHeader,
        events: &[Event],
    ) -> std::path::PathBuf {
        let path = dir.join(format!("koto-{}.state.jsonl", name));
        let mut content = serde_json::to_string(header).unwrap();
        content.push('\n');
        for event in events {
            content.push_str(&serde_json::to_string(event).unwrap());
            content.push('\n');
        }
        std::fs::write(&path, &content).unwrap();
        path
    }

    // -----------------------------------------------------------------------
    // Schema version guard
    // -----------------------------------------------------------------------

    #[test]
    fn schema_version_1_is_accepted() {
        let dir = TempDir::new().unwrap();
        let header = make_header(); // schema_version: 1
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        )];
        let path = write_state_file(dir.path(), "sv1", &header, &events);
        assert!(read_events(&path).is_ok());
    }

    #[test]
    fn schema_version_2_returns_incompatible_error() {
        let dir = TempDir::new().unwrap();
        let mut header = make_header();
        header.schema_version = 2;
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        )];
        let path = write_state_file(dir.path(), "sv2", &header, &events);
        let err = read_events(&path).unwrap_err();
        let engine_err = err.downcast::<EngineError>().expect("must be EngineError");
        assert!(
            matches!(
                engine_err,
                EngineError::IncompatibleSchemaVersion {
                    found: 2,
                    max_supported: 1
                }
            ),
            "unexpected error: {:?}",
            engine_err
        );
    }

    // -----------------------------------------------------------------------
    // Header parsing
    // -----------------------------------------------------------------------

    #[test]
    fn read_events_with_valid_header_and_events() {
        let dir = TempDir::new().unwrap();
        let header = make_header();
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
        ];
        let path = write_state_file(dir.path(), "test", &header, &events);

        let (parsed_header, parsed_events) = read_events(&path).unwrap();
        assert_eq!(parsed_header, header);
        assert_eq!(parsed_events.len(), 2);
        assert_eq!(parsed_events[0].seq, 1);
        assert_eq!(parsed_events[1].seq, 2);
    }

    // -----------------------------------------------------------------------
    // Sequence gap detection
    // -----------------------------------------------------------------------

    #[test]
    fn sequence_gap_in_middle_produces_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-gap.state.jsonl");

        let header = make_header();
        let e1 = make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        );
        // Seq jumps from 1 to 3 (gap at seq 2)
        let e3 = make_event(
            3,
            EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
        );
        let e4 = make_event(
            4,
            EventPayload::Transitioned {
                from: Some("gather".to_string()),
                to: "plan".to_string(),
                condition_type: "gate".to_string(),
                skip_if_matched: None,
            },
        );

        let mut content = serde_json::to_string(&header).unwrap() + "\n";
        content += &serde_json::to_string(&e1).unwrap();
        content.push('\n');
        content += &serde_json::to_string(&e3).unwrap();
        content.push('\n');
        content += &serde_json::to_string(&e4).unwrap();
        content.push('\n');
        std::fs::write(&path, &content).unwrap();

        let result = read_events(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("sequence gap"),
            "error should mention sequence gap, got: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Truncated final line recovery
    // -----------------------------------------------------------------------

    #[test]
    fn truncated_final_line_recovered() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-trunc.state.jsonl");

        let header = make_header();
        let e1 = make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        );

        let mut content = serde_json::to_string(&header).unwrap() + "\n";
        content += &serde_json::to_string(&e1).unwrap();
        content.push('\n');
        content += "{\"seq\":2,\"truncated"; // malformed final line
        content.push('\n');
        std::fs::write(&path, &content).unwrap();

        let (parsed_header, events) = read_events(&path).unwrap();
        assert_eq!(parsed_header, header);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
    }

    // -----------------------------------------------------------------------
    // derive_state_from_log
    // -----------------------------------------------------------------------

    #[test]
    fn derive_state_from_log_with_transitioned() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
        ];
        assert_eq!(derive_state_from_log(&events), Some("analyze".to_string()));
    }

    #[test]
    fn derive_state_from_log_with_rewound() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                4,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
                    rationale: None,
                },
            ),
        ];
        assert_eq!(derive_state_from_log(&events), Some("gather".to_string()));
    }

    #[test]
    fn derive_state_from_log_empty() {
        assert_eq!(derive_state_from_log(&[]), None);
    }

    #[test]
    fn derive_state_from_log_only_init() {
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        )];
        assert_eq!(derive_state_from_log(&events), None);
    }

    // -----------------------------------------------------------------------
    // derive_evidence
    // -----------------------------------------------------------------------

    #[test]
    fn derive_evidence_returns_only_current_epoch() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("file".to_string(), serde_json::json!("old.txt"));
                        m
                    },
                    submitter_cwd: None,
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
                    rationale: None,
                },
            ),
            // New evidence after rewind
            make_event(
                6,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("file".to_string(), serde_json::json!("new.txt"));
                        m
                    },
                    submitter_cwd: None,
                },
            ),
        ];

        let evidence = derive_evidence(&events);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].seq, 6);
    }

    #[test]
    fn derive_evidence_rewind_clears_prior_evidence() {
        // Arrive at gather, submit evidence, transition, rewind to gather.
        // Old evidence should be gone (epoch boundary resets).
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("data".to_string(), serde_json::json!("stale"));
                        m
                    },
                    submitter_cwd: None,
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
                    rationale: None,
                },
            ),
        ];

        let evidence = derive_evidence(&events);
        assert!(
            evidence.is_empty(),
            "rewind should clear prior evidence (epoch boundary)"
        );
    }

    // -----------------------------------------------------------------------
    // append_event with seq assignment
    // -----------------------------------------------------------------------

    #[test]
    fn append_event_assigns_sequential_seq() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-seq.state.jsonl");

        // Write header first
        let header = make_header();
        append_header(&path, &header).unwrap();

        let seq1 = append_event(
            &path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(seq1, 1);

        let seq2 = append_event(
            &path,
            &EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
            "2026-01-01T00:00:01Z",
        )
        .unwrap();
        assert_eq!(seq2, 2);

        let seq3 = append_event(
            &path,
            &EventPayload::Rewound {
                from: "gather".to_string(),
                to: "start".to_string(),
                rationale: None,
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();
        assert_eq!(seq3, 3);

        // Verify by reading back
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(events[2].seq, 3);
    }

    // -----------------------------------------------------------------------
    // File permissions (unix only)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn newly_created_state_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-perm.state.jsonl");

        let header = make_header();
        append_header(&path, &header).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "file permissions should be 0600, got {:o}",
            mode
        );
    }

    // -----------------------------------------------------------------------
    // derive_machine_state
    // -----------------------------------------------------------------------

    #[test]
    fn derive_machine_state_from_header_and_events() {
        let header = make_header();
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
        ];

        let ms = derive_machine_state(&header, &events).unwrap();
        assert_eq!(ms.current_state, "gather");
        assert_eq!(ms.template_path, "/cache/abc.json");
        assert_eq!(ms.template_hash, "deadbeef");
    }

    #[test]
    fn derive_machine_state_returns_none_for_empty() {
        let header = make_header();
        assert!(derive_machine_state(&header, &[]).is_none());
    }

    #[test]
    fn derive_machine_state_returns_none_without_state_change() {
        let header = make_header();
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        )];
        // Only init event, no transitioned -- no current state derivable
        assert!(derive_machine_state(&header, &events).is_none());
    }

    // -----------------------------------------------------------------------
    // derive_decisions
    // -----------------------------------------------------------------------

    #[test]
    fn derive_decisions_returns_only_current_epoch() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::DecisionRecorded {
                    state: "implementation".to_string(),
                    decision: serde_json::json!({
                        "choice": "Use retry",
                        "rationale": "No batch endpoint"
                    }),
                },
            ),
            make_event(
                4,
                EventPayload::DecisionRecorded {
                    state: "implementation".to_string(),
                    decision: serde_json::json!({
                        "choice": "Skip migration",
                        "rationale": "Data volume too small"
                    }),
                },
            ),
        ];

        let decisions = derive_decisions(&events);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].seq, 3);
        assert_eq!(decisions[1].seq, 4);
    }

    #[test]
    fn derive_decisions_empty_after_rewind() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::DecisionRecorded {
                    state: "implementation".to_string(),
                    decision: serde_json::json!({
                        "choice": "Use retry",
                        "rationale": "No batch endpoint"
                    }),
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("implementation".to_string()),
                    to: "review".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "review".to_string(),
                    to: "implementation".to_string(),
                    rationale: None,
                },
            ),
        ];

        let decisions = derive_decisions(&events);
        assert!(
            decisions.is_empty(),
            "rewind should clear prior decisions (epoch boundary)"
        );
    }

    #[test]
    fn derive_decisions_ignores_other_state_decisions() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            // A decision tagged with a different state name should be ignored.
            make_event(
                3,
                EventPayload::DecisionRecorded {
                    state: "analysis".to_string(),
                    decision: serde_json::json!({
                        "choice": "Wrong state",
                        "rationale": "Should not appear"
                    }),
                },
            ),
            make_event(
                4,
                EventPayload::DecisionRecorded {
                    state: "implementation".to_string(),
                    decision: serde_json::json!({
                        "choice": "Correct state",
                        "rationale": "Should appear"
                    }),
                },
            ),
        ];

        let decisions = derive_decisions(&events);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].seq, 4);
    }

    // -----------------------------------------------------------------------
    // derive_overrides
    // -----------------------------------------------------------------------

    fn make_override_event(seq: u64, state: &str, gate: &str) -> Event {
        make_event(
            seq,
            EventPayload::GateOverrideRecorded {
                state: state.to_string(),
                gate: gate.to_string(),
                rationale: "test rationale".to_string(),
                override_applied: serde_json::json!({"exit_code": 0, "error": ""}),
                actual_output: serde_json::json!({"exit_code": 1, "error": "failed"}),
                timestamp: "2026-04-01T00:00:00Z".to_string(),
            },
        )
    }

    fn make_gate_evaluated_event(
        seq: u64,
        state: &str,
        gate: &str,
        output: serde_json::Value,
    ) -> Event {
        make_event(
            seq,
            EventPayload::GateEvaluated {
                state: state.to_string(),
                gate: gate.to_string(),
                output,
                outcome: "failed".to_string(),
                timestamp: "2026-04-01T00:00:00Z".to_string(),
            },
        )
    }

    #[test]
    fn derive_overrides_no_state_change_returns_empty() {
        // Event log contains only GateOverrideRecorded events with no preceding state-changing
        // event. derive_state_from_log returns None, so derive_overrides returns an empty Vec
        // because no epoch boundary can be established.
        let events = vec![
            make_override_event(1, "review", "ci-passes"),
            make_override_event(2, "review", "lint-passes"),
        ];

        let overrides = derive_overrides(&events);
        assert_eq!(overrides.len(), 0);
    }

    #[test]
    fn derive_overrides_after_transitioned_returns_epoch_overrides() {
        // Transitioned to current state, then GateOverrideRecorded events.
        // derive_overrides returns only the events after the transition.
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(2, "review", "ci-passes"),
            make_override_event(3, "review", "lint-passes"),
        ];

        let overrides = derive_overrides(&events);
        assert_eq!(overrides.len(), 2);
        assert_eq!(overrides[0].seq, 2);
        assert_eq!(overrides[1].seq, 3);
    }

    #[test]
    fn derive_overrides_after_rewound_resets_epoch() {
        // Rewound to current state; overrides after the rewind are returned.
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(2, "review", "ci-passes"),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("review".to_string()),
                    to: "deploy".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                4,
                EventPayload::Rewound {
                    from: "deploy".to_string(),
                    to: "review".to_string(),
                    rationale: None,
                },
            ),
            make_override_event(5, "review", "ci-passes"),
        ];

        let overrides = derive_overrides(&events);
        // Only the override after the rewind should be returned.
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].seq, 5);
    }

    #[test]
    fn derive_overrides_to_field_match_required() {
        // Transitioned to other_state, then some overrides, then Transitioned to current_state,
        // then more overrides. derive_overrides returns only the overrides after the second transition.
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "other_state".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(2, "other_state", "some-gate"),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("other_state".to_string()),
                    to: "review".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(4, "review", "ci-passes"),
            make_override_event(5, "review", "lint-passes"),
        ];

        let overrides = derive_overrides(&events);
        // Only overrides after the transition to "review" should be returned.
        assert_eq!(overrides.len(), 2);
        assert_eq!(overrides[0].seq, 4);
        assert_eq!(overrides[1].seq, 5);
    }

    #[test]
    fn derive_overrides_state_field_mismatch_excluded() {
        // A GateOverrideRecorded event whose state field does not match the
        // current state must be excluded, even when it falls after the epoch
        // boundary. This mirrors the guard applied by derive_decisions and
        // derive_evidence.
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            // Mismatched state field: should be excluded.
            make_override_event(2, "other_state", "ci-passes"),
            // Correct state field: should be included.
            make_override_event(3, "review", "lint-passes"),
        ];

        let overrides = derive_overrides(&events);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].seq, 3);
    }

    // -----------------------------------------------------------------------
    // derive_overrides_all
    // -----------------------------------------------------------------------

    #[test]
    fn derive_overrides_all_returns_across_epoch_boundaries() {
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(2, "review", "ci-passes"),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("review".to_string()),
                    to: "deploy".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_override_event(4, "deploy", "smoke-test"),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "deploy".to_string(),
                    to: "review".to_string(),
                    rationale: None,
                },
            ),
            make_override_event(6, "review", "lint-passes"),
        ];

        let all_overrides = derive_overrides_all(&events);
        assert_eq!(all_overrides.len(), 3);
        assert_eq!(all_overrides[0].seq, 2);
        assert_eq!(all_overrides[1].seq, 4);
        assert_eq!(all_overrides[2].seq, 6);
    }

    // -----------------------------------------------------------------------
    // derive_last_gate_evaluated
    // -----------------------------------------------------------------------

    #[test]
    fn derive_last_gate_evaluated_returns_most_recent_in_epoch() {
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_gate_evaluated_event(
                2,
                "review",
                "ci-passes",
                serde_json::json!({"exit_code": 1, "error": "timeout"}),
            ),
            make_gate_evaluated_event(
                3,
                "review",
                "ci-passes",
                serde_json::json!({"exit_code": 0, "error": ""}),
            ),
        ];

        let output = derive_last_gate_evaluated(&events, "ci-passes");
        assert!(output.is_some());
        let val = output.unwrap();
        assert_eq!(val["exit_code"], serde_json::json!(0));
    }

    #[test]
    fn derive_last_gate_evaluated_returns_none_when_no_event() {
        let events = vec![make_event(
            1,
            EventPayload::Transitioned {
                from: None,
                to: "review".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
        )];

        let output = derive_last_gate_evaluated(&events, "ci-passes");
        assert!(output.is_none());
    }

    #[test]
    fn derive_last_gate_evaluated_epoch_scoped() {
        // GateEvaluated before the epoch boundary should not be returned.
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "review".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_gate_evaluated_event(
                2,
                "review",
                "ci-passes",
                serde_json::json!({"exit_code": 1, "error": "old failure"}),
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("review".to_string()),
                    to: "deploy".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                4,
                EventPayload::Rewound {
                    from: "deploy".to_string(),
                    to: "review".to_string(),
                    rationale: None,
                },
            ),
        ];

        // After rewinding to "review", the GateEvaluated event before the rewind is out of epoch.
        let output = derive_last_gate_evaluated(&events, "ci-passes");
        assert!(
            output.is_none(),
            "GateEvaluated before epoch boundary should not be returned"
        );
    }

    // -----------------------------------------------------------------------
    // derive_visit_counts
    // -----------------------------------------------------------------------

    #[test]
    fn derive_visit_counts_empty_events() {
        let counts = derive_visit_counts(&[]);
        assert!(counts.is_empty());
    }

    #[test]
    fn derive_visit_counts_single_transitioned() {
        let events = vec![make_event(
            1,
            EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
        )];
        let counts = derive_visit_counts(&events);
        assert_eq!(counts.len(), 1);
        assert_eq!(counts["gather"], 1);
    }

    #[test]
    fn derive_visit_counts_multiple_visits_same_state() {
        let events = vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
                    rationale: None,
                },
            ),
        ];
        let counts = derive_visit_counts(&events);
        assert_eq!(counts["gather"], 2);
        assert_eq!(counts["analyze"], 1);
    }

    #[test]
    fn derive_visit_counts_directed_transition() {
        let events = vec![make_event(
            1,
            EventPayload::DirectedTransition {
                from: "plan".to_string(),
                to: "implement".to_string(),
                rationale: None,
            },
        )];
        let counts = derive_visit_counts(&events);
        assert_eq!(counts.len(), 1);
        assert_eq!(counts["implement"], 1);
    }

    #[test]
    fn derive_visit_counts_rewound() {
        let events = vec![make_event(
            1,
            EventPayload::Rewound {
                from: "analyze".to_string(),
                to: "gather".to_string(),
                rationale: None,
            },
        )];
        let counts = derive_visit_counts(&events);
        assert_eq!(counts.len(), 1);
        assert_eq!(counts["gather"], 1);
    }

    #[test]
    fn derive_visit_counts_mixed_event_types() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: HashMap::new(),
                    submitter_cwd: None,
                },
            ),
            make_event(
                4,
                EventPayload::DirectedTransition {
                    from: "gather".to_string(),
                    to: "analyze".to_string(),
                    rationale: None,
                },
            ),
            make_event(
                5,
                EventPayload::DecisionRecorded {
                    state: "analyze".to_string(),
                    decision: serde_json::json!({"choice": "A"}),
                },
            ),
            make_event(
                6,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
                    rationale: None,
                },
            ),
        ];
        let counts = derive_visit_counts(&events);
        assert_eq!(counts["gather"], 2);
        assert_eq!(counts["analyze"], 1);
        // Non-entry events should not appear as keys
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn derive_visit_counts_ignores_non_entry_events() {
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            ),
            make_event(
                2,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: HashMap::new(),
                    submitter_cwd: None,
                },
            ),
            make_event(
                3,
                EventPayload::DecisionRecorded {
                    state: "gather".to_string(),
                    decision: serde_json::json!({"choice": "X"}),
                },
            ),
            make_event(
                4,
                EventPayload::IntegrationInvoked {
                    state: "gather".to_string(),
                    integration: "github".to_string(),
                    output: serde_json::json!(null),
                },
            ),
            make_event(
                5,
                EventPayload::DefaultActionExecuted {
                    state: "gather".to_string(),
                    command: "echo hi".to_string(),
                    exit_code: 0,
                    stdout: "hi\n".to_string(),
                    stderr: String::new(),
                },
            ),
            make_event(
                6,
                EventPayload::WorkflowCancelled {
                    state: "gather".to_string(),
                    reason: "test".to_string(),
                },
            ),
        ];
        let counts = derive_visit_counts(&events);
        assert!(counts.is_empty());
    }
}
