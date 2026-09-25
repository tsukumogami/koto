//! Compiled decider declarations on `accepts` fields.
//!
//! A template may mark an `enum` or `boolean` evidence field as one a typed
//! decider can answer, by writing a `decider` block inside the field. The
//! compiler (`src/template/compile.rs`) lowers that block into the types here
//! with every default resolved, and `CompiledTemplate::validate` checks it
//! with the `E-DECIDER-*` rules. See
//! docs/designs/DESIGN-jev-decision-offload.md, Decision 1.
//!
//! These types are the whole compiled contract: the request builder, the
//! provider client, and the consultation arm of the advance loop read a
//! declaration only through them. Nothing here does I/O.
//!
//! The declaration hash is deliberately not stored in the compiled JSON. It is
//! computed on demand by [`declaration_hash`], so a template with no `decider`
//! block serializes exactly as it did before the block existed and keeps its
//! `template_hash`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Threshold an answer compiles to when it declares none.
pub const DEFAULT_THRESHOLD: f64 = 0.9;

/// Lowest threshold a declaration may set. Below one half, an answer could
/// apply while the provider thinks it is more likely wrong than right.
pub const MIN_THRESHOLD: f64 = 0.5;

/// Highest threshold a declaration may set.
pub const MAX_THRESHOLD: f64 = 1.0;

/// Byte budget an input compiles to when it declares none.
pub const DEFAULT_MAX_BYTES: u32 = 8192;

/// Per-answer mode, as declared by the template.
///
/// `never` is template-only: the value may be consulted and recorded but is
/// never applied. The ordering used to combine this with the user's and the
/// project's configured modes belongs to the engine, not to this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeciderMode {
    Off,
    Shadow,
    Auto,
    Never,
}

impl DeciderMode {
    /// Mode an answer compiles to when it declares none.
    pub const DEFAULT: DeciderMode = DeciderMode::Shadow;

    /// Every accepted spelling, in the order error messages list them.
    pub const NAMES: [&'static str; 4] = ["off", "shadow", "auto", "never"];

    /// Parse a template mode string. Returns `None` for anything that is not
    /// exactly one of [`DeciderMode::NAMES`].
    pub fn parse(s: &str) -> Option<DeciderMode> {
        match s {
            "off" => Some(DeciderMode::Off),
            "shadow" => Some(DeciderMode::Shadow),
            "auto" => Some(DeciderMode::Auto),
            "never" => Some(DeciderMode::Never),
            _ => None,
        }
    }

    /// The template spelling of this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            DeciderMode::Off => "off",
            DeciderMode::Shadow => "shadow",
            DeciderMode::Auto => "auto",
            DeciderMode::Never => "never",
        }
    }
}

impl std::fmt::Display for DeciderMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One answer the decider may give for a field: a value in the field's
/// `values` (or `"true"`/`"false"` for a boolean field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeciderAnswer {
    /// What this answer means, shown to the agent in `value_descriptions` and
    /// sent to the provider.
    pub description: String,
    /// Resolved at compile time; [`DeciderMode::DEFAULT`] when omitted.
    pub mode: DeciderMode,
    /// Resolved at compile time; [`DEFAULT_THRESHOLD`] when omitted. Always
    /// finite and within [`MIN_THRESHOLD`, `MAX_THRESHOLD`] once validated.
    pub threshold: f64,
}

/// The enum escape: a value the decider may answer with to say the question
/// can't be judged from its inputs. It is never one of the field's `values`,
/// so it can't be submitted as evidence and never appears in `expects`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderEscape {
    pub value: String,
    pub description: String,
}

/// Where an input's content comes from.
///
/// The set is closed on purpose: a declaration can read a context-store key or
/// a template variable (declared or captured), and nothing else -- not an
/// environment variable, not a file path, not a runtime name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeciderInputSource {
    /// A context-store key. May contain `{{KEY}}` references, substituted at
    /// run time like a gate's key.
    Context(String),
    /// A declared template variable or a `capture_stdout_as` name.
    Var(String),
}

/// One labelled input sent with the question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderInput {
    pub source: DeciderInputSource,
    /// Name the provider sees the content under. Unique within the field.
    pub label: String,
    /// Resolved at compile time; [`DEFAULT_MAX_BYTES`] when omitted. Always
    /// greater than zero once validated.
    pub max_bytes: u32,
}

/// A compiled `decider` block, reachable as
/// `TemplateState.accepts[field].decider`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDecider {
    /// Keyed by value: the enum's `values`, or `"true"` and `"false"`.
    pub answers: BTreeMap<String, DeciderAnswer>,
    /// Required on enum fields, absent on boolean fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escape: Option<DeciderEscape>,
    /// In declaration order.
    pub inputs: Vec<DeciderInput>,
}

