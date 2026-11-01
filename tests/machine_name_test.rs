//! The machine name a session records: `KOTO_MACHINE_NAME` when it is set,
//! non-empty and valid, else `machine.name` from the user-level config file,
//! else the short hostname, coerced. The name survives every operation that
//! rewrites a session, an import records the importing machine, and
//! `koto dashboard --detail` reports it.
//!
//! Every test runs the built koto binary under a scratch `HOME` and
//! `KOTO_HOME`, against the in-process endpoint in `tests/support/fake_s3.rs`
//! when it uses the cloud backend; no real bucket is involved.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/project_trust.rs"]
mod project_trust;

use fake_s3::FakeS3;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUCKET: &str = "koto-machine-name";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// `work` waits for a marker; `done` is the terminal.
const WORK: &str = r#"---
name: machine-work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// A parent whose `plan` state spawns one child per submitted task.
const PARENT: &str = r#"---
name: machine-parent
version: "1.0"
initial_state: gather
states:
  gather:
    transitions:
      - target: plan
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: [yes]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## gather

Gather.

## plan

Plan.

## summarize

Summarize.
"#;

const CHILD: &str = r#"---
name: machine-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }

    /// The stderr lines that name `needle`.
    fn stderr_lines_naming(&self, needle: &str) -> usize {
        self.stderr.lines().filter(|l| l.contains(needle)).count()
    }
}

/// The bucket prefix koto derives for a directory.
fn prefix_of(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

fn mkdir(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// One machine: its own home, `KOTO_HOME`, template cache and working
/// directory, an optional `KOTO_MACHINE_NAME`, and a backend.
struct Machine {
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
    endpoint: Option<String>,
    env_name: Option<String>,
}

impl Machine {
    /// A machine under `root/<label>` working in `ws`, on the cloud backend
    /// when `s3` is given and the local backend otherwise.
    fn new(root: &Path, label: &str, ws: &Path, s3: Option<&FakeS3>) -> Self {
        let base = root.join(label);
        let m = Machine {
            home: mkdir(base.join("home")),
            cache: mkdir(base.join("cache")),
            ws: mkdir(ws.to_path_buf()),
            endpoint: s3.map(FakeS3::endpoint),
            env_name: None,
        };
        m.write_user_config("");
        m
    }

    fn named(mut self, name: &str) -> Self {
        self.env_name = Some(name.to_string());
        self
    }

    fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    fn user_config(&self) -> PathBuf {
        self.koto_home().join("config.toml")
    }

    /// Write the user-level config file with `text`. Under `KOTO_HOME` only
    /// the root's own config selects the cloud backend, so a cloud machine's
    /// file starts by naming it.
    fn write_user_config(&self, text: &str) {
        std::fs::create_dir_all(self.koto_home()).unwrap();
        let backend = if self.endpoint.is_some() {
            "[session]\nbackend = \"cloud\"\n\n"
        } else {
            ""
        };
        std::fs::write(self.user_config(), format!("{backend}{text}")).unwrap();
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.koto_home()
            .join("sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn workspace(&self) -> String {
        prefix_of(&self.ws)
    }

    fn cmd_in(&self, dir: &Path, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(dir)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    koto_command::koto_bin().parent().unwrap().display(),
                    SYSTEM_PATH
                ),
            )
            .env("HOME", &self.home)
            .env("KOTO_HOME", self.koto_home())
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        if let Some(name) = &self.env_name {
            cmd.env("KOTO_MACHINE_NAME", name);
        }
        match &self.endpoint {
            Some(endpoint) => {
                cmd.env("KOTO_SESSION_BACKEND", "cloud")
                    .env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint)
                    .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                    .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                    .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                    .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                    .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
            }
            None => {
                cmd.env("KOTO_SESSION_BACKEND", "local");
            }
        }
        cmd.args(args);
        cmd
    }

    fn run_in(&self, dir: &Path, args: &[&str], stdin: Option<&str>) -> Out {
        let mut cmd = self.cmd_in(dir, args);
        if let Some(input) = stdin {
            cmd.write_stdin(input.as_bytes().to_vec());
        }
        let o = cmd.output().unwrap();
        let out = Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        assert!(
            self.endpoint.is_none() || !out.stderr.contains("runs on the local backend"),
            "koto {args:?} fell back to the local backend: {}",
            out.describe()
        );
        out
    }

    fn koto(&self, args: &[&str]) -> Out {
        self.run_in(&self.ws, args, None)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert_eq!(out.code, Some(0), "koto {args:?}: {}", out.describe());
        out
    }

    fn write_templates(&self) {
        std::fs::write(self.ws.join("work.md"), WORK).unwrap();
        std::fs::write(self.ws.join("parent.md"), PARENT).unwrap();
        std::fs::write(self.ws.join("child.md"), CHILD).unwrap();
    }

    fn init(&self, name: &str) -> Out {
        self.ok(&["init", name, "--template", "work.md"])
    }

    fn init_inline(&self, name: &str) -> Out {
        let out = self.run_in(&self.ws, &["init", name, "--from-stdin"], Some(WORK));
        assert_eq!(out.code, Some(0), "inline init {name}: {}", out.describe());
        out
    }

    /// Create parent `name` and submit two tasks, so one `koto next` spawns
    /// `<name>.task-a` and `<name>.task-b`. Returns that spawning run.
    fn spawn_children(&self, name: &str) -> Out {
        self.ok(&["init", name, "--template", "parent.md"]);
        self.ok(&["next", name]);
        let tasks = serde_json::json!({
            "tasks": [
                {"name": "task-a", "waits_on": [], "vars": {}},
                {"name": "task-b", "waits_on": [], "vars": {}},
            ]
        })
        .to_string();
        self.ok(&["next", name, "--with-data", &tasks])
    }

    /// The `machine` recorded on `name`'s `workflow_initialized` event, read
    /// from the local state file.
    fn recorded(&self, name: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.state_path(name))
            .unwrap_or_else(|e| panic!("no state file for {name}: {e}"));
        text.lines()
            .skip(1)
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|e| e["type"] == "workflow_initialized")
            .and_then(|e| e["payload"]["machine"].as_str().map(str::to_string))
    }

    fn detail(&self, name: &str) -> Value {
        let out = self.ok(&["dashboard", "--detail", name]);
        serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("--detail {name} is not JSON ({e}): {}", out.describe()))
    }

    /// The `--once` rows, each split into its tab-separated fields.
    fn once_rows(&self) -> Vec<Vec<String>> {
        self.ok(&["dashboard", "--once"])
            .stdout
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    }

    fn last_event(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.state_path(name)).unwrap();
        serde_json::from_str(text.lines().rfind(|l| !l.trim().is_empty()).unwrap()).unwrap()
    }
}

