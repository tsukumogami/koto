//! The question-prompt evidence behind a person's answer (PRD R62 to R68;
//! DESIGN-decision-record.md, "A person's answer and the question content
//! area").
//!
//! A person's `answer` is recorded only when the harness's own record of
//! the answering session holds a question-prompt call that showed the
//! stored question at the position the caller names and returned the
//! user's reply. The caller hands in a pointer, `<harness session id>/<call
//! id>/<position>`; koto reads the record itself and takes the answer text
//! from the call's result.
//!
//! Where koto looks is fixed by koto, never by the caller: the harness
//! session is the one the answering command runs in (`CLAUDE_CODE_SESSION_ID`
//! through the run journal's driver rule), and its record is the one file
//! `$HOME/.claude/projects/<project>/<harness session id>.jsonl`. A pointer
//! naming any other harness session is refused before a file is opened.
//!
//! The record's format is observed, not a documented contract: the fields
//! read here are listed in [`FIELDS_READ`], each with its path in a captured
//! record, and a record in any other shape is refused rather than guessed
//! at. No refusal message carries text from the record: each names the one
//! check that failed and the pointer the caller handed in.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::decider::Sourced;

/// The harness's question-prompt tool.
pub const QUESTION_TOOL: &str = "AskUserQuestion";

/// The value of a prompt's verified mark.
pub const VERIFIED: &str = "verified";

/// The mark a read shows on an answer whose prompt another host checked.
pub const REMOTE: &str = "remote";

/// The `reading` a read gives such an answer.
pub const REMOTE_READING: &str = "a remote session's answer";

/// How many hex characters of the identity's SHA-256 a prompt header
/// carries. With the leading `#` the header is 12 characters, the most the
/// question tool shows.
pub const TAG_HEX: usize = 11;

/// The record fields koto reads, as paths into a captured record: the call
/// line first, then the result line. `[]` steps into an array; `{q}` is the
/// answers map's key, the question text. A path marked optional may be
/// absent from an accepted record.
pub const FIELDS_READ: &[(&str, bool)] = &[
    ("call: type", false),
    ("call: sessionId", false),
    ("call: message.content[].type", false),
    ("call: message.content[].id", false),
    ("call: message.content[].name", false),
    ("call: message.content[].input.questions[].question", false),
    ("call: message.content[].input.questions[].header", false),
    (
        "call: message.content[].input.questions[].options[].label",
        false,
    ),
    ("result: type", false),
    ("result: sessionId", false),
    ("result: message.content[].type", false),
    ("result: message.content[].tool_use_id", false),
    ("result: message.content[].is_error", true),
    ("result: toolUseResult.answers.{q}", false),
    ("result: permissionDecision.decision", false),
    ("result: permissionDecision.source", false),
];

/// The prompt a person's answer cites, as the answer stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    /// The harness session whose record holds the call.
    pub harness: String,
    /// The call's identifier in that record.
    pub call: String,
    /// The question's 1-based position among the questions the call asked.
    pub position: u32,
    /// Set by koto after every check passed: `verified`, labelled
    /// `observed`. No caller input sets it.
    pub mark: Sourced,
}

impl Prompt {
    /// The call position as a pointer prints it.
    pub fn display(&self) -> String {
        format!("{}/{}/{}", self.harness, self.call, self.position)
    }

    /// Whether `other` cites the same call position.
    pub fn same_position(&self, other: &Prompt) -> bool {
        self.harness == other.harness && self.call == other.call && self.position == other.position
    }
}

/// A pointer the caller hands in with `--prompt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub harness: String,
    pub call: String,
    pub position: u32,
}

impl Pointer {
    /// Parse `<harness session id>/<call id>/<position>`. Both ids are
    /// id-shaped (1 to 128 characters from `A-Z a-z 0-9 . _ : -`), so
    /// neither holds a `/`, and the position is a number from 1.
    pub fn parse(s: &str) -> Result<Self, String> {
        let shape = "--prompt is <harness session id>/<call id>/<position>, each id 1 to 128 \
                     characters from A-Z a-z 0-9 . _ : - and the position a number from 1";
        let mut parts = s.split('/');
        let (Some(h), Some(c), Some(p), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(shape.to_string());
        };
        let position: u32 = p.parse().map_err(|_| shape.to_string())?;
        if !crate::run_journal::is_id(h) || !crate::run_journal::is_id(c) || position == 0 {
            return Err(shape.to_string());
        }
        Ok(Self {
            harness: h.to_string(),
            call: c.to_string(),
            position,
        })
    }

