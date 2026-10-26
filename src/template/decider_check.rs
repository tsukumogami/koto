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
    /// The rule in a sentence, handed back on a criterion's first failure
    /// in a visit (DESIGN-doc-rule-checks-m4.md, Decision 4). Never sent to
    /// the decider and never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The rule's full text, handed back when the criterion fails again in
    /// the visit. Never sent to the decider and never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_text: Option<String>,
}

/// Longest `summary`, in bytes.
pub const MAX_SUMMARY_BYTES: usize = 300;

/// Longest `rule_text`, in bytes.
pub const MAX_RULE_TEXT_BYTES: usize = 4096;

/// How much of a failed criterion's rule a response hands back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleLevel {
    /// The one-sentence `summary`.
    Summary,
    /// The full `rule_text`.
    Full,
}

impl RuleLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleLevel::Summary => "summary",
            RuleLevel::Full => "full",
        }
    }
}

impl CheckCriterion {
    /// The rule to hand back for this criterion's `visit`-th failing
    /// evaluation in a visit (`None` when the count is unknown, read as the
    /// first): the summary on the first and the full text after it, the one
    /// declared text whenever only one is. `None` when the criterion
    /// declares neither.
    pub fn rule_for(&self, visit: Option<u64>) -> Option<(RuleLevel, &str)> {
        let summary = self.summary.as_deref().map(|t| (RuleLevel::Summary, t));
        let full = self.rule_text.as_deref().map(|t| (RuleLevel::Full, t));
        if visit.unwrap_or(1) <= 1 {
            summary.or(full)
        } else {
            full.or(summary)
        }
    }
}

/// How a check's extraction output is split into units
/// (DESIGN-doc-rule-checks-m4.md, Decision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitsMode {
    /// One JSON object per line: `key` and `text`, with optional `document`
    /// and `section`. Each unit's text is graded on its own.
    Jsonl,
}