/// This host's name as koto records it with no other source: the part
/// before the first `.`, every character outside `[A-Za-z0-9._-]` replaced
/// by `-`, cut to 64, `unknown` and empty becoming `unknown-host`.
fn host_machine_name() -> String {
    let out = std::process::Command::new("hostname").output().unwrap();
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let short = raw.split('.').next().unwrap_or("");
    let name: String = short
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if name.is_empty() || name == "unknown" {
        "unknown-host".to_string()
    } else {
        name
    }
}

/// Each source case, with what a new session records under it: the
/// environment value (`None` leaves the variable unset) and the user-level
/// `machine.name` (`None` leaves the key out).
fn source_cases() -> Vec<(
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    String,
)> {
    vec![
        (
            "both",
            Some("env-name"),
            Some("cfg-name"),
            "env-name".to_string(),
        ),
        ("cfg", None, Some("cfg-name"), "cfg-name".to_string()),
        ("none", None, None, host_machine_name()),
        ("empty", Some(""), Some("cfg-name"), "cfg-name".to_string()),
    ]
}

fn check_sources(cloud: bool) {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let base = Machine::new(&root, "m", &root.join("ws"), cloud.then_some(&s3));
    base.write_templates();
    for (case, env, cfg, want) in source_cases() {
        let mut m = Machine::new(&root, "m", &base.ws, cloud.then_some(&s3));
        m.env_name = env.map(str::to_string);
        match cfg {
            Some(name) => m.write_user_config(&format!("[machine]\nname = \"{name}\"\n")),
            None => m.write_user_config(""),
        }
        let init = format!("{case}-init");
        let inline = format!("{case}-inline");
        let parent = format!("{case}-parent");
        let outs = [
            m.init(&init),
            m.init_inline(&inline),
            m.spawn_children(&parent),
        ];
        for out in &outs {
            assert_eq!(
                out.stderr_lines_naming("machine name"),
                0,
                "{case}: no warning expected: {}",
                out.describe()
            );
        }
        for name in [
            init.clone(),
            inline.clone(),
            parent.clone(),
            format!("{parent}.task-a"),
            format!("{parent}.task-b"),
        ] {
            assert_eq!(
                m.recorded(&name).as_deref(),
                Some(want.as_str()),
                "{case} ({}): {name}",
                if cloud { "cloud" } else { "local" }
            );
        }
        assert_eq!(m.detail(&init)["machine"], Value::String(want.clone()));
        if cloud {
            let pushed = format!("{}/{init}/koto-{init}.state.jsonl", m.workspace());
            assert!(s3.object(&pushed).is_some(), "{case}: {pushed} not pushed");
        }
    }
}

