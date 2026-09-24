//! Turn a provider's answer into per-field outcomes and, when every field
//! qualifies, candidate evidence.
//!
//! Pure: no I/O and no settings. The caller supplies the effective mode of
//! every declared value; this module never derives one. Thresholds and the
//! template's per-value modes come from the compiled declaration, which has
//! every default resolved, so nothing here hard-codes a threshold or a mode
//! default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::template::decider::DeciderMode;

use super::request::{DeclaredField, DeclaredKind};
use super::types::{
    check_choice_probabilities, check_proposition, Answer, DeciderError, DecisionResponse,
};

/// The effective mode of each declared value, as computed by the caller.
///
/// Keyed by field, then by value (`"true"`/`"false"` for a boolean). The
/// caller fills it with `off`, `shadow`, or `auto`; a supplied `never` is
/// honored as `never`. A value with no entry is treated as `shadow`, never
/// as `auto`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveModes(BTreeMap<String, BTreeMap<String, DeciderMode>>);

impl EffectiveModes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the effective mode of `field`'s `value`.
    pub fn set(&mut self, field: impl Into<String>, value: impl Into<String>, mode: DeciderMode) {
        self.0
            .entry(field.into())
            .or_default()
            .insert(value.into(), mode);
    }

    /// Builder form of [`EffectiveModes::set`].
    pub fn with(mut self, field: &str, value: &str, mode: DeciderMode) -> Self {
        self.set(field, value, mode);
        self
    }

    /// The supplied mode, or `shadow` when none was supplied.
    pub fn get(&self, field: &str, value: &str) -> DeciderMode {
        self.0
            .get(field)
            .and_then(|m| m.get(value))
            .copied()
            .unwrap_or(DeciderMode::Shadow)
    }
}

/// A field's outcome, in precedence order: `escape` beats
/// `below_threshold`, which beats `never`, which beats `shadow`;
/// `qualified` only when none of the others holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOutcome {
    /// The winner is a declared value in `auto` at or above its threshold.
    Qualified,
    /// The winner is confident but its effective mode isn't `auto`.
    Shadow,
    /// The winner is confident but the template marks it `never`.
    Never,
    /// The winner's probability is under its threshold.
    BelowThreshold,
    /// No declared value won: the escape, a tie, or a boolean where neither
    /// or both sides met their thresholds.
    Escape,
}

impl FieldOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            FieldOutcome::Qualified => "qualified",
            FieldOutcome::Shadow => "shadow",
            FieldOutcome::Never => "never",
            FieldOutcome::BelowThreshold => "below_threshold",
            FieldOutcome::Escape => "escape",
        }
    }
}

/// The evaluation of one declared field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldEvaluation {
    pub field: String,
    /// Unrounded probability per value. For a boolean, `true` and `false`
    /// (the latter as `1 - P(true)`); for an enum, every value and the
    /// escape.
    pub probabilities: BTreeMap<String, f64>,
    /// The winning value: the enum value (or escape) with the strictly
    /// highest probability, or the one boolean side that met its
    /// threshold. `None` on a tie or when a boolean has no single winner.
    pub winning: Option<String>,
    /// The winner's probability (for no winner, the highest probability).
    /// Never the provider's own confidence figure.
    pub confidence: f64,
    /// The winning declared value's threshold; `None` when the escape won
    /// or there is no winner.
    pub threshold: Option<f64>,
    /// `confidence >= threshold` on the unrounded numbers; false when
    /// there's no threshold.
    pub at_threshold: bool,
    pub outcome: FieldOutcome,
}

/// The evaluation of one consultation.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// One entry per declared field, in the order given.
    pub fields: Vec<FieldEvaluation>,
    /// Evidence to submit, present only when every field is `qualified`:
    /// enum values as JSON strings, boolean values as JSON booleans.
    pub candidate: Option<Map<String, Value>>,
}

