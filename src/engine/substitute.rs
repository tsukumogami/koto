use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

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

/// Values produced during the tick that is running right now, layered over the
/// bindings [`Variables::from_events`] read before it started.
///
/// One tick creates one overlay and hands it to every consumer that resolves a
/// variable: the gate closure, the action closure, `advance_until_stop`, and
/// the final directive substitution. Without it those consumers read a binding
/// built before the advancement loop ran, so a value produced mid-tick is on
/// disk and invisible to the rest of the same tick -- which is exactly the case
/// auto-advance exists for.
///
/// The overlay is per-tick and lives only as long as the call. The event log
/// stays the durable record: a later tick reconstructs everything through
/// [`Variables::from_events`] and starts again with an empty overlay. Writers
/// update the overlay in the same step that appends the event, so the on-disk
/// record and the in-memory view never diverge.
///
/// It is passed as an explicit parameter rather than read from a global, so a
/// new consumer inside the loop that ignores it is visible in review.
///
/// Interior mutability is what lets the advancement loop write to an overlay
/// that the gate and action closures are holding a shared borrow of. Nothing
/// resolves a variable while a write is in flight, so the `RefCell` never
/// re-enters.
#[derive(Debug, Default)]
pub struct VariableOverlay {
    values: RefCell<HashMap<String, String>>,
}

impl VariableOverlay {
    /// Create an empty overlay for one tick.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a value under `key` for the rest of this tick.
    ///
    /// A second write to the same name replaces the first, matching the event
    /// fold: re-entering a producing state means the later value wins.
    pub fn insert(&self, key: impl Into<String>, value: impl Into<String>) {
        self.values.borrow_mut().insert(key.into(), value.into());
    }

    /// Look up a name written earlier in this tick.
    pub fn get(&self, key: &str) -> Option<String> {
        self.values.borrow().get(key).cloned()
    }

    /// True when nothing has been written this tick.
    pub fn is_empty(&self) -> bool {
        self.values.borrow().is_empty()
    }

    /// Layer this overlay over `base` for a consumer that needs a plain map --
    /// `vars.*` when-clause evaluation is the one in the tree.
    ///
    /// Call it at each read rather than once, so a value written earlier in the
    /// same tick is visible. An empty overlay borrows `base` untouched, which is
    /// what keeps a tick that captures nothing byte-identical to one that never
    /// had an overlay at all.
    pub fn layered_over<'a>(
        &self,
        base: &'a HashMap<String, String>,
    ) -> Cow<'a, HashMap<String, String>> {
        if self.is_empty() {
            return Cow::Borrowed(base);
        }
        let mut merged = base.clone();
        for (key, value) in self.values.borrow().iter() {
            merged.insert(key.clone(), value.clone());
        }
        Cow::Owned(merged)
    }
}

