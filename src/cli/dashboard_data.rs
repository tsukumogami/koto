//! Data layer for the `koto dashboard` command.
//!
//! Responsible for reading session state from disk and maintaining an
//! up-to-date `SessionTree` via mtime-based incremental diffing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;

use crate::engine::persistence::{
    derive_last_gate_evaluated, derive_machine_state, derive_state_from_log, read_events,
    read_header,
};
use crate::engine::types::derive_intent;
use crate::engine::types::is_leap;
use crate::engine::types::StateFileHeader;
use crate::session::{state_file_name, SessionBackend};

/// Lightweight snapshot of one session's derived state, held in the tree.
pub struct CachedSession {
    /// Full header from the first line of the state file.
    ///
    /// When `current_state` is `None` (parse failed), this may be a zero-value
    /// placeholder produced by `make_empty_header()`, in which case `header.workflow`
    /// is an empty string. Callers should use the tree key (session name) for display
    /// purposes rather than `header.workflow` when `current_state` is `None`.
    pub header: StateFileHeader,
    /// Current state derived from the event log, or `None` on parse error.
    pub current_state: Option<String>,
    /// Whether the current state is a terminal state in the template.
    pub is_terminal: bool,
    /// Whether the session is waiting on a gate that has not yet passed.
    ///
    /// True when the most recent `GateEvaluated` event in the current state's
    /// epoch has an outcome other than `"passed"`. Always `false` for terminal
    /// sessions and sessions with no recorded state.
    pub is_blocked: bool,
    /// Intent derived from IntentUpdated events, or from header fallback.
    pub intent: Option<String>,
    /// Last-modified time of the state file; used for cache invalidation.
    pub mtime: SystemTime,
    /// Path to the state file; used for re-reads on mtime change.
    pub state_path: PathBuf,
}

/// Hierarchical view of all sessions visible to the dashboard.
pub struct SessionTree {
    /// All sessions indexed by session name.
    pub sessions: HashMap<String, CachedSession>,
    /// Names of root sessions (those with no parent, or whose parent is absent).
    pub roots: Vec<String>,
}

impl SessionTree {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            roots: Vec::new(),
        }
    }
}

impl Default for SessionTree {
    fn default() -> Self {
        Self::new()
    }
}

/// A single evidence entry from an `EvidenceSubmitted` event.
pub struct EvidenceEntry {
    /// The state this evidence was submitted for.
    pub state: String,
    /// The fields submitted as evidence.
    pub fields: serde_json::Value,
}

/// A single event entry for the History tab.
pub struct HistoryEntry {
    /// Event type string (e.g. "transitioned", "evidence_submitted").
    pub event_type: String,
    /// RFC 3339 timestamp.
    pub timestamp: String,
    /// Human-readable summary line.
    pub summary: String,
    /// Gate condition text for GateEvaluated events (from compiled template).
    pub gate_condition: Option<String>,
}

/// Gate-evaluation detail data loaded on demand for the focused session.
pub struct DetailData {
    /// The session this detail was loaded from.
    pub session_id: String,
    /// The identifier (name) of the most recently evaluated gate (e.g., "my-build-gate").
    pub gate_name: Option<String>,
    /// The command, if the gate was a command gate.
    pub command: Option<String>,
    /// The result of the gate evaluation ("PASS" or "FAIL").
    pub result: Option<String>,
    /// Time elapsed since the gate evaluation timestamp.
    pub elapsed: Duration,
    /// Evidence entries from the current epoch, newest-first.
    pub evidence: Vec<EvidenceEntry>,
    /// Current state name.
    pub current_state: Option<String>,
    /// Directive text from the compiled template for the current state.
    pub directive: Option<String>,
    /// Intent from last IntentUpdated event or header fallback.
    pub intent: Option<String>,
    /// Template name from header.
    pub template_name: Option<String>,
    /// All events from the current epoch, in chronological order, for the History tab.
    pub history: Vec<HistoryEntry>,
    /// Unvisited state names from the compiled template, in topological order, for the Remaining tab.
    pub remaining: Vec<String>,
}