/// Evaluate `response` against the state's declared `fields`.
///
/// `modes` holds the caller's effective mode for each declared value (see
/// [`EffectiveModes`]). An answer missing for a declared field, or given
/// for an undeclared one, is `mismatched`; an answer of the wrong kind is
/// `malformed`; probabilities are re-checked with the same rules the
/// provider client applies, so a scripted or buggy provider can't slip an
/// undeclared value through.
pub fn evaluate(
    fields: &[DeclaredField<'_>],
    response: &DecisionResponse,
    modes: &EffectiveModes,
) -> Result<Evaluation, DeciderError> {
    if response
        .answers
        .keys()
        .any(|k| !fields.iter().any(|f| f.name == k))
    {
        return Err(DeciderError::mismatched(
            "answer given for a field that was not asked",
        ));
    }

    let mut results = Vec::with_capacity(fields.len());
    for field in fields {
        let answer = response
            .answers
            .get(field.name)
            .ok_or_else(|| DeciderError::mismatched("answer missing for an asked field"))?;
        let result = match (field.kind, answer) {
            (DeclaredKind::Enum { values }, Answer::Choice { probabilities, .. }) => {
                evaluate_enum(field, values, probabilities, modes)?
            }
            (DeclaredKind::Boolean, Answer::Proposition { p_true }) => {
                evaluate_boolean(field, *p_true, modes)?
            }
            _ => {
                return Err(DeciderError::malformed(
                    "answer kind does not match the question",
                ))
            }
        };
        results.push(result);
    }

    let candidate = if results.iter().all(|r| r.outcome == FieldOutcome::Qualified) {
        let mut map = Map::new();
        for (field, result) in fields.iter().zip(&results) {
            let Some(value) = &result.winning else {
                return Err(DeciderError::malformed("qualified field has no winner"));
            };
            let json = match field.kind {
                DeclaredKind::Enum { .. } => Value::String(value.clone()),
                DeclaredKind::Boolean => Value::Bool(value == "true"),
            };
            map.insert(field.name.to_string(), json);
        }
        Some(map)
    } else {
        None
    };

    Ok(Evaluation {
        fields: results,
        candidate,
    })
}

/// The outcome for a declared (non-escape) winner.
fn classify(at_threshold: bool, template: DeciderMode, supplied: DeciderMode) -> FieldOutcome {
    if !at_threshold {
        FieldOutcome::BelowThreshold
    } else if template == DeciderMode::Never || supplied == DeciderMode::Never {
        FieldOutcome::Never
    } else if supplied == DeciderMode::Auto {
        FieldOutcome::Qualified
    } else {
        FieldOutcome::Shadow
    }
}

fn evaluate_enum(
    field: &DeclaredField<'_>,
    values: &[String],
    probabilities: &BTreeMap<String, f64>,
    modes: &EffectiveModes,
) -> Result<FieldEvaluation, DeciderError> {
    let escape = field
        .decider
        .escape
        .as_ref()
        .ok_or_else(|| DeciderError::malformed("enum declaration has no escape"))?;
    let keys: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(escape.value.as_str()))
        .collect();
    check_choice_probabilities(&keys, probabilities)?;

    // Strictly highest probability wins; a tie at the top has no winner.
    let mut best: Option<(&str, f64)> = None;
    let mut tied = false;
    for key in &keys {
        let p = probabilities[*key];
        match best {
            None => best = Some((key, p)),
            Some((_, top)) if p > top => {
                best = Some((key, p));
                tied = false;
            }
            Some((_, top)) if p == top => tied = true,
            Some(_) => {}
        }
    }
    let (winner, confidence) = best.ok_or_else(|| DeciderError::malformed("no values"))?;

    let escape_result = |winning: Option<String>| FieldEvaluation {
        field: field.name.to_string(),
        probabilities: probabilities.clone(),
        winning,
        confidence,
        threshold: None,
        at_threshold: false,
        outcome: FieldOutcome::Escape,
    };
    if tied {
        return Ok(escape_result(None));
    }
    if winner == escape.value {
        return Ok(escape_result(Some(winner.to_string())));
    }

    let answer = field
        .decider
        .answers
        .get(winner)
        .ok_or_else(|| DeciderError::malformed("declaration has no answer for a value"))?;
    let at_threshold = confidence >= answer.threshold;
    Ok(FieldEvaluation {
        field: field.name.to_string(),
        probabilities: probabilities.clone(),
        winning: Some(winner.to_string()),
        confidence,
        threshold: Some(answer.threshold),
        at_threshold,
        outcome: classify(at_threshold, answer.mode, modes.get(field.name, winner)),
    })
}

