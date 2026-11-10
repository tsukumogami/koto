//! The decider, the agent, the run mode and the authority (PRD R7 to R11,
//! R13, R47; DESIGN-decision-record.md, Decision 2).
//!
//! Everything here comes from koto's own variables (`CLAUDE_CODE_SESSION_ID`
//! through the run journal's driver rule, `KOTO_ANCHOR`, `KOTO_AGENT_ID`,
//! `KOTO_RUN_MODE`), koto's own keys (the anchor's authority record, the
//! dispatched-by links) and the caller's flags. No workspace-manager file is
//! read. Every identity value carries a source label: `observed` when koto
//! read it itself, `handed` when a caller supplied it. Neither proves
//! identity.

use serde::{Deserialize, Serialize};

use super::{codes, DecisionError};
use crate::session::SessionBackend;

/// The variable koto-skills' hook sets to the acting agent's identifier.
pub const AGENT_ENV: &str = "KOTO_AGENT_ID";

/// The variable koto-skills' hook sets to the run mode the harness reports.
pub const RUN_MODE_ENV: &str = "KOTO_RUN_MODE";

/// The run modes R10 allows.
pub const RUN_MODES: [&str; 3] = ["attended", "unattended", "unknown"];

/// The surfaces a person decision can be entered through.
pub const SURFACES: [&str; 3] = ["terminal", "forge", "relay"];

/// The longest handle or check name stored, in bytes.
pub const MAX_HANDLE_BYTES: usize = 256;

/// Where an identity value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Observed,
    Handed,
}

/// A value and its source label (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sourced {
    pub value: String,
    pub source: Source,
}

impl Sourced {
    pub fn observed(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            source: Source::Observed,
        }
    }

    pub fn handed(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            source: Source::Handed,
        }
    }
}

/// The deciding session as koto observes it: its name, the harness session
/// identifier (empty unless id-shaped, R47) and the anchor (empty unless
/// `KOTO_ANCHOR` names a readable one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entering {
    pub session: Sourced,
    pub harness: Sourced,
    pub anchor: Sourced,
}

/// A person named on a `relay` without a link that qualifies: kept as a
/// handed note while the entering session is the decider (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonNote {
    pub handle: Sourced,
    pub surface: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Who decided (R7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Decider {
    Session {
        session: Sourced,
        harness: Sourced,
        anchor: Sourced,
        agent: Sourced,
        /// Whether koto filled the decider in for a gate pass nobody named
        /// one for (R24). `koto decisions record` never defaults.
        defaulted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<PersonNote>,
    },
    Person {
        handle: Sourced,
        surface: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
        entering: Entering,
    },
    Check {
        name: Sourced,
        /// The session that ran the check.
        session: Sourced,
    },
}

impl Decider {
    /// `session`, `person` or `check`.
    pub fn kind(&self) -> &'static str {
        match self {
            Decider::Session { .. } => "session",
            Decider::Person { .. } => "person",
            Decider::Check { .. } => "check",
        }
    }

    pub fn defaulted(&self) -> bool {
        matches!(
            self,
            Decider::Session {
                defaulted: true,
                ..
            }
        )
    }
}

/// The authority the decider held (R11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authority {
    /// `dispatch` for a session or check decider, `person` for a person.
    pub kind: String,
    /// Copied from the anchor's authority record; `null` when it records
    /// none. The record keeps no level today, so this is `null` until it
    /// does.
    pub level: Option<String>,
    /// The authority names the anchor's record holds, copied.
    pub grants: Vec<String>,
    /// Always `handed`: the anchor's record holds what its start was given.
    pub source: Source,
    /// The session the deciding session's nearest dispatched-by link names,
    /// or `none`.
    pub dispatcher: String,
    pub run_mode: Sourced,
}

/// The caller's decider flags.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub by: Option<String>,
    pub surface: Option<String>,
    pub link: Option<String>,
    pub agent: Option<String>,
    pub run_mode: Option<String>,
}

/// What koto read from its own environment, gathered once so tests can
/// supply it.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// `CLAUDE_CODE_SESSION_ID` when id-shaped by the run journal's driver
    /// rule (R47), else `None`.
    pub harness: Option<String>,
    /// `KOTO_ANCHOR` when it names a readable anchor in the store.
    pub anchor: Option<String>,
    /// `KOTO_AGENT_ID` when it matches koto's identifier pattern.
    pub agent: Option<String>,
    /// `KOTO_RUN_MODE` when it is one of [`RUN_MODES`].
    pub run_mode: Option<String>,
}