/// How a resolved value is written into the string being substituted.
///
/// The form follows what the surrounding string is about to be handed to, not
/// what the value looks like, because the consumer is what decides whether a
/// character in the value is read as itself. This is the one place the whole
/// argument is written down; the call-site helpers point here.
///
/// The three are not as uniform as that reads, and the difference matters to
/// anyone adding a fourth. Each variant settles two independent things -- what
/// an empty value renders as, and what a non-empty one is transformed into --
/// and no variant so far needs both. `Plain` is identity on both. `RegexLiteral`
/// escapes and leaves the empty case alone. `ShellWord` is the odd one: its
/// non-empty case falls through to `Plain`'s arm, and the whole variant exists
/// for its empty case, which is decided by looking at the characters on either
/// side of the reference rather than at the value. So a fourth *escaping* form
/// slots in beside `RegexLiteral` additively, while a form needing both cells
/// filled has to be placed in the right arm of `substitute_inner`'s match --
/// which is ordered, and which nothing here would catch getting wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueForm {
    /// Verbatim. Directives, details, an action's `working_dir`, and a context
    /// gate's `key` -- none of them are parsed further, so a value means itself
    /// already. An empty value renders as nothing, which is why
    /// `key: "{{PREFIX}}note"` with an empty prefix asks the context store for
    /// `note` rather than for `''note`.
    Plain,
    /// Verbatim, except that an empty value becomes an explicit `''` so an
    /// unquoted `--flag {{VAR}}` stays a well-formed argument (Issue #186).
    /// For strings that reach `sh -c`.
    ShellWord,
    /// Escaped, so the value contributes literal characters and nothing else.
    /// For a string the regex engine compiles -- a `context-matches` gate's
    /// `pattern` is the only one a tick substitutes (Issue #222).
    ///
    /// Escaping loses almost no expressive power, because [`VALUE_PATTERN`] has
    /// already spent it. A value can carry no anchor, group, alternation or
    /// quantifier into a pattern under either reading: not one of
    /// `^ $ ( ) [ ] * + ? | \` is in the set. What it can carry is narrow, and
    /// listing it is the whole of what the escape decides -- `.` as a wildcard,
    /// `-` as a range operator inside a class the author opened, `:` as the
    /// delimiter of a POSIX class name, whitespace under `(?x)`, and an
    /// alphanumeric as a range endpoint or the body of a class name. Every one
    /// of those is far more often an accident than an intent: a `.` is a version
    /// number or a session name, and `validate_workflow_name` permits one, so
    /// `{{SESSION_NAME}}` scoping a pattern would otherwise match sessions it
    /// was written to exclude. [`escape_value_for_pattern`] closes all of them,
    /// including the two `regex::escape` leaves alone. The cost is real but
    /// small -- `[{{CHARS}}]` with `CHARS` of `a-z` used to build a range and
    /// now matches those three characters literally.
    ///
    /// Two consequences a maintainer needs before touching this. The value is
    /// not wrapped in a group, so a quantifier written immediately after a
    /// reference binds to the value's last character: `{{TAG}}?` with a `TAG` of
    /// `v1` is `v1?`, not "optionally `v1`". Grouping would fix that and break
    /// the character-class case above, which is the more defensible of the two
    /// to keep working. And the escape is load-bearing outside this module --
    /// see the warning on [`Variables::substitute_regex_literal_with`].
    RegexLiteral,
}

