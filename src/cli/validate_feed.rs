use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use serde_json::Value;

use crate::template::split_frontmatter;

#[derive(Debug, Deserialize)]
struct FeedSpec {
    events: HashMap<String, EventSpec>,
}

#[derive(Debug, Deserialize)]
struct EventSpec {
    #[serde(default)]
    fields: HashMap<String, FieldSpec>,
}

#[derive(Debug, Deserialize)]
struct FieldSpec {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    nullable: bool,
    #[serde(rename = "enum", default)]
    enum_values: Option<Vec<String>>,
}

/// Resolve the path to `docs/reference/session-feed.md`.
///
/// Checks `KOTO_FEED_SPEC` first (for tests and CI overrides), then falls
/// back to `docs/reference/session-feed.md` relative to the current
/// working directory.
pub(crate) fn locate_feed_spec() -> anyhow::Result<PathBuf> {
    if let Ok(override_path) = std::env::var("KOTO_FEED_SPEC") {
        return Ok(PathBuf::from(override_path));
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd.join("docs/reference/session-feed.md"))
}

/// Validate a JSONL session log against the session-feed spec.
///
/// Returns `Ok(())` when the log is valid. Returns an error listing all
/// validation failures when any are found. Prints each failure to stderr
/// before returning.
pub(crate) fn validate_feed(log_file: &str) -> anyhow::Result<()> {
    let spec_path = locate_feed_spec()?;
    validate_feed_with_spec(log_file, &spec_path)
}

/// Core validation logic; accepts an explicit spec path for testability.
pub(crate) fn validate_feed_with_spec(log_file: &str, spec_path: &Path) -> anyhow::Result<()> {
    let spec_content = std::fs::read_to_string(spec_path).with_context(|| {
        format!(
            "failed to read feed spec from {}; set KOTO_FEED_SPEC to override the path",
            spec_path.display()
        )
    })?;

    let (frontmatter_str, _body) = split_frontmatter(&spec_content)
        .ok_or_else(|| anyhow!("feed spec has no YAML frontmatter"))?;

    let spec: FeedSpec = serde_yaml_ng::from_str(frontmatter_str)
        .context("failed to parse feed spec frontmatter")?;

    let log_content = std::fs::read_to_string(log_file)
        .with_context(|| format!("failed to read log file: {}", log_file))?;

    let mut errors: Vec<String> = Vec::new();

    for (line_idx, line) in log_content.lines().enumerate() {
        if line_idx == 0 {
            continue; // header line
        }
        let line_num = line_idx + 1;
        if line.trim().is_empty() {
            continue;
        }

        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("line {}: invalid JSON: {}", line_num, e));
                continue;
            }
        };

        let type_str = match event.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => {
                errors.push(format!("line {}: missing 'type' field", line_num));
                continue;
            }
        };

        let event_spec = match spec.events.get(type_str) {
            Some(s) => s,
            None => continue, // unknown event type: skip (forward-compatible)
        };

        let payload = match event.get("payload") {
            Some(p) => p,
            None => {
                errors.push(format!(
                    "line {} ({}): missing 'payload' field",
                    line_num, type_str
                ));
                continue;
            }
        };

        validate_fields(line_num, type_str, payload, &event_spec.fields, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for e in &errors {
            eprintln!("{}", e);
        }
        Err(anyhow!("{} validation error(s) found", errors.len()))
    }
}

