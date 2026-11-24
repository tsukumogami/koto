use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use regex::Regex;

use super::decider::{FieldDecider, FloorViolation, FloorViolationKind};

/// Regex for variable references in template strings: `{{KEY}}` where KEY is
/// uppercase letters, digits, and underscores.
pub const VAR_REF_PATTERN: &str = r"\{\{([A-Z][A-Z0-9_]*)\}\}";

/// Extract all `{{KEY}}` references from a string.
/// Used by compile-time validation and runtime substitution.
pub fn extract_refs(input: &str) -> Vec<String> {
    let re = Regex::new(VAR_REF_PATTERN).expect("VAR_REF_PATTERN is a valid regex");
    re.captures_iter(input)
        .map(|caps| caps[1].to_string())
        .collect()
}

/// A compiled template in FormatVersion=1 JSON format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledTemplate {
    pub format_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub initial_state: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, VariableDecl>,
    /// Read through [`deserialize_states`], which refuses a decider check
    /// with no spec (`E-DECIDER-CHECK-SPEC`). Written as before, so the
    /// compiled JSON and its hash don't change.
    #[serde(deserialize_with = "deserialize_states")]
    pub states: BTreeMap<String, TemplateState>,
    /// Environment variable names this template's commands need, read live
    /// on every tick in addition to koto's default list
    /// (DESIGN-koto-fixed-environment.md). Skipped when empty so a template
    /// that declares none keeps its compiled form and hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_env: Vec<String>,
    /// The seat names and section slugs `koto review record` may store as
    /// given for a session on this template; any other value is stored as
    /// `other`. Skipped when empty so a template that declares none keeps
    /// its compiled form and hash.
    #[serde(default, skip_serializing_if = "ReviewVocabulary::is_empty")]
    pub review_vocabulary: ReviewVocabulary,
}

/// The most names one `review_vocabulary` list may declare. Each declared
/// name can become a telemetry value, so the lists are kept well under the
/// number of distinct values a telemetry attribute can carry.
pub const REVIEW_VOCABULARY_MAX: usize = 100;

/// The review vocabulary a template declares (`review_vocabulary:` in its
/// front matter): which seat names and section slugs `koto review record`
/// records as given. A value the template doesn't declare is recorded as
/// `other`, and a template that declares none gets `other` for every
/// value, so a caller can't put an arbitrary string into telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ReviewVocabulary {
    /// Seat names, e.g. `clarity`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seats: Vec<String>,
    /// Section slugs, e.g. `scope-boundary` or `frontmatter`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<String>,
}

impl ReviewVocabulary {
    /// Whether the template declares no seat and no section.
    pub fn is_empty(&self) -> bool {
        self.seats.is_empty() && self.sections.is_empty()
    }
}

/// The error code a compiled template gets when one of its `decider-check`
/// gates has no `decider_check` spec.
pub const E_DECIDER_CHECK_SPEC: &str = "E-DECIDER-CHECK-SPEC";

/// The load-time refusal for a compiled `decider-check` gate with no spec,
/// or `None` when the gate is fine.
///
/// The compiler never writes such a gate: it refuses a check with no
/// criteria (`E-DECIDER-CHECK-FIELD`). But koto reads compiled JSON back
/// from the cache and a session's own directory without recompiling, so a
/// gate that lost its spec would otherwise load and then pass at run time
/// as a check with nothing to grade. It is a malformed template, not a
/// verdict the decider failed to give, so it is refused before any session
/// starts or resumes on it.
pub fn missing_decider_check_spec(
    state_name: &str,
    gate_name: &str,
    gate: &Gate,
) -> Option<String> {
    if gate.gate_type != GATE_TYPE_DECIDER_CHECK || gate.decider_check.is_some() {
        return None;
    }
    Some(format!(
        "{}: state {:?} check {:?}: the compiled template has a decider check with no \
         decider_check spec, so there is nothing to grade\n  \
         remedy: compile the template again from its source",
        E_DECIDER_CHECK_SPEC, state_name, gate_name
    ))
}

/// Read a compiled template's `states`, refusing any `decider-check` gate
/// with no spec ([`missing_decider_check_spec`]). Every place koto loads a
/// compiled template goes through `Deserialize`, so this one check covers
/// `koto init`, `koto next` on a session already in flight, and every other
/// command that acts on the template. Best-effort display readers (the
/// dashboard, the `/workflows` projection) already treat a template that
/// fails to parse as unreadable, and do the same here.
fn deserialize_states<'de, D>(deserializer: D) -> Result<BTreeMap<String, TemplateState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let states = BTreeMap::<String, TemplateState>::deserialize(deserializer)?;
    for (state_name, state) in &states {
        for (gate_name, gate) in &state.gates {
            if let Some(refusal) = missing_decider_check_spec(state_name, gate_name, gate) {
                return Err(serde::de::Error::custom(refusal));
            }
        }
    }
    Ok(states)
}

/// A variable declaration in a compiled template.
///
/// `values`, `pattern`, and `rebind` are skipped when unset, so a template
/// that declares none of them compiles to the same JSON (and so the same
/// template hash) as it did before they existed. The constraint checks live
/// in `src/template/variables.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VariableDecl {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
    /// Closed set of accepted values. Mutually exclusive with `pattern`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Regular expression (`regex` crate syntax) a value must match in full;
    /// applied as `^(?:<pattern>)$`. Mutually exclusive with `values`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// Whether the variable is re-applied from each invocation when a live
    /// session is attached. Non-rebind variables are fixed at `koto init`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rebind: bool,
}

