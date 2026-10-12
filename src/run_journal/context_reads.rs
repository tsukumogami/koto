//! The visit-close fold: a visit's `context_read` events, summed into
//! `context_reads` records when the visit ends.
//!
//! koto logs one `context_read` event per read of a context key. The journal
//! never gets one line per read: when a commit closes a visit (see
//! [`closes_visit`]), [`records`] reads that visit's `context_read` events
//! out of the log bytes the commit already read under its lock and writes
//! one record per distinct (key, reader, access) tuple, in first-read
//! order. Nothing is kept between processes, because the session's log
//! already holds every read of the visit and the event that opened it.
//!
//! The fold holds at most [`MAX_TUPLES`] tuples and one overflow counter,
//! however long the visit is, and parses only the lines whose type word is
//! `context_read`.

use std::borrow::Cow;

use serde::Deserialize;

use super::{name_or_digest, Record, Relay};
use crate::engine::persistence::opens_visit;
use crate::engine::types::EventPayload;

/// The most tuples one visit writes a record of their own for. Reads of
/// any further tuple are summed into one overflow record, so a visit writes
/// at most `MAX_TUPLES + 1` lines.
pub(super) const MAX_TUPLES: usize = 50;

/// `access` of a read logged without one: such a read saw the content.
const DEFAULT_ACCESS: &str = "content";

/// The event types whose line the fold reads by name.
const CONTEXT_READ: &str = "context_read";
const SESSION_IMPORTED: &str = "session_imported";
const REQUEST_STORE_RESULT: &str = "request_store.result";
const WORKFLOW_CANCELLED: &str = "workflow_cancelled";

/// Whether committing `payload` ends the visit the log was in: an arrival
/// (a transition or directed transition from a different state, or any
/// rewind), a cancel, or the `request_store.result` the terminal tick
/// appends once per arrival at a terminal, just before the `terminal`
/// record. A transition from a state to itself closes nothing.
pub(super) fn closes_visit(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::WorkflowCancelled { .. } | EventPayload::RequestStoreResult { .. }
    ) || opens_visit(payload)
}

/// The `context_reads` records for the visit that opened at `visit_seq`
/// and that the commit described by `relay` closes, read from `log`, the
/// session's log as it stood before that commit. Empty when the visit
/// logged no read, or when its reads were already written.
///
/// Each record carries `relay`'s `koto.seq` (the closing event's) and
/// `koto.visit.seq`, and the state the visit's reads name.
pub(super) fn records(log: &str, visit_seq: u64, relay: &Relay<'_>) -> Vec<Record> {
    let Some(window) = window(log, visit_seq) else {
        return Vec::new();
    };
    let mut fold = Fold::default();
    for line in window.lines() {
        if head(line).is_some_and(|(_, kind)| kind == CONTEXT_READ) {
            if let Ok(read) = serde_json::from_str::<ReadLine<'_>>(line) {
                fold.add(&read.payload);
            }
        }
    }
    fold.into_records(relay)
}

/// The part of `log` whose reads belong to the visit that opened at
/// `visit_seq` and haven't been written yet: the lines after the opening
/// event, or after the last `session_imported` among them, since the reads
/// before an import belong to the session it was imported from.
///
/// `None` when a `request_store.result` or `workflow_cancelled` comes
/// later in the visit than any import. Those already closed the visit and
/// wrote its reads, and a read logged after them (a `koto context get` on a
/// kept terminal session, say) belongs to no open visit. This is what keeps
/// a second cancel, or a rewind out of a kept terminal, from writing a
/// visit's reads twice.
///
/// Scans backwards from the end, reading only each line's leading `seq`
/// and type word, so the work is bounded by the visit's length.
fn window(log: &str, visit_seq: u64) -> Option<&str> {
    for line in log.lines().rev() {
        let Some((seq, kind)) = head(line) else {
            continue;
        };
        if seq <= visit_seq {
            return Some(after(log, line));
        }
        match kind.as_ref() {
            SESSION_IMPORTED => return Some(after(log, line)),
            REQUEST_STORE_RESULT | WORKFLOW_CANCELLED => return None,
            _ => {}
        }
    }
    None
}

