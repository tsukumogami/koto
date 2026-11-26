//! The question-prompt fixtures under `tests/fixtures/prompt/`, and the
//! helpers that install one as a harness session's record.
//!
//! `accepted.jsonl` and `denied.jsonl` are structural copies of a real
//! accepted question-tool call and a real denied one: each is two lines,
//! the call and its result, with every field name and nesting kept and
//! every text value replaced by a marker string holding [`MARKER`]. The
//! harness session id is the placeholder `@HARNESS@` and each question's
//! header `@HEADER<n>@`, filled in when a test installs the record.

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

/// The fixture as record text, with `harness` and each `headers[n-1]`
/// filled in.
pub fn render(lines: &[Value], harness: &str, headers: &[&str]) -> String {
    let mut out = String::new();
    for l in lines {
        let mut s = l.to_string().replace("@HARNESS@", harness);
        for (i, h) in headers.iter().enumerate() {
            s = s.replace(&format!("@HEADER{}@", i + 1), h);
        }
        out.push_str(&s);
        out.push('\n');
    }
    out
}

/// The record path for `harness` under `home`, in project folder `project`.
pub fn record_path(home: &Path, project: &str, harness: &str) -> PathBuf {
    home.join(".claude/projects")
        .join(project)
        .join(format!("{harness}.jsonl"))
}

/// Append `text` to `harness`'s record under `home`.
pub fn install(home: &Path, project: &str, harness: &str, text: &str) -> PathBuf {
    use std::io::Write;
    let path = record_path(home, project, harness);
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
