use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::engine::errors::EngineError;
use crate::engine::types::{Event, EventPayload, MachineState, StateFileHeader};

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
/// Performs three-tier format detection:
/// 1. First line has `current_state` or `CurrentState` -> old Go format
/// 2. First line has `type` but no `schema_version` -> #45 simple JSONL
/// 3. First line has `schema_version: 1` -> new format
/// 4. First line is not parseable JSON -> corrupted
pub fn read_header(path: &Path) -> anyhow::Result<StateFileHeader> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let reader = BufReader::new(file);
    let first_line = reader
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("state file is empty"))?
        .map_err(|e| anyhow::anyhow!("failed to read first line: {}", e))?;

    let trimmed = first_line.trim();
    if trimmed.is_empty() {
        return Err(
            EngineError::StateFileCorrupted("state file has empty first line".to_string()).into(),
        );
    }

    detect_and_parse_header(trimmed)
}

/// Three-tier format detection on the first line of a state file.
fn detect_and_parse_header(first_line: &str) -> anyhow::Result<StateFileHeader> {
    let val: serde_json::Value = serde_json::from_str(first_line).map_err(|_| {
        EngineError::StateFileCorrupted(
            "first line is not valid JSON; state file may be corrupted".to_string(),
        )
    })?;

    let obj = val.as_object().ok_or_else(|| {
        EngineError::StateFileCorrupted("first line is not a JSON object".to_string())
    })?;

    // Tier 1: Old Go format (has current_state or CurrentState)
    if obj.contains_key("current_state") || obj.contains_key("CurrentState") {
        return Err(EngineError::IncompatibleFormat(
            "state file uses old Go format; delete and re-initialize with 'koto init'".to_string(),
        )
        .into());
    }

    // Tier 2: #45 simple JSONL format (has "type" but no "schema_version")
    if obj.contains_key("type") && !obj.contains_key("schema_version") {
        return Err(EngineError::IncompatibleFormat(
            "state file uses an older format; delete and re-initialize with 'koto init'"
                .to_string(),
        )
        .into());
    }

    // Tier 3: New format (has schema_version)
    if obj.contains_key("schema_version") {
        let header: StateFileHeader = serde_json::from_str(first_line).map_err(|e| {
            EngineError::StateFileCorrupted(format!("failed to parse header: {}", e))
        })?;
        return Ok(header);
    }

    Err(EngineError::StateFileCorrupted(
        "first line is not a recognized state file format".to_string(),
    )
    .into())
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
    let header = detect_and_parse_header(lines[0].trim())?;

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
        }
    }

    fn make_event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
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
            },
        );
        // Seq jumps from 1 to 3 (gap at seq 2)
        let e3 = make_event(
            3,
            EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
            },
        );
        let e4 = make_event(
            4,
            EventPayload::Transitioned {
                from: Some("gather".to_string()),
                to: "plan".to_string(),
                condition_type: "gate".to_string(),
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
    // Format detection
    // -----------------------------------------------------------------------

    #[test]
    fn go_format_first_line_produces_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-go.state.jsonl");
        std::fs::write(
            &path,
            r#"{"current_state":"gather","template":"/path","template_hash":"abc"}"#,
        )
        .unwrap();

        let result = read_events(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("old Go format"),
            "should detect Go format, got: {}",
            err
        );
    }

    #[test]
    fn issue45_format_first_line_produces_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-45.state.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"init","state":"gather","timestamp":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let result = read_events(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("older format"),
            "should detect #45 format, got: {}",
            err
        );
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                },
            ),
            make_event(
                4,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
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
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                },
            ),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
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
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                },
            ),
            make_event(
                5,
                EventPayload::Rewound {
                    from: "analyze".to_string(),
                    to: "gather".to_string(),
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
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
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
            },
        )];
        // Only init event, no transitioned -- no current state derivable
        assert!(derive_machine_state(&header, &events).is_none());
    }
}