fn evaluate_boolean(
    field: &DeclaredField<'_>,
    p_true: f64,
    modes: &EffectiveModes,
) -> Result<FieldEvaluation, DeciderError> {
    check_proposition(p_true)?;
    let p_false = 1.0 - p_true;
    let answers = &field.decider.answers;
    let (Some(yes), Some(no)) = (answers.get("true"), answers.get("false")) else {
        return Err(DeciderError::malformed(
            "boolean declaration lacks a true or false answer",
        ));
    };
    let mut probabilities = BTreeMap::new();
    probabilities.insert("true".to_string(), p_true);
    probabilities.insert("false".to_string(), p_false);

    let true_meets = p_true >= yes.threshold;
    let false_meets = p_false >= no.threshold;
    let (value, confidence, answer) = match (true_meets, false_meets) {
        (true, false) => ("true", p_true, yes),
        (false, true) => ("false", p_false, no),
        // Neither side, or both, met its threshold.
        _ => {
            return Ok(FieldEvaluation {
                field: field.name.to_string(),
                probabilities,
                winning: None,
                confidence: p_true.max(p_false),
                threshold: None,
                at_threshold: false,
                outcome: FieldOutcome::Escape,
            })
        }
    };
    Ok(FieldEvaluation {
        field: field.name.to_string(),
        probabilities,
        winning: Some(value.to_string()),
        confidence,
        threshold: Some(answer.threshold),
        at_threshold: true,
        outcome: classify(true, answer.mode, modes.get(field.name, value)),
    })
}

#[cfg(test)]
mod tests {
    use super::super::request::test_fixtures::*;
    use super::super::types::ErrorClass;
    use super::*;
    use crate::template::decider::DEFAULT_THRESHOLD;
    use crate::template::types::FieldSchema;

    fn enum_answer(pairs: &[(&str, f64)], provider_confidence: Option<f64>) -> Answer {
        Answer::Choice {
            probabilities: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            provider_confidence,
        }
    }

    fn response(answers: Vec<(&str, Answer)>) -> DecisionResponse {
        DecisionResponse {
            model: "test".to_string(),
            answers: answers
                .into_iter()
                .map(|(k, a)| (k.to_string(), a))
                .collect(),
        }
    }

    fn all(field: &str, values: &[&str], mode: DeciderMode) -> EffectiveModes {
        let mut m = EffectiveModes::new();
        for v in values {
            m.set(field, *v, mode);
        }
        m
    }

    fn eval_enum(
        schema: &FieldSchema,
        pairs: &[(&str, f64)],
        modes: &EffectiveModes,
    ) -> FieldEvaluation {
        let fields = vec![DeclaredField::from_schema("verdict", schema).unwrap()];
        let r = response(vec![("verdict", enum_answer(pairs, None))]);
        evaluate(&fields, &r, modes).unwrap().fields.remove(0)
    }

    const PE: [&str; 2] = ["proceed", "exit"];

