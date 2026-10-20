//! The decision line: its types and the validation every recording path
//! shares.
//!
//! One decision is one JSON object on one line. Every identity it names
//! (the session, the agent, a person's handle, each dispatch-chain link)
//! is a [`Slot`]: a list of values each carrying its [`Source`]. One entry
//! is the usual case; two mean an observed and a handed value disagreed;
//! an empty list means nothing was found. The resolvers that fill the
//! slots live elsewhere; this module only defines the shapes and checks
//! that a decision's kind, outcome and subject are ones the record allows.

use serde::{Deserialize, Serialize};

/// The line format version every decision carries as `v`.
pub const DECISION_FORMAT_VERSION: u32 = 1;

/// Where a recorded value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// koto read the value itself.
    Observed,
    /// A caller supplied the value.
    Handed,
}

/// One value of an identity, with where it came from.
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

/// An identity slot: every value found for one identity, each labelled.
pub type Slot = Vec<Sourced>;

/// The closed set of decision kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Accept,
    Merge,
    GatePass,
    Escalate,
    DeclineEscalation,
    Judgment,
    Verdict,
    Reverse,
    Refused,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Accept,
        Kind::Merge,
        Kind::GatePass,
        Kind::Escalate,
        Kind::DeclineEscalation,
        Kind::Judgment,
        Kind::Verdict,
        Kind::Reverse,
        Kind::Refused,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Accept => "accept",
            Kind::Merge => "merge",
            Kind::GatePass => "gate-pass",
            Kind::Escalate => "escalate",
            Kind::DeclineEscalation => "decline-escalation",
            Kind::Judgment => "judgment",
            Kind::Verdict => "verdict",
            Kind::Reverse => "reverse",
            Kind::Refused => "refused",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The outcomes this kind allows.
    pub fn outcomes(self) -> &'static [&'static str] {
        match self {
            Kind::Accept => &["accepted", "rejected"],
            Kind::Merge => &["merged"],
            Kind::GatePass => &["passed"],
            Kind::Escalate => &["escalated"],
            Kind::DeclineEscalation => &["kept"],
            Kind::Judgment => &["settled"],
            Kind::Verdict => &["pass", "fail"],
            Kind::Reverse => &["reversed"],
            Kind::Refused => &["refused"],
        }
    }

    /// Whether the report counts this kind as a decision. `reverse` and
    /// `refused` are not decisions in the report's tables.
    pub fn counts_as_decision(self) -> bool {
        !matches!(self, Kind::Reverse | Kind::Refused)
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The closed set of decider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeciderKind {
    Session,
    Person,
    Check,
}

impl DeciderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DeciderKind::Session => "session",
            DeciderKind::Person => "person",
            DeciderKind::Check => "check",
        }
    }

    pub fn parse(s: &str) -> Option<DeciderKind> {
        [
            DeciderKind::Session,
            DeciderKind::Person,
            DeciderKind::Check,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// Who decided. The fields beyond `kind` are filled per decider kind: a
/// `session` decider names its session and agent, a `person` its handle,
/// surface, optional link and the entering session, a `check` its name and
/// the session that ran it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedBy {
    pub kind: DeciderKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session: Slot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent: Slot,
    /// The decider was not named by the caller; koto used the session it
    /// observed.
    #[serde(default)]
    pub defaulted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handle: Slot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entered_by: Slot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ran_in: Slot,
    /// On a `session` decider that relayed what a person said with no link
    /// to the original: the person the relaying session named, kept as a
    /// handed note so an auditor can follow up. The person is not the
    /// decider; the relaying session is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relayed_person: Slot,
}

impl DecidedBy {
    /// A `session` decider with nothing resolved yet.
    pub fn session(session: Slot, agent: Slot, defaulted: bool) -> Self {
        Self {
            kind: DeciderKind::Session,
            session,
            agent,
            defaulted,
            handle: Vec::new(),
            surface: None,
            link: None,
            entered_by: Vec::new(),
            name: None,
            ran_in: Vec::new(),
            relayed_person: Vec::new(),
        }
    }

    /// A `person` decider: a handed handle, the surface it came through, an
    /// optional link to the original and the session that entered it.
    pub fn person(handle: Slot, surface: String, link: Option<String>, entered_by: Slot) -> Self {
        Self {
            kind: DeciderKind::Person,
            handle,
            surface: Some(surface),
            link,
            entered_by,
            ..Self::session(Vec::new(), Vec::new(), false)
        }
    }

    /// A `check` decider: the check's name and the session that ran it.
    pub fn check(name: String, ran_in: Slot) -> Self {
        Self {
            kind: DeciderKind::Check,
            name: Some(name),
            ran_in,
            ..Self::session(Vec::new(), Vec::new(), false)
        }
    }

    /// Every session value this decider names: its own session, the
    /// session that entered a person's decision, or the one that ran a
    /// check.
    pub fn sessions(&self) -> impl Iterator<Item = &Sourced> {
        self.session
            .iter()
            .chain(self.entered_by.iter())
            .chain(self.ran_in.iter())
    }
}

/// The authority the decider held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authority {
    /// `dispatch` for a session or check decider, `person` for a person.
    pub kind: String,
    #[serde(default)]
    pub dispatch_id: Slot,
    #[serde(default)]
    pub instructions_digest: Slot,
    #[serde(default)]
    pub run_mode: Slot,
    #[serde(default)]
    pub dispatcher: Slot,
    /// On reversals only: `verified` or `asserted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing: Option<String>,
}

