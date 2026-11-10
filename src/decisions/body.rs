//! The decision body: kinds and outcomes (PRD R1), subjects (R2), the fields
//! every decision carries (R3), the harness pointers (R30) and the size caps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::chain::ChainSnapshot;
use super::decider::{Authority, Decider};
use super::{is_decision_id, DecisionError, FORMAT_VERSION};

/// The largest body a writer stores or a reader parses.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The largest reason, in bytes.
pub const MAX_REASON_BYTES: usize = 4 * 1024;

/// The most `basis` and `evidence` entries one decision carries.
pub const MAX_LIST_ENTRIES: usize = 64;

/// The closed set of kinds and, for each, its closed set of outcomes (R1).
pub const KINDS: [(&str, &[&str]); 9] = [
    ("accept", &["accepted", "rejected"]),
    ("merge", &["merged"]),
    ("gate-pass", &["passed"]),
    ("escalate", &["escalated"]),
    ("decline-escalation", &["kept"]),
    ("judgment", &["settled"]),
    ("verdict", &["pass", "fail", "inconclusive"]),
    ("reverse", &["reversed"]),
    ("refused", &["refused"]),
];

/// The outcomes `kind` allows, or `None` for a kind outside R1's set.
pub fn outcomes_of(kind: &str) -> Option<&'static [&'static str]> {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, o)| *o)
}

/// The deciding session, as a body names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub name: String,
    pub id: String,
}

/// An earlier decision a decision relies on (R3's `basis`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisRef {
    pub session: String,
    pub id: String,
}

/// A decision as it is stored. Field order is the order it serializes in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionBody {
    pub v: u32,
    pub id: String,
    pub session: SessionRef,
    /// UTC, to the millisecond, stamped by the writer from the clock.
    pub time: String,
    /// The workflow state the session was in, or empty.
    pub state: String,
    /// The sequence number of the session's last log event when the
    /// decision was recorded, 0 for an empty log.
    pub log_seq: u64,
    pub kind: String,
    pub outcome: String,
    pub subject: BTreeMap<String, Value>,
    pub decider: Decider,
    pub authority: Authority,
    pub chain: ChainSnapshot,
    #[serde(default)]
    pub basis: Vec<BasisRef>,
    #[serde(default)]
    pub evidence: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The harness's identifier for the tool call that recorded the
    /// decision (R30). The harness session and agent pointers are the
    /// decider's own `harness` and `agent` fields; nothing a pointer names
    /// is copied or fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<String>,
}

impl DecisionBody {
    /// The bytes a writer stores: compact JSON with a trailing newline.
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Parse stored bytes, refusing a body over [`MAX_BODY_BYTES`], one in
    /// another format version and one whose `id` isn't a decision id.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BODY_BYTES {
            return Err(format!("body is larger than {MAX_BODY_BYTES} bytes"));
        }
        let body: DecisionBody =
            serde_json::from_slice(bytes).map_err(|e| format!("body doesn't parse: {e}"))?;
        if body.v != FORMAT_VERSION {
            return Err(format!("body is format version {}", body.v));
        }
        if !is_decision_id(&body.id) {
            return Err("body's id isn't a decision id".to_string());
        }
        if outcomes_of(&body.kind).is_none_or(|o| !o.contains(&body.outcome.as_str())) {
            return Err("body's kind or outcome isn't one koto knows".to_string());
        }
        Ok(body)
    }
}

/// What a caller asked to record, before koto adds what it stamps itself.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub kind: String,
    pub outcome: String,
    /// `field=value` pairs from `--subject`, in the order given.
    pub subject: Vec<(String, String)>,
    pub basis: Vec<String>,
    pub evidence: Vec<String>,
    pub reason: Option<String>,
    pub tool_call: Option<String>,
}

/// The subject fields a kind takes: the ones it requires, and the optional
/// ones. `path` may repeat and is stored as the list `paths`.
struct SubjectShape {
    required: &'static [&'static str],
    optional: &'static [&'static str],
}