#[test]
fn a_new_session_records_env_then_config_then_hostname_on_the_local_backend() {
    check_sources(false);
}

#[test]
fn a_new_session_records_env_then_config_then_hostname_on_the_cloud_backend() {
    check_sources(true);
}

#[test]
fn config_set_user_accepts_a_valid_name_and_refuses_the_rest_byte_for_byte() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let m = Machine::new(&root, "m", &root.join("ws"), None);
    let long = "a".repeat(64);
    m.ok(&["config", "set", "--user", "machine.name", &long]);
    let written = std::fs::read(m.user_config()).unwrap();
    let doc: toml::Value = toml::from_str(std::str::from_utf8(&written).unwrap()).unwrap();
    assert_eq!(doc["machine"]["name"].as_str(), Some(long.as_str()));
    assert_eq!(
        m.ok(&["config", "get", "machine.name"]).stdout.trim(),
        long.as_str()
    );

    let too_long = "a".repeat(65);
    for bad in [too_long.as_str(), "unknown", "", "bad name!"] {
        let out = m.koto(&["config", "set", "--user", "machine.name", bad]);
        assert_ne!(out.code, Some(0), "{bad:?} accepted: {}", out.describe());
        assert!(out.stderr.contains("machine.name"), "{}", out.describe());
        assert_eq!(
            std::fs::read(m.user_config()).unwrap(),
            written,
            "{bad:?} changed the file"
        );
    }

    m.ok(&["config", "unset", "--user", "machine.name"]);
    assert_ne!(m.koto(&["config", "get", "machine.name"]).code, Some(0));
}

#[test]
fn an_invalid_env_value_warns_once_and_falls_through() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let ws = root.join("ws");
    let host = host_machine_name();
    for (i, bad) in ["bad name!", "unknown"].into_iter().enumerate() {
        // With a config value, the session records it.
        let m = Machine::new(&root, &format!("cfg{i}"), &ws, None).named(bad);
        m.write_templates();
        m.write_user_config("[machine]\nname = \"cfg-name\"\n");
        let out = m.init("wf");
        assert_eq!(
            out.stderr_lines_naming("KOTO_MACHINE_NAME"),
            1,
            "{}",
            out.describe()
        );
        assert!(!out.stderr.contains("bad name!"), "{}", out.describe());
        assert_eq!(m.recorded("wf").as_deref(), Some("cfg-name"));

        // A batch that spawns two children in one process warns once.
        let out = m.spawn_children("orch");
        assert_eq!(
            out.stderr_lines_naming("KOTO_MACHINE_NAME"),
            1,
            "{}",
            out.describe()
        );
        for child in ["orch.task-a", "orch.task-b"] {
            assert_eq!(m.recorded(child).as_deref(), Some("cfg-name"), "{child}");
        }

        // With no config value, the session records the hostname.
        let m = Machine::new(&root, &format!("host{i}"), &ws, None).named(bad);
        let out = m.init("wf");
        assert_eq!(
            out.stderr_lines_naming("KOTO_MACHINE_NAME"),
            1,
            "{}",
            out.describe()
        );
        assert_eq!(m.recorded("wf").as_deref(), Some(host.as_str()));
    }

    // A hand-edited invalid config value is skipped the same way.
    let m = Machine::new(&root, "edited", &ws, None);
    m.write_user_config("[machine]\nname = \"bad name!\"\n");
    let out = m.init("wf");
    assert_eq!(
        out.stderr_lines_naming("machine.name"),
        1,
        "{}",
        out.describe()
    );
    assert_eq!(m.recorded("wf").as_deref(), Some(host.as_str()));
}

