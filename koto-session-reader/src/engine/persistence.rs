//! State-file persistence, read half: the log header and event parsers,
//! every `derive_*` fold over a log, the visit, epoch and delivery-window
//! helpers, the supersede fence readers and the merged-last-line detector.
//! The write half (appends, locks, whole-log replacement) is koto's
//! `engine::persistence`, which re-exports this module.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::engine::errors::EngineError;
use crate::engine::types::{
    Event, EventPayload, MachineState, StateFileHeader, SupersedeActor, CURRENT_SCHEMA_VERSION,
};

/// What a log's first line must be able to tell the reader.
///
/// koto now has two log families — the session state file and the
/// request record under `~/.koto/requests/` — that share the JSONL
/// envelope, the sequence discipline, and the truncated-final-line
/// recovery but NOT the header type. Generifying the reader over this
/// trait rather than copying it keeps the sequence-gap validation and
/// the tail recovery in exactly one place
/// (DESIGN-request-lifecycle.md Decision 1).
///
/// The version gate is part of the trait rather than left to the
/// caller because it has to run *before* the events are parsed: a log
/// written by a newer koto must fail at line one with a version error
/// rather than at line forty with a mystery.
pub trait LogHeader: DeserializeOwned {
    /// The `schema_version` recorded on this header line.
    fn schema_version(&self) -> u32;
    /// The highest `schema_version` this build can read for this log
    /// family. Each family versions independently.
    fn max_supported_schema_version() -> u32;
}

impl LogHeader for StateFileHeader {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn max_supported_schema_version() -> u32 {
        CURRENT_SCHEMA_VERSION
    }
}

// ===== The supersede fence =====

/// A session's supersede fence: the [`EventPayload::SessionSuperseded`]
/// event in its own log, which names the session that superseded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeFence {
    /// The successor's name.
    pub by: String,
    /// The successor's session id, as its header records it.
    pub by_session_id: String,
    /// Who ran the command that fenced the session.
    pub actor: SupersedeActor,
    /// When the fence was appended.
    pub at: String,
}

impl SupersedeFence {
    /// The fence `payload` carries, if it is one.
    pub fn from_payload(payload: &EventPayload) -> Option<Self> {
        match payload {
            EventPayload::SessionSuperseded {
                by,
                by_session_id,
                actor,
                at,
            } => Some(SupersedeFence {
                by: by.clone(),
                by_session_id: by_session_id.clone(),
                actor: actor.clone(),
                at: at.clone(),
            }),
            _ => None,
        }
    }

    /// The event payload that records this fence.
    pub fn to_payload(&self) -> EventPayload {
        EventPayload::SessionSuperseded {
            by: self.by.clone(),
            by_session_id: self.by_session_id.clone(),
            actor: self.actor.clone(),
            at: self.at.clone(),
        }
    }
}

/// The fence in `events`, if the session was superseded. The first one
/// wins: it is the one every later append was refused by.
pub fn superseded_by(events: &[Event]) -> Option<SupersedeFence> {
    events
        .iter()
        .find_map(|e| SupersedeFence::from_payload(&e.payload))
}

/// [`superseded_by`] over a log's raw bytes, parsing only the lines that
/// mention the event type, so the check every append makes costs a
/// substring search on a log with no fence.
pub fn superseded_by_in_log(content: &str) -> Option<SupersedeFence> {
    const MARK: &str = "session_superseded";
    if !content.contains(MARK) {
        return None;
    }
    content
        .lines()
        .skip(1)
        .filter(|line| line.contains(MARK))
        .find_map(|line| {
            serde_json::from_str::<Event>(line.trim())
                .ok()
                .and_then(|e| SupersedeFence::from_payload(&e.payload))
        })
}

/// The fence in the log at `path`, read without a lock. A log that doesn't
/// exist has none.
pub fn read_fence(path: &Path) -> anyhow::Result<Option<SupersedeFence>> {
    let read = crate::fs_read::open_plain(path).and_then(|mut file| {
        let mut content = String::new();
        file.read_to_string(&mut content).map(|_| content)
    });
    match read {
        Ok(content) => Ok(superseded_by_in_log(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!(
            "failed to read state file {}: {}",
            path.display(),
            e
        )),
    }
}

/// The last non-empty line after a log's header, with where it starts.
struct LastEventLine<'a> {
    /// The line, without its newline.
    text: &'a str,
    /// Byte offset of the line's start in the log.
    offset: usize,
    /// 1-indexed line number; the header is line 1.
    line_number: usize,
    /// The non-empty line before it, when that is an event line too.
    previous: Option<&'a str>,
}

/// Find the last non-empty event line of `content`, or `None` when the log
/// holds only its header.
fn last_event_line(content: &str) -> Option<LastEventLine<'_>> {
    let mut last: Option<LastEventLine<'_>> = None;
    let mut offset = 0;
    for (i, segment) in content.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += segment.len();
        let text = segment.strip_suffix('\n').unwrap_or(segment);
        if i == 0 || text.trim().is_empty() {
            continue;
        }
        last = Some(LastEventLine {
            text,
            offset: start,
            line_number: i + 1,
            previous: last.map(|l| l.text),
        });
    }
    last
}

/// The events `line` holds when it is two or more well-formed events written
/// one after another with no newline between them, each with its own text;
/// `None` for anything else.
///
/// A line cut short partway through an event -- the torn tail of a crash --
/// is `None`: its last piece doesn't parse. So is a line holding one event,
/// which isn't merged.
pub(crate) fn split_concatenated(line: &str) -> Option<Vec<(&str, Event)>> {
    let mut stream = serde_json::Deserializer::from_str(line).into_iter::<Event>();
    let mut pieces = Vec::new();
    let mut start = 0;
    while let Some(next) = stream.next() {
        let event = next.ok()?;
        let end = stream.byte_offset();
        pieces.push((line[start..end].trim(), event));
        start = end;
    }
    (pieces.len() >= 2).then_some(pieces)
}

/// What a log's last line is, to an append that couldn't parse it.
enum SplitOutcome<'a> {
    /// Not well-formed events merged together: a truncated line, or
    /// anything else that isn't an event.
    NotMerged,
    /// Merged events whose seqs continue the log: split them.
    Split(Vec<(&'a str, Event)>),
    /// Merged events whose seqs don't continue the line before: leave them.
    DoesNotFollow(Vec<u64>),
}

fn split_last_line<'a>(last: &LastEventLine<'a>) -> SplitOutcome<'a> {
    let Some(pieces) = split_concatenated(last.text.trim()) else {
        return SplitOutcome::NotMerged;
    };
    let seqs: Vec<u64> = pieces.iter().map(|(_, e)| e.seq).collect();
    let previous_seq = match last.previous {
        None => Some(0),
        Some(prev) => serde_json::from_str::<serde_json::Value>(prev.trim())
            .ok()
            .and_then(|v| v.get("seq").and_then(|s| s.as_u64())),
    };
    if previous_seq.is_some_and(|prev| consecutive_from(&seqs, prev + 1)) {
        SplitOutcome::Split(pieces)
    } else {
        SplitOutcome::DoesNotFollow(seqs)
    }
}