/// Why a transition may not be taken on an `auto` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorViolationKind {
    /// The transition targets a terminal state.
    TerminalTarget,
    /// The target state's `default_action` has `requires_confirmation: true`.
    ConfirmationRequired,
    /// The transition's `when` clause also tests this `gates.*` key.
    GateConditioned { gate_key: String },
}

impl FloorViolationKind {
    /// The rule broken, in words, for error messages.
    pub fn describe(&self) -> String {
        match self {
            FloorViolationKind::TerminalTarget => "the target is a terminal state".to_string(),
            FloorViolationKind::ConfirmationRequired => {
                "the target's default_action has requires_confirmation: true".to_string()
            }
            FloorViolationKind::GateConditioned { gate_key } => {
                format!("the when clause also tests gate key {:?}", gate_key)
            }
        }
    }
}

/// One transition that the floor forbids an `auto` answer from taking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorViolation {
    /// Index into the source state's `transitions`.
    pub transition_index: usize,
    /// The transition's target state.
    pub target: String,
    pub kind: FloorViolationKind,
}

/// Domain separator for [`declaration_hash`]. Bump the suffix only if the
/// fingerprint layout below changes in a way that must not collide with
/// hashes already recorded.
const DECLARATION_HASH_DOMAIN: &[u8] = b"koto-decider-declaration/v1";

/// Hex SHA-256 identity of a declaration.
///
/// It covers the question, the value set, every answer's description, the
/// escape's value and description, and every input's source, label, and byte
/// budget. It leaves out modes and thresholds, so promoting an answer keeps the
/// evidence gathered under the same declaration. Values are hashed in sorted
/// order (the `answers` map is ordered), so reordering `values:` doesn't change
/// it; inputs are hashed in declaration order, because that order is what the
/// provider receives.
///
/// Every struct is destructured without a `..` rest pattern, so adding a field
/// to [`FieldDecider`], [`DeciderAnswer`], [`DeciderEscape`], or
/// [`DeciderInput`] is a compile error here until someone decides whether it
/// belongs in the identity. Widening what a declaration sends therefore can't
/// silently reuse an old identity.
pub fn declaration_hash(decider: &FieldDecider, question: &str) -> String {
    let FieldDecider {
        answers,
        escape,
        inputs,
    } = decider;

    let mut h = Fingerprint::new();
    h.tag(b"question");
    h.str(question);

    h.tag(b"answers");
    h.len(answers.len());
    for (value, answer) in answers {
        let DeciderAnswer {
            description,
            mode: _,
            threshold: _,
        } = answer;
        h.str(value);
        h.str(description);
    }

    match escape {
        Some(DeciderEscape { value, description }) => {
            h.tag(b"escape");
            h.str(value);
            h.str(description);
        }
        None => h.tag(b"no-escape"),
    }

    h.tag(b"inputs");
    h.len(inputs.len());
    for input in inputs {
        let DeciderInput {
            source,
            label,
            max_bytes,
        } = input;
        match source {
            DeciderInputSource::Context(key) => {
                h.tag(b"context");
                h.str(key);
            }
            DeciderInputSource::Var(name) => {
                h.tag(b"var");
                h.str(name);
            }
        }
        h.str(label);
        h.bytes(&max_bytes.to_be_bytes());
    }

    h.finish()
}

/// Unambiguous byte encoding for [`declaration_hash`]: every variable-length
/// item is length-prefixed, so no two different declarations can serialize to
/// the same bytes by shifting text between adjacent strings.
struct Fingerprint(Sha256);

