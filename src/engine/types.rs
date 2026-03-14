use serde::{Deserialize, Serialize};

/// A single event appended to the JSONL state log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Event type (e.g. "init", "rewind").
    #[serde(rename = "type")]
    pub event_type: String,

    /// The workflow state name after this event.
    pub state: String,

    /// ISO 8601 timestamp of when this event was recorded.
    pub timestamp: String,

    /// Path to the compiled template. Present on "init" events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// SHA256 hash of the compiled template. Present on "init" events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_hash: Option<String>,
}

/// Derived current state of a workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineState {
    /// The name of the current state (last event's `state` field).
    pub current_state: String,

    /// Path to the compiled template (from the "init" event).
    pub template_path: String,

    /// SHA256 hash of the compiled template (from the "init" event).
    pub template_hash: String,
}

/// Return the current UTC time as an ISO 8601 string.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Format as YYYY-MM-DDTHH:MM:SSZ using integer arithmetic.
    let s = secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400; // days since 1970-01-01

    // Compute year/month/day from days-since-epoch.
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian calendar computation.
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: &[u64] = if leap {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_type_field() {
        let e = Event {
            event_type: "init".to_string(),
            state: "gather".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            template: Some("path/to/template.json".to_string()),
            template_hash: Some("abc123".to_string()),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"init\""));
        assert!(json.contains("\"state\":\"gather\""));
        assert!(json.contains("\"template\":\"path/to/template.json\""));
        assert!(json.contains("\"template_hash\":\"abc123\""));
    }

    #[test]
    fn event_deserializes_type_field() {
        let json = r#"{"type":"rewind","state":"gather","timestamp":"2026-01-01T00:00:00Z"}"#;
        let e: Event = serde_json::from_str(json).unwrap();
        assert_eq!(e.event_type, "rewind");
        assert_eq!(e.state, "gather");
        assert!(e.template.is_none());
        assert!(e.template_hash.is_none());
    }

    #[test]
    fn event_omits_optional_fields_when_none() {
        let e = Event {
            event_type: "rewind".to_string(),
            state: "gather".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            template: None,
            template_hash: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("template"));
    }

    #[test]
    fn machine_state_roundtrip() {
        let ms = MachineState {
            current_state: "plan".to_string(),
            template_path: "/cache/abc.json".to_string(),
            template_hash: "abc123".to_string(),
        };
        let json = serde_json::to_string(&ms).unwrap();
        let ms2: MachineState = serde_json::from_str(&json).unwrap();
        assert_eq!(ms2.current_state, "plan");
        assert_eq!(ms2.template_hash, "abc123");
    }

    #[test]
    fn now_iso8601_format() {
        let ts = now_iso8601();
        // Basic sanity: correct length and ends with Z
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
    }
}
