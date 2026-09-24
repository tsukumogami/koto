//! Pure decider types shared by configuration, the engine, and the
//! provider client. Nothing here does I/O.
//!
//! The request and response types are provider-neutral: they speak of
//! questions, answer options, and per-value probabilities, never of any
//! one provider's wire vocabulary. A provider client (see `jev.rs`) maps
//! them to and from its own format.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A decider API key.
///
/// The type deliberately has no `Display` impl and its `Debug` prints
/// `<redacted>`, so the key can't reach a log line, an error message, or
/// a `{:?}` dump by accident. The raw value is reachable only through
/// [`ApiKey::expose_for_transport`], which exists for the HTTP transport
/// that puts it in the `Authorization` header and for nothing else.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap a raw key string.
    pub fn new(raw: impl Into<String>) -> Self {
        ApiKey(raw.into())
    }

    /// The raw key, for the transport's `Authorization` header only.
    ///
    /// Never interpolate the return value into a message, a warning, an
    /// error, or anything that is printed or persisted.
    pub fn expose_for_transport(&self) -> &str {
        &self.0
    }

    /// Whether `raw` can be sent as a bearer token in an HTTP header:
    /// non-empty and made only of printable ASCII (space through `~`).
    /// A control character (newline, CR, tab, DEL) or a non-ASCII
    /// character makes it unusable. Config treats such a key as absent.
    pub fn is_header_safe(raw: &str) -> bool {
        !raw.is_empty() && raw.bytes().all(|b| (0x20..0x7f).contains(&b))
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The user's global decider mode, ordered `Off < Shadow < Auto`.
///
/// The effective global mode is the minimum of the user-level mode (env
/// or user config) and the project mode, so a repository can only lower
/// it. `never` is a template-only mode and is not a valid global mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalMode {
    Off,
    Shadow,
    Auto,
}

impl GlobalMode {
    /// Parse a mode string, trimming whitespace and ignoring case.
    /// Returns `None` for anything other than `off`, `shadow`, or `auto`
    /// (including `never` and the empty string).
    pub fn parse(raw: &str) -> Option<GlobalMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Some(GlobalMode::Off),
            "shadow" => Some(GlobalMode::Shadow),
            "auto" => Some(GlobalMode::Auto),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GlobalMode::Off => "off",
            GlobalMode::Shadow => "shadow",
            GlobalMode::Auto => "auto",
        }
    }
}

/// Which configuration layer a decider setting came from.
///
/// Recorded on each consultation as `endpoint_origin`, and used by the
/// same-layer rule: a key is only sent to an endpoint from its own layer
/// or to the built-in default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingOrigin {
    /// Built-in default (only meaningful for the endpoint).
    #[default]
    Default,
    /// `~/.koto/config.toml`.
    User,
    /// A `KOTO_DECIDER*` environment variable.
    Env,
}

impl SettingOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingOrigin::Default => "default",
            SettingOrigin::User => "user",
            SettingOrigin::Env => "env",
        }
    }
}

// ---------------------------------------------------------------------------
// The provider interface
// ---------------------------------------------------------------------------

/// A typed decision model koto can consult instead of stopping for agent
/// evidence.
///
/// One call answers every declared field on a state. Implementations are
/// synchronous and must bound their own latency; the engine treats any
/// `Err` as "not applied" and falls back to the response an opted-out user
/// would get.
pub trait Decider {
    /// Stable provider name recorded with each consultation (`jev`).
    fn provider(&self) -> &str;

    /// Ask the provider to answer `req`.
    fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError>;
}

/// One consultation: every declared field of a state, plus the labelled
/// inputs the declarations read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// One question per declared field, in declaration order.
    pub questions: Vec<Question>,
    /// Labelled inputs, in declaration order, each label once.
    pub inputs: Vec<LabelledInput>,
}

/// The question asked for one declared field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// The `accepts` field name. The answer comes back under this key.
    pub field: String,
    pub kind: QuestionKind,
}