/// A state declaration in a compiled template.
///
/// Note: `#[serde(deny_unknown_fields)]` is intentionally NOT applied here.
/// `CompiledTemplate` is loaded from the compile cache, which may be written
/// by a newer binary than the reader. Adding new fields to `CompiledTemplate`
/// must remain non-breaking for older binaries during version churn.
/// The strict unknown-field check lives on `SourceState` (the YAML
/// front-matter intermediate) instead — see `src/template/compile.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TemplateState {
    pub directive: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gates: BTreeMap<String, Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<BTreeMap<String, FieldSchema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_action: Option<ActionDecl>,
    /// Declares that this state spawns child workflows from a task-list
    /// evidence field when entered. Runtime behavior is implemented by the
    /// batch scheduler; this field is purely the compile-time contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_children: Option<MaterializeChildrenSpec>,
    /// When true and the state is terminal, marks this state as a failure
    /// outcome (propagated by the batch scheduler to the parent's
    /// `children-complete` gate output). Meaningful only when `terminal` is
    /// true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failure: bool,
    /// When true, the scheduler can synthesize child markers that land
    /// directly in this state to represent "skipped" children. Meaningful
    /// only when `terminal` is true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped_marker: bool,
    /// Optional auto-advance predicate. When all key-value pairs in this map
    /// match the available evidence (gate output + template variables), the
    /// engine auto-transitions from this state without waiting for agent
    /// evidence. Uses the same dot-path syntax as `when` clauses on transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if: Option<BTreeMap<String, serde_json::Value>>,
    /// The declared result of a terminal state: a flat map from key to a
    /// string value holding literal text, `{{VAR}}`, or `${context.<key>}`.
    /// When present, koto resolves it once on the terminal tick and it
    /// becomes the `WorkflowResult`'s `payload` in place of the
    /// evidence-derived one. Meaningful only when `terminal` is true; the
    /// grammar lives in [`crate::template::result_map`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, String>>,
    /// Context keys koto removes whenever the workflow enters this state,
    /// except on the session's first entry and except a key written since
    /// the entry (DESIGN-koto-ci-wait-stale-keys.md Decisions 1 and 2).
    ///
    /// Each is a literal key: the compiler refuses a `{{VAR}}` reference, a
    /// key outside the context-key grammar, a duplicate, the field on a
    /// terminal state, and a key some transition assigns through
    /// `context_assignments`; the source compiler also refuses an explicit
    /// empty list, which the compiled form can't tell from an absent one.
    /// Omitted from the compiled JSON when empty, so a template that doesn't
    /// declare it keeps its template hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_on_entry: Vec<String>,
    /// Rules this state's directive carries, each recorded as a
    /// `rule_delivered` event (`via: directive`) when `koto next` prints the
    /// directive with the entry's `when_set` value non-empty.
    ///
    /// Telemetry only: nothing routes on it and the response is unchanged.
    /// Omitted from the compiled JSON when empty, so a template that doesn't
    /// declare it keeps its template hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directive_rules: Vec<DirectiveRule>,
}

/// One `directive_rules` entry: a rule a state's directive hands the agent.
///
/// `rule_id`, `check` and `extent` are copied onto the `rule_delivered`
/// event; `when_set` names the variable or capture whose value carries the
/// rule, so a run where it renders as nothing records no delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirectiveRule {
    /// The rule's id, within koto's rule-id bounds.
    pub rule_id: String,
    /// The check the rule belongs to, recorded as `koto.check.name`.
    pub check: String,
    /// How much of the rule the directive carries.
    pub extent: crate::template::decider_check::RuleLevel,
    /// A declared variable or `capture_stdout_as` name.
    pub when_set: String,
}

/// Template-level declaration that a state fans out child workflows from an
/// agent-submitted task list.
///
/// Placed on `TemplateState` alongside `gates`, `accepts`, and
/// `default_action`. The compiler validates the hook at load time; the
/// scheduler (lands in a later phase) reads the hook at runtime to
/// materialize children.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterializeChildrenSpec {
    /// Name of the `accepts` field on this state that carries the task
    /// list. The field's declared type must be `tasks`.
    pub from_field: String,
    /// Path to the default child template to use when a task entry omits
    /// `template`. The compiler validates that the path resolves.
    pub default_template: String,
    /// Policy for propagating failures to dependent tasks within the batch.
    /// Defaults to `SkipDependents`.
    #[serde(default = "default_failure_policy")]
    pub failure_policy: FailurePolicy,
}

/// Policy controlling how a batch handles failures of upstream tasks.
///
/// - `SkipDependents` (default): tasks whose `waits_on` chain transitively
///   includes a failed task are synthesized as terminal skip markers.
/// - `Continue`: a failure of one task does not prevent dependents from
///   running; each task is judged independently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    SkipDependents,
    Continue,
}

/// Serde default helper: `FailurePolicy::SkipDependents`.
pub fn default_failure_policy() -> FailurePolicy {
    FailurePolicy::SkipDependents
}

/// A structured transition with an optional condition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<BTreeMap<String, serde_json::Value>>,
    /// Context writes that land with this transition (koto#204).
    ///
    /// Maps a context key to a value template: a literal, `{{VAR}}`,
    /// `${evidence.<field>}` or `${gates.<gate>.<path>}`, with references
    /// allowed inside a string literal. The engine resolves the values when
    /// the transition fires, records them on the `Transitioned` event, and
    /// writes them to the session's context store. `crate::template::assignments`
    /// owns the reference grammar.
    ///
    /// Omitted from the compiled JSON when empty, so a template without
    /// assignments compiles byte-identically to one built before the field
    /// existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context_assignments: BTreeMap<String, String>,
}

/// Schema for an evidence field declared in an `accepts` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Compiled `decider` block, with every default resolved. Skipped when
    /// unset, so a template that declares none compiles byte for byte as it
    /// did before the block existed and keeps its `template_hash`. See
    /// `src/template/decider.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decider: Option<FieldDecider>,
}

/// A gate declaration in a compiled template state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    #[serde(rename = "type")]
    pub gate_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Timeout in seconds (0 = use default of 30s).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timeout: u32,
    /// Context key for context-exists and context-matches gates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Regex pattern for context-matches gates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// Optional instance-level override default value for this gate.
    ///
    /// When present, this value is used as the override default instead of
    /// (or in addition to) the built-in default for the gate type. The
    /// `koto overrides record` command resolves the override value by
    /// checking `--with-data`, then this field, then `built_in_default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_default: Option<serde_json::Value>,
    /// Completion condition for children-complete gates.
    ///
    /// Controls when a child workflow counts as complete. Currently only
    /// `"terminal"` is implemented (child reached a terminal state). The
    /// prefixes `"state:*"` and `"context:*"` are reserved for future use.
    /// Defaults to `"terminal"` when not specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<String>,
    /// Name prefix filter for children-complete gates.
    ///
    /// When set, only child workflows whose name starts with this prefix
    /// are considered by the gate. Enables multi-fanout scoping (e.g.,
    /// only research children, not all children).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_filter: Option<String>,
    /// Whether `koto overrides record` may force this gate. Defaults to `true`.
    ///
    /// A gate declared `overridable: false` refuses every override at record
    /// time, with or without `--with-data`, and the advance loop ignores any
    /// `GateOverrideRecorded` event for it (one written by an older koto or
    /// appended by hand), evaluating the gate for real instead. Use it for
    /// gates whose output decides progress that nothing downstream re-checks.
    ///
    /// Omitted from the compiled JSON when `true`, so a template that does
    /// not use the field compiles byte-identical to before and existing
    /// sessions' template hashes stay valid.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub overridable: bool,
    /// Request id for `request-leg` gates. May carry `{{VAR}}` references,
    /// which the tick substitutes before the gate reads the request store.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request: String,
    /// Leg name for `request-leg` gates. Substituted like `request`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub leg: String,
    /// Optional expectation set for `request-leg` gates: each key names a
    /// payload key, and its list holds the scalar values that key may carry
    /// for the gate to report `valid: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<BTreeMap<String, Vec<serde_json::Value>>>,
    /// Re-evaluation settings for a `command` gate whose command can answer
    /// "not yet" (DESIGN-koto-ci-wait-stale-keys.md Decisions 4 and 5).
    /// Absent on every other gate, and omitted from the compiled JSON when
    /// absent, so a template without it keeps its template hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<PollSpec>,
    /// The extraction budget, label and criteria of a `decider-check` gate
    /// (DESIGN-koto-decider-checks.md, Decision 1). Absent on every other
    /// gate, and omitted from the compiled JSON when absent, so a template
    /// without it keeps its template hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decider_check: Option<super::decider_check::DeciderCheckSpec>,
    /// Exit codes of a `command` gate that mean "my input wasn't ready to
    /// judge". An evaluation that exits with one is logged as not ready, the
    /// same as one whose command printed a `::koto-not-ready::` line. It is
    /// telemetry only: the outcome, routing and fallback finding stay as
    /// they are. Empty on every other gate, and omitted from the compiled
    /// JSON when empty, so a template without it keeps its template hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_ready_exit_codes: Vec<i32>,
}

