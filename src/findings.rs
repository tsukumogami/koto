//! Findings a check reports, and the one koto writes when it reports none
//! (DESIGN-koto-failure-reporting.md, Decision 1).
//!
//! A check reports a finding by printing a line to standard output whose
//! first bytes are [`FINDING_PREFIX`], followed by exactly one JSON object.
//! [`parse_findings`] reads those lines out of redacted stdout; anything that
//! doesn't match the grammar is ordinary output and stays that way. A failed
//! corrective check whose findings hold no `error` gets one more, written by
//! [`fallback_finding`] from its last line of output or from koto's own
//! description of the outcome. [`build_failure`] keeps a failed check's
//! parsed findings whole and the fallback separate; the capped list the
//! response carries is derived from the two.
//!
//! Every string in a finding has been redacted and is capped after
//! redaction, never splitting a marker or a character.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::redact::{fold_one_line, redact_str, safe_cut_len, RedactedText, Redactor};

/// The bytes a finding line starts with.
pub const FINDING_PREFIX: &str = "::koto-finding::";

/// Most findings a failed check returns in a `koto next` response, the
/// koto-written finding included.
pub const RESPONSE_FINDINGS_CAP: usize = 100;

/// Most findings a check event records in the session log, the
/// koto-written finding included, applied through [`cap_findings`].
pub const LOG_FINDINGS_CAP: usize = 50;

/// Longest `rule_id`, in bytes, after redaction.
pub const RULE_ID_MAX_BYTES: usize = 128;

/// Longest `path`, in bytes, after redaction.
pub const PATH_MAX_BYTES: usize = 512;

/// Longest `rule_ref`, in bytes, after redaction.
pub const RULE_REF_MAX_BYTES: usize = 512;

/// Longest `message`, in bytes, after redaction.
pub const MESSAGE_MAX_BYTES: usize = 1000;

/// Longest koto-written message, in characters, after it is folded onto one
/// line (the same bound as a terminal `failure_reason`). The
/// [`MESSAGE_MAX_BYTES`] cap applies after this fold.
pub const FALLBACK_MESSAGE_MAX_CHARS: usize =
    crate::engine::terminal_result::FAILURE_REASON_MAX_CHARS;

/// The line the CLI's truncation note adds to a cut `default_action` stream.
/// It is koto's bookkeeping, not the check's output, so it is never chosen
/// as a koto-written finding's message.
pub const TRUNCATION_NOTE_LINE: &str = "... [output truncated]";

/// A finding's severity. It never changes a check's outcome.
///
/// koto only ever writes `error`, `warning` and `info`, but the vocabulary is
/// open on read: a value a later koto writes deserializes as
/// [`FindingLevel::Other`] and serializes back unchanged, so an older reader
/// never rejects a record over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingLevel {
    Error,
    Warning,
    Info,
    /// A level this koto doesn't know. koto never produces it.
    Other(String),
}

impl FindingLevel {
    /// The level a check may print, or `None` for anything else. Unlike
    /// deserialization, the finding-line grammar is closed.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "error" => Some(FindingLevel::Error),
            "warning" => Some(FindingLevel::Warning),
            "info" => Some(FindingLevel::Info),
            _ => None,
        }
    }

    /// The wire value.
    pub fn as_str(&self) -> &str {
        match self {
            FindingLevel::Error => "error",
            FindingLevel::Warning => "warning",
            FindingLevel::Info => "info",
            FindingLevel::Other(s) => s,
        }
    }
}

impl Serialize for FindingLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FindingLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(FindingLevel::parse(&s).unwrap_or(FindingLevel::Other(s)))
    }
}

/// Where a finding's message came from.
///
/// Open on read, like [`FindingLevel`]: an unknown value deserializes as
/// [`MessageSource::Other`] and serializes back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSource {
    /// The check printed the finding.
    Check,
    /// koto wrote the finding from a line of the check's output.
    Output,
    /// koto wrote the finding and its message.
    Koto,
    /// A source this koto doesn't know. koto never produces it.
    Other(String),
}

impl MessageSource {
    /// The wire value.
    pub fn as_str(&self) -> &str {
        match self {
            MessageSource::Check => "check",
            MessageSource::Output => "output",
            MessageSource::Koto => "koto",
            MessageSource::Other(s) => s,
        }
    }
}

impl Serialize for MessageSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MessageSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "check" => MessageSource::Check,
            "output" => MessageSource::Output,
            "koto" => MessageSource::Koto,
            _ => MessageSource::Other(s),
        })
    }
}

/// One finding, as the response carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub rule_id: String,
    pub level: FindingLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_ref: Option<String>,
    /// The check's own claim when it made one. Otherwise the advance loop
    /// fills it in on every finding a response returns: `true` when this
    /// invocation recorded evidence for the state, or when the state's
    /// `default_action` exited 0 (and delivered its capture, if it declares
    /// one); `false` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_landed: Option<bool>,
    pub message_source: MessageSource,
}