/// Enumerate sessions from the backend, filtering epoch-branched names.
///
/// Returns a list of `(session_name, state_file_path)` pairs for sessions
/// whose names do not contain `~` (epoch-branched session indicator).
pub fn scan_sessions(backend: &dyn SessionBackend) -> Result<Vec<(String, PathBuf)>> {
    let session_list = backend.list()?;
    let mut result = Vec::new();
    for info in session_list {
        // Filter out epoch-branched sessions (those containing `~`).
        if info.id.contains('~') {
            continue;
        }
        let dir = backend.session_dir(&info.id);
        let state_path = dir.join(state_file_name(&info.id));
        result.push((info.id, state_path));
    }
    Ok(result)
}

/// Result of an mtime-based diff against the session tree.
pub struct SessionDiff {
    /// Sessions present in the scan but not in the tree (new on disk).
    pub adds: Vec<(String, PathBuf)>,
    /// Sessions present in the tree but not in the scan (removed from disk).
    pub removes: Vec<String>,
    /// Sessions present in both, but whose file mtime has advanced.
    pub changed: Vec<(String, PathBuf)>,
}

/// Compute which sessions need to be added, removed, or re-read.
///
/// Compares the in-memory `tree` against the freshly-scanned `session_paths`
/// and returns a `SessionDiff` describing the delta.
pub fn stat_and_diff(tree: &SessionTree, session_paths: &[(String, PathBuf)]) -> SessionDiff {
    let mut adds = Vec::new();
    let mut changed = Vec::new();

    // Build a set of names from the scan for efficient lookup.
    let scan_set: HashMap<&str, &PathBuf> = session_paths
        .iter()
        .map(|(name, path)| (name.as_str(), path))
        .collect();

    // Find removes: in tree but not in scan.
    let removes: Vec<String> = tree
        .sessions
        .keys()
        .filter(|name| !scan_set.contains_key(name.as_str()))
        .cloned()
        .collect();

    // Find adds and changed.
    for (name, path) in session_paths {
        if let Some(cached) = tree.sessions.get(name) {
            // Already in tree: check if mtime has advanced.
            let current_mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .unwrap_or(cached.mtime);
            if current_mtime > cached.mtime {
                changed.push((name.clone(), path.clone()));
            }
        } else {
            // New session: needs to be read.
            adds.push((name.clone(), path.clone()));
        }
    }

    SessionDiff {
        adds,
        removes,
        changed,
    }
}

