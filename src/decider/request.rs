//! Build a provider-neutral [`DecisionRequest`] from a state's compiled
//! declarations and the inputs the caller already assembled.
//!
//! Pure: no I/O, no settings, no environment. Budget enforcement and
//! unset-key detection belong to the caller, which assembles the inputs
//! before calling [`build_request`]; this module only refuses a request
//! with a gap in it.

use std::collections::BTreeMap;
use std::fmt;

use crate::template::decider::FieldDecider;
use crate::template::types::FieldSchema;

use super::types::{AnswerOption, DecisionRequest, LabelledInput, Question, QuestionKind};

/// The value type of a declared field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKind<'a> {
    /// An `enum` field, with its `values` in declaration order.
    Enum { values: &'a [String] },
    /// A `boolean` field.
    Boolean,
}

/// One `accepts` field that carries a `decider` block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeclaredField<'a> {
    pub name: &'a str,
    /// The field `description`: the question (enum) or proposition
    /// (boolean).
    pub description: &'a str,
    pub kind: DeclaredKind<'a>,
    pub decider: &'a FieldDecider,
}

impl<'a> DeclaredField<'a> {
    /// View a compiled field as a declaration. `None` when the field has no
    /// `decider` block or isn't an `enum` or `boolean` (the compiler
    /// rejects a block on any other type).
    pub fn from_schema(name: &'a str, schema: &'a FieldSchema) -> Option<Self> {
        let decider = schema.decider.as_ref()?;
        let kind = match schema.field_type.as_str() {
            "enum" => DeclaredKind::Enum {
                values: &schema.values,
            },
            "boolean" => DeclaredKind::Boolean,
            _ => return None,
        };
        Some(DeclaredField {
            name,
            description: &schema.description,
            kind,
            decider,
        })
    }
}

/// Every declared field of a state's `accepts`, in declaration order.
pub fn declared_fields(accepts: &BTreeMap<String, FieldSchema>) -> Vec<DeclaredField<'_>> {
    accepts
        .iter()
        .filter_map(|(name, schema)| DeclaredField::from_schema(name, schema))
        .collect()
}

/// Why no request could be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildRequestError {
    /// A declared input label has no assembled content.
    MissingInput { label: String },
    /// An enum value has no answer description in its declaration.
    MissingAnswer { field: String, value: String },
    /// An enum declaration has no escape.
    MissingEscape { field: String },
    /// The state declares no field.
    NoFields,
}

impl fmt::Display for BuildRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildRequestError::MissingInput { label } => {
                write!(f, "decider input {:?} was not assembled", label)
            }
            BuildRequestError::MissingAnswer { field, value } => write!(
                f,
                "field {:?} declares no decider answer for value {:?}",
                field, value
            ),
            BuildRequestError::MissingEscape { field } => {
                write!(f, "enum field {:?} declares no decider escape", field)
            }
            BuildRequestError::NoFields => write!(f, "the state declares no decider field"),
        }
    }
}

impl std::error::Error for BuildRequestError {}

/// Input contents keyed by label: the seam between assembling a
/// consultation's inputs and building its request.
///
/// The runtime assembles one from the context store and the variable
/// bindings; the fixture runner builds one from a fixture line's texts
/// with [`AssembledInputs::from_texts`]. Either way [`build_request`]
/// sees the same map, so both send the same request for the same texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssembledInputs(BTreeMap<String, String>);

impl AssembledInputs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `content` under `label`.
    pub fn insert(&mut self, label: impl Into<String>, content: impl Into<String>) {
        self.0.insert(label.into(), content.into());
    }

    pub fn contains(&self, label: &str) -> bool {
        self.0.contains_key(label)
    }

    /// The label-to-content map [`build_request`] takes.
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Inputs supplied as texts by label, checked against `fields`' declared
    /// inputs: every declared label present, no other label, and each text
    /// within its label's `max_bytes`.
    pub fn from_texts(
        fields: &[DeclaredField<'_>],
        texts: BTreeMap<String, String>,
    ) -> Result<Self, InputTextError> {
        let mut budgets: Vec<(&str, u32)> = Vec::new();
        for field in fields {
            for input in &field.decider.inputs {
                if !budgets.iter().any(|(l, _)| *l == input.label) {
                    budgets.push((&input.label, input.max_bytes));
                }
            }
        }
        if let Some(extra) = texts.keys().find(|k| !budgets.iter().any(|(l, _)| l == k)) {
            return Err(InputTextError::Extra {
                label: extra.clone(),
            });
        }
        for (label, max_bytes) in &budgets {
            let Some(text) = texts.get(*label) else {
                return Err(InputTextError::Missing {
                    label: label.to_string(),
                });
            };
            if text.len() > *max_bytes as usize {
                return Err(InputTextError::OverBudget {
                    label: label.to_string(),
                    bytes: text.len(),
                    max_bytes: *max_bytes,
                });
            }
        }
        Ok(AssembledInputs(texts))
    }
}