/// Whether the last line of `content` (a whole log) holds merged events an
/// append would split, so a repair that drops a torn tail must keep it.
#[doc(hidden)]
pub fn last_line_would_split(content: &str) -> bool {
    last_event_line(content)
        .is_some_and(|last| matches!(split_last_line(&last), SplitOutcome::Split(_)))
}

/// Whether `seqs` runs `first, first + 1, ...` with no gap.
fn consecutive_from(seqs: &[u64], first: u64) -> bool {
    seqs.iter()
        .enumerate()
        .all(|(i, seq)| first.checked_add(i as u64) == Some(*seq))
}

#[doc(hidden)]
pub fn join_seqs(seqs: &[u64]) -> String {
    seqs.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an append to a log will do, worked out from the log's bytes alone.
///
/// [`plan_append`] returns it and `prepare_append` acts on it: a plan with
/// no [`merged`](Self::merged) line appends at [`next_seq`](Self::next_seq)
/// as the bytes stand, and one with a merged line splits that line first.
#[doc(hidden)]
#[derive(Debug)]
pub struct AppendPlan<'a> {
    /// The seq the appended event takes.
    pub next_seq: u64,
    /// The log's last line has no newline, so the append supplies it first.
    pub unterminated: bool,
    /// The log's last line holds events written with no newline between
    /// them, whose seqs follow the line before: the append splits it first.
    pub merged: Option<MergedLastLine<'a>>,
}

/// A log's last line holding two or more well-formed events written with no
/// newline between them (koto#419), as [`plan_append`] found it.
#[doc(hidden)]
#[derive(Debug)]
pub struct MergedLastLine<'a> {
    /// Byte offset of the line's start in the log.
    pub offset: usize,
    /// 1-indexed line number; the header is line 1.
    pub line_number: usize,
    /// Each event on the line, with the exact text it was written with.
    pub pieces: Vec<(&'a str, Event)>,
}

/// Work out what an append to the log whose bytes are `content` will do:
/// the seq the next event takes, whether the last line lacks its newline,
/// and whether that line holds merged events to split first.
///
/// Pure: it reads nothing and writes nothing. `path` only names the log in
/// error messages. The errors are the ones an append returns for the same
/// bytes: a log with no header, a last line that doesn't parse and isn't
/// merged events (a truncated line), and merged events whose seqs don't
/// follow the line before.
#[doc(hidden)]
pub fn plan_append<'a, H: LogHeader>(
    content: &'a str,
    path: &Path,
) -> anyhow::Result<AppendPlan<'a>> {
    let mut lines = content.lines();
    let first_line = lines.next().map(str::trim).unwrap_or("");
    if first_line.is_empty() {
        let problem = if content.trim().is_empty() {
            "state file is empty"
        } else {
            "state file has empty first line"
        };
        return Err(EngineError::StateFileCorrupted(format!(
            "{problem}, so nothing was appended to {}",
            path.display()
        ))
        .into());
    }
    parse_header::<H>(first_line)?;

    let unterminated = !content.ends_with('\n');
    let Some(last) = last_event_line(content) else {
        return Ok(AppendPlan {
            next_seq: 1,
            unterminated,
            merged: None,
        });
    };
    let err = match serde_json::from_str::<serde_json::Value>(last.text.trim()) {
        Ok(val) => {
            return Ok(AppendPlan {
                next_seq: val.get("seq").and_then(|s| s.as_u64()).unwrap_or(0) + 1,
                unterminated,
                merged: None,
            })
        }
        Err(e) => e,
    };
    let pieces = match split_last_line(&last) {
        SplitOutcome::NotMerged => {
            return Err(anyhow::anyhow!("failed to parse last event line: {}", err));
        }
        SplitOutcome::DoesNotFollow(seqs) => {
            return Err(EngineError::StateFileCorrupted(format!(
                "line {} of {} holds {} events written with no newline between them (seq {}), \
                 and their seqs don't follow the line before it (or that line can't be \
                 read), so nothing was appended. \
                 Back up the log, then put each of those events on a line of its own",
                last.line_number,
                path.display(),
                seqs.len(),
                join_seqs(&seqs)
            ))
            .into());
        }
        SplitOutcome::Split(pieces) => pieces,
    };
    Ok(AppendPlan {
        next_seq: pieces[pieces.len() - 1].1.seq + 1,
        unterminated,
        merged: Some(MergedLastLine {
            offset: last.offset,
            line_number: last.line_number,
            pieces,
        }),
    })
}

/// Read the header line from a state file.
///
/// Parses the first line as a `StateFileHeader`. If it doesn't parse,
/// the file is treated as corrupted.
pub fn read_header(path: &Path) -> anyhow::Result<StateFileHeader> {
    read_header_only(path)
}