/// The subject shapes each kind accepts (R2). `accept` has three, and a
/// subject must match one of them exactly; every other kind has one.
fn shapes(kind: &str) -> &'static [SubjectShape] {
    const ACCEPT: &[SubjectShape] = &[
        // A document, plan, test change, or policy or configuration change.
        SubjectShape {
            required: &["repository", "path", "commit"],
            optional: &[],
        },
        // One step of a plan.
        SubjectShape {
            required: &["repository", "plan", "step", "commit"],
            optional: &[],
        },
        // An intent or a staged draft: a koto request id, a repository path
        // or a forge issue, and optionally the ticket or alert it came from.
        SubjectShape {
            required: &["location"],
            optional: &["link"],
        },
    ];
    const MERGE: &[SubjectShape] = &[SubjectShape {
        required: &["repository", "pr", "commit"],
        optional: &[],
    }];
    const GATE_PASS: &[SubjectShape] = &[SubjectShape {
        required: &["workflow", "state", "gate"],
        optional: &[],
    }];
    const ESCALATE: &[SubjectShape] = &[SubjectShape {
        required: &["question", "text", "recipient"],
        optional: &[],
    }];
    const DECLINE: &[SubjectShape] = &[SubjectShape {
        required: &["question", "text"],
        optional: &[],
    }];
    const JUDGMENT: &[SubjectShape] = &[SubjectShape {
        required: &["what"],
        optional: &["pointer"],
    }];
    const VERDICT: &[SubjectShape] = &[
        SubjectShape {
            required: &["what"],
            optional: &[],
        },
        // A change or a release: the repository and commit judged.
        SubjectShape {
            required: &["what", "repository", "commit"],
            optional: &[],
        },
    ];
    const REVERSE: &[SubjectShape] = &[SubjectShape {
        required: &["session", "id"],
        optional: &[],
    }];
    const REFUSED: &[SubjectShape] = &[SubjectShape {
        required: &["kind", "subject"],
        optional: &[],
    }];
    match kind {
        "accept" => ACCEPT,
        "merge" => MERGE,
        "gate-pass" => GATE_PASS,
        "escalate" => ESCALATE,
        "decline-escalation" => DECLINE,
        "judgment" => JUDGMENT,
        "verdict" => VERDICT,
        "reverse" => REVERSE,
        "refused" => REFUSED,
        _ => &[],
    }
}