    #[test]
    fn enum_winner_is_the_highest_probability() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.7), ("exit", 0.2), ("unclear", 0.1)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.winning.as_deref(), Some("proceed"));
        assert_eq!(f.confidence, 0.7);
        assert_eq!(f.threshold, Some(DEFAULT_THRESHOLD));
        assert!(!f.at_threshold);
        assert_eq!(f.outcome, FieldOutcome::BelowThreshold);
    }

    #[test]
    fn enum_tie_is_the_escape() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.45), ("exit", 0.45), ("unclear", 0.1)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.winning, None);
        assert_eq!(f.outcome, FieldOutcome::Escape);
        assert!(!f.at_threshold);
    }

    #[test]
    fn enum_escape_highest_is_the_escape() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.1), ("exit", 0.1), ("unclear", 0.8)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.winning.as_deref(), Some("unclear"));
        assert_eq!(f.outcome, FieldOutcome::Escape);
        assert_eq!(f.threshold, None);
    }

    #[test]
    fn confidence_ignores_provider_confidence() {
        let s = enum_schema(DeciderMode::Auto);
        let fields = vec![DeclaredField::from_schema("verdict", &s).unwrap()];
        let r = response(vec![(
            "verdict",
            enum_answer(
                &[("proceed", 0.6), ("exit", 0.3), ("unclear", 0.1)],
                Some(0.99),
            ),
        )]);
        let e = evaluate(&fields, &r, &all("verdict", &PE, DeciderMode::Auto)).unwrap();
        assert_eq!(e.fields[0].confidence, 0.6);
        assert_eq!(e.fields[0].outcome, FieldOutcome::BelowThreshold);
        assert!(e.candidate.is_none());
    }

    fn bool_eval(p_true: f64, threshold: f64) -> FieldEvaluation {
        let s = bool_schema(DeciderMode::Auto, threshold);
        let fields = vec![DeclaredField::from_schema("ready", &s).unwrap()];
        let r = response(vec![("ready", Answer::Proposition { p_true })]);
        let modes = all("ready", &["true", "false"], DeciderMode::Auto);
        evaluate(&fields, &r, &modes).unwrap().fields.remove(0)
    }

    #[test]
    fn boolean_winners_and_escapes() {
        let t = bool_eval(0.95, 0.9);
        assert_eq!(t.winning.as_deref(), Some("true"));
        assert_eq!(t.outcome, FieldOutcome::Qualified);
        assert_eq!(t.confidence, 0.95);

        let f = bool_eval(0.05, 0.9);
        assert_eq!(f.winning.as_deref(), Some("false"));
        assert_eq!(f.outcome, FieldOutcome::Qualified);

        let mid = bool_eval(0.5, 0.9);
        assert_eq!(mid.winning, None);
        assert_eq!(mid.outcome, FieldOutcome::Escape);

        // Both sides qualify at 0.5 / 0.5.
        let both = bool_eval(0.5, 0.5);
        assert_eq!(both.winning, None);
        assert_eq!(both.outcome, FieldOutcome::Escape);
    }

    #[test]
    fn boolean_candidate_is_a_json_boolean() {
        let s = bool_schema(DeciderMode::Auto, 0.9);
        let fields = vec![DeclaredField::from_schema("ready", &s).unwrap()];
        let r = response(vec![("ready", Answer::Proposition { p_true: 0.05 })]);
        let modes = all("ready", &["true", "false"], DeciderMode::Auto);
        let e = evaluate(&fields, &r, &modes).unwrap();
        assert_eq!(e.candidate.unwrap()["ready"], Value::Bool(false));
    }

    #[test]
    fn threshold_boundary_is_inclusive_and_exact() {
        let mut s = enum_schema(DeciderMode::Auto);
        let t = 0.85;
        s.decider
            .as_mut()
            .unwrap()
            .answers
            .get_mut("proceed")
            .unwrap()
            .threshold = t;
        let modes = all("verdict", &PE, DeciderMode::Auto);

        let at = eval_enum(
            &s,
            &[("proceed", t), ("exit", 0.1), ("unclear", 1.0 - t - 0.1)],
            &modes,
        );
        assert!(at.at_threshold);
        assert_eq!(at.outcome, FieldOutcome::Qualified);

        let below_p = f64::from_bits(t.to_bits() - 1);
        assert!(below_p < t);
        let below = eval_enum(
            &s,
            &[("proceed", below_p), ("exit", 0.1), ("unclear", 0.05)],
            &modes,
        );
        assert!(!below.at_threshold);
        assert_eq!(below.outcome, FieldOutcome::BelowThreshold);
    }

    #[test]
    fn at_threshold_uses_unrounded_numbers() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.89996), ("exit", 0.1), ("unclear", 0.00004)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert!(!f.at_threshold);
        assert_eq!(f.outcome, FieldOutcome::BelowThreshold);
    }

    // Outcome precedence, one test per pair.

    #[test]
    fn escape_beats_never() {
        let s = enum_schema(DeciderMode::Never);
        let f = eval_enum(
            &s,
            &[("proceed", 0.02), ("exit", 0.02), ("unclear", 0.96)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.outcome, FieldOutcome::Escape);
    }

    #[test]
    fn below_threshold_beats_never() {
        let s = enum_schema(DeciderMode::Never);
        let f = eval_enum(
            &s,
            &[("proceed", 0.6), ("exit", 0.3), ("unclear", 0.1)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.outcome, FieldOutcome::BelowThreshold);
    }

    #[test]
    fn never_beats_a_supplied_auto() {
        let s = enum_schema(DeciderMode::Never);
        let f = eval_enum(
            &s,
            &[("proceed", 0.95), ("exit", 0.03), ("unclear", 0.02)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.outcome, FieldOutcome::Never);
    }

    #[test]
    fn confident_winner_in_supplied_shadow_is_shadow() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.95), ("exit", 0.03), ("unclear", 0.02)],
            &all("verdict", &PE, DeciderMode::Shadow),
        );
        assert_eq!(f.outcome, FieldOutcome::Shadow);
    }

    #[test]
    fn confident_winner_in_supplied_auto_is_qualified() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.95), ("exit", 0.03), ("unclear", 0.02)],
            &all("verdict", &PE, DeciderMode::Auto),
        );
        assert_eq!(f.outcome, FieldOutcome::Qualified);
        assert!(f.at_threshold);
    }

    #[test]
    fn supplied_mode_alone_decides_shadow_or_qualified() {
        let s = enum_schema(DeciderMode::Auto);
        let p = [("proceed", 0.95), ("exit", 0.03), ("unclear", 0.02)];
        assert_eq!(
            eval_enum(&s, &p, &all("verdict", &PE, DeciderMode::Shadow)).outcome,
            FieldOutcome::Shadow
        );
        assert_eq!(
            eval_enum(&s, &p, &all("verdict", &PE, DeciderMode::Auto)).outcome,
            FieldOutcome::Qualified
        );
        assert_eq!(
            eval_enum(&s, &p, &all("verdict", &PE, DeciderMode::Off)).outcome,
            FieldOutcome::Shadow
        );
    }

    #[test]
    fn missing_mode_is_shadow_not_auto() {
        let s = enum_schema(DeciderMode::Auto);
        let f = eval_enum(
            &s,
            &[("proceed", 0.95), ("exit", 0.03), ("unclear", 0.02)],
            &EffectiveModes::new(),
        );
        assert_eq!(f.outcome, FieldOutcome::Shadow);
        assert_eq!(EffectiveModes::new().get("x", "y"), DeciderMode::Shadow);
    }

    #[test]
    fn default_threshold_comes_from_the_declaration() {
        // The fixture's answers were compiled with DEFAULT_THRESHOLD (0.9).
        let s = enum_schema(DeciderMode::Auto);
        let modes = all("verdict", &PE, DeciderMode::Auto);
        let at = eval_enum(
            &s,
            &[("proceed", 0.9), ("exit", 0.05), ("unclear", 0.05)],
            &modes,
        );
        assert_eq!(at.threshold, Some(0.9));
        assert_eq!(at.outcome, FieldOutcome::Qualified);
        let below = eval_enum(
            &s,
            &[("proceed", 0.89), ("exit", 0.06), ("unclear", 0.05)],
            &modes,
        );
        assert_eq!(below.outcome, FieldOutcome::BelowThreshold);
    }

    fn two_fields() -> (FieldSchema, FieldSchema) {
        (
            enum_schema(DeciderMode::Auto),
            bool_schema(DeciderMode::Auto, 0.9),
        )
    }

    fn two_modes() -> EffectiveModes {
        all("verdict", &PE, DeciderMode::Auto)
            .with("ready", "true", DeciderMode::Auto)
            .with("ready", "false", DeciderMode::Auto)
    }

    #[test]
    fn all_or_nothing_when_one_field_is_below_threshold() {
        let (e, b) = two_fields();
        let fields = vec![
            DeclaredField::from_schema("ready", &b).unwrap(),
            DeclaredField::from_schema("verdict", &e).unwrap(),
        ];
        let r = response(vec![
            ("ready", Answer::Proposition { p_true: 0.97 }),
            (
                "verdict",
                enum_answer(&[("proceed", 0.6), ("exit", 0.3), ("unclear", 0.1)], None),
            ),
        ]);
        let ev = evaluate(&fields, &r, &two_modes()).unwrap();
        assert!(ev.candidate.is_none());
        assert_eq!(ev.fields[0].outcome, FieldOutcome::Qualified);
        assert_eq!(ev.fields[1].outcome, FieldOutcome::BelowThreshold);
    }

    #[test]
    fn both_qualified_gives_a_typed_candidate() {
        let (e, b) = two_fields();
        let fields = vec![
            DeclaredField::from_schema("ready", &b).unwrap(),
            DeclaredField::from_schema("verdict", &e).unwrap(),
        ];
        let r = response(vec![
            ("ready", Answer::Proposition { p_true: 0.97 }),
            (
                "verdict",
                enum_answer(
                    &[("proceed", 0.05), ("exit", 0.93), ("unclear", 0.02)],
                    None,
                ),
            ),
        ]);
        let ev = evaluate(&fields, &r, &two_modes()).unwrap();
        let c = ev.candidate.expect("both qualified");
        assert_eq!(c.len(), 2);
        assert_eq!(c["ready"], Value::Bool(true));
        assert_eq!(c["verdict"], Value::String("exit".to_string()));
    }

    #[test]
    fn missing_answer_is_mismatched() {
        let (e, b) = two_fields();
        let fields = vec![
            DeclaredField::from_schema("ready", &b).unwrap(),
            DeclaredField::from_schema("verdict", &e).unwrap(),
        ];
        let r = response(vec![("ready", Answer::Proposition { p_true: 0.97 })]);
        let err = evaluate(&fields, &r, &two_modes()).unwrap_err();
        assert_eq!(err.class, ErrorClass::Mismatched);
    }

    #[test]
    fn extra_answer_is_mismatched() {
        let (_, b) = two_fields();
        let fields = vec![DeclaredField::from_schema("ready", &b).unwrap()];
        let r = response(vec![
            ("ready", Answer::Proposition { p_true: 0.97 }),
            ("other", Answer::Proposition { p_true: 0.97 }),
        ]);
        let err = evaluate(&fields, &r, &two_modes()).unwrap_err();
        assert_eq!(err.class, ErrorClass::Mismatched);
    }

    #[test]
    fn wrong_kind_is_malformed() {
        let (e, b) = two_fields();
        let fields = vec![
            DeclaredField::from_schema("ready", &b).unwrap(),
            DeclaredField::from_schema("verdict", &e).unwrap(),
        ];
        let r = response(vec![
            ("ready", enum_answer(&[("true", 0.9), ("false", 0.1)], None)),
            ("verdict", Answer::Proposition { p_true: 0.9 }),
        ]);
        let err = evaluate(&fields, &r, &two_modes()).unwrap_err();
        assert_eq!(err.class, ErrorClass::Malformed);
    }

    #[test]
    fn undeclared_value_or_bad_probability_is_rejected() {
        let s = enum_schema(DeciderMode::Auto);
        let fields = vec![DeclaredField::from_schema("verdict", &s).unwrap()];
        let modes = all("verdict", &PE, DeciderMode::Auto);
        let r = response(vec![(
            "verdict",
            enum_answer(&[("proceed", 0.9), ("exit", 0.05), ("merge", 0.05)], None),
        )]);
        assert_eq!(
            evaluate(&fields, &r, &modes).unwrap_err().class,
            ErrorClass::Mismatched
        );
        let r = response(vec![(
            "verdict",
            enum_answer(
                &[("proceed", f64::NAN), ("exit", 0.05), ("unclear", 0.05)],
                None,
            ),
        )]);
        assert_eq!(
            evaluate(&fields, &r, &modes).unwrap_err().class,
            ErrorClass::Malformed
        );
        let b = bool_schema(DeciderMode::Auto, 0.9);
        let fields = vec![DeclaredField::from_schema("ready", &b).unwrap()];
        let r = response(vec![("ready", Answer::Proposition { p_true: 1.5 })]);
        assert_eq!(
            evaluate(&fields, &r, &modes).unwrap_err().class,
            ErrorClass::Malformed
        );
    }

    #[test]
    fn field_outcome_names() {
        for (o, n) in [
            (FieldOutcome::Qualified, "qualified"),
            (FieldOutcome::Shadow, "shadow"),
            (FieldOutcome::Never, "never"),
            (FieldOutcome::BelowThreshold, "below_threshold"),
            (FieldOutcome::Escape, "escape"),
        ] {
            assert_eq!(o.as_str(), n);
            assert_eq!(serde_json::to_string(&o).unwrap(), format!("\"{}\"", n));
        }
    }
}