impl Fingerprint {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update((DECLARATION_HASH_DOMAIN.len() as u64).to_be_bytes());
        hasher.update(DECLARATION_HASH_DOMAIN);
        Fingerprint(hasher)
    }

    fn bytes(&mut self, b: &[u8]) {
        self.0.update((b.len() as u64).to_be_bytes());
        self.0.update(b);
    }

    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn tag(&mut self, t: &[u8]) {
        self.bytes(t);
    }

    fn len(&mut self, n: usize) {
        self.0.update((n as u64).to_be_bytes());
    }

    fn finish(self) -> String {
        hex::encode(self.0.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(description: &str) -> DeciderAnswer {
        DeciderAnswer {
            description: description.to_string(),
            mode: DeciderMode::Shadow,
            threshold: DEFAULT_THRESHOLD,
        }
    }

    fn base() -> FieldDecider {
        let mut answers = BTreeMap::new();
        answers.insert("proceed".to_string(), answer("Clear and scoped."));
        answers.insert("exit".to_string(), answer("Vague."));
        FieldDecider {
            answers,
            escape: Some(DeciderEscape {
                value: "unclear".to_string(),
                description: "Can't judge.".to_string(),
            }),
            inputs: vec![
                DeciderInput {
                    source: DeciderInputSource::Context("context.md".to_string()),
                    label: "outline_item".to_string(),
                    max_bytes: 12000,
                },
                DeciderInput {
                    source: DeciderInputSource::Var("PLAN_DOC".to_string()),
                    label: "plan_path".to_string(),
                    max_bytes: DEFAULT_MAX_BYTES,
                },
            ],
        }
    }

    const Q: &str = "Is the item clear enough?";

    fn hash(d: &FieldDecider) -> String {
        declaration_hash(d, Q)
    }

    #[test]
    fn hash_is_hex_sha256_and_deterministic() {
        let h = hash(&base());
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, hash(&base()));
    }

    #[test]
    fn hash_changes_with_the_question() {
        assert_ne!(
            hash(&base()),
            declaration_hash(&base(), "Another question?")
        );
    }

    #[test]
    fn hash_changes_with_a_value_description() {
        let mut d = base();
        d.answers.get_mut("exit").unwrap().description = "Vague or contradictory.".into();
        assert_ne!(hash(&base()), hash(&d));
    }

    #[test]
    fn hash_changes_with_the_escape_value_or_description() {
        let mut d = base();
        d.escape.as_mut().unwrap().value = "unknown".into();
        assert_ne!(hash(&base()), hash(&d));

        let mut d = base();
        d.escape.as_mut().unwrap().description = "Truncated.".into();
        assert_ne!(hash(&base()), hash(&d));

        let mut d = base();
        d.escape = None;
        assert_ne!(hash(&base()), hash(&d));
    }

    #[test]
    fn hash_changes_with_an_input_source_label_or_budget() {
        let mut d = base();
        d.inputs[0].source = DeciderInputSource::Context("other.md".into());
        assert_ne!(hash(&base()), hash(&d));

        // Same name, different namespace.
        let mut d = base();
        d.inputs[1].source = DeciderInputSource::Context("PLAN_DOC".into());
        assert_ne!(hash(&base()), hash(&d));

        let mut d = base();
        d.inputs[0].label = "item".into();
        assert_ne!(hash(&base()), hash(&d));

        let mut d = base();
        d.inputs[0].max_bytes = 12001;
        assert_ne!(hash(&base()), hash(&d));

        let mut d = base();
        d.inputs.pop();
        assert_ne!(hash(&base()), hash(&d));
    }

    #[test]
    fn hash_changes_with_the_value_set() {
        let mut d = base();
        d.answers.insert("defer".into(), answer("Later."));
        assert_ne!(hash(&base()), hash(&d));

        // Renaming a value, same description.
        let mut d = base();
        let a = d.answers.remove("exit").unwrap();
        d.answers.insert("stop".into(), a);
        assert_ne!(hash(&base()), hash(&d));
    }

    #[test]
    fn hash_ignores_modes_and_thresholds() {
        let mut d = base();
        let a = d.answers.get_mut("proceed").unwrap();
        a.mode = DeciderMode::Auto;
        a.threshold = 0.97;
        d.answers.get_mut("exit").unwrap().mode = DeciderMode::Never;
        assert_eq!(hash(&base()), hash(&d));
    }

    #[test]
    fn hash_ignores_value_insertion_order() {
        let a = base();
        let mut answers = BTreeMap::new();
        answers.insert("exit".to_string(), answer("Vague."));
        answers.insert("proceed".to_string(), answer("Clear and scoped."));
        let b = FieldDecider { answers, ..base() };
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn adjacent_strings_cannot_collide() {
        let mut a = base();
        a.inputs[0].label = "ab".into();
        a.inputs[1].label = "c".into();
        let mut b = base();
        b.inputs[0].label = "a".into();
        b.inputs[1].label = "bc".into();
        assert_ne!(hash(&a), hash(&b));
    }

    #[test]
    fn mode_parse_round_trips_every_name() {
        for name in DeciderMode::NAMES {
            assert_eq!(DeciderMode::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(DeciderMode::parse("Auto"), None);
        assert_eq!(DeciderMode::parse("automatic"), None);
    }

    #[test]
    fn compiled_declaration_round_trips_through_json() {
        let d = base();
        let json = serde_json::to_string_pretty(&d).unwrap();
        let back: FieldDecider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
        assert!(json.contains("\"context\": \"context.md\""));
        assert!(json.contains("\"var\": \"PLAN_DOC\""));
    }
}