/// Check `req`'s kind, outcome and subject against R1 and R2, and its
/// lists and reason against the caps, returning the subject as stored.
/// Every refusal names the field.
pub fn validate(req: &Request) -> Result<BTreeMap<String, Value>, DecisionError> {
    let outcomes = outcomes_of(&req.kind).ok_or_else(|| {
        DecisionError::invalid(
            "kind",
            format!(
                "unknown decision kind '{}': expected one of {}",
                preview(&req.kind),
                KINDS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
            ),
        )
    })?;
    if !outcomes.contains(&req.outcome.as_str()) {
        return Err(DecisionError::invalid(
            "outcome",
            format!(
                "outcome '{}' isn't allowed for kind '{}': expected one of {}",
                preview(&req.outcome),
                req.kind,
                outcomes.join(", ")
            ),
        ));
    }

    let mut given: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (field, value) in &req.subject {
        if field.is_empty() || !field.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            return Err(DecisionError::invalid(
                "subject",
                format!("subject field '{}' isn't a field name", preview(field)),
            ));
        }
        if value.trim().is_empty() {
            return Err(DecisionError::invalid(
                &format!("subject.{field}"),
                format!("subject field '{field}' is empty"),
            ));
        }
        let values = given.entry(field.clone()).or_default();
        if !values.is_empty() && field != "path" {
            return Err(DecisionError::invalid(
                &format!("subject.{field}"),
                format!("subject field '{field}' is given more than once"),
            ));
        }
        values.push(value.clone());
    }

    let kind_shapes = shapes(&req.kind);
    let fits = |shape: &SubjectShape| {
        shape.required.iter().all(|f| given.contains_key(*f))
            && given.keys().all(|f| {
                shape.required.contains(&f.as_str()) || shape.optional.contains(&f.as_str())
            })
    };
    if !kind_shapes.iter().any(fits) {
        // Name the first field that keeps the closest shape from fitting:
        // a missing required field, else one the kind doesn't take.
        let field = kind_shapes
            .iter()
            .flat_map(|s| s.required.iter())
            .find(|f| !given.contains_key(**f))
            .map(|f| format!("subject.{f}"))
            .or_else(|| {
                given
                    .keys()
                    .find(|f| {
                        !kind_shapes.iter().any(|s| {
                            s.required.contains(&f.as_str()) || s.optional.contains(&f.as_str())
                        })
                    })
                    .map(|f| format!("subject.{f}"))
            })
            .unwrap_or_else(|| "subject".to_string());
        let wanted = kind_shapes
            .iter()
            .map(|s| s.required.join("+"))
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(DecisionError::invalid(
            &field,
            format!(
                "the subject of a '{}' decision needs {}; '{}' is missing or not allowed",
                req.kind, wanted, field
            ),
        ));
    }

    if req.kind == "verdict" && req.evidence.is_empty() {
        return Err(DecisionError::invalid(
            "evidence",
            "a 'verdict' decision needs at least one --evidence pointer",
        ));
    }
    if req.kind == "refused" && req.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
        return Err(DecisionError::invalid(
            "reason",
            "a 'refused' decision needs a --reason",
        ));
    }
    if req.kind == "refused" {
        let attempted = given
            .get("kind")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();
        if outcomes_of(&attempted).is_none() {
            return Err(DecisionError::invalid(
                "subject.kind",
                format!(
                    "attempted kind '{}' isn't a decision kind",
                    preview(&attempted)
                ),
            ));
        }
    }
    if let Some(reason) = &req.reason {
        if reason.len() > MAX_REASON_BYTES {
            return Err(DecisionError::invalid(
                "reason",
                format!("reason is larger than {MAX_REASON_BYTES} bytes"),
            ));
        }
    }
    if req.basis.len() > MAX_LIST_ENTRIES {
        return Err(DecisionError::invalid(
            "basis",
            format!("more than {MAX_LIST_ENTRIES} --basis entries"),
        ));
    }
    if req.evidence.len() > MAX_LIST_ENTRIES {
        return Err(DecisionError::invalid(
            "evidence",
            format!("more than {MAX_LIST_ENTRIES} --evidence entries"),
        ));
    }
    if let Some(tool_call) = &req.tool_call {
        if !crate::run_journal::is_id(tool_call) {
            return Err(DecisionError::invalid(
                "tool_call",
                "the tool-call identifier must be 1 to 128 characters from A-Z a-z 0-9 . _ : -",
            ));
        }
    }

    let mut subject = BTreeMap::new();
    for (field, mut values) in given {
        if field == "path" {
            subject.insert(
                "paths".to_string(),
                Value::Array(values.into_iter().map(Value::String).collect()),
            );
        } else {
            subject.insert(field, Value::String(values.remove(0)));
        }
    }
    Ok(subject)
}

/// Parse `--basis <session>:<id>` entries.
pub fn parse_basis(entries: &[String]) -> Result<Vec<BasisRef>, DecisionError> {
    entries
        .iter()
        .map(|entry| {
            let (session, id) = entry.rsplit_once(':').ok_or_else(|| {
                DecisionError::invalid("basis", "a --basis entry is <session>:<id>")
            })?;
            if crate::discover::validate_workflow_name(session).is_err() || !is_decision_id(id) {
                return Err(DecisionError::invalid(
                    "basis",
                    format!(
                        "--basis '{}' needs a session name and a {}-digit id",
                        preview(entry),
                        super::ID_WIDTH
                    ),
                ));
            }
            Ok(BasisRef {
                session: session.to_string(),
                id: id.to_string(),
            })
        })
        .collect()
}