/// A failed check's captured output: the leading 64 KiB of each redacted
/// stream, with whether each was cut.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Captured {
    pub stdout: RedactedText,
    pub stderr: RedactedText,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// What a command-backed check produced, before its outcome is judged:
/// every finding it printed, its captured streams, and whether stderr ends
/// with a note koto wrote (a timeout, spawn, wait or polling note).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CheckOutput {
    pub findings: Vec<Finding>,
    pub captured: Captured,
    pub stderr_ends_with_note: bool,
}

impl CheckOutput {
    /// Parse the findings out of `captured`'s stdout.
    pub fn parse(captured: Captured, stderr_ends_with_note: bool, redactor: &Redactor) -> Self {
        let findings = parse_findings(&captured.stdout, captured.stdout_truncated, redactor);
        CheckOutput {
            findings,
            captured,
            stderr_ends_with_note,
        }
    }
}

/// What koto adds to a failed corrective check beyond the findings it
/// parsed: the finding koto wrote when none of them has level `error`, and
/// the captured output.
///
/// The parsed findings sit beside it, whole and uncapped (on
/// `StructuredGateResult::findings`), so the capped list the response
/// carries is a derived view, and the fallback stays addressable rather than
/// merged into a list. The response's `failure` object is
/// [`GateFailure::response`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GateFailure {
    /// The koto-written finding; `None` when a parsed finding has level
    /// `error`.
    pub fallback: Option<Finding>,
    /// Present for command gates and `default_action`; context gates run no
    /// command and have nothing to capture.
    pub captured: Option<Captured>,
}

impl GateFailure {
    /// The `failure` object the response returns beside the check's
    /// unchanged `output`: `parsed` and the fallback, capped at
    /// [`RESPONSE_FINDINGS_CAP`] by [`cap_findings`].
    pub fn response(&self, parsed: &[Finding]) -> FailureResponse {
        let (findings, findings_truncated) =
            cap_findings(parsed, self.fallback.clone(), RESPONSE_FINDINGS_CAP);
        FailureResponse {
            findings,
            findings_truncated,
            captured: self.captured.clone(),
        }
    }
}

/// The `failure` object a failed corrective check returns in a `koto next`
/// response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailureResponse {
    pub findings: Vec<Finding>,
    pub findings_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured: Option<Captured>,
}

/// Set `effect_landed` on every finding that doesn't state its own.
pub fn fill_effect_landed<'a>(findings: impl IntoIterator<Item = &'a mut Finding>, landed: bool) {
    for f in findings {
        if f.effect_landed.is_none() {
            f.effect_landed = Some(landed);
        }
    }
}

/// Every stdout line, each paired with whether it may be read as a finding
/// line. The unterminated last fragment of a stdout that was cut at the
/// capture bound may not: it is the head of a line whose end is missing.
fn stdout_lines(stdout: &str, truncated: bool) -> Vec<(&str, bool)> {
    let mut lines: Vec<(&str, bool)> = stdout.split('\n').map(|l| (l, true)).collect();
    if stdout.ends_with('\n') {
        // `split` yields an empty piece after the final newline.
        lines.pop();
    } else if truncated {
        if let Some(last) = lines.last_mut() {
            last.1 = false;
        }
    }
    lines
}

/// Read one line as a finding, before redaction and caps, or `None` when it
/// is ordinary output.
///
/// The grammar: [`FINDING_PREFIX`] at the start of the line, then one JSON
/// object, then nothing but trailing spaces or tabs, and at most one
/// carriage return as the line's very last byte. Required keys: `rule_id` (non-empty string),
/// `level` (`error`, `warning` or `info`), `message` (string). Optional:
/// `path`, `line` (integer >= 1, only with `path`), `column` (integer >= 1,
/// only with `line`), `rule_ref`, `effect_landed` (boolean). `null` is
/// absent and unknown keys are ignored. Any violation makes the whole line
/// ordinary output.
pub fn decode_line(line: &str) -> Option<Finding> {
    let rest = line.strip_prefix(FINDING_PREFIX)?;
    if !rest.starts_with('{') {
        return None;
    }
    let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
    let value = stream.next()?.ok()?;
    let tail = &rest[stream.byte_offset()..];
    let tail = tail.strip_suffix('\r').unwrap_or(tail);
    if !tail.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    let obj = value.as_object()?;

    // `null` and a missing key are the same thing.
    let get = |key: &str| obj.get(key).filter(|v| !v.is_null());
    let opt_string = |key: &str| -> Result<Option<String>, ()> {
        match get(key) {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(()),
        }
    };
    let opt_position = |key: &str| -> Result<Option<u64>, ()> {
        match get(key) {
            None => Ok(None),
            Some(v) => match v.as_u64() {
                Some(n) if n >= 1 => Ok(Some(n)),
                _ => Err(()),
            },
        }
    };

    let rule_id = opt_string("rule_id").ok()??;
    if rule_id.is_empty() {
        return None;
    }
    let level = FindingLevel::parse(get("level")?.as_str()?)?;
    let message = opt_string("message").ok()??;
    let path = opt_string("path").ok()?;
    let line_no = opt_position("line").ok()?;
    let column = opt_position("column").ok()?;
    let rule_ref = opt_string("rule_ref").ok()?;
    let effect_landed = match get("effect_landed") {
        None => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => return None,
    };
    if line_no.is_some() && path.is_none() {
        return None;
    }
    if column.is_some() && line_no.is_none() {
        return None;
    }
    Some(Finding {
        rule_id,
        level,
        message,
        path,
        line: line_no,
        column,
        rule_ref,
        effect_landed,
        message_source: MessageSource::Check,
    })
}