/// What shape of answer a question expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// An enum field: pick one of `options` or the `escape`.
    Choice {
        /// The field description.
        question: String,
        /// Every declared value with its answer description, in the
        /// field's `values` order.
        options: Vec<AnswerOption>,
        /// The value that means "can't be judged from these inputs".
        escape: AnswerOption,
    },
    /// A boolean field: how likely is the proposition to be true.
    Proposition {
        /// The field description.
        proposition: String,
    },
}

impl QuestionKind {
    /// Every value a probability map for this question must cover: the
    /// options in order, then the escape. Empty for a proposition.
    pub fn choice_keys(&self) -> Vec<&str> {
        match self {
            QuestionKind::Choice {
                options, escape, ..
            } => options
                .iter()
                .map(|o| o.value.as_str())
                .chain(std::iter::once(escape.value.as_str()))
                .collect(),
            QuestionKind::Proposition { .. } => Vec::new(),
        }
    }
}

/// A value the provider may answer with, and what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerOption {
    pub value: String,
    pub description: String,
}

/// One assembled input, as the provider sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelledInput {
    pub label: String,
    pub content: String,
}

/// A provider's answer to a [`DecisionRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// The model build the provider reports, already sanitized, or
    /// `unknown`.
    pub model: String,
    /// Keyed by field name.
    pub answers: BTreeMap<String, Answer>,
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Answer to a [`QuestionKind::Choice`].
    Choice {
        /// Probability per value, covering every option and the escape.
        probabilities: BTreeMap<String, f64>,
        /// The provider's own confidence figure, if it reports one. Kept
        /// for evaluation only: the winner and its confidence always come
        /// from `probabilities`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_confidence: Option<f64>,
    },
    /// Answer to a [`QuestionKind::Proposition`]: P(true).
    Proposition { p_true: f64 },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a consultation produced no usable answer. Recorded as
/// `error_class`; these five names are part of the event contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The request didn't finish within the budget.
    Timeout,
    /// The request couldn't be sent or the connection failed.
    Connect,
    /// The provider answered with a status other than 2xx.
    HttpStatus,
    /// The response (or the request koto would send) is badly formed.
    Malformed,
    /// The response is well formed but doesn't answer what was asked.
    Mismatched,
}

impl ErrorClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorClass::Timeout => "timeout",
            ErrorClass::Connect => "connect",
            ErrorClass::HttpStatus => "http_status",
            ErrorClass::Malformed => "malformed",
            ErrorClass::Mismatched => "mismatched",
        }
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Longest `detail` a [`DeciderError`] carries, in characters.
pub const MAX_ERROR_DETAIL_CHARS: usize = 200;

/// A failed consultation.
///
/// `detail` is built only from fixed text and, where useful, the kind of
/// the underlying failure (an I/O error kind, an HTTP status). It never
/// holds a response body, a header, a URL, or the key, and it is capped at
/// [`MAX_ERROR_DETAIL_CHARS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeciderError {
    pub class: ErrorClass,
    /// The HTTP status, set for [`ErrorClass::HttpStatus`] only.
    pub status: Option<u16>,
    pub detail: String,
}

impl DeciderError {
    /// An error with fixed text only.
    pub fn new(class: ErrorClass, detail: &'static str) -> Self {
        Self::build(class, None, detail.to_string())
    }

    /// An error with fixed text followed by the failure kind, for example
    /// `connection refused`. `kind` must be a closed-set description, never
    /// text received from the network.
    pub fn with_kind(class: ErrorClass, detail: &'static str, kind: impl fmt::Display) -> Self {
        Self::build(class, None, format!("{}: {}", detail, kind))
    }

    /// The provider answered with a non-2xx status.
    pub fn http_status(status: u16) -> Self {
        Self::build(
            ErrorClass::HttpStatus,
            Some(status),
            format!("provider returned HTTP {}", status),
        )
    }

    pub fn timeout() -> Self {
        Self::new(ErrorClass::Timeout, "request exceeded the decider timeout")
    }

    pub fn malformed(detail: &'static str) -> Self {
        Self::new(ErrorClass::Malformed, detail)
    }

    pub fn mismatched(detail: &'static str) -> Self {
        Self::new(ErrorClass::Mismatched, detail)
    }