/// Parse `--evidence` entries: `event:<session>:<seq>` points at a koto
/// session event; anything else is stored as an opaque pointer.
pub fn parse_evidence(entries: &[String]) -> Result<Vec<Value>, DecisionError> {
    entries
        .iter()
        .map(|entry| {
            if let Some(rest) = entry.strip_prefix("event:") {
                let (session, seq) = rest.rsplit_once(':').ok_or_else(|| {
                    DecisionError::invalid("evidence", "an event pointer is event:<session>:<seq>")
                })?;
                let seq: u64 = seq.parse().map_err(|_| {
                    DecisionError::invalid("evidence", "an event pointer's seq is a number")
                })?;
                if crate::discover::validate_workflow_name(session).is_err() {
                    return Err(DecisionError::invalid(
                        "evidence",
                        "an event pointer names a session",
                    ));
                }
                return Ok(serde_json::json!({"kind": "event", "session": session, "seq": seq}));
            }
            if entry.trim().is_empty() {
                return Err(DecisionError::invalid(
                    "evidence",
                    "an --evidence entry is empty",
                ));
            }
            Ok(serde_json::json!({"kind": "pointer", "value": entry}))
        })
        .collect()
}

/// Check a body that reached the store some other way than a fresh record,
/// such as one repaired from the event it was embedded in, with the checks
/// a fresh record gets: it parses as a body, it names `session_name` and
/// `session_id`, it isn't a reversal (those are aimed, never repaired into
/// `koto/decisions/`), and its kind, outcome, subject, basis, evidence,
/// reason and tool-call identifier pass [`validate`], [`parse_basis`] and
/// [`parse_evidence`] and come back as the body holds them.
pub fn validate_stored(
    body: &DecisionBody,
    session_name: &str,
    session_id: &str,
) -> Result<(), DecisionError> {
    let invalid = |m: String| DecisionError::invalid("body", m);
    let bytes = body
        .to_bytes()
        .map_err(|e| invalid(format!("body doesn't serialize: {e}")))?;
    let parsed = DecisionBody::parse(&bytes).map_err(invalid)?;
    if parsed.session.name != session_name || parsed.session.id != session_id {
        return Err(invalid(format!(
            "body names session '{}' ({}), not '{session_name}' ({session_id})",
            parsed.session.name, parsed.session.id
        )));
    }
    if parsed.kind == "reverse" {
        return Err(invalid(
            "a reversal isn't stored in koto/decisions/".to_string(),
        ));
    }
    let mut subject = Vec::new();
    for (field, value) in &parsed.subject {
        match (field.as_str(), value) {
            ("paths", Value::Array(paths)) => {
                for p in paths {
                    let p = p
                        .as_str()
                        .ok_or_else(|| invalid("subject.paths holds a non-string".to_string()))?;
                    subject.push(("path".to_string(), p.to_string()));
                }
            }
            (_, Value::String(v)) => subject.push((field.clone(), v.clone())),
            _ => return Err(invalid(format!("subject.{field} isn't a string"))),
        }
    }
    let basis: Vec<String> = parsed
        .basis
        .iter()
        .map(|b| format!("{}:{}", b.session, b.id))
        .collect();
    let evidence = parsed
        .evidence
        .iter()
        .map(|e| match (e["kind"].as_str(), e["value"].as_str()) {
            (Some("event"), _) => Ok(format!(
                "event:{}:{}",
                e["session"].as_str().unwrap_or_default(),
                e["seq"].as_u64().unwrap_or_default()
            )),
            (Some("pointer"), Some(v)) => Ok(v.to_string()),
            _ => Err(invalid(
                "an evidence entry is neither an event nor a pointer".to_string(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let req = Request {
        kind: parsed.kind.clone(),
        outcome: parsed.outcome.clone(),
        subject,
        basis,
        evidence,
        reason: parsed.reason.clone(),
        tool_call: parsed.tool_call.clone(),
    };
    if validate(&req)? != parsed.subject
        || parse_basis(&req.basis)? != parsed.basis
        || parse_evidence(&req.evidence)? != parsed.evidence
    {
        return Err(invalid(
            "body's subject, basis or evidence isn't as a fresh record stores it".to_string(),
        ));
    }
    Ok(())
}

/// A short, printable preview of caller input for an error message.
fn preview(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: &str, outcome: &str, subject: &[(&str, &str)]) -> Request {
        Request {
            kind: kind.to_string(),
            outcome: outcome.to_string(),
            subject: subject
                .iter()
                .map(|(f, v)| (f.to_string(), v.to_string()))
                .collect(),
            ..Request::default()
        }
    }

    #[test]
    fn every_kind_but_the_unknown_one_validates_with_its_subject() {
        let ok = [
            req(
                "accept",
                "accepted",
                &[
                    ("repository", "r"),
                    ("path", "a"),
                    ("path", "b"),
                    ("commit", "c"),
                ],
            ),
            req(
                "accept",
                "rejected",
                &[
                    ("repository", "r"),
                    ("plan", "p"),
                    ("step", "1"),
                    ("commit", "c"),
                ],
            ),
            req(
                "accept",
                "accepted",
                &[("location", "req-1"), ("link", "https://x/1")],
            ),
            req(
                "merge",
                "merged",
                &[("repository", "r"), ("pr", "12"), ("commit", "c")],
            ),
            req(
                "gate-pass",
                "passed",
                &[("workflow", "w"), ("state", "s"), ("gate", "g")],
            ),
            req(
                "escalate",
                "escalated",
                &[("question", "q1"), ("text", "t"), ("recipient", "coord")],
            ),
            req(
                "decline-escalation",
                "kept",
                &[("question", "q1"), ("text", "t")],
            ),
            req("judgment", "settled", &[("what", "a finding")]),
        ];
        for r in ok {
            validate(&r).unwrap_or_else(|e| panic!("{}: {e}", r.kind));
        }
        let paths = validate(&req(
            "accept",
            "accepted",
            &[
                ("repository", "r"),
                ("path", "a"),
                ("path", "b"),
                ("commit", "c"),
            ],
        ))
        .unwrap();
        assert_eq!(paths["paths"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn a_refusal_names_the_field() {
        assert_eq!(
            validate(&req("nope", "x", &[]))
                .unwrap_err()
                .field
                .as_deref(),
            Some("kind")
        );
        assert_eq!(
            validate(&req("merge", "accepted", &[]))
                .unwrap_err()
                .field
                .as_deref(),
            Some("outcome")
        );
        assert_eq!(
            validate(&req(
                "merge",
                "merged",
                &[("repository", "r"), ("commit", "c")]
            ))
            .unwrap_err()
            .field
            .as_deref(),
            Some("subject.pr")
        );
        assert_eq!(
            validate(&req("judgment", "settled", &[("what", "x"), ("gate", "g")]))
                .unwrap_err()
                .field
                .as_deref(),
            Some("subject.gate")
        );
        let mut verdict = req("verdict", "pass", &[("what", "x")]);
        assert_eq!(
            validate(&verdict).unwrap_err().field.as_deref(),
            Some("evidence")
        );
        verdict.evidence.push("event:w:3".to_string());
        validate(&verdict).unwrap();
    }

    #[test]
    fn the_reason_cap_is_four_kib() {
        let mut r = req("judgment", "settled", &[("what", "x")]);
        r.reason = Some("a".repeat(MAX_REASON_BYTES));
        validate(&r).unwrap();
        r.reason = Some("a".repeat(MAX_REASON_BYTES + 1));
        assert_eq!(validate(&r).unwrap_err().field.as_deref(), Some("reason"));
    }

    #[test]
    fn evidence_and_basis_parse() {
        let ev = parse_evidence(&["event:w-1:7".to_string(), "https://x/y".to_string()]).unwrap();
        assert_eq!(
            ev[0],
            serde_json::json!({"kind": "event", "session": "w-1", "seq": 7})
        );
        assert_eq!(ev[1]["kind"], "pointer");
        let b = parse_basis(&["w-1:000000000003".to_string()]).unwrap();
        assert_eq!(b[0].id, "000000000003");
        assert!(parse_basis(&["w-1:3".to_string()]).is_err());
    }
}
