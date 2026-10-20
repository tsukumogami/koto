//! Who decided: the decider resolver every recording command shares.
//!
//! A decider is handed (`--by`) or observed. With `--by`, the caller's
//! decider is used as given and labelled `handed`. Without it, the decider
//! is the harness session koto observes in `CLAUDE_CODE_SESSION_ID`, marked
//! `defaulted` because nobody named it. With neither, there is no decider
//! and the resolver says so ([`DeciderError::Missing`], `decider_missing`);
//! the caller refuses rather than recording an anonymous decision.
//!
//! The agent within a session comes from `KOTO_AGENT_ID`, which the
//! koto-skills hook sets from the harness's own payload, or from `--agent`.
//! The variable's contract: its name is [`AGENT_ID_ENV`], its value must
//! satisfy [`is_identifier`] (or be `main`, which does), and any other
//! value is read as [`UNKNOWN`]. Nothing here assumes `main`: an agent no
//! source establishes is `unknown`.
//!
//! The run mode comes from `CLAUDE_CODE_SESSION_ATTENDED` when the harness
//! sets it, else from `--run-mode`, else it is `unknown`.

use crate::host_env::{host_env, CLAUDE_SESSION_ID_ENV};

use super::record::{DecidedBy, FieldError, Slot, Source, Sourced};

/// The variable the koto-skills hook sets to the acting agent's identifier.
pub const AGENT_ID_ENV: &str = "KOTO_AGENT_ID";

/// The variable the agent harness sets to say whether its session is
/// attended (`1`/`true`) or not (`0`/`false`).
pub const SESSION_ATTENDED_ENV: &str = "CLAUDE_CODE_SESSION_ATTENDED";

/// The value recorded when no source establishes an agent or a run mode.
pub const UNKNOWN: &str = "unknown";

/// The agent value for the main agent of a session.
pub const MAIN_AGENT: &str = "main";

/// The longest identifier koto accepts for a session, agent, dispatch or
/// parent.
pub const MAX_IDENTIFIER_LEN: usize = 128;

/// The longest person handle or check name koto accepts.
pub const MAX_NAME_LEN: usize = 256;

/// The run modes a caller may hand.
pub const RUN_MODES: [&str; 2] = ["attended", "unattended"];

/// The surfaces a person's decision can come through.
pub const SURFACES: [&str; 3] = ["terminal", "forge", "relay"];

/// Whether `value` is an identifier koto will record or build a path from:
/// 1 to [`MAX_IDENTIFIER_LEN`] characters from letters, digits, `.`, `_`
/// and `-`, not starting with `.` and not containing `..`. A `/` can't
/// appear, so a checked value never leaves the directory it names a file
/// in.
pub fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_LEN
        && !value.starts_with('.')
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `value` is a handle or check name koto will store: non-empty,
/// at most [`MAX_NAME_LEN`] bytes, no control characters. koto doesn't
/// otherwise validate or resolve it.
pub fn is_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_NAME_LEN && !value.chars().any(char::is_control)
}

/// What koto observed in its own environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// The harness session, when set and identifier-shaped.
    pub session: Option<String>,
    /// The acting agent from [`AGENT_ID_ENV`]: `Some` only when set and
    /// identifier-shaped. A set but malformed value is `None`, which the
    /// agent slot records as `unknown`.
    pub agent: Option<String>,
    /// `attended` or `unattended`, from [`SESSION_ATTENDED_ENV`].
    pub run_mode: Option<String>,
}

impl Observed {
    /// Read the host's variables through the shared [`host_env`] seam.
    pub fn from_host() -> Self {
        let session = host_env(CLAUDE_SESSION_ID_ENV).filter(|v| is_identifier(v));
        let agent = host_env(AGENT_ID_ENV).filter(|v| is_identifier(v));
        let run_mode = host_env(SESSION_ATTENDED_ENV).and_then(|v| parse_attended(&v));
        Self {
            session,
            agent,
            run_mode,
        }
    }
}

/// The run mode a harness attended flag names.
fn parse_attended(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some("attended".to_string()),
        "0" | "false" => Some("unattended".to_string()),
        _ => None,
    }
}

/// A decider the caller named with `--by`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandedDecider {
    /// `--by session`: the observed session, named rather than defaulted.
    ObservedSession,
    /// `--by session:<id>`: a session the caller names.
    Session(String),
    /// `--by person:<handle>`.
    Person(String),
    /// `--by check:<name>`.
    Check(String),
}

