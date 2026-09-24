//! The Jev client: maps a provider-neutral [`DecisionRequest`] to Jev's
//! wire format, sends it over [`post_json_with_deadline`], and validates the
//! answer back into a [`DecisionResponse`].
//!
//! Wire format (<https://docs.typesafe.ai/api.md>):
//!
//! ```text
//! request:  {"model":"jev-latest",
//!            "state":{"<label>":"<content>", ...},
//!            "questions":{"<field>":{"type":"choice","instructions":"...",
//!                                    "criteria":{"<value>":"<description>", ...}},
//!                         "<field>":{"type":"noul","instructions":"..."}}}
//! response: {"model":"jev-1.13.0",
//!            "answers":{"<field>":{"type":"choice","choice":"...",
//!                                  "probabilities":{"<value>":p, ...},"confidence":c},
//!                       "<field>":{"type":"noul","noul":p}},
//!            "usage":{...}}
//! ```
//!
//! Enum fields become `choice` questions whose `criteria` are the value
//! descriptions in `values` order followed by the escape; boolean fields
//! become `noul` questions (P(true)). Jev's own `confidence` is kept as
//! `provider_confidence` and never used to pick or score the winner.

use std::fmt;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::config::validate::describe_endpoint;

use super::http::post_json_with_deadline;
use super::types::{
    check_choice_probabilities, check_proposition, Answer, ApiKey, Decider, DeciderError,
    DecisionRequest, DecisionResponse, QuestionKind,
};

/// Provider name recorded with each consultation.
pub const PROVIDER: &str = "jev";

/// Model name sent with every request. Jev echoes the dated build it used.
pub const MODEL: &str = "jev-latest";

/// Fewest and most `criteria` keys Jev accepts on a `choice` question.
pub const MIN_CHOICE_KEYS: usize = 2;
pub const MAX_CHOICE_KEYS: usize = 255;

/// Longest model string recorded, in characters.
pub const MAX_MODEL_CHARS: usize = 128;

/// Model string recorded when the response names none.
pub const UNKNOWN_MODEL: &str = "unknown";

/// The Jev decider. Built only by [`super::build_decider`].
pub struct JevDecider {
    endpoint: Url,
    key: ApiKey,
    timeout: Duration,
}

impl fmt::Debug for JevDecider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevDecider")
            .field("endpoint", &describe_endpoint(&self.endpoint))
            .field("key", &self.key)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl JevDecider {
    /// Crate-private: production code gets a client only through
    /// [`super::build_decider`], which checks opt-in first.
    pub(crate) fn new(endpoint: Url, key: ApiKey, timeout: Duration) -> Self {
        JevDecider {
            endpoint,
            key,
            timeout,
        }
    }

    /// The endpoint this client sends to.
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// The per-request budget.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Decider for JevDecider {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError> {
        let body = encode_request(req)?;
        let (status, bytes) =
            post_json_with_deadline(&self.endpoint, &self.key, body.into_bytes(), self.timeout)?;
        if !(200..300).contains(&status) {
            if let Some(line) = auth_failure_notice(status) {
                eprintln!("{}", line);
            }
            return Err(DeciderError::http_status(status));
        }
        decode_response(req, &bytes)
    }
}

