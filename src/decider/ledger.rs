//! The decider ledger: `<koto_root>/_decider_ledger.jsonl`.
//!
//! Every consultation and every agent answer to a consulted visit is
//! appended here as one JSON line. Session logs are deleted on cleanup and
//! prune, and child sessions are always cleaned up, so the ledger is the
//! only place the pairs that promotion is judged on survive. It is
//! authoritative state: it can't be rebuilt, nothing in koto deletes or
//! compacts it, and it is created mode 0600.
//!
//! ## Line format
//!
//! Each line is one [`LedgerRecord`], tagged by `kind`:
//!
//! - `{"kind":"consulted","v":1,"at":…,"session":…,"session_id":…,
//!   …every DeciderConsultation field…}` with `"trimmed":true` when the
//!   probabilities were dropped to fit the line bound;
//! - `{"kind":"answered","v":1,"at":…,"session":…,"session_id":…,
//!   "state":…,"visit_seq":…,"values":{field: value}}`.
//!
//! `at` is RFC 3339 UTC. `session_id` is the session header's UUID, or
//! `null` for a header that has none; records with a null id can't be
//! paired. Pairs join on `(session_id, visit_seq)`, because session names
//! are reused across runs.
//!
//! Lines are capped at [`MAX_LEDGER_LINE_BYTES`] including the newline, so
//! every append is a single atomic `O_APPEND` write. A `consulted` line
//! over the cap is rewritten without any `probabilities` map; one still
//! over it is not written.
//!
//! No record carries input content, the API key, a response body, or error
//! text: [`DeciderConsultation`] holds none, and an `answered` record
//! holds only the values the agent submitted for declared fields.
//!
//! This module holds the writers. Reading the ledger belongs to
//! [`super::report`], behind `koto decider report`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::record::DeciderConsultation;
use crate::engine::jsonl_append::append_bounded_line;

/// File name of the ledger, joined under `<koto_root>`.
pub const LEDGER_FILE_NAME: &str = "_decider_ledger.jsonl";

/// Maximum ledger line length in bytes, trailing newline included.
pub const MAX_LEDGER_LINE_BYTES: usize = 4096;

/// The `v` every record carries.
pub const LEDGER_VERSION: u32 = 1;

/// `<koto_root>/_decider_ledger.jsonl`. Creates nothing.
pub fn ledger_path(koto_root: &Path) -> PathBuf {
    koto_root.join(LEDGER_FILE_NAME)
}

/// The keys both record kinds carry after `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordEnvelope {
    /// Always [`LEDGER_VERSION`].
    pub v: u32,
    /// When the record was written, RFC 3339 UTC.
    pub at: String,
    /// The session name. Names are reused across runs; don't join on it.
    pub session: String,
    /// The session header's `session_id`, or `None` (serialized `null`)
    /// when the header has none.
    pub session_id: Option<String>,
}

impl RecordEnvelope {
    /// An envelope stamped now. An empty `session_id` becomes `None`.
    pub fn now(session: &str, session_id: Option<&str>) -> Self {
        RecordEnvelope {
            v: LEDGER_VERSION,
            at: crate::engine::types::now_iso8601(),
            session: session.to_string(),
            session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
        }
    }
}

/// One consultation: the envelope plus every [`DeciderConsultation`] field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsultedRecord {
    #[serde(flatten)]
    pub envelope: RecordEnvelope,
    #[serde(flatten)]
    pub consultation: DeciderConsultation,
    /// `true` when every `probabilities` map was dropped to fit the line
    /// bound. `winning`, `confidence`, `at_threshold`, and `outcome` stay.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trimmed: bool,
}

/// The agent's answer on a visit whose consultation wasn't applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnsweredRecord {
    #[serde(flatten)]
    pub envelope: RecordEnvelope,
    /// The state the evidence was submitted on.
    pub state: String,
    /// The consultation's `visit_seq`, which this answer pairs with.
    pub visit_seq: u64,
    /// The submitted value of each field that carries a `decider` block.
    pub values: BTreeMap<String, serde_json::Value>,
}