/// How koto re-evaluates a polling command gate.
///
/// The command's exit status decides: 0 is done, `pending_exit_code` is
/// still pending, anything else is failed. Within one tick koto re-runs a
/// pending command every `interval_secs` for at most `hold_secs`; across
/// ticks it gives up `timeout_secs` after the first run since the latest
/// entry into the state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollSpec {
    pub interval_secs: u32,
    pub timeout_secs: u32,
    #[serde(default)]
    pub hold_secs: u32,
    #[serde(default = "default_pending_exit_code")]
    pub pending_exit_code: i32,
}

/// `EX_TEMPFAIL`: the conventional "try again later" status.
pub const DEFAULT_PENDING_EXIT_CODE: i32 = 75;

fn default_pending_exit_code() -> i32 {
    DEFAULT_PENDING_EXIT_CODE
}

impl Gate {
    /// Every field of this gate that a tick substitutes `{{KEY}}` references in,
    /// paired with the name errors report it under.
    ///
    /// One list, next to the struct that owns the fields, because two lists is
    /// the bug. The compiler validates references in exactly this set and
    /// `substitute_gate_fields` rewrites exactly this set, and each of the
    /// fixes before this one was an instance of those two sets disagreeing: a
    /// `default_action` command the compiler accepted and the runtime skipped
    /// (Issue #220), a gate `key` and `pattern` neither of them touched
    /// (Issue #222), and `name_filter` on a `children-complete` gate, in
    /// neither until Issue #224 put it here.
    ///
    /// `name_filter` is `Option<String>`, which is why this returns a `Vec`
    /// rather than a fixed-size array: a field that is absent contributes no
    /// entry, and there is no `String` to borrow a name from. A gate that does
    /// not declare the field is unaffected by everything downstream of this
    /// list, which is the distinction the empty-value refusal in
    /// `evaluate_children_complete` depends on.
    ///
    /// Adding a field here is what wires it into compile-time validation;
    /// `every_field_the_compiler_validates_is_one_the_tick_substitutes` in
    /// `src/cli/mod.rs` then fails until the runtime side is wired too, so the
    /// pair cannot drift apart silently again.
    ///
    /// The body destructures `self` exhaustively rather than reaching for the
    /// fields it wants, so a field added to `Gate` stops this function
    /// compiling until someone says whether it carries references. That is the
    /// whole guarantee: an enumeration a maintainer can forget to update is
    /// what the fixes above were each an instance of. Do not simplify the
    /// pattern to `..`.
    pub fn substitutable_fields(&self) -> Vec<(&'static str, &str)> {
        let Self {
            gate_type: _,
            command,
            timeout: _,
            key,
            pattern,
            override_default: _,
            completion: _,
            name_filter,
            overridable: _,
            request,
            leg,
            // `expect` holds literal scalar values compared against a leg's
            // payload; it carries no references.
            expect: _,
            // `poll` holds numbers only; it carries no references.
            poll: _,
            // A decider check's criteria are the fixed text the decider
            // reads as its question, never substituted: only its `command`
            // is.
            decider_check: _,
            // Exit codes are numbers only; they carry no references.
            not_ready_exit_codes: _,
        } = self;
        let mut fields = vec![
            ("command", command.as_str()),
            ("key", key.as_str()),
            ("pattern", pattern.as_str()),
            ("request", request.as_str()),
            ("leg", leg.as_str()),
        ];
        if let Some(filter) = name_filter {
            fields.push(("name_filter", filter.as_str()));
        }
        fields
    }
}

/// A default action declaration for a template state.
///
/// When present on a state, the engine executes this command automatically
/// on state entry (unless override evidence exists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionDecl {
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_confirmation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling: Option<PollingConfig>,
    /// Prose the agent reads when this action fails.
    ///
    /// Spliced onto the failure response's `directive` rather than placed in
    /// `details`, so detail suppression can never withhold it
    /// (DESIGN-koto-runs-commands.md Decision 4). It is spliced after
    /// variable substitution and is therefore never expanded — write it as
    /// literal prose. A state that declares no fallback still stops on
    /// failure; the response simply carries no prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// The name this command's trimmed stdout is delivered under.
    ///
    /// The name is its own declaration; it is deliberately not written in the
    /// template's `variables:` block, because a declared variable is
    /// materialized by `koto init` and would render as the empty string on a
    /// run that never entered this state
    /// (DESIGN-koto-runs-commands.md Decision 2). The compiler validates
    /// `{{KEY}}` references against the union of the variables block, every
    /// state's capture name, and the runtime names, and rejects a capture name
    /// that collides with any of them.
    ///
    /// A state that declares no name behaves exactly as it did before captures
    /// existed: the command's stdout reaches the event log and the response,
    /// and nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_stdout_as: Option<String>,
    /// When true, an output that is empty once trimmed is delivered under
    /// `capture_stdout_as` as the empty string instead of failing the tick,
    /// so a directive that interpolates it renders it as nothing. A
    /// non-empty output is checked as it always is. Requires
    /// `capture_stdout_as`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub capture_allow_empty: bool,
    /// The host pool this action takes a slot of before it runs
    /// (DESIGN-host-resource-pools.md). The operator sizes the pool in the
    /// user's koto config; a pool the config doesn't define imposes no limit.
    ///
    /// This and the two fields below are serialized only when set, so a
    /// template that uses none of them compiles to the same output and
    /// template hash as before they existed, and an older koto, which ignores
    /// unknown keys here, runs the action unthrottled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// Run the command through a detached runner that outlives the tick.
    /// Requires `timeout_secs`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub background: bool,
    /// A background action's time limit, 1 to 86400 seconds. Only with
    /// `background: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
}