    /// The pointer as the caller wrote it.
    pub fn display(&self) -> String {
        format!("{}/{}/{}", self.harness, self.call, self.position)
    }

    /// Whether `prompt` cites the call position this pointer names.
    pub fn cites(&self, prompt: &Prompt) -> bool {
        self.harness == prompt.harness
            && self.call == prompt.call
            && self.position == prompt.position
    }

    /// The prompt an accepted answer stores.
    pub fn verified(&self) -> Prompt {
        Prompt {
            harness: self.harness.clone(),
            call: self.call.clone(),
            position: self.position,
            mark: Sourced::observed(VERIFIED),
        }
    }
}

/// The checks a pointer must pass, each named in its refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The pointer names a harness session other than the one the
    /// answering command runs in, or the command runs in none.
    Session,
    /// No single record exists for the harness session.
    Record,
    /// The record holds no call with the pointer's id.
    Call,
    /// The call isn't the question tool.
    Tool,
    /// The position is outside the questions the call asked.
    Position,
    /// The question at the position doesn't carry the question's identity.
    Identity,
    /// The question text differs from the stored question's.
    QuestionText,
    /// The option labels differ from the stored question's.
    Options,
    /// The result didn't come from the user's reply.
    Result,
    /// The record isn't in a shape koto recognizes.
    Shape,
    /// An earlier answer already cites this call and position.
    CallPositionReused,
    /// The question has no stored content koto can compare against.
    Content,
}

impl Check {
    pub fn name(self) -> &'static str {
        match self {
            Check::Session => "session",
            Check::Record => "record",
            Check::Call => "call",
            Check::Tool => "tool",
            Check::Position => "position",
            Check::Identity => "identity",
            Check::QuestionText => "question-text",
            Check::Options => "options",
            Check::Result => "result",
            Check::Shape => "shape",
            Check::CallPositionReused => "call-position-reused",
            Check::Content => "content",
        }
    }

    /// What failing the check means, in words that hold no record text.
    pub fn meaning(self) -> &'static str {
        match self {
            Check::Session => {
                "the pointer must name the harness session the answering command runs in"
            }
            Check::Record => "there is no single record for that harness session",
            Check::Call => "the pointer names no call in that session's record",
            Check::Tool => "the call named is not the question tool",
            Check::Position => "the position is outside the questions the call asked",
            Check::Identity => {
                "the question at that position does not carry the identity of the question \
                 being answered"
            }
            Check::QuestionText => "the question text shown differs from the stored question",
            Check::Options => "the option labels shown differ from the stored question's",
            Check::Result => "the call's result did not come from the user's reply",
            Check::Shape => "the record's shape is not recognized",
            Check::CallPositionReused => "an earlier answer already cites this call position",
            Check::Content => {
                "the question has no stored content holding a question and option labels"
            }
        }
    }

    /// The refusal for `pointer`: code `prompt_unverified`, field `prompt`.
    pub fn error(self, pointer: &str) -> super::DecisionError {
        super::DecisionError {
            code: super::codes::PROMPT_UNVERIFIED,
            field: Some("prompt".to_string()),
            message: self.message(pointer),
        }
    }

    /// The refusal message for `pointer`.
    pub fn message(self, pointer: &str) -> String {
        format!(
            "the question-prompt evidence {pointer} fails the '{}' check: {}",
            self.name(),
            self.meaning()
        )
    }
}

/// What the stored question says the prompt must have shown.
#[derive(Debug, Clone)]
pub struct Expected {
    pub question: String,
    pub labels: Vec<String>,
    /// The header that carries the question's identity ([`tag`]).
    pub header: String,
}