/// Header-type-generic form of [`read_header`]: parse the first line
/// and stop.
///
/// Used by the request store's listing walk, which must not pay for a
/// full replay of every request just to answer an identity filter.
pub fn read_header_only<H: LogHeader>(path: &Path) -> anyhow::Result<H> {
    let file = crate::fs_read::open_plain(path)
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

/// Parse a header line, rejecting a log written by a newer koto.
///
/// The version gate runs here — before any event is looked at — so a
/// forward-incompatible log fails at line one rather than partway
/// through a replay.
fn parse_header<H: LogHeader>(first_line: &str) -> anyhow::Result<H> {
    let header = serde_json::from_str::<H>(first_line)
        .map_err(|e| EngineError::StateFileCorrupted(header_parse_failure(first_line, &e)))?;
    if header.schema_version() > H::max_supported_schema_version() {
        return Err(EngineError::IncompatibleSchemaVersion {
            found: header.schema_version(),
            max_supported: H::max_supported_schema_version(),
        }
        .into());
    }
    Ok(header)
}

/// Say why a first line isn't a header.
///
/// A first line that is itself an event is the shape an append to a
/// missing log used to leave (koto#236, koto#200), and serde's "missing
/// field `workflow`" doesn't tell anyone that. Every other failure keeps
/// the serde message.
fn header_parse_failure(first_line: &str, err: &serde_json::Error) -> String {
    let event = serde_json::from_str::<serde_json::Value>(first_line)
        .ok()
        .filter(|v| v.get("seq").is_some() && v.get("type").is_some());
    match event {
        Some(v) => format!(
            "state log has no header: its first line is a `{}` event (seq {}). \
             The session was never initialized, or its log was recreated after the \
             session was removed, and nothing in the log can rebuild it",
            v["type"].as_str().unwrap_or("unknown"),
            v["seq"]
        ),
        None => format!("failed to parse header: {}", err),
    }
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
/// - A final line holding well-formed events with no newline between them,
///   whose seqs continue the log, is read as those events, with a warning
pub fn read_events(path: &Path) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
    read_log(path)
}

/// Whether a recovered truncated final line is announced on stderr.
///
/// A session log has several writers (a tick, `koto context` readers, a
/// command's own `koto` calls), but each append is one write under the
/// append lock, so a reader rarely meets a torn tail there: one means a
/// writer died partway and is worth a warning. A request log is multi-writer and is
/// read lock-free on a two-second poll, so a reader racing a
/// concurrent append hits the same condition legitimately — warning
/// would turn normal operation into a stderr stream
/// (DESIGN-request-lifecycle.md Decision 1, "reads must be quiet").
#[derive(Clone, Copy, PartialEq, Eq)]
enum TailReport {
    Warn,
    Quiet,
}

/// Header-type-generic form of [`read_events`].
///
/// This is the one implementation of the sequence-gap validation and
/// the truncated-final-line recovery; both log families funnel through
/// it (DESIGN-request-lifecycle.md Decision 1).
pub fn read_log<H: LogHeader>(path: &Path) -> anyhow::Result<(H, Vec<Event>)> {
    read_log_inner(path, TailReport::Warn)
}

/// [`read_log`] with the truncated-final-line warning suppressed.
///
/// For lock-free readers of a multi-writer log, where a torn tail is an
/// expected race rather than evidence of a crash.
pub fn read_log_quiet<H: LogHeader>(path: &Path) -> anyhow::Result<(H, Vec<Event>)> {
    read_log_inner(path, TailReport::Quiet)
}

fn read_log_inner<H: LogHeader>(
    path: &Path,
    report: TailReport,
) -> anyhow::Result<(H, Vec<Event>)> {
    let content = crate::fs_read::open_plain(path)
        .and_then(|mut file| {
            let mut content = String::new();
            file.read_to_string(&mut content).map(|_| content)
        })
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;
    parse_log_content(&content, &path.display().to_string(), report)
}

/// Parse a session's state log held in memory, as [`read_events`] parses
/// the file: the same header parse, sequence check and tail recovery, with
/// the truncated-tail warning suppressed. For a caller that already holds
/// the bytes it is about to push, such as the summary writer, so it never
/// reads the log a second time.
pub fn parse_state_log(bytes: &[u8]) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
    let content = std::str::from_utf8(bytes)
        .map_err(|e| EngineError::StateFileCorrupted(format!("state log is not UTF-8: {}", e)))?;
    parse_log_content(content, "the state log", TailReport::Quiet)
}

/// The one parse of a log's text, shared by the file and in-memory readers.
/// `source` names the log in a warning.
fn parse_log_content<H: LogHeader>(
    content: &str,
    source: &str,
    report: TailReport,
) -> anyhow::Result<(H, Vec<Event>)> {
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
                    // Well-formed events merged onto one line by a writer
                    // that never wrote its newline (koto#419): read them
                    // as the events they are. The next append splits the
                    // line on disk.
                    if let Some(pieces) = split_concatenated(trimmed) {
                        let seqs: Vec<u64> = pieces.iter().map(|(_, e)| e.seq).collect();
                        if consecutive_from(&seqs, expected_seq) {
                            if report == TailReport::Warn {
                                eprintln!(
                                    "warning: final line in {} holds {} events written with no \
                                     newline between them (seq {}), reading them as separate \
                                     events; the next write to the session splits the line",
                                    source,
                                    seqs.len(),
                                    join_seqs(&seqs)
                                );
                            }
                            events.extend(pieces.into_iter().map(|(_, event)| event));
                            break;
                        }
                    }
                    // Truncated final line: recoverable. Return what we
                    // have, announcing it only when the caller asked.
                    if report == TailReport::Warn {
                        eprintln!(
                            "warning: truncated final line in {}, recovering: {}",
                            source, e
                        );
                    }
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

/// Resolve a `template_path` recorded on a `WorkflowInitialized` event
/// against a session directory.
///
/// The inline (`--from-stdin`) path stores the compiled artifact *inside*
/// the session directory and records a **session-relative** `template_path`
/// (e.g. just `<sha256>.json`). The file-template (`--template <path>`)
/// path records an **absolute** `~/.cache/koto/<sha256>.json` path. This
/// helper is the single resolution seam: a relative path is joined against
/// `session_dir`; an absolute path passes through unchanged.
///
/// This mirrors the "absolute passes through" convention of
/// [`crate::engine::path_resolution::resolve_template_path`], but uses a
/// distinct resolution base — the session directory rather than the parent
/// source dir / submitter cwd — so the two resolvers stay independent.
///
/// An empty `template_path` (the unclaimed needs-agent placeholder) passes
/// through unchanged: callers detect and reject that case separately, and
/// joining it against the session dir would silently produce the session
/// directory itself.
///
/// One fallback: an absolute path that doesn't exist, whose file name is a
/// content hash (exactly 64 lowercase hex digits plus `.json`), resolves to
/// the file of that name in `session_dir` when one is there. That is how a
/// session imported from another host (`koto session import`) runs: its log
/// keeps the origin host's cache path byte for byte, and the import stores
/// the compiled template beside the log under its hash. The name check means
/// a crafted `template_path` can't point the fallback at any other file in
/// the session directory, and every reader still verifies the bytes against
/// the header's `template_hash`.
#[doc(hidden)]
pub fn resolve_template_path_in_session(template_path: &str, session_dir: &Path) -> String {
    if template_path.is_empty() {
        return template_path.to_string();
    }
    let path = Path::new(template_path);
    if path.is_absolute() {
        if !path.exists() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let local = session_dir.join(name);
                if is_content_hash_file_name(name) && local.is_file() {
                    return local.to_string_lossy().into_owned();
                }
            }
        }
        template_path.to_string()
    } else {
        session_dir.join(path).to_string_lossy().into_owned()
    }
}

