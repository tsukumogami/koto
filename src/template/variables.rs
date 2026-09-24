//! Constraints a template can put on a declared variable.
//!
//! A variable may declare `values:` (a closed set) or `pattern:` (a regular
//! expression the whole value must match), and `rebind: true` to let a later
//! invocation re-apply it on a live session. This module holds the checks
//! that read those declarations: the compile-time check that a declaration is
//! coherent, and the value check `koto init` and the rebind primitive run.
//!
//! Every value, constrained or not, still has to pass the allowlist in
//! `crate::engine::substitute`; a constraint narrows the allowlist and never
//! widens it.

use regex::Regex;

use super::types::VariableDecl;
use crate::engine::substitute::validate_value;

/// Compile a declared `pattern:` so it matches only a whole value.
///
/// The author's expression is compiled on its own first, so an expression
/// that is only valid once wrapped (an unbalanced `)` that the wrapper's own
/// group would close) is refused rather than silently reinterpreted.
pub fn compile_whole_value_pattern(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(pattern)?;
    Regex::new(&format!("^(?:{})$", pattern))
}

impl VariableDecl {
    /// The declared constraint as a caller sees it in an error body:
    /// `values:[a,b]`, `pattern:<re>`, or `None` when nothing is declared.
    pub fn constraint_label(&self) -> Option<String> {
        if !self.values.is_empty() {
            Some(format!("values:[{}]", self.values.join(",")))
        } else if !self.pattern.is_empty() {
            Some(format!("pattern:{}", self.pattern))
        } else {
            None
        }
    }

    /// Whether `value` satisfies the declared `values:` or `pattern:`.
    ///
    /// A variable with no constraint accepts every value (the allowlist is
    /// checked separately). A pattern that fails to compile accepts nothing;
    /// compilation refuses such a template, so this only matters for a
    /// hand-edited compiled artifact.
    pub fn satisfies_constraint(&self, value: &str) -> bool {
        if !self.values.is_empty() {
            return self.values.iter().any(|v| v == value);
        }
        if !self.pattern.is_empty() {
            return compile_whole_value_pattern(&self.pattern)
                .map(|re| re.is_match(value))
                .unwrap_or(false);
        }
        true
    }

    /// Check that the declaration named `name` is coherent. Called by the
    /// compiler; the error names the variable and what is wrong with it.
    pub fn validate_declaration(&self, name: &str) -> Result<(), String> {
        if !self.values.is_empty() && !self.pattern.is_empty() {
            return Err(format!(
                "variable {:?}: declares both values: and pattern:; declare at most one",
                name
            ));
        }
        // An explicitly empty list deserializes to the same `Vec` as an
        // omitted one; the source layer reports that case before building
        // the declaration, so here a non-empty list is the only list.
        for entry in &self.values {
            if let Err(e) = validate_value(name, entry) {
                return Err(format!(
                    "variable {:?}: values: entry {:?} {}",
                    name, entry, e.message
                ));
            }
        }
        if !self.pattern.is_empty() {
            if let Err(e) = compile_whole_value_pattern(&self.pattern) {
                return Err(format!(
                    "variable {:?}: pattern {:?} is not a valid regular expression \
                     (regex crate syntax; lookaround is not supported): {}",
                    name, self.pattern, e
                ));
            }
        }
        let Some(constraint) = self.constraint_label() else {
            return Ok(());
        };
        if !self.default.is_empty() {
            if !self.satisfies_constraint(&self.default) {
                return Err(format!(
                    "variable {:?}: default {:?} does not satisfy {}",
                    name, self.default, constraint
                ));
            }
        } else if !self.required && !self.satisfies_constraint("") {
            // `koto init` materializes an optional variable with no default as
            // an empty binding, so the constraint has to admit "".
            return Err(format!(
                "variable {:?}: optional with no default, so it resolves to \"\" when not \
                 passed, but \"\" does not satisfy {}; declare a default that satisfies \
                 the constraint, mark it required, or let the constraint accept the empty value",
                name, constraint
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(values: &[&str], pattern: &str) -> VariableDecl {
        VariableDecl {
            values: values.iter().map(|v| v.to_string()).collect(),
            pattern: pattern.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn pattern_matches_the_whole_value_without_author_anchors() {
        let d = decl(&[], "[a-z]+");
        assert!(d.satisfies_constraint("abc"));
        assert!(!d.satisfies_constraint("abc-1"));
    }

    #[test]
    fn author_anchors_are_harmless_inside_the_wrapper() {
        let d = decl(&[], "^(continue|stop)?$");
        assert!(d.satisfies_constraint(""));
        assert!(d.satisfies_constraint("continue"));
        assert!(!d.satisfies_constraint("maybe"));
    }

    #[test]
    fn a_pattern_only_valid_once_wrapped_is_refused() {
        assert!(compile_whole_value_pattern("a)|(b").is_err());
    }

    #[test]
    fn values_is_an_exact_match() {
        let d = decl(&["yes", "no"], "");
        assert!(d.satisfies_constraint("yes"));
        assert!(!d.satisfies_constraint("ye"));
        assert!(!d.satisfies_constraint(""));
    }

    #[test]
    fn constraint_labels() {
        assert_eq!(
            decl(&["yes", "no"], "").constraint_label().as_deref(),
            Some("values:[yes,no]")
        );
        assert_eq!(
            decl(&[], "[0-9]+").constraint_label().as_deref(),
            Some("pattern:[0-9]+")
        );
        assert_eq!(decl(&[], "").constraint_label(), None);
    }
}
