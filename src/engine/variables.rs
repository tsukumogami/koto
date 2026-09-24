//! Typed variable refusals and the rebind primitive.
//!
//! [`VarError`] is the one vocabulary for refusing a variable, shared by
//! `koto init`'s resolution (`crate::cli::resolve_variables`) and by the
//! rebind primitive below, so a caller reads the same `code` and fields
//! whichever path refused.
//!
//! The rebind primitive re-applies a template's `rebind: true` variables from
//! one invocation onto a live session. It has no CLI surface of its own: a
//! rebind happens only inside an accepted attach, which runs its own checks
//! between [`validate_rebind`] and [`apply_rebind`]. That split is the point --
//! validate is side-effect free, so a refused attach changes nothing on the
//! session, and apply appends exactly one `variables_rebound` event.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::engine::persistence::derive_state_from_log;
use crate::engine::substitute::{bindings_from_events, validate_value, VALUE_PATTERN};
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::session::SessionBackend;
use crate::template::types::{CompiledTemplate, VariableDecl};

/// The `constraint` reported when a value fails the global allowlist rather
/// than a declared `values:` or `pattern:`.
pub const ALLOWLIST_CONSTRAINT: &str = "allowlist";

/// Why a variable was refused.
///
/// The typed variants carry a machine-readable [`code`](VarError::code) and
/// the fields a caller needs to act without parsing the message. `Malformed`
/// and `Missing` predate the codes and carry none; their messages are
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarError {
    /// A `KEY=VALUE` entry that isn't one.
    Malformed { entry: String, reason: String },
    /// The same key passed twice in one invocation (`duplicate_var`).
    Duplicate { var: String },
    /// A key the template doesn't declare (`unknown_var`).
    Unknown { var: String },
    /// A required variable with no default that wasn't passed.
    Missing { var: String },
    /// A value failing its declared constraint or the allowlist
    /// (`invalid_var`). `constraint` is `values:[...]`, `pattern:<re>`, or
    /// [`ALLOWLIST_CONSTRAINT`].
    Invalid {
        var: String,
        value: String,
        constraint: String,
    },
    /// An explicit value for a non-rebind variable that differs from the one
    /// the session recorded (`var_mismatch`).
    Mismatch {
        var: String,
        recorded: String,
        requested: String,
    },
    /// The session's current state is terminal, so nothing on it can be
    /// rebound (`terminal_session`).
    TerminalSession { state: String },
}

impl VarError {
    /// The machine-readable code, or `None` for the untyped refusals.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            VarError::Malformed { .. } | VarError::Missing { .. } => None,
            VarError::Duplicate { .. } => Some("duplicate_var"),
            VarError::Unknown { .. } => Some("unknown_var"),
            VarError::Invalid { .. } => Some("invalid_var"),
            VarError::Mismatch { .. } => Some("var_mismatch"),
            VarError::TerminalSession { .. } => Some("terminal_session"),
        }
    }

    /// The typed fields to merge into an error body: `code` plus the
    /// variant's detail. Empty for the untyped refusals.
    pub fn fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        let Some(code) = self.code() else {
            return m;
        };
        m.insert("code".into(), code.into());
        match self {
            VarError::Duplicate { var } | VarError::Unknown { var } => {
                m.insert("var".into(), var.as_str().into());
            }
            VarError::Invalid {
                var,
                value,
                constraint,
            } => {
                m.insert("var".into(), var.as_str().into());
                m.insert("value".into(), value.as_str().into());
                m.insert("constraint".into(), constraint.as_str().into());
            }
            VarError::Mismatch {
                var,
                recorded,
                requested,
            } => {
                m.insert("var".into(), var.as_str().into());
                m.insert("recorded".into(), recorded.as_str().into());
                m.insert("requested".into(), requested.as_str().into());
            }
            VarError::TerminalSession { state } => {
                m.insert("state".into(), state.as_str().into());
            }
            VarError::Malformed { .. } | VarError::Missing { .. } => {}
        }
        m
    }
}

