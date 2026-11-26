//! A cloud append releases the log's append lock before its after-commit
//! steps run (koto#728).
//!
//! With a `/workflows` folder configured, the after-commit steps publish the
//! session's location, which appends a nested `context_added` through the
//! commit funnel and takes the same sidecar `append.lock`. An append that
//! still held the lock while they ran waited forever on its own process, so
//! `koto init` never returned.
//!
//! Every command runs under a timeout, so a regression fails here instead of
//! hanging the suite.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUCKET: &str = "koto-after-commit-lock";
const NAME: &str = "coord";
const TEMPLATE_FILE: &str = "coord.md";
const LOCATION: &str = "workflows/publish-location";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// A first state whose action adds a context key from inside the tick, a
/// state waiting on evidence, and a terminal.
const TEMPLATE: &str = r#"---
name: coord
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
        printf one | koto context add coord one.md
    transitions:
      - target: wait
  wait:
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

## wait

Wait.

## done

Done.
"#;

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// Run koto in `ws` on the cloud backend against `s3`, with `wf` as the
/// `/workflows` folder, killed after [`BOUND`].
fn koto(s3: &FakeS3, root: &Path, ws: &Path, wf: &Path, args: &[&str]) -> (Option<i32>, String) {
    let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
    let mut cmd = koto_command::koto();
    cmd.env_clear()
        .current_dir(ws)
        .env("PATH", format!("{}:{}", koto_dir.display(), SYSTEM_PATH))
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("KOTO_SESSION_BACKEND", "cloud")
        .env("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint())
        .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
        .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
        .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
        .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
        .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
        .env("KOTO_WORKFLOWS_DIR", wf)
        .args(args)
        .timeout(BOUND);
    match cmd.output() {
        Ok(o) => (
            o.status.code(),
            format!(
                "status {:?}; stdout: {}; stderr: {}",
                o.status,
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => (None, format!("did not finish within {:?}: {e}", BOUND)),
    }
}

#[test]
fn a_cloud_append_with_a_workflows_folder_does_not_wait_on_its_own_lock() {
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = mk(tmp.path().to_path_buf());
    mk(root.join("home"));
    mk(root.join("cache"));
    let ws = mk(root.join("ws"));
    let wf = mk(root.join("workflows"));
    std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();

    let (code, out) = koto(
        &s3,
        &root,
        &ws,
        &wf,
        &["init", NAME, "--template", TEMPLATE_FILE],
    );
    assert_eq!(code, Some(0), "init: {out}");

    // The action's `context add` appends under the lock and publishes again.
    let (code, out) = koto(&s3, &root, &ws, &wf, &["next", NAME]);
    assert_eq!(code, Some(0), "next: {out}");

    let (code, out) = koto(&s3, &root, &ws, &wf, &["context", "get", NAME, LOCATION]);
    assert_eq!(code, Some(0), "get {LOCATION}: {out}");
    assert!(out.contains(wf.to_str().unwrap()), "location: {out}");
}