/// Parse a `--by` value: `session`, `session:<id>`, `person:<handle>` or
/// `check:<name>`.
pub fn parse_by(value: &str) -> Result<HandedDecider, FieldError> {
    let bad = |why: &str| {
        FieldError::new(
            "by",
            format!(
                "'{}' {} (use session, session:<id>, person:<handle> or check:<name>)",
                value, why
            ),
        )
    };
    if value == "session" {
        return Ok(HandedDecider::ObservedSession);
    }
    let Some((kind, rest)) = value.split_once(':') else {
        return Err(bad("is not a decider"));
    };
    match kind {
        "session" if is_identifier(rest) => Ok(HandedDecider::Session(rest.to_string())),
        "session" => Err(bad("does not name a session identifier")),
        "person" if is_name(rest) => Ok(HandedDecider::Person(rest.to_string())),
        "person" => Err(bad("does not name a person")),
        "check" if is_name(rest) => Ok(HandedDecider::Check(rest.to_string())),
        "check" => Err(bad("does not name a check")),
        _ => Err(bad("is not a decider")),
    }
}

/// Everything a caller hands the decider resolver.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeciderInput {
    pub by: Option<HandedDecider>,
    /// `--surface`, for a person.
    pub surface: Option<String>,
    /// `--link`, for a person: an already-validated https URL.
    pub link: Option<String>,
    /// `--agent`, for a session.
    pub agent: Option<String>,
}

/// Why no decider resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeciderError {
    /// Nothing was handed and no harness session was observed.
    Missing,
    /// A handed flag is malformed or doesn't fit the decider.
    Invalid(FieldError),
}

impl DeciderError {
    /// The machine-readable reason for [`DeciderError::Missing`].
    pub const MISSING_REASON: &'static str = "decider_missing";
}

impl std::fmt::Display for DeciderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeciderError::Missing => write!(
                f,
                "{}: no decider was named with --by and no harness session was observed",
                Self::MISSING_REASON
            ),
            DeciderError::Invalid(e) => write!(f, "{}", e),
        }
    }
}

/// One value per source, both kept when they differ: the observed value
/// first, then a handed one that disagrees with it.
pub fn merge_slot(observed: Option<&str>, handed: Option<&str>) -> Slot {
    let mut slot = Vec::new();
    if let Some(o) = observed {
        slot.push(Sourced::observed(o));
    }
    if let Some(h) = handed {
        if observed != Some(h) {
            slot.push(Sourced::handed(h));
        }
    }
    slot
}

/// A slot that records `unknown` when no source gives a value.
fn or_unknown(slot: Slot) -> Slot {
    if slot.is_empty() {
        vec![Sourced {
            value: UNKNOWN.to_string(),
            source: Source::Observed,
        }]
    } else {
        slot
    }
}

/// The agent within a session: the observed [`AGENT_ID_ENV`], the handed
/// `--agent`, both when they differ, `unknown` when neither.
pub fn agent_slot(observed: &Observed, handed: Option<&str>) -> Slot {
    or_unknown(merge_slot(observed.agent.as_deref(), handed))
}

/// The run mode: observed from the harness, handed with `--run-mode`, both
/// when they differ, `unknown` when neither.
pub fn run_mode_slot(observed: &Observed, handed: Option<&str>) -> Slot {
    or_unknown(merge_slot(observed.run_mode.as_deref(), handed))
}

/// Check a handed `--agent` value.
pub fn validate_agent(value: &str) -> Result<(), FieldError> {
    if is_identifier(value) {
        Ok(())
    } else {
        Err(FieldError::new(
            "agent",
            format!(
                "'{}' is not an agent identifier (letters, digits, '.', '_', '-', at most {} characters)",
                value, MAX_IDENTIFIER_LEN
            ),
        ))
    }
}

/// Check a handed `--run-mode` value.
pub fn validate_run_mode(value: &str) -> Result<(), FieldError> {
    if RUN_MODES.contains(&value) {
        Ok(())
    } else {
        Err(FieldError::new(
            "run_mode",
            format!(
                "'{}' is not a run mode (allowed: {})",
                value,
                RUN_MODES.join(", ")
            ),
        ))
    }
}