impl std::fmt::Display for VarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VarError::Malformed { entry, reason } => {
                write!(f, "invalid --var format {:?}: {}", entry, reason)
            }
            VarError::Duplicate { var } => write!(f, "duplicate --var key {:?}", var),
            VarError::Unknown { var } => {
                write!(f, "unknown variable {:?}: not declared in template", var)
            }
            VarError::Missing { var } => write!(
                f,
                "missing required variable {:?}: provide --var {}=VALUE",
                var, var
            ),
            VarError::Invalid {
                var,
                value,
                constraint,
            } if constraint == ALLOWLIST_CONSTRAINT => write!(
                f,
                "variable {:?} value {:?}: contains characters not allowed by the value pattern {}",
                var, value, VALUE_PATTERN
            ),
            VarError::Invalid {
                var,
                value,
                constraint,
            } => write!(
                f,
                "variable {:?} value {:?}: does not satisfy {}",
                var, value, constraint
            ),
            VarError::Mismatch {
                var,
                recorded,
                requested,
            } => write!(
                f,
                "variable {:?} is fixed for this session: recorded {:?}, requested {:?}",
                var, recorded, requested
            ),
            VarError::TerminalSession { state } => write!(
                f,
                "session is in terminal state {:?}; its variables can no longer be rebound",
                state
            ),
        }
    }
}

impl std::error::Error for VarError {}

/// Check one resolved value against its declaration's constraint and the
/// allowlist. The declared constraint is checked first, so a value failing
/// both is reported against the constraint the author wrote.
pub fn check_value(var: &str, decl: &VariableDecl, value: &str) -> Result<(), VarError> {
    if !decl.satisfies_constraint(value) {
        return Err(VarError::Invalid {
            var: var.to_string(),
            value: value.to_string(),
            constraint: decl.constraint_label().unwrap_or_default(),
        });
    }
    if validate_value(var, value).is_err() {
        return Err(VarError::Invalid {
            var: var.to_string(),
            value: value.to_string(),
            constraint: ALLOWLIST_CONSTRAINT.to_string(),
        });
    }
    Ok(())
}

/// What an accepted rebind will change: each `rebind: true` variable whose
/// value from this invocation differs from the session's current binding.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RebindPlan {
    /// New values, keyed by variable. This is the `variables_rebound` payload.
    pub changes: BTreeMap<String, String>,
    /// The values those variables held before, for a caller that reports
    /// the change.
    pub previous: BTreeMap<String, String>,
}