/// A pool name: lowercase letters, digits and hyphens, starting with a letter
/// or digit, at most 63 characters (PRD-host-resource-pools.md R1). Checked
/// before a name reaches a path.
pub fn is_valid_pool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The most a background action's `timeout_secs` may be.
pub const MAX_ACTION_TIMEOUT_SECS: u32 = 86_400;

impl ActionDecl {
    /// Every field of this action declaration that a tick substitutes `{{KEY}}`
    /// references in, paired with the name errors report it under.
    ///
    /// The `Gate` counterpart above says why one list beats two, and this is
    /// the same argument one struct over: the compiler validated `command` and
    /// `working_dir` from two hand-written loops, and the field it validated
    /// from neither -- `fallback` -- is Issue #228. A `default_action` command
    /// the compiler accepted and the runtime skipped was Issue #220, on this
    /// same struct.
    ///
    /// `fallback` is not here. It carries references and is refused them; see
    /// [`ActionDecl::literal_fields`], which is the list that says so.
    /// `capture_stdout_as` is in neither list: it declares a name rather than
    /// carrying a reference, and the compiler checks it for collisions on its
    /// own path.
    ///
    /// The body destructures `self` exhaustively for the reason
    /// [`Gate::substitutable_fields`] gives. Do not simplify the pattern
    /// to `..`.
    pub fn substitutable_fields(&self) -> Vec<(&'static str, &str)> {
        let Self {
            command,
            working_dir,
            requires_confirmation: _,
            polling: _,
            fallback: _,
            capture_stdout_as: _,
            capture_allow_empty: _,
            // A pool name is never substituted; a reference in it is
            // refused by `validate_action_pooling`.
            pool: _,
            background: _,
            timeout_secs: _,
        } = self;
        vec![
            ("command", command.as_str()),
            ("working_dir", working_dir.as_str()),
        ]
    }

    /// Every field of this action declaration that reaches its reader as
    /// literal prose, paired with the name errors report it under.
    ///
    /// A field here promises the opposite of one in
    /// [`ActionDecl::substitutable_fields`]: a `{{KEY}}` reference written into
    /// it is refused at compile time rather than resolved at run time. The two
    /// promises are why there are two lists instead of one list with a tag --
    /// each carries its own compiler error and its own test.
    ///
    /// `fallback` is spliced onto a failure response's directive *after*
    /// substitution has run, deliberately, so author prose is never exposed to
    /// expansion. That behaviour is intended and documented on the field. What
    /// was missing until Issue #228 was the other half: nothing told an author
    /// that the reference they wrote would not resolve.
    ///
    /// The body destructures `self` exhaustively for the reason
    /// [`Gate::substitutable_fields`] gives. Do not simplify the pattern
    /// to `..`.
    pub fn literal_fields(&self) -> Vec<(&'static str, &str)> {
        let Self {
            command: _,
            working_dir: _,
            requires_confirmation: _,
            polling: _,
            fallback,
            capture_stdout_as: _,
            capture_allow_empty: _,
            // A pool name is never substituted; a reference in it is
            // refused by `validate_action_pooling`.
            pool: _,
            background: _,
            timeout_secs: _,
        } = self;
        let mut fields = Vec::new();
        if let Some(prose) = fallback {
            fields.push(("fallback", prose.as_str()));
        }
        fields
    }
}

/// Polling configuration for actions that need repeated execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollingConfig {
    pub interval_secs: u32,
    pub timeout_secs: u32,
}

fn is_false(b: &bool) -> bool {
    !b
}

fn is_true(b: &bool) -> bool {
    *b
}