/// One ledger line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerRecord {
    Consulted(ConsultedRecord),
    Answered(AnsweredRecord),
}

impl LedgerRecord {
    /// The wire `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            LedgerRecord::Consulted(_) => "consulted",
            LedgerRecord::Answered(_) => "answered",
        }
    }

    /// A `consulted` record stamped now.
    pub fn consulted(
        session: &str,
        session_id: Option<&str>,
        consultation: DeciderConsultation,
    ) -> Self {
        LedgerRecord::Consulted(ConsultedRecord {
            envelope: RecordEnvelope::now(session, session_id),
            consultation,
            trimmed: false,
        })
    }

    /// An `answered` record stamped now.
    pub fn answered(
        session: &str,
        session_id: Option<&str>,
        state: &str,
        visit_seq: u64,
        values: BTreeMap<String, serde_json::Value>,
    ) -> Self {
        LedgerRecord::Answered(AnsweredRecord {
            envelope: RecordEnvelope::now(session, session_id),
            state: state.to_string(),
            visit_seq,
            values,
        })
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether `line` plus its newline fits the bound.
fn fits(line: &str) -> bool {
    line.len() < MAX_LEDGER_LINE_BYTES
}

/// Serialize `record` as one ledger line (no newline), trimming a
/// `consulted` record's probabilities when the full line doesn't fit.
///
/// Fails when the line is still over [`MAX_LEDGER_LINE_BYTES`].
pub fn render_line(record: &LedgerRecord) -> Result<String> {
    let line = serde_json::to_string(record).context("failed to serialize ledger record")?;
    if fits(&line) {
        return Ok(line);
    }
    let LedgerRecord::Consulted(c) = record else {
        anyhow::bail!(
            "{} record is {} bytes, over the {}-byte line bound",
            record.kind(),
            line.len() + 1,
            MAX_LEDGER_LINE_BYTES
        );
    };
    let mut trimmed = c.clone();
    for field in trimmed.consultation.fields.values_mut() {
        field.probabilities.clear();
    }
    trimmed.trimmed = true;
    let line = serde_json::to_string(&LedgerRecord::Consulted(trimmed))
        .context("failed to serialize ledger record")?;
    if fits(&line) {
        return Ok(line);
    }
    anyhow::bail!(
        "consulted record is {} bytes without probabilities, over the {}-byte line bound",
        line.len() + 1,
        MAX_LEDGER_LINE_BYTES
    )
}

/// Append `record` to the ledger under `koto_root`, creating the directory
/// and (mode 0600) the file when absent.
pub fn append_record(koto_root: &Path, record: &LedgerRecord) -> Result<()> {
    let line = render_line(record)?;
    append_bounded_line(
        koto_root,
        &ledger_path(koto_root),
        &line,
        MAX_LEDGER_LINE_BYTES,
    )
}

/// [`append_record`], turning any failure into one warning on stderr. A
/// ledger that can't be written never changes a tick's outcome.
///
/// `koto_root` is `None` when there is no home directory.
pub fn append_or_warn(koto_root: Option<&Path>, record: &LedgerRecord) {
    let Some(root) = koto_root else {
        eprintln!("warning: decider ledger write failed (no home directory)");
        return;
    };
    if let Err(e) = append_record(root, record) {
        eprintln!(
            "warning: decider ledger write failed ({}: {})",
            ledger_path(root).display(),
            e.root_cause()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decider::evaluate::FieldOutcome;
    use crate::decider::record::{ConsultationOutcome, FieldConsultation};
    use crate::decider::types::{ErrorClass, SettingOrigin};
    use crate::template::decider::DeciderMode;

    fn field(values: &[&str]) -> FieldConsultation {
        FieldConsultation {
            declaration_hash: "d".repeat(64),
            modes: values
                .iter()
                .map(|v| (v.to_string(), DeciderMode::Shadow))
                .collect(),
            probabilities: values
                .iter()
                .enumerate()
                .map(|(i, v)| (v.to_string(), 0.1234 + i as f64 / 10_000.0))
                .collect(),
            winning: Some(values[0].to_string()),
            confidence: Some(0.1234),
            threshold: Some(0.9),
            at_threshold: false,
            outcome: Some(FieldOutcome::BelowThreshold),
        }
    }

    fn consultation(fields: BTreeMap<String, FieldConsultation>) -> DeciderConsultation {
        DeciderConsultation {
            state: "review".to_string(),
            visit_seq: 7,
            provider: "jev".to_string(),
            model: "jev-1".to_string(),
            input_sha256: Some("a".repeat(64)),
            outcome: ConsultationOutcome::NotApplied,
            error_class: None,
            latency_ms: 12,
            directive_bytes: 300,
            endpoint_origin: SettingOrigin::Default,
            fields,
        }
    }

    fn one_field() -> DeciderConsultation {
        let mut fields = BTreeMap::new();
        fields.insert(
            "verdict".to_string(),
            field(&["proceed", "exit", "unclear"]),
        );
        consultation(fields)
    }

    #[test]
    fn ledger_path_is_under_the_koto_root() {
        assert_eq!(
            ledger_path(Path::new("/h/.koto")),
            PathBuf::from("/h/.koto/_decider_ledger.jsonl")
        );
    }

    #[test]
    fn a_consulted_record_round_trips_with_every_field_flattened() {
        let rec = LedgerRecord::consulted("wf", Some("uuid-1"), one_field());
        let line = render_line(&rec).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "consulted");
        assert_eq!(v["v"], 1);
        assert_eq!(v["session"], "wf");
        assert_eq!(v["session_id"], "uuid-1");
        assert!(v["at"].as_str().unwrap().ends_with('Z'));
        for key in [
            "state",
            "visit_seq",
            "provider",
            "model",
            "input_sha256",
            "outcome",
            "latency_ms",
            "directive_bytes",
            "endpoint_origin",
            "fields",
        ] {
            assert!(v.get(key).is_some(), "missing {}: {}", key, line);
        }
        let f = &v["fields"]["verdict"];
        for key in [
            "declaration_hash",
            "modes",
            "probabilities",
            "winning",
            "confidence",
            "threshold",
            "at_threshold",
            "outcome",
        ] {
            assert!(f.get(key).is_some(), "field missing {}: {}", key, f);
        }
        assert!(v.get("trimmed").is_none());
        let back: LedgerRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn error_class_is_carried_when_present() {
        let mut c = one_field();
        c.outcome = ConsultationOutcome::Error;
        c.error_class = Some(ErrorClass::Timeout);
        let rec = LedgerRecord::consulted("wf", Some("u"), c);
        let line = render_line(&rec).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error_class"], "timeout");
        assert_eq!(serde_json::from_str::<LedgerRecord>(&line).unwrap(), rec);
    }

    #[test]
    fn an_answered_record_round_trips() {
        let mut values = BTreeMap::new();
        values.insert("verdict".to_string(), serde_json::json!("exit"));
        let rec = LedgerRecord::answered("wf", Some("uuid-1"), "review", 7, values);
        let line = render_line(&rec).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "answered");
        assert_eq!(v["v"], 1);
        assert_eq!(v["visit_seq"], 7);
        assert_eq!(v["state"], "review");
        assert_eq!(v["values"]["verdict"], "exit");
        assert_eq!(serde_json::from_str::<LedgerRecord>(&line).unwrap(), rec);
    }

    #[test]
    fn an_empty_or_absent_session_id_serializes_null() {
        for id in [None, Some("")] {
            let c = LedgerRecord::consulted("wf", id, one_field());
            let a = LedgerRecord::answered("wf", id, "review", 7, BTreeMap::new());
            for rec in [c, a] {
                let v: serde_json::Value =
                    serde_json::from_str(&render_line(&rec).unwrap()).unwrap();
                assert!(v.as_object().unwrap().contains_key("session_id"));
                assert!(v["session_id"].is_null(), "{}", v);
            }
        }
    }

    /// A field with `n` values named `<prefix><i>`.
    fn wide(prefix: &str, n: usize) -> DeciderConsultation {
        let names: Vec<String> = (0..n).map(|i| format!("{}{:03}", prefix, i)).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut fields = BTreeMap::new();
        fields.insert("verdict".to_string(), field(&refs));
        consultation(fields)
    }

    #[test]
    fn an_oversized_consulted_line_drops_probabilities_and_fits() {
        let c = wide("value-name-", 80);
        let full =
            serde_json::to_string(&LedgerRecord::consulted("wf", Some("u"), c.clone())).unwrap();
        assert!(full.len() > MAX_LEDGER_LINE_BYTES, "fixture too small");

        let rec = LedgerRecord::consulted("wf", Some("u"), c);
        let line = render_line(&rec).unwrap();
        assert!(line.len() < MAX_LEDGER_LINE_BYTES, "{}", line.len());
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["trimmed"], true);
        let f = &v["fields"]["verdict"];
        assert!(f.get("probabilities").is_none());
        assert_eq!(f["winning"], "value-name-000");
        assert_eq!(f["confidence"], 0.1234);
        assert_eq!(f["at_threshold"], false);
        assert_eq!(f["outcome"], "below_threshold");
        let back: LedgerRecord = serde_json::from_str(&line).unwrap();
        let LedgerRecord::Consulted(back) = back else {
            panic!("kind");
        };
        assert!(back.trimmed);
        assert!(back.consultation.fields["verdict"].probabilities.is_empty());
    }

    #[test]
    fn a_line_still_over_the_bound_is_refused_and_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = LedgerRecord::consulted("wf", Some("u"), wide(&"v".repeat(60), 80));
        let err = render_line(&rec).unwrap_err();
        assert!(err.to_string().contains("without probabilities"), "{}", err);
        assert!(append_record(tmp.path(), &rec).is_err());
        assert!(!ledger_path(tmp.path()).exists());
    }

    #[test]
    fn append_record_writes_one_line_per_record() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".koto");
        append_record(
            &root,
            &LedgerRecord::consulted("wf", Some("u"), one_field()),
        )
        .unwrap();
        append_record(
            &root,
            &LedgerRecord::answered("wf", Some("u"), "review", 7, BTreeMap::new()),
        )
        .unwrap();
        let body = std::fs::read_to_string(ledger_path(&root)).unwrap();
        let kinds: Vec<String> = body
            .lines()
            .map(|l| {
                serde_json::from_str::<LedgerRecord>(l)
                    .unwrap()
                    .kind()
                    .to_string()
            })
            .collect();
        assert_eq!(kinds, vec!["consulted", "answered"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ledger_path(&root))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// Mirrors the terminal index's `race_n_writers_produce_n_parseable_lines`.
    #[test]
    fn race_n_writers_produce_n_parseable_ledger_lines() {
        use std::sync::Arc;
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let root = Arc::new(tmp.path().to_path_buf());
        let n = 32_usize;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let root = Arc::clone(&root);
                thread::spawn(move || {
                    let mut c = one_field();
                    c.visit_seq = i as u64;
                    let rec = LedgerRecord::consulted("wf", Some("u"), c);
                    let line = render_line(&rec).unwrap();
                    append_bounded_line(&root, &ledger_path(&root), &line, MAX_LEDGER_LINE_BYTES)
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let body = std::fs::read_to_string(ledger_path(&root)).unwrap();
        let mut seqs: Vec<u64> = body
            .lines()
            .map(|l| match serde_json::from_str::<LedgerRecord>(l).unwrap() {
                LedgerRecord::Consulted(c) => c.consultation.visit_seq,
                LedgerRecord::Answered(_) => panic!("kind"),
            })
            .collect();
        assert_eq!(seqs.len(), n);
        seqs.sort();
        assert_eq!(seqs, (0..n as u64).collect::<Vec<_>>());
    }
}
