use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{anyhow, Context};
use serde::Deserialize;

use super::decider::{
    DeciderAnswer, DeciderEscape, DeciderInput, DeciderInputSource, DeciderMode, FieldDecider,
    DEFAULT_MAX_BYTES, DEFAULT_THRESHOLD,
};
use super::types::{
    default_failure_policy, ActionDecl, CompiledTemplate, FailurePolicy, FieldSchema, Gate,
    MaterializeChildrenSpec, PollingConfig, TemplateState, Transition, VariableDecl,
    GATE_TYPE_CHILDREN_COMPLETE, GATE_TYPE_COMMAND, GATE_TYPE_CONTEXT_EXISTS,
    GATE_TYPE_CONTEXT_MATCHES, GATE_TYPE_REQUEST_LEG, SUPPORTED_GATE_TYPES,
};

/// YAML front-matter structure of a template source file.
#[derive(Debug, Deserialize)]
struct SourceFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    initial_state: String,
    #[serde(default)]
    variables: HashMap<String, SourceVariable>,
    #[serde(default)]
    states: HashMap<String, SourceState>,
}

/// YAML front-matter view of a variable declaration.
///
/// `deny_unknown_fields` so a misspelled constraint (`valuez:`) fails
/// compilation instead of being dropped, which would leave the variable
/// unconstrained without anyone noticing. `values` and `pattern` are
/// `Option` so an explicitly empty declaration is told apart from an omitted
/// one and refused.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SourceVariable {
    #[serde(default)]
    description: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default: String,
    #[serde(default)]
    values: Option<Vec<String>>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    rebind: bool,
}

/// YAML front-matter view of a single state.
///
/// `#[serde(deny_unknown_fields)]` is applied here (but NOT on
/// `CompiledTemplate::TemplateState`) so that typos or unknown keys in
/// template source are caught at compile time, while the compile cache
/// remains forward-compatible with newer binaries that may add fields.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SourceState {
    #[serde(default)]
    transitions: Vec<SourceTransition>,
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    gates: HashMap<String, SourceGate>,
    #[serde(default)]
    accepts: HashMap<String, SourceFieldSchema>,
    #[serde(default)]
    integration: Option<String>,
    #[serde(default)]
    default_action: Option<SourceActionDecl>,
    #[serde(default)]
    materialize_children: Option<SourceMaterializeChildrenSpec>,
    #[serde(default)]
    failure: bool,
    #[serde(default)]
    skipped_marker: bool,
    #[serde(default)]
    skip_if: Option<HashMap<String, serde_json::Value>>,
    /// A terminal state's declared result. Values stay YAML values here so
    /// a mapping or sequence can be reported by name rather than as a serde
    /// type error; scalars are converted to strings in `compile`.
    #[serde(default)]
    result: Option<BTreeMap<String, serde_yaml_ng::Value>>,
}

/// YAML front-matter view of a `materialize_children` hook.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMaterializeChildrenSpec {
    #[serde(default)]
    from_field: String,
    #[serde(default)]
    default_template: String,
    #[serde(default = "default_failure_policy")]
    failure_policy: FailurePolicy,
}

/// Action declaration in source YAML.
#[derive(Debug, Deserialize)]
struct SourceActionDecl {
    #[serde(default)]
    command: String,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    requires_confirmation: bool,
    #[serde(default)]
    polling: Option<SourcePollingConfig>,
    #[serde(default)]
    fallback: Option<String>,
    #[serde(default)]
    capture_stdout_as: Option<String>,
}

/// Polling configuration in source YAML.
#[derive(Debug, Deserialize)]
struct SourcePollingConfig {
    #[serde(default)]
    interval_secs: u32,
    #[serde(default)]
    timeout_secs: u32,
}

/// A transition in source YAML:
/// `{target: "done", when: {field: value}, context_assignments: {key: value}}`.
///
/// Unknown keys are collected rather than refused by serde, so the error can
/// name the state and the target the typo sits on (`compile_transition`). They
/// used to be dropped silently, which is how every `context_assignments` block
/// written before koto#204 compiled and never ran.
#[derive(Debug, Deserialize)]
struct SourceTransition {
    target: String,
    #[serde(default)]
    when: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    context_assignments: Option<HashMap<String, serde_json::Value>>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// Fields a source transition may declare, for the unknown-field message.
const SOURCE_TRANSITION_FIELDS: &[&str] = &["target", "when", "context_assignments"];

/// Convert one source transition, refusing unknown keys and non-scalar
/// assignment values. Reference checks that need the rest of the template
/// (declared evidence fields, gates, variables) run in
/// `CompiledTemplate::validate`.
fn compile_transition(state_name: &str, st: &SourceTransition) -> anyhow::Result<Transition> {
    if let Some(field) = st.unknown.keys().next() {
        return Err(anyhow!(
            "state {:?} transition to {:?}: unknown field {:?}; expected one of: {}",
            state_name,
            st.target,
            field,
            SOURCE_TRANSITION_FIELDS.join(", ")
        ));
    }
    let mut context_assignments: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in st.context_assignments.iter().flatten() {
        let rendered = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(_) | serde_json::Value::Number(_) => value.to_string(),
            serde_json::Value::Null => {
                return Err(anyhow!(
                    "state {:?} transition to {:?}: context_assignments {:?} has no value; \
                     write a string (use \"\" for an empty value)",
                    state_name,
                    st.target,
                    key
                ));
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                return Err(anyhow!(
                    "state {:?} transition to {:?}: context_assignments {:?} must be a string, not {}",
                    state_name,
                    st.target,
                    key,
                    if value.is_array() {
                        "a sequence"
                    } else {
                        "a mapping"
                    }
                ));
            }
        };
        context_assignments.insert(key.clone(), rendered);
    }
    Ok(Transition {
        target: st.target.clone(),
        when: st
            .when
            .as_ref()
            .map(|w| w.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        context_assignments,
    })
}

/// Field schema in source YAML for an `accepts` block.
#[derive(Debug, Deserialize)]
struct SourceFieldSchema {
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    values: Vec<String>,
    #[serde(default)]
    description: String,
    /// Optional decider declaration. `SourceFieldSchema` itself stays lenient
    /// (no `deny_unknown_fields`), which is what lets koto v0.12.2 drop this
    /// whole block silently; the structs inside the block are strict, so a
    /// typo in it fails on a current koto.
    #[serde(default)]
    decider: Option<SourceDecider>,
}

/// A field-level `decider` block in source YAML.
///
/// See docs/designs/DESIGN-jev-decision-offload.md, Decision 1. Modes are
/// read as strings and input sources as two optional keys, so that an unknown
/// mode or an input naming both or neither source reaches lowering and fails
/// with its own `E-DECIDER-*` code rather than a generic parse error.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDecider {
    #[serde(default)]
    answers: SourceDeciderAnswers,
    /// `Some` whenever the key is written at all, even as `escape: ~`, so a
    /// boolean field can't carry an escape key that lowering fails to see.
    #[serde(default, deserialize_with = "present_as_some")]
    escape: Option<SourceDeciderEscape>,
    #[serde(default)]
    inputs: Vec<SourceDeciderInput>,
}

/// One entry of a `decider.answers` map.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SourceDeciderAnswer {
    #[serde(default)]
    description: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    threshold: Option<f64>,
}

/// `decider.escape` in source YAML.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SourceDeciderEscape {
    #[serde(default)]
    value: String,
    #[serde(default)]
    description: String,
}

/// One entry of `decider.inputs` in source YAML.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDeciderInput {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    var: Option<String>,
    #[serde(default)]
    label: String,
    #[serde(default)]
    max_bytes: Option<u32>,
}

/// The `answers` map, kept as written (in order, duplicates included) so that
/// lowering can report a duplicate rather than serde silently keeping one.
///
/// Keys may be written as YAML booleans: a boolean field's answers are
/// naturally spelled `true:` and `false:`, which YAML parses as booleans, not
/// strings.
#[derive(Debug, Default)]
struct SourceDeciderAnswers(Vec<(String, SourceDeciderAnswer)>);

impl<'de> Deserialize<'de> for SourceDeciderAnswers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct AnswersVisitor;
        impl<'de> serde::de::Visitor<'de> for AnswersVisitor {
            type Value = SourceDeciderAnswers;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map from each value to its answer")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(AnswerKey(key)) = map.next_key()? {
                    let answer: SourceDeciderAnswer = map.next_value()?;
                    entries.push((key, answer));
                }
                Ok(SourceDeciderAnswers(entries))
            }
        }
        d.deserialize_map(AnswersVisitor)
    }
}

/// An `answers` key: a string, or a YAML boolean or integer rendered as the
/// string evidence would carry.
struct AnswerKey(String);