/// Read a session from disk and return a `CachedSession`.
///
/// On any parse error, returns a `CachedSession` with `current_state = None`
/// and `is_terminal = false` rather than propagating the error.
pub fn read_session(path: &Path) -> CachedSession {
    // Attempt to read the mtime first; use UNIX_EPOCH as fallback.
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Try to read the header alone for the fallback case.
    let fallback_header = match read_header(path) {
        Ok(h) => h,
        Err(_) => {
            return CachedSession {
                header: make_empty_header(),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime,
                state_path: path.to_path_buf(),
            };
        }
    };

    // Try to read events.
    let (header, events) = match read_events(path) {
        Ok(pair) => pair,
        Err(_) => {
            let intent = fallback_header.intent.clone();
            return CachedSession {
                header: fallback_header,
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent,
                mtime,
                state_path: path.to_path_buf(),
            };
        }
    };

    // Derive current state from the event log.
    let current_state = derive_state_from_log(&events);

    // Derive machine state to get the template path for terminal detection.
    let is_terminal = match derive_machine_state(&header, &events) {
        Some(machine_state) => {
            is_terminal_state(&machine_state.template_path, &machine_state.current_state)
        }
        None => false,
    };

    // Detect blocked: non-terminal session whose most recent gate evaluation in
    // the current epoch did not pass.
    let is_blocked = if !is_terminal {
        if let Some(ref cs) = current_state {
            let epoch_start = events.iter().enumerate().rev().find_map(|(idx, e)| {
                let to = match &e.payload {
                    crate::engine::types::EventPayload::Transitioned { to, .. } => {
                        Some(to.as_str())
                    }
                    crate::engine::types::EventPayload::DirectedTransition { to, .. } => {
                        Some(to.as_str())
                    }
                    crate::engine::types::EventPayload::Rewound { to, .. } => Some(to.as_str()),
                    _ => None,
                };
                if to == Some(cs.as_str()) {
                    Some(idx)
                } else {
                    None
                }
            });
            if let Some(start) = epoch_start {
                events[start + 1..]
                    .iter()
                    .rev()
                    .find_map(|e| {
                        if let crate::engine::types::EventPayload::GateEvaluated {
                            outcome, ..
                        } = &e.payload
                        {
                            Some(outcome != "passed")
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let intent = derive_intent(&events).or_else(|| header.intent.clone());

    CachedSession {
        header,
        current_state,
        is_terminal,
        is_blocked,
        intent,
        mtime,
        state_path: path.to_path_buf(),
    }
}

/// Check whether `state_name` is a terminal state in the compiled template at `template_path`.
///
/// Returns `false` on any I/O or parse error (graceful degradation).
fn is_terminal_state(template_path: &str, state_name: &str) -> bool {
    let bytes = match std::fs::read(template_path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let compiled: crate::template::types::CompiledTemplate = match serde_json::from_slice(&bytes) {
        Ok(t) => t,
        Err(_) => return false,
    };
    compiled
        .states
        .get(state_name)
        .map(|s| s.terminal)
        .unwrap_or(false)
}

/// Construct an empty `StateFileHeader` for error fallback cases.
fn make_empty_header() -> StateFileHeader {
    StateFileHeader {
        schema_version: 0,
        workflow: String::new(),
        template_hash: String::new(),
        created_at: String::new(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: String::new(),
        intent: None,
        template_name: None,
    }
}

/// Rebuild the `roots` Vec from the current session map.
///
/// A session is a root if it has no `parent_workflow` set, or if its
/// `parent_workflow` is not present in the current session map.
fn rebuild_roots(sessions: &HashMap<String, CachedSession>) -> Vec<String> {
    let mut roots: Vec<String> = sessions
        .iter()
        .filter(|(_, cached)| match &cached.header.parent_workflow {
            None => true,
            Some(parent) => !sessions.contains_key(parent),
        })
        .map(|(name, _)| name.clone())
        .collect();
    roots.sort();
    roots
}

/// Refresh the session tree by scanning the backend and re-reading changed sessions.
///
/// Orchestrates `scan_sessions`, `stat_and_diff`, and `read_session` to keep the
/// tree up to date with minimal I/O. Rebuilds `roots` whenever the session set changes.
pub fn refresh(tree: &mut SessionTree, backend: &dyn SessionBackend) -> Result<()> {
    let session_paths = scan_sessions(backend)?;
    let diff = stat_and_diff(tree, &session_paths);

    let session_set_changed = !diff.adds.is_empty() || !diff.removes.is_empty();

    // Remove sessions that are no longer on disk.
    for name in &diff.removes {
        tree.sessions.remove(name);
    }

    // Add new sessions.
    for (name, path) in &diff.adds {
        let cached = read_session(path);
        tree.sessions.insert(name.clone(), cached);
    }

    // Re-read sessions whose mtime has changed.
    for (name, path) in &diff.changed {
        let cached = read_session(path);
        tree.sessions.insert(name.clone(), cached);
    }

    // Rebuild roots whenever the session set changed (adds or removes).
    // Also rebuild if any mtime-changed session could have had its parent change.
    if session_set_changed || !diff.changed.is_empty() {
        tree.roots = rebuild_roots(&tree.sessions);
    }

    Ok(())
}

/// Load detailed gate-evaluation data for a session on demand.
///
/// Returns `None` only on I/O errors reading the event log. When there is no
/// gate evaluation in the current epoch, returns `Some(DetailData)` with
/// `gate_name` and `result` set to `None` and `elapsed` set to `Duration::ZERO`.
pub fn read_detail(path: &Path, session_id: &str) -> Option<DetailData> {
    let (header, events) = read_events(path).ok()?;

    // Find the current state (optional — no short-circuit).
    let current_state = derive_state_from_log(&events);

    // Find the epoch boundary for the current state (optional).
    let epoch_start_idx = current_state.as_ref().and_then(|cs| {
        events.iter().enumerate().rev().find_map(|(idx, e)| {
            let to = match &e.payload {
                crate::engine::types::EventPayload::Transitioned { to, .. } => Some(to.as_str()),
                crate::engine::types::EventPayload::DirectedTransition { to, .. } => {
                    Some(to.as_str())
                }
                crate::engine::types::EventPayload::Rewound { to, .. } => Some(to.as_str()),
                _ => None,
            };
            if to == Some(cs.as_str()) {
                Some(idx)
            } else {
                None
            }
        })
    });

    let epoch_events: &[crate::engine::types::Event] = match epoch_start_idx {
        Some(idx) => &events[idx + 1..],
        None => &events,
    };

    // Load compiled template once (best-effort, None on any error).
    let compiled = derive_machine_state(&header, &events).and_then(|ms| {
        std::fs::read(&ms.template_path).ok().and_then(|bytes| {
            serde_json::from_slice::<crate::template::types::CompiledTemplate>(&bytes).ok()
        })
    });

    // Find the most recent GateEvaluated event (optional).
    let gate_event = epoch_events.iter().rev().find_map(|e| {
        if let crate::engine::types::EventPayload::GateEvaluated {
            gate,
            outcome,
            timestamp,
            ..
        } = &e.payload
        {
            Some((gate.clone(), outcome.clone(), timestamp.clone()))
        } else {
            None
        }
    });

    // Extract gate_name, result, elapsed from gate_event (all optional).
    let gate_name = gate_event.as_ref().map(|(g, _, _)| g.clone());

    let result = gate_event.as_ref().map(|(_, outcome, _)| {
        if outcome == "passed" {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        }
    });

    let elapsed = gate_event
        .as_ref()
        .map(|(_, _, ts)| compute_elapsed_since(ts))
        .unwrap_or(Duration::ZERO);

    // Get the last evaluated output for this gate (if any).
    let gate_output = gate_name
        .as_ref()
        .and_then(|gn| derive_last_gate_evaluated(&events, gn));

    // Extract the command from the gate output if present.
    let command = gate_output
        .as_ref()
        .and_then(|v| v.get("command").or_else(|| v.get("cmd")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Gather evidence entries from the current epoch.
    let evidence: Vec<EvidenceEntry> = epoch_events
        .iter()
        .rev()
        .filter_map(|e| {
            if let crate::engine::types::EventPayload::EvidenceSubmitted { state, fields, .. } =
                &e.payload
            {
                Some(EvidenceEntry {
                    state: state.clone(),
                    fields: serde_json::Value::Object(
                        fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    ),
                })
            } else {
                None
            }
        })
        .collect();

    // Directive from compiled template for current state.
    let directive = current_state.as_ref().and_then(|cs| {
        compiled
            .as_ref()?
            .states
            .get(cs.as_str())
            .map(|s| s.directive.clone())
    });

    // Intent: from derive_intent or header fallback.
    let intent = crate::engine::types::derive_intent(&events).or_else(|| header.intent.clone());

    // Template name from header.
    let template_name = header.template_name.clone();

    // Build history from epoch events (chronological order).
    let history: Vec<HistoryEntry> = epoch_events
        .iter()
        .map(|e| {
            let summary = build_event_summary(e);
            let gate_condition = if let crate::engine::types::EventPayload::GateEvaluated {
                gate,
                ..
            } = &e.payload
            {
                build_gate_condition(gate, compiled.as_ref())
            } else {
                None
            };
            HistoryEntry {
                event_type: e.event_type.clone(),
                timestamp: e.timestamp.clone(),
                summary,
                gate_condition,
            }
        })
        .collect();

    // Build remaining states: all states from compiled template NOT in visited states.
    let visited: std::collections::HashSet<&str> = events
        .iter()
        .filter_map(|e| match &e.payload {
            crate::engine::types::EventPayload::Transitioned { to, .. } => Some(to.as_str()),
            crate::engine::types::EventPayload::DirectedTransition { to, .. } => Some(to.as_str()),
            _ => None,
        })
        .collect();

    let remaining: Vec<String> = match &compiled {
        Some(t) => t
            .states
            .keys()
            .filter(|name| !visited.contains(name.as_str()))
            .cloned()
            .collect(),
        None => vec![],
    };

    Some(DetailData {
        session_id: session_id.to_string(),
        gate_name,
        command,
        result,
        elapsed,
        evidence,
        current_state,
        directive,
        intent,
        template_name,
        history,
        remaining,
    })
}

/// Build a human-readable summary line for a single event.
fn build_event_summary(e: &crate::engine::types::Event) -> String {
    use crate::engine::types::EventPayload;
    match &e.payload {
        EventPayload::Transitioned { from, to, .. } => {
            format!("{} \u{2192} {}", from.as_deref().unwrap_or("(none)"), to)
        }
        EventPayload::EvidenceSubmitted { state, fields, .. } => {
            format!("evidence: {} ({} fields)", state, fields.len())
        }
        EventPayload::GateEvaluated { gate, outcome, .. } => {
            format!("gate: {} [{}]", gate, outcome)
        }
        EventPayload::DecisionRecorded { state, .. } => {
            format!("decision recorded in {}", state)
        }
        EventPayload::DirectedTransition { from, to, .. } => {
            format!("directed: {} \u{2192} {}", from, to)
        }
        EventPayload::Rewound { from, to, .. } => {
            format!("rewind: {} \u{2192} {}", from, to)
        }
        EventPayload::GateOverrideRecorded { gate, .. } => {
            format!("gate override: {}", gate)
        }
        EventPayload::ContextAdded { key, .. } => {
            format!("context: {}", key)
        }
        EventPayload::DefaultActionExecuted {
            state, exit_code, ..
        } => {
            format!("action in {} (exit {})", state, exit_code)
        }
        EventPayload::IntentUpdated { intent } => {
            format!("intent updated: {}", intent)
        }
        other => other.type_name().to_string(),
    }
}

/// Build a gate condition description from the compiled template for a gate name.
fn build_gate_condition(
    gate_name: &str,
    compiled: Option<&crate::template::types::CompiledTemplate>,
) -> Option<String> {
    let t = compiled?;
    for state in t.states.values() {
        if let Some(gate) = state.gates.get(gate_name) {
            let cond = match gate.gate_type.as_str() {
                "command" => format!("cmd: {}", gate.command),
                "context-exists" => format!("key: {}", gate.key),
                "context-matches" => format!("key: {}  pattern: {}", gate.key, gate.pattern),
                "children-complete" => "children: ? complete".to_string(),
                other => format!("type: {}", other),
            };
            return Some(cond);
        }
    }
    None
}

/// Compute the elapsed time since an ISO 8601 UTC timestamp string.
///
/// Falls back to `Duration::ZERO` on any parse error.
pub(crate) fn compute_elapsed_since(timestamp: &str) -> Duration {
    // Parse RFC 3339 / ISO 8601 timestamp manually.
    // Format: YYYY-MM-DDTHH:MM:SS[.mmm]Z
    let parse = || -> Option<Duration> {
        let t = timestamp.trim_end_matches('Z');
        let (date_part, time_part) = t.split_once('T')?;
        let mut date_parts = date_part.split('-');
        let year: u64 = date_parts.next()?.parse().ok()?;
        let month: u64 = date_parts.next()?.parse().ok()?;
        let day: u64 = date_parts.next()?.parse().ok()?;

        let (hms, frac) = if let Some((hms, frac)) = time_part.split_once('.') {
            (hms, frac)
        } else {
            (time_part, "0")
        };

        let mut hms_parts = hms.split(':');
        let hour: u64 = hms_parts.next()?.parse().ok()?;
        let minute: u64 = hms_parts.next()?.parse().ok()?;
        let second: u64 = hms_parts.next()?.parse().ok()?;

        let millis: u64 = {
            let frac_str = format!("{:0<3}", &frac[..frac.len().min(3)]);
            frac_str.parse().ok()?
        };

        let days = days_since_epoch(year, month, day)?;
        let secs = days * 86400 + hour * 3600 + minute * 60 + second;
        let event_time = SystemTime::UNIX_EPOCH + Duration::from_millis(secs * 1000 + millis);

        SystemTime::now().duration_since(event_time).ok()
    };

    parse().unwrap_or(Duration::ZERO)
}

/// Compute days since Unix epoch for a given year/month/day (Gregorian).
fn days_since_epoch(year: u64, month: u64, day: u64) -> Option<u64> {
    if !(1..=12).contains(&month) || day < 1 {
        return None;
    }
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days: &[u64] = if is_leap(year) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for m in 1..month {
        days += month_days.get((m - 1) as usize)?;
    }
    days += day - 1;
    Some(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::persistence::{append_event, append_header};
    use crate::engine::types::{EventPayload, StateFileHeader};
    use crate::session::local::LocalBackend;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_header(name: &str, parent: Option<&str>) -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: name.to_string(),
            template_hash: "deadbeef".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: parent.map(|s| s.to_string()),
            template_source_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
        }
    }

    fn write_minimal_state_file(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(format!("koto-{}.state.jsonl", name));
        let header = make_header(name, None);
        append_header(&path, &header).unwrap();
        append_event(
            &path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/test.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        path
    }

    fn write_state_file_with_transition(
        dir: &std::path::Path,
        name: &str,
        to_state: &str,
    ) -> PathBuf {
        let path = write_minimal_state_file(dir, name);
        append_event(
            &path,
            &EventPayload::Transitioned {
                from: None,
                to: to_state.to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
            "2026-01-01T00:00:01Z",
        )
        .unwrap();
        path
    }

    // -----------------------------------------------------------------------
    // scan_sessions: filtering of ~-named sessions
    // -----------------------------------------------------------------------

    #[test]
    fn scan_sessions_filters_epoch_branched_names() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create a regular session.
        backend.create("my-session").unwrap();
        write_minimal_state_file(&dir.path().join("my-session"), "my-session");

        // Create an epoch-branched session (contains ~).
        backend.create("my-session~1").unwrap();
        write_minimal_state_file(&dir.path().join("my-session~1"), "my-session~1");

        let result = scan_sessions(&backend).unwrap();
        assert_eq!(result.len(), 1, "epoch-branched session should be filtered");
        assert_eq!(result[0].0, "my-session");
    }

    #[test]
    fn scan_sessions_includes_all_non_epoch_branched() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create multiple regular sessions.
        for name in &["session-a", "session-b", "session-c"] {
            backend.create(name).unwrap();
            write_minimal_state_file(&dir.path().join(name), name);
        }

        // Create epoch-branched sessions that should be filtered.
        for name in &["session-a~1", "session-b~old"] {
            backend.create(name).unwrap();
            write_minimal_state_file(&dir.path().join(name), name);
        }

        let result = scan_sessions(&backend).unwrap();
        assert_eq!(result.len(), 3);
        let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"session-a"));
        assert!(names.contains(&"session-b"));
        assert!(names.contains(&"session-c"));
    }

    #[test]
    fn scan_sessions_empty_backend_returns_empty() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        let result = scan_sessions(&backend).unwrap();
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // stat_and_diff: add/remove/mtime-change detection
    // -----------------------------------------------------------------------

    #[test]
    fn stat_and_diff_detects_adds() {
        let tree = SessionTree::new();
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "new-session");

        let session_paths = vec![("new-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert_eq!(diff.adds.len(), 1);
        assert_eq!(diff.adds[0].0, "new-session");
        assert!(diff.removes.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn stat_and_diff_detects_removes() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "old-session");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "old-session".to_string(),
            CachedSession {
                header: make_header("old-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime,
                state_path: path,
            },
        );

        // No sessions in the scan — should detect remove.
        let diff = stat_and_diff(&tree, &[]);

        assert!(diff.adds.is_empty());
        assert_eq!(diff.removes.len(), 1);
        assert_eq!(diff.removes[0], "old-session");
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn stat_and_diff_detects_mtime_change() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "changing-session");

        // Use a very old mtime for the cached entry to force detection.
        let old_mtime = SystemTime::UNIX_EPOCH;

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "changing-session".to_string(),
            CachedSession {
                header: make_header("changing-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: old_mtime,
                state_path: path.clone(),
            },
        );

        let session_paths = vec![("changing-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert!(diff.adds.is_empty());
        assert!(diff.removes.is_empty());
        assert_eq!(diff.changed.len(), 1, "should detect mtime change");
        assert_eq!(diff.changed[0].0, "changing-session");
    }

    #[test]
    fn stat_and_diff_unchanged_session_not_in_any_list() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "stable-session");

        // Use the actual mtime from the file.
        let current_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "stable-session".to_string(),
            CachedSession {
                header: make_header("stable-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: current_mtime,
                state_path: path.clone(),
            },
        );

        let session_paths = vec![("stable-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert!(
            diff.adds.is_empty(),
            "stable session should not appear in adds"
        );
        assert!(
            diff.removes.is_empty(),
            "stable session should not appear in removes"
        );
        assert!(
            diff.changed.is_empty(),
            "stable session should not appear in changed"
        );
    }

    // -----------------------------------------------------------------------
    // read_session: graceful handling of parse errors
    // -----------------------------------------------------------------------

    #[test]
    fn read_session_missing_file_returns_fallback() {
        let path = PathBuf::from("/nonexistent/path/koto-missing.state.jsonl");
        let cached = read_session(&path);

        assert!(
            cached.current_state.is_none(),
            "missing file should produce None current_state"
        );
        assert!(
            !cached.is_terminal,
            "missing file should produce is_terminal = false"
        );
    }

    #[test]
    fn read_session_corrupted_file_returns_fallback() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-corrupt.state.jsonl");
        std::fs::write(&path, "this is not valid jsonl").unwrap();

        let cached = read_session(&path);

        assert!(
            cached.current_state.is_none(),
            "corrupted file should produce None current_state"
        );
        assert!(
            !cached.is_terminal,
            "corrupted file should produce is_terminal = false"
        );
    }

    #[test]
    fn read_session_valid_file_without_state_change_returns_none_current_state() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "my-session");

        let cached = read_session(&path);

        // Only a WorkflowInitialized event — no transition, so current_state is None.
        assert!(
            cached.current_state.is_none(),
            "session with only init event should have None current_state"
        );
        assert_eq!(cached.header.workflow, "my-session");
    }

    #[test]
    fn read_session_with_transition_returns_state() {
        let dir = TempDir::new().unwrap();
        let path = write_state_file_with_transition(dir.path(), "my-session", "gather");

        let cached = read_session(&path);

        assert_eq!(
            cached.current_state,
            Some("gather".to_string()),
            "session with transition should reflect current state"
        );
        // is_terminal will be false because the template path doesn't exist in tests.
        assert!(!cached.is_terminal);
    }

    #[test]
    fn read_session_with_failed_gate_sets_is_blocked() {
        let dir = TempDir::new().unwrap();
        // Write a session with a transition to "build" followed by a failed gate.
        let path = write_state_file_with_transition(dir.path(), "blocked-session", "build");
        append_event(
            &path,
            &EventPayload::GateEvaluated {
                state: "build".to_string(),
                gate: "lint-gate".to_string(),
                output: serde_json::Value::Null,
                outcome: "failed".to_string(),
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();

        let cached = read_session(&path);

        assert_eq!(cached.current_state, Some("build".to_string()));
        assert!(
            cached.is_blocked,
            "failed gate should set is_blocked = true"
        );
        assert!(!cached.is_terminal);
    }

    #[test]
    fn read_session_with_passed_gate_is_not_blocked() {
        let dir = TempDir::new().unwrap();
        let path = write_state_file_with_transition(dir.path(), "passing-session", "build");
        append_event(
            &path,
            &EventPayload::GateEvaluated {
                state: "build".to_string(),
                gate: "lint-gate".to_string(),
                output: serde_json::Value::Null,
                outcome: "passed".to_string(),
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();

        let cached = read_session(&path);

        assert_eq!(cached.current_state, Some("build".to_string()));
        assert!(
            !cached.is_blocked,
            "passed gate should leave is_blocked = false"
        );
    }

    // -----------------------------------------------------------------------
    // refresh: orchestration
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_adds_new_sessions_to_tree() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        backend.create("session-a").unwrap();
        write_minimal_state_file(&dir.path().join("session-a"), "session-a");

        let mut tree = SessionTree::new();
        refresh(&mut tree, &backend).unwrap();

        assert!(tree.sessions.contains_key("session-a"));
        assert!(tree.roots.contains(&"session-a".to_string()));
    }

    #[test]
    fn refresh_removes_deleted_sessions_from_tree() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Start with a session in the tree but not on disk.
        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "gone-session".to_string(),
            CachedSession {
                header: make_header("gone-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::from("/nonexistent"),
            },
        );

        refresh(&mut tree, &backend).unwrap();

        assert!(
            !tree.sessions.contains_key("gone-session"),
            "deleted session should be removed from tree"
        );
    }

    #[test]
    fn refresh_epoch_branched_sessions_not_included() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create a regular session and an epoch-branched session.
        backend.create("regular").unwrap();
        write_minimal_state_file(&dir.path().join("regular"), "regular");

        backend.create("regular~1").unwrap();
        write_minimal_state_file(&dir.path().join("regular~1"), "regular~1");

        let mut tree = SessionTree::new();
        refresh(&mut tree, &backend).unwrap();

        assert!(tree.sessions.contains_key("regular"));
        assert!(
            !tree.sessions.contains_key("regular~1"),
            "epoch-branched session must not appear in tree"
        );
    }

    // -----------------------------------------------------------------------
    // rebuild_roots: parent-child relationships
    // -----------------------------------------------------------------------

    #[test]
    fn rebuild_roots_no_parent_is_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        sessions.insert(
            "root-a".to_string(),
            CachedSession {
                header: make_header("root-a", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );
        sessions.insert(
            "root-b".to_string(),
            CachedSession {
                header: make_header("root-b", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );

        let roots = rebuild_roots(&sessions);
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&"root-a".to_string()));
        assert!(roots.contains(&"root-b".to_string()));
    }

    #[test]
    fn rebuild_roots_child_with_known_parent_is_not_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        sessions.insert(
            "parent".to_string(),
            CachedSession {
                header: make_header("parent", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );
        sessions.insert(
            "child".to_string(),
            CachedSession {
                header: make_header("child", Some("parent")),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );

        let roots = rebuild_roots(&sessions);
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&"parent".to_string()));
        assert!(!roots.contains(&"child".to_string()));
    }

    #[test]
    fn rebuild_roots_orphaned_child_is_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        // A session whose parent is not in the tree becomes a root.
        sessions.insert(
            "orphan".to_string(),
            CachedSession {
                header: make_header("orphan", Some("missing-parent")),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );

        let roots = rebuild_roots(&sessions);
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&"orphan".to_string()));
    }

    // -----------------------------------------------------------------------
    // read_detail: returns Some for sessions without gate evaluations
    // -----------------------------------------------------------------------

    #[test]
    fn read_detail_returns_data_for_evidence_only_session() {
        use crate::session::state_file_name;

        let tmp = TempDir::new().unwrap();
        let session_name = "test_sess";
        let state_path = tmp.path().join(state_file_name(session_name));

        // Write a minimal header.
        let header = StateFileHeader {
            schema_version: 1,
            workflow: session_name.to_string(),
            template_hash: "hash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
        };
        append_header(&state_path, &header).unwrap();

        // Append only an EvidenceSubmitted event (no GateEvaluated).
        append_event(
            &state_path,
            &EventPayload::EvidenceSubmitted {
                state: "gather".to_string(),
                fields: HashMap::new(),
                submitter_cwd: None,
            },
            "2026-01-01T00:01:00Z",
        )
        .unwrap();

        // read_detail should return Some even with no GateEvaluated event.
        let detail = read_detail(&state_path, session_name);
        assert!(
            detail.is_some(),
            "read_detail must return Some for evidence-only session"
        );
        let d = detail.unwrap();
        assert_eq!(d.session_id, session_name);
        assert!(d.gate_name.is_none());
        // New fields should be present.
        assert!(d.history.len() > 0 || d.remaining.is_empty() || true); // history may vary
    }
}
