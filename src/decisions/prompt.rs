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
//! Where koto looks is fixed by koto: the record is the one file
//! `<account home>/.claude/projects/<project>/<harness session id>.jsonl`,
//! where the account home is the effective user's entry in the account
//! database, never `$HOME`, and the record's real path must lie under that
//! root. The harness session is the one the answering command runs in
//! (`CLAUDE_CODE_SESSION_ID` through the run journal's driver rule), and
//! when koto has recorded a driving harness session for the answering koto
//! session, the two must agree. Neither is authenticated: both are values
//! a process running as the user can set, so the check is as strong as the
//! user's control of their own processes and home.
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

use super::body::DecisionBody;
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
    ("call: isSidechain", false),
    ("call: timestamp", false),
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
    ("result: isSidechain", false),
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
        if !crate::names::is_id(h) || !crate::names::is_id(c) || position == 0 {
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
    /// The call or its result was made in a side conversation (a
    /// subagent's), not the harness session's own.
    Sidechain,
    /// The call was made before the question was raised.
    Time,
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
            Check::Sidechain => "sidechain",
            Check::Time => "time",
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
            Check::Sidechain => {
                "the call or its result was made in a side conversation, not the session's own"
            }
            Check::Time => "the call was made before the question was raised",
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
    /// When the question was raised: a prompt opened earlier can't have
    /// shown it. `None` when the question's time can't be read, which
    /// fails the `time` check.
    pub asked: Option<std::time::SystemTime>,
}