impl<'de> Deserialize<'de> for AnswerKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct KeyVisitor;
        impl serde::de::Visitor<'_> for KeyVisitor {
            type Value = AnswerKey;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a value name")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<AnswerKey, E> {
                Ok(AnswerKey(v.to_string()))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<AnswerKey, E> {
                Ok(AnswerKey(v.to_string()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<AnswerKey, E> {
                Ok(AnswerKey(v.to_string()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<AnswerKey, E> {
                Ok(AnswerKey(v.to_string()))
            }
        }
        d.deserialize_any(KeyVisitor)
    }
}

/// Deserialize a present key as `Some`, treating an explicit null as the
/// type's default rather than as absence. Paired with `#[serde(default)]`,
/// only a missing key yields `None`.
fn present_as_some<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Some(Option::<T>::deserialize(d)?.unwrap_or_default()))
}

/// Lower a source `decider` block, resolving every default.
///
/// Only the two rules the compiled types can't represent fail here: an unknown
/// mode string (`E-DECIDER-MODE`) and an input naming both or neither of
/// `context` and `var` (`E-DECIDER-INPUT`), plus a duplicated answer key
/// (`E-DECIDER-ANSWERS`), which a map can't hold. Everything else is checked
/// on the compiled form by `CompiledTemplate::validate`, so a compiled
/// template loaded from the cache gets the same checks.
fn lower_decider(
    state_name: &str,
    field_name: &str,
    source: &SourceDecider,
) -> anyhow::Result<FieldDecider> {
    let mut answers: BTreeMap<String, DeciderAnswer> = BTreeMap::new();
    for (value, sa) in &source.answers.0 {
        let mode = match &sa.mode {
            None => DeciderMode::DEFAULT,
            Some(m) => DeciderMode::parse(m).ok_or_else(|| {
                anyhow!(
                    "validation error: E-DECIDER-MODE: state {:?} field {:?} value {:?}: \
                     unknown mode {:?}; a mode is one of {}\n  \
                     remedy: use one of those modes, or omit mode for {}",
                    state_name,
                    field_name,
                    value,
                    m,
                    DeciderMode::NAMES.join(", "),
                    DeciderMode::DEFAULT
                )
            })?,
        };
        let lowered = DeciderAnswer {
            description: sa.description.clone(),
            mode,
            threshold: sa.threshold.unwrap_or(DEFAULT_THRESHOLD),
        };
        if answers.insert(value.clone(), lowered).is_some() {
            return Err(anyhow!(
                "validation error: E-DECIDER-ANSWERS: state {:?} field {:?} value {:?}: \
                 the value has more than one entry under answers\n  \
                 remedy: keep one entry per value",
                state_name,
                field_name,
                value
            ));
        }
    }

    let mut inputs = Vec::with_capacity(source.inputs.len());
    for (i, si) in source.inputs.iter().enumerate() {
        let src = match (&si.context, &si.var) {
            (Some(key), None) => DeciderInputSource::Context(key.clone()),
            (None, Some(name)) => DeciderInputSource::Var(name.clone()),
            (Some(_), Some(_)) | (None, None) => {
                let which = if si.context.is_some() {
                    "names both context and var"
                } else {
                    "names neither context nor var"
                };
                return Err(anyhow!(
                    "validation error: E-DECIDER-INPUT: state {:?} field {:?}: input {} \
                     (label {:?}) {}; each input names exactly one source\n  \
                     remedy: write either `context: <key>` or `var: <NAME>`",
                    state_name,
                    field_name,
                    i + 1,
                    si.label,
                    which
                ));
            }
        };
        inputs.push(DeciderInput {
            source: src,
            label: si.label.clone(),
            max_bytes: si.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        });
    }

    Ok(FieldDecider {
        answers,
        escape: source.escape.as_ref().map(|e| DeciderEscape {
            value: e.value.clone(),
            description: e.description.clone(),
        }),
        inputs,
    })
}

#[derive(Debug, Deserialize)]
struct SourceGate {
    #[serde(rename = "type")]
    gate_type: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    timeout: u32,
    #[serde(default)]
    key: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    override_default: Option<serde_json::Value>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    name_filter: Option<String>,
    /// Kept as a raw YAML value so `compile_gate` can reject anything but a
    /// boolean with an error that names the state and gate.
    #[serde(default)]
    overridable: Option<serde_yaml_ng::Value>,
    /// Request id for `request-leg` gates.
    #[serde(default)]
    request: String,
    /// Leg name for `request-leg` gates.
    #[serde(default)]
    leg: String,
    /// Kept as a raw YAML value so `compile_gate` can reject a malformed
    /// `expect` with an error that names the state and gate.
    #[serde(default)]
    expect: Option<serde_yaml_ng::Value>,
    /// Every key the fields above don't name. `SourceState` uses
    /// `deny_unknown_fields`, but serde's error for it surfaces only as the
    /// outer "failed to parse front-matter" context, which names neither the
    /// state nor the gate. Collecting the leftovers lets `compile_gate` refuse
    /// them with an error that does, so a misspelled `overrideable: false`
    /// can't compile and quietly leave the gate overridable.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_yaml_ng::Value>,
}

/// The keys a gate declaration may carry, listed in unknown-key errors.
const SOURCE_GATE_KEYS: &[&str] = &[
    "type",
    "command",
    "timeout",
    "key",
    "pattern",
    "override_default",
    "completion",
    "name_filter",
    "overridable",
    "request",
    "leg",
    "expect",
];

/// Compile a YAML/Markdown template source file to a FormatVersion=1 CompiledTemplate.
///
/// `strict` is passed through to `validate()`. When `true`, a state with gates
/// but no `gates.*` when-clause references is a hard error. When `false`, the
/// same condition emits a warning to stderr and compilation continues.
pub fn compile(source_path: &Path, strict: bool) -> anyhow::Result<CompiledTemplate> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read template source: {}", source_path.display()))?;

    let (frontmatter_str, body) = split_frontmatter(&content).ok_or_else(|| {
        anyhow!("invalid YAML: template must begin with YAML front-matter delimited by '---'")
    })?;

    // The parse error is part of the message, not only the error chain: the
    // CLI prints the top-level message, and a key misspelled inside a strict
    // block (say `thresold:` in a decider answer, or `valuez:` in a variable
    // declaration) is only named down there.
    let fm: SourceFrontmatter = serde_yaml_ng::from_str(frontmatter_str)
        .map_err(|e| anyhow!("invalid YAML: failed to parse front-matter: {}", e))?;

    // Validate required front-matter fields.
    if fm.name.is_empty() {
        return Err(anyhow!("missing required field: name"));
    }
    if fm.version.is_empty() {
        return Err(anyhow!("missing required field: version"));
    }
    if fm.initial_state.is_empty() {
        return Err(anyhow!("missing required field: initial_state"));
    }
    if fm.states.is_empty() {
        return Err(anyhow!("template has no states"));
    }

    // Extract directives from the markdown body for each declared state.
    let directives = extract_directives(&fm.states, body);

    // Build compiled states.
    let mut compiled_states: BTreeMap<String, TemplateState> = BTreeMap::new();
    for (state_name, source_state) in &fm.states {
        let (directive, details) = directives.get(state_name).cloned().unwrap_or_default();
        if directive.is_empty() {
            return Err(anyhow!(
                "state {:?} has no directive section in markdown body",
                state_name
            ));
        }

        let mut compiled_gates: BTreeMap<String, Gate> = BTreeMap::new();
        for (gate_name, source_gate) in &source_state.gates {
            let gate = compile_gate(state_name, gate_name, source_gate)?;
            compiled_gates.insert(gate_name.clone(), gate);
        }

        // Transform source transitions to compiled transitions.
        let compiled_transitions: Vec<Transition> = source_state
            .transitions
            .iter()
            .map(|st| compile_transition(state_name, st))
            .collect::<anyhow::Result<_>>()?;

        // Transform source accepts to compiled accepts.
        let compiled_accepts: Option<BTreeMap<String, FieldSchema>> =
            if source_state.accepts.is_empty() {
                None
            } else {
                let mut fields = BTreeMap::new();
                for (k, v) in &source_state.accepts {
                    let decider = v
                        .decider
                        .as_ref()
                        .map(|d| lower_decider(state_name, k, d))
                        .transpose()?;
                    fields.insert(
                        k.clone(),
                        FieldSchema {
                            field_type: v.field_type.clone(),
                            required: v.required,
                            values: v.values.clone(),
                            description: v.description.clone(),
                            decider,
                        },
                    );
                }
                Some(fields)
            };

        // Transform source default_action to compiled ActionDecl.
        let compiled_action = source_state.default_action.as_ref().map(|sa| ActionDecl {
            command: sa.command.clone(),
            working_dir: sa.working_dir.clone(),
            requires_confirmation: sa.requires_confirmation,
            polling: sa.polling.as_ref().map(|sp| PollingConfig {
                interval_secs: sp.interval_secs,
                timeout_secs: sp.timeout_secs,
            }),
            fallback: sa.fallback.clone(),
            capture_stdout_as: sa.capture_stdout_as.clone(),
        });

        let compiled_materialize_children =
            source_state
                .materialize_children
                .as_ref()
                .map(|sm| MaterializeChildrenSpec {
                    from_field: sm.from_field.clone(),
                    default_template: sm.default_template.clone(),
                    failure_policy: sm.failure_policy,
                });

        // Transform source skip_if to compiled skip_if (HashMap -> BTreeMap for determinism).
        let compiled_skip_if: Option<std::collections::BTreeMap<String, serde_json::Value>> =
            source_state
                .skip_if
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

        let compiled_result = match &source_state.result {
            Some(map) => Some(compile_result_map(state_name, map)?),
            None => None,
        };

        compiled_states.insert(
            state_name.clone(),
            TemplateState {
                directive,
                details,
                transitions: compiled_transitions,
                terminal: source_state.terminal,
                gates: compiled_gates,
                accepts: compiled_accepts,
                integration: source_state.integration.clone(),
                default_action: compiled_action,
                materialize_children: compiled_materialize_children,
                failure: source_state.failure,
                skipped_marker: source_state.skipped_marker,
                skip_if: compiled_skip_if,
                result: compiled_result,
            },
        );
    }

    // Validate transition targets exist.
    for (state_name, state) in &compiled_states {
        for transition in &state.transitions {
            if !compiled_states.contains_key(&transition.target) {
                return Err(anyhow!(
                    "state {:?} references undefined transition target {:?}",
                    state_name,
                    transition.target
                ));
            }
        }
    }

    // Validate initial_state is declared.
    if !compiled_states.contains_key(&fm.initial_state) {
        return Err(anyhow!(
            "initial_state {:?} is not a declared state",
            fm.initial_state
        ));
    }

    // Build compiled variables, refusing an incoherent constraint.
    let mut variables: BTreeMap<String, VariableDecl> = BTreeMap::new();
    for (k, v) in fm.variables {
        if matches!(&v.values, Some(list) if list.is_empty()) {
            return Err(anyhow!("variable {:?}: values: must not be empty", k));
        }
        if matches!(&v.pattern, Some(p) if p.is_empty()) {
            return Err(anyhow!("variable {:?}: pattern: must not be empty", k));
        }
        let decl = VariableDecl {
            description: v.description,
            required: v.required,
            default: v.default,
            values: v.values.unwrap_or_default(),
            pattern: v.pattern.unwrap_or_default(),
            rebind: v.rebind,
        };
        decl.validate_declaration(&k).map_err(|e| anyhow!(e))?;
        variables.insert(k, decl);
    }

    let template = CompiledTemplate {
        format_version: 1,
        name: fm.name,
        version: fm.version,
        description: fm.description,
        initial_state: fm.initial_state,
        variables,
        states: compiled_states,
    };

    // Run validation rules (includes evidence routing validation).
    template
        .validate(strict)
        .map_err(|e| anyhow!("validation error: {}", e))?;

    // Issue 8: E9 resolution check and F5 (skipped_marker reachability)
    // on batch-eligible child templates. Both need the source path to
    // resolve relative `default_template` references against the parent
    // template's directory, so they live here rather than in `validate`.
    validate_default_template_references(&template, source_path)?;

    Ok(template)
}

/// Convert a source `result:` map to its compiled form.
///
/// A value must be a scalar. Strings are taken as written; a number or a
/// boolean is a literal and is taken in its YAML text form, so `pr: 12`
/// and `pr: "12"` compile alike. A mapping, a sequence, or an empty value
/// fails, because a result is a flat map of strings that a reader routes
/// on without knowing the template. The rest of the grammar is checked in
/// [`crate::template::result_map::validate_result_map`] during `validate`.
fn compile_result_map(
    state_name: &str,
    source: &BTreeMap<String, serde_yaml_ng::Value>,
) -> anyhow::Result<BTreeMap<String, String>> {
    use serde_yaml_ng::Value;
    let mut out = BTreeMap::new();
    for (key, value) in source {
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            other => {
                let kind = match other {
                    Value::Mapping(_) => "a mapping",
                    Value::Sequence(_) => "a sequence",
                    Value::Null => "empty",
                    _ => "not a string",
                };
                return Err(anyhow!(
                    "state {:?}: result key {:?} is {}; a result value must be a string\n  \
                     remedy: write the value as a string holding literal text, {{{{VAR}}}}, \
                     or ${{context.<key>}}",
                    state_name,
                    key,
                    kind
                ));
            }
        };
        out.insert(key.clone(), text);
    }
    Ok(out)
}