fn validate_fields(
    line_num: usize,
    event_type: &str,
    payload: &Value,
    fields: &HashMap<String, FieldSpec>,
    errors: &mut Vec<String>,
) {
    for (field_name, spec) in fields {
        let value = payload.get(field_name);

        if spec.required {
            match value {
                None => {
                    errors.push(format!(
                        "line {} ({}): required field '{}' is missing",
                        line_num, event_type, field_name
                    ));
                    continue;
                }
                Some(Value::Null) if !spec.nullable => {
                    errors.push(format!(
                        "line {} ({}): field '{}' is null but not nullable",
                        line_num, event_type, field_name
                    ));
                    continue;
                }
                _ => {}
            }
        }

        let value = match value {
            Some(v) if !v.is_null() => v,
            _ => continue, // absent or null: skip further checks
        };

        let type_ok = match spec.type_name.as_str() {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "any" => true,
            _ => true, // unknown vocabulary type: skip
        };

        if !type_ok {
            errors.push(format!(
                "line {} ({}): field '{}' expected type '{}', got {}",
                line_num,
                event_type,
                field_name,
                spec.type_name,
                json_type_name(value)
            ));
        }

        if let (Some(enum_vals), Some(s)) = (&spec.enum_values, value.as_str()) {
            if !enum_vals.iter().any(|v| v == s) {
                errors.push(format!(
                    "line {} ({}): field '{}' value {:?} not in enum {:?}",
                    line_num, event_type, field_name, s, enum_vals
                ));
            }
        }
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "integer",
        Value::Bool(_) => "boolean",
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::Null => "null",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::NamedTempFile;

    use super::*;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn minimal_spec() -> NamedTempFile {
        write_temp(
            r#"---
events:
  test_event:
    tier: 1
    fields:
      name:
        type: string
        required: true
      count:
        type: integer
        required: false
      mode:
        type: string
        required: false
        enum: ["fast", "slow"]
      data:
        type: object
        required: false
        nullable: true
---
Body text.
"#,
        )
    }

    fn header_line() -> &'static str {
        r#"{"schema_version":1,"workflow":"w","template_hash":"h","created_at":"2024-01-01T00:00:00Z"}"#
    }

    #[test]
    fn valid_log_passes() {
        let spec = minimal_spec();
        let log = write_temp(&format!(
            "{}\n{}\n",
            header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"test_event","payload":{"name":"hello"}}"#
        ));
        assert!(validate_feed_with_spec(log.path().to_str().unwrap(), spec.path()).is_ok());
    }

    #[test]
    fn required_field_absent_triggers_error() {
        let spec = minimal_spec();
        let log = write_temp(&format!(
            "{}\n{}\n",
            header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"test_event","payload":{"count":1}}"#
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), spec.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("validation error"), "got: {}", msg);
    }

    #[test]
    fn wrong_field_type_triggers_error() {
        let spec = minimal_spec();
        let log = write_temp(&format!(
            "{}\n{}\n",
            header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"test_event","payload":{"name":42}}"#
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), spec.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("validation error"), "got: {}", msg);
    }

    #[test]
    fn unknown_event_type_is_skipped() {
        let spec = minimal_spec();
        let log = write_temp(&format!(
            "{}\n{}\n",
            header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"future_event","payload":{"anything":true}}"#
        ));
        assert!(validate_feed_with_spec(log.path().to_str().unwrap(), spec.path()).is_ok());
    }

    #[test]
    fn enum_field_invalid_value_triggers_error() {
        let spec = minimal_spec();
        let log = write_temp(&format!(
            "{}\n{}\n",
            header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"test_event","payload":{"name":"ok","mode":"turbo"}}"#
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), spec.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("validation error"), "got: {}", msg);
    }

    // -- The shipped contract --
    //
    // The tests above run against a hand-written spec, which proves the
    // validator works but says nothing about what koto actually ships. These
    // run against `docs/reference/session-feed.md` itself, so an event the
    // engine emits but the contract never declares fails here.

    fn shipped_spec() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/session-feed.md")
    }

    /// A header carrying `execution_dir`.
    fn anchored_header_line() -> &'static str {
        r#"{"schema_version":1,"workflow":"w","template_hash":"h","created_at":"2024-01-01T00:00:00Z","execution_dir":"/home/user/src/koto"}"#
    }

    /// A log holding everything the command-running work added -- the anchored
    /// header, the anchoring events, and an action event carrying `truncated`
    /// -- validates clean against the contract koto ships. The rebind appears
    /// twice: once with the anchor it moved away from, once without, which is
    /// what a session that had no anchor to move away from writes.
    #[test]
    fn shipped_spec_accepts_the_command_running_events() {
        let log = write_temp(&format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            anchored_header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"execution_anchor_adopted","payload":{"anchor":"/home/user/src/koto"}}"#,
            r#"{"seq":4,"timestamp":"2024-01-01T00:00:03Z","type":"execution_anchor_rebound","payload":{"from":"/home/user/src/koto","to":"/home/user/work/koto"}}"#,
            r#"{"seq":5,"timestamp":"2024-01-01T00:00:04Z","type":"execution_anchor_rebound","payload":{"to":"/home/user/work/koto"}}"#,
            r#"{"seq":2,"timestamp":"2024-01-01T00:00:01Z","type":"default_action_executed","payload":{"state":"detect","command":"git branch --show-current","exit_code":0,"stdout":"main","stderr":"","truncated":true}}"#,
            r#"{"seq":3,"timestamp":"2024-01-01T00:00:02Z","type":"variable_captured","payload":{"key":"BRANCH","value":"main"}}"#,
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());

        // The decider's event, in each shape the engine writes: applied with
        // every key, and an error with the optional keys absent or null.
        let log = write_temp(&format!(
            "{}\n{}\n{}\n{}\n",
            anchored_header_line(),
            DECIDER_CONSULTED,
            r#"{"seq":2,"timestamp":"2024-01-01T00:00:01Z","type":"decider_consulted","payload":{"state":"review","visit_seq":1,"provider":"jev","model":"unknown","outcome":"error","error_class":"some_future_class","latency_ms":2000,"directive_bytes":10,"endpoint_origin":"env","fields":{}}}"#,
            r#"{"seq":3,"timestamp":"2024-01-01T00:00:02Z","type":"decider_consulted","payload":{"state":"review","visit_seq":1,"provider":"jev","model":"unknown","input_sha256":null,"outcome":"input_unavailable","error_class":null,"latency_ms":0,"directive_bytes":10,"fields":{}}}"#,
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());
    }

    /// A `variables_rebound` event, as the rebind primitive writes it,
    /// validates clean against the shipped contract.
    #[test]
    fn shipped_spec_accepts_variables_rebound() {
        let log = write_temp(&format!(
            "{}\n{}\n",
            anchored_header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"variables_rebound","payload":{"variables":{"MERGE":"true"}}}"#,
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());
    }

    /// An anchor's log, as `koto session anchor open` and a held tick write
    /// it, validates clean against the shipped contract.
    #[test]
    fn shipped_spec_accepts_an_anchor_log() {
        let log = write_temp(&format!(
            "{}\n{}\n{}\n",
            r#"{"schema_version":1,"workflow":"anchor-h1","template_hash":"h","created_at":"2024-01-01T00:00:00Z","kind":"anchor"}"#,
            r#"{"seq":3,"timestamp":"2024-01-01T00:00:00Z","type":"anchor_started","payload":{"source":"clear","harness_session":"h1","opened":"successor","process":"ab","predecessor":"anchor-h0","reason":"clear"}}"#,
            r#"{"seq":4,"timestamp":"2024-01-01T00:00:01Z","type":"anchor_activity","payload":{"session":"wf","command":"next"}}"#,
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());
    }

    /// Acceptance alone would prove nothing: the validator skips event types it
    /// does not recognize, so an unregistered event passes silently. Feeding a
    /// malformed instance of each new event is what shows the contract declares
    /// it and the validator is checking its fields.
    #[test]
    fn shipped_spec_declares_the_new_events_rather_than_skipping_them() {
        for (event, malformed) in [
            (
                "execution_anchor_adopted",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"execution_anchor_adopted","payload":{}}"#,
            ),
            (
                "variable_captured",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"variable_captured","payload":{"key":"BRANCH"}}"#,
            ),
            (
                "variables_rebound",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"variables_rebound","payload":{}}"#,
            ),
            (
                "execution_anchor_rebound",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"execution_anchor_rebound","payload":{"from":"/home/user/src/koto"}}"#,
            ),
            (
                "decider_consulted",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"decider_consulted","payload":{"state":"review"}}"#,
            ),
            (
                "decider_fallback",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"decider_fallback","payload":{"state":"review"}}"#,
            ),
            (
                "decider_check_answered",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"decider_check_answered","payload":{"state":"review"}}"#,
            ),
            (
                "finding_disputed",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"finding_disputed","payload":{"state":"review"}}"#,
            ),
            (
                "anchor_started",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"anchor_started","payload":{"source":"startup"}}"#,
            ),
            (
                "anchor_activity",
                r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"anchor_activity","payload":{"session":"wf"}}"#,
            ),
        ] {
            let log = write_temp(&format!("{}\n{}\n", anchored_header_line(), malformed));
            let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
            assert!(
                result.is_err(),
                "{} is not declared in the shipped spec: a payload missing its \
                 required fields validated clean, which is the unknown-type skip, \
                 not acceptance",
                event
            );
        }
    }

    /// The four `koto review record` events: a well-formed line of each
    /// validates clean, and one missing a required field or carrying a value
    /// outside its closed set is rejected, so the contract declares them
    /// rather than skipping them as unknown.
    #[test]
    fn shipped_spec_declares_the_review_events() {
        let line = |event: &str, payload: Value| {
            serde_json::json!({
                "seq": 1,
                "timestamp": "2024-01-01T00:00:00Z",
                "type": event,
                "payload": payload,
            })
            .to_string()
        };
        let good = [
            line(
                "review_round_started",
                serde_json::json!({"doc_type": "prd", "schema": "prd/v1", "round": 1, "seat_count": 3, "fallback": false}),
            ),
            line(
                "review_verdict_recorded",
                serde_json::json!({"doc_type": "plan", "schema": "plan/v2", "round": 2, "seat": "other", "rule_id": "rs-001", "section": "goals", "verdict": "fail"}),
            ),
            line(
                "review_decision_recorded",
                serde_json::json!({"doc_type": "design", "schema": "absent", "round": 1, "decision": "fix_and_proceed"}),
            ),
            line(
                "doc_check_recorded",
                serde_json::json!({"doc_type": "brief", "schema": "other", "rule_id": "r1", "outcome": "error"}),
            ),
            line(
                "review_round_started",
                serde_json::json!({"doc_format": "vision", "schema": "absent", "round": 1, "seat_count": 3, "fallback": false}),
            ),
            line(
                "doc_rule_checks_recorded",
                serde_json::json!({"state": "draft", "doc_format": "roadmap", "moment": "pre_jury", "entries": [{"rule_id": "r1", "outcome": "not_ready"}]}),
            ),
            line(
                "doc_section_recorded",
                serde_json::json!({"state": "draft", "doc_format": "prd", "section": "other", "write": "first"}),
            ),
            line(
                "doc_fix_recorded",
                serde_json::json!({"state": "draft", "doc_format": "design", "answers": "jury_fail", "round": 2, "entries": [{"rule_id": "rs-1", "section": "goals", "check_rule": "none"}]}),
            ),
        ];
        let log = write_temp(&format!(
            "{}\n{}\n",
            anchored_header_line(),
            good.join("\n")
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());

        for bad in [
            line(
                "review_round_started",
                serde_json::json!({"doc_type": "prd"}),
            ),
            line(
                "review_verdict_recorded",
                serde_json::json!({"doc_type": "plan", "schema": "plan/v2", "round": 2, "seat": "other", "rule_id": "rs-001", "section": "goals", "verdict": "PASS"}),
            ),
            line(
                "review_decision_recorded",
                serde_json::json!({"doc_type": "design", "schema": "design/v9", "round": 1, "decision": "proceed"}),
            ),
            line(
                "doc_check_recorded",
                serde_json::json!({"doc_type": "brief", "schema": "other", "rule_id": "r1", "outcome": "timed_out"}),
            ),
            line(
                "review_round_started",
                serde_json::json!({"doc_format": "memo", "schema": "absent", "round": 1, "seat_count": 3, "fallback": false}),
            ),
            line(
                "doc_rule_checks_recorded",
                serde_json::json!({"state": "draft", "doc_format": "prd", "moment": "later", "entries": []}),
            ),
            line(
                "doc_section_recorded",
                serde_json::json!({"state": "draft", "doc_format": "prd", "section": "other", "write": "append"}),
            ),
            line(
                "doc_fix_recorded",
                serde_json::json!({"state": "draft", "doc_format": "design", "answers": "jury_fail"}),
            ),
        ] {
            let log = write_temp(&format!("{}\n{}\n", anchored_header_line(), bad));
            let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
            assert!(result.is_err(), "accepted: {bad}");
        }
    }

    /// A well-formed `decider_consulted` line, as a consulting run writes it.
    const DECIDER_CONSULTED: &str = r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"decider_consulted","payload":{"state":"review","visit_seq":1,"provider":"jev","model":"jev-1","input_sha256":"ab","outcome":"applied","latency_ms":12,"directive_bytes":300,"endpoint_origin":"default","fields":{"verdict":{"declaration_hash":"cd","modes":{"proceed":"auto"},"probabilities":{"proceed":0.95,"unclear":0.05},"winning":"proceed","confidence":0.95,"threshold":0.9,"at_threshold":true,"outcome":"qualified"}}}}"#;

    /// Each required key and the closed `outcome` set are checked, not just
    /// the event's presence.
    #[test]
    fn shipped_spec_rejects_a_decider_consulted_without_visit_seq_or_with_a_bogus_outcome() {
        let base: Value = serde_json::from_str(DECIDER_CONSULTED).unwrap();
        let mut missing = base.clone();
        missing["payload"]
            .as_object_mut()
            .unwrap()
            .remove("visit_seq");
        let mut bogus = base.clone();
        bogus["payload"]["outcome"] = Value::from("bogus");
        for (name, line) in [("missing visit_seq", missing), ("bogus outcome", bogus)] {
            let log = write_temp(&format!("{}\n{}\n", anchored_header_line(), line));
            let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
            assert!(result.is_err(), "{} validated clean", name);
        }
    }

    /// `truncated` is pinned as a boolean, not merely tolerated as an unknown
    /// key. The action event's fields are listed individually, so a field the
    /// engine writes and the contract omits is a break.
    #[test]
    fn shipped_spec_pins_the_action_events_truncated_field() {
        let log = write_temp(&format!(
            "{}\n{}\n",
            anchored_header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"default_action_executed","payload":{"state":"lint","command":"c","exit_code":0,"stdout":"","stderr":"","truncated":"yes"}}"#
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("validation error"), "got: {}", msg);
    }

    /// `truncated` is optional: events written before the field existed omit it
    /// and must still validate (R24).
    #[test]
    fn shipped_spec_accepts_an_action_event_without_truncated() {
        let log = write_temp(&format!(
            "{}\n{}\n",
            anchored_header_line(),
            r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"default_action_executed","payload":{"state":"lint","command":"c","exit_code":0,"stdout":"","stderr":""}}"#
        ));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());
    }

    // -- Failure reporting (DESIGN-koto-failure-reporting.md, Gate-event schema) --

    /// Check events carrying every failure-reporting field, the four gate
    /// outcomes, and context events whose `reader`, `writer` and `access`
    /// hold values no koto writes yet: open vocabularies have no `enum`, so
    /// a later value validates.
    #[test]
    fn shipped_spec_accepts_the_failure_reporting_fields_and_open_vocabularies() {
        let finding = r#"{"rule_id":"E501","level":"notice","message":"m","effect_landed":true,"message_source":"linter"}"#;
        let check = format!(
            r#""attempt":2,"visit_attempt":1,"findings":[{finding}],"findings_truncated":true,"rule_counts":{{"E501":{{"visit":1,"session":2}}}},"rule_counts_truncated":true,"duration_ms":12"#
        );
        let mut lines = vec![anchored_header_line().to_string()];
        for (seq, outcome) in ["passed", "failed", "timed_out", "error"]
            .iter()
            .enumerate()
        {
            lines.push(format!(
                r#"{{"seq":{},"timestamp":"t","type":"gate_evaluated","payload":{{"state":"s","gate":"g","output":{{}},"outcome":"{outcome}","timestamp":"t",{check},"stdout":"o","stderr":"","stdout_truncated":true,"stderr_truncated":false}}}}"#,
                seq + 1
            ));
        }
        lines.push(format!(
            r#"{{"seq":5,"timestamp":"t","type":"default_action_executed","payload":{{"state":"s","command":"c","exit_code":0,"stdout":"","stderr":"","truncated":false,{check}}}}}"#
        ));
        lines.push(r#"{"seq":6,"timestamp":"t","type":"context_added","payload":{"key":"k","hash":"h","size":1,"writer":"a_later_writer"}}"#.to_string());
        lines.push(r#"{"seq":7,"timestamp":"t","type":"context_removed","payload":{"key":"k","writer":"a_later_writer"}}"#.to_string());
        lines.push(r#"{"seq":8,"timestamp":"t","type":"context_read","payload":{"key":"k","reader":"a_later_reader","state":"s","present":true,"hash":"h","access":"a_later_access","gate":"g"}}"#.to_string());
        lines.push(r#"{"seq":9,"timestamp":"t","type":"context_read","payload":{"key":"k","reader":"cli","state":"s","present":false}}"#.to_string());
        let log = write_temp(&(lines.join("\n") + "\n"));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());
    }

    /// `context_read` is declared, not skipped as unknown, and its required
    /// fields and the check events' new field types are enforced.
    #[test]
    fn shipped_spec_rejects_a_context_read_without_key_and_a_string_attempt() {
        for bad in [
            r#"{"seq":1,"timestamp":"t","type":"context_read","payload":{"reader":"cli","state":"s","present":false}}"#,
            r#"{"seq":1,"timestamp":"t","type":"gate_evaluated","payload":{"state":"s","gate":"g","output":{},"outcome":"failed","timestamp":"t","attempt":"2"}}"#,
            r#"{"seq":1,"timestamp":"t","type":"default_action_executed","payload":{"state":"s","command":"c","exit_code":1,"stdout":"","stderr":"","rule_counts":[]}}"#,
            r#"{"seq":1,"timestamp":"t","type":"gate_evaluated","payload":{"state":"s","gate":"g","output":{},"outcome":"skipped","timestamp":"t"}}"#,
        ] {
            let log = write_temp(&format!("{}\n{}\n", anchored_header_line(), bad));
            let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
            assert!(result.is_err(), "shipped spec accepted: {bad}");
        }
    }

    /// A reader's anchor log: one `context_read` carrying the optional
    /// `session` field naming the session read, and one without it, both
    /// accepted; a `session` that isn't a string is refused, so the field is
    /// declared rather than skipped as unknown.
    #[test]
    fn shipped_spec_accepts_a_context_read_with_and_without_session() {
        let lines = [
            anchored_header_line(),
            r#"{"seq":1,"timestamp":"t","type":"context_read","payload":{"key":"k","reader":"cli","state":"s","present":true,"hash":"h","access":"content","session":"wf-b"}}"#,
            r#"{"seq":2,"timestamp":"t","type":"context_read","payload":{"key":"k","reader":"cli","state":"s","present":false,"access":"presence"}}"#,
        ];
        let log = write_temp(&(lines.join("\n") + "\n"));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_ok(), "shipped spec rejected: {:?}", result.err());

        let bad = r#"{"seq":1,"timestamp":"t","type":"context_read","payload":{"key":"k","reader":"cli","state":"s","present":false,"session":7}}"#;
        let log = write_temp(&format!("{}\n{}\n", anchored_header_line(), bad));
        let result = validate_feed_with_spec(log.path().to_str().unwrap(), &shipped_spec());
        assert!(result.is_err(), "shipped spec accepted: {bad}");
    }
}
