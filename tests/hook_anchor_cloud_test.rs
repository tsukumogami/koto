//! The koto-skills start hook, `plugins/koto-skills/hooks/session-start.sh`,
//! driven itself under a cloud backend, against the in-process endpoint in
//! `tests/support/fake_s3.rs`.
//!
//! An agent session's shells run koto with whatever `KOTO_SESSION_*`
//! configuration the environment carries, so the anchor the hook opens must
//! land in that same store: with the backend configured in the environment
//! alone, the anchor must reach the bucket and import into another
//! workspace, where a session it held keeps its held-by link. With the
//! backend in `~/.koto/config.toml` and only the credentials in the
//! environment, the hook's open must succeed. The hook must still never put
//! a `KOTO_SESSION_*` variable into the env file the session's shells
//! source.
//!
//! Every command here (the hook and each koto run) starts from an empty
//! environment: `PATH` with the koto under test first, a scratch `HOME` and
//! template cache, and only the variables the test sets. Nothing reaches a
//! real store or bucket.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const BUCKET: &str = "koto-hook-anchor";

/// Every failure of the anchor to land in the bucket says this.
const MISSING: &str = "anchor never reaches the bucket";

const BLOCK_BEGIN: &str = "# >>> koto-skills";
const BLOCK_END: &str = "# <<< koto-skills";

const TEMPLATE_FILE: &str = "hook-test.md";

/// `start` waits for `go`; `yes` reaches the terminal `done`.
const TEMPLATE: &str = r#"---
name: hook-test
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Start.

## done

Done.
"#;

/// The hook under test.
fn hook_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/koto-skills/hooks/session-start.sh")
}

/// `PATH` with the directory of the koto under test first.
fn test_path() -> String {
    let bin = koto_command::koto_bin();
    let dir = bin.parent().unwrap().to_str().unwrap().to_string();
    match std::env::var("PATH") {
        Ok(rest) if !rest.is_empty() => format!("{dir}:{rest}"),
        _ => format!("{dir}:/usr/bin:/bin:/usr/sbin:/sbin"),
    }
}

