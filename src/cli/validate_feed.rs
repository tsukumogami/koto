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
    let spec_content = std::fs::read_to_string(spec_path)
        .with_context(|| format!("failed to read feed spec from {}", spec_path.display()))?;

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
}
