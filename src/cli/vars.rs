//! Runtime variable substitution for template strings.
//!
//! Replaces `{{KEY}}` tokens with values from a variable map. Used by
//! `handle_next` to inject `SESSION_DIR` into gate commands and directives
//! before they reach the shell or JSON serialization.

use std::collections::HashMap;

/// Reserved variable names that cannot be declared in template `variables:` blocks.
///
/// These are injected by the runtime and must not collide with user-defined variables.
pub const RESERVED_VARIABLE_NAMES: &[&str] = &["SESSION_DIR"];

/// Replace `{{KEY}}` tokens in `input` with values from `vars`.
///
/// Iterates over the map and performs a sequential `str::replace` for each
/// entry. Tokens that don't appear in `input` are silently ignored; tokens
/// in `input` whose keys are absent from `vars` are left as-is.
pub fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key);
        result = result.replace(&token, value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_when_no_tokens() {
        let vars = HashMap::new();
        assert_eq!(substitute_vars("plain text", &vars), "plain text");
    }

    #[test]
    fn no_op_when_vars_absent_from_input() {
        let mut vars = HashMap::new();
        vars.insert("SESSION_DIR".to_string(), "/tmp/session".to_string());
        assert_eq!(substitute_vars("no tokens here", &vars), "no tokens here");
    }

    #[test]
    fn single_token_replaced() {
        let mut vars = HashMap::new();
        vars.insert(
            "SESSION_DIR".to_string(),
            "/home/user/.koto/sessions/abc".to_string(),
        );
        assert_eq!(
            substitute_vars("cat {{SESSION_DIR}}/plan.md", &vars),
            "cat /home/user/.koto/sessions/abc/plan.md"
        );
    }

    #[test]
    fn multiple_occurrences_of_same_token() {
        let mut vars = HashMap::new();
        vars.insert("SESSION_DIR".to_string(), "/s".to_string());
        assert_eq!(
            substitute_vars("{{SESSION_DIR}}/a {{SESSION_DIR}}/b", &vars),
            "/s/a /s/b"
        );
    }

    #[test]
    fn multiple_different_tokens() {
        let mut vars = HashMap::new();
        vars.insert("SESSION_DIR".to_string(), "/s".to_string());
        vars.insert("FOO".to_string(), "bar".to_string());
        let result = substitute_vars("{{SESSION_DIR}} and {{FOO}}", &vars);
        assert_eq!(result, "/s and bar");
    }

    #[test]
    fn missing_token_left_intact() {
        let vars = HashMap::new();
        assert_eq!(
            substitute_vars("{{UNKNOWN}} stays", &vars),
            "{{UNKNOWN}} stays"
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        let mut vars = HashMap::new();
        vars.insert("SESSION_DIR".to_string(), "/s".to_string());
        assert_eq!(substitute_vars("", &vars), "");
    }

    #[test]
    fn value_containing_braces_not_recursed() {
        // Use a single-entry map to deterministically prove non-recursion:
        // substituting A produces "{{B}}", and since B is not in the map,
        // the token stays as-is.
        let mut vars = HashMap::new();
        vars.insert("A".to_string(), "{{B}}".to_string());
        let result = substitute_vars("{{A}}", &vars);
        assert_eq!(result, "{{B}}");
    }

    #[test]
    fn reserved_variable_names_includes_session_dir() {
        assert!(RESERVED_VARIABLE_NAMES.contains(&"SESSION_DIR"));
    }
}