    fn build(class: ErrorClass, status: Option<u16>, detail: String) -> Self {
        let status = if class == ErrorClass::HttpStatus {
            status
        } else {
            None
        };
        let detail = if detail.chars().count() > MAX_ERROR_DETAIL_CHARS {
            detail.chars().take(MAX_ERROR_DETAIL_CHARS).collect()
        } else {
            detail
        };
        DeciderError {
            class,
            status,
            detail,
        }
    }
}

impl fmt::Display for DeciderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decider {}: {}", self.class, self.detail)
    }
}

impl std::error::Error for DeciderError {}

// ---------------------------------------------------------------------------
// Shared answer checks
// ---------------------------------------------------------------------------

/// How far a choice's probabilities may sum from 1.
pub const PROBABILITY_SUM_TOLERANCE: f64 = 0.01;

/// Whether `p` is a usable probability: finite and within [0, 1].
pub fn is_probability(p: f64) -> bool {
    p.is_finite() && (0.0..=1.0).contains(&p)
}

/// Check a choice's probability map against the values that were asked.
///
/// A key outside `expected`, or an `expected` key with no probability, is
/// `mismatched`. A value that isn't finite or lies outside [0, 1], or a sum
/// more than [`PROBABILITY_SUM_TOLERANCE`] from 1, is `malformed`.
pub fn check_choice_probabilities(
    expected: &[&str],
    probabilities: &BTreeMap<String, f64>,
) -> Result<(), DeciderError> {
    if probabilities
        .keys()
        .any(|k| !expected.contains(&k.as_str()))
    {
        return Err(DeciderError::mismatched(
            "answer names a value that was not asked",
        ));
    }
    if expected.iter().any(|k| !probabilities.contains_key(*k)) {
        return Err(DeciderError::mismatched(
            "answer is missing a probability for an asked value",
        ));
    }
    if probabilities.values().any(|p| !is_probability(*p)) {
        return Err(DeciderError::malformed(
            "probability is not a finite number in [0, 1]",
        ));
    }
    let sum: f64 = probabilities.values().sum();
    if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(DeciderError::malformed("probabilities do not sum to 1"));
    }
    Ok(())
}

