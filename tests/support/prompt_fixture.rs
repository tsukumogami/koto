//! The question-prompt fixtures under `tests/fixtures/prompt/`, and the
//! helpers that install one as a harness session's record.
//!
//! `accepted.jsonl` and `denied.jsonl` are structural copies of a real
//! accepted question-tool call and a real denied one: each is two lines,
//! the call and its result, with every field name and nesting kept and
//! every text value replaced by a marker string holding [`MARKER`]. The
//! harness session id is the placeholder `@HARNESS@` and each question's
//! header `@HEADER<n>@`, filled in when a test installs the record; each
//! line's timestamp is set to the moment the record is rendered.
//!
//! koto reads a harness record only under the effective user's home from
//! the account database, never under `$HOME`, so a test installs its
//! records in the real `~/.claude/projects`, in a project folder of its own
//! named `koto-test-<tag>` that [`Records`] removes when dropped. Harness
//! session ids carry the same per-thread tag, so tests running at once
//! never find each other's records.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The call id every fixture uses.
pub const CALL: &str = "toolu_ZQXMARKERcall000000000001";

/// The string every fixture text value holds, which no koto output may.
pub const MARKER: &str = "ZQX-MARKER";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/prompt")
}

/// The fixture `name` (without `.jsonl`), one value per line.
pub fn lines(name: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(dir().join(format!("{name}.jsonl"))).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The call line's questions.
pub fn questions_mut(lines: &mut [Value]) -> &mut Vec<Value> {
    lines[0]["message"]["content"][0]["input"]["questions"]
        .as_array_mut()
        .unwrap()
}

/// The question text and option labels the call shows at `position`.
pub fn shown(lines: &[Value], position: usize) -> (String, Vec<String>) {
    let q = &lines[0]["message"]["content"][0]["input"]["questions"][position - 1];
    let labels = q["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap().to_string())
        .collect();
    (q["question"].as_str().unwrap().to_string(), labels)
}

/// The answer text the result holds for `position`.
pub fn answer_of(lines: &[Value], position: usize) -> String {
    let (question, _) = shown(lines, position);
    lines[1]["toolUseResult"]["answers"][question.as_str()]
        .as_str()
        .unwrap()
        .to_string()
}

/// Question content as koto stores it: the question and its labels.
pub fn content(question: &str, labels: &[String]) -> Vec<u8> {
    let options: Vec<Value> = labels
        .iter()
        .map(|l| serde_json::json!({"label": l, "description": "d"}))
        .collect();
    serde_json::to_vec(&serde_json::json!({"question": question, "options": options})).unwrap()
}

/// Change every occurrence of the call id, so one record can hold
/// several copies of a fixture.
pub fn with_call(lines: &[Value], call: &str) -> Vec<Value> {
    lines
        .iter()
        .map(|l| serde_json::from_str(&l.to_string().replace(CALL, call)).unwrap())
        .collect()
}

/// The time now as koto and the harness write it: UTC, milliseconds.
pub fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// The fixture as record text, with `harness` and each `headers[n-1]`
/// filled in and every line's timestamp set to now.
pub fn render(lines: &[Value], harness: &str, headers: &[&str]) -> String {
    render_at(lines, harness, headers, &now())
}

/// [`render`], with every line's timestamp set to `at`.
pub fn render_at(lines: &[Value], harness: &str, headers: &[&str], at: &str) -> String {
    let mut out = String::new();
    for l in lines {
        let mut l = l.clone();
        if l.get("timestamp").is_some() {
            l["timestamp"] = Value::String(at.to_string());
        }
        let mut s = l.to_string().replace("@HARNESS@", harness);
        for (i, h) in headers.iter().enumerate() {
            s = s.replace(&format!("@HEADER{}@", i + 1), h);
        }
        out.push_str(&s);
        out.push('\n');
    }
    out
}

/// The root koto reads records under: the account home's
/// `.claude/projects`, as koto itself resolves it.
pub fn records_root() -> PathBuf {
    koto::decisions::prompt::projects_root().expect("the account database names no home")
}

thread_local! {
    static TAG: String = {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        format!(
            "{}-{}-{nanos:x}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    };
}

fn tag() -> String {
    TAG.with(Clone::clone)
}

/// This thread's harness session id.
pub fn harness() -> String {
    format!("koto-test-{}-a", tag())
}

/// A second harness session id for this thread.
pub fn other_harness() -> String {
    format!("koto-test-{}-b", tag())
}

/// This thread's project folder under [`records_root`].
pub fn project() -> String {
    format!("koto-test-{}", tag())
}

/// This thread's project folder, emptied when made and removed when
/// dropped: a test that makes a second environment starts it with no
/// records from the first.
pub struct Records(());

impl Records {
    pub fn new() -> Self {
        let _ = std::fs::remove_dir_all(records_root().join(project()));
        Records(())
    }
}

impl Drop for Records {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(records_root().join(project()));
    }
}

/// The record path for `harness` in this thread's project folder.
pub fn record_path(harness: &str) -> PathBuf {
    records_root()
        .join(project())
        .join(format!("{harness}.jsonl"))
}

/// Append `text` to `harness`'s record in this thread's project folder.
pub fn install(harness: &str, text: &str) -> PathBuf {
    use std::io::Write;
    let path = record_path(harness);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
    path
}

/// The hash an escalate's `packet` carries for `bytes`.
pub fn packet(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