/// The rest of `log` after `line`, which is a slice of it.
fn after<'a>(log: &'a str, line: &str) -> &'a str {
    let start = line.as_ptr() as usize - log.as_ptr() as usize;
    &log[start + line.len()..]
}

/// A log line's `seq` and type word, or `None` for a line that isn't an
/// event (the header, a torn write).
///
/// An event line starts `{"seq":N,"timestamp":"...","type":"<type>",` (see
/// `impl Serialize for Event`), so both are read from its first bytes
/// without parsing the rest, which for a check event can run to kilobytes
/// of output. A line laid out any other way is parsed for just those two
/// fields.
fn head(line: &str) -> Option<(u64, Cow<'_, str>)> {
    const SEQ_KEY: &str = "{\"seq\":";
    const TYPE_KEY: &[u8] = b"\"type\":\"";
    // Spelled as an escape so this file's double quotes stay paired for the
    // reference test that reads its string literals.
    const QUOTE: char = '\x22';
    let fast = line.strip_prefix(SEQ_KEY).and_then(|rest| {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let seq = rest[..digits].parse::<u64>().ok()?;
        let bytes = rest.as_bytes();
        let head = &bytes[..bytes.len().min(96)];
        let at = head.windows(TYPE_KEY.len()).position(|w| w == TYPE_KEY)? + TYPE_KEY.len();
        // `at` follows an ASCII quote, so it is a character boundary.
        let word = &rest[at..];
        let end = word.find(QUOTE)?;
        Some((seq, Cow::Borrowed(&word[..end])))
    });
    if fast.is_some() {
        return fast;
    }
    #[derive(Deserialize)]
    struct Head<'a> {
        seq: u64,
        #[serde(rename = "type", borrow)]
        kind: Cow<'a, str>,
    }
    let parsed: Head<'_> = serde_json::from_str(line.trim()).ok()?;
    Some((parsed.seq, parsed.kind))
}

/// The parts of a `context_read` line the fold reads. Strings borrow from
/// the line unless they hold an escape.
#[derive(Deserialize)]
struct ReadLine<'a> {
    #[serde(borrow)]
    payload: Read<'a>,
}

#[derive(Deserialize)]
struct Read<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
    #[serde(borrow)]
    reader: Cow<'a, str>,
    #[serde(borrow)]
    state: Cow<'a, str>,
    present: bool,
    #[serde(default, borrow)]
    hash: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    access: Option<Cow<'a, str>>,
}

/// Reads and present reads.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    reads: u64,
    present: u64,
}

impl Counts {
    fn add(&mut self, present: bool) {
        self.reads += 1;
        self.present += u64::from(present);
    }
}

/// One (key, reader, access) tuple's reads in a visit.
#[derive(Debug)]
struct Tuple {
    key: String,
    reader: String,
    access: String,
    counts: Counts,
    /// The hash of the tuple's first present read.
    first_hash: Option<String>,
    /// The hash of its last present read.
    last_hash: Option<String>,
}

/// A visit's reads so far: the state they name, up to [`MAX_TUPLES`] tuples
/// in first-read order, and the summed reads of every tuple past those.
#[derive(Debug, Default)]
struct Fold {
    state: Option<String>,
    tuples: Vec<Tuple>,
    overflow: Counts,
}