/// Why supplied input texts don't match a declaration. Never carries
/// input content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputTextError {
    /// A declared label has no text.
    Missing { label: String },
    /// A label the declaration doesn't read.
    Extra { label: String },
    /// A text over its label's `max_bytes`.
    OverBudget {
        label: String,
        bytes: usize,
        max_bytes: u32,
    },
}

impl fmt::Display for InputTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputTextError::Missing { label } => {
                write!(f, "input label {:?} is declared but missing", label)
            }
            InputTextError::Extra { label } => {
                write!(f, "input label {:?} is not declared", label)
            }
            InputTextError::OverBudget {
                label,
                bytes,
                max_bytes,
            } => write!(
                f,
                "input {:?} is {} bytes, over its max_bytes of {}",
                label, bytes, max_bytes
            ),
        }
    }
}

impl std::error::Error for InputTextError {}

/// Build the request for one consultation.
///
/// `fields` are the state's declared fields in declaration order (see
/// [`declared_fields`]). `inputs` maps each declared input label to its
/// assembled content. The request has one [`Question`] per field, in the
/// order given, and one [`LabelledInput`] per distinct label, in the order
/// the declarations list them (a label shared by two fields appears once,
/// at its first use). Contents are copied as given.
pub fn build_request(
    fields: &[DeclaredField<'_>],
    inputs: &BTreeMap<String, String>,
) -> Result<DecisionRequest, BuildRequestError> {
    if fields.is_empty() {
        return Err(BuildRequestError::NoFields);
    }

    let mut questions = Vec::with_capacity(fields.len());
    for field in fields {
        let kind = match field.kind {
            DeclaredKind::Enum { values } => {
                let mut options = Vec::with_capacity(values.len());
                for value in values {
                    let answer = field.decider.answers.get(value).ok_or_else(|| {
                        BuildRequestError::MissingAnswer {
                            field: field.name.to_string(),
                            value: value.clone(),
                        }
                    })?;
                    options.push(AnswerOption {
                        value: value.clone(),
                        description: answer.description.clone(),
                    });
                }
                let escape = field.decider.escape.as_ref().ok_or_else(|| {
                    BuildRequestError::MissingEscape {
                        field: field.name.to_string(),
                    }
                })?;
                QuestionKind::Choice {
                    question: field.description.to_string(),
                    options,
                    escape: AnswerOption {
                        value: escape.value.clone(),
                        description: escape.description.clone(),
                    },
                }
            }
            DeclaredKind::Boolean => QuestionKind::Proposition {
                proposition: field.description.to_string(),
            },
        };
        questions.push(Question {
            field: field.name.to_string(),
            kind,
        });
    }

    let mut labelled: Vec<LabelledInput> = Vec::new();
    for field in fields {
        for input in &field.decider.inputs {
            if labelled.iter().any(|l| l.label == input.label) {
                continue;
            }
            let content =
                inputs
                    .get(&input.label)
                    .ok_or_else(|| BuildRequestError::MissingInput {
                        label: input.label.clone(),
                    })?;
            labelled.push(LabelledInput {
                label: input.label.clone(),
                content: content.clone(),
            });
        }
    }

    Ok(DecisionRequest {
        questions,
        inputs: labelled,
    })
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    //! Declarations shared by the request and evaluate tests.

    use std::collections::BTreeMap;

    use crate::template::decider::{
        DeciderAnswer, DeciderEscape, DeciderInput, DeciderInputSource, DeciderMode, FieldDecider,
        DEFAULT_MAX_BYTES, DEFAULT_THRESHOLD,
    };
    use crate::template::types::FieldSchema;

    pub fn answer(description: &str, mode: DeciderMode, threshold: f64) -> DeciderAnswer {
        DeciderAnswer {
            description: description.to_string(),
            mode,
            threshold,
        }
    }

    pub fn input(label: &str, key: &str) -> DeciderInput {
        DeciderInput {
            source: DeciderInputSource::Context(key.to_string()),
            label: label.to_string(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// `verdict`: enum `proceed`/`exit` with escape `unclear`, both values
    /// in `mode` at the default threshold.
    pub fn enum_schema(mode: DeciderMode) -> FieldSchema {
        let mut answers = BTreeMap::new();
        answers.insert(
            "proceed".to_string(),
            answer("Clear and scoped.", mode, DEFAULT_THRESHOLD),
        );
        answers.insert(
            "exit".to_string(),
            answer("Vague or contradictory.", mode, DEFAULT_THRESHOLD),
        );
        FieldSchema {
            field_type: "enum".to_string(),
            required: true,
            // Deliberately not alphabetical, to prove `values` order wins.
            values: vec!["proceed".to_string(), "exit".to_string()],
            description: "Is the item clear enough to implement?".to_string(),
            decider: Some(FieldDecider {
                answers,
                escape: Some(DeciderEscape {
                    value: "unclear".to_string(),
                    description: "The inputs don't say.".to_string(),
                }),
                inputs: vec![
                    input("outline_item", "outline.md"),
                    input("plan", "plan.md"),
                ],
            }),
        }
    }

    /// `ready`: boolean with `true`/`false` in `mode` at `threshold`.
    pub fn bool_schema(mode: DeciderMode, threshold: f64) -> FieldSchema {
        let mut answers = BTreeMap::new();
        answers.insert("true".to_string(), answer("It is ready.", mode, threshold));
        answers.insert(
            "false".to_string(),
            answer("It is not ready.", mode, threshold),
        );
        FieldSchema {
            field_type: "boolean".to_string(),
            required: true,
            values: vec![],
            description: "The change is ready to merge.".to_string(),
            decider: Some(FieldDecider {
                answers,
                escape: None,
                inputs: vec![input("plan", "plan.md"), input("diff_stat", "diff.txt")],
            }),
        }
    }

    pub fn inputs() -> BTreeMap<String, String> {
        [
            ("outline_item", "Add the decider trait."),
            ("plan", "PLAN body"),
            ("diff_stat", "3 files changed"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixtures::*;
    use super::*;
    use crate::template::decider::DeciderMode;

    #[test]
    fn enum_field_becomes_a_choice_in_values_order() {
        let schema = enum_schema(DeciderMode::Shadow);
        let fields = vec![DeclaredField::from_schema("verdict", &schema).unwrap()];
        let req = build_request(&fields, &inputs()).unwrap();
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.questions[0].field, "verdict");
        match &req.questions[0].kind {
            QuestionKind::Choice {
                question,
                options,
                escape,
            } => {
                assert_eq!(question, "Is the item clear enough to implement?");
                let got: Vec<(&str, &str)> = options
                    .iter()
                    .map(|o| (o.value.as_str(), o.description.as_str()))
                    .collect();
                assert_eq!(
                    got,
                    vec![
                        ("proceed", "Clear and scoped."),
                        ("exit", "Vague or contradictory.")
                    ]
                );
                assert_eq!(escape.value, "unclear");
                assert_eq!(escape.description, "The inputs don't say.");
            }
            other => panic!("expected a choice, got {:?}", other),
        }
        let labels: Vec<&str> = req.inputs.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["outline_item", "plan"]);
        assert_eq!(req.inputs[0].content, "Add the decider trait.");
    }

    #[test]
    fn boolean_field_becomes_a_proposition() {
        let schema = bool_schema(DeciderMode::Shadow, 0.9);
        let fields = vec![DeclaredField::from_schema("ready", &schema).unwrap()];
        let req = build_request(&fields, &inputs()).unwrap();
        assert_eq!(
            req.questions[0].kind,
            QuestionKind::Proposition {
                proposition: "The change is ready to merge.".to_string()
            }
        );
        let labels: Vec<&str> = req.inputs.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["plan", "diff_stat"]);
    }

    #[test]
    fn questions_follow_field_order_and_shared_labels_appear_once() {
        let mut accepts = BTreeMap::new();
        accepts.insert("verdict".to_string(), enum_schema(DeciderMode::Shadow));
        accepts.insert("ready".to_string(), bool_schema(DeciderMode::Shadow, 0.9));
        accepts.insert(
            "notes".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
                decider: None,
            },
        );
        let fields = declared_fields(&accepts);
        let names: Vec<&str> = fields.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["ready", "verdict"]);
        let req = build_request(&fields, &inputs()).unwrap();
        let q: Vec<&str> = req.questions.iter().map(|q| q.field.as_str()).collect();
        assert_eq!(q, vec!["ready", "verdict"]);
        let labels: Vec<&str> = req.inputs.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["plan", "diff_stat", "outline_item"]);
    }

    #[test]
    fn missing_input_is_an_error_not_a_gap() {
        let schema = enum_schema(DeciderMode::Shadow);
        let fields = vec![DeclaredField::from_schema("verdict", &schema).unwrap()];
        let mut partial = inputs();
        partial.remove("plan");
        assert_eq!(
            build_request(&fields, &partial).unwrap_err(),
            BuildRequestError::MissingInput {
                label: "plan".to_string()
            }
        );
    }

    #[test]
    fn same_declaration_and_inputs_give_an_equal_request() {
        let a = enum_schema(DeciderMode::Auto);
        let b = bool_schema(DeciderMode::Shadow, 0.8);
        let fields = vec![
            DeclaredField::from_schema("ready", &b).unwrap(),
            DeclaredField::from_schema("verdict", &a).unwrap(),
        ];
        let one = build_request(&fields, &inputs()).unwrap();
        let two = build_request(&fields, &inputs()).unwrap();
        assert_eq!(one, two);
        assert_eq!(
            serde_json::to_string(&one).unwrap(),
            serde_json::to_string(&two).unwrap()
        );
    }

    #[test]
    fn no_fields_is_an_error() {
        assert_eq!(
            build_request(&[], &inputs()).unwrap_err(),
            BuildRequestError::NoFields
        );
    }

    #[test]
    fn texts_must_match_the_declared_labels_and_budgets() {
        let schema = enum_schema(DeciderMode::Shadow);
        let fields = vec![DeclaredField::from_schema("verdict", &schema).unwrap()];
        let texts = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };

        let ok =
            AssembledInputs::from_texts(&fields, texts(&[("outline_item", "o"), ("plan", "p")]))
                .unwrap();
        assert_eq!(
            build_request(&fields, ok.as_map()).unwrap(),
            build_request(&fields, &texts(&[("outline_item", "o"), ("plan", "p")])).unwrap()
        );

        assert_eq!(
            AssembledInputs::from_texts(&fields, texts(&[("outline_item", "o")])).unwrap_err(),
            InputTextError::Missing {
                label: "plan".to_string()
            }
        );
        assert_eq!(
            AssembledInputs::from_texts(
                &fields,
                texts(&[("outline_item", "o"), ("plan", "p"), ("x", "y")])
            )
            .unwrap_err(),
            InputTextError::Extra {
                label: "x".to_string()
            }
        );

        let mut small = enum_schema(DeciderMode::Shadow);
        small.decider.as_mut().unwrap().inputs[1].max_bytes = 3;
        let fields = vec![DeclaredField::from_schema("verdict", &small).unwrap()];
        assert!(AssembledInputs::from_texts(
            &fields,
            texts(&[("outline_item", "o"), ("plan", "abc")])
        )
        .is_ok());
        assert_eq!(
            AssembledInputs::from_texts(&fields, texts(&[("outline_item", "o"), ("plan", "abcd")]))
                .unwrap_err(),
            InputTextError::OverBudget {
                label: "plan".to_string(),
                bytes: 4,
                max_bytes: 3
            }
        );
    }

    #[test]
    fn from_schema_skips_undeclared_and_other_types() {
        let mut s = enum_schema(DeciderMode::Shadow);
        s.field_type = "string".to_string();
        assert!(DeclaredField::from_schema("x", &s).is_none());
        let mut s = enum_schema(DeciderMode::Shadow);
        s.decider = None;
        assert!(DeclaredField::from_schema("x", &s).is_none());
    }
}