/// Whether `name` is `<64 lowercase hex digits>.json`, the file name the
/// template cache gives a compiled template.
fn is_content_hash_file_name(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(is_template_hash)
}

/// Whether `hash` has the form of a `template_hash`: exactly 64 lowercase
/// hex digits, the SHA-256 the template cache names files by. Anything that
/// turns a recorded hash into a file name checks this first.
#[doc(hidden)]
pub fn is_template_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Derive full machine state from header and event log.
///
/// Uses `derive_state_from_log` for the current state, and extracts
/// template info from the header and workflow_initialized event.
///
/// `session_dir` is the directory the session's state file lives in. A
/// session-relative `template_path` (used by the inline `--from-stdin`
/// path so the compiled artifact survives `~/.cache` eviction and session
/// relocation) is resolved against it here, once, so every reader inherits
/// an absolute, directly-readable path. Absolute `template_path` values
/// (the `--template <file>` path) pass through unchanged.
pub fn derive_machine_state(
    header: &StateFileHeader,
    events: &[Event],
    session_dir: &Path,
) -> Option<MachineState> {
    let current_state = derive_state_from_log(events)?;

    // Find the template_path from the workflow_initialized event.
    let template_path = events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
        _ => None,
    })?;

    let template_path = resolve_template_path_in_session(&template_path, session_dir);

    Some(MachineState {
        current_state,
        template_path,
        template_hash: header.template_hash.clone(),
    })
}

/// Check whether `state_name` is a terminal state in the compiled template at
/// `template_path`.
///
/// Returns `false` on any I/O or parse error (graceful degradation). This is
/// the single implementation shared by the dashboard read seam and the
/// `/workflows` projection writer.
pub fn is_terminal_state(template_path: &str, state_name: &str) -> bool {
    let read = crate::fs_read::open_plain(template_path).and_then(|mut file| {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map(|_| bytes)
    });
    let bytes = match read {
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

/// Return true if `state` names a failure terminal, matching the dashboard's
/// `classify_status` rule: the state name contains "failed" or "error"
/// (case-insensitive). Shared by the dashboard read seam and the `/workflows`
/// projection writer so both classify failure identically.
pub fn is_failed_state(state: Option<&str>) -> bool {
    state
        .map(|s| {
            let lower = s.to_lowercase();
            lower.contains("failed") || lower.contains("error")
        })
        .unwrap_or(false)
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

/// Which state-entry events close a scan window.
///
/// Two questions read the log backwards from an entry into the current phase,
/// and they no longer want the same entry. See [`epoch_slice`] and
/// [`delivery_window`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// Every entry naming the phase, a self-entry included.
    AnyEntry,
    /// Entries that mean the agent arrived rather than looped: a transition from
    /// a different phase, and every rewind. A rewind counts however it records
    /// its source, because it is an instruction to redo rather than continue.
    ArrivalFromElsewhere,
}

impl Boundary {
    /// Whether an entry whose source phase equals its target opens a window.
    ///
    /// Written as an exhaustive match rather than an equality test so that
    /// adding a variant fails to compile here -- at the one place that names the
    /// decision -- instead of silently defaulting at the two call sites inside
    /// the scan.
    fn opens_on_self_entry(self) -> bool {
        match self {
            Boundary::AnyEntry => true,
            Boundary::ArrivalFromElsewhere => false,
        }
    }
}

/// The events since the workflow last entered `current_state` under `boundary`.
///
/// State-entry events are `Transitioned`, `DirectedTransition`, and `Rewound`.
/// Under [`Boundary::AnyEntry`] every one of them naming the phase closes the
/// window. Under [`Boundary::ArrivalFromElsewhere`] a transition whose source
/// phase equals its target does not: the agent did not arrive anywhere, it went
/// around a loop it was already in.
///
/// **The rewind arm deliberately does not bind `from`.** A rewind opens both
/// windows whatever phases it records, and not reading the field is what keeps
/// the delivery answer from coming to depend on how `koto rewind` chooses its
/// destination.
///
/// **`current_state` must be the phase the workflow currently occupies.** The
/// returned slice runs from the last qualifying entry event to the end of the
/// log, and nothing truncates it at the departure -- there is nothing to
/// truncate when the phase is the current one. Ask about a phase the workflow
/// has since left and the slice will span everything that happened after it,
/// which answers a different question than the caller meant.
///
/// With no qualifying entry event, every event is in scope. Under either
/// boundary that arm is unreachable for a log `koto init` produced: init always
/// appends `Transitioned { from: None, to: <initial> }`, and a `None` source
/// opens under both -- `AnyEntry` never looks at the source, and
/// `ArrivalFromElsewhere` reads `None` as "not from this phase". The arm exists
/// for synthetic and truncated logs, and under `ArrivalFromElsewhere` it is the
/// one place this boundary's failure direction inverts: a log whose only entries
/// naming the phase are self-entries falls through here and can be answered by a
/// record from an earlier visit, suppressing where `AnyEntry` would deliver.
///
/// Only inspects `.payload` and position within `events`; `.seq`,
/// `.timestamp`, `.event_type`, and `.idempotency_hash` never factor in. The
/// directed-transition path relies on this precisely: it builds its
/// post-append event list in memory from the payload it just appended,
/// without re-reading the log, and a caller-constructed `Event` with
/// placeholder metadata is exactly as valid an input here as one read back
/// from disk.
///
/// Inside `.payload`, `to` is read on all three entry variants and `from` on
/// two. `Transitioned.from` is an `Option<String>` and the other two are plain
/// `String`s, so the `Transitioned` arm compares through `as_deref()`; an absent
/// source reads as "not from this phase" and opens the window, which is both
/// what initialization needs and the safe direction for a log written before the
/// field existed.
fn entry_slice<'a>(events: &'a [Event], current_state: &str, boundary: Boundary) -> &'a [Event] {
    match entry_index(events, current_state, boundary) {
        Some(idx) => &events[idx + 1..],
        None => events,
    }
}

/// Index of the entry event that opens [`entry_slice`]'s window, or `None`
/// when no entry event names `current_state`. The one scan both the slice
/// and [`any_entry_index`] read, so the two can't disagree.
fn entry_index(events: &[Event], current_state: &str, boundary: Boundary) -> Option<usize> {
    events.iter().enumerate().rev().find_map(|(idx, e)| {
        if opens_window(&e.payload, current_state, boundary) {
            Some(idx)
        } else {
            None
        }
    })
}