impl Fold {
    fn add(&mut self, read: &Read<'_>) {
        if self.state.is_none() {
            self.state = Some(read.state.to_string());
        }
        let access = read.access.as_deref().unwrap_or(DEFAULT_ACCESS);
        let found = self
            .tuples
            .iter()
            .position(|t| t.key == read.key && t.reader == read.reader && t.access == access);
        let tuple = match found {
            Some(i) => &mut self.tuples[i],
            None if self.tuples.len() < MAX_TUPLES => {
                self.tuples.push(Tuple {
                    key: read.key.to_string(),
                    reader: read.reader.to_string(),
                    access: access.to_string(),
                    counts: Counts::default(),
                    first_hash: None,
                    last_hash: None,
                });
                self.tuples.last_mut().expect("just pushed")
            }
            None => {
                self.overflow.add(read.present);
                return;
            }
        };
        tuple.counts.add(read.present);
        if let Some(hash) = read.hash.as_deref().filter(|_| read.present) {
            if tuple.first_hash.is_none() {
                tuple.first_hash = Some(hash.to_string());
            }
            if tuple.last_hash.as_deref() != Some(hash) {
                tuple.last_hash = Some(hash.to_string());
            }
        }
    }

    fn into_records(self, relay: &Relay<'_>) -> Vec<Record> {
        let state = self.state.as_deref();
        let mut out = Vec::with_capacity(self.tuples.len() + 1);
        for t in &self.tuples {
            let mut record = relay
                .record("context_reads")
                .name("koto.state", state)
                .name("koto.context.key", Some(name_or_digest(&t.key).as_str()))
                .name("koto.context.reader", Some(t.reader.as_str()))
                .name("koto.context.access", Some(t.access.as_str()))
                .int("koto.context.read_count", Some(t.counts.reads))
                .int("koto.context.present_count", Some(t.counts.present));
            if t.counts.present > 0 {
                record = record
                    .id("koto.context.first_hash", t.first_hash.as_deref())
                    .id("koto.context.last_hash", t.last_hash.as_deref());
            }
            out.push(record);
        }
        if self.overflow.reads > 0 {
            out.push(
                relay
                    .record("context_reads")
                    .name("koto.state", state)
                    .flag("koto.context.overflow", true)
                    .int("koto.context.read_count", Some(self.overflow.reads))
                    .int("koto.context.present_count", Some(self.overflow.present)),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: u64, kind: &str, payload: &str) -> String {
        format!(
            r#"{{"seq":{seq},"timestamp":"2026-10-11T00:00:00.000Z","type":"{kind}","payload":{payload}}}"#
        )
    }

    fn read(seq: u64, key: &str, present: bool) -> String {
        let hash = if present {
            format!(r#","hash":"{}""#, "ab".repeat(32))
        } else {
            String::new()
        };
        line(
            seq,
            CONTEXT_READ,
            &format!(
                r#"{{"key":"{key}","reader":"gate","state":"a","present":{present}{hash},"access":"content","gate":"g"}}"#
            ),
        )
    }

    #[test]
    fn head_reads_seq_and_type_from_the_lines_first_bytes_or_parses_them() {
        let l = read(12, "k", true);
        let (seq, kind) = head(&l).unwrap();
        assert_eq!((seq, kind.as_ref()), (12, CONTEXT_READ));
        assert!(matches!(head(&l).unwrap().1, Cow::Borrowed(_)));
        // Another key order is parsed.
        let odd = r#"{"type":"workflow_cancelled","payload":{},"seq":7,"timestamp":"t"}"#;
        let (seq, kind) = head(odd).unwrap();
        assert_eq!((seq, kind.as_ref()), (7, WORKFLOW_CANCELLED));
        // A header and a torn line are not events.
        assert!(head(r#"{"schema_version":1,"workflow":"w"}"#).is_none());
        assert!(head(r#"{"seq":9,"timest"#).is_none());
    }

    #[test]
    fn the_window_starts_after_the_opening_event_or_the_last_import() {
        let log = [
            r#"{"schema_version":1}"#.to_string(),
            read(1, "before", true),
            line(2, "transitioned", r#"{"from":null,"to":"a"}"#),
            read(3, "in", true),
            read(4, "in", false),
        ]
        .join("\n");
        let w = window(&log, 2).unwrap();
        assert_eq!(w.lines().filter(|l| !l.is_empty()).count(), 2, "{w}");
        assert!(!w.contains("before"));

        let imported = format!(
            "{log}\n{}\n{}",
            line(5, SESSION_IMPORTED, "{}"),
            read(6, "after", true)
        );
        let w = window(&imported, 2).unwrap();
        assert!(w.contains("after") && !w.contains("\"in\""), "{w}");

        for closer in [REQUEST_STORE_RESULT, WORKFLOW_CANCELLED] {
            let closed = format!(
                "{log}\n{}\n{}",
                line(5, closer, "{}"),
                read(6, "late", true)
            );
            assert!(window(&closed, 2).is_none(), "{closer}");
        }
    }

    fn read_payload<'a>(key: &'a str, reader: &'a str, hash: Option<&'a str>) -> Read<'a> {
        Read {
            key: Cow::Borrowed(key),
            reader: Cow::Borrowed(reader),
            state: Cow::Borrowed("a"),
            present: hash.is_some(),
            hash: hash.map(Cow::Borrowed),
            access: None,
        }
    }

    /// However many reads and tuples a visit has, the fold holds at most 50
    /// tuples and the overflow counts.
    #[test]
    fn the_fold_holds_fifty_tuples_and_the_overflow_whatever_the_visit() {
        let mut fold = Fold::default();
        let keys: Vec<String> = (0..500).map(|i| format!("key-{i}")).collect();
        for i in 0..20_000 {
            let present = i % 4 == 0;
            fold.add(&read_payload(
                &keys[i % keys.len()],
                "gate",
                present.then_some("ab"),
            ));
            assert!(fold.tuples.len() <= MAX_TUPLES);
        }
        assert_eq!(fold.tuples.len(), MAX_TUPLES);
        let kept: u64 = fold.tuples.iter().map(|t| t.counts.reads).sum();
        assert_eq!(kept + fold.overflow.reads, 20_000);
        assert_eq!(fold.tuples[0].key, "key-0");
        assert_eq!(fold.tuples[0].access, DEFAULT_ACCESS);
    }

    /// At the longest values koto allows (a 255-byte session name, 128-byte
    /// state, reader and access, a 128-byte key written as itself and a
    /// 255-byte one written as its digest, 128-byte hashes, the largest
    /// counts), every line, the overflow's included, fits the line cap.
    #[test]
    fn context_reads_fit_the_line_cap_at_the_longest_values() {
        let session = "s".repeat(255);
        let id = "i".repeat(128);
        let relay = Relay {
            session: &session,
            session_id: &id,
            run_id: Some(&id),
            seq: u64::MAX,
            visit_seq: Some(u64::MAX),
        };
        let at_limit = "k".repeat(128);
        let too_long = "x".repeat(255);
        let hash = "h".repeat(128);
        let mut fold = Fold {
            state: Some("t".repeat(128)),
            ..Fold::default()
        };
        for key in [&at_limit, &too_long] {
            fold.tuples.push(Tuple {
                key: key.clone(),
                reader: "r".repeat(128),
                access: "a".repeat(128),
                counts: Counts {
                    reads: u64::MAX,
                    present: u64::MAX,
                },
                first_hash: Some(hash.clone()),
                last_hash: Some(hash.clone()),
            });
        }
        fold.overflow = Counts {
            reads: u64::MAX,
            present: u64::MAX,
        };
        let lines: Vec<String> = fold
            .into_records(&relay)
            .iter()
            .map(Record::to_line)
            .collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(
                line.len() < super::super::MAX_LINE_BYTES,
                "{} bytes: {line}",
                line.len()
            );
        }
        assert!(lines[0].contains(&at_limit));
        assert!(lines[1].contains(&name_or_digest(&too_long)));
        assert!(!lines[1].contains(&too_long));
        assert!(lines[2].contains("\"koto.context.overflow\":true"));
    }
}