impl UnitsMode {
    /// Every accepted spelling, in the order error messages list them.
    pub const NAMES: [&'static str; 1] = ["jsonl"];

    pub fn parse(s: &str) -> Option<UnitsMode> {
        match s {
            "jsonl" => Some(UnitsMode::Jsonl),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            UnitsMode::Jsonl => "jsonl",
        }
    }
}

/// The decider-specific half of a `decider-check` gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeciderCheckSpec {
    /// Largest slice, in redacted bytes, that is sent. In unit mode, the
    /// largest unit text.
    pub max_bytes: u32,
    /// The slice's label in the request.
    pub label: String,
    /// In declaration order: criteria are consulted in this order.
    pub criteria: Vec<CheckCriterion>,
    /// Set when the extraction prints units, each graded on its own; absent
    /// for a check that grades its whole output as one slice, which then
    /// serializes exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<UnitsMode>,
    /// In unit mode, the most units an evaluation grades: the first
    /// `max_units` printed are graded and every later one falls back with
    /// `unit_limit`, whatever its size. Absent, the check is bounded only by
    /// the per-call unit budget. Left out of the declaration hash, so
    /// changing it keeps the evidence gathered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_units: Option<u32>,
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
/// budget and label, and, only for a check in unit mode, `units=jsonl`, so
/// a check without units keeps the hash it had before units existed.
///
/// Mode, threshold, `rule_ref`, `summary` and `rule_text` are left out, so
/// changing them keeps the evidence gathered under the declaration; the two
/// texts are never sent to the decider. The fields are destructured, so
/// adding one to [`CheckCriterion`] fails to compile here until someone
/// decides whether it belongs in the hash.
pub fn declaration_hash(
    criterion: &CheckCriterion,
    command: &str,
    max_bytes: u32,
    label: &str,
    units: Option<UnitsMode>,
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
        summary: _,
        rule_text: _,
    } = criterion;
    let mut fingerprint = serde_json::json!({
        "rule_id": rule_id,
        "question": question,
        "pass": pass,
        "fail": fail,
        "escape": escape,
        "command": command,
        "max_bytes": max_bytes,
        "label": label,
    });
    if let Some(units) = units {
        fingerprint["units"] = serde_json::Value::from(units.as_str());
    }
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
        for (key, text, max) in [
            ("summary", &c.summary, MAX_SUMMARY_BYTES),
            ("rule_text", &c.rule_text, MAX_RULE_TEXT_BYTES),
        ] {
            let Some(text) = text else { continue };
            if text.trim().is_empty() || text.len() > max {
                return Err(format!(
                    "{}: {} must be 1 to {} bytes, found {}\n  \
                     remedy: write the {} within {} bytes, or omit it",
                    at("E-DECIDER-CHECK-RULE"),
                    key,
                    max,
                    text.len(),
                    key,
                    max
                ));
            }
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
            summary: None,
            rule_text: None,
        }
    }

    fn spec() -> DeciderCheckSpec {
        DeciderCheckSpec {
            max_bytes: DEFAULT_CHECK_MAX_BYTES,
            label: DEFAULT_CHECK_LABEL.into(),
            criteria: vec![criterion()],
            units: None,
            max_units: None,
        }
    }

    #[test]
    fn declaration_hash_moves_with_units_and_keeps_the_old_hash_without_them() {
        let c = criterion();
        // The fingerprint as it was before units existed.
        let old = serde_json::json!({
            "rule_id": c.rule_id, "question": c.question, "pass": c.pass,
            "fail": c.fail, "escape": c.escape, "command": "cmd",
            "max_bytes": 2560, "label": "artifact",
        });
        let old = hex::encode(Sha256::digest(serde_json::to_vec(&old).unwrap()));
        let without = declaration_hash(&c, "cmd", 2560, "artifact", None);
        assert_eq!(without, old);
        let with = declaration_hash(&c, "cmd", 2560, "artifact", Some(UnitsMode::Jsonl));
        assert_ne!(with, without);
    }

    #[test]
    fn a_spec_without_units_serializes_without_the_key() {
        let v = serde_json::to_value(spec()).unwrap();
        assert!(v.get("units").is_none(), "{}", v);
        let mut s = spec();
        s.units = Some(UnitsMode::Jsonl);
        assert_eq!(serde_json::to_value(&s).unwrap()["units"], "jsonl");
    }

    #[test]
    fn declaration_hash_ignores_mode_threshold_and_rule_ref() {
        let base = declaration_hash(&criterion(), "cmd", 2560, "artifact", None);
        let mut c = criterion();
        c.mode = CheckMode::Veto;
        c.threshold = 0.95;
        c.rule_ref = "elsewhere".into();
        c.summary = Some("A sentence.".into());
        c.rule_text = Some("The whole rule.".into());
        assert_eq!(declaration_hash(&c, "cmd", 2560, "artifact", None), base);
    }

    #[test]
    fn a_criterion_without_rule_texts_serializes_without_the_keys() {
        let v = serde_json::to_value(criterion()).unwrap();
        assert!(v.get("summary").is_none() && v.get("rule_text").is_none());
    }

    #[test]
    fn the_rule_is_the_summary_first_and_the_full_text_on_a_repeat() {
        let mut c = criterion();
        assert_eq!(c.rule_for(Some(1)), None);
        c.summary = Some("S".into());
        assert_eq!(c.rule_for(Some(1)), Some((RuleLevel::Summary, "S")));
        assert_eq!(c.rule_for(Some(3)), Some((RuleLevel::Summary, "S")));
        c.rule_text = Some("F".into());
        assert_eq!(c.rule_for(None), Some((RuleLevel::Summary, "S")));
        assert_eq!(c.rule_for(Some(2)), Some((RuleLevel::Full, "F")));
        c.summary = None;
        assert_eq!(c.rule_for(Some(1)), Some((RuleLevel::Full, "F")));
    }

    #[test]
    fn rule_texts_outside_their_bounds_are_refused_with_the_rule_code() {
        let edits: Vec<Edit> = vec![
            Box::new(|c| c.summary = Some(String::new())),
            Box::new(|c| c.summary = Some(" ".into())),
            Box::new(|c| c.summary = Some("x".repeat(MAX_SUMMARY_BYTES + 1))),
            Box::new(|c| c.rule_text = Some(String::new())),
            Box::new(|c| c.rule_text = Some("x".repeat(MAX_RULE_TEXT_BYTES + 1))),
        ];
        for edit in edits {
            let mut s = spec();
            edit(&mut s.criteria[0]);
            let e = validate_spec("st", "g", &s).unwrap_err();
            assert!(e.starts_with("E-DECIDER-CHECK-RULE"), "{}", e);
        }
        let mut s = spec();
        s.criteria[0].summary = Some("x".repeat(MAX_SUMMARY_BYTES));
        s.criteria[0].rule_text = Some("x".repeat(MAX_RULE_TEXT_BYTES));
        assert!(validate_spec("st", "g", &s).is_ok());
    }

    #[test]
    fn declaration_hash_covers_the_question_descriptions_and_check() {
        let base = declaration_hash(&criterion(), "cmd", 2560, "artifact", None);
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
            assert_ne!(declaration_hash(&c, "cmd", 2560, "artifact", None), base);
        }
        assert_ne!(
            declaration_hash(&criterion(), "cmd2", 2560, "artifact", None),
            base
        );
        assert_ne!(
            declaration_hash(&criterion(), "cmd", 2561, "artifact", None),
            base
        );
        assert_ne!(
            declaration_hash(&criterion(), "cmd", 2560, "comments", None),
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