/// Redact a decoded string and cap it at `max` bytes.
///
/// The stdout it came from was already redacted, but JSON escaping (a `\u`
/// escape such as `\u0041` for `A`, or `\/` for `/`) can spell a known value the raw-byte pass couldn't see, so the
/// decoded value goes through the redactor again before the cap.
fn redact_field(value: &str, max: usize, redactor: &Redactor) -> String {
    redact_str(value, redactor).cut_bytes(max).0.into_string()
}

/// Every finding line in `stdout`, in emission order, redacted and capped.
///
/// `stdout_truncated` says the stream was cut at the capture bound, so its
/// unterminated last fragment is never parsed.
pub fn parse_findings(
    stdout: &RedactedText,
    stdout_truncated: bool,
    redactor: &Redactor,
) -> Vec<Finding> {
    stdout_lines(stdout.as_str(), stdout_truncated)
        .into_iter()
        .filter(|(_, parseable)| *parseable)
        .filter_map(|(line, _)| decode_line(line))
        .map(|f| Finding {
            rule_id: redact_field(&f.rule_id, RULE_ID_MAX_BYTES, redactor),
            message: redact_field(&f.message, MESSAGE_MAX_BYTES, redactor),
            path: f.path.map(|p| redact_field(&p, PATH_MAX_BYTES, redactor)),
            rule_ref: f
                .rule_ref
                .map(|r| redact_field(&r, RULE_REF_MAX_BYTES, redactor)),
            ..f
        })
        .collect()
}

/// Whether `line` may be a koto-written finding's message.
fn usable_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed != TRUNCATION_NOTE_LINE
}

/// The last non-blank line of `stderr`.
fn last_stderr_line(stderr: &str) -> Option<&str> {
    stderr.lines().rev().find(|l| usable_line(l))
}

/// The last non-blank line of `stdout` that isn't a finding line.
fn last_stdout_line(stdout: &str, truncated: bool) -> Option<&str> {
    stdout_lines(stdout, truncated)
        .into_iter()
        .rev()
        .find(|(line, parseable)| usable_line(line) && !(*parseable && decode_line(line).is_some()))
        .map(|(line, _)| line)
}

/// The finding koto writes for a failed corrective check that reported no
/// `error`.
///
/// Its `rule_id` is `check` (the gate's name, or `__action__`), its level
/// `error`, with no location and no `rule_ref`; `effect_landed` is left for
/// the advance loop. The message is the first of: the last non-blank line of
/// stderr (koto's own note when `captured` says stderr ends with one); the
/// last non-blank stdout line that isn't a finding line; `sentence`, koto's
/// description of the outcome. It is folded onto one line and cut to
/// [`FALLBACK_MESSAGE_MAX_CHARS`] (500) characters, ending in `...`, as a
/// terminal `failure_reason` is. The result is then held to
/// [`MESSAGE_MAX_BYTES`] (1,000) like any message: 500 multibyte characters
/// can exceed it, and such a message loses its `...` and ends at the last
/// whole character (or whole marker) that fits.
pub fn fallback_finding(check: &str, output: Option<&CheckOutput>, sentence: &str) -> Finding {
    let from_output = output.and_then(|o| {
        if let Some(line) = last_stderr_line(&o.captured.stderr) {
            let source = if o.stderr_ends_with_note {
                MessageSource::Koto
            } else {
                MessageSource::Output
            };
            return Some((line, source));
        }
        last_stdout_line(&o.captured.stdout, o.captured.stdout_truncated)
            .map(|line| (line, MessageSource::Output))
    });
    let (text, message_source) = from_output.unwrap_or((sentence, MessageSource::Koto));
    let mut message =
        fold_one_line(text, FALLBACK_MESSAGE_MAX_CHARS).unwrap_or_else(|| check.to_string());
    message.truncate(safe_cut_len(message.as_bytes(), MESSAGE_MAX_BYTES));
    Finding {
        rule_id: check.to_string(),
        level: FindingLevel::Error,
        message,
        path: None,
        line: None,
        column: None,
        rule_ref: None,
        effect_landed: None,
        message_source,
    }
}