fn default_true() -> bool {
    true
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The context key a failed session writes to say why it failed. A parent's
/// batch view reports it as the child's `reason` (koto#278), and compile
/// warning W5 checks that a failure terminal has a path writing it.
pub const FAILURE_REASON_KEY: &str = "failure_reason";

/// Gate type: shell command.
pub const GATE_TYPE_COMMAND: &str = "command";

/// Gate type: check whether a context key exists.
pub const GATE_TYPE_CONTEXT_EXISTS: &str = "context-exists";

/// Gate type: check whether context content matches a regex pattern.
pub const GATE_TYPE_CONTEXT_MATCHES: &str = "context-matches";

/// Gate type: check whether all child workflows have reached their completion condition.
pub const GATE_TYPE_CHILDREN_COMPLETE: &str = "children-complete";

/// Gate type: read one request leg's disposition and recorded result.
pub const GATE_TYPE_REQUEST_LEG: &str = "request-leg";

/// Gate type that grades an extracted slice against decider criteria
/// (DESIGN-koto-decider-checks.md).
pub const GATE_TYPE_DECIDER_CHECK: &str = "decider-check";

/// Every gate type koto evaluates, in the order error messages list them.
pub const SUPPORTED_GATE_TYPES: &[&str] = &[
    GATE_TYPE_COMMAND,
    GATE_TYPE_CONTEXT_EXISTS,
    GATE_TYPE_CONTEXT_MATCHES,
    GATE_TYPE_CHILDREN_COMPLETE,
    GATE_TYPE_REQUEST_LEG,
    GATE_TYPE_DECIDER_CHECK,
];

/// The object-typed output field of a `request-leg` gate. A `when` clause
/// routes on a key inside it (`gates.<gate>.payload.<key>`), never on the
/// object itself.
pub const REQUEST_LEG_PAYLOAD_FIELD: &str = "payload";

/// Evidence namespace reserved for engine-injected gate output.
/// Agent submissions starting with this prefix are rejected (Feature 2, R7).
/// All `gates.*` key checks in advance.rs and types.rs use this constant.
pub const GATES_EVIDENCE_NAMESPACE: &str = "gates";

/// Condition name reserved for the synthetic result a failed `default_action`
/// produces (DESIGN-koto-runs-commands.md Decision 3).
///
/// A failing action routes through the gate-blocked path under this name, so
/// the compiler rejects any state that declares a gate called `__action__` —
/// otherwise an author's gate and the engine's synthetic result would collide
/// in the same map.
pub const ACTION_CONDITION_NAME: &str = "__action__";

/// Condition name reserved for a pooled action waiting for a slot of its
/// host pool (DESIGN-host-resource-pools.md Decision 3). Like
/// [`ACTION_CONDITION_NAME`], the compiler refuses a gate of this name.
pub const POOL_CONDITION_NAME: &str = "__pool__";

/// Prefix used in when-clause keys to match on the presence of an agent-submitted
/// evidence field. Paired with the sentinel value [`PRESENT_MATCHER_VALUE`] to form
/// expressions like `evidence.retry_failed: present` that fire when the named field
/// appears in any event since the last state transition (Issue #11).
pub const EVIDENCE_NAMESPACE: &str = "evidence";

/// Sentinel string value that, when it appears as the value of a when-clause entry
/// under the `evidence.<field>` key, triggers the presence matcher instead of the
/// default scalar-equality check.
pub const PRESENT_MATCHER_VALUE: &str = "present";

/// Returns true if `value` is the JSON string `"present"` used as the presence-matcher
/// sentinel. The check is case-sensitive so literal values like `"Present"` continue
/// to route via the value-equality path.
pub fn is_present_matcher(value: &serde_json::Value) -> bool {
    value.as_str() == Some(PRESENT_MATCHER_VALUE)
}

/// Prefix of every template variable condition in `when` clauses and
/// `skip_if` maps. Keys like `vars.MY_VAR` reference declared template
/// variables.
pub const VARS_PREFIX: &str = "vars.";

/// Check if a `vars.*` condition value is a value matcher: a JSON string the
/// variable's value must equal exactly. Returns the string, or `None` for
/// any other shape (including `{is_set: bool}`).
pub fn vars_value_matcher(value: &serde_json::Value) -> Option<&str> {
    value.as_str()
}

/// Check if a `when` value is an `{is_set: bool}` matcher.
///
/// Returns `Some(true)` for `{"is_set": true}`, `Some(false)` for
/// `{"is_set": false}`, and `None` for any other shape.
pub fn is_is_set_matcher(value: &serde_json::Value) -> Option<bool> {
    let obj = value.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    obj.get("is_set")?.as_bool()
}

/// Return `true` when every key-value pair in `when` is satisfied by
/// `skip_conditions` acting as synthetic evidence.
///
/// Used by the compile-time E-SKIP-AMBIGUOUS check in `validate()` to determine
/// which conditional transitions the skip_if values would activate. The runtime
/// evaluator (`conditions_satisfied()` in `src/engine/advance.rs`) answers a
/// different question — whether actual runtime state satisfies the skip_if
/// predicate — so it does not call this function. The two agree on the
/// `vars.NAME` matcher shapes: `is_is_set_matcher` for `{is_set: bool}` and a
/// string for a value condition.
///
/// # Matching rules
///
/// - **`vars.NAME: {is_set: bool}`** in `when` — compile-time approximation:
///   the condition is satisfied when `skip_conditions` provides a set/unset
///   signal for the same variable key. No variable store is available at
///   compile time; we look at whether `skip_conditions` contains an
///   `is_set`-shaped value (or any non-null value) for the key.
/// - **direct value equality** — every other key, including a `vars.NAME`
///   value condition (a string): the `skip_conditions` map must contain the
///   key with an equal JSON value, so `skip_if: {vars.MODE: auto}` selects the
///   `when: {vars.MODE: auto}` route.
#[doc(hidden)]
pub fn skip_if_matches_when(
    skip_conditions: &BTreeMap<String, serde_json::Value>,
    when: &BTreeMap<String, serde_json::Value>,
) -> bool {
    let vars_prefix = VARS_PREFIX;
    when.iter().all(|(field, expected)| {
        // vars.NAME: {is_set: bool} path.
        if field.starts_with(vars_prefix) {
            if let Some(expected_set) = is_is_set_matcher(expected) {
                let is_set = skip_conditions
                    .get(field.as_str())
                    .or_else(|| {
                        let var_name = &field[vars_prefix.len()..];
                        skip_conditions.get(var_name)
                    })
                    .map(|v| {
                        if let Some(b) = is_is_set_matcher(v) {
                            b
                        } else {
                            !v.is_null()
                        }
                    })
                    .unwrap_or(false);
                return is_set == expected_set;
            }
        }
        // Direct value equality path.
        skip_conditions.get(field.as_str()) == Some(expected)
    })
}

/// The JSON value type of a gate output field.
///
/// Used by [`gate_type_schema`] to describe the expected type of each field
/// in a gate type's output schema. The compiler uses this for exact-match
/// validation of `override_default` values (D2) and the reachability check (D4).
#[derive(Debug, Clone, PartialEq)]
pub enum GateSchemaFieldType {
    Number,
    /// String-typed field. Named `Str` to avoid shadowing `std::string::String`
    /// when this enum is glob-imported with `use GateSchemaFieldType::*`.
    Str,
    Boolean,
    /// JSON-array field. Only `children-complete` produces one (`children`),
    /// and it exists in the schema so an `override_default` can carry the same
    /// shape the gate actually emits. Routing on an array in a `when` clause
    /// compares whole values, which is rarely what an author wants -- route on
    /// the aggregate booleans instead.
    Array,
    /// JSON-object field. Only `request-leg` produces one (`payload`). A
    /// `when` clause cannot compare it whole; it routes on a key inside it
    /// through `gates.<gate>.payload.<key>`.
    Object,
}

/// Return the static output field schema for a known gate type.
///
/// Each element is `(field_name, field_type)`. Returns `None` for unknown
/// gate type strings.
///
/// Gate schemas:
/// - `command`:        `[("exit_code", Number), ("error", Str)]`
/// - `context-exists`: `[("exists", Boolean), ("error", Str)]`
/// - `context-matches`:`[("matches", Boolean), ("error", Str)]`
pub fn gate_type_schema(gate_type: &str) -> Option<&'static [(&'static str, GateSchemaFieldType)]> {
    use GateSchemaFieldType::*;
    match gate_type {
        GATE_TYPE_COMMAND => Some(&[("exit_code", Number), ("error", Str)]),
        GATE_TYPE_CONTEXT_EXISTS => Some(&[("exists", Boolean), ("error", Str)]),
        GATE_TYPE_CONTEXT_MATCHES => Some(&[("matches", Boolean), ("error", Str)]),
        // Must stay in step with the output `build_children_complete_output()`
        // assembles in `src/cli/batch.rs` and with the built-in default below.
        // When these drift, the compiler rejects `when` clauses on fields the
        // gate really does emit -- which is how routing on `all_success` and
        // `needs_attention` came to be rejected while warning W4 was actively
        // recommending them (issue #207).
        GATE_TYPE_CHILDREN_COMPLETE => Some(&[
            ("total", Number),
            ("completed", Number),
            ("pending", Number),
            ("success", Number),
            ("failed", Number),
            ("skipped", Number),
            ("blocked", Number),
            ("spawn_failed", Number),
            ("all_complete", Boolean),
            ("all_success", Boolean),
            ("any_failed", Boolean),
            ("any_skipped", Boolean),
            ("any_spawn_failed", Boolean),
            ("needs_attention", Boolean),
            ("children", Array),
            ("error", Str),
        ]),
        // Must stay in step with `request_leg_output()` in `src/gate.rs`; a
        // test there asserts every key the evaluator emits is listed here and
        // the other way round.
        GATE_TYPE_REQUEST_LEG => Some(&[
            ("found", Boolean),
            ("disposition", Str),
            ("bound", Boolean),
            ("source", Str),
            ("status", Str),
            ("final_state", Str),
            ("template", Str),
            ("outcome", Str),
            ("step", Str),
            ("reason", Str),
            ("valid", Boolean),
            ("payload", Object),
            ("error", Str),
        ]),
        // The keys every decider-check output has carried since the gate
        // type shipped; the evaluator in `src/cli/check_evaluator.rs` builds
        // them. Its later lists, `awaiting` and `agent_answered`, are left
        // out on purpose: the schema is enforced on `override_default`, so
        // adding them would break every template that declares one, and an
        // override value without `awaiting` correctly doesn't block. No
        // `when` clause may read it; the schema exists for override values
        // and the override record.
        GATE_TYPE_DECIDER_CHECK => {
            Some(&[("failed", Array), ("unanswered", Array), ("error", Str)])
        }
        _ => None,
    }
}