impl Observed {
    /// Read the environment. The anchor counts only when the store holds a
    /// readable anchor of that name.
    pub fn from_env(store: &dyn SessionBackend) -> Self {
        let anchor = crate::session::anchor::anchor_from_env()
            .filter(|a| crate::session::anchor::is_anchor(store, a));
        Self {
            harness: crate::run_journal::driver::driver(),
            anchor,
            agent: std::env::var(AGENT_ENV)
                .ok()
                .filter(|v| crate::run_journal::is_id(v)),
            run_mode: std::env::var(RUN_MODE_ENV)
                .ok()
                .filter(|v| RUN_MODES.contains(&v.as_str())),
        }
    }
}

/// Resolve the decider for a decision recorded in `session`. Refuses with
/// `decider_missing` when nothing establishes one, and with
/// `invalid_decision` naming the flag when a flag is malformed.
pub fn resolve(
    session: &str,
    flags: &Flags,
    observed: &Observed,
) -> Result<Decider, DecisionError> {
    let entering = Entering {
        session: Sourced::observed(session),
        harness: Sourced::observed(observed.harness.clone().unwrap_or_default()),
        anchor: Sourced::observed(observed.anchor.clone().unwrap_or_default()),
    };
    let agent = match &flags.agent {
        Some(a) if crate::run_journal::is_id(a) => Sourced::handed(a.clone()),
        Some(_) => {
            return Err(DecisionError::invalid(
                "agent",
                "--agent must be 1 to 128 characters from A-Z a-z 0-9 . _ : -",
            ))
        }
        None => match &observed.agent {
            Some(a) => Sourced::observed(a.clone()),
            None => Sourced::observed("unknown"),
        },
    };
    let session_decider = |note: Option<PersonNote>| Decider::Session {
        session: entering.session.clone(),
        harness: entering.harness.clone(),
        anchor: entering.anchor.clone(),
        agent: agent.clone(),
        defaulted: false,
        note,
    };

    if flags.by.is_none() && (flags.surface.is_some() || flags.link.is_some()) {
        return Err(DecisionError::invalid(
            "by",
            "--surface and --link describe a person decider and need --by person:<handle>",
        ));
    }
    let Some(by) = flags.by.as_deref() else {
        if observed.harness.is_none() && observed.anchor.is_none() {
            return Err(DecisionError::new(
                codes::DECIDER_MISSING,
                format!(
                    "no decider for a decision in '{session}': pass --by, or run it from an \
                     agent session (an id-shaped CLAUDE_CODE_SESSION_ID or a KOTO_ANCHOR naming \
                     a readable anchor)"
                ),
            ));
        }
        return Ok(session_decider(None));
    };

    if by == "session" {
        refuse_person_flags(flags)?;
        return Ok(session_decider(None));
    }
    if let Some(name) = by.strip_prefix("check:") {
        refuse_person_flags(flags)?;
        check_handle("by", name)?;
        return Ok(Decider::Check {
            name: Sourced::handed(name),
            session: Sourced::observed(session),
        });
    }
    let Some(handle) = by.strip_prefix("person:") else {
        return Err(DecisionError::invalid(
            "by",
            "--by takes session, person:<handle> or check:<name>",
        ));
    };
    check_handle("by", handle)?;
    let surface = flags.surface.clone().ok_or_else(|| {
        DecisionError::invalid(
            "surface",
            "a person decider needs --surface terminal|forge|relay",
        )
    })?;
    if !SURFACES.contains(&surface.as_str()) {
        return Err(DecisionError::invalid(
            "surface",
            "--surface takes terminal, forge or relay",
        ));
    }
    if let Some(link) = &flags.link {
        if link.len() > 2048 || link.chars().any(char::is_control) {
            return Err(DecisionError::invalid(
                "link",
                "--link is too long or holds control characters",
            ));
        }
    }
    if surface == "relay" && !flags.link.as_deref().is_some_and(is_relay_link) {
        // R9: without a qualifying link the entering session decided, and
        // the person it names is kept as a note.
        return Ok(session_decider(Some(PersonNote {
            handle: Sourced::handed(handle),
            surface,
            link: flags.link.clone(),
        })));
    }
    Ok(Decider::Person {
        handle: Sourced::handed(handle),
        surface,
        link: flags.link.clone(),
        entering,
    })
}

fn refuse_person_flags(flags: &Flags) -> Result<(), DecisionError> {
    if flags.surface.is_some() || flags.link.is_some() {
        return Err(DecisionError::invalid(
            "surface",
            "--surface and --link apply only to --by person:<handle>",
        ));
    }
    Ok(())
}

/// A handle or check name is stored as given, opaque, never looked up
/// (R8); it only has to be non-empty, bounded and free of control
/// characters.
fn check_handle(field: &str, value: &str) -> Result<(), DecisionError> {
    if value.trim().is_empty()
        || value.len() > MAX_HANDLE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(DecisionError::invalid(
            field,
            format!(
                "the name after '{}' must be 1 to {MAX_HANDLE_BYTES} bytes with no control characters",
                if field == "by" { "--by person:/check:" } else { field }
            ),
        ));
    }
    Ok(())
}