/// Where a level sorts when a list is over its cap: `error`, then
/// `warning`, then `info`, then any level this koto doesn't know.
fn level_rank(level: &FindingLevel) -> u8 {
    match level {
        FindingLevel::Error => 0,
        FindingLevel::Warning => 1,
        FindingLevel::Info => 2,
        FindingLevel::Other(_) => 3,
    }
}

/// The list a response or check event carries: `parsed` with `fallback`
/// after it, at most `cap` long, and whether a finding was dropped.
///
/// Within the cap the list keeps emission order, the fallback last. Over
/// it, findings are kept by level (errors, then warnings, then info, then
/// any other level), each level in emission order, so an error is never cut
/// in favour of a warning. The fallback is an `error` and is written only
/// when no parsed finding is, so it leads the list; it is kept whenever
/// `cap` is at least 1.
pub fn cap_findings(
    parsed: &[Finding],
    fallback: Option<Finding>,
    cap: usize,
) -> (Vec<Finding>, bool) {
    let mut all: Vec<Finding> = parsed.iter().cloned().chain(fallback).collect();
    if all.len() <= cap {
        return (all, false);
    }
    // Stable, so each level keeps emission order and the fallback, emitted
    // last, sorts after any parsed error.
    all.sort_by_key(|f| level_rank(&f.level));
    all.truncate(cap);
    (all, true)
}

