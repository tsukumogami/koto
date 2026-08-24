//! Runtime variable substitution for template strings.
//!
//! Replaces `{{KEY}}` tokens with values from a variable map. `handle_next` uses
//! it to inject `SESSION_DIR` and `SESSION_NAME` into every string a tick
//! substitutes -- directives and details, a gate's `command`, `key` and
//! `pattern`, and a `default_action` command and its `working_dir` -- before
//! they reach the shell, the context store, the regex engine, or JSON
//! serialization. `handle_status` uses it on the same directive and details, so
//! phase retrieval matches what `koto next` renders.
//!
//! This pass runs before the one that resolves declared variables, and it
//! validates nothing. The two names it injects are checked when the session is
//! created rather than against the value allowlist.

use std::collections::HashMap;

/// Reserved variable names that cannot be declared in template `variables:` blocks.
///
/// These are injected by the runtime and must not collide with user-defined variables.
pub const RESERVED_VARIABLE_NAMES: &[&str] = &["SESSION_DIR", "SESSION_NAME"];

/// Replace `{{KEY}}` tokens in `input` with values from `vars`.
///
/// Iterates over the map and performs a sequential `str::replace` for each
/// entry. Tokens that don't appear in `input` are silently ignored; tokens
/// in `input` whose keys are absent from `vars` are left as-is.
///
/// [`substitute_vars_regex_literal`] is a second copy of this loop for the one
/// input the regex engine compiles. Change how a token is matched here and it
/// needs changing there too.
pub fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key);
        result = result.replace(&token, value);
    }
    result
}

/// Like [`substitute_vars`], but for an `input` the regex engine will compile --
/// a `context-matches` gate's `pattern` is the one in the tree.
///
/// Each value is escaped so it matches itself. Both names need it. A session
/// name may contain a dot (`validate_workflow_name`), and `SESSION_DIR` is a
/// filesystem path built from the user's koto root, so it can hold characters
/// the declared-variable allowlist never has to consider -- a `+` or a paren in
/// a home directory would otherwise be read as a quantifier or a group
/// (Issue #222).
pub fn substitute_vars_regex_literal(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key);
        result = result.replace(
            &token,
            &crate::engine::substitute::escape_value_for_pattern(value),
        );
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

    #[test]
    fn regex_literal_escapes_the_value_not_the_pattern() {
        // A session name may hold a dot, and a session directory is a path
        // built from the user's home, so either can carry a character the regex
        // engine would otherwise read as structure (Issue #222).
        let mut vars = HashMap::new();
        vars.insert("SESSION_NAME".to_string(), "probe.one".to_string());

        let out = substitute_vars_regex_literal("^saw {{SESSION_NAME}}$", &vars);
        assert_eq!(out, r"^saw probe\.one$");

        // The anchors the author wrote still anchor, and the escaped dot only
        // matches a dot.
        let re = regex::Regex::new(&out).unwrap();
        assert!(re.is_match("saw probe.one"));
        assert!(!re.is_match("saw probeXone"));
    }

    #[test]
    fn regex_literal_closes_the_two_gaps_regex_escape_leaves_open() {
        // Found by probing rather than by reading `regex::escape`'s docs, and
        // both need the author to have opened the shape -- which is why they are
        // pinned here rather than left as a caveat in a comment.
        let mut vars = HashMap::new();

        // A colon can complete a POSIX class name inside a class the author
        // opened: `[[{{V}}digit:]]` would become `[[:digit:]]`.
        vars.insert("SESSION_NAME".to_string(), ":".to_string());
        let out = substitute_vars_regex_literal("^[[{{SESSION_NAME}}digit:]]+$", &vars);
        let re = regex::Regex::new(&out).unwrap();
        assert!(
            !re.is_match("123"),
            "a value must not be able to supply a character class; got {out}"
        );

        // Whitespace is discarded under `(?x)`, so an unescaped space makes the
        // value stop matching itself and start matching something wider.
        vars.insert("SESSION_NAME".to_string(), "a b".to_string());
        let out = substitute_vars_regex_literal("(?x)^{{SESSION_NAME}}$", &vars);
        let re = regex::Regex::new(&out).unwrap();
        assert!(
            re.is_match("a b"),
            "the value should match itself; got {out}"
        );
        assert!(
            !re.is_match("ab"),
            "free-spacing mode must not widen the value; got {out}"
        );
    }

    #[test]
    fn regex_literal_leaves_a_pattern_without_tokens_alone() {
        let mut vars = HashMap::new();
        vars.insert("SESSION_NAME".to_string(), "probe.one".to_string());
        assert_eq!(
            substitute_vars_regex_literal(r"status:\s+PASS", &vars),
            r"status:\s+PASS"
        );
    }
}