impl Authority {
    /// The authority kind a decider of `kind` holds.
    pub fn for_decider(kind: DeciderKind) -> Self {
        Self {
            kind: match kind {
                DeciderKind::Person => "person",
                DeciderKind::Session | DeciderKind::Check => "dispatch",
            }
            .to_string(),
            dispatch_id: Vec::new(),
            instructions_digest: Vec::new(),
            run_mode: Vec::new(),
            dispatcher: Vec::new(),
            standing: None,
        }
    }
}

/// The dispatch chain from the deciding session up towards the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    #[serde(default)]
    pub links: Vec<Slot>,
    /// No source supplied the next link: the chain doesn't reach an
    /// affirmed root.
    pub incomplete: bool,
}

impl Chain {
    /// A chain no source supplied any link for.
    pub fn unknown() -> Self {
        Self {
            links: Vec::new(),
            incomplete: true,
        }
    }
}

/// What a decision is about. Which fields are required, and which are
/// allowed at all, depends on the kind; [`validate_subject`] checks that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    /// What an `accept` accepts (`document`, `plan`, `step`, `test-change`,
    /// `intent`), or what a `verdict` judges (`change`, `release`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    /// What a `judgment` or `verdict` judged, as free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// The decision a `reverse` overrules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// On a `refused` decision: the kind that was attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted_kind: Option<String>,
    /// On a `refused` decision: the subject that was attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted: Option<Box<Subject>>,
    /// On a `refused` decision: why koto refused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
}

impl Subject {
    /// The names of every field that is set, in declaration order.
    pub fn set_fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        macro_rules! opt {
            ($($f:ident),*) => {
                $( if self.$f.is_some() { out.push(stringify!($f)); } )*
            };
        }
        opt!(
            of,
            repo,
            path,
            commit,
            plan,
            step,
            intent_location,
            source_link,
            pr,
            workflow,
            workflow_session_id,
            state,
            gate,
            question_id,
            question,
            recipient,
            what,
            pointer,
            decision,
            attempted_kind,
            attempted,
            refusal_reason
        );
        if !self.paths.is_empty() {
            out.push("paths");
        }
        out
    }
}

/// What a reversal line copies from the decision it reverses, from the
/// same locked read that authorized it, so reversal status and the
/// report's reversal table never need the month files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Original {
    pub kind: Kind,
    pub decider_kind: DeciderKind,
    #[serde(default)]
    pub session: Slot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_key: Option<String>,
}