/// Render `value` so a regex reads every character in it as itself.
///
/// `regex::escape` is most of this, and would be all of it if the only thing
/// that mattered were the metacharacter set. Two characters it deliberately
/// leaves alone can still reach a pattern's structure, and both were found by
/// probing rather than by reading the escape's documentation:
///
/// - `:` delimits a POSIX class name, so a value of `:` completes
///   `[[{{V}}digit:]]` into `[[:digit:]]` and the value contributes a character
///   class rather than a literal.
/// - whitespace is discarded under `(?x)`, so `(?x)^{{V}}$` with a `V` of `a b`
///   stops matching `a b` and starts matching `ab`. That is the widening
///   direction -- the gate rejects the value it was scoped to and accepts a
///   string nobody wrote -- which is the exact hazard escaping exists to close.
///
/// Both need the author to have opened the shape (`[[`, or `(?x)`), so neither
/// is likely. They are closed anyway, because "the value means itself" is worth
/// being true rather than nearly true: a caveat is something the next author has
/// to know, and this one would have lived only in a comment.
///
/// `\x{..}` rather than a backslash escape because it is unambiguous in all
/// three positions a value can land in -- top level, inside a character class
/// the author opened, and under `(?x)`.
pub fn escape_value_for_pattern(value: &str) -> String {
    let escaped = regex::escape(value);
    if !escaped.chars().any(|c| c == ':' || c.is_whitespace()) {
        return escaped;
    }
    escaped
        .chars()
        .map(|c| {
            if c == ':' || c.is_whitespace() {
                format!("\\x{{{:x}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
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

/// Fold a session's log into the variable bindings a tick starts from: the
/// `WorkflowInitialized` block, then every value a `default_action` captured.
///
/// Captures fold in event order, so re-entering a producing state means the
/// later value wins. Nothing removes a binding: a rewind appends a `Rewound`
/// event and truncates no log, so a value captured before the rewind is still
/// bound after it (DESIGN-koto-runs-commands.md, "Lifetime and identity of a
/// captured value").
///
/// One function rather than two folds, because `Variables` and the advance
/// loop's `vars.*` map must agree on what is set -- a directive that renders a
/// captured value while a `vars.NAME is_set` clause calls the same name unset
/// would be the worse kind of bug to chase.
pub fn bindings_from_events(events: &[Event]) -> HashMap<String, String> {
    let mut vars: HashMap<String, String> = HashMap::new();
    for event in events {
        match &event.payload {
            EventPayload::WorkflowInitialized { variables, .. } => {
                vars.extend(variables.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            EventPayload::VariableCaptured { key, value } => {
                vars.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    vars
}

impl Variables {
    /// Extract variables from the log: the WorkflowInitialized bindings plus
    /// every captured value, folded in event order.
    /// Re-validates all values against the allowlist as defense in depth.
    pub fn from_events(events: &[Event]) -> Result<Self, SubstitutionError> {
        let vars = bindings_from_events(events);

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
        self.substitute_inner(input, ValueForm::Plain, None)
    }

    /// Like [`substitute`](Self::substitute), but consults a per-tick overlay
    /// before the bindings read from the log.
    ///
    /// The lookup order across the whole engine is fixed: runtime names
    /// (`SESSION_DIR`, `SESSION_NAME`) substitute first, in a separate pass the
    /// caller runs through `crate::cli::vars::substitute_vars`; then this
    /// overlay; then the `WorkflowInitialized` bindings. Because a value from
    /// either of the last two layers is written into the output of a single
    /// pass, a value that itself contains a `{{...}}` token is never
    /// re-expanded.
    pub fn substitute_with(&self, input: &str, overlay: &VariableOverlay) -> String {
        self.substitute_inner(input, ValueForm::Plain, Some(overlay))
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
        self.substitute_inner(input, ValueForm::ShellWord, None)
    }

    /// Like [`substitute_command`](Self::substitute_command), but consults a
    /// per-tick overlay before the bindings read from the log. Same lookup
    /// order as [`substitute_with`](Self::substitute_with).
    pub fn substitute_command_with(&self, input: &str, overlay: &VariableOverlay) -> String {
        self.substitute_inner(input, ValueForm::ShellWord, Some(overlay))
    }

    /// Like [`substitute_with`](Self::substitute_with), but for a string that is
    /// compiled as a regex -- a `context-matches` gate's `pattern` is the one in
    /// the tree.
    ///
    /// Each resolved value is escaped, so it contributes literal characters and
    /// nothing else. The surrounding pattern is untouched: anchors, classes and
    /// quantifiers the author wrote still mean what they say, and a pattern
    /// holding no `{{KEY}}` reference comes back byte-identical. Why escaping
    /// rather than raw substitution, and what it costs, is argued once on
    /// [`ValueForm::RegexLiteral`].
    ///
    /// **The escape is load-bearing outside this module, and weakening it is not
    /// a local change.** `CompiledTemplate::validate` checks that a `pattern` is
    /// a valid regex before any value exists, by compiling it with each
    /// reference stood in for by one literal character
    /// (`pattern_with_refs_as_literals` in `src/template/types.rs`). That check
    /// is sound only because an escaped value contributes literal characters and
    /// nothing else, so one placeholder stands in for it faithfully. Substitute
    /// raw and the check becomes one a value can defeat: a pattern that compiles
    /// at `koto init` and fails at the gate, or -- worse -- a value that quietly
    /// widens a narrow pattern. The unit tests that pin the escape fail if it is
    /// removed, but they read as "escaping changed", so they will not tell you
    /// this. Revisit the compile-time check in the same breath.
    pub fn substitute_regex_literal_with(&self, input: &str, overlay: &VariableOverlay) -> String {
        self.substitute_inner(input, ValueForm::RegexLiteral, Some(overlay))
    }

    fn substitute_inner(
        &self,
        input: &str,
        form: ValueForm,
        overlay: Option<&VariableOverlay>,
    ) -> String {
        let re = Regex::new(VAR_REF_PATTERN).expect("VAR_REF_PATTERN is a valid regex");
        let mut result = String::with_capacity(input.len());
        let mut last_end = 0;

        for caps in re.captures_iter(input) {
            let whole_match = caps.get(0).unwrap();
            let key = &caps[1];

            result.push_str(&input[last_end..whole_match.start()]);

            // Overlay before bindings: a value captured earlier in this tick
            // wins over the one the log carried into it. Runtime names were
            // already replaced by the caller's earlier pass, so they cannot
            // reach here.
            let resolved = overlay
                .and_then(|o| o.get(key))
                .or_else(|| self.vars.get(key).cloned());

            match resolved.as_deref() {
                Some(value) if form == ValueForm::ShellWord && value.is_empty() => {
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
                Some(value) if form == ValueForm::RegexLiteral => {
                    // Not only so the value means itself: the compiler's
                    // regex check on a `pattern` stands each reference in for
                    // one literal character, which is faithful only while this
                    // escape holds. See `substitute_regex_literal_with`.
                    result.push_str(&escape_value_for_pattern(value))
                }
                Some(value) => result.push_str(value),
                None => {
                    // Undefined reference: pass the literal token through rather
                    // than panic (Issue #184). See the method docs. Under
                    // `RegexLiteral` that token is not a valid regex, so the
                    // gate reports `invalid regex pattern` instead of quietly
                    // failing to match -- which is the louder of the two, and
                    // the reason `pattern` was the better-off half before
                    // Issue #222.
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

    /// True when `key` resolves against the bindings read from the log.
    ///
    /// Used to tell an undelivered capture name from a delivered one before
    /// substitution runs, where an unresolved name has to become an error
    /// rather than the token itself.
    pub fn is_bound(&self, key: &str) -> bool {
        self.vars.contains_key(key)
    }
}

/// The first `{{KEY}}` reference in `input` that names a declared capture no
/// state has delivered, paired with the state that would have delivered it.
///
/// A capture name is not a declared variable, so `koto init` never
/// materializes one and the ordinary pass-through behaviour would render the
/// raw `{{KEY}}` token into an agent's instructions -- the outcome R4 exists to
/// prevent. The caller turns a hit into a typed run-time stop; declared
/// variables keep passing through untouched.
///
/// Both layers a value can arrive from are consulted, in the tick's fixed
/// order: the overlay for a capture this tick produced, then the bindings the
/// log carried in.
pub fn first_unset_capture(
    input: &str,
    captures: &BTreeMap<String, String>,
    variables: &Variables,
    overlay: &VariableOverlay,
) -> Option<(String, String)> {
    if captures.is_empty() {
        return None;
    }
    for name in crate::template::types::extract_refs(input) {
        let Some(producer) = captures.get(&name) else {
            continue;
        };
        if overlay.get(&name).is_none() && !variables.is_bound(&name) {
            return Some((name, producer.clone()));
        }
    }
    None
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

    // -----------------------------------------------------------------------
    // VariableOverlay
    // -----------------------------------------------------------------------

    fn vars_from(pairs: &[(&str, &str)]) -> Variables {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];
        Variables::from_events(&events).unwrap()
    }

    #[test]
    fn overlay_starts_empty() {
        let overlay = VariableOverlay::new();
        assert!(overlay.is_empty());
        assert_eq!(overlay.get("BRANCH"), None);
    }

    #[test]
    fn overlay_later_write_wins() {
        let overlay = VariableOverlay::new();
        overlay.insert("BRANCH", "first");
        overlay.insert("BRANCH", "second");
        assert_eq!(overlay.get("BRANCH").as_deref(), Some("second"));
    }

    #[test]
    fn empty_overlay_substitutes_exactly_as_before() {
        // Behaviour-neutrality at the substitution layer: with nothing written
        // this tick, the overlay-aware entry points must agree with the ones
        // that predate them, character for character.
        let vars = vars_from(&[("BRANCH", "main"), ("EMPTY", "")]);
        let overlay = VariableOverlay::new();

        let plain = "on {{BRANCH}} and {{EMPTY}} and {{UNKNOWN}}";
        assert_eq!(
            vars.substitute(plain),
            vars.substitute_with(plain, &overlay)
        );

        let command = "git checkout {{BRANCH}} --flag {{EMPTY}} {{UNKNOWN}}";
        assert_eq!(
            vars.substitute_command(command),
            vars.substitute_command_with(command, &overlay)
        );
    }

    #[test]
    fn overlay_resolves_a_name_the_log_never_carried() {
        let vars = vars_from(&[("OTHER", "x")]);
        let overlay = VariableOverlay::new();
        overlay.insert("BRANCH", "feature-42");

        assert_eq!(
            vars.substitute_with("work on {{BRANCH}}", &overlay),
            "work on feature-42"
        );
        assert_eq!(
            vars.substitute_command_with("git log {{BRANCH}}", &overlay),
            "git log feature-42"
        );
    }

    #[test]
    fn overlay_wins_over_the_log_binding() {
        // Lookup order: overlay before the WorkflowInitialized bindings.
        let vars = vars_from(&[("BRANCH", "stale")]);
        let overlay = VariableOverlay::new();
        overlay.insert("BRANCH", "fresh");

        assert_eq!(vars.substitute_with("{{BRANCH}}", &overlay), "fresh");
        assert_eq!(
            vars.substitute_command_with("{{BRANCH}}", &overlay),
            "fresh"
        );
    }

    #[test]
    fn overlay_value_is_not_re_expanded() {
        // Overlay and bindings resolve in one pass, so a value that itself
        // looks like a reference is emitted literally rather than expanded
        // through the layer below it.
        let vars = vars_from(&[("INNER", "expanded")]);
        let overlay = VariableOverlay::new();
        overlay.insert("OUTER", "{{INNER}}");

        assert_eq!(vars.substitute_with("{{OUTER}}", &overlay), "{{INNER}}");
    }

    #[test]
    fn overlay_empty_value_stays_shell_safe() {
        // The #186 empty-argument rule applies to an overlay value too: an
        // unquoted reference resolving to empty still renders as `''`.
        let vars = vars_from(&[]);
        let overlay = VariableOverlay::new();
        overlay.insert("BRANCH", "");

        assert_eq!(
            vars.substitute_command_with("git log {{BRANCH}} --oneline", &overlay),
            "git log '' --oneline"
        );
    }

    #[test]
    fn layered_over_borrows_base_when_overlay_is_empty() {
        let base = HashMap::from([("A".to_string(), "1".to_string())]);
        let overlay = VariableOverlay::new();
        let layered = overlay.layered_over(&base);
        assert!(matches!(layered, Cow::Borrowed(_)));
        assert_eq!(layered.get("A").map(String::as_str), Some("1"));
    }

    #[test]
    fn layered_over_shadows_base_and_adds_new_names() {
        let base = HashMap::from([
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "stale".to_string()),
        ]);
        let overlay = VariableOverlay::new();
        overlay.insert("B", "fresh");
        overlay.insert("C", "new");

        let layered = overlay.layered_over(&base);
        assert_eq!(layered.get("A").map(String::as_str), Some("1"));
        assert_eq!(layered.get("B").map(String::as_str), Some("fresh"));
        assert_eq!(layered.get("C").map(String::as_str), Some("new"));
        // The base is untouched: the overlay layers over it, never into it.
        assert_eq!(base.get("B").map(String::as_str), Some("stale"));
    }

    // -----------------------------------------------------------------------
    // captured values: the event fold and the undelivered-name check
    // -----------------------------------------------------------------------

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn initialized(pairs: &[(&str, &str)]) -> EventPayload {
        EventPayload::WorkflowInitialized {
            template_path: "/cache/abc.json".to_string(),
            variables: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            spawn_entry: None,
        }
    }

    fn captured(key: &str, value: &str) -> EventPayload {
        EventPayload::VariableCaptured {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn bindings_fold_captures_over_the_initialized_block() {
        let events = vec![
            event(1, initialized(&[("REPO", "widgets")])),
            event(2, captured("BRANCH", "main")),
        ];
        let bindings = bindings_from_events(&events);
        assert_eq!(bindings.get("REPO").map(String::as_str), Some("widgets"));
        assert_eq!(bindings.get("BRANCH").map(String::as_str), Some("main"));
    }

    #[test]
    fn a_second_capture_of_the_same_name_wins() {
        // Re-entering the producing state appends rather than replaces, so
        // the fold has to be ordered for the later value to win.
        let events = vec![
            event(1, initialized(&[])),
            event(2, captured("BRANCH", "first")),
            event(3, captured("BRANCH", "second")),
        ];
        assert_eq!(
            bindings_from_events(&events)
                .get("BRANCH")
                .map(String::as_str),
            Some("second")
        );
    }

    #[test]
    fn a_rewind_leaves_a_captured_value_bound() {
        // A rewind appends an event and truncates nothing, so the value a
        // command already produced is still bound afterwards.
        let events = vec![
            event(1, initialized(&[])),
            event(2, captured("BRANCH", "main")),
            event(
                3,
                EventPayload::Rewound {
                    from: "report".to_string(),
                    to: "detect".to_string(),
                    rationale: None,
                },
            ),
        ];
        assert_eq!(
            bindings_from_events(&events)
                .get("BRANCH")
                .map(String::as_str),
            Some("main")
        );
    }

    #[test]
    fn from_events_validates_a_captured_value() {
        // Defense in depth: delivery already ran the value through the
        // allowlist, so a value that fails here came from a hand-edited log.
        let events = vec![
            event(1, initialized(&[])),
            event(2, captured("BRANCH", "value;rm -rf")),
        ];
        let err = Variables::from_events(&events).unwrap_err();
        assert_eq!(err.key, "BRANCH");
    }

    fn capture_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn first_unset_capture_finds_an_undelivered_name() {
        let variables = vars_from(&[]);
        let overlay = VariableOverlay::new();
        let captures = capture_map(&[("BRANCH", "detect")]);
        assert_eq!(
            first_unset_capture("on {{BRANCH}}", &captures, &variables, &overlay),
            Some(("BRANCH".to_string(), "detect".to_string()))
        );
    }

    #[test]
    fn first_unset_capture_ignores_a_name_the_tick_just_produced() {
        let variables = vars_from(&[]);
        let overlay = VariableOverlay::new();
        overlay.insert("BRANCH", "main");
        let captures = capture_map(&[("BRANCH", "detect")]);
        assert_eq!(
            first_unset_capture("on {{BRANCH}}", &captures, &variables, &overlay),
            None
        );
    }

    #[test]
    fn first_unset_capture_ignores_a_name_an_earlier_tick_produced() {
        let variables = vars_from(&[("BRANCH", "main")]);
        let overlay = VariableOverlay::new();
        let captures = capture_map(&[("BRANCH", "detect")]);
        assert_eq!(
            first_unset_capture("on {{BRANCH}}", &captures, &variables, &overlay),
            None
        );
    }

    #[test]
    fn first_unset_capture_leaves_declared_variables_alone() {
        // An unresolved declared variable keeps its pass-through behaviour:
        // only capture names become a stop.
        let variables = vars_from(&[]);
        let overlay = VariableOverlay::new();
        let captures = capture_map(&[("BRANCH", "detect")]);
        assert_eq!(
            first_unset_capture("on {{REPO}}", &captures, &variables, &overlay),
            None
        );
    }
}