/// Return the built-in default override value for a known gate type.
///
/// Mirrors `built_in_default()` in `src/gate.rs` — the two functions serve
/// the same purpose in different contexts (compile-time vs. runtime). A unit
/// test in this module asserts they return identical values for every known
/// gate type. Update both functions in tandem if a gate type's default changes.
///
/// Returns `None` for unknown gate types.
pub fn gate_type_builtin_default(gate_type: &str) -> Option<serde_json::Value> {
    match gate_type {
        GATE_TYPE_COMMAND => Some(serde_json::json!({"exit_code": 0, "error": ""})),
        GATE_TYPE_CONTEXT_EXISTS => Some(serde_json::json!({"exists": true, "error": ""})),
        GATE_TYPE_CONTEXT_MATCHES => Some(serde_json::json!({"matches": true, "error": ""})),
        GATE_TYPE_DECIDER_CHECK => Some(decider_check_default_output()),
        GATE_TYPE_CHILDREN_COMPLETE => Some(serde_json::json!({
            "total": 0,
            "completed": 0,
            "pending": 0,
            "success": 0,
            "failed": 0,
            "skipped": 0,
            "blocked": 0,
            "spawn_failed": 0,
            "all_complete": true,
            "all_success": true,
            "any_failed": false,
            "any_skipped": false,
            "any_spawn_failed": false,
            "needs_attention": false,
            "children": [],
            "error": ""
        })),
        GATE_TYPE_REQUEST_LEG => Some(request_leg_builtin_default()),
        _ => None,
    }
}

/// The built-in default for a `decider-check` gate: no failing and no
/// unanswered criteria. Shared by both default functions and the gate
/// evaluator's pass when no check evaluator is supplied. It matches the
/// schema's three keys; a reader treats the absent `awaiting` and
/// `agent_answered` lists as empty.
pub fn decider_check_default_output() -> serde_json::Value {
    serde_json::json!({"failed": [], "unanswered": [], "error": ""})
}

/// The built-in default for a `request-leg` gate: a resolved, valid record
/// that names no child outcome. Shared by both default functions so the two
/// cannot disagree about this type.
pub fn request_leg_builtin_default() -> serde_json::Value {
    serde_json::json!({
        "found": true,
        "disposition": "resolved",
        "bound": true,
        "source": "promoted",
        "status": "",
        "final_state": "",
        "template": "",
        "outcome": "",
        "step": "",
        "reason": "",
        "valid": true,
        "payload": {},
        "error": ""
    })
}

/// Field type marker for structured task-list evidence fields consumed by
/// the `materialize_children` hook.
pub const FIELD_TYPE_TASKS: &str = "tasks";

/// Runtime-injected variable names that are valid in templates but not
/// declared in the variables block. These are provided by the engine at
/// runtime (e.g., SESSION_DIR is the session directory path).
#[doc(hidden)]
pub const RUNTIME_VARIABLE_NAMES: &[&str] = &["SESSION_DIR", "SESSION_NAME"];

/// True when `name` is spelled the way a `{{KEY}}` reference can find it:
/// an uppercase letter followed by uppercase letters, digits, or underscores.
///
/// This is the name half of [`VAR_REF_PATTERN`]. A capture spelled any other
/// way is unreferenceable, so the compiler rejects it at the declaration
/// rather than leaving an author to discover the reference never resolves.
fn is_capture_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// True when a `when` value names `answer`: a string equal to it, or a
/// boolean or number whose text equals it. Used by the decider floor and the
/// escape-routing check, which both err toward matching.
#[doc(hidden)]
pub fn when_value_matches_answer(value: &serde_json::Value, answer: &str) -> bool {
    match value {
        serde_json::Value::String(s) => s == answer,
        serde_json::Value::Bool(b) => b.to_string() == answer,
        serde_json::Value::Number(n) => n.to_string() == answer,
        _ => false,
    }
}

impl CompiledTemplate {
    /// Every `capture_stdout_as` name in the template, mapped to the state
    /// that delivers it.
    ///
    /// The map is the capture namespace: names live here rather than in the
    /// `variables:` block, so `koto init` never materializes one and a run
    /// that skipped the producing state has no value to render
    /// (DESIGN-koto-runs-commands.md Decision 2). Returning the producing
    /// state is what lets the run-time stop for an undelivered name say which
    /// state would have delivered it.
    ///
    /// Fails on the three collisions that would make a name ambiguous: with a
    /// declared variable, with a reserved runtime name, or with another
    /// state's capture name. Answering all three here means precedence between
    /// two producers never arises at run time.
    pub fn capture_names(&self) -> Result<BTreeMap<String, String>, String> {
        let mut captures: BTreeMap<String, String> = BTreeMap::new();
        for (state_name, state) in &self.states {
            let Some(action) = &state.default_action else {
                continue;
            };
            let Some(key) = &action.capture_stdout_as else {
                continue;
            };
            if key.is_empty() {
                return Err(format!(
                    "state {:?}: default_action capture_stdout_as must not be empty\n  \
                     remedy: name the capture or drop the field",
                    state_name
                ));
            }
            if !is_capture_name(key) {
                return Err(format!(
                    "state {:?}: default_action capture_stdout_as {:?} is not a valid name; \
                     a name starts with an uppercase letter and continues with uppercase \
                     letters, digits, or underscores, so that {{{{{}}}}} can reference it\n  \
                     remedy: rename the capture",
                    state_name, key, key
                ));
            }
            if self.variables.contains_key(key) {
                return Err(format!(
                    "state {:?}: default_action capture_stdout_as {:?} collides with a \
                     declared variable; a capture name is its own declaration and must not \
                     appear in the variables block\n  \
                     remedy: rename the capture, or remove {:?} from the variables block",
                    state_name, key, key
                ));
            }
            if RUNTIME_VARIABLE_NAMES.contains(&key.as_str()) {
                return Err(format!(
                    "state {:?}: default_action capture_stdout_as {:?} is a reserved runtime \
                     variable name\n  \
                     remedy: rename the capture",
                    state_name, key
                ));
            }
            if let Some(other) = captures.get(key) {
                return Err(format!(
                    "state {:?}: default_action capture_stdout_as {:?} is already delivered by \
                     state {:?}; two states must not declare the same capture name\n  \
                     remedy: rename one of the captures",
                    state_name, key, other
                ));
            }
            captures.insert(key.clone(), state_name.clone());
        }
        Ok(captures)
    }
}

