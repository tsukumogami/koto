//! A person's verified answer read from a copy of its session that this
//! host didn't record it in (PRD R61): a copy brought in by `koto session
//! import`, and one brought down by a pull from shared storage. Both read
//! it as a remote session's answer, though the entry's stored bytes are
//! the original's; the host that recorded it still reads it verified after
//! its session has synced. Runs against the in-process S3 endpoint in
//! `support/fake_s3.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use prompt_fixture as fx;
use serde_json::Value;

const BUCKET: &str = "koto-person-answer";
const TEMPLATE_FILE: &str = "work.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const HARNESS: &str = "harness-a";
const RAISER: &str = "raiser";
const ANSWERER: &str = "answerer";

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

fn workspace(dir: PathBuf) -> PathBuf {
    let ws = mk(dir);
    std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
    ws
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

/// One simulated host: its own `HOME` and template cache.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: String,
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        Host {
            _root: root,
            home,
            cache,
            endpoint: s3.endpoint(),
        }
    }

    fn run(&self, ws: &Path, harness: Option<&str>, args: &[&str]) -> Out {
        let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
        let mut cmd = koto_command::koto_std();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", format!("{}:{SYSTEM_PATH}", koto_dir.display()))
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .args(args);
        if let Some(h) = harness {
            cmd.env("CLAUDE_CODE_SESSION_ID", h);
        }
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, ws: &Path, harness: Option<&str>, args: &[&str]) -> Value {
        let out = self.run(ws, harness, args);
        assert_eq!(
            out.code,
            Some(0),
            "{args:?}\n{}\n{}",
            out.stdout,
            out.stderr
        );
        out.json()
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.home.join(".koto/sessions").join(name)
    }
}

/// The answer as recorded: its id, and the path of its file relative to
/// the answering session's directory.
struct Answer {
    id: String,
    rel: PathBuf,
}

/// On `host` in `ws`: raise a question in the raiser, store its content,
/// install the accepted fixture as harness A's record and record a person's
/// answer from the answerer citing it.
fn record_verified_answer(host: &Host, ws: &Path) -> Answer {
    for name in [RAISER, ANSWERER] {
        host.ok(ws, None, &["init", name, "--template", TEMPLATE_FILE]);
    }
    let lines = fx::lines("accepted");
    let (question, labels) = fx::shown(&lines, 1);
    let content = fx::content(&question, &labels);
    let packet = format!("packet={}", fx::packet(&content));
    let q = host.ok(
        ws,
        None,
        &[
            "decisions",
            "record",
            RAISER,
            "--kind",
            "escalate",
            "--outcome",
            "escalated",
            "--subject",
            "question=which-way",
            "--subject",
            "text=which way?",
            "--subject",
            "recipient=person:owner",
            "--subject",
            &packet,
            "--by",
            "session",
        ],
    );
    let qid = q["id"].as_str().unwrap().to_string();
    let qsid = q["decision"]["session"]["id"].as_str().unwrap().to_string();
    let file = ws.join("content.json");
    std::fs::write(&file, &content).unwrap();
    let written = host.ok(
        ws,
        None,
        &[
            "decisions",
            "content",
            "write",
            RAISER,
            &qid,
            "--file",
            file.to_str().unwrap(),
        ],
    );
    let header = written["prompt_header"].as_str().unwrap();
    fx::install(
        &host.home,
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[header]),
    );
    let pointer = format!("{HARNESS}/{}/1", fx::CALL);
    let answer = host.ok(
        ws,
        Some(HARNESS),
        &[
            "decisions",
            "answer",
            RAISER,
            &qid,
            "--in",
            ANSWERER,
            "--by",
            "person:owner",
            "--prompt",
            &pointer,
        ],
    );
    let id = answer["id"].as_str().unwrap().to_string();
    let shown = host.ok(ws, None, &["decisions", "show", ANSWERER, &id]);
    assert_eq!(shown["reading"], "verified");
    assert_eq!(shown["decision"]["prompt"]["mark"]["value"], "verified");
    assert_eq!(shown["decision"]["prompt"]["mark"]["source"], "observed");
    Answer {
        id,
        rel: PathBuf::from("ctx/koto/aimed")
            .join(qsid)
            .join(answer["id"].as_str().unwrap()),
    }
}