#[test]
fn the_project_config_never_supplies_the_machine_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let m = Machine::new(&root, "m", &root.join("ws"), None);
    m.write_templates();

    let out = m.koto(&["config", "set", "machine.name", "x"]);
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!m.ws.join(".koto").join("config.toml").exists());
    let out = m.koto(&["config", "unset", "machine.name"]);
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!m.ws.join(".koto").join("config.toml").exists());

    std::fs::create_dir_all(m.ws.join(".koto")).unwrap();
    std::fs::write(
        m.ws.join(".koto").join("config.toml"),
        "[machine]\nname = \"proj-name\"\n",
    )
    .unwrap();
    // Trusted, so the key is refused for what it is rather than for an
    // untrusted file: a trusted project file still can't name the machine.
    project_trust::trust(&m.koto_home(), &m.ws);
    let out = m.init("wf");
    assert_eq!(
        out.stderr_lines_naming("machine.name"),
        1,
        "{}",
        out.describe()
    );
    assert_eq!(m.recorded("wf"), Some(host_machine_name()));
    assert_ne!(m.koto(&["config", "get", "machine.name"]).code, Some(0));
}

#[test]
fn the_recorded_machine_survives_pull_push_resolve_rewind_cancel_and_rebind() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let ws = root.join("ws");
    let a = Machine::new(&root, "a", &ws, Some(&s3)).named("machine-a");
    let b = Machine::new(&root, "b", &ws, Some(&s3)).named("machine-b");
    a.write_templates();
    let machine_a = Value::String("machine-a".to_string());

    a.init("wf");
    assert_eq!(a.detail("wf")["machine"], machine_a);

    // B pulls A's session, ticks it (a push), and A takes it back.
    b.ok(&["session", "resolve", "wf", "--keep", "remote"]);
    assert_eq!(b.detail("wf")["machine"], machine_a);
    b.ok(&[
        "next",
        "wf",
        "--with-data",
        r#"{"marker":"done"}"#,
        "--no-cleanup",
    ]);
    assert_eq!(b.detail("wf")["machine"], machine_a);
    a.ok(&["session", "resolve", "wf", "--keep", "remote"]);
    assert_eq!(a.detail("wf")["current_state"], "done");
    assert_eq!(a.detail("wf")["machine"], machine_a);

    a.ok(&["rewind", "wf"]);
    assert_eq!(a.detail("wf")["current_state"], "work");
    assert_eq!(a.detail("wf")["machine"], machine_a);

    let moved = mkdir(root.join("moved"));
    a.ok(&["session", "rebind", "wf", "--to", moved.to_str().unwrap()]);
    assert_eq!(a.detail("wf")["machine"], machine_a);

    let out = a.run_in(&moved, &["cancel", "wf"], None);
    assert_eq!(out.code, Some(0), "cancel: {}", out.describe());
    assert_eq!(a.detail("wf")["machine"], machine_a);
    assert_eq!(a.recorded("wf").as_deref(), Some("machine-a"));
}

#[test]
fn an_import_records_the_importing_machine() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let a = Machine::new(&root, "a", &root.join("a").join("ws"), Some(&s3)).named("machine-a");
    let b = Machine::new(&root, "b", &root.join("b").join("ws"), Some(&s3)).named("machine-b");
    a.write_templates();
    b.write_templates();

    a.init("wf");
    a.ok(&[
        "next",
        "wf",
        "--with-data",
        r#"{"marker":"done"}"#,
        "--no-cleanup",
    ]);
    b.ok(&["template", "compile", "work.md"]);
    // Wait out the one-second retention.
    std::thread::sleep(Duration::from_millis(1300));

    let from = a.ws.to_string_lossy().into_owned();
    b.ok(&["session", "import", "wf", "--from", &from]);

    let last = b.last_event("wf");
    assert_eq!(last["type"], "session_imported", "{last}");
    assert_eq!(last["payload"]["machine_name"], "machine-b", "{last}");
    // The copied init still names the source's machine.
    assert_eq!(b.recorded("wf").as_deref(), Some("machine-a"));
    assert_eq!(b.detail("wf")["machine"], "machine-b");
    // Back to `work`, so the copy runs and `--once` lists it.
    b.ok(&["rewind", "wf"]);
    assert_eq!(b.detail("wf")["machine"], "machine-b");
    let rows = b.once_rows();
    let copy = rows
        .iter()
        .find(|r| r[0] == "wf" && r.get(9).map(String::as_str) == Some(b.workspace().as_str()))
        .unwrap_or_else(|| panic!("no row for B's copy: {rows:?}"));
    assert_eq!(copy[8], "machine-b", "{copy:?}");

    assert_eq!(a.detail("wf")["machine"], "machine-a");

    // An import made before the field existed records no machine, though
    // the copied init names the source's.
    let path = b.state_path("wf");
    let text = std::fs::read_to_string(&path).unwrap();
    let stripped = text.replace(",\"machine_name\":\"machine-b\"", "");
    assert_ne!(stripped, text);
    std::fs::write(&path, stripped).unwrap();
    assert_eq!(b.detail("wf")["machine"], Value::Null);
}