impl CompiledTemplate {
    /// Every transition out of `state` that an `auto` answer `answer` for
    /// `field` is forbidden to take, with the reason.
    ///
    /// This is the floor from docs/designs/current/DESIGN-jev-decision-offload.md: an
    /// answer in `auto` may never route to a terminal state, to a state whose
    /// `default_action` requires confirmation, or along a `when` clause that
    /// also tests a `gates.*` key. It considers every transition whose `when`
    /// tests `field` directly, and matches the answer generously: a `when`
    /// value that is a string, a boolean, or a number counts when its text
    /// equals `answer`, so `"true"` and `true` both match a boolean answer
    /// `"true"`. Over-matching is the safe direction -- a template can't dodge
    /// the floor by spelling a value differently.
    ///
    /// Pure: no I/O, and nothing about the declaration's modes is consulted, so
    /// a caller can ask about any answer. An unknown state yields no
    /// violations. Routes that reach the field only through `evidence.*` or
    /// `vars.*` keys aren't visible here; the runtime checks the transition it
    /// actually matched with [`CompiledTemplate::transition_floor_violations`].
    pub fn floor_violations(&self, state: &str, field: &str, answer: &str) -> Vec<FloorViolation> {
        let Some(st) = self.states.get(state) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (index, transition) in st.transitions.iter().enumerate() {
            let tests_answer = transition
                .when
                .as_ref()
                .and_then(|w| w.get(field))
                .is_some_and(|v| when_value_matches_answer(v, answer));
            if !tests_answer {
                continue;
            }
            for kind in self.transition_floor_violations(transition) {
                out.push(FloorViolation {
                    transition_index: index,
                    target: transition.target.clone(),
                    kind,
                });
            }
        }
        out
    }

    /// The floor rules `transition` breaks, whatever its `when` tests.
    ///
    /// This is the per-transition half of [`CompiledTemplate::floor_violations`],
    /// public so the engine can recheck the one transition an answer actually
    /// matched, including routes the compiler couldn't attribute to a field.
    /// Pure: no I/O. A target that isn't a declared state breaks no rule here
    /// (validation has already refused it).
    pub fn transition_floor_violations(&self, transition: &Transition) -> Vec<FloorViolationKind> {
        let mut out = Vec::new();
        if let Some(target) = self.states.get(&transition.target) {
            if target.terminal {
                out.push(FloorViolationKind::TerminalTarget);
            }
            if target
                .default_action
                .as_ref()
                .is_some_and(|a| a.requires_confirmation)
            {
                out.push(FloorViolationKind::ConfirmationRequired);
            }
        }
        if let Some(when) = &transition.when {
            let gates_prefix = format!("{}.", GATES_EVIDENCE_NAMESPACE);
            for key in when.keys() {
                if key.starts_with(&gates_prefix) {
                    out.push(FloorViolationKind::GateConditioned {
                        gate_key: key.clone(),
                    });
                }
            }
        }
        out
    }
}

impl CompiledTemplate {
    /// Collect non-fatal warnings W1-W5 tied to `materialize_children`,
    /// `failure`, and `skipped_marker` usage.
    ///
    /// | Rule | Trigger |
    /// |------|---------|
    /// | W1 | State with `materialize_children` routes to a state reachable from the declaring state that does not observe the `children-complete` gate |
    /// | W2 | `children-complete.name_filter` is set but does not end with `.` (ergo not scoped to one parent) |
    /// | W3 | Terminal state whose name matches /block|fail|error/ lacks `failure: true` |
    /// | W4 | State with `materialize_children` routes only on `all_complete: true` without a second transition handling failures |
    /// | W5 | Terminal state with `failure: true` has no path writing `failure_reason` to context: no `context_assignments` entry on every incoming transition (a `default_action` or `koto context add` write is not detected, so templates relying on one may see false positives) |
    ///
    /// Warnings are returned as formatted strings; callers emit them via
    /// stderr (`validate`) or collect them for tests.
    pub fn collect_materialize_children_warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();

        // W1: children-complete gate reachable from the declaring state.
        //
        // In the single-state fan-out shape (E10 enforces the gate lives on
        // the declaring state itself), the gate is trivially reachable from
        // the declaring state. W1 still catches the degenerate case where
        // the hook is declared but no gate on this state evaluates
        // `children-complete` via a `when` clause — meaning the gate output
        // is never observed by any transition.
        for (state_name, state) in &self.states {
            if state.materialize_children.is_none() {
                continue;
            }
            let gate_names: Vec<&String> = state
                .gates
                .iter()
                .filter_map(|(n, g)| (g.gate_type == GATE_TYPE_CHILDREN_COMPLETE).then_some(n))
                .collect();
            if gate_names.is_empty() {
                // E10 already fired; no W1 noise.
                continue;
            }
            let any_referenced = gate_names.iter().any(|gn| {
                let prefix = format!("{}.{}.", GATES_EVIDENCE_NAMESPACE, gn);
                state.transitions.iter().any(|t| {
                    t.when
                        .as_ref()
                        .is_some_and(|w| w.keys().any(|k| k.starts_with(&prefix)))
                })
            });
            if !any_referenced {
                warnings.push(format!(
                    "W1: state {:?}: children-complete gate is declared but no transition's when clause observes it\n  \
                     remedy: add a transition with a when clause referencing gates.<gate_name>.all_complete (or any children-complete output field)",
                    state_name
                ));
            }
        }