/// The one stderr line printed when the provider refuses the key. Names
/// the status only.
pub fn auth_failure_notice(status: u16) -> Option<String> {
    match status {
        401 | 403 => Some(format!(
            "warning: the decider provider rejected the API key (HTTP {}); \
             check KOTO_DECIDER_API_KEY or decider.api_key",
            status
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// Encode `req` as Jev's request body.
///
/// Written by hand so key order is fixed (`model`, `state`, `questions`;
/// labels, fields, and values in request order) whatever `serde_json`
/// features are enabled. Refuses, as `malformed`, a request Jev can't
/// accept: duplicate input labels or field names, or a choice with fewer
/// than [`MIN_CHOICE_KEYS`], more than [`MAX_CHOICE_KEYS`], or duplicate
/// values.
pub fn encode_request(req: &DecisionRequest) -> Result<String, DeciderError> {
    check_request(req)?;

    let mut out = String::new();
    out.push_str("{\"model\":");
    push_json_str(&mut out, MODEL);

    out.push_str(",\"state\":{");
    for (i, input) in req.inputs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_str(&mut out, &input.label);
        out.push(':');
        push_json_str(&mut out, &input.content);
    }
    out.push('}');

    out.push_str(",\"questions\":{");
    for (i, q) in req.questions.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_str(&mut out, &q.field);
        out.push(':');
        match &q.kind {
            QuestionKind::Choice {
                question,
                options,
                escape,
            } => {
                out.push_str("{\"type\":\"choice\",\"instructions\":");
                push_json_str(&mut out, question);
                out.push_str(",\"criteria\":{");
                for (j, opt) in options.iter().chain(std::iter::once(escape)).enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    push_json_str(&mut out, &opt.value);
                    out.push(':');
                    push_json_str(&mut out, &opt.description);
                }
                out.push_str("}}");
            }
            QuestionKind::Proposition { proposition } => {
                out.push_str("{\"type\":\"noul\",\"instructions\":");
                push_json_str(&mut out, proposition);
                out.push('}');
            }
        }
    }
    out.push_str("}}");
    Ok(out)
}

fn push_json_str(out: &mut String, s: &str) {
    // Serializing a &str can't fail.
    out.push_str(&Value::String(s.to_string()).to_string());
}

fn check_request(req: &DecisionRequest) -> Result<(), DeciderError> {
    for (i, input) in req.inputs.iter().enumerate() {
        if req.inputs[..i].iter().any(|o| o.label == input.label) {
            return Err(DeciderError::malformed("duplicate input label"));
        }
    }
    if req.questions.is_empty() {
        return Err(DeciderError::malformed("request asks no question"));
    }
    for (i, q) in req.questions.iter().enumerate() {
        if req.questions[..i].iter().any(|o| o.field == q.field) {
            return Err(DeciderError::malformed("duplicate question field"));
        }
        if let QuestionKind::Choice { .. } = q.kind {
            let keys = q.kind.choice_keys();
            if keys.len() < MIN_CHOICE_KEYS || keys.len() > MAX_CHOICE_KEYS {
                return Err(DeciderError::malformed(
                    "choice needs between 2 and 255 values including the escape",
                ));
            }
            for (j, k) in keys.iter().enumerate() {
                if keys[..j].contains(k) {
                    return Err(DeciderError::malformed("duplicate choice value"));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// Validate a Jev response body against the request it answers.
pub fn decode_response(
    req: &DecisionRequest,
    body: &[u8],
) -> Result<DecisionResponse, DeciderError> {
    let root: Value = serde_json::from_slice(body)
        .map_err(|_| DeciderError::malformed("response body is not JSON"))?;
    let root = root
        .as_object()
        .ok_or_else(|| DeciderError::malformed("response is not a JSON object"))?;
    let answers = root
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| DeciderError::malformed("response has no answers object"))?;

    if answers
        .keys()
        .any(|k| !req.questions.iter().any(|q| &q.field == k))
    {
        return Err(DeciderError::mismatched(
            "answer given for a field that was not asked",
        ));
    }

    let mut out = std::collections::BTreeMap::new();
    for q in &req.questions {
        let raw = answers
            .get(&q.field)
            .ok_or_else(|| DeciderError::mismatched("answer missing for an asked field"))?;
        let obj = raw
            .as_object()
            .ok_or_else(|| DeciderError::malformed("answer is not a JSON object"))?;
        let ty = obj.get("type").and_then(Value::as_str);
        let answer = match (&q.kind, ty) {
            (QuestionKind::Choice { .. }, Some("choice")) => {
                // The quickstart's example answer omits `probabilities`; the
                // API reference includes it. Live validation will settle it.
                let probs = obj
                    .get("probabilities")
                    .and_then(Value::as_object)
                    .ok_or_else(|| DeciderError::malformed("choice answer has no probabilities"))?;
                let mut probabilities = std::collections::BTreeMap::new();
                for (k, v) in probs {
                    let p = v
                        .as_f64()
                        .ok_or_else(|| DeciderError::malformed("probability is not a number"))?;
                    probabilities.insert(k.clone(), p);
                }
                check_choice_probabilities(&q.kind.choice_keys(), &probabilities)?;
                Answer::Choice {
                    probabilities,
                    provider_confidence: obj
                        .get("confidence")
                        .and_then(Value::as_f64)
                        .filter(|c| c.is_finite()),
                }
            }
            (QuestionKind::Proposition { .. }, Some("noul")) => {
                let p_true = obj
                    .get("noul")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| DeciderError::malformed("noul answer has no number"))?;
                check_proposition(p_true)?;
                Answer::Proposition { p_true }
            }
            _ => {
                return Err(DeciderError::malformed(
                    "answer type does not match the question",
                ))
            }
        };
        out.insert(q.field.clone(), answer);
    }

    Ok(DecisionResponse {
        model: sanitize_model(root.get("model")),
        answers: out,
    })
}

/// The model string to record: control characters stripped, trimmed, and
/// capped at [`MAX_MODEL_CHARS`]. Missing, non-string, or empty after
/// cleaning is [`UNKNOWN_MODEL`].
pub fn sanitize_model(raw: Option<&Value>) -> String {
    let Some(s) = raw.and_then(Value::as_str) else {
        return UNKNOWN_MODEL.to_string();
    };
    let stripped: String = s.chars().filter(|c| !c.is_control()).collect();
    let capped: String = stripped.trim().chars().take(MAX_MODEL_CHARS).collect();
    let cleaned = capped.trim_end();
    if cleaned.is_empty() {
        UNKNOWN_MODEL.to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{AnswerOption, ErrorClass, LabelledInput, Question};
    use super::*;
    use serde_json::json;

    fn opt(v: &str, d: &str) -> AnswerOption {
        AnswerOption {
            value: v.to_string(),
            description: d.to_string(),
        }
    }

    fn request() -> DecisionRequest {
        DecisionRequest {
            questions: vec![
                Question {
                    field: "verdict".to_string(),
                    kind: QuestionKind::Choice {
                        question: "Clear?".to_string(),
                        options: vec![opt("proceed", "Yes."), opt("exit", "No.")],
                        escape: opt("unclear", "Can't tell."),
                    },
                },
                Question {
                    field: "ready".to_string(),
                    kind: QuestionKind::Proposition {
                        proposition: "Ready to merge.".to_string(),
                    },
                },
            ],
            inputs: vec![
                LabelledInput {
                    label: "zeta".to_string(),
                    content: "z \"quoted\"\n".to_string(),
                },
                LabelledInput {
                    label: "alpha".to_string(),
                    content: "a".to_string(),
                },
            ],
        }
    }

    #[test]
    fn encoding_keeps_request_order() {
        let body = encode_request(&request()).unwrap();
        assert_eq!(
            body,
            concat!(
                r#"{"model":"jev-latest","state":{"zeta":"z \"quoted\"\n","alpha":"a"},"#,
                r#""questions":{"verdict":{"type":"choice","instructions":"Clear?","#,
                r#""criteria":{"proceed":"Yes.","exit":"No.","unclear":"Can't tell."}},"#,
                r#""ready":{"type":"noul","instructions":"Ready to merge."}}}"#
            )
        );
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["state"]["zeta"], "z \"quoted\"\n");
        assert_eq!(body, encode_request(&request()).unwrap());
    }

    #[test]
    fn encoding_refuses_bad_shapes() {
        let mut r = request();
        r.inputs.push(LabelledInput {
            label: "alpha".to_string(),
            content: "again".to_string(),
        });
        assert_eq!(encode_request(&r).unwrap_err().class, ErrorClass::Malformed);

        let mut r = request();
        if let QuestionKind::Choice { options, .. } = &mut r.questions[0].kind {
            options.clear();
        }
        assert_eq!(encode_request(&r).unwrap_err().class, ErrorClass::Malformed);

        let mut r = request();
        if let QuestionKind::Choice { options, .. } = &mut r.questions[0].kind {
            *options = (0..255).map(|i| opt(&format!("v{i}"), "d")).collect();
        }
        assert_eq!(encode_request(&r).unwrap_err().class, ErrorClass::Malformed);
        if let QuestionKind::Choice { options, .. } = &mut r.questions[0].kind {
            options.pop();
        }
        assert!(encode_request(&r).is_ok());
    }

    fn ok_body() -> Value {
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "verdict": {"type": "choice", "choice": "proceed",
                            "probabilities": {"proceed": 0.8, "exit": 0.15, "unclear": 0.05},
                            "confidence": 0.42},
                "ready": {"type": "noul", "noul": 0.7}
            },
            "usage": {"input_tokens": 10, "output_tokens": 2}
        })
    }

    fn decode(v: &Value) -> Result<DecisionResponse, DeciderError> {
        decode_response(&request(), v.to_string().as_bytes())
    }

    #[test]
    fn decodes_a_valid_answer() {
        let r = decode(&ok_body()).unwrap();
        assert_eq!(r.model, "jev-1.13.0");
        match &r.answers["verdict"] {
            Answer::Choice {
                probabilities,
                provider_confidence,
            } => {
                assert_eq!(probabilities["proceed"], 0.8);
                assert_eq!(*provider_confidence, Some(0.42));
            }
            other => panic!("{:?}", other),
        }
        assert_eq!(r.answers["ready"], Answer::Proposition { p_true: 0.7 });
    }

    #[test]
    fn choice_without_probabilities_is_malformed() {
        let mut v = ok_body();
        v["answers"]["verdict"]
            .as_object_mut()
            .unwrap()
            .remove("probabilities");
        assert_eq!(decode(&v).unwrap_err().class, ErrorClass::Malformed);
    }

    #[test]
    fn model_is_sanitized() {
        assert_eq!(sanitize_model(None), "unknown");
        assert_eq!(sanitize_model(Some(&json!(""))), "unknown");
        assert_eq!(sanitize_model(Some(&json!("   "))), "unknown");
        assert_eq!(sanitize_model(Some(&json!(42))), "unknown");
        let raw = format!("  \u{1b}[31mjev\u{7}-{}\n ", "x".repeat(300));
        let got = sanitize_model(Some(&json!(raw)));
        assert!(got.starts_with("[31mjev-x"));
        assert!(!got.chars().any(|c| c.is_control()));
        assert_eq!(got.chars().count(), MAX_MODEL_CHARS);
    }

    #[test]
    fn auth_notice_names_the_status_only() {
        assert!(auth_failure_notice(401).unwrap().contains("HTTP 401"));
        assert!(auth_failure_notice(403).unwrap().contains("HTTP 403"));
        assert!(auth_failure_notice(500).is_none());
    }

    #[test]
    fn debug_hides_key_and_userinfo() {
        let d = JevDecider::new(
            Url::parse("https://api.example.com/v1/x?q=1").unwrap(),
            ApiKey::new("sk-debug-secret"),
            Duration::from_secs(1),
        );
        let s = format!("{:?}", d);
        assert!(!s.contains("sk-debug-secret"));
        assert!(!s.contains("q=1"));
        assert_eq!(d.provider(), "jev");
    }
}