/// Copy the checked-in store koto v0.33.0 wrote under `root`, re-rooting
/// its paths there.
fn install_v0_33_store(root: &Path, m: &Machine) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions-v0.33.0");
    let src = fixture.join("sessions").join("v033");
    let dst = m.koto_home().join("sessions").join("v033");
    std::fs::create_dir_all(&dst).unwrap();
    for entry in std::fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let text = std::fs::read_to_string(entry.path()).unwrap();
        std::fs::write(
            dst.join(entry.file_name()),
            text.replace("@ROOT@", &root.to_string_lossy()),
        )
        .unwrap();
    }
}

fn v0_33_detail(root: &Path) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sessions-v0.33.0/detail-v0.33.0.json");
    std::fs::read_to_string(fixture)
        .unwrap()
        .trim()
        .replace("@ROOT@", &root.to_string_lossy())
}

fn header_origin(m: &Machine, name: &str) -> String {
    let text = std::fs::read_to_string(m.state_path(name)).unwrap();
    let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    serde_json::to_string(&header["origin"]).unwrap()
}

#[test]
fn detail_reports_null_for_a_v0_33_store_and_changes_nothing_else() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    // The store's own layout: `home/.koto/sessions` and `work`.
    let local = Machine {
        home: mkdir(root.join("home")),
        cache: mkdir(root.join("cache")),
        ws: mkdir(root.join("work")),
        endpoint: None,
        env_name: Some("machine-b".to_string()),
    };
    install_v0_33_store(&root, &local);
    let state_before = std::fs::read(local.state_path("v033")).unwrap();
    let origin_before = header_origin(&local, "v033");

    // Every field v0.33.0 printed is byte-identical; `machine` is added as
    // null.
    let out = local.ok(&["dashboard", "--detail", "v033"]);
    let printed = out.stdout.trim();
    let v: Value = serde_json::from_str(printed).unwrap();
    assert_eq!(v["machine"], Value::Null, "{printed}");
    assert!(v.as_object().unwrap().contains_key("machine"), "{printed}");
    assert_eq!(
        printed.replace(",\"machine\":null", ""),
        v0_33_detail(&root),
        "{printed}"
    );

    // Under the cloud backend: an empty ninth column, and a --detail that
    // writes nothing to the bucket.
    let cloud = Machine {
        endpoint: Some(s3.endpoint()),
        ..local
    };
    cloud.write_user_config("");
    let rows = cloud.once_rows();
    let row = rows
        .iter()
        .find(|r| r[0] == "v033")
        .unwrap_or_else(|| panic!("no v033 row: {rows:?}"));
    assert_eq!(row.get(8).map(String::as_str), Some(""), "{row:?}");

    s3.clear_requests();
    let out = cloud.ok(&["dashboard", "--detail", "v033"]);
    let v: Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(v["machine"], Value::Null);
    let writes: Vec<_> = s3
        .requests()
        .into_iter()
        .filter(|r| !matches!(r.method.as_str(), "GET" | "HEAD"))
        .map(|r| format!("{} {}", r.method, r.key))
        .collect();
    assert!(
        writes.is_empty(),
        "--detail wrote to the bucket: {writes:?}"
    );

    assert_eq!(header_origin(&cloud, "v033"), origin_before);
    assert_eq!(
        std::fs::read(cloud.state_path("v033")).unwrap(),
        state_before
    );
}