        // W2: children-complete.name_filter should end with `.` to scope
        // matching to children of a single parent (children are named
        // `<parent>.<task>`). We cannot know the parent name at compile
        // time, but a filter that lacks the trailing `.` would match by
        // prefix across parents and is almost certainly a bug.
        for (state_name, state) in &self.states {
            for (gate_name, gate) in &state.gates {
                if gate.gate_type != GATE_TYPE_CHILDREN_COMPLETE {
                    continue;
                }
                if let Some(filter) = &gate.name_filter {
                    if !filter.is_empty() && !filter.ends_with('.') {
                        warnings.push(format!(
                            "W2: state {:?} gate {:?}: name_filter {:?} does not end with \".\"; \
                             children-complete gates scope to one parent by matching the \"<parent>.\" prefix\n  \
                             remedy: append \".\" to the filter (e.g. {:?})",
                            state_name,
                            gate_name,
                            filter,
                            format!("{}.", filter)
                        ));
                    }
                }
            }
        }

        // W3: terminal state whose name contains "block"/"fail"/"error"
        // lacks `failure: true`.
        for (state_name, state) in &self.states {
            if !state.terminal {
                continue;
            }
            if state.failure {
                continue;
            }
            let lower = state_name.to_lowercase();
            let looks_failureish =
                lower.contains("block") || lower.contains("fail") || lower.contains("error");
            if looks_failureish {
                warnings.push(format!(
                    "W3: state {:?}: terminal state name suggests a failure outcome but `failure: true` is not set\n  \
                     remedy: set `failure: true` if this state represents a failure, or rename it if it represents a success",
                    state_name
                ));
            }
        }

        // W4: materialize_children state routes only on `all_complete: true`
        // without a second transition handling `any_failed > 0` or
        // `any_skipped > 0`. Failed or skipped children would silently
        // take the success branch.
        //
        // We look for transitions whose `when` clause references any of
        // the derived booleans `any_failed`, `any_skipped`,
        // `any_spawn_failed`, or `needs_attention` against any gate.
        for (state_name, state) in &self.states {
            if state.materialize_children.is_none() {
                continue;
            }
            let mut routes_on_all_complete = false;
            let mut has_failure_branch = false;
            for transition in &state.transitions {
                let when = match &transition.when {
                    Some(w) => w,
                    None => continue,
                };
                for key in when.keys() {
                    if !key.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)) {
                        continue;
                    }
                    // Key shape: gates.<gate>.<field>
                    let segments: Vec<&str> = key.splitn(3, '.').collect();
                    if segments.len() != 3 {
                        continue;
                    }
                    let field = segments[2];
                    match field {
                        "all_complete" | "all_success" => routes_on_all_complete = true,
                        "any_failed" | "any_skipped" | "any_spawn_failed" | "needs_attention" => {
                            has_failure_branch = true
                        }
                        _ => {}
                    }
                }
            }
            if routes_on_all_complete && !has_failure_branch {
                warnings.push(format!(
                    "W4: state {:?}: materialize_children routes only on all_complete/all_success \
                     with no branch handling failed or skipped children\n  \
                     remedy: add a transition guarded by gates.<gate>.any_failed or gates.<gate>.needs_attention to route failures somewhere meaningful",
                    state_name
                ));
            }
        }

        // W5: terminal state with `failure: true` has no path writing
        // `failure_reason` to context. The key can land two ways:
        //   (a) the transitions into the state carry a `context_assignments`
        //       entry writing `failure_reason`
        //   (b) a `default_action`, or the agent, runs `koto context add`
        //
        // Only (a) is checked, and it credits the state only when every
        // incoming edge assigns the key: a single edge that doesn't is a path
        // reaching the terminal with no reason. A terminal no edge reaches is
        // not credited.
        //
        // Declaring `failure_reason` in the terminal state's own `accepts`
        // is not credited (koto#278): evidence writes no context, and
        // `koto next --with-data` refuses evidence for a session already in
        // a terminal state, so the declaration never stores the key.
        //
        // TODO(issue-8/W5): (b) is still unchecked. A default_action is a
        // shell command with no declared set of keys it writes, so W5 can
        // over-warn on templates that rely on it.
        for (state_name, state) in &self.states {
            if !(state.terminal && state.failure) {
                continue;
            }
            let incoming: Vec<&Transition> = self
                .states
                .values()
                .flat_map(|s| s.transitions.iter())
                .filter(|t| &t.target == state_name)
                .collect();
            let every_edge_assigns = !incoming.is_empty()
                && incoming
                    .iter()
                    .all(|t| t.context_assignments.contains_key(FAILURE_REASON_KEY));
            if !every_edge_assigns {
                warnings.push(format!(
                    "W5: state {:?}: `failure: true` terminal state has no declared path writing the `failure_reason` context key; \
                     the batch view's per-child `reason` will fall back to the state name\n  \
                     remedy: assign it in context_assignments on the edge out of the state that takes the evidence \
                     (for example `failure_reason: \"${{evidence.failure_reason}}\"`); an assignment on an auto-advancing state's edge \
                     resolves to an empty value, so a path through one should assign earlier (W5 then still warns)",
                    state_name
                ));
            }
        }

        warnings
    }

    /// Collect non-fatal warnings tied to when-clause matcher usage.
    ///
    /// | Rule | Trigger |
    /// |------|---------|
    /// | W6 | The string `"present"` appears as a when-clause value under a key that is not in the `evidence.<field>` namespace. The present matcher is only meaningful when checking whether an agent-submitted evidence field exists. A flat agent-evidence key, a `gates.*` path, or any other prefix with value `"present"` almost certainly means the template author intended presence matching but used the wrong path. |
    ///
    /// Warnings are returned as formatted strings; callers emit them via
    /// stderr (`validate`) or collect them for tests.
    pub fn collect_when_clause_warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();
        let evidence_prefix = format!("{}.", EVIDENCE_NAMESPACE);

        for (state_name, state) in &self.states {
            for transition in &state.transitions {
                let when = match &transition.when {
                    Some(w) => w,
                    None => continue,
                };
                for (field, value) in when {
                    if !is_present_matcher(value) {
                        continue;
                    }
                    if field.starts_with(&evidence_prefix) {
                        continue;
                    }
                    warnings.push(format!(
                        "W6: state {:?} transition to {:?}: when value {:?} is the presence-matcher \
                         sentinel but key {:?} is not in the evidence.<field> namespace\n  \
                         remedy: rewrite the key as evidence.<field> to check for field presence, or replace the value with the intended scalar for equality matching",
                        state_name,
                        transition.target,
                        PRESENT_MATCHER_VALUE,
                        field,
                    ));
                }
            }
        }

        warnings
    }
}
