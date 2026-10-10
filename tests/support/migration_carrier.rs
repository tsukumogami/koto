//! The session-migration carrier: one session moved between simulated
//! hosts by `koto session import`, step by step.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/migration_carrier.rs"]
//! mod migration_carrier;
//! ```
//!
//! A simulated host is a [`Host`]: its own `HOME` (so its own session store
//! and machine config), its own `XDG_CACHE_HOME` (so its own template
//! cache) and its own workspace directory, whose `.koto/config.toml` points
//! the cloud backend at a shared bucket. Hosts share nothing else.
//!
//! The steps are functions on [`Carrier`] so a test can put its own checks
//! between them; `tests/session_migration_test.rs` runs them against an
//! in-process endpoint and `tests/cloud_integration_test.rs` against a real
//! bucket. Every failure names its step: `[<step>] <what went wrong>`.

#![allow(dead_code)]

use assert_cmd::Command;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The carrier's template: a gated first state, a state waiting on
/// evidence, a second evidence state, and a terminal. The gate reads a
/// context key, so passing it on the importing host shows the keys arrived.
pub const TEMPLATE: &str = r#"---
name: migration-carrier
version: "1.0"
initial_state: start
states:
  start:
    gates:
      notes:
        type: context-exists
        key: notes.md
    transitions:
      - target: wait
        when:
          gates.notes.exists: true
  wait:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: review
        when:
          choice: go
  review:
    accepts:
      verdict:
        type: enum
        required: true
        values: [approve]
    transitions:
      - target: done
        when:
          verdict: approve
  done:
    terminal: true
---

## start

Gather notes.

## wait

Wait for the go-ahead.

## review

Review the work.

## done

Done.
"#;

/// File name the template is written under in every workspace.
pub const TEMPLATE_FILE: &str = "carrier.md";

/// How long the carrier's import step may take, compile included.
pub const IMPORT_STEP_LIMIT: Duration = Duration::from_secs(30);

/// Where the cloud backend points, shared by every host.
#[derive(Debug, Clone)]
pub struct Cloud {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub path_style: bool,
    /// Credentials to pass as `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`;
    /// `None` inherits the test process's.
    pub credentials: Option<(String, String)>,
}

/// Fail the current step, naming it.
pub fn fail(step: &str, msg: impl std::fmt::Display) -> ! {
    panic!("[{step}] {msg}")
}

