//! Integration tests for the question content area, `koto/packets/`
//! (PRD R69 to R71): a question's content is written once, at most 16 KiB,
//! hashing to the escalate's `packet` subject, and read back byte for byte
//! by the question's identity.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::path::PathBuf;

use assert_fs::TempDir;
use serde_json::Value;

const CLEARED_ENV: [&str; 5] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
];

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

struct Env {
    _tmp: TempDir,
    root: PathBuf,
}

struct Out {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

impl Out {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.stdout)
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.text(), self.stderr))
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "other-ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        let env = Env { _tmp: tmp, root };
        for s in ["raiser", "reader"] {
            let t = env.root.join("work.md");
            env.ok(&["init", s, "--template", t.to_str().unwrap()]);
        }
        env
    }

    fn run_in(&self, cwd: &str, harness: Option<&str>, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        if let Some(h) = harness {
            cmd.env("CLAUDE_CODE_SESSION_ID", h);
        }
        cmd.current_dir(self.root.join(cwd));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.root.join("sessions"));
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_in("ws", None, args)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(out.code, 0, "{args:?}\n{}\n{}", out.text(), out.stderr);
        out
    }

    /// Raise a question in `raiser` whose `packet` is `packet`; returns
    /// its id and identity.
    fn raise(&self, packet: &str) -> (String, String) {
        let p = format!("packet={packet}");
        let out = self
            .ok(&[
                "decisions",
                "record",
                "raiser",
                "--kind",
                "escalate",
                "--outcome",
                "escalated",
                "--subject",
                "question=q",
                "--subject",
                "text=which way?",
                "--subject",
                "recipient=person:owner",
                "--subject",
                &p,
                "--by",
                "session",
            ])
            .json();
        let id = out["id"].as_str().unwrap().to_string();
        let sid = out["decision"]["session"]["id"].as_str().unwrap();
        (id.clone(), format!("{sid}/{id}"))
    }

    fn write(&self, id: &str, bytes: &[u8]) -> Out {
        let file = self.root.join("content.bin");
        std::fs::write(&file, bytes).unwrap();
        self.run(&[
            "decisions",
            "content",
            "write",
            "raiser",
            id,
            "--file",
            file.to_str().unwrap(),
        ])
    }

    fn stored(&self, id: &str) -> PathBuf {
        self.root.join("sessions/raiser/ctx/koto/packets").join(id)
    }
}

#[test]
fn content_of_exactly_16_kib_is_accepted_and_one_byte_more_is_refused() {
    let env = Env::new();
    let exact = vec![b'a'; 16 * 1024];
    let (id, _) = env.raise(&prompt_fixture::packet(&exact));
    let out = env.write(&id, &exact);
    assert_eq!(out.code, 0, "{}\n{}", out.text(), out.stderr);
    assert_eq!(out.json()["bytes"], 16384);
    assert_eq!(std::fs::read(env.stored(&id)).unwrap(), exact);

    let over = vec![b'a'; 16 * 1024 + 1];
    let (id, _) = env.raise(&prompt_fixture::packet(&over));
    let out = env.write(&id, &over);
    assert_eq!(out.code, 2, "{}", out.text());
    assert!(out.json()["error"].as_str().unwrap().contains("16 KiB"));
    assert!(!env.stored(&id).exists());
}

#[test]
fn a_second_write_is_refused_and_the_first_bytes_stay() {
    let env = Env::new();
    let bytes = b"{\"question\":\"q\",\"options\":[]}\n".to_vec();
    let (id, identity) = env.raise(&prompt_fixture::packet(&bytes));
    assert_eq!(env.write(&id, &bytes).code, 0);
    for again in [bytes.clone(), b"other bytes".to_vec()] {
        let out = env.write(&id, &again);
        assert_ne!(out.code, 0, "{}", out.text());
    }
    let out = env.ok(&["decisions", "content", "read", &identity]);
    assert_eq!(out.stdout, bytes);
}

#[test]
fn the_context_store_refuses_a_key_under_koto_packets() {
    let env = Env::new();
    let input = env.root.join("input");
    std::fs::write(&input, b"x").unwrap();
    let key = "koto/packets/000000000001";
    let out = env.run(&[
        "context",
        "add",
        "raiser",
        key,
        "--from-file",
        input.to_str().unwrap(),
    ]);
    assert_ne!(out.code, 0, "{}", out.text());
    assert!(out.text().contains(key) || out.stderr.contains(key));
    assert!(!env.stored("000000000001").exists());
    let out = env.run(&["context", "remove", "raiser", key]);
    assert_ne!(out.code, 0);
}

#[test]
fn the_read_from_another_session_prints_the_bytes_written() {
    let env = Env::new();
    for bytes in [
        b"{\"question\":\"with newline\"}\n".to_vec(),
        b"{\"question\":\"without newline\"}".to_vec(),
        vec![0u8, 255, 10, 13, 10],
    ] {
        let (id, identity) = env.raise(&prompt_fixture::packet(&bytes));
        assert_eq!(env.write(&id, &bytes).code, 0);
        // Run from another directory, by a command bound to another
        // session's harness: the read needs only the identity.
        let out = env.run_in(
            "other-ws",
            Some("reader-harness"),
            &["decisions", "content", "read", &identity],
        );
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert_eq!(out.stdout, bytes);
    }
}

#[test]
fn the_escalates_packet_is_the_hash_and_a_mismatch_is_refused() {
    let env = Env::new();
    let bytes = b"the content".to_vec();
    let packet = prompt_fixture::packet(&bytes);
    let (id, identity) = env.raise(&packet);
    let shown = env.ok(&["decisions", "show", "raiser", &id]).json();
    assert_eq!(shown["decision"]["subject"]["packet"], packet.as_str());
    assert_eq!(shown["prompt_header"].as_str().unwrap().len(), 12);

    let out = env.write(&id, b"other content");
    assert_eq!(out.code, 2, "{}", out.text());
    assert!(!env.stored(&id).exists());
    assert_eq!(env.write(&id, &bytes).code, 0);
    let read = env.ok(&["decisions", "content", "read", &identity]);
    assert_eq!(prompt_fixture::packet(&read.stdout), packet);

    // A packet that isn't a hash is refused at the escalate.
    let out = env.run(&[
        "decisions",
        "record",
        "raiser",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q",
        "--subject",
        "text=t",
        "--subject",
        "recipient=person:owner",
        "--subject",
        "packet=sha256:ABC",
        "--by",
        "session",
    ]);
    assert_eq!(out.code, 2, "{}", out.text());
    // An escalate with no packet takes no content.
    let out = env.ok(&[
        "decisions",
        "record",
        "raiser",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q2",
        "--subject",
        "text=t",
        "--subject",
        "recipient=person:owner",
        "--by",
        "session",
    ]);
    let plain = out.json()["id"].as_str().unwrap().to_string();
    assert_eq!(env.write(&plain, &bytes).code, 2);
}