/// Judge what a failed corrective check reported: its parsed findings,
/// returned whole, and the [`GateFailure`] beside them.
///
/// `output` is the command's output for a command gate or `default_action`,
/// and `None` for a context gate. The fallback finding is written when no
/// parsed finding has level `error` -- judged over all of them, not only
/// those a capped list keeps.
pub fn build_failure(
    check: &str,
    output: Option<CheckOutput>,
    sentence: &str,
) -> (Vec<Finding>, GateFailure) {
    let has_error = output
        .as_ref()
        .is_some_and(|o| o.findings.iter().any(|f| f.level == FindingLevel::Error));
    let fallback = (!has_error).then(|| fallback_finding(check, output.as_ref(), sentence));
    match output {
        Some(o) => (
            o.findings,
            GateFailure {
                fallback,
                captured: Some(o.captured),
            },
        ),
        None => (
            Vec::new(),
            GateFailure {
                fallback,
                captured: None,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> RedactedText {
        RedactedText::koto_note(s)
    }

    fn line(json: &str) -> String {
        format!("{FINDING_PREFIX}{json}")
    }

    /// The response's `failure` object for a failed check.
    fn respond(check: &str, output: Option<CheckOutput>, sentence: &str) -> FailureResponse {
        let (parsed, failure) = build_failure(check, output, sentence);
        failure.response(&parsed)
    }

    fn numbered(level: &str, prefix: &str, range: std::ops::Range<usize>) -> String {
        range
            .map(|i| {
                format!(
                    "{}\n",
                    line(&format!(
                        r#"{{"rule_id":"{prefix}{i}","level":"{level}","message":"m"}}"#
                    ))
                )
            })
            .collect()
    }

    #[test]
    fn build_failure_keeps_every_parsed_finding_and_the_fallback_apart() {
        let stdout = numbered("warning", "W", 0..150);
        let (parsed, failure) = build_failure("g", Some(output(&stdout, "boom")), "s");
        assert_eq!(parsed.len(), 150, "the parsed list is never capped");
        assert_eq!(parsed[149].rule_id, "W149");
        let fallback = failure.fallback.as_ref().expect("no error, so a fallback");
        assert_eq!(fallback.rule_id, "g");
        assert_eq!(fallback.message, "boom");
        assert!(parsed
            .iter()
            .all(|f| f.message_source == MessageSource::Check));
        assert!(failure.captured.is_some());

        // The views derive from the two: over the cap, the fallback (the
        // only error) leads, then 99 warnings for the response and 49 for
        // the log.
        let view = failure.response(&parsed);
        assert_eq!(view.findings.len(), RESPONSE_FINDINGS_CAP);
        assert_eq!(view.findings.first(), Some(fallback));
        assert_eq!(view.findings[99].rule_id, "W98");
        assert!(view.findings_truncated);
        let (log, cut) = cap_findings(&parsed, failure.fallback.clone(), LOG_FINDINGS_CAP);
        assert_eq!(log.len(), LOG_FINDINGS_CAP);
        assert_eq!(log.first(), Some(fallback));
        assert_eq!(log[49].rule_id, "W48");
        assert!(cut);
    }

    #[test]
    fn an_error_past_the_cap_suppresses_the_fallback_and_leads_the_view() {
        let stdout = format!(
            "{}{}",
            numbered("warning", "W", 0..100),
            numbered("error", "E", 0..1)
        );
        let (parsed, failure) = build_failure("g", Some(output(&stdout, "")), "s");
        assert_eq!(parsed.len(), 101);
        assert_eq!(parsed[100].rule_id, "E0");
        assert_eq!(parsed[100].level, FindingLevel::Error);
        assert!(failure.fallback.is_none());
        let view = failure.response(&parsed);
        assert_eq!(view.findings.len(), RESPONSE_FINDINGS_CAP);
        assert!(view.findings_truncated);
        assert_eq!(view.findings[0].rule_id, "E0");
        assert_eq!(view.findings[1].rule_id, "W0");
        assert_eq!(view.findings[99].rule_id, "W98");
    }

    #[test]
    fn the_response_view_serializes_as_the_failure_object() {
        let finding = line(r#"{"rule_id":"E1","level":"error","message":"e"}"#);
        let view = respond("g", Some(output(&format!("{finding}\n"), "")), "s");
        let json = serde_json::to_value(&view).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["captured", "findings", "findings_truncated"]);
        assert_eq!(json["findings"][0]["rule_id"], "E1");
    }

    fn output(stdout: &str, stderr: &str) -> CheckOutput {
        CheckOutput::parse(
            Captured {
                stdout: text(stdout),
                stderr: text(stderr),
                stdout_truncated: false,
                stderr_truncated: false,
            },
            false,
            &Redactor::empty(),
        )
    }

    #[test]
    fn a_full_finding_decodes_field_for_field() {
        let f = decode_line(&line(
            r#"{"rule_id":"E501","level":"error","message":"line too long (104 > 88)","path":"src/app.py","line":12,"column":89,"rule_ref":"https://docs.example.org/rules/E501"}"#,
        ))
        .unwrap();
        assert_eq!(f.rule_id, "E501");
        assert_eq!(f.level, FindingLevel::Error);
        assert_eq!(f.message, "line too long (104 > 88)");
        assert_eq!(f.path.as_deref(), Some("src/app.py"));
        assert_eq!(f.line, Some(12));
        assert_eq!(f.column, Some(89));
        assert_eq!(
            f.rule_ref.as_deref(),
            Some("https://docs.example.org/rules/E501")
        );
        assert_eq!(f.effect_landed, None);
        assert_eq!(f.message_source, MessageSource::Check);
    }

    #[test]
    fn null_is_absent_and_unknown_keys_are_ignored() {
        let f = decode_line(&line(
            r#"{"rule_id":"R","level":"info","message":"m","path":null,"line":null,"extra":[1],"effect_landed":true}"#,
        ))
        .unwrap();
        assert_eq!(f.path, None);
        assert_eq!(f.line, None);
        assert_eq!(f.effect_landed, Some(true));
    }

    #[test]
    fn trailing_whitespace_and_one_final_carriage_return_are_allowed() {
        let base = r#"{"rule_id":"R","level":"warning","message":"m"}"#;
        assert!(decode_line(&line(&format!("{base}  \t"))).is_some());
        assert!(decode_line(&line(&format!("{base}\r"))).is_some());
        assert!(decode_line(&line(&format!("{base} \t\r"))).is_some());
        // The carriage return must be the very last byte.
        assert!(decode_line(&line(&format!("{base} \r "))).is_none());
        assert!(decode_line(&line(&format!("{base}\r\t"))).is_none());
        assert!(decode_line(&line(&format!("{base}\r\r"))).is_none());
        assert!(decode_line(&line(&format!("{base} x"))).is_none());
        assert!(decode_line(&line(&format!("{base}{base}"))).is_none());
    }

    #[test]
    fn every_violation_is_ordinary_output() {
        let bad = [
            // prefix not at the line start
            format!(
                " {}",
                line(r#"{"rule_id":"R","level":"error","message":"m"}"#)
            ),
            line(r#" {"rule_id":"R","level":"error","message":"m"}"#),
            line(r#"["rule_id"]"#),
            line(r#"{"rule_id":"R","level":"error","message":"m""#),
            line(r#"{"level":"error","message":"m"}"#),
            line(r#"{"rule_id":null,"level":"error","message":"m"}"#),
            line(r#"{"rule_id":"","level":"error","message":"m"}"#),
            line(r#"{"rule_id":7,"level":"error","message":"m"}"#),
            line(r#"{"rule_id":"R","level":"fatal","message":"m"}"#),
            line(r#"{"rule_id":"R","level":"ERROR","message":"m"}"#),
            line(r#"{"rule_id":"R","level":"error"}"#),
            line(r#"{"rule_id":"R","level":"error","message":1}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","line":3}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","path":"p","column":3}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","path":"p","line":0}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","path":"p","line":1.5}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","path":"p","line":"3"}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","path":5}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","rule_ref":false}"#),
            line(r#"{"rule_id":"R","level":"error","message":"m","effect_landed":"yes"}"#),
        ];
        for l in &bad {
            assert!(decode_line(l).is_none(), "{l}");
        }
    }

    #[test]
    fn the_unterminated_fragment_of_a_cut_stdout_is_never_parsed() {
        let one = line(r#"{"rule_id":"A","level":"error","message":"m"}"#);
        let two = line(r#"{"rule_id":"B","level":"error","message":"m"}"#);
        let stdout = text(&format!("{one}\n{two}"));
        let red = Redactor::empty();
        assert_eq!(parse_findings(&stdout, false, &red).len(), 2);
        let cut = parse_findings(&stdout, true, &red);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].rule_id, "A");
        // A cut that landed on a line end loses nothing.
        let ended = text(&format!("{one}\n{two}\n"));
        assert_eq!(parse_findings(&ended, true, &red).len(), 2);
    }

    #[test]
    fn decoded_strings_are_redacted_again() {
        let value = "sekrit-value-123";
        let red = Redactor::new([("KOTO_T".to_string(), value.to_string())]);
        // `\u0073` spells the leading `s`, which the raw-byte pass can't see.
        let escaped = format!("\\u0073{}", &value[1..]);
        let stdout = text(&format!(
            "{}\n",
            line(&format!(
                r#"{{"rule_id":"R","level":"error","message":"m {escaped}","path":"p/{escaped}","rule_ref":"u/{escaped}"}}"#
            ))
        ));
        let f = &parse_findings(&stdout, false, &red)[0];
        for field in [
            &f.message,
            f.path.as_ref().unwrap(),
            f.rule_ref.as_ref().unwrap(),
        ] {
            assert!(!field.contains(value), "{field}");
            assert!(field.contains("[REDACTED:KOTO_T]"), "{field}");
        }
    }

    #[test]
    fn field_caps_apply_after_redaction_without_splitting_a_marker_or_character() {
        let value = "sekrit-value-123";
        let red = Redactor::new([("KOTO_T".to_string(), value.to_string())]);
        let marker = "[REDACTED:KOTO_T]";
        // rule_id: 120 bytes, then the value; the marker would straddle 128.
        let rule_id = format!("{}{}", "r".repeat(120), value);
        // path: a 2-byte character straddling 512.
        let path = format!("{}é", "p".repeat(511));
        let rule_ref = "u".repeat(600);
        let message = "m".repeat(1200);
        let stdout = text(&format!(
            "{}\n",
            line(
                &serde_json::json!({
                    "rule_id": rule_id, "level": "error", "message": message,
                    "path": path, "rule_ref": rule_ref,
                })
                .to_string()
            )
        ));
        let f = &parse_findings(&stdout, false, &red)[0];
        assert_eq!(f.rule_id, "r".repeat(120), "the marker is dropped whole");
        assert!(!f.rule_id.contains(&marker[..5]));
        assert_eq!(f.path.as_deref(), Some("p".repeat(511).as_str()));
        assert_eq!(f.rule_ref.as_ref().unwrap().len(), RULE_REF_MAX_BYTES);
        assert_eq!(f.message.len(), MESSAGE_MAX_BYTES);

        // A rule id that fits once redacted keeps its whole marker.
        let short = format!("{}{}", "r".repeat(100), value);
        let stdout = text(&format!(
            "{}\n",
            line(
                &serde_json::json!({"rule_id": short, "level": "error", "message": "m"})
                    .to_string()
            )
        ));
        let f = &parse_findings(&stdout, false, &red)[0];
        assert_eq!(f.rule_id, format!("{}{}", "r".repeat(100), marker));
    }

    #[test]
    fn fallback_prefers_stderr_then_stdout_then_the_sentence() {
        let f = respond("lint", Some(output("out-line\n", "err-line\n")), "s");
        assert_eq!(f.findings.len(), 1);
        assert_eq!(f.findings[0].message, "err-line");
        assert_eq!(f.findings[0].message_source, MessageSource::Output);
        assert_eq!(f.findings[0].rule_id, "lint");
        assert_eq!(f.findings[0].level, FindingLevel::Error);

        let f = respond("lint", Some(output("out-line\n\n  \n", " \n")), "s");
        assert_eq!(f.findings[0].message, "out-line");

        let finding = line(r#"{"rule_id":"W1","level":"warning","message":"w"}"#);
        let f = respond(
            "lint",
            Some(output(&format!("plain\n{finding}\n"), "")),
            "command exited with status 1",
        );
        assert_eq!(f.findings.len(), 2, "the warning, then the fallback");
        assert_eq!(f.findings[0].rule_id, "W1");
        assert_eq!(f.findings[1].message, "plain", "finding lines are skipped");

        let f = respond("lint", Some(output("", "")), "command exited with status 1");
        assert_eq!(f.findings[0].message, "command exited with status 1");
        assert_eq!(f.findings[0].message_source, MessageSource::Koto);
    }

    #[test]
    fn a_koto_note_on_stderr_is_koto_sourced() {
        let mut o = output("partial\n", "err\ncommand timed out after 1 seconds");
        o.stderr_ends_with_note = true;
        let f = respond("slow", Some(o), "unused");
        assert_eq!(f.findings[0].message, "command timed out after 1 seconds");
        assert_eq!(f.findings[0].message_source, MessageSource::Koto);
    }

    #[test]
    fn the_truncation_note_is_never_the_message() {
        let f = respond(
            "__action__",
            Some(output(
                "real line\n... [output truncated]",
                "\n... [output truncated]",
            )),
            "s",
        );
        assert_eq!(f.findings[0].message, "real line");
    }

    #[test]
    fn a_long_line_folds_to_500_characters() {
        let f = respond("g", Some(output("", &"x".repeat(600))), "s");
        let m = &f.findings[0].message;
        assert_eq!(m.chars().count(), 500);
        assert!(m.ends_with("..."));
    }

    #[test]
    fn the_500_character_fold_never_splits_a_marker() {
        let line = format!("{}[REDACTED:GH_TOKEN] tail", "x".repeat(490));
        let f = respond("g", Some(output("", &line)), "s");
        assert_eq!(f.findings[0].message, format!("{}...", "x".repeat(490)));
    }

    #[test]
    fn an_error_finding_means_no_fallback() {
        let finding = line(r#"{"rule_id":"E1","level":"error","message":"e"}"#);
        let f = respond("g", Some(output(&format!("{finding}\n"), "boom")), "s");
        assert_eq!(f.findings.len(), 1);
        assert_eq!(f.findings[0].rule_id, "E1");
        assert!(!f.findings_truncated);
    }

    #[test]
    fn the_response_keeps_100_with_the_fallback_first_when_over_the_cap() {
        let warn = |i: usize| {
            line(&format!(
                r#"{{"rule_id":"W{i}","level":"warning","message":"w"}}"#
            ))
        };
        let stdout: String = (0..150).map(|i| format!("{}\n", warn(i))).collect();
        let f = respond("g", Some(output(&stdout, "boom")), "s");
        assert_eq!(f.findings.len(), RESPONSE_FINDINGS_CAP);
        assert!(f.findings_truncated);
        assert_eq!(f.findings[0].rule_id, "g");
        assert_eq!(f.findings[1].rule_id, "W0");
        assert_eq!(f.findings[99].rule_id, "W98");

        let err = |i: usize| {
            line(&format!(
                r#"{{"rule_id":"E{i}","level":"error","message":"e"}}"#
            ))
        };
        let stdout: String = (0..150).map(|i| format!("{}\n", err(i))).collect();
        let f = respond("g", Some(output(&stdout, "")), "s");
        assert_eq!(f.findings.len(), RESPONSE_FINDINGS_CAP);
        assert_eq!(f.findings[99].rule_id, "E99");
        assert!(f.findings_truncated);

        let (kept, cut) = cap_findings(&[], None, LOG_FINDINGS_CAP);
        assert!(kept.is_empty() && !cut);
    }

    fn at(level: FindingLevel, rule_id: &str) -> Finding {
        Finding {
            rule_id: rule_id.to_string(),
            level,
            message: "m".to_string(),
            path: None,
            line: None,
            column: None,
            rule_ref: None,
            effect_landed: None,
            message_source: MessageSource::Check,
        }
    }

    fn ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule_id.as_str()).collect()
    }

    #[test]
    fn over_the_cap_findings_are_kept_by_level_each_in_emission_order() {
        let parsed = vec![
            at(FindingLevel::Info, "I0"),
            at(FindingLevel::Warning, "W0"),
            at(FindingLevel::Error, "E0"),
            at(FindingLevel::Info, "I1"),
            at(FindingLevel::Warning, "W1"),
            at(FindingLevel::Error, "E1"),
            at(FindingLevel::Warning, "W2"),
        ];
        let (kept, cut) = cap_findings(&parsed, None, 5);
        assert_eq!(ids(&kept), ["E0", "E1", "W0", "W1", "W2"]);
        assert!(cut);
        let (kept, cut) = cap_findings(&parsed, None, 1);
        assert_eq!(ids(&kept), ["E0"]);
        assert!(cut);
    }

    #[test]
    fn within_the_cap_emission_order_is_kept_with_the_fallback_last() {
        let parsed = vec![
            at(FindingLevel::Info, "I0"),
            at(FindingLevel::Warning, "W0"),
        ];
        let fallback = at(FindingLevel::Error, "g");
        let (kept, cut) = cap_findings(&parsed, Some(fallback.clone()), 3);
        assert_eq!(ids(&kept), ["I0", "W0", "g"]);
        assert!(!cut);
        let (kept, cut) = cap_findings(&parsed, None, 2);
        assert_eq!(ids(&kept), ["I0", "W0"]);
        assert!(!cut);
    }

    #[test]
    fn over_the_cap_the_fallback_is_kept_ahead_of_warnings() {
        let parsed: Vec<Finding> = (0..5)
            .map(|i| at(FindingLevel::Warning, &format!("W{i}")))
            .chain([at(FindingLevel::Info, "I0")])
            .collect();
        let fallback = at(FindingLevel::Error, "g");
        let (kept, cut) = cap_findings(&parsed, Some(fallback), 3);
        assert_eq!(ids(&kept), ["g", "W0", "W1"]);
        assert!(cut);
    }

    #[test]
    fn over_the_cap_an_unknown_level_sorts_after_info() {
        let parsed = vec![
            at(FindingLevel::Other("notice".to_string()), "N0"),
            at(FindingLevel::Info, "I0"),
            at(FindingLevel::Other("hint".to_string()), "N1"),
            at(FindingLevel::Warning, "W0"),
        ];
        let (kept, cut) = cap_findings(&parsed, None, 3);
        assert_eq!(ids(&kept), ["W0", "I0", "N0"]);
        assert!(cut);
        let (kept, _) = cap_findings(&parsed, None, 4);
        assert_eq!(
            ids(&kept),
            ["N0", "I0", "N1", "W0"],
            "within the cap, unchanged"
        );
    }

    #[test]
    fn a_context_failure_has_no_captured_output() {
        let f = respond("ctx", None, "context key 'k' is not set");
        assert!(f.captured.is_none());
        assert_eq!(f.findings[0].message, "context key 'k' is not set");
        let json = serde_json::to_value(&f).unwrap();
        assert!(json.get("captured").is_none());
        assert_eq!(json["findings_truncated"], false);
    }

    #[test]
    fn a_finding_round_trips_through_json() {
        let finding = Finding {
            rule_id: "E501".to_string(),
            level: FindingLevel::Warning,
            message: "line too long".to_string(),
            path: Some("src/app.py".to_string()),
            line: Some(12),
            column: Some(89),
            rule_ref: Some("https://docs.example.org/rules/E501".to_string()),
            effect_landed: Some(false),
            message_source: MessageSource::Output,
        };
        let json = serde_json::to_value(&finding).unwrap();
        assert_eq!(json["level"], "warning");
        assert_eq!(json["message_source"], "output");
        let back: Finding = serde_json::from_value(json).unwrap();
        assert_eq!(back, finding);

        // Absent optional fields stay absent.
        let bare = fallback_finding("g", None, "s");
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("path").is_none());
        let back: Finding = serde_json::from_value(json).unwrap();
        assert_eq!(back, bare);
    }

    #[test]
    fn an_unknown_level_or_source_reads_and_writes_back_unchanged() {
        let json = serde_json::json!({
            "rule_id": "R", "level": "critical", "message": "m",
            "message_source": "plugin",
        });
        let f: Finding = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(f.level, FindingLevel::Other("critical".to_string()));
        assert_eq!(f.message_source, MessageSource::Other("plugin".to_string()));
        assert_eq!(serde_json::to_value(&f).unwrap(), json);

        // The finding-line grammar stays closed: a check can't print one.
        assert!(
            decode_line(&line(r#"{"rule_id":"R","level":"critical","message":"m"}"#)).is_none()
        );
    }

    #[test]
    fn a_multibyte_fallback_is_held_to_the_byte_cap() {
        // 497 three-byte characters plus `...` is 1,494 bytes; the byte cap
        // keeps the 333 whole characters that fit in 1,000.
        let f = respond("g", Some(output("", &"€".repeat(600))), "s");
        let m = &f.findings[0].message;
        assert!(m.len() <= MESSAGE_MAX_BYTES);
        assert_eq!(m.chars().count(), MESSAGE_MAX_BYTES / 3);
        assert!(
            m.chars().all(|c| c == '€'),
            "the `...` is past the byte cap"
        );
    }

    #[test]
    fn effect_landed_fills_only_unset_findings() {
        let stated = line(r#"{"rule_id":"A","level":"error","message":"m","effect_landed":true}"#);
        let unstated = line(r#"{"rule_id":"B","level":"error","message":"m"}"#);
        let mut o = output(&format!("{stated}\n{unstated}\n"), "");
        fill_effect_landed(o.findings.iter_mut(), false);
        assert_eq!(o.findings[0].effect_landed, Some(true));
        assert_eq!(o.findings[1].effect_landed, Some(false));
    }
}