/// One decision line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub v: u32,
    pub id: String,
    /// UTC, to the second: `YYYY-MM-DDTHH:MM:SSZ`.
    pub time: String,
    pub kind: Kind,
    pub outcome: String,
    pub subject: Subject,
    pub decided_by: DecidedBy,
    pub authority: Authority,
    pub chain: Chain,
    #[serde(default)]
    pub basis: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// On a `reverse`: the identifier of the decision it overrules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverses: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<Original>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repaired_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    /// On the reversal of a merge or a release verdict: the compensating
    /// change or action, or that none exists yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensating: Option<Compensating>,
    /// On the reversal of a judgment or a verdict: the outcome that
    /// replaces the reversed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// What compensates for a reversed merge or release verdict: a revert pull
/// request or a rollback, by link, or a record that none exists yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compensating {
    /// Whether a compensating change or action exists.
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// A stable key for what a decision is about, copied onto a reversal line
/// so readers of the reversal log never need the month files.
///
/// It is a JSON array of strings, the kind first. A `gate-pass` key is
/// `["gate-pass", workflow, workflow_session_id, state, gate]`, with an
/// empty session id when the pass named none; [`gate_pass_key`] reads one
/// back.
pub fn subject_key(kind: Kind, s: &Subject) -> String {
    let f = |o: &Option<String>| o.clone().unwrap_or_default();
    let mut parts: Vec<String> = vec![kind.as_str().to_string()];
    match kind {
        Kind::GatePass => parts.extend([
            f(&s.workflow),
            f(&s.workflow_session_id),
            f(&s.state),
            f(&s.gate),
        ]),
        Kind::Merge => parts.extend([f(&s.repo), s.pr.map(|p| p.to_string()).unwrap_or_default()]),
        Kind::Accept => {
            parts.extend([
                f(&s.of),
                f(&s.repo),
                f(&s.path),
                f(&s.plan),
                f(&s.step),
                f(&s.intent_location),
            ]);
            parts.extend(s.paths.iter().cloned());
        }
        Kind::Escalate | Kind::DeclineEscalation => parts.push(f(&s.question_id)),
        Kind::Judgment | Kind::Verdict => parts.extend([
            f(&s.of),
            f(&s.what),
            f(&s.pointer),
            f(&s.repo),
            f(&s.commit),
        ]),
        Kind::Reverse => parts.push(f(&s.decision)),
        Kind::Refused => parts.push(f(&s.attempted_kind)),
    }
    serde_json::to_string(&parts).unwrap_or_default()
}

/// The parts of a `gate-pass` subject key: workflow, workflow session id,
/// state and gate. `None` when the key is not a gate pass's.
pub fn gate_pass_key(key: &str) -> Option<[String; 4]> {
    let parts: Vec<String> = serde_json::from_str(key).ok()?;
    match <[String; 5]>::try_from(parts) {
        Ok([kind, w, sid, state, gate]) if kind == Kind::GatePass.as_str() => {
            Some([w, sid, state, gate])
        }
        _ => None,
    }
}

/// A decision that fails validation, naming the field at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// What an `accept` may be of.
pub const ACCEPT_OF: [&str; 5] = ["document", "plan", "step", "test-change", "intent"];

/// What a `verdict` may be on, when it names one.
pub const VERDICT_OF: [&str; 2] = ["change", "release"];

/// Check that `outcome` is one `kind` allows.
pub fn validate_outcome(kind: Kind, outcome: &str) -> Result<(), FieldError> {
    if kind.outcomes().contains(&outcome) {
        Ok(())
    } else {
        Err(FieldError::new(
            "outcome",
            format!(
                "outcome '{}' is not allowed for kind '{}' (allowed: {})",
                outcome,
                kind,
                kind.outcomes().join(", ")
            ),
        ))
    }
}