fn assert_remote(shown: &Value) {
    assert_eq!(shown["reading"], "a remote session's answer", "{shown}");
    assert_eq!(shown["decision"]["prompt"]["mark"]["value"], "remote");
    assert_eq!(shown["decision"]["prompt"]["mark"]["source"], "observed");
    assert_ne!(shown["reading"], "verified");
}

#[test]
fn an_imported_copy_reads_the_answer_as_a_remote_sessions_answer() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let shared = tempfile::TempDir::new().unwrap();
    let ws_a = workspace(shared.path().join("ws-a"));
    let answer = record_verified_answer(&first, &ws_a);

    let second = Host::new(&s3);
    let ws_b = workspace(shared.path().join("ws-b"));
    let compiled = second.run(&ws_b, None, &["template", "compile", TEMPLATE_FILE]);
    assert_eq!(compiled.code, Some(0), "{}", compiled.stderr);
    second.ok(
        &ws_b,
        None,
        &[
            "session",
            "import",
            ANSWERER,
            "--from",
            ws_a.to_str().unwrap(),
        ],
    );
    let original = std::fs::read(first.session_dir(ANSWERER).join(&answer.rel)).unwrap();
    let imported = std::fs::read(second.session_dir(ANSWERER).join(&answer.rel)).unwrap();
    assert_eq!(original, imported, "the entry's stored bytes differ");
    // The host-local note that marks an answer verified here stays behind:
    // it sits outside the copy's context and manifest.
    assert!(first
        .session_dir(ANSWERER)
        .join("koto-verified-here")
        .is_file());
    assert!(!second
        .session_dir(ANSWERER)
        .join("koto-verified-here")
        .exists());

    assert_remote(&second.ok(&ws_b, None, &["decisions", "show", ANSWERER, &answer.id]));
    let listed = second.ok(&ws_b, None, &["decisions", "list", ANSWERER, "--format", "v1"]);
    let entry = listed["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == answer.id.as_str())
        .unwrap_or_else(|| panic!("the imported copy lists no answer: {listed}"))
        .clone();
    assert_remote(&entry);
}

#[test]
fn a_pulled_copy_reads_remote_and_the_recording_host_still_reads_verified() {
    let s3 = FakeS3::start(BUCKET);
    let shared = tempfile::TempDir::new().unwrap();
    let ws = workspace(shared.path().join("ws"));
    let first = Host::new(&s3);
    let answer = record_verified_answer(&first, &ws);

    let second = Host::new(&s3);
    let pulled = second.run(
        &ws,
        None,
        &["session", "resolve", ANSWERER, "--keep", "remote"],
    );
    assert_eq!(pulled.code, Some(0), "{}\n{}", pulled.stdout, pulled.stderr);
    let original = std::fs::read(first.session_dir(ANSWERER).join(&answer.rel)).unwrap();
    let copy = std::fs::read(second.session_dir(ANSWERER).join(&answer.rel)).unwrap();
    assert_eq!(original, copy, "the entry's stored bytes differ");
    assert_remote(&second.ok(&ws, None, &["decisions", "show", ANSWERER, &answer.id]));

    // The recording host, after its session has synced (and pulled back
    // what the bucket holds), still reads the answer verified.
    let pulled = first.run(
        &ws,
        None,
        &["session", "resolve", ANSWERER, "--keep", "remote"],
    );
    assert_eq!(pulled.code, Some(0), "{}\n{}", pulled.stdout, pulled.stderr);
    let shown = first.ok(&ws, None, &["decisions", "show", ANSWERER, &answer.id]);
    assert_eq!(shown["reading"], "verified");
    assert_eq!(shown["decision"]["prompt"]["mark"]["value"], "verified");
}