impl Expected {
    /// Read the stored content: a JSON object with a `question` string and
    /// an `options` array whose every element has a `label` string. Other
    /// fields are allowed and ignored.
    pub fn from_content(bytes: &[u8], identity: &str) -> Result<Self, Check> {
        let v: Value = serde_json::from_slice(bytes).map_err(|_| Check::Content)?;
        let question = v
            .get("question")
            .and_then(Value::as_str)
            .ok_or(Check::Content)?;
        let labels = v
            .get("options")
            .and_then(Value::as_array)
            .ok_or(Check::Content)?
            .iter()
            .map(|o| o.get("label").and_then(Value::as_str).map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or(Check::Content)?;
        Ok(Self {
            question: question.to_string(),
            labels,
            header: tag(identity),
        })
    }
}

/// The header a prompt position carries for the question `identity`
/// (`<raiser's session id>/<decision id>`): `#` and the first
/// [`TAG_HEX`] hex characters of the identity's SHA-256.
pub fn tag(identity: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex = hex::encode(Sha256::digest(identity.as_bytes()));
    format!("#{}", &hex[..TAG_HEX])
}

/// The file in a session's directory, beside its state file and outside
/// everything sync carries, listing the answers whose prompt this host
/// checked: one `<header session id> <decision id>` line each.
pub const VERIFIED_HERE_FILE: &str = "koto-verified-here";

/// The most bytes [`verified_here`] reads of the note.
const MAX_NOTE_BYTES: u64 = 1 << 20;

/// Note that this host checked the prompt of answer `decision` in the
/// session with header id `session_id`.
pub fn note_verified_here(
    session_dir: &Path,
    session_id: &str,
    decision: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(session_dir.join(VERIFIED_HERE_FILE))?;
    file.write_all(format!("{session_id} {decision}\n").as_bytes())?;
    file.sync_all()
}

/// Whether this host checked the prompt of answer `decision` in the
/// session with header id `session_id`. A session that arrived by import
/// or by a pull has no such note for an answer recorded elsewhere, so its
/// answers read remote.
pub fn verified_here(session_dir: &Path, session_id: &str, decision: &str) -> bool {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(session_dir.join(VERIFIED_HERE_FILE)) else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut bytes = Vec::new();
    if file.take(MAX_NOTE_BYTES).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let want = format!("{session_id} {decision}");
    String::from_utf8_lossy(&bytes).lines().any(|l| l == want)
}

/// Where Claude Code keeps each project's session records:
/// `$HOME/.claude/projects`, the root koto's `/workflows` surface reads.
pub fn projects_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".claude").join("projects"))
}