fn require(present: bool, field: &str, kind: &str) -> Result<(), FieldError> {
    if present {
        Ok(())
    } else {
        Err(FieldError::new(
            field,
            format!("a {} decision requires its subject's '{}'", kind, field),
        ))
    }
}

/// Check the subject against what its kind requires and allows, and the
/// evidence a `verdict` needs. The first problem found is returned,
/// naming its field.
pub fn validate_subject(
    kind: Kind,
    subject: &Subject,
    evidence: &[String],
) -> Result<(), FieldError> {
    let s = subject;
    let present = |o: &Option<String>| o.as_deref().is_some_and(|v| !v.is_empty());
    let label: String;
    let allowed: &[&str] = match kind {
        Kind::Accept => {
            let Some(of) = s.of.as_deref() else {
                return Err(FieldError::new(
                    "of",
                    format!(
                        "an accept decision requires 'of', one of: {}",
                        ACCEPT_OF.join(", ")
                    ),
                ));
            };
            label = format!("accept of a {}", of);
            match of {
                "document" | "plan" => {
                    require(present(&s.repo), "repo", &label)?;
                    require(present(&s.path), "path", &label)?;
                    require(present(&s.commit), "commit", &label)?;
                    &["of", "repo", "path", "commit"]
                }
                "step" => {
                    require(present(&s.repo), "repo", &label)?;
                    require(present(&s.plan), "plan", &label)?;
                    require(present(&s.step), "step", &label)?;
                    require(present(&s.commit), "commit", &label)?;
                    &["of", "repo", "plan", "step", "commit"]
                }
                "test-change" => {
                    require(present(&s.repo), "repo", &label)?;
                    require(!s.paths.is_empty(), "paths", &label)?;
                    require(present(&s.commit), "commit", &label)?;
                    &["of", "repo", "paths", "commit"]
                }
                "intent" => {
                    require(present(&s.intent_location), "intent_location", &label)?;
                    &["of", "intent_location", "source_link"]
                }
                other => {
                    return Err(FieldError::new(
                        "of",
                        format!(
                            "'{}' is not something an accept can be of (allowed: {})",
                            other,
                            ACCEPT_OF.join(", ")
                        ),
                    ))
                }
            }
        }
        Kind::Merge => {
            require(present(&s.repo), "repo", "merge")?;
            require(s.pr.is_some(), "pr", "merge")?;
            require(present(&s.commit), "commit", "merge")?;
            &["repo", "pr", "commit"]
        }
        Kind::GatePass => {
            require(present(&s.workflow), "workflow", "gate-pass")?;
            require(present(&s.state), "state", "gate-pass")?;
            require(present(&s.gate), "gate", "gate-pass")?;
            &["workflow", "workflow_session_id", "state", "gate"]
        }
        Kind::Escalate => {
            require(present(&s.question_id), "question_id", "escalate")?;
            require(present(&s.question), "question", "escalate")?;
            require(present(&s.recipient), "recipient", "escalate")?;
            &["question_id", "question", "recipient"]
        }
        Kind::DeclineEscalation => {
            require(present(&s.question_id), "question_id", "decline-escalation")?;
            require(present(&s.question), "question", "decline-escalation")?;
            &["question_id", "question"]
        }
        Kind::Judgment => {
            require(present(&s.what), "what", "judgment")?;
            &["what", "pointer"]
        }
        Kind::Verdict => {
            require(present(&s.what), "what", "verdict")?;
            if evidence.is_empty() {
                return Err(FieldError::new(
                    "evidence",
                    "a verdict decision requires at least one evidence pointer",
                ));
            }
            match s.of.as_deref() {
                None => &["what", "pointer"],
                Some(of @ ("change" | "release")) => {
                    let label = format!("verdict on a {}", of);
                    require(present(&s.repo), "repo", &label)?;
                    require(present(&s.commit), "commit", &label)?;
                    &["of", "what", "pointer", "repo", "commit"]
                }
                Some(other) => {
                    return Err(FieldError::new(
                        "of",
                        format!(
                            "'{}' is not something a verdict can be on (allowed: {})",
                            other,
                            VERDICT_OF.join(", ")
                        ),
                    ))
                }
            }
        }
        Kind::Reverse => {
            require(present(&s.decision), "decision", "reverse")?;
            &["decision"]
        }
        Kind::Refused => {
            require(present(&s.attempted_kind), "attempted_kind", "refused")?;
            require(present(&s.refusal_reason), "refusal_reason", "refused")?;
            &["attempted_kind", "attempted", "refusal_reason"]
        }
    };
    for field in s.set_fields() {
        if !allowed.contains(&field) {
            return Err(FieldError::new(
                field,
                format!(
                    "'{}' is not part of a {} decision's subject",
                    field,
                    kind.as_str()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_pass_key_reads_back_and_other_keys_dont() {
        let s = Subject {
            workflow: Some("wf".into()),
            workflow_session_id: Some("sid".into()),
            state: Some("review".into()),
            gate: Some("panel".into()),
            ..Default::default()
        };
        let key = subject_key(Kind::GatePass, &s);
        assert_eq!(
            gate_pass_key(&key),
            Some(["wf".into(), "sid".into(), "review".into(), "panel".into()])
        );
        let j = Subject {
            what: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(gate_pass_key(&subject_key(Kind::Judgment, &j)), None);
        assert_eq!(gate_pass_key("not json"), None);
    }

    #[test]
    fn every_kind_round_trips_its_name() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
        assert_eq!(Kind::parse("approve"), None);
    }

    #[test]
    fn outcome_outside_the_kind_names_the_field() {
        assert!(validate_outcome(Kind::Accept, "rejected").is_ok());
        let err = validate_outcome(Kind::Merge, "rejected").unwrap_err();
        assert_eq!(err.field, "outcome");
    }

    #[test]
    fn merge_without_pr_names_the_field() {
        let s = Subject {
            repo: Some("o/r".into()),
            commit: Some("abc".into()),
            ..Default::default()
        };
        let err = validate_subject(Kind::Merge, &s, &[]).unwrap_err();
        assert_eq!(err.field, "pr");
    }

    #[test]
    fn verdict_needs_evidence_and_a_commit_on_a_change() {
        let mut s = Subject {
            what: Some("review".into()),
            ..Default::default()
        };
        assert_eq!(
            validate_subject(Kind::Verdict, &s, &[]).unwrap_err().field,
            "evidence"
        );
        let ev = vec!["journal:wf#3".to_string()];
        assert!(validate_subject(Kind::Verdict, &s, &ev).is_ok());
        s.of = Some("change".into());
        s.repo = Some("o/r".into());
        assert_eq!(
            validate_subject(Kind::Verdict, &s, &ev).unwrap_err().field,
            "commit"
        );
        s.commit = Some("abc".into());
        assert!(validate_subject(Kind::Verdict, &s, &ev).is_ok());
    }

    #[test]
    fn a_field_foreign_to_the_kind_is_refused_by_name() {
        let s = Subject {
            what: Some("x".into()),
            pr: Some(3),
            ..Default::default()
        };
        assert_eq!(
            validate_subject(Kind::Judgment, &s, &[]).unwrap_err().field,
            "pr"
        );
    }

    #[test]
    fn accept_of_intent_takes_a_location_and_an_optional_link() {
        let s = Subject {
            of: Some("intent".into()),
            intent_location: Some("forge:o/r#12".into()),
            source_link: Some("https://example.com/t/1".into()),
            ..Default::default()
        };
        assert!(validate_subject(Kind::Accept, &s, &[]).is_ok());
        let s = Subject {
            of: Some("intent".into()),
            ..Default::default()
        };
        assert_eq!(
            validate_subject(Kind::Accept, &s, &[]).unwrap_err().field,
            "intent_location"
        );
    }
}
