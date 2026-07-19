use std::collections::HashMap;

use regex::Regex;

use crate::engine::types::{Event, EventPayload};
use crate::template::types::VAR_REF_PATTERN;

/// Allowlist regex for variable values.
///
/// A substituted `{{KEY}}` value can land in a `sh -c` gate command or an agent
/// instruction, so the value set is an allowlist, not a denylist: every
/// character that could execute a command, trigger an expansion, or redirect
/// I/O stays out by default. The set is deliberately conservative -- widen it
/// only with a per-character justification.
///
/// Allowed characters:
/// - `a-z A-Z 0-9` and `. _ -` -- identifiers, versions, filenames.
/// - `/` -- path separators (e.g. `org/repo`).
/// - `:` `@` -- structured data values such as Gmail filters (`newer_than:90d`,
///   `from:user@example.com`). Neither is a shell metacharacter, so both are
///   literal inside a `sh -c` word (Issue #180).
/// - space -- structured names such as a calendar title. A space is not a
///   command-injection vector: it introduces no command, expansion, or
///   redirection. Its only effect in an unquoted interpolation is word
///   splitting, so template authors should quote `{{KEY}}` where a value must
///   stay a single shell argument (Issue #180).
///
/// Empty strings are allowed for optional variables with no default (Issue #141).
const VALUE_PATTERN: &str = r"^[a-zA-Z0-9._/:@ \-]*$";

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
    /// An undefined reference is left intact rather than substituted, and never
    /// panics. Compile-time validation (`src/template/types.rs`) rejects any
    /// template that references an undeclared variable with an actionable error,
    /// and `koto init` materializes every declared variable (including empty
    /// defaults), so a `{{KEY}}` that reaches substitution always resolves in
    /// practice. Passing an unresolved token through unchanged is defense in
    /// depth: a missing variable is a user or template error, not an internal
    /// invariant break that should crash with a backtrace (Issue #184).
    pub fn substitute(&self, input: &str) -> String {
        self.substitute_inner(input, false)
    }

    /// Like [`substitute`](Self::substitute), but safe for values that land in a
    /// `sh -c` command string.
    ///
    /// When a variable resolves to an empty string and its `{{KEY}}` reference
    /// is not already wrapped in a shell quote, the token is rendered as an
    /// explicit empty argument (`''`). Without this, an unquoted `--flag
    /// {{VAR}}` with an empty `VAR` renders `--flag ` -- the argv splitter drops
    /// the empty token and the next flag is consumed as the value, corrupting
    /// the command (Issue #186). This pairs with Issue #184: once an optional
    /// variable's empty default is materialized, safe interpolation is what
    /// keeps the resulting command well-formed.
    ///
    /// Non-empty values are emitted verbatim, exactly as [`substitute`](Self::substitute)
    /// does: quoting a value that may contain spaces stays the template author's
    /// responsibility (Issue #180), so this method changes nothing for them.
    pub fn substitute_command(&self, input: &str) -> String {
        self.substitute_inner(input, true)
    }

    fn substitute_inner(&self, input: &str, shell_safe: bool) -> String {
        let re = Regex::new(VAR_REF_PATTERN).expect("VAR_REF_PATTERN is a valid regex");
        let mut result = String::with_capacity(input.len());
        let mut last_end = 0;

        for caps in re.captures_iter(input) {
            let whole_match = caps.get(0).unwrap();
            let key = &caps[1];

            result.push_str(&input[last_end..whole_match.start()]);

            match self.vars.get(key) {
                Some(value) if shell_safe && value.is_empty() => {
                    // Empty value in a shell command. Emit an explicit empty
                    // argument so the token stays a distinct, empty word --
                    // unless the author already wrapped the reference in a
                    // quote, in which case injecting `''` would produce the
                    // literal two-character string instead.
                    let prev = input[..whole_match.start()].chars().next_back();
                    let next = input[whole_match.end()..].chars().next();
                    let author_quoted = matches!(prev, Some('\'') | Some('"'))
                        || matches!(next, Some('\'') | Some('"'));
                    if !author_quoted {
                        result.push_str("''");
                    }
                }
                Some(value) => result.push_str(value),
                None => {
                    // Undefined reference: pass the literal token through rather
                    // than panic (Issue #184). See the method docs.
                    result.push_str(whole_match.as_str());
                }
            }

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
    fn validate_value_accepts_spaces() {
        // Spaces are allowed for structured data values such as a calendar name
        // (Issue #180). A space introduces no shell command, expansion, or
        // redirection; its only effect in an unquoted interpolation is word
        // splitting, which template authors control by quoting `{{KEY}}`.
        validate_value("KEY", "Weekly Planning").unwrap();
    }

    #[test]
    fn validate_value_accepts_colon_and_at() {
        // Colon and at-sign are not shell metacharacters, so they are literal
        // inside a `sh -c` word. They unblock structured values like Gmail
        // search filters (Issue #180).
        validate_value("SINCE", "newer_than:90d").unwrap();
        validate_value("FROM", "from:delta@delta.com").unwrap();
    }

    #[test]
    fn validate_value_accepts_empty() {
        // Empty strings are valid for optional variables with no default (Issue #141).
        validate_value("KEY", "").unwrap();
    }

    #[test]
    fn validate_value_rejects_special_chars() {
        // The allowlist must keep out every character that can execute a
        // command, trigger an expansion, or redirect I/O once the value lands
        // in a `sh -c` gate command or an agent instruction (Issue #180 keeps
        // this guarantee intact while widening the safe set).
        validate_value("KEY", "value;rm -rf").unwrap_err(); // command separator
        validate_value("KEY", "$(evil)").unwrap_err(); // command substitution
        validate_value("KEY", "`evil`").unwrap_err(); // backtick substitution
        validate_value("KEY", "a\nb").unwrap_err(); // newline
        validate_value("KEY", "a|b").unwrap_err(); // pipe
        validate_value("KEY", "a&b").unwrap_err(); // background / and-list
        validate_value("KEY", "a>b").unwrap_err(); // redirection
        validate_value("KEY", "a*b").unwrap_err(); // glob
        validate_value("KEY", "${HOME}").unwrap_err(); // parameter expansion
        validate_value("KEY", "a'b").unwrap_err(); // single quote
        validate_value("KEY", "a\"b").unwrap_err(); // double quote
        validate_value("KEY", "a\\b").unwrap_err(); // backslash
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
    fn substitute_leaves_undefined_ref_intact() {
        // A missing variable must never panic. Compile-time validation
        // (src/template/types.rs) already rejects a template that references an
        // undeclared variable with an actionable error, and `koto init`
        // materializes every declared variable, so this path is defense in
        // depth: if an undefined reference ever reaches substitution, leave the
        // literal token in place rather than crash with a backtrace (Issue #184).
        let vars = Variables {
            vars: HashMap::new(),
        };
        assert_eq!(vars.substitute("{{UNDEFINED}}"), "{{UNDEFINED}}");
        assert_eq!(vars.substitute("a {{UNDEFINED}} b"), "a {{UNDEFINED}} b");
    }

    // -----------------------------------------------------------------------
    // Variables::substitute_command (empty-value shell safety, Issue #186)
    // -----------------------------------------------------------------------

    #[test]
    fn substitute_command_quotes_empty_unquoted_token() {
        // An unquoted `{{VAR}}` whose value is empty would otherwise vanish,
        // letting the argv splitter consume the next flag as the value. Render
        // it as an explicit empty shell argument instead (Issue #186).
        let vars = Variables {
            vars: HashMap::from([("START".to_string(), String::new())]),
        };
        assert_eq!(
            vars.substitute_command("cmd --start {{START}} --dir d"),
            "cmd --start '' --dir d"
        );
    }

    #[test]
    fn substitute_command_leaves_nonempty_value_unquoted() {
        // Non-empty values are substituted verbatim, exactly as before. Word
        // splitting on spaces stays the template author's responsibility to
        // quote (Issue #180); command substitution must not change that.
        let vars = Variables {
            vars: HashMap::from([("START".to_string(), "2026-01".to_string())]),
        };
        assert_eq!(
            vars.substitute_command("cmd --start {{START}}"),
            "cmd --start 2026-01"
        );
    }

    #[test]
    fn substitute_command_preserves_author_double_quoted_empty() {
        // When the author already wraps the reference in quotes, an empty value
        // is well-formed on its own -- injecting `''` inside would produce the
        // literal two-character string `''`. Detect the adjacent quote and
        // leave the value empty.
        let vars = Variables {
            vars: HashMap::from([("CAL".to_string(), String::new())]),
        };
        assert_eq!(
            vars.substitute_command("cmd --calendar \"{{CAL}}\""),
            "cmd --calendar \"\""
        );
    }

    #[test]
    fn substitute_command_preserves_author_single_quoted_empty() {
        let vars = Variables {
            vars: HashMap::from([("CAL".to_string(), String::new())]),
        };
        assert_eq!(
            vars.substitute_command("cmd --calendar '{{CAL}}'"),
            "cmd --calendar ''"
        );
    }

    #[test]
    fn substitute_command_leaves_undefined_ref_intact() {
        // Same defense-in-depth guarantee as substitute(): never panic.
        let vars = Variables {
            vars: HashMap::new(),
        };
        assert_eq!(
            vars.substitute_command("cmd {{UNDEFINED}}"),
            "cmd {{UNDEFINED}}"
        );
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
            idempotency_hash: None,
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
                variables: HashMap::from([("BAD".to_string(), "value;rm -rf".to_string())]),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];

        let err = Variables::from_events(&events).unwrap_err();
        assert_eq!(err.key, "BAD");
    }

    #[test]
    fn from_events_accepts_structured_data_values() {
        // The motivating Issue #180 values: a Gmail window, a sender filter
        // with a colon and `@`, and a calendar name with spaces.
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::from([
                    ("SINCE".to_string(), "newer_than:90d".to_string()),
                    ("FROM".to_string(), "from:delta@delta.com".to_string()),
                    ("CALENDAR".to_string(), "Weekly Planning".to_string()),
                ]),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];

        let vars = Variables::from_events(&events).unwrap();
        assert_eq!(vars.substitute("{{SINCE}}"), "newer_than:90d");
        assert_eq!(vars.substitute("{{FROM}}"), "from:delta@delta.com");
        assert_eq!(vars.substitute("{{CALENDAR}}"), "Weekly Planning");
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
            idempotency_hash: None,
        }];

        let vars = Variables::from_events(&events).unwrap();
        assert_eq!(vars.substitute("{{PATH}}"), "org/repo-name_v1.2");
    }
}