/// Whether `link` qualifies a `relay` decision as the person's (R9): an
/// absolute `https` URL with a host, no user information and no query.
pub fn is_relay_link(link: &str) -> bool {
    let Ok(url) = url::Url::parse(link) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
}

/// The run mode: `--run-mode` (handed), else `KOTO_RUN_MODE` (observed),
/// else `unknown` (R10).
pub fn run_mode(flags: &Flags, observed: &Observed) -> Result<Sourced, DecisionError> {
    match &flags.run_mode {
        Some(m) if RUN_MODES.contains(&m.as_str()) => Ok(Sourced::handed(m.clone())),
        Some(_) => Err(DecisionError::invalid(
            "run_mode",
            "--run-mode takes attended, unattended or unknown",
        )),
        None => Ok(match &observed.run_mode {
            Some(m) => Sourced::observed(m.clone()),
            None => Sourced::observed("unknown"),
        }),
    }
}

/// The authority (R11): `level` and `grants` copied from the authority
/// record of the deciding agent session's anchor, `dispatcher` from the
/// deciding session's nearest dispatched-by link.
pub fn authority(
    store: &dyn SessionBackend,
    decider: &Decider,
    anchor: Option<&str>,
    dispatcher: Option<String>,
    run_mode: Sourced,
) -> Authority {
    use crate::session::anchor::{read_authority, AuthorityRead};
    let grants = anchor
        .map(|a| read_authority(&store.session_dir(a)))
        .and_then(|read| match read {
            AuthorityRead::Record(r) => Some(r.names),
            _ => None,
        })
        .unwrap_or_default();
    Authority {
        kind: match decider {
            Decider::Person { .. } => "person",
            _ => "dispatch",
        }
        .to_string(),
        level: None,
        grants,
        source: Source::Handed,
        dispatcher: dispatcher.unwrap_or_else(|| "none".to_string()),
        run_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(harness: Option<&str>, anchor: Option<&str>) -> Observed {
        Observed {
            harness: harness.map(str::to_string),
            anchor: anchor.map(str::to_string),
            ..Observed::default()
        }
    }

    #[test]
    fn the_session_name_alone_establishes_no_decider() {
        let err = resolve("w", &Flags::default(), &observed(None, None)).unwrap_err();
        assert_eq!(err.code, codes::DECIDER_MISSING);
    }

    #[test]
    fn a_harness_id_or_an_anchor_establishes_the_session_decider() {
        let d = resolve(
            "w",
            &Flags::default(),
            &observed(Some("abc-1"), Some("anchor-1")),
        )
        .unwrap();
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["kind"], "session");
        assert_eq!(
            json["harness"],
            serde_json::json!({"value": "abc-1", "source": "observed"})
        );
        assert_eq!(
            json["anchor"],
            serde_json::json!({"value": "anchor-1", "source": "observed"})
        );
        assert_eq!(
            json["agent"],
            serde_json::json!({"value": "unknown", "source": "observed"})
        );
        assert_eq!(json["defaulted"], false);
    }

    #[test]
    fn a_person_on_a_terminal_is_handed_with_the_entering_session() {
        let flags = Flags {
            by: Some("person:dana".to_string()),
            surface: Some("terminal".to_string()),
            ..Flags::default()
        };
        let d = resolve("w", &flags, &observed(None, None)).unwrap();
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["kind"], "person");
        assert_eq!(
            json["handle"],
            serde_json::json!({"value": "dana", "source": "handed"})
        );
        assert_eq!(json["entering"]["session"]["value"], "w");
    }

    #[test]
    fn a_relay_without_a_qualifying_link_is_the_entering_sessions() {
        for link in [
            None,
            Some("http://x.example/1"),
            Some("https://u@x.example/1"),
            Some("https://x.example/1?q=1"),
            Some("/relative"),
        ] {
            let flags = Flags {
                by: Some("person:dana".to_string()),
                surface: Some("relay".to_string()),
                link: link.map(str::to_string),
                ..Flags::default()
            };
            let d = resolve("w", &flags, &observed(None, None)).unwrap();
            assert_eq!(d.kind(), "session", "{link:?}");
        }
        let flags = Flags {
            by: Some("person:dana".to_string()),
            surface: Some("relay".to_string()),
            link: Some("https://chat.example/m/1".to_string()),
            ..Flags::default()
        };
        assert_eq!(
            resolve("w", &flags, &observed(None, None)).unwrap().kind(),
            "person"
        );
    }

    #[test]
    fn a_check_names_the_session_that_ran_it() {
        let flags = Flags {
            by: Some("check:grader".to_string()),
            ..Flags::default()
        };
        let json =
            serde_json::to_value(resolve("w", &flags, &observed(None, None)).unwrap()).unwrap();
        assert_eq!(json["kind"], "check");
        assert_eq!(json["name"]["value"], "grader");
        assert_eq!(json["session"]["value"], "w");
    }
}