/// Check a proposition's P(true).
pub fn check_proposition(p_true: f64) -> Result<(), DeciderError> {
    if is_probability(p_true) {
        Ok(())
    } else {
        Err(DeciderError::malformed(
            "probability is not a finite number in [0, 1]",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_class_serializes_to_the_five_names() {
        let cases = [
            (ErrorClass::Timeout, "timeout"),
            (ErrorClass::Connect, "connect"),
            (ErrorClass::HttpStatus, "http_status"),
            (ErrorClass::Malformed, "malformed"),
            (ErrorClass::Mismatched, "mismatched"),
        ];
        for (class, name) in cases {
            assert_eq!(
                serde_json::to_string(&class).unwrap(),
                format!("\"{}\"", name)
            );
            assert_eq!(class.as_str(), name);
            assert_eq!(
                serde_json::from_str::<ErrorClass>(&format!("\"{}\"", name)).unwrap(),
                class
            );
        }
    }

    #[test]
    fn decider_error_status_only_for_http_status() {
        let e = DeciderError::http_status(503);
        assert_eq!(e.class, ErrorClass::HttpStatus);
        assert_eq!(e.status, Some(503));
        assert!(e.detail.contains("503"));
        assert_eq!(DeciderError::timeout().status, None);
        assert_eq!(
            DeciderError::build(ErrorClass::Connect, Some(500), "x".into()).status,
            None
        );
    }

    #[test]
    fn decider_error_detail_is_capped() {
        let e = DeciderError::with_kind(ErrorClass::Connect, "request failed", "k".repeat(500));
        assert_eq!(e.detail.chars().count(), MAX_ERROR_DETAIL_CHARS);
        assert!(e.to_string().starts_with("decider connect: request failed"));
    }

    #[test]
    fn decider_is_object_safe() {
        struct Null;
        impl Decider for Null {
            fn provider(&self) -> &str {
                "null"
            }
            fn decide(&self, _: &DecisionRequest) -> Result<DecisionResponse, DeciderError> {
                Err(DeciderError::timeout())
            }
        }
        let d: Box<dyn Decider> = Box::new(Null);
        assert_eq!(d.provider(), "null");
    }

    fn probs(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn choice_probability_checks() {
        let keys = ["a", "b", "esc"];
        let ok = probs(&[("a", 0.5), ("b", 0.3), ("esc", 0.2)]);
        assert!(check_choice_probabilities(&keys, &ok).is_ok());

        let extra = probs(&[("a", 0.5), ("b", 0.3), ("esc", 0.1), ("z", 0.1)]);
        assert_eq!(
            check_choice_probabilities(&keys, &extra).unwrap_err().class,
            ErrorClass::Mismatched
        );
        let missing = probs(&[("a", 0.7), ("b", 0.3)]);
        assert_eq!(
            check_choice_probabilities(&keys, &missing)
                .unwrap_err()
                .class,
            ErrorClass::Mismatched
        );
        for bad in [f64::NAN, -0.1, 1.5, f64::INFINITY] {
            let p = probs(&[("a", bad), ("b", 0.3), ("esc", 0.2)]);
            assert_eq!(
                check_choice_probabilities(&keys, &p).unwrap_err().class,
                ErrorClass::Malformed
            );
        }
        let high = probs(&[("a", 0.52), ("b", 0.3), ("esc", 0.2)]);
        assert_eq!(
            check_choice_probabilities(&keys, &high).unwrap_err().class,
            ErrorClass::Malformed
        );
        let near = probs(&[("a", 0.509), ("b", 0.3), ("esc", 0.2)]);
        assert!(check_choice_probabilities(&keys, &near).is_ok());
    }

    #[test]
    fn proposition_checks() {
        assert!(check_proposition(0.0).is_ok());
        assert!(check_proposition(1.0).is_ok());
        for bad in [f64::NAN, -0.01, 1.01] {
            assert_eq!(
                check_proposition(bad).unwrap_err().class,
                ErrorClass::Malformed
            );
        }
    }

    #[test]
    fn api_key_debug_is_redacted() {
        let key = ApiKey::new("sk-very-secret-value");
        let dbg = format!("{:?}", key);
        assert_eq!(dbg, "<redacted>");
        assert!(!dbg.contains("sk-very-secret-value"));
        let dbg_opt = format!("{:?}", Some(key.clone()));
        assert!(!dbg_opt.contains("sk-very-secret-value"));
        assert_eq!(key.expose_for_transport(), "sk-very-secret-value");
    }

    #[test]
    fn header_safe_keys_are_printable_ascii_only() {
        assert!(ApiKey::is_header_safe("sk-abc_123.XYZ"));
        assert!(ApiKey::is_header_safe("a b~!"));
        for bad in [
            "",
            "sk-SECRET\nX",
            "sk\rX",
            "sk\tX",
            "sk\u{0}X",
            "sk\u{7f}X",
            "sk\u{e9}X",
            "sk\u{202e}X",
        ] {
            assert!(!ApiKey::is_header_safe(bad), "{:?}", bad);
        }
    }

    #[test]
    fn global_mode_parse_trims_and_lowercases() {
        assert_eq!(GlobalMode::parse(" Shadow "), Some(GlobalMode::Shadow));
        assert_eq!(GlobalMode::parse("AUTO"), Some(GlobalMode::Auto));
        assert_eq!(GlobalMode::parse("off"), Some(GlobalMode::Off));
        assert_eq!(GlobalMode::parse("never"), None);
        assert_eq!(GlobalMode::parse("bogus"), None);
        assert_eq!(GlobalMode::parse(""), None);
    }

    #[test]
    fn global_mode_orders_off_shadow_auto() {
        assert!(GlobalMode::Off < GlobalMode::Shadow);
        assert!(GlobalMode::Shadow < GlobalMode::Auto);
        assert_eq!(GlobalMode::Auto.min(GlobalMode::Shadow), GlobalMode::Shadow);
    }

    #[test]
    fn origin_strings() {
        assert_eq!(SettingOrigin::Default.as_str(), "default");
        assert_eq!(SettingOrigin::User.as_str(), "user");
        assert_eq!(SettingOrigin::Env.as_str(), "env");
        assert_eq!(
            serde_json::to_string(&SettingOrigin::Env).unwrap(),
            "\"env\""
        );
    }
}
