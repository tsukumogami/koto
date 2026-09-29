//! Compiled `decider-check` gates.
//!
//! A `decider-check` gate runs an extraction command and asks an opted-in
//! decider whether its output meets each of a few closed criteria
//! (docs/designs/DESIGN-koto-decider-checks.md, Decision 1). The compiler
//! (`src/template/compile.rs`) lowers the source keys into
//! [`DeciderCheckSpec`] with every default resolved; `CompiledTemplate::validate`
//! checks it with the `E-DECIDER-CHECK-*` rules, so a compiled template loaded
//! from the cache gets the same checks. Nothing here does I/O.
//!
//! A gate without the spec serializes exactly as before, so a template that
//! declares no decider check keeps its `template_hash`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Byte budget a check compiles to when it declares none: the range the
/// accuracy evidence behind the two proven criteria measured.
pub const DEFAULT_CHECK_MAX_BYTES: u32 = 2560;

/// Largest byte budget a check may declare.
pub const MAX_CHECK_MAX_BYTES: u32 = 8192;

/// Input label a check compiles to when it declares none.
pub const DEFAULT_CHECK_LABEL: &str = "artifact";

/// Longest input label, in bytes.
pub const MAX_CHECK_LABEL_BYTES: usize = 64;

/// Most criteria one state may declare, across all its decider checks. It
/// equals the per-call consultation cap, so a state's criteria fit one call.
pub const MAX_CRITERIA_PER_STATE: usize = 4;

/// Longest `rule_id`, in bytes. Matches the finding bound.
pub const MAX_RULE_ID_BYTES: usize = crate::findings::RULE_ID_MAX_BYTES;

/// Longest `rule_ref`, in bytes. Matches the finding bound.
pub const MAX_RULE_REF_BYTES: usize = crate::findings::RULE_REF_MAX_BYTES;

/// Threshold a criterion compiles to when it declares none.
pub const DEFAULT_CHECK_THRESHOLD: f64 = super::decider::DEFAULT_THRESHOLD;

/// The choice value that means the slice meets the criterion.
pub const PASS_VALUE: &str = "pass";

/// The choice value that means the slice breaks the criterion.
pub const FAIL_VALUE: &str = "fail";

/// The escape: the slice can't be judged from what is shown.
pub const ESCAPE_VALUE: &str = "unclear";

/// A criterion's template mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckMode {
    /// Consult and record, never block.
    Shadow,
    /// Block on a fail when the user's effective mode is `auto`. A missing
    /// verdict is recorded but never blocks.
    Veto,
}

impl CheckMode {
    /// Mode a criterion compiles to when it declares none.
    pub const DEFAULT: CheckMode = CheckMode::Shadow;

    /// Every accepted spelling, in the order error messages list them.
    pub const NAMES: [&'static str; 2] = ["shadow", "veto"];

    pub fn parse(s: &str) -> Option<CheckMode> {
        match s {
            "shadow" => Some(CheckMode::Shadow),
            "veto" => Some(CheckMode::Veto),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CheckMode::Shadow => "shadow",
            CheckMode::Veto => "veto",
        }
    }
}

/// One closed question a decider check asks about its slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckCriterion {
    /// Opaque id, unique on its state. Findings carry it.
    pub rule_id: String,
    /// Opaque reference to the rule's text. Findings carry it.
    pub rule_ref: String,
    /// The question the decider answers.
    pub question: String,
    /// What a pass means.
    pub pass: String,
    /// What a fail means.
    pub fail: String,
    /// What the escape means.
    pub escape: String,
    pub threshold: f64,
    pub mode: CheckMode,
}

/// The decider-specific half of a `decider-check` gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeciderCheckSpec {
    /// Largest slice, in redacted bytes, that is sent.
    pub max_bytes: u32,
    /// The slice's label in the request.
    pub label: String,
    /// In declaration order: criteria are consulted in this order.
    pub criteria: Vec<CheckCriterion>,
}