/// Whether `payload` is an entry into `current_state` that opens a window
/// under `boundary`: the one predicate [`entry_index`] and [`visit_start`]
/// both apply.
fn opens_window(payload: &EventPayload, current_state: &str, boundary: Boundary) -> bool {
    match payload {
        EventPayload::Transitioned { from, to, .. } => {
            to == current_state
                && (boundary.opens_on_self_entry() || from.as_deref() != Some(current_state))
        }
        EventPayload::DirectedTransition { from, to, .. } => {
            to == current_state && (boundary.opens_on_self_entry() || from != current_state)
        }
        EventPayload::Rewound { to, .. } => to == current_state,
        _ => false,
    }
}

/// The state a state-entry event enters, or `None` for any other event.
#[doc(hidden)]
pub fn entered_state(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::Transitioned { to, .. }
        | EventPayload::DirectedTransition { to, .. }
        | EventPayload::Rewound { to, .. } => Some(to.as_str()),
        _ => None,
    }
}

/// Whether `payload` opens a visit: a state entry that is an arrival (a
/// first entry, a transition or directed transition from a different state,
/// or any rewind). A transition from a state to itself stays in its visit.
#[doc(hidden)]
pub fn opens_visit(payload: &EventPayload) -> bool {
    entered_state(payload)
        .is_some_and(|to| opens_window(payload, to, Boundary::ArrivalFromElsewhere))
}

/// Whether a raw log line can be a state entry, without parsing it.
///
/// [`Event`] serializes `seq`, `timestamp`, then `type`, so the type word
/// sits a few dozen bytes in, and reading it there keeps a long visit's
/// lines (a failed check's logged streams run to kilobytes) from being
/// searched end to end. A line laid out any other way falls back to
/// searching all of it for an entry's type word. Either way a `true` is only
/// a candidate: the caller parses the line.
#[doc(hidden)]
pub fn may_be_entry(line: &str) -> bool {
    const ENTRY_TYPES: [&[u8]; 3] = [b"transitioned\"", b"directed_transition\"", b"rewound\""];
    const TYPE_KEY: &[u8] = b"\"type\":\"";
    let bytes = line.as_bytes();
    // Fast path: relies on `impl Serialize for Event` writing `seq`, then
    // `timestamp`, then `type`, so the first `"type":"` within the head is
    // the event's own. Reorder that serializer and this must change with it.
    let head = &bytes[..bytes.len().min(96)];
    if let Some(at) = head.windows(TYPE_KEY.len()).position(|w| w == TYPE_KEY) {
        let word = &bytes[at + TYPE_KEY.len()..];
        return ENTRY_TYPES.iter().any(|t| word.starts_with(t));
    }
    line.contains("\"transitioned\"")
        || line.contains("\"directed_transition\"")
        || line.contains("\"rewound\"")
}

/// The `seq` of the event that opened the visit a session's log ends in,
/// read from the log's raw bytes (header line included).
///
/// A visit runs from an arrival in a state -- its first entry, an arrival
/// from a different state, or any rewind -- until the session next arrives
/// somewhere. A transition from a state to itself stays in the visit. This
/// is [`arrival_index`]'s boundary, the one `visit_attempt` and the
/// delivery window cut at, applied to the state the log's last entry names,
/// so the two can't drift.
///
/// Scans backwards from the end and parses only lines that can be a state
/// entry, so the work is bounded by the length of the current visit, not
/// the log. `None` when the log holds no state entry. A line that doesn't
/// parse is skipped, as [`read_events`] skips it.
#[doc(hidden)]
pub fn visit_start(log: &str) -> Option<u64> {
    current_visit(log).map(|(_, seq)| seq)
}

/// [`visit_start`], together with the state the log ends in (the state its
/// last state entry names, as [`derive_state_from_log`] reads it), both
/// from one scan of the same bytes.
#[doc(hidden)]
pub fn current_visit(log: &str) -> Option<(String, u64)> {
    let mut current: Option<String> = None;
    for line in log.lines().rev() {
        if !may_be_entry(line) {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(line.trim()) else {
            continue;
        };
        let Some(to) = entered_state(&event.payload) else {
            continue;
        };
        let state = current.get_or_insert_with(|| to.to_string());
        if opens_window(&event.payload, state, Boundary::ArrivalFromElsewhere) {
            return current.map(|state| (state, event.seq));
        }
    }
    None
}

/// The highest `seq` in a session's log, read from its raw bytes (header
/// line included): the `seq` of the last non-empty line that parses as a
/// JSON object with an unsigned `seq`. Appends take strictly increasing
/// seqs, so the last event holds the highest. Lines that don't parse that
/// way, such as a partial last write, are passed over. This is a cheaper
/// reading than [`read_events`] and doesn't promise to agree with it on a
/// damaged log. `None` when no line carries a `seq`.
#[doc(hidden)]
pub fn last_seq(log: &str) -> Option<u64> {
    #[derive(serde::Deserialize)]
    struct Seq {
        seq: u64,
    }
    log.lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .find_map(|line| serde_json::from_str::<Seq>(line.trim()).ok())
        .map(|s| s.seq)
}

/// Index of the event that began `current_state`'s current epoch: the last
/// state-entry event naming it, a self-entry included. The
/// [`Boundary::AnyEntry`] boundary [`epoch_slice`] cuts at, exposed as a
/// position so the decider's visit key (`visit_seq`) comes from the same
/// rule rather than a second copy of it.
///
/// Like [`entry_slice`], only meaningful when `current_state` is the phase
/// the workflow currently occupies; `engine::decider::visit_start_index`
/// checks that before calling.
#[doc(hidden)]
pub fn any_entry_index(events: &[Event], current_state: &str) -> Option<usize> {
    entry_index(events, current_state, Boundary::AnyEntry)
}

/// Index of the event that opened `current_state`'s current visit: the last
/// arrival from a different state, or rewind, naming it. A self-transition
/// doesn't open one. The [`Boundary::ArrivalFromElsewhere`] boundary
/// [`delivery_window`] and the `visit_attempt` stamp cut at, exposed so a
/// decider check's `visit_seq` comes from the same rule.
#[doc(hidden)]
pub fn arrival_index(events: &[Event], current_state: &str) -> Option<usize> {
    entry_index(events, current_state, Boundary::ArrivalFromElsewhere)
}

/// The events since the workflow last entered `current_state` at all -- the
/// phase's current epoch.
///
/// Every state-entry event naming the phase closes the epoch, a self-transition
/// included -- see [`entry_slice`] for the mechanism. This is the boundary the
/// gate-blocked classification reads, and it is unchanged from the one koto has
/// always used.
///
/// **Widening it to match [`delivery_window`] breaks the dashboard, silently.**
/// The predicate built on this slice takes the *latest* gate evaluation inside
/// it, so a wider window changes the answer whenever the narrow epoch holds no
/// gate evaluation and the wider one does -- every tick after a self-entry until
/// a fresh evaluation lands. A gate that failed before a loop's last lap would
/// keep the blocked badge on. Nothing in the repo exercises that predicate
/// directly except the one unit test that asserts both boundaries against a
/// single log.
#[doc(hidden)]
pub fn epoch_slice<'a>(events: &'a [Event], current_state: &str) -> &'a [Event] {
    entry_slice(events, current_state, Boundary::AnyEntry)
}