/// E9 resolution + F5 warning.
///
/// For every state with a `materialize_children` hook, resolve the hook's
/// `default_template` against the compiling template's directory. The path
/// must point to a file that itself compiles without error; any failure is
/// surfaced as an E9 error naming the declaring state.
///
/// When the child template compiles, fire warning F5 on stderr if the
/// child lacks a terminal state with `skipped_marker: true` that is
/// reachable from its initial state. The check is intentionally permissive
/// about "scheduler-writable transitions" (Decision 9): for now any
/// transition counts as reachable. F5 is a warning, not an error, because
/// batch-eligibility is not knowable when a child template is compiled in
/// isolation.
///
/// Warnings are printed to stderr via `eprintln!` in the same style as
/// D4/D5 and W1-W5.
fn validate_default_template_references(
    template: &CompiledTemplate,
    source_path: &Path,
) -> anyhow::Result<()> {
    // Resolve the parent template's directory. Relative default_template
    // paths anchor here. `canonicalize` may fail if the source path is a
    // temporary with a stripped parent; fall back to the raw parent.
    let source_dir = source_path
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .or_else(|| source_path.parent().map(|p| p.to_path_buf()));

    for (state_name, state) in &template.states {
        let hook = match &state.materialize_children {
            Some(h) => h,
            None => continue,
        };
        // E1 already caught empty from_field; E9's non-emptiness is
        // checked in validate(). Skip empty here defensively.
        if hook.default_template.is_empty() {
            continue;
        }

        // Resolve default_template against source_dir. Absolute paths
        // pass through unchanged.
        let candidate = std::path::Path::new(&hook.default_template);
        let resolved: std::path::PathBuf = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else if let Some(dir) = &source_dir {
            dir.join(candidate)
        } else {
            candidate.to_path_buf()
        };

        // Guard against infinite recursion: a template that names itself
        // as its own default_template would loop. Compare canonical paths
        // when possible, raw otherwise.
        let same_as_source = match (resolved.canonicalize(), source_path.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => resolved == source_path,
        };
        if same_as_source {
            return Err(anyhow!(
                "E9: state {:?}: default_template {:?} resolves to the parent template itself; \
                 child templates must be distinct files\n  \
                 remedy: point default_template at a separate child template file",
                state_name,
                hook.default_template
            ));
        }

        // Compile the child. Any error is wrapped as E9.
        let child = compile(&resolved, true).map_err(|e| {
            anyhow!(
                "E9: state {:?}: default_template {:?} (resolved to {}) did not compile: {}\n  \
                 remedy: fix the child template so it compiles, or point default_template at a valid template file",
                state_name,
                hook.default_template,
                resolved.display(),
                e
            )
        })?;

        // F5 warning: child lacks a reachable skipped_marker terminal.
        if !child_has_reachable_skipped_marker(&child) {
            eprintln!(
                "warning: F5: child template {:?} (referenced by state {:?} default_template {:?}) \
                 has no reachable terminal state with `skipped_marker: true`; \
                 the batch scheduler will not be able to materialize skip markers for this template\n  \
                 remedy: add a terminal state with `skipped_marker: true` reachable from the initial state",
                child.name,
                state_name,
                hook.default_template
            );
        }
    }
    Ok(())
}

/// F5 helper: walk transitions from `initial_state` and return true if any
/// reachable state is terminal with `skipped_marker: true`.
///
/// Decision 9 distinguishes scheduler-writable transitions from
/// agent-submitted ones, but we conservatively treat every transition as
/// reachable here. Narrowing to scheduler-writable transitions lands in a
/// later phase once the transition metadata is finalized.
// TODO(issue-8/F5): narrow reachability to scheduler-writable transitions
// once Decision 9's metadata is exposed on `Transition`.
fn child_has_reachable_skipped_marker(child: &CompiledTemplate) -> bool {
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut frontier: Vec<&str> = Vec::new();
    if child.states.contains_key(child.initial_state.as_str()) {
        frontier.push(child.initial_state.as_str());
    }
    while let Some(name) = frontier.pop() {
        if !visited.insert(name) {
            continue;
        }
        let state = match child.states.get(name) {
            Some(s) => s,
            None => continue,
        };
        if state.terminal && state.skipped_marker {
            return true;
        }
        for transition in &state.transitions {
            if !visited.contains(transition.target.as_str()) {
                frontier.push(transition.target.as_str());
            }
        }
    }
    false
}

fn compile_gate(state_name: &str, gate_name: &str, source: &SourceGate) -> anyhow::Result<Gate> {
    if let Some(unknown_key) = source.unknown.keys().next() {
        return Err(anyhow!(
            "state {:?} gate {:?}: unknown key {:?}; a gate accepts only: {}",
            state_name,
            gate_name,
            unknown_key,
            SOURCE_GATE_KEYS.join(", ")
        ));
    }
    let overridable = match &source.overridable {
        None => true,
        Some(serde_yaml_ng::Value::Bool(b)) => *b,
        Some(other) => {
            return Err(anyhow!(
                "state {:?} gate {:?}: overridable must be a boolean (true or false), found {}",
                state_name,
                gate_name,
                serde_json::to_string(other).unwrap_or_else(|_| format!("{:?}", other))
            ));
        }
    };
    if !overridable && source.override_default.is_some() {
        return Err(anyhow!(
            "state {:?} gate {:?}: override_default is declared on a gate with overridable: false; \
             no override can ever apply it. Remove override_default, or make the gate overridable",
            state_name,
            gate_name
        ));
    }
    match source.gate_type.as_str() {
        GATE_TYPE_COMMAND => {
            if source.command.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: command must not be empty",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: source.command.clone(),
                timeout: source.timeout,
                key: String::new(),
                pattern: String::new(),
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
                overridable,
                request: String::new(),
                leg: String::new(),
                expect: None,
            })
        }
        GATE_TYPE_CONTEXT_EXISTS => {
            if source.key.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-exists gate must have a non-empty key",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: source.key.clone(),
                pattern: String::new(),
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
                overridable,
                request: String::new(),
                leg: String::new(),
                expect: None,
            })
        }
        GATE_TYPE_CONTEXT_MATCHES => {
            if source.key.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-matches gate must have a non-empty key",
                    state_name,
                    gate_name
                ));
            }
            if source.pattern.is_empty() {
                return Err(anyhow!(
                    "state {:?} gate {:?}: context-matches gate must have a non-empty pattern",
                    state_name,
                    gate_name
                ));
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: source.key.clone(),
                pattern: source.pattern.clone(),
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
                overridable,
                request: String::new(),
                leg: String::new(),
                expect: None,
            })
        }
        GATE_TYPE_CHILDREN_COMPLETE => {
            // Validate completion prefix.
            if let Some(ref completion) = source.completion {
                if completion != "terminal"
                    && !completion.starts_with("state:")
                    && !completion.starts_with("context:")
                {
                    return Err(anyhow!(
                        "state {:?} gate {:?}: unknown completion prefix {:?}; \
                         only \"terminal\" is supported (\"state:*\" and \"context:*\" are reserved)",
                        state_name,
                        gate_name,
                        completion
                    ));
                }
                if completion.starts_with("state:") || completion.starts_with("context:") {
                    return Err(anyhow!(
                        "state {:?} gate {:?}: completion mode {:?} is reserved but not yet implemented",
                        state_name,
                        gate_name,
                        completion
                    ));
                }
            }
            Ok(Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: source.override_default.clone(),
                completion: source.completion.clone(),
                name_filter: source.name_filter.clone(),
                overridable,
                request: String::new(),
                leg: String::new(),
                expect: None,
            })
        }
        GATE_TYPE_REQUEST_LEG => {
            let expect = match &source.expect {
                None => None,
                Some(raw) => Some(compile_expect(state_name, gate_name, raw)?),
            };
            let gate = Gate {
                gate_type: source.gate_type.clone(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: source.override_default.clone(),
                completion: None,
                name_filter: None,
                overridable,
                request: source.request.clone(),
                leg: source.leg.clone(),
                expect,
            };
            super::types::validate_request_leg_gate(state_name, gate_name, &gate)
                .map_err(|e| anyhow!(e))?;
            Ok(gate)
        }
        other => Err(anyhow!(
            "state {:?} gate {:?}: unsupported gate type {:?}; supported types: {}. \
             Field-based gates (field_not_empty, field_equals) have been replaced by accepts/when. \
             Use accepts blocks for evidence schema and when conditions for routing.",
            state_name,
            gate_name,
            other,
            SUPPORTED_GATE_TYPES.join(", ")
        )),
    }
}

/// Convert a `request-leg` gate's raw `expect` value into its compiled form.
///
/// The shape is a map from payload key to a list of scalar values. Anything
/// else is refused here, naming the state and gate; emptiness and scalar
/// checks on the converted map run in `validate_request_leg_gate`, which the
/// compiled-template validator shares.
fn compile_expect(
    state_name: &str,
    gate_name: &str,
    raw: &serde_yaml_ng::Value,
) -> anyhow::Result<BTreeMap<String, Vec<serde_json::Value>>> {
    let serde_yaml_ng::Value::Mapping(map) = raw else {
        return Err(anyhow!(
            "state {:?} gate {:?}: expect must be a map from payload key to a list of values",
            state_name,
            gate_name
        ));
    };
    let mut out = BTreeMap::new();
    for (key, values) in map {
        let Some(key) = key.as_str() else {
            return Err(anyhow!(
                "state {:?} gate {:?}: expect keys must be strings naming payload keys",
                state_name,
                gate_name
            ));
        };
        let serde_yaml_ng::Value::Sequence(list) = values else {
            return Err(anyhow!(
                "state {:?} gate {:?}: expect key {:?} must map to a list of values, such as [a, b]",
                state_name,
                gate_name,
                key
            ));
        };
        let mut converted = Vec::with_capacity(list.len());
        for value in list {
            let json = serde_json::to_value(value).map_err(|e| {
                anyhow!(
                    "state {:?} gate {:?}: expect key {:?} has a value that is not valid JSON: {}",
                    state_name,
                    gate_name,
                    key,
                    e
                )
            })?;
            converted.push(json);
        }
        out.insert(key.to_string(), converted);
    }
    Ok(out)
}

/// Split a markdown file into front-matter and body.
/// Returns (frontmatter_str, body_str) if the file starts with `---`.
pub(crate) fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim_start_matches('\n');
    if !content.starts_with("---") {
        return None;
    }
    // Find the closing `---` delimiter.
    let after_open = &content[3..];
    // Skip a newline immediately after the opening `---`.
    let after_open = after_open.trim_start_matches('\n');
    // Find the closing delimiter.
    let close_pos = find_frontmatter_close(after_open)?;
    let frontmatter = &after_open[..close_pos];
    let rest = &after_open[close_pos..];
    // Skip past the closing `---` line.
    let body_start = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
    Some((frontmatter, &rest[body_start..]))
}

fn find_frontmatter_close(s: &str) -> Option<usize> {
    let mut pos = 0;
    for line in s.lines() {
        if line.trim() == "---" {
            return Some(pos);
        }
        pos += line.len() + 1; // +1 for the newline
    }
    None
}

/// The `<!-- details -->` marker used to split directive from details content.
const DETAILS_MARKER: &str = "<!-- details -->";