/// Resolve the decider before anything is written.
///
/// - `--by session` names the observed session (not defaulted); with no
///   observed session it is [`DeciderError::Missing`].
/// - `--by session:<id>` records the handed id, and the observed session
///   beside it when that differs.
/// - `--by person:<handle>` needs `--surface`. With surface `relay` and no
///   `--link`, the decider is the entering (observed) session and the person
///   is kept as a handed note in `relayed_person`.
/// - `--by check:<name>` records the check and the session that ran it.
/// - No `--by`: the observed session, `defaulted`; else `Missing`.
///
/// `--surface` and `--link` belong to a person and are refused on any other
/// decider.
pub fn resolve_decider(
    input: &DeciderInput,
    observed: &Observed,
) -> Result<DecidedBy, DeciderError> {
    let invalid = |field: &str, msg: String| DeciderError::Invalid(FieldError::new(field, msg));
    let is_person = matches!(input.by, Some(HandedDecider::Person(_)));
    if !is_person {
        if input.surface.is_some() {
            return Err(invalid(
                "surface",
                "--surface applies to a person decider (--by person:<handle>)".to_string(),
            ));
        }
        if input.link.is_some() {
            return Err(invalid(
                "link",
                "--link applies to a person decider (--by person:<handle>)".to_string(),
            ));
        }
    }
    if let Some(a) = &input.agent {
        validate_agent(a).map_err(DeciderError::Invalid)?;
    }
    let observed_session = observed.session.as_deref();
    let observed_slot = || merge_slot(observed_session, None);
    let agent = || agent_slot(observed, input.agent.as_deref());

    match &input.by {
        None => match observed_session {
            Some(_) => Ok(DecidedBy::session(observed_slot(), agent(), true)),
            None => Err(DeciderError::Missing),
        },
        Some(HandedDecider::ObservedSession) => match observed_session {
            Some(_) => Ok(DecidedBy::session(observed_slot(), agent(), false)),
            None => Err(DeciderError::Missing),
        },
        Some(HandedDecider::Session(id)) => Ok(DecidedBy::session(
            merge_slot(observed_session, Some(id)),
            agent(),
            false,
        )),
        Some(HandedDecider::Check(name)) => Ok(DecidedBy::check(name.clone(), observed_slot())),
        Some(HandedDecider::Person(handle)) => {
            let Some(surface) = input.surface.clone() else {
                return Err(invalid(
                    "surface",
                    format!(
                        "a person decider needs --surface (one of: {})",
                        SURFACES.join(", ")
                    ),
                ));
            };
            if !SURFACES.contains(&surface.as_str()) {
                return Err(invalid(
                    "surface",
                    format!(
                        "'{}' is not a surface (allowed: {})",
                        surface,
                        SURFACES.join(", ")
                    ),
                ));
            }
            if surface == "relay" && input.link.is_none() {
                // A relayed decision with nothing to follow back to the
                // person is the relaying session's own decision.
                if observed_session.is_none() {
                    return Err(DeciderError::Missing);
                }
                let mut d = DecidedBy::session(observed_slot(), agent(), false);
                d.surface = Some(surface);
                d.relayed_person = vec![Sourced::handed(handle.clone())];
                return Ok(d);
            }
            Ok(DecidedBy::person(
                vec![Sourced::handed(handle.clone())],
                surface,
                input.link.clone(),
                observed_slot(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_store::record::DeciderKind;
    use crate::host_env::set_host_env_for_test;

    fn observed(session: Option<&str>, agent: Option<&str>) -> Observed {
        Observed {
            session: session.map(str::to_string),
            agent: agent.map(str::to_string),
            run_mode: None,
        }
    }

    fn values(slot: &Slot) -> Vec<(String, Source)> {
        slot.iter().map(|s| (s.value.clone(), s.source)).collect()
    }

    #[test]
    fn identifiers_reject_slashes_dots_and_overlong_values() {
        assert!(is_identifier("9c1e-ab_12.x"));
        assert!(is_identifier("main"));
        for bad in ["", "a/b", "..", "a..b", ".hidden", "a b", "a:b"] {
            assert!(!is_identifier(bad), "{:?} passed", bad);
        }
        assert!(is_identifier(&"a".repeat(128)));
        assert!(!is_identifier(&"a".repeat(129)));
    }

    #[test]
    fn host_values_are_read_through_the_seam_and_checked() {
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("sess-1"));
        set_host_env_for_test(AGENT_ID_ENV, Some("bad/agent"));
        set_host_env_for_test(SESSION_ATTENDED_ENV, Some("0"));
        let o = Observed::from_host();
        assert_eq!(o.session.as_deref(), Some("sess-1"));
        assert_eq!(o.agent, None, "a malformed agent id is not observed");
        assert_eq!(o.run_mode.as_deref(), Some("unattended"));
        set_host_env_for_test(AGENT_ID_ENV, Some("main"));
        set_host_env_for_test(SESSION_ATTENDED_ENV, Some("true"));
        let o = Observed::from_host();
        assert_eq!(o.agent.as_deref(), Some("main"));
        assert_eq!(o.run_mode.as_deref(), Some("attended"));
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("../etc"));
        set_host_env_for_test(SESSION_ATTENDED_ENV, Some("maybe"));
        let o = Observed::from_host();
        assert_eq!(o.session, None);
        assert_eq!(o.run_mode, None);
        for n in [CLAUDE_SESSION_ID_ENV, AGENT_ID_ENV, SESSION_ATTENDED_ENV] {
            set_host_env_for_test(n, None);
        }
    }

    #[test]
    fn no_by_defaults_to_the_observed_session() {
        let d = resolve_decider(&DeciderInput::default(), &observed(Some("s1"), None)).unwrap();
        assert_eq!(d.kind, DeciderKind::Session);
        assert!(d.defaulted);
        assert_eq!(values(&d.session), vec![("s1".into(), Source::Observed)]);
        assert_eq!(values(&d.agent), vec![("unknown".into(), Source::Observed)]);
    }

    #[test]
    fn neither_by_nor_a_session_is_decider_missing() {
        let e = resolve_decider(&DeciderInput::default(), &observed(None, None)).unwrap_err();
        assert_eq!(e, DeciderError::Missing);
        assert!(e.to_string().starts_with("decider_missing"));
        let by_session = DeciderInput {
            by: Some(HandedDecider::ObservedSession),
            ..Default::default()
        };
        assert_eq!(
            resolve_decider(&by_session, &observed(None, None)).unwrap_err(),
            DeciderError::Missing
        );
    }

    #[test]
    fn a_handed_session_is_not_defaulted_and_keeps_a_differing_observed_one() {
        let input = DeciderInput {
            by: Some(HandedDecider::Session("s2".into())),
            ..Default::default()
        };
        let d = resolve_decider(&input, &observed(Some("s1"), None)).unwrap();
        assert!(!d.defaulted);
        assert_eq!(
            values(&d.session),
            vec![
                ("s1".into(), Source::Observed),
                ("s2".into(), Source::Handed)
            ]
        );
        let d = resolve_decider(&input, &observed(Some("s2"), None)).unwrap();
        assert_eq!(values(&d.session), vec![("s2".into(), Source::Observed)]);
        let d = resolve_decider(&input, &observed(None, None)).unwrap();
        assert_eq!(values(&d.session), vec![("s2".into(), Source::Handed)]);
    }

    #[test]
    fn the_agent_is_observed_handed_both_or_unknown() {
        let base = DeciderInput::default();
        let d = resolve_decider(&base, &observed(Some("s"), Some("main"))).unwrap();
        assert_eq!(values(&d.agent), vec![("main".into(), Source::Observed)]);
        let handed = DeciderInput {
            agent: Some("a-7".into()),
            ..Default::default()
        };
        let d = resolve_decider(&handed, &observed(Some("s"), None)).unwrap();
        assert_eq!(values(&d.agent), vec![("a-7".into(), Source::Handed)]);
        let d = resolve_decider(&handed, &observed(Some("s"), Some("main"))).unwrap();
        assert_eq!(
            values(&d.agent),
            vec![
                ("main".into(), Source::Observed),
                ("a-7".into(), Source::Handed)
            ]
        );
        let bad = DeciderInput {
            agent: Some("a/7".into()),
            ..Default::default()
        };
        match resolve_decider(&bad, &observed(Some("s"), None)).unwrap_err() {
            DeciderError::Invalid(e) => assert_eq!(e.field, "agent"),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn a_person_needs_a_surface_and_records_the_entering_session() {
        let mut input = DeciderInput {
            by: Some(HandedDecider::Person("dana".into())),
            ..Default::default()
        };
        match resolve_decider(&input, &observed(Some("s"), None)).unwrap_err() {
            DeciderError::Invalid(e) => assert_eq!(e.field, "surface"),
            other => panic!("{:?}", other),
        }
        input.surface = Some("terminal".into());
        let d = resolve_decider(&input, &observed(Some("s"), None)).unwrap();
        assert_eq!(d.kind, DeciderKind::Person);
        assert_eq!(values(&d.handle), vec![("dana".into(), Source::Handed)]);
        assert_eq!(d.surface.as_deref(), Some("terminal"));
        assert_eq!(values(&d.entered_by), vec![("s".into(), Source::Observed)]);
        assert!(!d.defaulted);
        input.surface = Some("email".into());
        assert!(resolve_decider(&input, &observed(Some("s"), None)).is_err());
    }

    #[test]
    fn a_relay_without_a_link_is_the_relaying_sessions_decision() {
        let mut input = DeciderInput {
            by: Some(HandedDecider::Person("dana".into())),
            surface: Some("relay".into()),
            ..Default::default()
        };
        let d = resolve_decider(&input, &observed(Some("coord"), None)).unwrap();
        assert_eq!(d.kind, DeciderKind::Session);
        assert_eq!(values(&d.session), vec![("coord".into(), Source::Observed)]);
        assert_eq!(
            values(&d.relayed_person),
            vec![("dana".into(), Source::Handed)]
        );
        assert!(d.handle.is_empty());
        input.link = Some("https://example.com/o/r/pull/1#pullrequestreview-9".into());
        let d = resolve_decider(&input, &observed(Some("coord"), None)).unwrap();
        assert_eq!(d.kind, DeciderKind::Person);
        assert_eq!(d.link, input.link);
        assert!(d.relayed_person.is_empty());
    }

    #[test]
    fn surface_and_link_are_refused_on_anything_but_a_person() {
        for (input, field) in [
            (
                DeciderInput {
                    link: Some("https://example.com/x".into()),
                    ..Default::default()
                },
                "link",
            ),
            (
                DeciderInput {
                    by: Some(HandedDecider::Check("grader".into())),
                    surface: Some("terminal".into()),
                    ..Default::default()
                },
                "surface",
            ),
        ] {
            match resolve_decider(&input, &observed(Some("s"), None)).unwrap_err() {
                DeciderError::Invalid(e) => assert_eq!(e.field, field),
                other => panic!("{:?}", other),
            }
        }
    }

    #[test]
    fn a_check_names_the_session_that_ran_it() {
        let input = DeciderInput {
            by: Some(HandedDecider::Check("koto-grader".into())),
            ..Default::default()
        };
        let d = resolve_decider(&input, &observed(Some("s"), None)).unwrap();
        assert_eq!(d.kind, DeciderKind::Check);
        assert_eq!(d.name.as_deref(), Some("koto-grader"));
        assert_eq!(values(&d.ran_in), vec![("s".into(), Source::Observed)]);
    }

    #[test]
    fn by_values_parse_or_name_the_field() {
        assert_eq!(parse_by("session"), Ok(HandedDecider::ObservedSession));
        assert_eq!(
            parse_by("session:abc"),
            Ok(HandedDecider::Session("abc".into()))
        );
        assert_eq!(
            parse_by("person:@dana"),
            Ok(HandedDecider::Person("@dana".into()))
        );
        assert_eq!(parse_by("check:g"), Ok(HandedDecider::Check("g".into())));
        for bad in ["", "robot", "session:../x", "person:", "check:\n", "team:x"] {
            assert_eq!(parse_by(bad).unwrap_err().field, "by", "{:?}", bad);
        }
    }

    #[test]
    fn run_mode_is_observed_handed_both_or_unknown() {
        let mut o = observed(None, None);
        assert_eq!(
            values(&run_mode_slot(&o, None)),
            vec![("unknown".into(), Source::Observed)]
        );
        assert_eq!(
            values(&run_mode_slot(&o, Some("unattended"))),
            vec![("unattended".into(), Source::Handed)]
        );
        o.run_mode = Some("attended".into());
        assert_eq!(
            values(&run_mode_slot(&o, Some("unattended"))),
            vec![
                ("attended".into(), Source::Observed),
                ("unattended".into(), Source::Handed)
            ]
        );
        assert!(validate_run_mode("unknown").is_err());
    }
}