/// The events since the workflow last arrived at `current_state` from somewhere
/// else -- the phase's current delivery window.
///
/// A transition from the phase to itself does not open one -- see
/// [`entry_slice`] for the mechanism -- so a delivery recorded before a
/// self-transition still counts after it and an agent going around a loop is not
/// re-sent instructions it already holds. Any rewind does open one: a rewind is
/// an instruction to redo the work rather than to continue it, and the agent it
/// addresses needs the procedure again.
#[doc(hidden)]
pub fn delivery_window<'a>(events: &'a [Event], current_state: &str) -> &'a [Event] {
    entry_slice(events, current_state, Boundary::ArrivalFromElsewhere)
}

/// Whether the most recent gate evaluation in `current_state`'s current epoch
/// did not pass -- the epoch-scoped half of the dashboard's blocked
/// classification.
///
/// The epoch is [`epoch_slice`]'s -- the events after the last transition,
/// directed transition or rewind into `current_state`, a self-transition
/// included. Within it, the latest `GateEvaluated` decides. Returns `false` when
/// the current epoch has no gate evaluation. Callers combine this with a
/// non-terminal check to get "blocked": the dashboard read seam (`read_session`)
/// and the `/workflows` projection writer both call this so they classify a
/// blocked session identically (no drift).
///
/// This boundary is *not* the delivery window. A self-transition closes the
/// epoch, so a gate verdict from before a loop's last lap drops out of scope and
/// the blocked badge clears; it does not close the delivery window, so an agent
/// looping in one phase is not re-sent instructions it holds. The two are
/// separate answers to separate questions and a change to one must not move the
/// other.
pub fn latest_epoch_gate_failed(events: &[Event], current_state: &str) -> bool {
    latest_epoch_gate(events, current_state).is_some_and(|(_, passed)| !passed)
}

/// The most recent gate evaluated in `current_state`'s current epoch: its
/// name and whether it passed, or `None` when the epoch holds no gate
/// evaluation.
///
/// This is the read [`latest_epoch_gate_failed`] is built on, so a caller
/// that names the gate a session is blocked on names the same gate the
/// blocked classification decided on.
pub fn latest_epoch_gate(events: &[Event], current_state: &str) -> Option<(String, bool)> {
    epoch_slice(events, current_state)
        .iter()
        .rev()
        .find_map(|e| {
            if let EventPayload::GateEvaluated { gate, outcome, .. } = &e.payload {
                Some((gate.clone(), outcome == "passed"))
            } else {
                None
            }
        })
}