/// Check `pointer` for a person's answer recorded by a command running in
/// harness session `observed`, against the record under `root`, and return
/// the answer text the call's result holds for the pointer's position.
pub fn verify(
    root: Option<&Path>,
    observed: Option<&str>,
    pointer: &Pointer,
    expected: &Expected,
) -> Result<String, Check> {
    if observed != Some(pointer.harness.as_str()) {
        return Err(Check::Session);
    }
    let record = find_record(root.ok_or(Check::Record)?, &pointer.harness)?;
    let (call, result) = read_call(&record, &pointer.call)?;
    let call = call.ok_or(Check::Call)?;
    if call.get("sessionId").and_then(Value::as_str) != Some(pointer.harness.as_str()) {
        return Err(if call.get("sessionId").is_some_and(Value::is_string) {
            Check::Session
        } else {
            Check::Shape
        });
    }
    let item = call_item(&call, &pointer.call).ok_or(Check::Shape)?;
    match item.get("name").and_then(Value::as_str) {
        Some(QUESTION_TOOL) => {}
        Some(_) => return Err(Check::Tool),
        None => return Err(Check::Shape),
    }
    let questions = item
        .get("input")
        .and_then(|i| i.get("questions"))
        .and_then(Value::as_array)
        .ok_or(Check::Shape)?;
    let mut shown = Vec::with_capacity(questions.len());
    for q in questions {
        let text = q.get("question").and_then(Value::as_str);
        let header = q.get("header").and_then(Value::as_str);
        let labels = q.get("options").and_then(Value::as_array).and_then(|os| {
            os.iter()
                .map(|o| o.get("label").and_then(Value::as_str))
                .collect::<Option<Vec<_>>>()
        });
        match (text, header, labels) {
            (Some(t), Some(h), Some(l)) => shown.push((t, h, l)),
            _ => return Err(Check::Shape),
        }
    }
    let index = pointer.position as usize - 1;
    let Some((text, header, labels)) = shown.get(index) else {
        return Err(Check::Position);
    };
    // The result's answers are keyed by question text, so two questions
    // with one text can't be told apart.
    if shown.iter().filter(|(t, _, _)| t == text).count() > 1 {
        return Err(Check::Shape);
    }
    if *header != expected.header {
        return Err(Check::Identity);
    }
    if text.as_bytes() != expected.question.as_bytes() {
        return Err(Check::QuestionText);
    }
    if labels.len() != expected.labels.len()
        || labels
            .iter()
            .zip(&expected.labels)
            .any(|(a, b)| a.as_bytes() != b.as_bytes())
    {
        return Err(Check::Options);
    }

    let result = result.ok_or(Check::Result)?;
    match result.get("sessionId").and_then(Value::as_str) {
        Some(s) if s == pointer.harness => {}
        Some(_) => return Err(Check::Session),
        None => return Err(Check::Shape),
    }
    let reply = result_item(&result, &pointer.call).ok_or(Check::Shape)?;
    match reply.get("is_error") {
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => return Err(Check::Result),
        Some(_) => return Err(Check::Shape),
    }
    let tool_result = match result.get("toolUseResult") {
        Some(Value::Object(m)) => m,
        Some(_) => return Err(Check::Result),
        None => return Err(Check::Shape),
    };
    let decision = match result.get("permissionDecision") {
        Some(Value::Object(m)) => m,
        _ => return Err(Check::Result),
    };
    // The reply must be the person's: an accepted decision whose source is
    // one of the user's own choices. Those sources share the `user` prefix
    // (a hook or a rule names another), and the prefix rather than one
    // observed value keeps a person's other replies from being refused.
    if decision.get("decision").and_then(Value::as_str) != Some("accept")
        || !decision
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("user"))
    {
        return Err(Check::Result);
    }
    let answer = tool_result
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(Check::Shape)?
        .get(*text)
        .and_then(Value::as_str)
        .ok_or(Check::Shape)?;
    Ok(answer.to_string())
}

/// The one record `<root>/<project>/<harness>.jsonl`: a regular file, not
/// a symlink, in a project directory that isn't one either. None, or more
/// than one, fails the `record` check.
fn find_record(root: &Path, harness: &str) -> Result<PathBuf, Check> {
    let name = format!("{harness}.jsonl");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root)
        .map_err(|_| Check::Record)?
        .flatten()
    {
        let dir = entry.path();
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let candidate = dir.join(&name);
        if std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.is_file()) {
            found.push(candidate);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        _ => Err(Check::Record),
    }
}

/// Stream the record line by line and return the first line holding the
/// call and the first holding its result. Only lines that mention the call
/// id are parsed; such a line that isn't valid JSON fails the `shape`
/// check.
fn read_call(path: &Path, call: &str) -> Result<(Option<Value>, Option<Value>), Check> {
    let file = std::fs::File::open(path).map_err(|_| Check::Record)?;
    let needle = format!("\"{call}\"");
    let needle = needle.as_bytes();
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let (mut found_call, mut found_result) = (None, None);
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return Err(Check::Record),
        }
        if !line.windows(needle.len()).any(|w| w == needle) {
            continue;
        }
        let value: Value = serde_json::from_slice(&line).map_err(|_| Check::Shape)?;
        if found_call.is_none() && call_item(&value, call).is_some() {
            found_call = Some(value);
        } else if found_result.is_none() && result_item(&value, call).is_some() {
            found_result = Some(value);
        }
        if found_call.is_some() && found_result.is_some() {
            break;
        }
    }
    Ok((found_call, found_result))
}

/// The `tool_use` content item with id `call` in an assistant line.
fn call_item<'a>(line: &'a Value, call: &str) -> Option<&'a Value> {
    if line.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    line.get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .find(|c| {
            c.get("type").and_then(Value::as_str) == Some("tool_use")
                && c.get("id").and_then(Value::as_str) == Some(call)
        })
}