/// Fail the current step unless `cond` holds.
pub fn ensure(step: &str, cond: bool, msg: impl std::fmt::Display) {
    if !cond {
        fail(step, msg);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The remote prefix koto derives for a workspace: the first 16 hex digits
/// of the SHA-256 of its canonical path.
pub fn prefix_of(dir: &Path) -> String {
    let canonical = std::fs::canonicalize(dir).expect("canonicalize workspace");
    sha256_hex(canonical.to_string_lossy().as_bytes())[..16].to_string()
}

/// The result of one koto invocation.
#[derive(Debug)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// stdout as JSON (the whole of it, else its last non-empty line), or
    /// `Null`.
    pub json: serde_json::Value,
}

impl Run {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// One line for a failure message.
    pub fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// One simulated host.
pub struct Host {
    pub label: String,
    pub home: PathBuf,
    pub cache: PathBuf,
    /// The workspace, canonical.
    pub ws: PathBuf,
    credentials: Option<(String, String)>,
}

impl Host {
    /// Lay out a host under `root/<label>` and write its workspace's cloud
    /// config and a copy of the carrier template.
    pub fn new(root: &Path, label: &str, cloud: &Cloud) -> Host {
        let base = root.join(label);
        let home = base.join("home");
        let cache = base.join("cache");
        let ws = base.join("ws");
        for d in [&home, &cache, &ws] {
            std::fs::create_dir_all(d).unwrap();
        }
        let ws = std::fs::canonicalize(&ws).unwrap();
        let home = std::fs::canonicalize(&home).unwrap();
        let cache = std::fs::canonicalize(&cache).unwrap();

        std::fs::create_dir_all(ws.join(".koto")).unwrap();
        let mut config = format!(
            "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nendpoint = {:?}\n\
             bucket = {:?}\nregion = {:?}\n",
            cloud.endpoint, cloud.bucket, cloud.region
        );
        if cloud.path_style {
            config.push_str("path_style = true\n");
        }
        std::fs::write(ws.join(".koto").join("config.toml"), config).unwrap();
        std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();

        Host {
            label: label.to_string(),
            home,
            cache,
            ws,
            credentials: cloud.credentials.clone(),
        }
    }

    pub fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        cmd.current_dir(&self.ws)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE");
        if let Some((key, secret)) = &self.credentials {
            cmd.env("AWS_ACCESS_KEY_ID", key)
                .env("AWS_SECRET_ACCESS_KEY", secret);
        }
        cmd
    }

    pub fn koto(&self, args: &[&str]) -> Run {
        let output = self.cmd().args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
            let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
            serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
        });
        Run {
            code: output.status.code(),
            stdout,
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            json,
        }
    }

    /// This host's remote prefix.
    pub fn prefix(&self) -> String {
        prefix_of(&self.ws)
    }

    pub fn ws_str(&self) -> String {
        self.ws.to_string_lossy().into_owned()
    }

    pub fn session_dir(&self, name: &str) -> PathBuf {
        self.home.join(".koto").join("sessions").join(name)
    }

    pub fn state_path(&self, name: &str) -> PathBuf {
        self.session_dir(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    /// Non-empty lines of the local state file.
    pub fn state_lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    pub fn header(&self, name: &str) -> serde_json::Value {
        self.state_lines(name)
            .first()
            .map(|l| serde_json::from_str(l).unwrap())
            .unwrap_or(serde_json::Value::Null)
    }

    /// The local context manifest, as JSON.
    pub fn manifest(&self, name: &str) -> serde_json::Value {
        std::fs::read(self.session_dir(name).join("ctx").join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

/// The five context keys the carrier moves: text of several sizes and
/// binary content, one of them under a namespace.
pub fn carrier_keys() -> Vec<(&'static str, Vec<u8>)> {
    let binary: Vec<u8> = (0u32..4096).map(|i| (i * 31 % 256) as u8).collect();
    let large: Vec<u8> = "line of research findings\n".repeat(8000).into_bytes();
    vec![
        ("notes.md", b"# Notes\n\nGathered on host A.\n".to_vec()),
        ("plan.md", b"1. move\n2. continue\n".to_vec()),
        ("research/findings.txt", large),
        ("data/blob.bin", binary),
        ("one.txt", b"x".to_vec()),
    ]
}

/// The carrier between three hosts: A creates the session, B imports it
/// from A, C imports it from B.
pub struct Carrier<'a> {
    pub a: &'a Host,
    pub b: &'a Host,
    pub c: &'a Host,
    pub name: String,
    /// SHA-256 of each key as A stored it, filled by [`Carrier::keys_a`].
    pub key_hashes: BTreeMap<String, String>,
    /// How long each step took.
    pub timings: Vec<(&'static str, Duration)>,
}

impl<'a> Carrier<'a> {
    pub fn new(a: &'a Host, b: &'a Host, c: &'a Host, name: &str) -> Self {
        Carrier {
            a,
            b,
            c,
            name: name.to_string(),
            key_hashes: BTreeMap::new(),
            timings: Vec::new(),
        }
    }

    /// Run one step, timing it. A panic inside the step that doesn't
    /// already name it (an `unwrap`, an index) is re-raised with the step's
    /// name, so every failure says which step it was.
    fn timed<T>(&mut self, step: &'static str, f: impl FnOnce(&mut Self) -> T) -> T {
        let started = Instant::now();
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        let out = match out {
            Ok(out) => out,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()));
                match msg {
                    Some(m) if m.starts_with(&format!("[{step}]")) => {
                        std::panic::resume_unwind(payload)
                    }
                    Some(m) => fail(step, m),
                    None => fail(step, "panicked"),
                }
            }
        };
        self.timings.push((step, started.elapsed()));
        out
    }

    /// `init-a`: start the session in A.
    pub fn init_a(&mut self) {
        self.timed("init-a", |c| {
            let step = "init-a";
            let template = c.a.ws.join(TEMPLATE_FILE);
            let run =
                c.a.koto(&["init", &c.name, "--template", template.to_str().unwrap()]);
            ensure(
                step,
                run.ok(),
                format!("koto init failed: {}", run.describe()),
            );
            let header = c.a.header(&c.name);
            ensure(
                step,
                header["workflow"] == c.name.as_str(),
                format!("A has no local state file for {}: {}", c.name, header),
            );
        })
    }

    /// `keys-a`: add five keys in A, binary included, and record their
    /// hashes.
    pub fn keys_a(&mut self) {
        self.timed("keys-a", |c| {
            let step = "keys-a";
            let scratch = c.a.home.join("key-sources");
            std::fs::create_dir_all(&scratch).unwrap();
            for (i, (key, bytes)) in carrier_keys().into_iter().enumerate() {
                let file = scratch.join(format!("key-{}", i));
                std::fs::write(&file, &bytes).unwrap();
                let run = c.a.koto(&[
                    "context",
                    "add",
                    &c.name,
                    key,
                    "--from-file",
                    file.to_str().unwrap(),
                ]);
                ensure(
                    step,
                    run.ok(),
                    format!("koto context add {} failed: {}", key, run.describe()),
                );
                c.key_hashes.insert(key.to_string(), sha256_hex(&bytes));
            }
            let manifest = c.a.manifest(&c.name);
            for (key, hash) in &c.key_hashes {
                ensure(
                    step,
                    manifest["keys"][key]["hash"] == hash.as_str(),
                    format!(
                        "A's manifest doesn't record {} as {}: {}",
                        key, hash, manifest
                    ),
                );
            }
        })
    }

    /// The template's hash as A's session recorded it.
    pub fn template_hash(&self) -> String {
        self.a.header(&self.name)["template_hash"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Compile the carrier template on `host`, as an operator would before
    /// importing, and check it lands under the session's hash.
    pub fn compile_on(&self, step: &str, host: &Host) {
        let template = host.ws.join(TEMPLATE_FILE);
        let run = host.koto(&["template", "compile", template.to_str().unwrap()]);
        ensure(
            step,
            run.ok(),
            format!("koto template compile failed: {}", run.describe()),
        );
        let expected = host
            .cache
            .join("koto")
            .join(format!("{}.json", self.template_hash()));
        ensure(
            step,
            run.stdout.trim() == expected.to_string_lossy(),
            format!(
                "compile wrote {} rather than {}",
                run.stdout.trim(),
                expected.display()
            ),
        );
    }

    /// Run the import of the carrier session on `to` from `from` and check
    /// its output.
    fn import(&self, step: &str, to: &Host, from: &Host) -> Run {
        let run = to.koto(&["session", "import", &self.name, "--from", &from.ws_str()]);
        ensure(
            step,
            run.ok(),
            format!("koto session import failed: {}", run.describe()),
        );
        let want = serde_json::json!({
            "name": self.name,
            "imported": true,
            "from": {"workspace": from.ws_str(), "session": self.name},
            "keys": self.key_hashes.len(),
            "template": "local-cache",
            "marked": true,
        });
        ensure(
            step,
            run.json == want,
            format!("import output {} is not {}", run.json, want),
        );
        run
    }

    /// Check the session `to` imported from `from`: the source's events
    /// verbatim, one `session_imported` event, and a header that belongs
    /// to `to`.
    fn check_imported_log(&self, step: &str, to: &Host, from: &Host, source_lines: &[String]) {
        let lines = to.state_lines(&self.name);
        ensure(
            step,
            lines.len() == source_lines.len() + 1,
            format!(
                "{} has {} lines; the source had {} (+1 expected)",
                to.label,
                lines.len(),
                source_lines.len()
            ),
        );
        ensure(
            step,
            lines[1..source_lines.len()] == source_lines[1..],
            format!("{}'s events are not the source's, verbatim", to.label),
        );

        let source_header: serde_json::Value = serde_json::from_str(&source_lines[0]).unwrap();
        let header = to.header(&self.name);
        let ws = to.ws_str();
        let store = to.home.join(".koto").join("sessions");
        let checks = [
            ("workflow", header["workflow"] == self.name.as_str()),
            (
                "a new session_id",
                header["session_id"].as_str().is_some_and(|s| !s.is_empty())
                    && header["session_id"] != source_header["session_id"],
            ),
            ("execution_dir", header["execution_dir"] == ws.as_str()),
            ("origin.anchor", header["origin"]["anchor"] == ws.as_str()),
            (
                "origin.store.kind",
                header["origin"]["store"]["kind"] == "cloud",
            ),
            (
                "origin.store.base",
                header["origin"]["store"]["base"] == store.to_string_lossy().as_ref(),
            ),
            (
                "command_environment.home",
                header["command_environment"]["home"] == to.home.to_string_lossy().as_ref(),
            ),
            (
                "template_hash",
                header["template_hash"] == source_header["template_hash"],
            ),
        ];
        for (what, ok) in checks {
            ensure(
                step,
                ok,
                format!("{}'s header has the wrong {}: {}", to.label, what, header),
            );
        }

        let imported: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        let last_seq =
            serde_json::from_str::<serde_json::Value>(&source_lines[source_lines.len() - 1])
                .unwrap()["seq"]
                .as_u64()
                .unwrap();
        ensure(
            step,
            imported["type"] == "session_imported"
                && imported["seq"] == last_seq + 1
                && imported["payload"]["from_workspace"] == from.ws_str().as_str()
                && imported["payload"]["from_session"] == self.name.as_str()
                && imported["payload"]["from_session_id"] == source_header["session_id"]
                && imported["payload"]["machine_id"]
                    .as_str()
                    .is_some_and(|m| !m.is_empty()),
            format!(
                "the last event is not the expected session_imported: {}",
                imported
            ),
        );

        let template = to
            .session_dir(&self.name)
            .join(format!("{}.json", self.template_hash()));
        let bytes = std::fs::read(&template).unwrap_or_default();
        ensure(
            step,
            sha256_hex(&bytes) == self.template_hash(),
            format!("{} doesn't hold the compiled template", template.display()),
        );
    }

    /// `import-b`: compile the template in B, then import from A.
    ///
    /// A's template cache is removed first: on separate machines B could
    /// never read it, and on this one it would let B's first tick read the
    /// path A's log records instead of the copy in B's session directory.
    ///
    /// The step must finish within [`IMPORT_STEP_LIMIT`].
    pub fn import_b(&mut self) -> Run {
        self.timed("import-b", |c| {
            let step = "import-b";
            let started = Instant::now();
            let _ = std::fs::remove_dir_all(c.a.cache.join("koto"));
            c.compile_on(step, c.b);
            let source_lines = c.a.state_lines(&c.name);
            let run = c.import(step, c.b, c.a);
            c.check_imported_log(step, c.b, c.a, &source_lines);
            let took = started.elapsed();
            ensure(
                step,
                took < IMPORT_STEP_LIMIT,
                format!("the step took {:?}, over {:?}", took, IMPORT_STEP_LIMIT),
            );
            run
        })
    }

    /// `keys-b`: read every key in B and compare its SHA-256, and its
    /// manifest record, with A's.
    pub fn keys_b(&mut self) {
        self.timed("keys-b", |c| {
            let step = "keys-b";
            let a_manifest = c.a.manifest(&c.name);
            let b_manifest = c.b.manifest(&c.name);
            let out = c.b.home.join("key-out");
            for (key, hash) in &c.key_hashes {
                for field in ["hash", "size", "writer"] {
                    ensure(
                        step,
                        b_manifest["keys"][key][field] == a_manifest["keys"][key][field],
                        format!(
                            "{}'s {} differs: A {} B {}",
                            key, field, a_manifest["keys"][key], b_manifest["keys"][key]
                        ),
                    );
                }
                let _ = std::fs::remove_file(&out);
                let run = c.b.koto(&[
                    "context",
                    "get",
                    &c.name,
                    key,
                    "--to-file",
                    out.to_str().unwrap(),
                ]);
                ensure(
                    step,
                    run.ok(),
                    format!("koto context get {} failed: {}", key, run.describe()),
                );
                let got = sha256_hex(&std::fs::read(&out).unwrap_or_default());
                ensure(
                    step,
                    &got == hash,
                    format!("{} reads back as {} in B, A stored {}", key, got, hash),
                );
            }
        })
    }

    /// `advance-b`: tick B past the gate and past the waiting state.
    pub fn advance_b(&mut self) {
        self.timed("advance-b", |c| {
            let step = "advance-b";
            let run = c.b.koto(&["next", &c.name]);
            ensure(
                step,
                run.ok(),
                format!("first koto next failed: {}", run.describe()),
            );
            ensure(
                step,
                run.json["state"] == "wait",
                format!("the gate on notes.md didn't pass in B: {}", run.describe()),
            );
            let run =
                c.b.koto(&["next", &c.name, "--with-data", r#"{"choice":"go"}"#]);
            ensure(
                step,
                run.ok(),
                format!("koto next with evidence failed: {}", run.describe()),
            );
            ensure(
                step,
                run.json["state"] == "review",
                format!("B didn't advance past wait: {}", run.describe()),
            );
        })
    }

    /// `refuse-a`: A's copy now refuses, names B, and is left as it was.
    pub fn refuse_a(&mut self) {
        self.timed("refuse-a", |c| {
            let step = "refuse-a";
            let before = std::fs::read(c.a.state_path(&c.name)).unwrap();

            let run = c.a.koto(&["next", &c.name]);
            ensure(
                step,
                run.code == Some(2),
                format!("koto next in A: {}", run.describe()),
            );
            ensure(
                step,
                run.json["error"]["code"] == "session_migrated",
                format!("koto next in A: {}", run.describe()),
            );
            let message = run.json["error"]["message"].as_str().unwrap_or_default();
            ensure(
                step,
                message.starts_with("session_migrated:")
                    && message.contains(&format!("'{}'", c.name))
                    && message.contains(&c.b.ws_str()),
                format!(
                    "the refusal doesn't name B's session and workspace: {}",
                    message
                ),
            );

            let run = c.a.koto(&["status", &c.name]);
            ensure(
                step,
                run.code == Some(2),
                format!("koto status in A: {}", run.describe()),
            );
            ensure(
                step,
                run.json["error"].as_str() == Some(message),
                format!("koto status in A gave another message: {}", run.describe()),
            );

            ensure(
                step,
                std::fs::read(c.a.state_path(&c.name)).unwrap() == before,
                "A's local state file changed",
            );
        })
    }

    /// `reimport-c`: import from B into C; C carries both imports and
    /// stands where B left off, and B's copy refuses naming C.
    pub fn reimport_c(&mut self) {
        self.timed("reimport-c", |c| {
            let step = "reimport-c";
            c.compile_on(step, c.c);
            let source_lines = c.b.state_lines(&c.name);
            c.import(step, c.c, c.b);
            c.check_imported_log(step, c.c, c.b, &source_lines);

            let imports =
                c.c.state_lines(&c.name)
                    .iter()
                    .filter(|l| l.contains("\"type\":\"session_imported\""))
                    .count();
            ensure(
                step,
                imports == 2,
                format!("C's log holds {} imports, not 2", imports),
            );

            let run = c.c.koto(&["status", &c.name]);
            ensure(
                step,
                run.ok() && run.json["current_state"] == "review",
                format!("koto status in C: {}", run.describe()),
            );

            let run = c.b.koto(&["next", &c.name]);
            ensure(
                step,
                run.code == Some(2)
                    && run.json["error"]["code"] == "session_migrated"
                    && run.json["error"]["message"]
                        .as_str()
                        .is_some_and(|m| m.contains(&c.c.ws_str())),
                format!("B's copy doesn't refuse naming C: {}", run.describe()),
            );
        })
    }

    /// Run every step in order.
    pub fn run_all(&mut self) {
        self.init_a();
        self.keys_a();
        self.import_b();
        self.keys_b();
        self.advance_b();
        self.refuse_a();
        self.reimport_c();
    }

    /// One line per step, for a test's output.
    pub fn report(&self) -> String {
        self.timings
            .iter()
            .map(|(step, d)| format!("{}: ok ({} ms)", step, d.as_millis()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
