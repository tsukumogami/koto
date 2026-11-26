//! The trait headers template validation sits under.
//!
//! The validators are methods of the template data types and their bodies
//! live beside those types, but they are declared through these traits rather
//! than inherent impls, so the data types carry no validation of their own
//! and can be read without the checks that compile them. A caller keeps
//! method syntax and adds one trait import.
//!
//! [`CompiledTemplate`](super::types::CompiledTemplate)'s validators sit
//! under four traits, one per run of methods in its source, because a type
//! can implement a trait only once and the runs are kept where they are.

use std::collections::BTreeMap;

use super::types::TemplateState;

/// Whole-template validation of a
/// [`CompiledTemplate`](super::types::CompiledTemplate).
pub trait ValidateTemplate {
    /// Check the template against every schema rule, refusing it with the
    /// first error found. `strict` makes a state with gates but no
    /// gate-referencing when-clause an error rather than a warning.
    fn validate(&self, strict: bool) -> Result<(), String>;
}

/// The `pass_env:` warning of a
/// [`CompiledTemplate`](super::types::CompiledTemplate).
pub trait PassEnvWarnings {
    /// Warnings about `pass_env:` names koto supplies itself, reported
    /// without refusing the template.
    fn collect_pass_env_warnings(&self) -> Vec<String>;
}

/// Per-state validators [`ValidateTemplate::validate`] runs: directive
/// rules, `clear_on_entry`, deciders and `materialize_children`. Private to
/// the template module: nothing else calls them.
pub(super) trait ValidateSteps {
    fn validate_directive_rules(
        &self,
        state_name: &str,
        state: &TemplateState,
        captures: &BTreeMap<String, String>,
    ) -> Result<(), String>;
    fn validate_clear_on_entry(
        &self,
        state_name: &str,
        state: &TemplateState,
    ) -> Result<(), String>;
    fn validate_deciders(
        &self,
        state_name: &str,
        state: &TemplateState,
        captures: &BTreeMap<String, String>,
    ) -> Result<(), String>;
    fn validate_materialize_children_errors(&self) -> Result<(), String>;
}

/// Per-state validators [`ValidateTemplate::validate`] runs on gates,
/// when-clauses and evidence routing. Private to the template module:
/// nothing else calls them.
pub(super) trait ValidateRouting {
    fn validate_gate_reachability(
        &self,
        state_name: &str,
        state: &TemplateState,
        strict: bool,
    ) -> Result<(), String>;
    fn validate_vars_condition(
        &self,
        state_name: &str,
        target: Option<&str>,
        clause: &str,
        field: &str,
        value: &serde_json::Value,
        captures: &BTreeMap<String, String>,
    ) -> Result<(), String>;
    fn validate_evidence_routing(
        &self,
        state_name: &str,
        state: &TemplateState,
        captures: &BTreeMap<String, String>,
    ) -> Result<(), String>;
}

/// Validation of a template's
/// [`ReviewVocabulary`](super::types::ReviewVocabulary).
pub trait ValidateVocabulary {
    /// Refuse an over-long list, a malformed name, or a name declared twice.
    fn validate(&self) -> Result<(), String>;
}

/// The checks that read a [`VariableDecl`](super::types::VariableDecl)'s
/// constraint.
pub trait VariableDeclExt {
    /// The declared constraint as a caller sees it in an error body, or
    /// `None` when nothing is declared.
    fn constraint_label(&self) -> Option<String>;
    /// Whether `value` satisfies the declared `values:` or `pattern:`.
    fn satisfies_constraint(&self, value: &str) -> bool;
    /// Check that the declaration named `name` is coherent.
    fn validate_declaration(&self, name: &str) -> Result<(), String>;
}
