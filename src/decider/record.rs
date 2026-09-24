//! The record of one consultation.
//!
//! [`DeciderConsultation`] is the payload of the `decider_consulted` event
//! and, flattened, the body of the ledger's `consulted` record. It holds
//! names, hashes, numbers, modes, outcomes, and the error class only: no
//! input content, no API key, no response body, and no error text. Nothing
//! here does I/O.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::template::decider::DeciderMode;

use super::evaluate::{FieldEvaluation, FieldOutcome};
use super::types::{ErrorClass, SettingOrigin};

/// Decimal places probabilities and confidences are recorded to.
pub const RECORDED_DECIMALS: i32 = 4;

/// Round `p` to [`RECORDED_DECIMALS`] places for the record. Comparisons
/// such as `at_threshold` are made on the unrounded number before this is
/// applied.
pub fn round_recorded(p: f64) -> f64 {
    let scale = 10f64.powi(RECORDED_DECIMALS);
    (p * scale).round() / scale
}

/// How a consultation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsultationOutcome {
    /// Every field qualified and the answer was submitted as evidence.
    Applied,
    /// The provider answered, but the answer wasn't applied.
    NotApplied,
    /// A declared input was unset or over its byte budget, so nothing was
    /// sent.
    InputUnavailable,
    /// The provider call failed; `error_class` says how.
    Error,
}

impl ConsultationOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConsultationOutcome::Applied => "applied",
            ConsultationOutcome::NotApplied => "not_applied",
            ConsultationOutcome::InputUnavailable => "input_unavailable",
            ConsultationOutcome::Error => "error",
        }
    }
}

/// One declared field in a consultation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldConsultation {
    /// The field's `declaration_hash`, so evidence gathered under one
    /// declaration is never mixed with another's.
    pub declaration_hash: String,
    /// The effective mode of each declared value (`"true"`/`"false"` for a
    /// boolean).
    pub modes: BTreeMap<String, DeciderMode>,
    /// Probability per value, rounded to four places. Empty when the
    /// provider gave no usable answer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub probabilities: BTreeMap<String, f64>,
    /// The winning value, if one won.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winning: Option<String>,
    /// The winner's probability, rounded to four places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// The winning declared value's threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Whether the confidence met the threshold, compared on the unrounded
    /// numbers.
    #[serde(default)]
    pub at_threshold: bool,
    /// The field's outcome. Absent when the provider gave no usable answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<FieldOutcome>,
}

impl FieldConsultation {
    /// A field with no evaluation: the consultation stopped before or at
    /// the provider.
    pub fn unevaluated(declaration_hash: String, modes: BTreeMap<String, DeciderMode>) -> Self {
        FieldConsultation {
            declaration_hash,
            modes,
            probabilities: BTreeMap::new(),
            winning: None,
            confidence: None,
            threshold: None,
            at_threshold: false,
            outcome: None,
        }
    }

    /// A field with its evaluation. Probabilities and confidence are
    /// rounded here; `at_threshold` is copied from the evaluation, which
    /// compared the unrounded numbers.
    pub fn evaluated(
        declaration_hash: String,
        modes: BTreeMap<String, DeciderMode>,
        eval: &FieldEvaluation,
    ) -> Self {
        FieldConsultation {
            declaration_hash,
            modes,
            probabilities: eval
                .probabilities
                .iter()
                .map(|(k, p)| (k.clone(), round_recorded(*p)))
                .collect(),
            winning: eval.winning.clone(),
            confidence: Some(round_recorded(eval.confidence)),
            threshold: eval.threshold,
            at_threshold: eval.at_threshold,
            outcome: Some(eval.outcome),
        }
    }
}

/// One consultation, as recorded on the `decider_consulted` event and in
/// the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeciderConsultation {
    /// The state consulted.
    pub state: String,
    /// The seq of the event that began the visit: the stickiness key.
    pub visit_seq: u64,
    /// Provider name (`jev`).
    pub provider: String,
    /// Model build the provider reported, or `unknown`.
    pub model: String,
    /// SHA-256 of the assembled inputs; absent when they couldn't be
    /// assembled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_sha256: Option<String>,
    pub outcome: ConsultationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
    /// Wall time of the provider call, in milliseconds.
    pub latency_ms: u64,
    /// Byte length of the substituted directive plus details the agent
    /// would have received for this state.
    pub directive_bytes: u64,
    /// Where the endpoint came from: `default`, `user`, or `env`.
    pub endpoint_origin: SettingOrigin,
    /// Keyed by field name.
    pub fields: BTreeMap<String, FieldConsultation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DeciderConsultation {
        let mut modes = BTreeMap::new();
        modes.insert("proceed".to_string(), DeciderMode::Auto);
        modes.insert("exit".to_string(), DeciderMode::Never);
        let mut fields = BTreeMap::new();
        fields.insert(
            "verdict".to_string(),
            FieldConsultation::unevaluated("ab".repeat(32), modes),
        );
        DeciderConsultation {
            state: "review".to_string(),
            visit_seq: 7,
            provider: "jev".to_string(),
            model: "unknown".to_string(),
            input_sha256: None,
            outcome: ConsultationOutcome::Error,
            error_class: Some(ErrorClass::Timeout),
            latency_ms: 201,
            directive_bytes: 42,
            endpoint_origin: SettingOrigin::Env,
            fields,
        }
    }

    #[test]
    fn round_trips_and_uses_wire_names() {
        let c = sample();
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["outcome"], "error");
        assert_eq!(v["error_class"], "timeout");
        assert_eq!(v["endpoint_origin"], "env");
        assert!(v.get("input_sha256").is_none());
        assert_eq!(v["fields"]["verdict"]["modes"]["exit"], "never");
        assert!(v["fields"]["verdict"].get("probabilities").is_none());
        let back: DeciderConsultation = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn outcome_names() {
        for (o, n) in [
            (ConsultationOutcome::Applied, "applied"),
            (ConsultationOutcome::NotApplied, "not_applied"),
            (ConsultationOutcome::InputUnavailable, "input_unavailable"),
            (ConsultationOutcome::Error, "error"),
        ] {
            assert_eq!(o.as_str(), n);
            assert_eq!(serde_json::to_string(&o).unwrap(), format!("\"{}\"", n));
        }
    }

    #[test]
    fn rounding_keeps_at_threshold_from_the_unrounded_number() {
        let eval = FieldEvaluation {
            field: "verdict".to_string(),
            probabilities: [
                ("proceed".to_string(), 0.89996),
                ("exit".to_string(), 0.1),
                ("unclear".to_string(), 0.00004),
            ]
            .into_iter()
            .collect(),
            winning: Some("proceed".to_string()),
            confidence: 0.89996,
            threshold: Some(0.9),
            at_threshold: 0.89996 >= 0.9,
            outcome: FieldOutcome::BelowThreshold,
        };
        let f = FieldConsultation::evaluated("h".to_string(), BTreeMap::new(), &eval);
        assert_eq!(f.probabilities["proceed"], 0.9);
        assert_eq!(f.confidence, Some(0.9));
        assert!(!f.at_threshold);
        assert_eq!(f.outcome, Some(FieldOutcome::BelowThreshold));
    }

    #[test]
    fn record_holds_no_free_text_fields() {
        // Every string field is a name, a hash, a mode, or a closed-set
        // value. Guard the shape so a free-text field can't slip in.
        let v = serde_json::to_value(sample()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "directive_bytes",
                "endpoint_origin",
                "error_class",
                "fields",
                "latency_ms",
                "model",
                "outcome",
                "provider",
                "state",
                "visit_seq",
            ]
        );
    }
}
