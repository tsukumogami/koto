use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Header line written as the first line of a state file.
///
/// Contains metadata about the workflow log. Has no `seq` field -- it is
/// not an event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateFileHeader {
    /// Format version; currently `1`.
    pub schema_version: u32,

    /// Workflow name; must match the state filename.
    pub workflow: String,

    /// SHA-256 hex of the compiled template JSON.
    pub template_hash: String,

    /// RFC 3339 UTC timestamp of workflow creation.
    pub created_at: String,
}

/// Type-specific payload for each event variant.
///
/// Each variant's inner fields are serialized directly as the `payload`
/// object. The discriminant is carried by `Event.event_type`, not by
/// serde's enum tagging.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum EventPayload {
    WorkflowInitialized {
        template_path: String,
        #[serde(default)]
        variables: HashMap<String, serde_json::Value>,
    },
    Transitioned {
        from: Option<String>,
        to: String,
        condition_type: String,
    },
    EvidenceSubmitted {
        state: String,
        fields: HashMap<String, serde_json::Value>,
    },
    IntegrationInvoked {
        state: String,
        integration: String,
        output: serde_json::Value,
    },
    DirectedTransition {
        from: String,
        to: String,
    },
    Rewound {
        from: String,
        to: String,
    },
}

impl EventPayload {
    /// Return the string name matching the serialized `type` field.
    pub fn type_name(&self) -> &'static str {
        match self {
            EventPayload::WorkflowInitialized { .. } => "workflow_initialized",
            EventPayload::Transitioned { .. } => "transitioned",
            EventPayload::EvidenceSubmitted { .. } => "evidence_submitted",
            EventPayload::DirectedTransition { .. } => "directed_transition",
            EventPayload::IntegrationInvoked { .. } => "integration_invoked",
            EventPayload::Rewound { .. } => "rewound",
        }
    }
}

/// A single event appended to the JSONL state log.
///
/// The `type` field serializes as a string matching the payload variant
/// name (e.g., "workflow_initialized", "transitioned", "rewound").
/// The `payload` field contains variant-specific data.
///
/// Custom Serialize/Deserialize: on serialization, `event_type` is set
/// from the payload variant name. On deserialization, the `type` field
/// drives which `EventPayload` variant to decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic sequence number starting at 1.
    pub seq: u64,

    /// RFC 3339 UTC timestamp of when this event was recorded.
    pub timestamp: String,

    /// Event type string (e.g., "workflow_initialized", "transitioned").
    pub event_type: String,

    /// Type-specific payload.
    pub payload: EventPayload,
}

impl Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("seq", &self.seq)?;
        map.serialize_entry("timestamp", &self.timestamp)?;
        map.serialize_entry("type", &self.payload.type_name())?;
        map.serialize_entry("payload", &self.payload)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: serde_json::Value = Deserialize::deserialize(deserializer)?;
        let obj = raw
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("event must be a JSON object"))?;

        let seq = obj
            .get("seq")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| serde::de::Error::custom("missing or invalid seq field"))?;

        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("missing timestamp field"))?
            .to_string();

        let event_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("missing type field"))?
            .to_string();

        let payload_val = obj
            .get("payload")
            .ok_or_else(|| serde::de::Error::custom("missing payload field"))?;

        let payload: EventPayload = match event_type.as_str() {
            "workflow_initialized" => {
                let p: WorkflowInitializedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::WorkflowInitialized {
                    template_path: p.template_path,
                    variables: p.variables,
                }
            }
            "transitioned" => {
                let p: TransitionedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::Transitioned {
                    from: p.from,
                    to: p.to,
                    condition_type: p.condition_type,
                }
            }
            "evidence_submitted" => {
                let p: EvidenceSubmittedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::EvidenceSubmitted {
                    state: p.state,
                    fields: p.fields,
                }
            }
            "directed_transition" => {
                let p: DirectedTransitionPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::DirectedTransition {
                    from: p.from,
                    to: p.to,
                }
            }
            "integration_invoked" => {
                let p: IntegrationInvokedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::IntegrationInvoked {
                    state: p.state,
                    integration: p.integration,
                    output: p.output,
                }
            }
            "rewound" => {
                let p: RewoundPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::Rewound {
                    from: p.from,
                    to: p.to,
                }
            }
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unknown event type: {}",
                    other
                )));
            }
        };

        Ok(Event {
            seq,
            timestamp,
            event_type,
            payload,
        })
    }
}

// Helper structs for typed deserialization of payload variants.
#[derive(Deserialize)]
struct WorkflowInitializedPayload {
    template_path: String,
    #[serde(default)]
    variables: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct TransitionedPayload {
    from: Option<String>,
    to: String,
    condition_type: String,
}

#[derive(Deserialize)]
struct EvidenceSubmittedPayload {
    state: String,
    fields: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct DirectedTransitionPayload {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct IntegrationInvokedPayload {
    state: String,
    integration: String,
    output: serde_json::Value,
}

#[derive(Deserialize)]
struct RewoundPayload {
    from: String,
    to: String,
}

/// Metadata about a workflow, derived from the state file header.
///
/// Used by `koto workflows` to return structured information about
/// each active workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowMetadata {
    /// Workflow name.
    pub name: String,

    /// RFC 3339 UTC timestamp of workflow creation.
    pub created_at: String,

    /// SHA-256 hex of the compiled template JSON.
    pub template_hash: String,
}

/// Derived current state of a workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineState {
    /// The name of the current state, derived from log replay.
    pub current_state: String,

    /// Path to the compiled template (from the header / init event).
    pub template_path: String,

    /// SHA-256 hash of the compiled template (from the header).
    pub template_hash: String,
}

/// Return the current UTC time as an ISO 8601 string.
///
/// Implemented without an external time crate to keep the binary self-contained.
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
    fn header_parsing_round_trip() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "my-workflow".to_string(),
            template_hash: "abc123def456".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&header).unwrap();
        let parsed: StateFileHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(header, parsed);
    }

    #[test]
    fn event_serializes_type_and_payload() {
        let e = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "path/to/template.json".to_string(),
                variables: HashMap::new(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"workflow_initialized\""));
        assert!(json.contains("\"seq\":1"));
        assert!(json.contains("\"template_path\":\"path/to/template.json\""));
        // payload should be flat (no variant wrapper)
        assert!(!json.contains("\"workflow_initialized\":{"));
    }

    #[test]
    fn event_round_trip() {
        let e = Event {
            seq: 3,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "rewound".to_string(),
            payload: EventPayload::Rewound {
                from: "analyze".to_string(),
                to: "gather".to_string(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn event_transitioned_round_trip() {
        let e = Event {
            seq: 2,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "transitioned".to_string(),
            payload: EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn event_payload_type_name() {
        let p = EventPayload::Transitioned {
            from: Some("a".to_string()),
            to: "b".to_string(),
            condition_type: "auto".to_string(),
        };
        assert_eq!(p.type_name(), "transitioned");

        let p2 = EventPayload::Rewound {
            from: "b".to_string(),
            to: "a".to_string(),
        };
        assert_eq!(p2.type_name(), "rewound");
    }

    #[test]
    fn workflow_metadata_roundtrip() {
        let wm = WorkflowMetadata {
            name: "test-wf".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            template_hash: "abc123".to_string(),
        };
        let json = serde_json::to_string(&wm).unwrap();
        let parsed: WorkflowMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(wm, parsed);
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
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
    }
}