/// The `tool_result` content item answering `call` in a user line.
fn result_item<'a>(line: &'a Value, call: &str) -> Option<&'a Value> {
    if line.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    line.get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .find(|c| {
            c.get("type").and_then(Value::as_str) == Some("tool_result")
                && c.get("tool_use_id").and_then(Value::as_str) == Some(call)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The captured fixtures: structural copies of a real accepted call and
    /// a real denied one, every text value a marker string.
    const ACCEPTED: &str = include_str!("../../tests/fixtures/prompt/accepted.jsonl");
    const DENIED: &str = include_str!("../../tests/fixtures/prompt/denied.jsonl");

    /// Whether `path` (a [`FIELDS_READ`] path after its `call: ` or
    /// `result: ` prefix) reaches a value in `v`: every element of an array
    /// stepped into with `[]`, and every entry of the answers map at `{q}`,
    /// must hold the rest of the path, and at least one must exist.
    fn holds(v: &Value, path: &[&str]) -> bool {
        let Some((step, rest)) = path.split_first() else {
            return !v.is_null();
        };
        if *step == "{q}" {
            return v
                .as_object()
                .is_some_and(|m| !m.is_empty() && m.values().all(|x| holds(x, rest)));
        }
        let (name, each) = match step.strip_suffix("[]") {
            Some(n) => (n, true),
            None => (*step, false),
        };
        let Some(x) = v.get(name) else { return false };
        if each {
            x.as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(|e| holds(e, rest)))
        } else {
            holds(x, rest)
        }
    }

    fn check(record: &str, path: &str) -> bool {
        let lines: Vec<Value> = record
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let (line, rest) = match path.split_once(": ") {
            Some(("call", r)) => (&lines[0], r),
            Some(("result", r)) => (&lines[1], r),
            _ => panic!("{path} names neither the call nor the result"),
        };
        holds(line, &rest.split('.').collect::<Vec<_>>())
    }

    #[test]
    fn every_field_koto_reads_is_in_the_captured_structure() {
        for (path, optional) in FIELDS_READ {
            if !optional {
                assert!(check(ACCEPTED, path), "the accepted fixture lacks {path}");
            }
        }
        assert!(check(DENIED, "result: message.content[].is_error"));
        assert!(check(DENIED, "result: permissionDecision.decision"));
        assert!(check(
            DENIED,
            "call: message.content[].input.questions[].header"
        ));
    }

    #[test]
    fn a_path_the_structure_lacks_is_caught() {
        assert!(!check(ACCEPTED, "result: toolUseResult.answer.{q}"));
        assert!(!check(
            ACCEPTED,
            "call: message.content[].input.question[].header"
        ));
    }

    #[test]
    fn a_pointer_has_two_ids_and_a_position_from_one() {
        let p = Pointer::parse("h-1/toolu_9/2").unwrap();
        assert_eq!(
            (p.harness.as_str(), p.call.as_str(), p.position),
            ("h-1", "toolu_9", 2)
        );
        assert_eq!(p.display(), "h-1/toolu_9/2");
        for bad in [
            "h/c", "h/c/0", "h/c/x", "h/c/1/2", "/c/1", "h//1", "h c/c/1",
        ] {
            assert!(Pointer::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_tag_is_twelve_characters_from_the_identity_hash() {
        let t = tag("0f3c/000000000001");
        assert_eq!(t.len(), 12);
        assert!(t.starts_with('#'));
        assert_ne!(t, tag("0f3c/000000000002"));
    }

    #[test]
    fn content_needs_a_question_and_labelled_options() {
        let ok =
            br#"{"question":"q","options":[{"label":"a"},{"label":"b","description":"d"}],"x":1}"#;
        let e = Expected::from_content(ok, "s/1").unwrap();
        assert_eq!(e.question, "q");
        assert_eq!(e.labels, vec!["a", "b"]);
        for bad in [
            &b"not json"[..],
            br#"{"options":[]}"#,
            br#"{"question":"q"}"#,
            br#"{"question":"q","options":[{"description":"d"}]}"#,
        ] {
            assert_eq!(
                Expected::from_content(bad, "s/1").unwrap_err(),
                Check::Content
            );
        }
    }
}