/// Whether jq is on the test `PATH`; without it the hook does nothing.
fn jq_present() -> bool {
    std::process::Command::new("bash")
        .env_clear()
        .env("PATH", test_path())
        .args(["-c", "command -v jq"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

macro_rules! need_jq {
    () => {
        if !jq_present() {
            eprintln!("skipped: jq is not on the PATH, and the hook does nothing without it");
            return;
        }
    };
}

/// The cloud configuration and credentials, as `KOTO_SESSION_*` variables.
fn cloud_env(s3: &FakeS3) -> Vec<(String, String)> {
    vec![
        ("KOTO_SESSION_BACKEND".into(), "cloud".into()),
        ("KOTO_SESSION_CLOUD_ENDPOINT".into(), s3.endpoint()),
        ("KOTO_SESSION_CLOUD_BUCKET".into(), BUCKET.into()),
        ("KOTO_SESSION_CLOUD_REGION".into(), "us-east-1".into()),
        ("KOTO_SESSION_CLOUD_PATH_STYLE".into(), "true".into()),
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY".into(),
            "test-access-key".into(),
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY".into(),
            "test-secret-key".into(),
        ),
    ]
}

/// The credentials alone, as `KOTO_SESSION_*` variables.
fn credentials_env() -> Vec<(String, String)> {
    vec![
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY".into(),
            "test-access-key".into(),
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY".into(),
            "test-secret-key".into(),
        ),
    ]
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// What one run of the hook left.
struct HookRun {
    out: Out,
    /// The whole env file, or empty when the hook wrote none.
    env_file: String,
    /// The anchor the env file exports, if any.
    anchor: Option<String>,
}

impl HookRun {
    /// The hook's own block of the env file.
    fn block(&self) -> String {
        let mut inside = false;
        let mut lines = Vec::new();
        for line in self.env_file.lines() {
            if !inside && line == BLOCK_BEGIN {
                inside = true;
            } else if inside && line == BLOCK_END {
                inside = false;
            } else if inside {
                lines.push(line);
            }
        }
        lines.join("\n")
    }

    fn describe(&self) -> String {
        format!(
            "hook {}; env file:\n{}",
            self.out.describe(),
            self.env_file.trim_end()
        )
    }
}

/// One simulated host: its own `HOME`, template cache, `session-env` tree
/// and workspaces.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    session_env: PathBuf,
    root: PathBuf,
}

impl Host {
    fn new() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let base = std::fs::canonicalize(root.path()).unwrap();
        let mk = |name: &str| {
            let p = base.join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        };
        let home = mk("home");
        let cache = mk("cache");
        let session_env = mk("session-env");
        Host {
            _root: root,
            home,
            cache,
            session_env,
            root: base,
        }
    }

    /// A workspace directory holding the template.
    fn workspace(&self, name: &str) -> PathBuf {
        let ws = self.root.join(name);
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        ws
    }

    /// Writes `~/.koto/config.toml`.
    fn user_config(&self, body: &str) {
        let dir = self.home.join(".koto");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto").join("sessions")
    }

    /// koto in `ws` with only `env` beyond `PATH`, `HOME` and the cache.
    fn koto(&self, ws: &Path, args: &[&str], env: &[(String, String)]) -> Out {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", test_path())
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.args(args);
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, ws: &Path, args: &[&str], env: &[(String, String)]) -> Out {
        let out = self.koto(ws, args, env);
        assert!(out.ok(), "koto {args:?}: {}", out.describe());
        out
    }

    /// Runs the start hook for agent session `id` in workspace `ws`, with a
    /// startup payload on stdin, its env file at
    /// `<session-env>/<id>/sessionstart-hook-0.sh`, niwa's reap key set (so
    /// no watcher is left behind) and only `env` beyond that.
    fn hook(&self, ws: &Path, id: &str, env: &[(String, String)]) -> HookRun {
        let dir = self.session_env.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join("sessionstart-hook-0.sh");
        let payload = serde_json::json!({
            "session_id": id,
            "source": "startup",
            "transcript_path": "",
            "cwd": ws.to_str().unwrap(),
        })
        .to_string();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg(hook_script())
            .env_clear()
            .current_dir(ws)
            .env("PATH", test_path())
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("CLAUDE_ENV_FILE", &env_path)
            .env("NIWA_REAP_KEY", format!("key-{id}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        let env_file = std::fs::read_to_string(&env_path).unwrap_or_default();
        let anchor = env_file.lines().find_map(|l| {
            l.strip_prefix("export KOTO_ANCHOR='")
                .and_then(|r| r.strip_suffix('\''))
                .map(str::to_string)
        });
        HookRun {
            out: Out {
                code: o.status.code(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            env_file,
            anchor,
        }
    }
}

/// The bucket's keys under `anchor`'s directory, whatever the workspace.
fn anchor_keys(s3: &FakeS3, anchor: &str) -> Vec<String> {
    let needle = format!("/{anchor}/");
    s3.keys()
        .into_iter()
        .filter(|k| k.contains(&needle))
        .collect()
}

/// The hook exported an anchor and the bucket holds it.
fn assert_anchor_in_bucket(s3: &FakeS3, run: &HookRun) -> String {
    let anchor = run.anchor.clone().unwrap_or_else(|| {
        panic!(
            "{MISSING}: the hook opened no anchor (no `export KOTO_ANCHOR=` line); {}",
            run.describe()
        )
    });
    let keys = anchor_keys(s3, &anchor);
    assert!(
        !keys.is_empty(),
        "{MISSING}: the bucket holds nothing under {anchor}; bucket keys: {:?}; {}",
        s3.keys(),
        run.describe()
    );
    anchor
}

/// With the cloud backend configured in the environment alone, the anchor
/// the hook opens is in the bucket.
#[test]
fn under_environment_only_cloud_configuration_the_hooks_anchor_is_in_the_bucket() {
    need_jq!();
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new();
    let ws = h.workspace("ws1");
    let run = h.hook(&ws, "envonly", &cloud_env(&s3));
    assert_eq!(run.out.code, Some(0), "{}", run.describe());
    assert_anchor_in_bucket(&s3, &run);
}

/// With the cloud backend configured in the environment alone: the hook's
/// anchor and a finished session it held import, from a bare shell on
/// another host, into another workspace, where the session's held-by link
/// names the imported anchor.
#[test]
fn under_environment_only_cloud_configuration_the_anchor_and_its_session_import_elsewhere() {
    need_jq!();
    let s3 = FakeS3::start(BUCKET);
    let h1 = Host::new();
    let ws1 = h1.workspace("ws1");
    let cloud = cloud_env(&s3);

    let run = h1.hook(&ws1, "iam-agent", &cloud);
    assert_eq!(run.out.code, Some(0), "{}", run.describe());
    let anchor = run
        .anchor
        .clone()
        .unwrap_or_else(|| panic!("the hook opened no anchor; {}", run.describe()));

    // A shell of the agent session: the env file's anchor plus the cloud
    // configuration.
    let mut shell = cloud.clone();
    shell.push(("KOTO_ANCHOR".into(), anchor.clone()));
    shell.push(("KOTO_DISPATCHER_ANCHOR".into(), anchor.clone()));
    h1.ok(
        &ws1,
        &["init", "iam_B", "--template", TEMPLATE_FILE],
        &shell,
    );
    h1.ok(&ws1, &["next", "iam_B"], &shell);
    h1.ok(
        &ws1,
        &["next", "iam_B", "--with-data", r#"{"go":"yes"}"#],
        &shell,
    );
    let link = format!("ctx/koto/links/held-by/{anchor}");
    assert!(
        h1.sessions().join("iam_B").join(&link).exists(),
        "iam_B holds no held-by link to {anchor} on the first host"
    );

    // Another host, a bare shell: the cloud configuration only.
    let h2 = Host::new();
    let ws3 = h2.workspace("ws3");
    let template = ws3.join(TEMPLATE_FILE);
    h2.ok(
        &ws3,
        &["template", "compile", template.to_str().unwrap()],
        &cloud,
    );
    let from = ws1.to_str().unwrap();
    let out = h2.koto(
        &ws3,
        &["session", "import", &anchor, "--from", from],
        &cloud,
    );
    assert!(
        out.ok(),
        "{MISSING}: importing {anchor} from {from} failed: {}; bucket keys: {:?}",
        out.describe(),
        s3.keys()
    );
    h2.ok(
        &ws3,
        &["session", "import", "iam_B", "--from", from],
        &cloud,
    );
    assert!(
        h2.sessions().join("iam_B").join(&link).exists(),
        "the imported iam_B's held-by link doesn't name the imported anchor {anchor}"
    );
}

/// With the backend in `~/.koto/config.toml` and only the credentials in
/// the environment, the hook's open succeeds and the anchor is in the
/// bucket.
#[test]
fn with_the_backend_in_a_config_file_and_credentials_in_the_environment_the_hook_opens() {
    need_jq!();
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new();
    h.user_config(&format!(
        "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nendpoint = {:?}\nbucket = {:?}\n\
         region = \"us-east-1\"\npath_style = true\n",
        s3.endpoint(),
        BUCKET
    ));
    let ws = h.workspace("ws1");
    let run = h.hook(&ws, "configfile", &credentials_env());
    assert_eq!(run.out.code, Some(0), "{}", run.describe());
    assert!(
        run.block().contains("export KOTO_ANCHOR="),
        "{MISSING}: the hook's open failed and it wrote the unset block; {}",
        run.describe()
    );
    assert_anchor_in_bucket(&s3, &run);
}

/// Whatever the configuration, the hook's block of the env file sets no
/// `KOTO_SESSION_*` variable, so the session's shells, and the tests and
/// suites they run, get none from the hook.
#[test]
fn the_hooks_env_file_block_names_no_session_backend_variable() {
    need_jq!();
    let s3 = FakeS3::start(BUCKET);

    let h = Host::new();
    let ws = h.workspace("ws1");
    let run = h.hook(&ws, "envonly", &cloud_env(&s3));
    assert!(
        run.env_file.contains(BLOCK_BEGIN),
        "no block written; {}",
        run.describe()
    );
    assert!(
        !run.block().contains("KOTO_SESSION_"),
        "environment-only: {}",
        run.describe()
    );

    let h = Host::new();
    h.user_config(&format!(
        "[session]\nbackend = \"cloud\"\n\n[session.cloud]\nendpoint = {:?}\nbucket = {:?}\n\
         region = \"us-east-1\"\npath_style = true\n",
        s3.endpoint(),
        BUCKET
    ));
    let ws = h.workspace("ws1");
    let run = h.hook(&ws, "configfile", &credentials_env());
    assert!(
        run.env_file.contains(BLOCK_BEGIN),
        "no block written; {}",
        run.describe()
    );
    assert!(
        !run.block().contains("KOTO_SESSION_"),
        "config file: {}",
        run.describe()
    );
}