/// Whether `label` is a usable input label.
pub fn label_is_valid(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_CHECK_LABEL_BYTES
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The declaration hash of one criterion: SHA-256, as lowercase hex, over
/// the criterion's `rule_id`, question and three descriptions and its
/// check's command (as the template wrote it, before substitution), byte
/// budget and label.
///
/// Mode, threshold and `rule_ref` are left out, so changing them keeps the
/// evidence gathered under the declaration. The fields are destructured, so
/// adding one to [`CheckCriterion`] fails to compile here until someone
/// decides whether it belongs in the hash.
pub fn declaration_hash(
    criterion: &CheckCriterion,
    command: &str,
    max_bytes: u32,
    label: &str,
) -> String {
    let CheckCriterion {
        rule_id,
        rule_ref: _,
        question,
        pass,
        fail,
        escape,
        threshold: _,
        mode: _,
    } = criterion;
    let fingerprint = serde_json::json!({
        "rule_id": rule_id,
        "question": question,
        "pass": pass,
        "fail": fail,
        "escape": escape,
        "command": command,
        "max_bytes": max_bytes,
        "label": label,
    });
    let bytes = serde_json::to_vec(&fingerprint).expect("a JSON value always serializes");
    hex::encode(Sha256::digest(&bytes))
}

/// The error prefix for one criterion.
fn at(code: &str, state: &str, gate: &str, rule_id: &str) -> String {
    format!(
        "{}: state {:?} check {:?} criterion {:?}",
        code, state, gate, rule_id
    )
}

/// Check one decider check's spec: its budget, its label, and each
/// criterion's required text, bounds and threshold. The per-state rules
/// (count, duplicates, routing) are `CompiledTemplate::validate`'s.
pub fn validate_spec(state: &str, gate: &str, spec: &DeciderCheckSpec) -> Result<(), String> {
    if spec.max_bytes == 0 || spec.max_bytes > MAX_CHECK_MAX_BYTES {
        return Err(format!(
            "E-DECIDER-CHECK-BUDGET: state {:?} check {:?}: max_bytes {} is outside 1 to {}\n  \
             remedy: set max_bytes from 1 to {}, or omit it for {}",
            state,
            gate,
            spec.max_bytes,
            MAX_CHECK_MAX_BYTES,
            MAX_CHECK_MAX_BYTES,
            DEFAULT_CHECK_MAX_BYTES
        ));
    }
    if !label_is_valid(&spec.label) {
        return Err(format!(
            "E-DECIDER-CHECK-LABEL: state {:?} check {:?}: label {:?} must be 1 to {} bytes of \
             letters, digits, `_` and `-`\n  \
             remedy: rename the label, or omit it for {:?}",
            state, gate, spec.label, MAX_CHECK_LABEL_BYTES, DEFAULT_CHECK_LABEL
        ));
    }
    if spec.criteria.is_empty() {
        return Err(format!(
            "E-DECIDER-CHECK-FIELD: state {:?} check {:?}: a decider check needs at least one \
             criterion\n  \
             remedy: add a criterion under criteria",
            state, gate
        ));
    }
    for c in &spec.criteria {
        if c.rule_id.trim().is_empty() || c.rule_id.len() > MAX_RULE_ID_BYTES {
            return Err(format!(
                "E-DECIDER-CHECK-FIELD: state {:?} check {:?}: a criterion's rule_id must be \
                 non-empty and at most {} bytes, found {:?}\n  \
                 remedy: name the criterion with a shorter id",
                state, gate, MAX_RULE_ID_BYTES, c.rule_id
            ));
        }
        let at = |code: &str| at(code, state, gate, &c.rule_id);
        if c.rule_ref.trim().is_empty() || c.rule_ref.len() > MAX_RULE_REF_BYTES {
            return Err(format!(
                "{}: rule_ref must be non-empty and at most {} bytes\n  \
                 remedy: point rule_ref at the rule's text",
                at("E-DECIDER-CHECK-FIELD"),
                MAX_RULE_REF_BYTES
            ));
        }
        for (key, text) in [
            ("question", &c.question),
            ("pass", &c.pass),
            ("fail", &c.fail),
            ("escape", &c.escape),
        ] {
            if text.trim().is_empty() {
                return Err(format!(
                    "{}: {} must be non-empty; the decider reads it as part of the question\n  \
                     remedy: write the {}",
                    at("E-DECIDER-CHECK-FIELD"),
                    key,
                    key
                ));
            }
        }
        if !c.threshold.is_finite()
            || c.threshold < super::decider::MIN_THRESHOLD
            || c.threshold > super::decider::MAX_THRESHOLD
        {
            return Err(format!(
                "{}: threshold {} is outside [{}, {}]\n  \
                 remedy: set a threshold from {} to {}, or omit it for {}",
                at("E-DECIDER-CHECK-THRESHOLD"),
                c.threshold,
                super::decider::MIN_THRESHOLD,
                super::decider::MAX_THRESHOLD,
                super::decider::MIN_THRESHOLD,
                super::decider::MAX_THRESHOLD,
                DEFAULT_CHECK_THRESHOLD
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Edit = Box<dyn Fn(&mut CheckCriterion)>;

    fn criterion() -> CheckCriterion {
        CheckCriterion {
            rule_id: "comment_reason".into(),
            rule_ref: "https://example.org/rules/comment-reason".into(),
            question: "Does each comment give a reason?".into(),
            pass: "Every comment says why.".into(),
            fail: "A comment restates the code.".into(),
            escape: "No comment, or can't tell.".into(),
            threshold: 0.9,
            mode: CheckMode::Shadow,
        }
    }

    fn spec() -> DeciderCheckSpec {
        DeciderCheckSpec {
            max_bytes: DEFAULT_CHECK_MAX_BYTES,
            label: DEFAULT_CHECK_LABEL.into(),
            criteria: vec![criterion()],
        }
    }

    #[test]
    fn declaration_hash_ignores_mode_threshold_and_rule_ref() {
        let base = declaration_hash(&criterion(), "cmd", 2560, "artifact");
        let mut c = criterion();
        c.mode = CheckMode::Veto;
        c.threshold = 0.95;
        c.rule_ref = "elsewhere".into();
        assert_eq!(declaration_hash(&c, "cmd", 2560, "artifact"), base);
    }

    #[test]
    fn declaration_hash_covers_the_question_descriptions_and_check() {
        let base = declaration_hash(&criterion(), "cmd", 2560, "artifact");
        let edits: Vec<Edit> = vec![
            Box::new(|c| c.rule_id = "other".into()),
            Box::new(|c| c.question = "Other?".into()),
            Box::new(|c| c.pass = "p".into()),
            Box::new(|c| c.fail = "f".into()),
            Box::new(|c| c.escape = "e".into()),
        ];
        for edit in edits {
            let mut c = criterion();
            edit(&mut c);
            assert_ne!(declaration_hash(&c, "cmd", 2560, "artifact"), base);
        }
        assert_ne!(
            declaration_hash(&criterion(), "cmd2", 2560, "artifact"),
            base
        );
        assert_ne!(
            declaration_hash(&criterion(), "cmd", 2561, "artifact"),
            base
        );
        assert_ne!(
            declaration_hash(&criterion(), "cmd", 2560, "comments"),
            base
        );
    }

    #[test]
    fn thresholds_at_the_bounds_are_accepted_and_outside_refused() {
        for t in [0.5, 1.0] {
            let mut s = spec();
            s.criteria[0].threshold = t;
            assert!(validate_spec("st", "g", &s).is_ok(), "{}", t);
        }
        for t in [0.49, 1.01, f64::NAN] {
            let mut s = spec();
            s.criteria[0].threshold = t;
            let e = validate_spec("st", "g", &s).unwrap_err();
            assert!(e.starts_with("E-DECIDER-CHECK-THRESHOLD"), "{}", e);
        }
    }

    #[test]
    fn budget_bounds() {
        for (b, ok) in [(0, false), (1, true), (8192, true), (8193, false)] {
            let mut s = spec();
            s.max_bytes = b;
            let r = validate_spec("st", "g", &s);
            assert_eq!(r.is_ok(), ok, "{}", b);
            if let Err(e) = r {
                assert!(e.starts_with("E-DECIDER-CHECK-BUDGET"), "{}", e);
            }
        }
    }

    #[test]
    fn labels() {
        assert!(label_is_valid("comments"));
        assert!(label_is_valid("a_b-9"));
        assert!(!label_is_valid(""));
        assert!(!label_is_valid("has space"));
        assert!(!label_is_valid(&"x".repeat(65)));
        let mut s = spec();
        s.label = "bad label".into();
        assert!(validate_spec("st", "g", &s)
            .unwrap_err()
            .starts_with("E-DECIDER-CHECK-LABEL"));
    }

    #[test]
    fn empty_text_and_ids_are_refused_with_the_field_code() {
        let edits: Vec<Edit> = vec![
            Box::new(|c| c.rule_id = String::new()),
            Box::new(|c| c.rule_id = "x".repeat(129)),
            Box::new(|c| c.rule_ref = " ".into()),
            Box::new(|c| c.question = String::new()),
            Box::new(|c| c.pass = String::new()),
            Box::new(|c| c.fail = String::new()),
            Box::new(|c| c.escape = String::new()),
        ];
        for edit in edits {
            let mut s = spec();
            edit(&mut s.criteria[0]);
            let e = validate_spec("st", "g", &s).unwrap_err();
            assert!(e.starts_with("E-DECIDER-CHECK-FIELD"), "{}", e);
        }
        let mut s = spec();
        s.criteria.clear();
        assert!(validate_spec("st", "g", &s)
            .unwrap_err()
            .starts_with("E-DECIDER-CHECK-FIELD"));
    }
}