/// Extract directive and details content for each declared state from the
/// markdown body.
///
/// States are identified by `## <state-name>` headings. Content between two
/// consecutive state headings belongs to the first. The declared state list
/// from the front-matter is the authority — headings that don't match a
/// declared state name are treated as directive content, not state boundaries.
///
/// Within each state's content, if a `<!-- details -->` line is found, the
/// content before it becomes the directive and the content after becomes the
/// details. Only the first occurrence of the marker is used. If no marker is
/// present, the entire content is the directive and details is empty.
fn extract_directives(
    states: &HashMap<String, SourceState>,
    body: &str,
) -> HashMap<String, (String, String)> {
    let state_names: std::collections::HashSet<&str> = states.keys().map(|s| s.as_str()).collect();

    let mut directives: HashMap<String, (String, String)> = HashMap::new();
    let mut current_state: Option<&str> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = parse_h2_heading(line) {
            if state_names.contains(heading) {
                // Save the previous state's directive.
                if let Some(state) = current_state {
                    directives.insert(state.to_string(), split_directive_details(&current_lines));
                }
                current_state = Some(heading);
                current_lines.clear();
            } else {
                // Not a state boundary — treat as content.
                current_lines.push(line);
            }
        } else {
            current_lines.push(line);
        }
    }

    // Save the last state's directive.
    if let Some(state) = current_state {
        directives.insert(state.to_string(), split_directive_details(&current_lines));
    }

    directives
}

/// Split collected lines into (directive, details) at the first `<!-- details -->` marker.
fn split_directive_details(lines: &[&str]) -> (String, String) {
    let marker_pos = lines.iter().position(|line| line.trim() == DETAILS_MARKER);

    match marker_pos {
        Some(pos) => {
            let directive = lines[..pos].join("\n").trim().to_string();
            let details = lines[pos + 1..].join("\n").trim().to_string();
            (directive, details)
        }
        None => {
            let directive = lines.join("\n").trim().to_string();
            (directive, String::new())
        }
    }
}