impl Expected {
    /// Read the stored content: a JSON object with a `question` string and
    /// an `options` array whose every element has a `label` string. Other
    /// fields are allowed and ignored.
    pub fn from_content(bytes: &[u8], identity: &str, asked: &str) -> Result<Self, Check> {
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
            asked: crate::engine::wake::parse_rfc3339_millis(asked),
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
/// checked: one `<session id> <decision id> <seal>` line each, the seal
/// binding the line to this host, to the session directory's name and to
/// the stored answer ([`seal`]).
pub const VERIFIED_HERE_FILE: &str = "koto-verified-here";

/// The file under the koto root holding this host's identity: 32 random
/// bytes as 64 hex digits, created once and never synced or imported.
pub const HOST_ID_FILE: &str = "host-id";

/// The directory under the koto root holding one create-only marker per
/// call position a person's answer on this host has cited.
pub const PROMPT_USES_DIR: &str = "prompt-uses";

/// The most bytes [`verified_here`] reads of the note.
const MAX_NOTE_BYTES: u64 = 1 << 20;

/// The most bytes of a record koto reads on one line. A longer line fails
/// the `shape` check, so a huge line can't exhaust memory.
pub const MAX_RECORD_LINE_BYTES: usize = 16 << 20;

/// The most bytes of a record koto reads in all. A longer record fails the
/// `shape` check.
pub const MAX_RECORD_BYTES: u64 = 1 << 30;

/// Open `path` for reading without following a final symlink, and only
/// when it is a regular file.
fn open_plain(path: &Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    file.metadata().is_ok_and(|m| m.is_file()).then_some(file)
}

/// This host's identity under `koto_home`, or `None` when it doesn't exist
/// yet, isn't what koto writes, or its group or others may write it (anyone
/// who can replace it can seal a note line).
fn read_host_id(koto_home: &Path) -> Option<[u8; 32]> {
    use std::io::Read;
    let file = open_plain(&koto_home.join(HOST_ID_FILE))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata().ok()?.permissions().mode() & 0o022 != 0 {
            return None;
        }
    }
    let mut text = String::new();
    file.take(65).read_to_string(&mut text).ok()?;
    let bytes = hex::decode(text.trim_end_matches('\n')).ok()?;
    bytes.try_into().ok()
}

/// `n` random bytes from the system's source.
fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    use std::io::Read;
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

/// This host's identity, created under `koto_home` when there is none.
/// The bytes are written whole to a file of their own in the same
/// directory and then linked into place create-only, so no reader ever sees
/// a partly written identity and two racing processes read back the one
/// that won.
fn host_id(koto_home: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Write;
    if let Some(id) = read_host_id(koto_home) {
        return Ok(id);
    }
    let unusable = || {
        std::io::Error::other(
            "the host identity file under the koto root isn't one koto wrote, or its group or \
             others may write it",
        )
    };
    crate::koto_home::ensure_private_root(koto_home)?;
    let target = koto_home.join(HOST_ID_FILE);
    if target.symlink_metadata().is_ok() {
        return read_host_id(koto_home).ok_or_else(unusable);
    }
    let id: [u8; 32] = random_bytes()?;
    let tmp = koto_home.join(format!(
        ".{HOST_ID_FILE}.{}.{}",
        std::process::id(),
        hex::encode(random_bytes::<8>()?)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let written = options.open(&tmp).and_then(|mut f| {
        f.write_all(format!("{}\n", hex::encode(id)).as_bytes())?;
        f.sync_all()
    });
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, &target));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    read_host_id(koto_home).ok_or_else(unusable)
}

/// The seal a note line carries for answer `body` held in the session
/// directory named `holder`: SHA-256 over this host's identity, the
/// holder's name and the SHA-256 of the answer as stored. A line copied to
/// another host, to another session directory, or written for another body
/// doesn't match.
fn seal(host: &[u8; 32], holder: &str, body: &DecisionBody) -> String {
    use sha2::{Digest, Sha256};
    let stored = Sha256::digest(serde_json::to_vec(body).unwrap_or_default());
    let mut h = Sha256::new();
    h.update(host);
    h.update([0]);
    h.update(holder.as_bytes());
    h.update([0]);
    h.update(stored);
    hex::encode(h.finalize())
}

fn note_line(host: &[u8; 32], holder: &str, body: &DecisionBody) -> String {
    format!(
        "{} {} {}",
        body.session.id,
        body.id,
        seal(host, holder, body)
    )
}

/// Note that this host checked the prompt of answer `body`, about to be
/// written in the session `holder` whose directory is `session_dir`. The
/// note is written before the answer is published, so a process stopped
/// between the two leaves a line naming no stored answer, which reads as
/// nothing, rather than a stored answer this host can't read verified.
pub fn note_verified_here(
    koto_home: &Path,
    session_dir: &Path,
    holder: &str,
    body: &DecisionBody,
) -> std::io::Result<()> {
    use std::io::Write;
    let host = host_id(koto_home)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(session_dir.join(VERIFIED_HERE_FILE))?;
    file.write_all(format!("{}\n", note_line(&host, holder, body)).as_bytes())?;
    file.sync_all()
}

/// Whether this host checked the prompt of answer `body`, as held in the
/// session `holder` whose directory is `session_dir`. A copy that arrived
/// by import or by a pull, a session directory copied by hand, and a host
/// whose koto root holds no identity all read it remote.
pub fn verified_here(
    koto_home: Option<&Path>,
    session_dir: &Path,
    holder: &str,
    body: &DecisionBody,
) -> bool {
    use std::io::Read;
    let Some(host) = koto_home.and_then(read_host_id) else {
        return false;
    };
    let Some(file) = open_plain(&session_dir.join(VERIFIED_HERE_FILE)) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(MAX_NOTE_BYTES).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let want = note_line(&host, holder, body);
    String::from_utf8_lossy(&bytes).lines().any(|l| l == want)
}

/// The marker that holds one call position for one answer on this host.
/// While an answer is being written its process holds an exclusive `flock`
/// on the marker. Dropping it without [`UseMarker::keep`] removes it, so a
/// write that fails frees the position again.
pub struct UseMarker {
    path: Option<PathBuf>,
    _lock: std::fs::File,
}

impl UseMarker {
    /// Keep the marker: the answer citing it was written.
    pub fn keep(mut self) {
        self.path = None;
    }
}

impl Drop for UseMarker {
    fn drop(&mut self) {
        // Removed while the lock is still held, so a waiter that locks it
        // next finds the path gone and starts again.
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Whether `file` is still the file at `path`.
#[cfg(unix)]
fn same_file(file: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::symlink_metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Take the call position `pointer` names for one answer, host-wide:
/// `Ok(None)` when another answer on this host holds it.
///
/// A new marker is written and locked under a name of its own, then linked
/// into place create-only, so it is never visible unlocked while its answer
/// is being written. A marker that already exists is held when another
/// process has it locked (an answer being written now) or when `cited`
/// reports a stored answer on this host citing the position. Otherwise its
/// writer stopped before publishing anything: the marker is stale, and the
/// caller takes it over under the lock, so of two answers racing for a
/// stale marker the second waits for the first and then finds its answer.
pub fn take_use(
    koto_home: &Path,
    pointer: &Pointer,
    cited: impl Fn() -> bool,
) -> std::io::Result<Option<UseMarker>> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::OpenOptionsExt;
    let dir = koto_home.join(PROMPT_USES_DIR);
    crate::koto_home::ensure_private_root(&dir)?;
    let name = hex::encode(Sha256::digest(pointer.display().as_bytes()));
    let path = dir.join(&name);
    for _ in 0..16 {
        let tmp = dir.join(format!(
            ".{name}.{}.{}",
            std::process::id(),
            hex::encode(random_bytes::<8>()?)
        ));
        let fresh = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        if !crate::pool::store::try_flock(&fresh) {
            let _ = std::fs::remove_file(&tmp);
            return Err(std::io::Error::other("a new marker couldn't be locked"));
        }
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => {
                return Ok(Some(UseMarker {
                    path: Some(path),
                    _lock: fresh,
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        drop(fresh);
        let existing = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !crate::pool::store::try_flock(&existing) {
            return Ok(None);
        }
        if !same_file(&existing, &path) {
            continue;
        }
        if cited() {
            return Ok(None);
        }
        return Ok(Some(UseMarker {
            path: Some(path),
            _lock: existing,
        }));
    }
    Err(std::io::Error::other(
        "the marker kept changing under this answer",
    ))
}

/// The effective user's home directory from the account database, never
/// from `$HOME`. `None` when there is no entry or it names no absolute
/// directory.
#[cfg(unix)]
pub fn account_home() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: geteuid has no preconditions and can't fail.
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0u8; 4096];
    loop {
        // SAFETY: getpwuid_r writes only `pwd`, `result` and `buf`, which
        // is live for the length passed; every pointer read from `pwd`
        // afterwards points into `buf`.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        // SAFETY: pw_dir is a NUL-terminated string inside `buf`.
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) };
        let path = PathBuf::from(OsStr::from_bytes(dir.to_bytes()));
        return path.is_absolute().then_some(path);
    }
}

/// Without an account database there is no home koto trusts.
#[cfg(not(unix))]
pub fn account_home() -> Option<PathBuf> {
    None
}

/// Where Claude Code keeps each project's session records:
/// `<account home>/.claude/projects`.
pub fn projects_root() -> Option<PathBuf> {
    Some(account_home()?.join(".claude").join("projects"))
}

/// Check `pointer` for a person's answer recorded by a command running in
/// harness session `observed`, against the record under `root`, and return
/// the answer text the call's result holds for the pointer's position.
/// `recorded` is the harness session koto last recorded driving the
/// answering koto session, when it holds one; it must be `observed`.
pub fn verify(
    root: Option<&Path>,
    observed: Option<&str>,
    recorded: Option<&str>,
    pointer: &Pointer,
    expected: &Expected,
) -> Result<String, Check> {
    if observed != Some(pointer.harness.as_str()) {
        return Err(Check::Session);
    }
    if recorded.is_some_and(|r| r != pointer.harness) {
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
    main_line(&call)?;
    let item = call_item(&call, &pointer.call).ok_or(Check::Shape)?;
    match item.get("name").and_then(Value::as_str) {
        Some(QUESTION_TOOL) => {}
        Some(_) => return Err(Check::Tool),
        None => return Err(Check::Shape),
    }
    let made = call
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(crate::engine::wake::parse_rfc3339_millis)
        .ok_or(Check::Shape)?;
    match expected.asked {
        Some(asked) if made >= asked => {}
        _ => return Err(Check::Time),
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
    main_line(&result)?;
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
    // The reply must be the person's: an accepted decision from one of
    // the sources the captured records show for a reply the user gave.
    // Any other source, a hook's, a rule's or one not seen yet, is refused
    // rather than guessed at.
    if decision.get("decision").and_then(Value::as_str) != Some("accept")
        || !decision
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| USER_REPLY_SOURCES.contains(&s))
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

/// The permission sources a captured record shows on a reply the user
/// gave to a question prompt. Exact values only.
pub const USER_REPLY_SOURCES: &[&str] = &["user_temporary"];

/// A line of the harness session's own conversation: `isSidechain` is
/// `false`. A side conversation's line fails the `sidechain` check, and a
/// line without the flag the `shape` check.
fn main_line(line: &Value) -> Result<(), Check> {
    match line.get("isSidechain") {
        Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) => Err(Check::Sidechain),
        _ => Err(Check::Shape),
    }
}

/// The one record `<root>/<project>/<harness>.jsonl`: a regular file, not
/// a symlink, in a project directory that isn't one either, whose real
/// path lies under `root`'s. None, or more than one, fails the `record`
/// check.
fn find_record(root: &Path, harness: &str) -> Result<PathBuf, Check> {
    let real_root = std::fs::canonicalize(root).map_err(|_| Check::Record)?;
    let name = format!("{harness}.jsonl");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&real_root)
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
    if found.len() != 1 {
        return Err(Check::Record);
    }
    let record = std::fs::canonicalize(found.remove(0)).map_err(|_| Check::Record)?;
    if !record.starts_with(&real_root) {
        return Err(Check::Record);
    }
    Ok(record)
}

/// Stream the record line by line and return the line holding the call
/// and the line holding its result. Only lines that mention the call id
/// are parsed. The `shape` check fails on such a line that isn't valid
/// JSON, on a second line holding the call or a second holding its
/// result, on a result that comes before its call, and on a line or a
/// record past [`MAX_RECORD_LINE_BYTES`] or [`MAX_RECORD_BYTES`].
fn read_call(path: &Path, call: &str) -> Result<(Option<Value>, Option<Value>), Check> {
    use std::io::Read;
    let file = open_plain(path).ok_or(Check::Record)?;
    if file.metadata().map_err(|_| Check::Record)?.len() > MAX_RECORD_BYTES {
        return Err(Check::Shape);
    }
    let needle = format!("\"{call}\"");
    let needle = needle.as_bytes();
    let mut reader = BufReader::new(file.take(MAX_RECORD_BYTES + 1));
    let mut line = Vec::new();
    let mut total: u64 = 0;
    let (mut found_call, mut found_result) = (None, None);
    loop {
        line.clear();
        let n = (&mut reader)
            .take(MAX_RECORD_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| Check::Record)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_RECORD_BYTES
            || (line.len() > MAX_RECORD_LINE_BYTES && line.last() != Some(&b'\n'))
        {
            return Err(Check::Shape);
        }
        if !line.windows(needle.len()).any(|w| w == needle) {
            continue;
        }
        let value: Value = serde_json::from_slice(&line).map_err(|_| Check::Shape)?;
        if call_item(&value, call).is_some() {
            if found_call.is_some() {
                return Err(Check::Shape);
            }
            found_call = Some(value);
        } else if result_item(&value, call).is_some() {
            if found_result.is_some() || found_call.is_none() {
                return Err(Check::Shape);
            }
            found_result = Some(value);
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
        let e = Expected::from_content(ok, "s/1", "2026-01-01T00:00:00.000Z").unwrap();
        assert_eq!(e.question, "q");
        assert_eq!(e.labels, vec!["a", "b"]);
        for bad in [
            &b"not json"[..],
            br#"{"options":[]}"#,
            br#"{"question":"q"}"#,
            br#"{"question":"q","options":[{"description":"d"}]}"#,
        ] {
            assert_eq!(
                Expected::from_content(bad, "s/1", "2026-01-01T00:00:00.000Z").unwrap_err(),
                Check::Content
            );
        }
    }

    #[test]
    fn racing_first_uses_agree_on_one_complete_host_identity() {
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("koto");
            let ids: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..16)
                    .map(|_| s.spawn(|| host_id(&home).unwrap()))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(ids.iter().all(|i| *i == ids[0]));
            assert_eq!(read_host_id(&home), Some(ids[0]));
            let names: Vec<_> = std::fs::read_dir(&home)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(names, [HOST_ID_FILE], "no partial file is left behind");
        }
    }

    #[test]
    fn a_host_identity_others_may_write_is_not_used() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("koto");
        let id = host_id(&home).unwrap();
        let file = home.join(HOST_ID_FILE);
        for mode in [0o620, 0o602] {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(read_host_id(&home), None);
            assert!(host_id(&home).is_err());
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(host_id(&home).unwrap(), id);
    }

    #[test]
    fn a_marker_is_held_while_locked_or_cited_and_taken_over_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("koto");
        let pointer = Pointer::parse("ZQX-harness/toolu_ZQXMARKER/1").unwrap();
        // A live holder: refused, cited or not.
        let live = take_use(&home, &pointer, || false).unwrap().unwrap();
        assert!(take_use(&home, &pointer, || false).unwrap().is_none());
        // Kept after its answer was written: held while that answer is
        // stored, taken over once nothing cites the position.
        live.keep();
        assert!(take_use(&home, &pointer, || true).unwrap().is_none());
        let taken = take_use(&home, &pointer, || false).unwrap().unwrap();
        assert!(take_use(&home, &pointer, || false).unwrap().is_none());
        // Dropped: the position is free again.
        drop(taken);
        let names: Vec<_> = std::fs::read_dir(home.join(PROMPT_USES_DIR))
            .unwrap()
            .collect();
        assert!(names.is_empty());
        assert!(take_use(&home, &pointer, || true).unwrap().is_some());
    }
}
