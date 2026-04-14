use std::collections::HashMap;

use regex::Regex;

use crate::engine::types::{Event, EventPayload};
use crate::template::types::VAR_REF_PATTERN;

/// Allowlist regex for variable values: alphanumeric, dots, underscores, hyphens, forward slashes.
const VALUE_PATTERN: &str = r"^[a-zA-Z0-9._/\-]+$";

/// Holds resolved variable bindings for substitution.
#[derive(Debug)]
pub struct Variables {
    vars: HashMap<String, String>,
}

/// Error returned when a variable value fails validation.
#[derive(Debug)]
pub struct SubstitutionError {
    pub key: String,
    pub value: String,
    pub message: String,
}

impl std::fmt::Display for SubstitutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "variable {:?} value {:?}: {}",
            self.key, self.value, self.message
        )
    }
}

impl std::error::Error for SubstitutionError {}

impl Variables {
    /// Extract variables from the WorkflowInitialized event in the log.
    /// Re-validates all values against the allowlist as defense in depth.
    pub fn from_events(events: &[Event]) -> Result<Self, SubstitutionError> {
        let vars = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::WorkflowInitialized { variables, .. } => Some(variables.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // Re-validate every value against the allowlist.
        for (key, value) in &vars {
            validate_value(key, value)?;
        }

        Ok(Variables { vars })
    }

    /// Replace `{{KEY}}` patterns with variable values.
    ///
    /// Panics on undefined references. Compile-time validation prevents this
    /// from happening in practice, so a panic here indicates a bug in the
    /// validation layer rather than user error.
    pub fn substitute(&self, input: &str) -> String {
        let re = Regex::new(VAR_REF_PATTERN).expect("VAR_REF_PATTERN is a valid regex");
        let mut result = String::with_capacity(input.len());
        let mut last_end = 0;

        for caps in re.captures_iter(input) {
            let whole_match = caps.get(0).unwrap();
            let key = &caps[1];

            result.push_str(&input[last_end..whole_match.start()]);

            let value = self
                .vars
                .get(key)
                .unwrap_or_else(|| panic!("undefined variable reference: {{{{{}}}}}", key));
            result.push_str(value);

            last_end = whole_match.end();
        }

        result.push_str(&input[last_end..]);
        result
    }

    /// Check if this Variables instance is empty (no variables defined).
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

/// Validate a variable value against the allowlist regex.
/// Exported for reuse by `koto init` validation (Issue 2).
pub fn validate_value(key: &str, value: &str) -> Result<(), SubstitutionError> {
    let re = Regex::new(VALUE_PATTERN).expect("VALUE_PATTERN is a valid regex");
    if !re.is_match(value) {
        return Err(SubstitutionError {
            key: key.to_string(),
            value: value.to_string(),
            message: format!(
                "contains characters not allowed by the value pattern {}",
                VALUE_PATTERN
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::types::extract_refs;

    // -----------------------------------------------------------------------
    // validate_value
    // -----------------------------------------------------------------------

    #[test]
    fn validate_value_accepts_alphanumeric() {
        validate_value("KEY", "hello123").unwrap();
    }

    #[test]
    fn validate_value_accepts_dots_underscores_hyphens_slashes() {
        validate_value("KEY", "my-org/repo_name.v2").unwrap();
    }

    #[test]
    fn validate_value_rejects_spaces() {
        let err = validate_value("KEY", "hello world").unwrap_err();
        assert_eq!(err.key, "KEY");
        assert_eq!(err.value, "hello world");
    }

    #[test]
    fn validate_value_rejects_empty() {
        let err = validate_value("KEY", "").unwrap_err();
        assert_eq!(err.key, "KEY");
    }

    #[test]
    fn validate_value_rejects_special_chars() {
        validate_value("KEY", "value;rm -rf").unwrap_err();
        validate_value("KEY", "$(evil)").unwrap_err();
        validate_value("KEY", "a\nb").unwrap_err();
    }

    // -----------------------------------------------------------------------
    // extract_refs
    // -----------------------------------------------------------------------

    #[test]
    fn extract_refs_finds_single_ref() {
        assert_eq!(extract_refs("Hello {{NAME}}"), vec!["NAME"]);
    }

    #[test]
    fn extract_refs_finds_multiple_refs() {
        let refs = extract_refs("{{A}} and {{B2}} then {{C_D}}");
        assert_eq!(refs, vec!["A", "B2", "C_D"]);
    }

    #[test]
    fn extract_refs_ignores_lowercase() {
        assert!(extract_refs("{{name}}").is_empty());
    }

    #[test]
    fn extract_refs_ignores_unclosed() {
        assert!(extract_refs("{{NAME").is_empty());
        assert!(extract_refs("NAME}}").is_empty());
    }

    #[test]
    fn extract_refs_empty_input() {
        assert!(extract_refs("").is_empty());
    }

    #[test]
    fn extract_refs_no_refs() {
        assert!(extract_refs("plain text without refs").is_empty());
    }

    // -----------------------------------------------------------------------
    // Variables::substitute
    // -----------------------------------------------------------------------

    #[test]
    fn substitute_basic_replacement() {
        let vars = Variables {
            vars: HashMap::from([("NAME".to_string(), "world".to_string())]),
        };
        assert_eq!(vars.substitute("Hello {{NAME}}!"), "Hello world!");
    }

    #[test]
    fn substitute_multiple_variables() {
        let vars = Variables {
            vars: HashMap::from([
                ("OWNER".to_string(), "acme".to_string()),
                ("REPO".to_string(), "widgets".to_string()),
            ]),
        };
        assert_eq!(vars.substitute("{{OWNER}}/{{REPO}}"), "acme/widgets");
    }

    #[test]
    fn substitute_single_pass_no_reprocessing() {
        // If INNER expands to something with {{...}}, it should NOT be re-expanded.
        let vars = Variables {
            vars: HashMap::from([
                ("OUTER".to_string(), "{{INNER}}".to_string()),
                ("INNER".to_string(), "deep".to_string()),
            ]),
        };
        assert_eq!(vars.substitute("{{OUTER}}"), "{{INNER}}");
    }

    #[test]
    fn substitute_passes_through_unclosed_braces() {
        let vars = Variables {
            vars: HashMap::new(),
        };
        assert_eq!(vars.substitute("{{NAME"), "{{NAME");
        assert_eq!(vars.substitute("NAME}}"), "NAME}}");
    }

    #[test]
    fn substitute_passes_through_lowercase_patterns() {
        let vars = Variables {
            vars: HashMap::new(),
        };
        assert_eq!(vars.substitute("{{name}}"), "{{name}}");
    }

    #[test]
    fn substitute_no_match_passthrough() {
        let vars = Variables {
            vars: HashMap::new(),
        };
        assert_eq!(vars.substitute("plain text"), "plain text");
    }

    #[test]
    #[should_panic(expected = "undefined variable reference")]
    fn substitute_panics_on_undefined_ref() {
        let vars = Variables {
            vars: HashMap::new(),
        };
        vars.substitute("{{UNDEFINED}}");
    }

    // -----------------------------------------------------------------------
    // Variables::from_events
    // -----------------------------------------------------------------------

    #[test]
    fn from_events_extracts_variables() {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::from([
                    ("OWNER".to_string(), "acme".to_string()),
                    ("REPO".to_string(), "widgets".to_string()),
                ]),
                spawn_entry: None,
            },
        }];

        let vars = Variables::from_events(&events).unwrap();
        assert!(!vars.is_empty());
        assert_eq!(vars.substitute("{{OWNER}}/{{REPO}}"), "acme/widgets");
    }

    #[test]
    fn from_events_empty_when_no_init() {
        let vars = Variables::from_events(&[]).unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn from_events_rejects_invalid_value() {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::from([("BAD".to_string(), "has spaces".to_string())]),
                spawn_entry: None,
            },
        }];

        let err = Variables::from_events(&events).unwrap_err();
        assert_eq!(err.key, "BAD");
    }

    #[test]
    fn from_events_with_valid_special_chars() {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::from([("PATH".to_string(), "org/repo-name_v1.2".to_string())]),
                spawn_entry: None,
            },
        }];

        let vars = Variables::from_events(&events).unwrap();
        assert_eq!(vars.substitute("{{PATH}}"), "org/repo-name_v1.2");
    }
}