/// If the line is a `## heading`, return the heading text.
fn parse_h2_heading(line: &str) -> Option<&str> {
    let line = line.trim_end();
    line.strip_prefix("## ").map(|s| s.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    // -----------------------------------------------------------------------
    // Variable declarations: values:, pattern:, rebind:
    // -----------------------------------------------------------------------

    mod variable_constraints {
        use super::*;

        /// The compiled JSON of existing fixtures, captured before `values:`,
        /// `pattern:`, and `rebind:` existed. A template that declares none of
        /// them must compile to the same bytes, or every existing session's
        /// template hash stops matching its cache entry.
        #[test]
        fn compile_output_is_unchanged_for_templates_without_constraints() {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"));
            for name in ["hello-koto", "skip-if-vars"] {
                let source = root
                    .join("test/functional/fixtures/templates")
                    .join(format!("{name}.md"));
                let snapshot = root
                    .join("tests/fixtures/compile-snapshot")
                    .join(format!("{name}.json"));
                let compiled = compile(&source, false).unwrap();
                let json = serde_json::to_string_pretty(&compiled).unwrap();
                if std::env::var_os("KOTO_UPDATE_COMPILE_SNAPSHOT").is_some() {
                    std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
                    std::fs::write(&snapshot, &json).unwrap();
                }
                let expected = std::fs::read_to_string(&snapshot).unwrap();
                assert_eq!(json, expected, "compiled output of {name} changed");
            }
        }

        /// A one-state template whose only variable is declared by `decl`
        /// (YAML lines indented under the variable name).
        fn template_with_variable(decl: &str) -> String {
            format!(
                "---\nname: t\nversion: \"1.0\"\ninitial_state: done\nvariables:\n  X:\n{}\nstates:\n  done:\n    terminal: true\n---\n\n## done\n\nDone {{{{X}}}}.\n",
                decl.lines()
                    .map(|l| format!("    {}", l))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }

        fn compile_decl(decl: &str) -> anyhow::Result<CompiledTemplate> {
            let f = write_temp(&template_with_variable(decl));
            compile(f.path(), true)
        }

        fn compile_err(decl: &str) -> String {
            match compile_decl(decl) {
                Ok(_) => panic!("declaration should be refused:\n{decl}"),
                Err(e) => e.to_string(),
            }
        }

        #[test]
        fn declared_constraints_reach_the_compiled_template() {
            let t = compile_decl("values: [yes, no]\ndefault: \"no\"\nrebind: true").unwrap();
            let x = &t.variables["X"];
            assert_eq!(x.values, vec!["yes".to_string(), "no".to_string()]);
            assert!(x.rebind);
            let t = compile_decl("pattern: \"[a-z]+\"\nrequired: true").unwrap();
            assert_eq!(t.variables["X"].pattern, "[a-z]+");
        }

        #[test]
        fn an_unknown_key_is_refused_naming_the_variable_and_key() {
            let err = compile_err("valuez: [a]");
            assert!(err.contains("X"), "{err}");
            assert!(err.contains("valuez"), "{err}");
        }

        #[test]
        fn values_and_pattern_together_are_refused() {
            let err = compile_err("values: [a]\npattern: \"a\"\nrequired: true");
            assert!(
                err.contains("\"X\"") && err.contains("at most one"),
                "{err}"
            );
        }

        #[test]
        fn an_empty_values_list_is_refused() {
            let err = compile_err("values: []\nrequired: true");
            assert!(
                err.contains("\"X\"") && err.contains("must not be empty"),
                "{err}"
            );
        }

        #[test]
        fn a_values_entry_outside_the_allowlist_is_refused() {
            let err = compile_err("values: [ok, \"a;b\"]\nrequired: true");
            assert!(err.contains("\"X\"") && err.contains("a;b"), "{err}");
        }

        #[test]
        fn an_invalid_pattern_is_refused() {
            let err = compile_err("pattern: \"[a-\"\nrequired: true");
            assert!(
                err.contains("\"X\"") && err.contains("not a valid regular expression"),
                "{err}"
            );
        }

        #[test]
        fn a_lookaround_pattern_is_refused() {
            let err = compile_err("pattern: \"(?=a)a\"\nrequired: true");
            assert!(err.contains("\"X\""), "{err}");
        }

        #[test]
        fn a_pattern_matches_the_whole_value() {
            let t = compile_decl("pattern: \"[a-z]+\"\nrequired: true").unwrap();
            let x = &t.variables["X"];
            assert!(x.satisfies_constraint("abc"));
            assert!(!x.satisfies_constraint("abc-1"));
        }

        #[test]
        fn a_default_outside_the_constraint_is_refused() {
            let err = compile_err("values: [\"yes\", \"no\"]\ndefault: maybe");
            assert!(err.contains("\"X\""), "{err}");
            assert!(err.contains("maybe"), "{err}");
            assert!(err.contains("values:[yes,no]"), "{err}");
        }

        #[test]
        fn an_optional_variable_whose_constraint_rejects_empty_needs_a_default() {
            let err = compile_err("values: [\"yes\", \"no\"]");
            assert!(err.contains("\"X\"") && err.contains("optional"), "{err}");
            // A constraint admitting the empty value compiles without one.
            compile_decl("pattern: \"([1-9]|[1-4][0-9]|50)?\"").unwrap();
            // So does a required variable, which is never materialized empty.
            compile_decl("values: [\"yes\", \"no\"]\nrequired: true").unwrap();
        }

        #[test]
        fn rebind_accepts_only_a_boolean() {
            let err = compile_err("rebind: \"yes\"");
            assert!(err.contains("rebind"), "{err}");
            compile_decl("rebind: true").unwrap();
        }

        #[test]
        fn a_capture_cannot_write_a_declared_rebind_variable() {
            let src = "---\nname: t\nversion: \"1.0\"\ninitial_state: work\nvariables:\n  MERGE:\n    values: [\"true\", \"false\"]\n    default: \"false\"\n    rebind: true\nstates:\n  work:\n    default_action:\n      command: \"echo true\"\n      capture_stdout_as: MERGE\n    transitions:\n      - target: done\n  done:\n    terminal: true\n---\n\n## work\n\nWork.\n\n## done\n\nDone.\n";
            let f = write_temp(src);
            let err = compile(f.path(), true).unwrap_err().to_string();
            assert!(err.contains("MERGE") && err.contains("collides"), "{err}");
        }
    }

    /// Write a parent template source to `<dir>/<parent_file>` alongside a
    /// default child template at `<dir>/<child_file>`. Returns the
    /// `TempDir` (to keep it alive) and the parent template path.
    ///
    /// The child is a minimal valid template; by default it includes a
    /// terminal state with `skipped_marker: true` so F5 does not fire.
    /// Pass `child_src` to override with a template that should trigger F5
    /// or exercise other child-compile behavior.
    fn write_parent_with_child(
        parent_src: &str,
        parent_file: &str,
        child_file: &str,
        child_src: Option<&str>,
    ) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let child = child_src.unwrap_or(
            r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
      - target: skipped
  done:
    terminal: true
  skipped:
    terminal: true
    skipped_marker: true
---

## start

Do work.

## done

Complete.

## skipped

Skipped.
"#,
        );
        std::fs::write(dir.path().join(child_file), child).unwrap();
        let parent_path = dir.path().join(parent_file);
        std::fs::write(&parent_path, parent_src).unwrap();
        (dir, parent_path)
    }

    #[test]
    fn valid_template_compiles_correctly() {
        let src = r#"---
name: quick-task
version: "1.0"
description: A focused task workflow
initial_state: assess

variables:
  TASK:
    description: What to build
    required: true

states:
  assess:
    transitions:
      - target: done
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

## done

Work is complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        assert_eq!(result.format_version, 1);
        assert_eq!(result.name, "quick-task");
        assert_eq!(result.version, "1.0");
        assert_eq!(result.initial_state, "assess");
        assert!(result.states.contains_key("assess"));
        assert!(result.states.contains_key("done"));

        let assess = &result.states["assess"];
        assert_eq!(assess.directive, "Analyze the task: {{TASK}}");
        assert_eq!(assess.transitions.len(), 1);
        assert_eq!(assess.transitions[0].target, "done");
        assert!(assess.transitions[0].when.is_none());

        let done = &result.states["done"];
        assert!(done.terminal);
        assert_eq!(done.directive, "Work is complete.");

        let var = &result.variables["TASK"];
        assert!(var.required);
        assert_eq!(var.description, "What to build");
    }

    #[test]
    fn missing_name_returns_error() {
        let src = r#"---
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("missing required field: name"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn missing_initial_state_returns_error() {
        let src = r#"---
name: test
version: "1.0"
states:
  start:
    terminal: true
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string()
                .contains("missing required field: initial_state"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let src = r#"---
name: [broken yaml
version: "1.0"
---
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("invalid YAML") || err.to_string().contains("YAML"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn no_frontmatter_returns_error() {
        let src = "This is not a valid template.\n";
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("front-matter"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn unknown_gate_type_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: unknown_type
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("unsupported gate type"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn field_not_empty_gate_rejected() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: field_not_empty
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported gate type"), "got: {}", msg);
        assert!(msg.contains("accepts/when"), "got: {}", msg);
    }

    #[test]
    fn field_equals_gate_rejected() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: field_equals
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported gate type"), "got: {}", msg);
        assert!(msg.contains("accepts/when"), "got: {}", msg);
    }

    #[test]
    fn command_gate_empty_command_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    gates:
      my_gate:
        type: command
        command: ""
---

## start

Directive.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("command must not be empty"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn compiled_json_round_trips() {
        let src = r#"---
name: round-trip
version: "2.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

The one and only state.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);
    }

    #[test]
    fn undefined_transition_target_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: nonexistent
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("undefined transition target"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn state_missing_directive_returns_error() {
        let src = r#"---
name: test
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
  orphan:
    terminal: true
---

## start

Hello.
"#;
        // orphan has no ## orphan heading in body
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        assert!(
            err.to_string().contains("orphan") && err.to_string().contains("directive"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn accepts_when_integration_compiles() {
        let src = r#"---
name: review
version: "1.0"
initial_state: analyze
states:
  analyze:
    integration: delegate_review
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: review
        when:
          decision: escalate
  deploy:
    transitions:
      - target: done
  review:
    transitions:
      - target: done
  done:
    terminal: true
---

## analyze

Review the results.

## deploy

Deploy to production.

## review

Escalate to senior review.

## done

Complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let analyze = &result.states["analyze"];
        assert_eq!(analyze.integration, Some("delegate_review".to_string()));
        assert!(analyze.accepts.is_some());
        let accepts = analyze.accepts.as_ref().unwrap();
        assert!(accepts.contains_key("decision"));
        let schema = &accepts["decision"];
        assert_eq!(schema.field_type, "enum");
        assert!(schema.required);
        assert_eq!(schema.values, vec!["proceed", "escalate"]);

        assert_eq!(analyze.transitions.len(), 2);
        assert_eq!(analyze.transitions[0].target, "deploy");
        assert!(analyze.transitions[0].when.is_some());
        let when = analyze.transitions[0].when.as_ref().unwrap();
        assert_eq!(when["decision"], serde_json::json!("proceed"));
    }

    #[test]
    fn command_gate_alongside_accepts_when() {
        // A gate with gates.* routing alongside agent accepts/when compiles in strict mode.
        let src = r#"---
name: mixed
version: "1.0"
initial_state: check
states:
  check:
    accepts:
      decision:
        type: enum
        values: [go, stop]
        required: true
    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
      - target: halt
        when:
          gates.ci.exit_code: 1
    gates:
      ci:
        type: command
        command: ./check-ci.sh
  done:
    terminal: true
  halt:
    terminal: true
---

## check

Check the environment and decide.

## done

Proceed.

## halt

Stop.
"#;
        let f = write_temp(src);
        compile(f.path(), true).unwrap();
    }

    #[test]
    fn compiled_json_round_trips_with_evidence_routing() {
        let src = r#"---
name: evidence-rt
version: "1.0"
initial_state: decide
states:
  decide:
    accepts:
      choice:
        type: enum
        values: [a, b]
        required: true
    transitions:
      - target: path_a
        when:
          choice: a
      - target: path_b
        when:
          choice: b
    integration: my_tool
  path_a:
    transitions:
      - target: done
  path_b:
    transitions:
      - target: done
  done:
    terminal: true
---

## decide

Pick a path.

## path_a

Path A.

## path_b

Path B.

## done

Complete.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);
    }

    #[test]
    fn details_marker_splits_directive_and_details() {
        let src = r#"---
name: details-test
version: "1.0"
initial_state: work
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do the main task.

<!-- details -->

Here are some extra guidelines:
- Step 1
- Step 2

## done

Work is complete.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let work = &result.states["work"];
        assert_eq!(work.directive, "Do the main task.");
        assert_eq!(
            work.details,
            "Here are some extra guidelines:\n- Step 1\n- Step 2"
        );

        // State without marker should have empty details.
        let done = &result.states["done"];
        assert_eq!(done.directive, "Work is complete.");
        assert!(done.details.is_empty());
    }

    #[test]
    fn no_details_marker_produces_empty_details() {
        let src = r#"---
name: no-details
version: "1.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

Just a directive, no details marker here.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let only = &result.states["only"];
        assert_eq!(only.directive, "Just a directive, no details marker here.");
        assert!(only.details.is_empty());
    }

    #[test]
    fn multiple_details_markers_only_first_splits() {
        let src = r#"---
name: multi-details
version: "1.0"
initial_state: work
states:
  work:
    terminal: true
---

## work

The directive part.

<!-- details -->

First details section.

<!-- details -->

This stays in details, not a second split.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();

        let work = &result.states["work"];
        assert_eq!(work.directive, "The directive part.");
        assert_eq!(
            work.details,
            "First details section.\n\n<!-- details -->\n\nThis stays in details, not a second split."
        );
    }

    #[test]
    fn compiled_json_round_trips_with_details() {
        let src = r#"---
name: round-trip-details
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Main directive.

<!-- details -->

Extra context for first visit.

## done

Complete.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();

        // Verify the details field is populated.
        assert_eq!(
            compiled.states["start"].details,
            "Extra context for first visit."
        );
        assert!(compiled.states["done"].details.is_empty());

        // Round-trip through JSON.
        let json = serde_json::to_string(&compiled).unwrap();
        let restored: CompiledTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(compiled, restored);

        // Verify details is present in JSON for start but absent for done.
        let json_val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json_val["states"]["start"]["details"].is_string());
        assert!(json_val["states"]["done"].get("details").is_none());
    }

    // D5 integration: verify compile() propagates strict through to validate().
    // scenario-1 (strict=true): legacy-gate template fails compilation.
    // scenario-2 (strict=false): legacy-gate template compiles with a warning to stderr.
    #[test]
    fn compile_strict_true_errors_on_legacy_gate() {
        let src = r#"---
name: legacy
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: ./check.sh
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do work.

## done

Done.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("has no gates.* routing"),
            "expected D5 error, got: {}",
            msg
        );
        assert!(
            msg.contains("--allow-legacy-gates"),
            "error should hint at --allow-legacy-gates, got: {}",
            msg
        );
    }

    #[test]
    fn compile_strict_false_permits_legacy_gate() {
        let src = r#"---
name: legacy
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: ./check.sh
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do work.

## done

Done.
"#;
        let f = write_temp(src);
        // strict=false: warning to stderr, but compile returns Ok.
        compile(f.path(), false).unwrap();
    }

    // -------------------------------------------------------------------
    // Issue 7 — tasks accepts type, materialize_children hook, and the
    // narrow deny_unknown_fields on SourceState.
    // -------------------------------------------------------------------

    #[test]
    fn tasks_typed_accepts_field_compiles() {
        // Minimal batch parent template: plan_and_await has a tasks-typed
        // accepts field and transitions out on a structured condition. The
        // compiler must accept `type: tasks`.
        let src = r#"---
name: batch-parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
    transitions:
      - target: done
        when:
          tasks: submitted
  done:
    terminal: true
---

## plan_and_await

Submit the task list.

## done

All done.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        let state = &compiled.states["plan_and_await"];
        let accepts = state.accepts.as_ref().unwrap();
        assert_eq!(accepts["tasks"].field_type, "tasks");
        assert!(accepts["tasks"].required);
    }

    /// A minimal batch-parent template body that satisfies every Issue 8
    /// E-rule: accepts `tasks` (type tasks, required), has a
    /// children-complete gate, routes on it, and is non-terminal.
    /// Callers override the inner fields to trip specific rules.
    fn batch_parent_src(hook_extra: &str, gate_present: bool) -> String {
        let gate_block = if gate_present {
            "    gates:\n      cc:\n        type: children-complete\n"
        } else {
            ""
        };
        format!(
            r#"---
name: batch-parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
{gate}    materialize_children:
      from_field: tasks
      default_template: child.md
{extra}    transitions:
      - target: done
        when:
          gates.cc.all_complete: true
  done:
    terminal: true
---

## plan_and_await

Submit tasks.

## done

Done.
"#,
            gate = gate_block,
            extra = hook_extra,
        )
    }

    #[test]
    fn materialize_children_hook_parses_with_default_policy() {
        // The hook declares from_field and default_template only; the
        // failure_policy defaults to skip_dependents.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        let compiled = compile(&path, true).unwrap();
        let state = &compiled.states["plan_and_await"];
        let hook = state.materialize_children.as_ref().unwrap();
        assert_eq!(hook.from_field, "tasks");
        assert_eq!(hook.default_template, "child.md");
        assert_eq!(
            hook.failure_policy,
            crate::template::types::FailurePolicy::SkipDependents
        );
    }

    #[test]
    fn materialize_children_hook_accepts_continue_policy() {
        // failure_policy: continue is the explicit opt-out.
        let src = batch_parent_src("      failure_policy: continue\n", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        let compiled = compile(&path, true).unwrap();
        let hook = compiled.states["plan_and_await"]
            .materialize_children
            .as_ref()
            .unwrap();
        assert_eq!(
            hook.failure_policy,
            crate::template::types::FailurePolicy::Continue
        );
    }

    #[test]
    fn template_without_materialize_children_has_none() {
        // A plain (non-batch) template compiles with materialize_children
        // set to None on every state.
        let src = r#"---
name: plain
version: "1.0"
initial_state: work
states:
  work:
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        assert!(compiled.states["work"].materialize_children.is_none());
        assert!(compiled.states["done"].materialize_children.is_none());
    }

    #[test]
    fn failure_and_skipped_marker_flags_parse() {
        // failure: true and skipped_marker: true are set on a terminal state.
        // Runtime validation of "meaningful only when terminal" lives with
        // compile rules (Issue 8); here we only verify the parse.
        let src = r#"---
name: parent
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: failed_marker
      - target: skipped
  failed_marker:
    terminal: true
    failure: true
  skipped:
    terminal: true
    skipped_marker: true
---

## start

Start.

## failed_marker

Failed.

## skipped

Skipped.
"#;
        let f = write_temp(src);
        let compiled = compile(f.path(), true).unwrap();
        assert!(compiled.states["failed_marker"].failure);
        assert!(!compiled.states["failed_marker"].skipped_marker);
        assert!(compiled.states["skipped"].skipped_marker);
        assert!(!compiled.states["skipped"].failure);
        // The non-terminal entry state leaves both flags false.
        assert!(!compiled.states["start"].failure);
        assert!(!compiled.states["start"].skipped_marker);
    }

    #[test]
    fn source_state_rejects_unknown_fields() {
        // deny_unknown_fields on SourceState catches typos at compile time.
        let src = r#"---
name: parent
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
    materialize_childern: {}
---

## start

Hello.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        // Use the full Debug/chain format — the underlying serde unknown-field
        // error lives in the error chain beneath the "invalid YAML" wrapper.
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("unknown field") || msg.contains("materialize_childern"),
            "expected unknown-field error, got: {}",
            msg
        );
    }

    #[test]
    fn source_materialize_children_rejects_unknown_fields() {
        // Inner spec also uses deny_unknown_fields.
        let src = r#"---
name: parent
version: "1.0"
initial_state: plan_and_await
states:
  plan_and_await:
    accepts:
      tasks:
        type: tasks
        required: true
    materialize_children:
      from_field: tasks
      default_template: child.md
      unknown_knob: 7
    transitions:
      - target: done
        when:
          tasks: submitted
  done:
    terminal: true
---

## plan_and_await

Submit.

## done

Done.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("unknown field") || msg.contains("unknown_knob"),
            "expected unknown-field error, got: {}",
            msg
        );
    }

    // ---------------------------------------------------------------------
    // Issue 8: E9 (default_template resolves) and F5 (skipped_marker
    // reachability) live in compile.rs because they need the source path.
    // ---------------------------------------------------------------------

    #[test]
    fn issue8_e9_valid_child_path_compiles() {
        // Positive case for E9: a valid child template next to the parent
        // compiles without error.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        compile(&path, true).expect("valid parent+child pair should compile");
    }

    #[test]
    fn issue8_e9_missing_child_file_is_rejected() {
        // E9 negative: child template does not exist on disk.
        let src = batch_parent_src("", true);
        // Write the parent without creating child.md.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("parent.md");
        std::fs::write(&path, &src).unwrap();
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
    }

    #[test]
    fn issue8_e9_uncompilable_child_surfaces_nested_error() {
        // E9 negative: child exists but has a compile error of its own.
        // The E9 error wraps the child's error message so authors can
        // trace the root cause.
        let bad_child = r#"---
name: broken
version: "1.0"
initial_state: missing
states:
  start:
    terminal: true
---

## start

Hi.
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", Some(bad_child));
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
        // The child's own error message should be in the chain.
        assert!(
            msg.contains("initial_state") || msg.contains("missing"),
            "expected nested child error, got: {}",
            msg
        );
    }

    #[test]
    fn issue8_e9_self_referential_default_template_rejected() {
        // Guard against a parent naming itself as its own default_template.
        let src = batch_parent_src("", true);
        // Rename so default_template (`child.md`) resolves back to the parent.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("child.md");
        std::fs::write(&path, &src).unwrap();
        let err = compile(&path, true).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("E9:"), "expected E9 error, got: {}", msg);
        assert!(
            msg.contains("parent template itself") || msg.contains("distinct"),
            "expected self-reference hint, got: {}",
            msg
        );
    }

    #[test]
    fn issue8_f5_child_with_skipped_marker_is_silent() {
        // Positive F5: the default child template includes a terminal
        // state with skipped_marker: true; compilation emits no F5 warning.
        let src = batch_parent_src("", true);
        let (_dir, path) = write_parent_with_child(&src, "parent.md", "child.md", None);
        // compile returns Ok; F5 is a stderr warning only when missing.
        compile(&path, true).expect("child with skipped_marker should compile cleanly");
    }

    #[test]
    fn issue8_f5_child_without_skipped_marker_compiles_with_warning() {
        // Negative F5: child lacks any skipped_marker terminal. Compile
        // still returns Ok because F5 is a warning, not an error.
        let child_no_marker = r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Work.

## done

Done.
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) =
            write_parent_with_child(&src, "parent.md", "child.md", Some(child_no_marker));
        compile(&path, true).expect("F5 is a warning, not an error");
    }

    #[test]
    fn issue8_f5_skipped_marker_unreachable_still_warns() {
        // Child has a skipped_marker terminal, but it is not reachable
        // from initial_state via any transition chain. F5 should fire,
        // and compile still returns Ok.
        let child_unreachable = r#"---
name: child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
  orphan_skip:
    terminal: true
    skipped_marker: true
---

## start

Work.

## done

Done.

## orphan_skip

Skipped (but unreachable).
"#;
        let src = batch_parent_src("", true);
        let (_dir, path) =
            write_parent_with_child(&src, "parent.md", "child.md", Some(child_unreachable));
        compile(&path, true).expect("F5 is a warning, not an error");
    }

    #[test]
    fn compiled_template_not_deny_unknown_fields() {
        // Decision 3 in the design: deny_unknown_fields is applied to
        // SourceState, NOT CompiledTemplate/TemplateState. A compile-cache
        // JSON with an extra field must still deserialize so that newer
        // binaries remain cache-compatible with older readers. This test
        // guards the invariant by directly constructing JSON with a future
        // field and deserializing.
        let json = serde_json::json!({
            "format_version": 1,
            "name": "future",
            "version": "9.9",
            "initial_state": "start",
            "states": {
                "start": {
                    "directive": "hi",
                    "terminal": true,
                    "some_future_field": ["yes"]
                }
            }
        });
        // If deny_unknown_fields were on CompiledTemplate/TemplateState this
        // would error; the test is that it does not.
        let _: CompiledTemplate = serde_json::from_value(json).unwrap();
    }

    // -------------------------------------------------------------------
    // Issue 1: skip_if field validation rules (E-SKIP-TERMINAL,
    // E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS, W-SKIP-GATE-ABSENT).
    // -------------------------------------------------------------------

    /// scenario-1: skip_if field accepted in a valid template without error.
    #[test]
    fn skip_if_valid_field_compiles() {
        let src = r#"---
name: skip-if-test
version: "1.0"
initial_state: decide
states:
  decide:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
    skip_if:
      verdict: proceed
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: bypassed
        when:
          verdict: skip
  done:
    terminal: true
  bypassed:
    terminal: true
---

## decide

Make a decision.

## done

Done.

## bypassed

Bypassed.
"#;
        let f = write_temp(src);
        let result = compile(f.path(), true).unwrap();
        let decide = &result.states["decide"];
        assert!(decide.skip_if.is_some(), "skip_if should be Some");
        let skip_map = decide.skip_if.as_ref().unwrap();
        assert_eq!(skip_map.get("verdict"), Some(&serde_json::json!("proceed")));
    }

    /// scenario-2: E-SKIP-TERMINAL — skip_if on terminal state is rejected.
    #[test]
    fn skip_if_on_terminal_state_is_error() {
        let src = r#"---
name: skip-terminal
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
    skip_if:
      verdict: proceed
---

## start

Start.

## done

Done.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("E-SKIP-TERMINAL"),
            "expected E-SKIP-TERMINAL error, got: {}",
            msg
        );
        assert!(
            msg.contains("done"),
            "error should name the offending state, got: {}",
            msg
        );
    }

    /// scenario-3: E-SKIP-NO-TRANSITIONS — skip_if with no transitions is rejected.
    #[test]
    fn skip_if_with_no_transitions_is_error() {
        let src = r#"---
name: skip-no-transitions
version: "1.0"
initial_state: orphan
states:
  orphan:
    skip_if:
      verdict: proceed
    transitions: []
---

## orphan

No outgoing transitions.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("E-SKIP-NO-TRANSITIONS"),
            "expected E-SKIP-NO-TRANSITIONS error, got: {}",
            msg
        );
    }

    /// scenario-4: E-SKIP-AMBIGUOUS — skip_if values matching zero conditional transitions.
    #[test]
    fn skip_if_ambiguous_routing_is_error_zero_matches() {
        // All transitions are conditional and skip_if values match none of them.
        let src = r#"---
name: skip-ambiguous-zero
version: "1.0"
initial_state: decide
states:
  decide:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
    skip_if:
      verdict: unknown
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: bypassed
        when:
          verdict: skip
  done:
    terminal: true
  bypassed:
    terminal: true
---

## decide

Make a decision.

## done

Done.

## bypassed

Bypassed.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("E-SKIP-AMBIGUOUS"),
            "expected E-SKIP-AMBIGUOUS error (zero matches), got: {}",
            msg
        );
    }

    /// scenario-5: E-SKIP-AMBIGUOUS — skip_if values matching more than one conditional transition.
    ///
    /// E-SKIP-AMBIGUOUS is validated before the mutual exclusivity check (Rule 4), so the
    /// compile() path can be used directly. We construct a template where both conditional
    /// transitions match the skip_if evidence — each transition's when clause is a subset
    /// of the skip_if map. The mutual exclusivity check would later reject transitions that
    /// share no fields, but E-SKIP-AMBIGUOUS fires first.
    #[test]
    fn skip_if_ambiguous_routing_is_error() {
        // Two conditional transitions with disjoint field keys; both match skip_if evidence.
        // Transition A: when: {verdict: proceed} — matches skip_if.verdict = proceed
        // Transition B: when: {mode: fast}       — matches skip_if.mode = fast
        // skip_if has both verdict and mode, so BOTH transitions match.
        // E-SKIP-AMBIGUOUS fires before the mutual exclusivity check.
        let src = r#"---
name: skip-ambiguous-multi
version: "1.0"
initial_state: decide
states:
  decide:
    accepts:
      verdict:
        type: enum
        values: [proceed, skip]
        required: true
      mode:
        type: enum
        values: [fast, slow]
        required: true
    skip_if:
      verdict: proceed
      mode: fast
    transitions:
      - target: fast_path
        when:
          verdict: proceed
      - target: slow_path
        when:
          mode: fast
  fast_path:
    terminal: true
  slow_path:
    terminal: true
---

## decide

Make a decision.

## fast_path

Fast.

## slow_path

Slow.
"#;
        let f = write_temp(src);
        let err = compile(f.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("E-SKIP-AMBIGUOUS"),
            "expected E-SKIP-AMBIGUOUS error (multiple matches), got: {}",
            msg
        );
    }

    /// scenario-6: W-SKIP-GATE-ABSENT — skip_if key referencing undeclared gate name produces a warning (not an error).
    #[test]
    fn skip_if_gate_absent_produces_warning_not_error() {
        // skip_if references gates.missing_gate.exists but no gate named
        // missing_gate is declared on the state. This should compile
        // successfully (warning only, not error).
        let src = r#"---
name: skip-gate-absent
version: "1.0"
initial_state: check
states:
  check:
    skip_if:
      gates.missing_gate.exists: true
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check the context.

## done

Done.
"#;
        let f = write_temp(src);
        // W-SKIP-GATE-ABSENT is a warning, not an error — compile succeeds.
        compile(f.path(), true).expect("W-SKIP-GATE-ABSENT should warn but not fail compilation");
    }

    // -------------------------------------------------------------------
    // koto#204: transition context_assignments
    // -------------------------------------------------------------------

    /// A two-state template whose `start` state carries `state_extra` (YAML
    /// at state indentation, e.g. an accepts block) and the `transitions`
    /// list given, indented under `transitions:`.
    fn assignment_template(state_extra: &str, transitions: &str) -> String {
        format!(
            r#"---
name: assign
version: "1.0"
initial_state: start
variables:
  TOPIC:
    default: t
states:
  start:
{state_extra}    transitions:
{transitions}
  done:
    terminal: true
---

## start

Work.

## done

Done.
"#
        )
    }

    fn compile_src(src: &str) -> anyhow::Result<CompiledTemplate> {
        let f = write_temp(src);
        compile(f.path(), true)
    }

    const DETAIL_ACCEPTS: &str = "    accepts:\n      detail:\n        type: string\n";
    const CI_GATE: &str =
        "    gates:\n      ci:\n        type: command\n        command: \"true\"\n";

    #[test]
    fn transition_unknown_field_is_refused_naming_state_target_and_field() {
        let src = assignment_template(
            "",
            "      - target: done\n        context_assignment:\n          a: b",
        );
        let err = compile_src(&src).unwrap_err().to_string();
        assert!(err.contains("\"start\""), "state missing: {err}");
        assert!(err.contains("\"done\""), "target missing: {err}");
        assert!(err.contains("context_assignment"), "field missing: {err}");
    }

    #[test]
    fn transition_context_assignments_compile_into_the_transition() {
        let src = assignment_template(
            &format!("{DETAIL_ACCEPTS}{CI_GATE}"),
            "      - target: done\n        when:\n          gates.ci.exit_code: 0\n        context_assignments:\n          outcome: landed\n          reason: \"blocked: ${evidence.detail}\"\n          code: ${gates.ci.exit_code}\n          topic: \"{{TOPIC}}\"\n          count: 3",
        );
        let t = compile_src(&src).unwrap();
        let a = &t.states["start"].transitions[0].context_assignments;
        assert_eq!(a["outcome"], "landed");
        assert_eq!(a["reason"], "blocked: ${evidence.detail}");
        assert_eq!(a["code"], "${gates.ci.exit_code}");
        assert_eq!(a["topic"], "{{TOPIC}}");
        assert_eq!(a["count"], "3");
    }

    #[test]
    fn template_without_assignments_omits_the_field_from_compiled_json() {
        let src = assignment_template("", "      - target: done");
        let json = serde_json::to_string(&compile_src(&src).unwrap()).unwrap();
        assert!(!json.contains("context_assignments"), "got: {json}");
    }

    #[test]
    fn assignment_key_must_be_a_usable_context_key() {
        let src = assignment_template(
            "",
            "      - target: done\n        context_assignments:\n          \"bad key\": x",
        );
        let err = compile_src(&src).unwrap_err().to_string();
        assert!(
            err.contains("bad key") && err.contains("is not usable"),
            "got: {err}"
        );
    }

    #[test]
    fn assignment_value_must_not_be_a_mapping_or_sequence() {
        for value in ["{a: b}", "[a, b]"] {
            let src = assignment_template(
                "",
                &format!(
                    "      - target: done\n        context_assignments:\n          k: {value}"
                ),
            );
            let err = compile_src(&src).unwrap_err().to_string();
            assert!(err.contains("must be a string"), "{value}: {err}");
        }
    }

    #[test]
    fn assignment_evidence_field_must_be_declared_in_accepts() {
        // With an accepts block that lacks the field, and with none at all.
        for extra in [DETAIL_ACCEPTS, ""] {
            let src = assignment_template(
                extra,
                "      - target: done\n        context_assignments:\n          r: ${evidence.missing}",
            );
            let err = compile_src(&src).unwrap_err().to_string();
            assert!(
                err.contains("\"start\"")
                    && err.contains("\"done\"")
                    && err.contains("\"missing\"")
                    && err.contains("not declared in accepts"),
                "got: {err}"
            );
        }
    }

    #[test]
    fn assignment_gate_must_be_declared_on_the_state() {
        let src = assignment_template(
            "",
            "      - target: done\n        context_assignments:\n          r: ${gates.ci.exit_code}",
        );
        let err = compile_src(&src).unwrap_err().to_string();
        assert!(
            err.contains("references gate \"ci\" which is not declared in this state"),
            "got: {err}"
        );
    }

    #[test]
    fn assignment_variable_must_be_declared() {
        let src = assignment_template(
            "",
            "      - target: done\n        context_assignments:\n          r: \"{{NOPE}}\"",
        );
        let err = compile_src(&src).unwrap_err().to_string();
        assert!(
            err.contains("is not declared in the template's variables block"),
            "got: {err}"
        );
    }

    #[test]
    fn assignment_other_namespaces_are_refused() {
        for reference in ["${context.x}", "${foo.bar}", "${gates.ci}"] {
            let src = assignment_template(
                CI_GATE,
                &format!(
                    "      - target: done\n        when:\n          gates.ci.exit_code: 0\n        context_assignments:\n          r: \"{reference}\""
                ),
            );
            let err = compile_src(&src).unwrap_err().to_string();
            assert!(err.contains("unsupported reference"), "{reference}: {err}");
        }
    }

    // -----------------------------------------------------------------
    // overridable: false on gates
    // -----------------------------------------------------------------

    /// A one-state template whose `check` state declares the given gate
    /// body (already indented to sit under `gates: g:`).
    fn gate_template(gate_body: &str) -> String {
        format!(
            "---\nname: ov\nversion: \"1.0\"\ninitial_state: check\nstates:\n  check:\n    gates:\n      g:\n{}    transitions:\n      - target: done\n  done:\n    terminal: true\n---\n\n## check\n\nCheck.\n\n## done\n\nDone.\n",
            gate_body
        )
    }

    fn compile_err(src: &str) -> String {
        let f = write_temp(src);
        compile(f.path(), false)
            .expect_err("template should fail to compile")
            .to_string()
    }

    #[test]
    fn overridable_false_compiles_on_every_gate_type() {
        let bodies = [
            "        type: command\n        command: \"true\"\n",
            "        type: context-exists\n        key: some.key\n",
            "        type: context-matches\n        key: some.key\n        pattern: \"^ok$\"\n",
            "        type: children-complete\n",
            "        type: request-leg\n        request: req-a\n        leg: scope\n",
        ];
        for body in bodies {
            let src = gate_template(&format!("{}        overridable: false\n", body));
            let f = write_temp(&src);
            let compiled = compile(f.path(), false)
                .unwrap_or_else(|e| panic!("gate {:?} should compile: {}", body, e));
            let gate = &compiled.states["check"].gates["g"];
            assert!(
                !gate.overridable,
                "gate {:?} should be non-overridable",
                body
            );
            let json = serde_json::to_value(gate).unwrap();
            assert_eq!(json["overridable"], serde_json::json!(false));
        }
    }

    #[test]
    fn overridable_defaults_to_true_and_is_omitted_from_json() {
        let src = gate_template("        type: command\n        command: \"true\"\n");
        let f = write_temp(&src);
        let compiled = compile(f.path(), false).unwrap();
        let gate = &compiled.states["check"].gates["g"];
        assert!(gate.overridable);
        let json = serde_json::to_value(gate).unwrap();
        assert!(json.get("overridable").is_none(), "got {}", json);

        let explicit = gate_template(
            "        type: command\n        command: \"true\"\n        overridable: true\n",
        );
        let f = write_temp(&explicit);
        let compiled_explicit = compile(f.path(), false).unwrap();
        assert_eq!(
            serde_json::to_string_pretty(&compiled_explicit).unwrap(),
            serde_json::to_string_pretty(&compiled).unwrap(),
            "overridable: true must compile byte-identical to omitting it"
        );
    }

    #[test]
    fn misspelled_overridable_key_is_rejected_naming_state_gate_and_key() {
        let err = compile_err(&gate_template(
            "        type: command\n        command: \"true\"\n        overrideable: false\n",
        ));
        assert!(err.contains("\"check\""), "state missing: {}", err);
        assert!(err.contains("\"g\""), "gate missing: {}", err);
        assert!(err.contains("\"overrideable\""), "key missing: {}", err);
        assert!(err.contains("unknown key"), "{}", err);
    }

    #[test]
    fn overridable_rejects_non_boolean_values() {
        for value in ["\"no\"", "no", "\"false\"", "0", "[false]"] {
            let err = compile_err(&gate_template(&format!(
                "        type: command\n        command: \"true\"\n        overridable: {}\n",
                value
            )));
            assert!(
                err.contains("overridable must be a boolean"),
                "value {} gave: {}",
                value,
                err
            );
            assert!(
                err.contains("\"check\"") && err.contains("\"g\""),
                "{}",
                err
            );
        }
    }

    #[test]
    fn override_default_on_non_overridable_gate_is_rejected() {
        let err = compile_err(&gate_template(
            "        type: command\n        command: \"true\"\n        overridable: false\n        override_default:\n          exit_code: 0\n          error: \"\"\n",
        ));
        assert!(err.contains("override_default"), "{}", err);
        assert!(err.contains("overridable: false"), "{}", err);
        assert!(
            err.contains("\"check\"") && err.contains("\"g\""),
            "{}",
            err
        );
    }

    #[test]
    fn override_default_on_overridable_gate_still_compiles() {
        let src = gate_template(
            "        type: command\n        command: \"true\"\n        overridable: true\n        override_default:\n          exit_code: 0\n          error: \"\"\n",
        );
        let f = write_temp(&src);
        compile(f.path(), false).expect("override_default on an overridable gate is fine");
    }

    // -----------------------------------------------------------------
    // request-leg gates, payload paths, and D4 for non-overridable gates
    // -----------------------------------------------------------------

    mod request_leg_gate {
        use super::*;

        /// A template whose `run` state is `state_yaml` (indented to sit
        /// under `run:`), with terminals `scoped`, `declined`, `absent`,
        /// and `other`, and a declared `REQ` variable.
        fn template(state_yaml: &str) -> String {
            format!(
                "---\nname: leg\nversion: \"1.0\"\ninitial_state: run\nvariables:\n  REQ:\n    default: req-a\nstates:\n  run:\n{state_yaml}  scoped:\n    terminal: true\n  declined:\n    terminal: true\n  absent:\n    terminal: true\n  other:\n    terminal: true\n---\n\n## run\n\nRun.\n\n## scoped\n\nScoped.\n\n## declined\n\nDeclined.\n\n## absent\n\nAbsent.\n\n## other\n\nOther.\n"
            )
        }

        /// A `run` state with one gate `g` (body indented under `g:`) and
        /// the given transitions block.
        fn state(gate_body: &str, transitions: &str) -> String {
            format!("    gates:\n      g:\n{gate_body}    transitions:\n{transitions}")
        }

        const LEG_GATE: &str =
            "        type: request-leg\n        request: \"{{REQ}}\"\n        leg: scope\n";
        const OUTCOME_ARMS: &str = "      - target: scoped\n        when:\n          gates.g.payload.outcome: scoped\n      - target: declined\n        when:\n          gates.g.payload.outcome: declined\n";

        fn compile_src(src: &str, strict: bool) -> anyhow::Result<CompiledTemplate> {
            let f = write_temp(src);
            compile(f.path(), strict)
        }

        fn err(src: &str, strict: bool) -> String {
            compile_src(src, strict)
                .expect_err("template should fail to compile")
                .to_string()
        }

        fn non_overridable(body: &str) -> String {
            format!("{body}        overridable: false\n")
        }

        #[test]
        fn a_request_leg_gate_with_request_and_leg_compiles() {
            let src = template(&state(&non_overridable(LEG_GATE), OUTCOME_ARMS));
            let compiled = compile_src(&src, true).expect("the /deliver scope_run shape");
            let gate = &compiled.states["run"].gates["g"];
            assert_eq!(gate.gate_type, "request-leg");
            assert_eq!(gate.request, "{{REQ}}");
            assert_eq!(gate.leg, "scope");
            let json = serde_json::to_value(gate).unwrap();
            assert_eq!(json["request"], "{{REQ}}");
            assert_eq!(json["leg"], "scope");
            assert!(json.get("expect").is_none());
        }

        #[test]
        fn request_and_leg_are_required() {
            for body in [
                "        type: request-leg\n        leg: scope\n",
                "        type: request-leg\n        request: req-a\n",
                "        type: request-leg\n        request: \"\"\n        leg: scope\n",
                "        type: request-leg\n        request: req-a\n        leg: \"\"\n",
            ] {
                let e = err(&template(&state(body, OUTCOME_ARMS)), false);
                assert!(
                    e.contains("\"run\"") && e.contains("\"g\"") && e.contains("non-empty"),
                    "{e}"
                );
            }
        }

        #[test]
        fn a_literal_request_or_leg_is_checked_against_the_store_rules() {
            let e = err(
                &template(&state(
                    "        type: request-leg\n        request: Req-A\n        leg: scope\n",
                    OUTCOME_ARMS,
                )),
                false,
            );
            assert!(e.contains("not a valid request id"), "{e}");
            let e = err(
                &template(&state(
                    "        type: request-leg\n        request: \"req/../a\"\n        leg: scope\n",
                    OUTCOME_ARMS,
                )),
                false,
            );
            assert!(e.contains("not a valid request id"), "{e}");
            let e = err(
                &template(&state(
                    "        type: request-leg\n        request: req-a\n        leg: \"-scope\"\n",
                    OUTCOME_ARMS,
                )),
                false,
            );
            assert!(e.contains("not a valid leg name"), "{e}");
        }

        #[test]
        fn a_reference_is_checked_for_declaration_not_for_shape() {
            // Declared: accepted even though `{{REQ}}` is not itself a valid id.
            compile_src(&template(&state(LEG_GATE, OUTCOME_ARMS)), false).unwrap();
            // Undeclared: refused, naming the field.
            let e = err(
                &template(&state(
                    "        type: request-leg\n        request: req-a\n        leg: \"{{NOPE}}\"\n",
                    OUTCOME_ARMS,
                )),
                false,
            );
            assert!(e.contains("NOPE") && e.contains("'leg'"), "{e}");
        }

        #[test]
        fn expect_must_map_keys_to_non_empty_scalar_lists() {
            let ok = format!("{LEG_GATE}        expect:\n          outcome: [scoped, declined]\n          n: [1, true]\n");
            let compiled = compile_src(&template(&state(&ok, OUTCOME_ARMS)), false).unwrap();
            let expect = compiled.states["run"].gates["g"].expect.clone().unwrap();
            assert_eq!(
                expect["outcome"],
                vec![serde_json::json!("scoped"), serde_json::json!("declined")]
            );

            let bad = [
                ("        expect: [a]\n", "must be a map"),
                ("        expect: {}\n", "at least one payload key"),
                (
                    "        expect:\n          outcome: scoped\n",
                    "must map to a list",
                ),
                (
                    "        expect:\n          outcome: []\n",
                    "at least one value",
                ),
                (
                    "        expect:\n          outcome: [{a: 1}]\n",
                    "non-scalar value",
                ),
                (
                    "        expect:\n          outcome: [[a]]\n",
                    "non-scalar value",
                ),
            ];
            for (expect, needle) in bad {
                let e = err(
                    &template(&state(&format!("{LEG_GATE}{expect}"), OUTCOME_ARMS)),
                    false,
                );
                assert!(e.contains(needle), "{expect:?}: {e}");
                assert!(e.contains("\"run\"") && e.contains("\"g\""), "{e}");
            }
        }

        #[test]
        fn a_request_leg_gate_without_routing_gets_d5() {
            let src = template(&state(LEG_GATE, "      - target: scoped\n"));
            let e = err(&src, true);
            assert!(e.contains("has no gates.* routing"), "{e}");
            compile_src(&src, false).expect("permissive mode only warns");
        }

        #[test]
        fn d3_accepts_payload_key_paths_on_a_request_leg_gate() {
            let arms = "      - target: scoped\n        when:\n          gates.g.payload.outcome: scoped\n      - target: declined\n        when:\n          gates.g.payload.detail.kind: declined\n          gates.g.payload.outcome: declined\n";
            compile_src(&template(&state(&non_overridable(LEG_GATE), arms)), true)
                .expect("one or more segments after payload");
        }

        #[test]
        fn d3_rejects_the_whole_payload_object() {
            let arms = "      - target: scoped\n        when:\n          gates.g.payload: scoped\n";
            let e = err(&template(&state(LEG_GATE, arms)), false);
            assert!(e.contains("object field \"payload\""), "{e}");
            // An object value is refused as a non-scalar, as today.
            let arms = "      - target: scoped\n        when:\n          gates.g.payload:\n            outcome: scoped\n";
            let e = err(&template(&state(LEG_GATE, arms)), false);
            assert!(e.contains("must be a scalar"), "{e}");
        }

        #[test]
        fn d3_still_rejects_deeper_paths_everywhere_else() {
            let invalid = "has invalid format; expected \"gates.<gate>.<field>\"";
            // Under another request-leg field.
            let arms =
                "      - target: scoped\n        when:\n          gates.g.status.x: success\n";
            assert!(err(&template(&state(LEG_GATE, arms)), false).contains(invalid));
            // An empty segment after payload.
            let arms = "      - target: scoped\n        when:\n          gates.g.payload..x: a\n";
            assert!(err(&template(&state(LEG_GATE, arms)), false).contains(invalid));
            // `payload` on other gate types.
            for body in [
                "        type: context-matches\n        key: k\n        pattern: x\n",
                "        type: command\n        command: \"true\"\n",
                "        type: children-complete\n",
            ] {
                let arms =
                    "      - target: scoped\n        when:\n          gates.g.payload.outcome: a\n";
                let e = err(&template(&state(body, arms)), false);
                assert!(e.contains(invalid), "{body:?}: {e}");
            }
        }

        #[test]
        fn a_mixed_when_clause_with_agent_evidence_compiles() {
            let src = template(
                "    accepts:\n      child_returned:\n        type: enum\n        values: [yes, no]\n        required: true\n    gates:\n      scope_leg:\n        type: request-leg\n        request: \"{{REQ}}\"\n        leg: scope\n        overridable: false\n    transitions:\n      - target: absent\n        when:\n          gates.scope_leg.bound: false\n          child_returned: \"yes\"\n      - target: scoped\n        when:\n          gates.scope_leg.bound: true\n          gates.scope_leg.payload.outcome: scoped\n",
            );
            compile_src(&src, true).expect("mixed gate and evidence arms compile");
        }

        // ----- D4 -----

        const D4_ERROR: &str = "no transition fires when all gates use override defaults";

        #[test]
        fn d4_exempts_arms_on_a_non_overridable_request_leg_gate() {
            let src = template(&state(&non_overridable(LEG_GATE), OUTCOME_ARMS));
            compile_src(&src, true).expect("the /deliver scope_run shape passes strict D4");
        }

        #[test]
        fn d4_still_fails_the_same_state_when_the_gate_is_overridable() {
            // The built-in default names no outcome, so nothing fires.
            let src = template(&state(LEG_GATE, OUTCOME_ARMS));
            let e = err(&src, true);
            assert!(e.contains(D4_ERROR), "{e}");
        }

        #[test]
        fn an_override_default_on_an_overridable_leg_gate_satisfies_d4() {
            let body = format!(
                "{LEG_GATE}        override_default:\n          found: true\n          disposition: resolved\n          bound: true\n          source: promoted\n          status: success\n          final_state: \"\"\n          template: \"\"\n          outcome: scoped\n          step: \"\"\n          reason: \"\"\n          valid: true\n          payload:\n            outcome: scoped\n          error: \"\"\n"
            );
            compile_src(&template(&state(&body, OUTCOME_ARMS)), true)
                .expect("the override default fires the scoped arm");
        }

        #[test]
        fn an_override_default_is_schema_checked_including_the_object_payload() {
            let body = format!(
                "{LEG_GATE}        override_default:\n          found: true\n          disposition: resolved\n          bound: true\n          source: promoted\n          status: success\n          final_state: \"\"\n          template: \"\"\n          outcome: scoped\n          step: \"\"\n          reason: \"\"\n          valid: true\n          payload: scoped\n          error: \"\"\n"
            );
            let e = err(&template(&state(&body, OUTCOME_ARMS)), false);
            assert!(
                e.contains("\"payload\"") && e.contains("expected: object"),
                "{e}"
            );
            let body = format!(
                "{LEG_GATE}        override_default:\n          found: true\n          error: \"\"\n"
            );
            let e = err(&template(&state(&body, OUTCOME_ARMS)), false);
            assert!(e.contains("missing required field"), "{e}");
        }

        const MATCHES_GATE: &str =
            "        type: context-matches\n        key: verdict\n        pattern: \"^merged$\"\n";
        const MATCHES_ARMS: &str =
            "      - target: other\n        when:\n          gates.g.matches: false\n";

        #[test]
        fn d4_exempts_arms_on_a_non_overridable_context_matches_gate() {
            let src = template(&state(&non_overridable(MATCHES_GATE), MATCHES_ARMS));
            compile_src(&src, true).expect("non-overridable context-matches is exempt");
        }

        #[test]
        fn d4_still_fails_the_overridable_context_matches_state() {
            let src = template(&state(MATCHES_GATE, MATCHES_ARMS));
            let e = err(&src, true);
            assert!(e.contains(D4_ERROR), "{e}");
        }

        #[test]
        fn d4_still_checks_arms_on_the_overridable_gate_of_a_mixed_state() {
            // `ci` is overridable and its only arm needs exit_code 1, which
            // its default (0) never gives; the non-overridable leg gate's
            // arm is left out of the check and cannot rescue the state.
            let src = template(
                "    gates:\n      ci:\n        type: command\n        command: \"true\"\n      leg:\n        type: request-leg\n        request: req-a\n        leg: scope\n        overridable: false\n    transitions:\n      - target: other\n        when:\n          gates.ci.exit_code: 1\n      - target: scoped\n        when:\n          gates.leg.payload.outcome: scoped\n          gates.ci.exit_code: 0\n",
            );
            let e = err(&src, true);
            assert!(e.contains(D4_ERROR), "{e}");

            // With the ci arm firing on its default, the state passes.
            let src = template(
                "    gates:\n      ci:\n        type: command\n        command: \"true\"\n      leg:\n        type: request-leg\n        request: req-a\n        leg: scope\n        overridable: false\n    transitions:\n      - target: other\n        when:\n          gates.ci.exit_code: 0\n      - target: scoped\n        when:\n          gates.leg.payload.outcome: scoped\n          gates.ci.exit_code: 1\n",
            );
            compile_src(&src, true).expect("the overridable arm fires on its default");
        }
    }

    // ---------------------------------------------------------------------
    // Decider declarations: a template that declares none is untouched.
    // ---------------------------------------------------------------------

    /// `template_hash` of `tests/fixtures/template-hash/no-decider.md`,
    /// captured by compiling it with the koto on main before decider blocks
    /// existed. The fixture has described enum, boolean, and string fields,
    /// gates, a capture, and variables, so any change to how an undeclared
    /// field compiles moves this hash.
    const NO_DECIDER_TEMPLATE_HASH: &str =
        "4c831681fe2c9ee56ed6bce3568f08b2e1c756ad080deb5f7b5e63378b1224d6";

    #[test]
    fn template_without_decider_keeps_its_template_hash() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/template-hash/no-decider.md");
        let compiled = compile(&path, true).expect("fixture compiles");
        let json = serde_json::to_string_pretty(&compiled).unwrap();
        assert!(!json.contains("\"decider\""));
        assert_eq!(
            crate::cache::sha256_hex(json.as_bytes()),
            NO_DECIDER_TEMPLATE_HASH
        );
    }
}