impl RebindPlan {
    /// True when applying the plan would change nothing.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Validate one invocation's explicit variable pairs against a live session,
/// writing nothing.
///
/// `bindings` are the session's current bindings
/// ([`bindings_from_events`]) and `current_state` its current state. `pairs`
/// is a list rather than a map so a repeated key survives to be refused.
///
/// Refuses, in this order:
/// - a session whose current state is terminal ([`VarError::TerminalSession`]);
/// - a repeated key ([`VarError::Duplicate`]) or an undeclared one
///   ([`VarError::Unknown`]);
/// - a value failing its constraint or the allowlist ([`VarError::Invalid`]);
/// - an explicit value for a non-rebind variable that differs from the
///   recorded one ([`VarError::Mismatch`]). An equal value is accepted, and an
///   omitted non-rebind variable keeps its recorded value.
///
/// Every `rebind: true` variable then resolves from this invocation: its
/// explicit value, else its declared default. An omitted rebind variable
/// resets to its default rather than keeping an earlier run's value -- a
/// setting like `MERGE` is never inherited from another invocation.
pub fn validate_rebind(
    template: &CompiledTemplate,
    bindings: &HashMap<String, String>,
    current_state: &str,
    pairs: &[(String, String)],
) -> Result<RebindPlan, VarError> {
    if template
        .states
        .get(current_state)
        .map(|s| s.terminal)
        .unwrap_or(false)
    {
        return Err(VarError::TerminalSession {
            state: current_state.to_string(),
        });
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut explicit: HashMap<&str, &str> = HashMap::new();
    for (key, value) in pairs {
        if !seen.insert(key.as_str()) {
            return Err(VarError::Duplicate { var: key.clone() });
        }
        let Some(decl) = template.variables.get(key) else {
            return Err(VarError::Unknown { var: key.clone() });
        };
        check_value(key, decl, value)?;
        explicit.insert(key.as_str(), value.as_str());
    }

    // Non-rebind variables: an explicit value must equal the recorded one.
    for (key, requested) in &explicit {
        let decl = &template.variables[*key];
        if decl.rebind {
            continue;
        }
        let recorded = bindings.get(*key).map(String::as_str).unwrap_or("");
        if recorded != *requested {
            return Err(VarError::Mismatch {
                var: key.to_string(),
                recorded: recorded.to_string(),
                requested: requested.to_string(),
            });
        }
    }

    let mut plan = RebindPlan::default();
    for (key, decl) in &template.variables {
        if !decl.rebind {
            continue;
        }
        let value = match explicit.get(key.as_str()) {
            Some(v) => v.to_string(),
            None if !decl.default.is_empty() => decl.default.clone(),
            None if decl.required => return Err(VarError::Missing { var: key.clone() }),
            None => String::new(),
        };
        check_value(key, decl, &value)?;
        let current = bindings.get(key).map(String::as_str).unwrap_or("");
        if current != value {
            plan.previous.insert(key.clone(), current.to_string());
            plan.changes.insert(key.clone(), value);
        }
    }
    Ok(plan)
}

/// [`validate_rebind`] over a session's event log: derives the bindings and
/// the current state the way a tick does.
pub fn validate_rebind_from_events(
    template: &CompiledTemplate,
    events: &[Event],
    pairs: &[(String, String)],
) -> Result<RebindPlan, VarError> {
    let bindings = bindings_from_events(events);
    let state = derive_state_from_log(events).unwrap_or_default();
    validate_rebind(template, &bindings, &state, pairs)
}

/// Apply a validated plan: append exactly one `variables_rebound` event
/// carrying the changed values, or nothing when the plan is empty.
///
/// Returns whether an event was appended.
pub fn apply_rebind(
    backend: &dyn SessionBackend,
    session: &str,
    plan: &RebindPlan,
) -> anyhow::Result<bool> {
    if plan.is_empty() {
        return Ok(false);
    }
    backend.append_event(
        session,
        &EventPayload::VariablesRebound {
            variables: plan.changes.clone(),
        },
        &now_iso8601(),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::local::LocalBackend;
    use crate::session::state_file_name;
    use tempfile::TempDir;

    /// A template with a live `work` state and a terminal `done` state, a
    /// rebindable `MERGE` (`values: [true, false]`, default `false`), and a
    /// fixed `TOPIC`. Built from JSON, as the compile cache stores it, so the
    /// fixture keeps compiling as the template gains optional fields.
    fn template() -> CompiledTemplate {
        serde_json::from_value(serde_json::json!({
            "format_version": 1,
            "name": "t",
            "version": "1",
            "initial_state": "work",
            "variables": {
                "MERGE": {"default": "false", "values": ["true", "false"], "rebind": true},
                "TOPIC": {}
            },
            "states": {
                "work": {"directive": "work", "transitions": [{"target": "done"}]},
                "done": {"directive": "done", "terminal": true}
            }
        }))
        .unwrap()
    }

    fn bindings(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_explicit_rebind_value_that_differs_is_reported() {
        let plan = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "false"), ("TOPIC", "x")]),
            "work",
            &pairs(&[("MERGE", "true")]),
        )
        .unwrap();
        assert_eq!(plan.changes.get("MERGE").map(String::as_str), Some("true"));
        assert_eq!(
            plan.previous.get("MERGE").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn an_omitted_rebind_variable_resets_to_its_default() {
        // Never inherited: a session initialized with MERGE=true and attached
        // by an invocation that doesn't pass MERGE goes back to false.
        let plan = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "true"), ("TOPIC", "x")]),
            "work",
            &[],
        )
        .unwrap();
        assert_eq!(plan.changes.get("MERGE").map(String::as_str), Some("false"));
    }

    #[test]
    fn an_unchanged_rebind_value_plans_nothing() {
        let plan = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "false"), ("TOPIC", "x")]),
            "work",
            &pairs(&[("MERGE", "false"), ("TOPIC", "x")]),
        )
        .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn a_differing_non_rebind_value_is_a_mismatch() {
        let err = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "false"), ("TOPIC", "x")]),
            "work",
            &pairs(&[("TOPIC", "y")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            VarError::Mismatch {
                var: "TOPIC".into(),
                recorded: "x".into(),
                requested: "y".into(),
            }
        );
        assert_eq!(err.code(), Some("var_mismatch"));
        let fields = err.fields();
        assert_eq!(fields["recorded"], "x");
        assert_eq!(fields["requested"], "y");
    }

    #[test]
    fn an_omitted_non_rebind_variable_keeps_its_value_and_is_no_mismatch() {
        let plan = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "false"), ("TOPIC", "x")]),
            "work",
            &pairs(&[("MERGE", "true")]),
        )
        .unwrap();
        assert!(!plan.changes.contains_key("TOPIC"));
    }

    #[test]
    fn refusals_carry_their_codes() {
        let t = template();
        let b = bindings(&[("MERGE", "false"), ("TOPIC", "x")]);
        let cases: Vec<(Vec<(String, String)>, &str)> = vec![
            (pairs(&[("MERGE", "maybe")]), "invalid_var"),
            (pairs(&[("TOPIC", "a;b")]), "invalid_var"),
            (pairs(&[("NOPE", "1")]), "unknown_var"),
            (
                pairs(&[("MERGE", "true"), ("MERGE", "true")]),
                "duplicate_var",
            ),
        ];
        for (p, code) in cases {
            let err = validate_rebind(&t, &b, "work", &p).unwrap_err();
            assert_eq!(err.code(), Some(code), "pairs {:?}: {}", p, err);
        }
        let err = validate_rebind(&t, &b, "NOPE_STATE", &pairs(&[("TOPIC", "a;b")])).unwrap_err();
        assert_eq!(
            err.fields()["constraint"],
            ALLOWLIST_CONSTRAINT,
            "an unconstrained variable is refused by the allowlist"
        );
    }

    #[test]
    fn a_terminal_session_is_refused() {
        let err = validate_rebind(
            &template(),
            &bindings(&[("MERGE", "false")]),
            "done",
            &pairs(&[("MERGE", "true")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            VarError::TerminalSession {
                state: "done".into()
            }
        );
        assert_eq!(err.code(), Some("terminal_session"));
    }

    // -----------------------------------------------------------------
    // apply: exactly one event, or none
    // -----------------------------------------------------------------

    fn seeded_session(dir: &TempDir, merge: &str) -> (LocalBackend, std::path::PathBuf) {
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        backend.create("s").unwrap();
        // Built from JSON so the fixture keeps compiling as the header gains
        // optional fields.
        let header = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "workflow": "s",
            "template_hash": "h",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        let init = serde_json::json!({
            "seq": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "workflow_initialized",
            "payload": {"template_path": "t.json", "variables": {"MERGE": merge, "TOPIC": "x"}}
        });
        let entered = serde_json::json!({
            "seq": 2,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "transitioned",
            "payload": {"from": null, "to": "work", "condition_type": "auto"}
        });
        let events: Vec<Event> = vec![
            serde_json::from_value(init).unwrap(),
            serde_json::from_value(entered).unwrap(),
        ];
        backend.init_state_file("s", header, events).unwrap();
        let path = dir.path().join("s").join(state_file_name("s"));
        (backend, path)
    }

    #[test]
    fn apply_appends_one_event_the_fold_then_reads() {
        let dir = TempDir::new().unwrap();
        let (backend, _) = seeded_session(&dir, "false");
        let (_, events) = backend.read_events("s").unwrap();
        let plan = validate_rebind_from_events(&template(), &events, &pairs(&[("MERGE", "true")]))
            .unwrap();
        assert!(apply_rebind(&backend, "s", &plan).unwrap());

        let (_, after) = backend.read_events("s").unwrap();
        assert_eq!(after.len(), events.len() + 1);
        let last = after.last().unwrap();
        assert_eq!(last.event_type, "variables_rebound");
        assert_eq!(
            bindings_from_events(&after)
                .get("MERGE")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn apply_of_an_empty_plan_appends_nothing() {
        let dir = TempDir::new().unwrap();
        let (backend, path) = seeded_session(&dir, "false");
        let before = std::fs::read(&path).unwrap();
        let (_, events) = backend.read_events("s").unwrap();
        let plan = validate_rebind_from_events(&template(), &events, &[]).unwrap();
        assert!(!apply_rebind(&backend, "s", &plan).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_refused_validate_leaves_the_log_byte_identical() {
        let dir = TempDir::new().unwrap();
        let (backend, path) = seeded_session(&dir, "false");
        let before = std::fs::read(&path).unwrap();
        let (_, events) = backend.read_events("s").unwrap();
        for p in [
            pairs(&[("TOPIC", "y"), ("MERGE", "true")]),
            pairs(&[("MERGE", "maybe")]),
            pairs(&[("NOPE", "1")]),
            pairs(&[("MERGE", "true"), ("MERGE", "true")]),
        ] {
            assert!(validate_rebind_from_events(&template(), &events, &p).is_err());
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