/// Whether `current_state`'s instructions have already been delivered to a
/// caller inside the phase's current delivery window.
///
/// Called from both `koto next` response-construction sites in
/// `src/cli/mod.rs` -- the natural-advancement path and the directed-
/// transition path -- through the shared
/// [`crate::cli::next_types::NextResponse::with_details_suppressed_unless_full`]
/// combinator, so the two cannot disagree about when a phase has already
/// delivered.
///
/// The window is [`delivery_window`]'s. The same precondition applies as for the
/// epoch: `current_state` is the phase the workflow currently occupies.
///
/// The question asked inside the window is unchanged -- has a delivery been
/// *recorded*? A rule keyed on the shape of the entry event instead would
/// suppress on a self-entry whose window holds no record at all, which is
/// reachable when a crash lands between printing a response and appending its
/// record, and would leave that agent with no lap that ever hands the procedure
/// back.
///
/// The record is matched on the phase it names as well as on its position in
/// the slice. Position alone would be enough for the ordinary case, where the
/// only records after the entry event belong to the phase just entered. The
/// name check answers the cases where that does not hold: a record for another
/// phase landing inside this window, and the fallback where no entry event
/// names the phase and the slice is the whole log. It also errs in the safe
/// direction -- a record naming the wrong phase reads as "not delivered" and
/// koto delivers again, the same direction the design accepts for a crash
/// between printing a response and appending its record.
///
/// Takes a plain event slice and touches no backend, so a caller that has the
/// events in memory does not pay a read to ask.
///
/// Only a record of a response that carried the details counts: one whose
/// `details_included` is `true`, or absent, as on every record written
/// before the field existed (koto then appended the record only for such a
/// response). A record of a response whose details were suppressed says the
/// directive was printed, not that the details were.
pub fn instructions_delivered_this_window(events: &[Event], current_state: &str) -> bool {
    delivery_window(events, current_state).iter().any(|e| {
        matches!(
            &e.payload,
            EventPayload::InstructionsDelivered { state, details_included }
                if state == current_state && details_included.unwrap_or(true)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, StateFileHeader};
    use std::collections::HashMap;

    fn make_header() -> StateFileHeader {
        StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: "test-wf".to_string(),
            template_hash: "deadbeef".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
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
            kind: None,
            root_session_id: None,
            parent_session_id: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                machine: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    source: None,
                    exported_fields: None,
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    source: None,
                    exported_fields: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    source: None,
                    exported_fields: None,
                },
            ),
            make_event(
                4,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
        ];

        // An absolute template_path (the --template <file> path) must
        // pass through derive_machine_state unchanged regardless of the
        // session dir supplied.
        let ms = derive_machine_state(&header, &events, Path::new("/sessions/test-wf")).unwrap();
        assert_eq!(ms.current_state, "gather");
        assert_eq!(ms.template_path, "/cache/abc.json");
        assert_eq!(ms.template_hash, "deadbeef");
    }

    #[test]
    fn derive_machine_state_resolves_relative_path_against_session_dir() {
        let header = make_header();
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    // A session-relative path (the --from-stdin path): just
                    // the content-addressed filename, no directory.
                    template_path: "abc123.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
        ];

        let session_dir = Path::new("/home/x/.koto/sessions/test-wf");
        let ms = derive_machine_state(&header, &events, session_dir).unwrap();
        assert_eq!(
            ms.template_path, "/home/x/.koto/sessions/test-wf/abc123.json",
            "a relative template_path must be joined against the session dir"
        );
    }

    #[test]
    fn derive_machine_state_passes_empty_template_path_through() {
        // The unclaimed needs-agent placeholder records an empty
        // template_path; it must not be joined against the session dir
        // (which would silently yield the session directory itself).
        let header = make_header();
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: String::new(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
        ];
        let ms = derive_machine_state(&header, &events, Path::new("/sessions/test-wf")).unwrap();
        assert_eq!(ms.template_path, "");
    }

    #[test]
    fn a_template_hash_is_exactly_64_lowercase_hex_digits() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(is_template_hash(hash));
        assert!(!is_template_hash(""));
        assert!(!is_template_hash(&hash[1..]));
        assert!(!is_template_hash(&format!("{}0", hash)));
        assert!(!is_template_hash(&hash.to_uppercase()));
        assert!(!is_template_hash(&format!("../{}", &hash[3..])));
        assert!(!is_template_hash(&format!("{}g", &hash[1..])));
    }

    #[test]
    fn derive_machine_state_returns_none_for_empty() {
        let header = make_header();
        assert!(derive_machine_state(&header, &[], Path::new("/sessions/test-wf")).is_none());
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
                machine: None,
            },
        )];
        // Only init event, no transitioned -- no current state derivable
        assert!(derive_machine_state(&header, &events, Path::new("/sessions/test-wf")).is_none());
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "implementation".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                decision: None,
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
                check: Default::default(),
                streams: None,
                poll: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                context_assignments: None,
                vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
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
                context_assignments: None,
                vars_matched: None,
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
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: Some("gather".to_string()),
                    to: "analyze".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
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
                decision: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
            make_event(
                3,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: HashMap::new(),
                    submitter_cwd: None,
                    source: None,
                    exported_fields: None,
                },
            ),
            make_event(
                4,
                EventPayload::DirectedTransition {
                    from: "gather".to_string(),
                    to: "analyze".to_string(),
                    rationale: None,
                    decision: None,
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
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: HashMap::new(),
                    submitter_cwd: None,
                    source: None,
                    exported_fields: None,
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
                    truncated: false,
                    check: Default::default(),
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

    // -----------------------------------------------------------------------
    // instructions_delivered_this_window / delivery_window
    // -----------------------------------------------------------------------

    fn transitioned(seq: u64, from: Option<&str>, to: &str) -> Event {
        make_event(
            seq,
            EventPayload::Transitioned {
                from: from.map(str::to_string),
                to: to.to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
        )
    }

    fn rewound(seq: u64, from: &str, to: &str) -> Event {
        make_event(
            seq,
            EventPayload::Rewound {
                from: from.to_string(),
                to: to.to_string(),
                rationale: None,
            },
        )
    }

    fn directed(seq: u64, from: &str, to: &str) -> Event {
        make_event(
            seq,
            EventPayload::DirectedTransition {
                from: from.to_string(),
                to: to.to_string(),
                rationale: None,
                decision: None,
            },
        )
    }

    fn gate_evaluated(seq: u64, state: &str, gate: &str, outcome: &str) -> Event {
        make_event(
            seq,
            EventPayload::GateEvaluated {
                state: state.to_string(),
                gate: gate.to_string(),
                output: serde_json::json!({}),
                outcome: outcome.to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                check: Default::default(),
                streams: None,
                poll: None,
            },
        )
    }

    fn delivered(seq: u64, state: &str) -> Event {
        make_event(
            seq,
            EventPayload::InstructionsDelivered {
                state: state.to_string(),
                details_included: None,
            },
        )
    }

    #[test]
    fn only_a_record_of_delivered_details_counts_as_a_delivery() {
        let with = |details: Option<bool>| {
            vec![
                transitioned(1, None, "gather"),
                make_event(
                    2,
                    EventPayload::InstructionsDelivered {
                        state: "gather".to_string(),
                        details_included: details,
                    },
                ),
            ]
        };
        assert!(instructions_delivered_this_window(
            &with(Some(true)),
            "gather"
        ));
        assert!(instructions_delivered_this_window(&with(None), "gather"));
        assert!(!instructions_delivered_this_window(
            &with(Some(false)),
            "gather"
        ));
    }

    #[test]
    fn instructions_delivered_false_when_nothing_was_delivered() {
        let events = vec![transitioned(1, None, "gather")];
        assert!(!instructions_delivered_this_window(&events, "gather"));
    }

    #[test]
    fn instructions_delivered_true_within_the_current_window() {
        let events = vec![
            transitioned(1, None, "gather"),
            delivered(2, "gather"),
            make_event(
                3,
                EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields: HashMap::new(),
                    submitter_cwd: None,
                    source: None,
                    exported_fields: None,
                },
            ),
        ];
        assert!(instructions_delivered_this_window(&events, "gather"));
    }

    #[test]
    fn instructions_delivered_false_when_the_record_predates_the_entry_event() {
        // gather -> implement -> gather: the first occupancy's delivery does
        // not carry over into the second.
        let events = vec![
            transitioned(1, None, "gather"),
            delivered(2, "gather"),
            transitioned(3, Some("gather"), "implement"),
            transitioned(4, Some("implement"), "gather"),
        ];
        assert!(!instructions_delivered_this_window(&events, "gather"));
    }

    #[test]
    fn instructions_delivered_resets_on_arrival_by_rewind() {
        let events = vec![
            transitioned(1, None, "gather"),
            delivered(2, "gather"),
            transitioned(3, Some("gather"), "implement"),
            delivered(4, "implement"),
            rewound(5, "implement", "gather"),
        ];
        assert!(!instructions_delivered_this_window(&events, "gather"));
    }

    #[test]
    fn instructions_delivered_survives_a_self_transition() {
        // A self-transition does not open a delivery window -- the agent went
        // around a loop it was already in rather than arriving anywhere -- so
        // the delivery recorded before it still answers after it.
        let events = vec![
            transitioned(1, None, "review"),
            delivered(2, "review"),
            transitioned(3, Some("review"), "review"),
        ];
        assert!(instructions_delivered_this_window(&events, "review"));

        // ...and it keeps answering however many laps follow.
        let mut events = events;
        events.push(transitioned(4, Some("review"), "review"));
        assert!(instructions_delivered_this_window(&events, "review"));
    }

    #[test]
    fn instructions_delivered_false_on_a_self_transition_with_no_record() {
        // The rule is keyed on the recorded delivery, not on the shape of the
        // entry event. A window whose record was lost -- a crash between
        // printing a response and appending it -- delivers again on the next
        // lap rather than starving an agent that never received the procedure.
        let events = vec![
            transitioned(1, None, "gather"),
            transitioned(2, Some("gather"), "review"),
            transitioned(3, Some("review"), "review"),
        ];
        assert!(!instructions_delivered_this_window(&events, "review"));
    }

    #[test]
    fn instructions_delivered_resets_on_a_same_phase_rewind() {
        // `koto rewind` right after a self-transition records the same phase as
        // both source and target. A rewind is an instruction to redo the work,
        // so it opens a window whatever phases it names -- and the rewind arm
        // does not read `from` at all, which is what keeps this answer
        // independent of how rewind picks its destination.
        let events = vec![
            transitioned(1, None, "review"),
            delivered(2, "review"),
            transitioned(3, Some("review"), "review"),
            rewound(4, "review", "review"),
        ];
        assert!(!instructions_delivered_this_window(&events, "review"));
    }

    #[test]
    fn the_epoch_and_the_delivery_window_disagree_across_a_self_transition() {
        // One log, two boundaries, opposite answers -- which is the whole point
        // of splitting them. The self-transition closes the epoch, so the gate
        // verdict from the previous lap drops out of scope and the blocked badge
        // clears; it does not close the delivery window, so the instructions
        // stay suppressed.
        //
        // This is the test that fails loudly if someone widens the shared scan
        // in place instead of keeping two boundaries.
        let events = vec![
            transitioned(1, None, "gather"),
            transitioned(2, Some("gather"), "review"),
            delivered(3, "review"),
            gate_evaluated(4, "review", "approval", "failed"),
        ];
        // Before the lap: the failed gate is inside both windows.
        assert!(latest_epoch_gate_failed(&events, "review"));
        assert!(instructions_delivered_this_window(&events, "review"));

        let mut events = events;
        events.push(transitioned(5, Some("review"), "review"));
        assert!(!latest_epoch_gate_failed(&events, "review"));
        assert!(instructions_delivered_this_window(&events, "review"));
    }

    #[test]
    fn latest_epoch_gate_names_the_latest_gate_and_its_outcome() {
        let events = vec![
            transitioned(1, None, "gather"),
            transitioned(2, Some("gather"), "review"),
            gate_evaluated(3, "review", "lint", "passed"),
            gate_evaluated(4, "review", "approval", "failed"),
        ];
        assert_eq!(
            latest_epoch_gate(&events, "review"),
            Some(("approval".to_string(), false))
        );
        assert!(latest_epoch_gate_failed(&events, "review"));

        let mut events = events;
        events.push(gate_evaluated(5, "review", "approval", "passed"));
        assert_eq!(
            latest_epoch_gate(&events, "review"),
            Some(("approval".to_string(), true))
        );
        assert!(!latest_epoch_gate_failed(&events, "review"));
    }

    #[test]
    fn latest_epoch_gate_is_none_without_a_gate_in_the_epoch() {
        let events = vec![
            transitioned(1, None, "gather"),
            transitioned(2, Some("gather"), "review"),
        ];
        assert_eq!(latest_epoch_gate(&events, "review"), None);
    }

    #[test]
    fn latest_epoch_gate_ignores_an_earlier_epoch() {
        // A gate that failed on an earlier visit to the state drops out once
        // the state is entered again.
        let events = vec![
            transitioned(1, None, "review"),
            gate_evaluated(2, "review", "approval", "failed"),
            transitioned(3, Some("review"), "gather"),
            transitioned(4, Some("gather"), "review"),
        ];
        assert_eq!(latest_epoch_gate(&events, "review"), None);
        assert!(!latest_epoch_gate_failed(&events, "review"));
    }

    #[test]
    fn instructions_delivered_ignores_an_intermediate_phases_record() {
        // A multi-hop advance leaves a delivery for each phase it passed
        // through. Asking about the phase the workflow now occupies must not
        // be answered by the record an intermediate phase left behind.
        let events = vec![
            transitioned(1, None, "gather"),
            delivered(2, "gather"),
            transitioned(3, Some("gather"), "implement"),
            delivered(4, "implement"),
            transitioned(5, Some("implement"), "verify"),
        ];
        assert!(!instructions_delivered_this_window(&events, "verify"));

        // The slicing alone carries the case above, because the entry into
        // `verify` is the last event. Put an intermediate phase's record
        // *inside* the current occupancy and the name check is the only thing
        // left holding the answer.
        let mut events = events;
        events.push(delivered(6, "implement"));
        assert!(!instructions_delivered_this_window(&events, "verify"));
    }

    #[test]
    fn instructions_delivered_survives_a_directed_self_transition() {
        // `DirectedTransition` is the third state-entry kind, and the one the
        // `--to` handler appends. Arriving at a different phase opens a window;
        // a second `--to` issued while already standing there does not, because
        // routing to the phase you occupy is a lap rather than an arrival.
        let events = vec![
            transitioned(1, None, "gather"),
            directed(2, "gather", "implement"),
            delivered(3, "implement"),
        ];
        assert!(instructions_delivered_this_window(&events, "implement"));

        let mut events = events;
        events.push(directed(4, "implement", "implement"));
        assert!(instructions_delivered_this_window(&events, "implement"));
    }

    #[test]
    fn instructions_delivered_reads_the_whole_log_when_no_entry_event_names_the_state() {
        // Same fallback as `latest_epoch_gate_failed`: with no entry event for
        // the state, every event is in scope.
        let events = vec![delivered(1, "gather")];
        assert!(instructions_delivered_this_window(&events, "gather"));
        assert!(!instructions_delivered_this_window(&events, "implement"));
    }

    // -----------------------------------------------------------------------
    // Merged and unterminated last lines (koto#419)
    // -----------------------------------------------------------------------

    fn intent(seq: u64, text: &str) -> Event {
        make_event(
            seq,
            EventPayload::IntentUpdated {
                intent: text.to_string(),
            },
        )
    }

    fn line(event: &Event) -> String {
        serde_json::to_string(event).unwrap()
    }

    #[test]
    fn split_concatenated_takes_only_whole_events() {
        let a = line(&intent(1, "a"));
        let b = line(&intent(2, "b"));
        let merged = format!("{a}{b}");
        let pieces = split_concatenated(&merged).unwrap();
        assert_eq!(
            pieces.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            vec![a.as_str(), b.as_str()]
        );
        assert!(split_concatenated(&a).is_none(), "one event is not merged");
        assert!(split_concatenated(&format!("{a}{}", &b[..5])).is_none());
        assert!(split_concatenated(&format!("{a}{{\"x\":1}}")).is_none());
        assert!(split_concatenated("").is_none());
    }
}
