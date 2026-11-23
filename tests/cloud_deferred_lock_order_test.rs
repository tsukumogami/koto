//! A deferred cloud append runs its after-commit steps with neither the
//! log's append lock nor the batch lock held (koto#728).
//!
//! A plain append takes the append lock and then the batch lock. The
//! after-commit steps can append a nested `context_added`, which takes the
//! append lock again. An append inside a tick that still held the batch lock
//! while those steps ran took the two locks in the other order, so two
//! processes writing one deferred session could each wait on the other
//! forever.
//!
//! The test holds an append inside its after-commit steps, using the debug
//! build's journal rotation hold, and checks that both locks are free there.
//! Every command runs under a timeout, so a regression fails here instead of
//! hanging the suite.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BUCKET: &str = "koto-deferred-lock-order";
const NAME: &str = "coord";
const TEMPLATE_FILE: &str = "coord.md";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const ROTATE_HOLD_ENV: &str = "KOTO_TEST_JOURNAL_ROTATE_HOLD";

/// Long enough for a loaded host, far short of a hung suite.
const BOUND: Duration = Duration::from_secs(60);

/// A first state whose action adds a context key from inside the tick (a
/// deferred append), a state waiting on evidence, and a terminal.
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

/// koto in `ws` on the cloud backend against `s3`, with `wf` as the
/// `/workflows` folder.
fn koto_cmd(
    s3: &FakeS3,
    root: &Path,
    ws: &Path,
    wf: &Path,
    args: &[&str],
) -> std::process::Command {
    let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
    let mut cmd = koto_command::koto_std();
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
        .args(args);
    cmd
}

/// Run `cmd` to completion, killed after [`BOUND`].
fn run(mut cmd: std::process::Command) -> (Option<i32>, String) {
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    finish(child, Instant::now())
}

fn finish(mut child: std::process::Child, started: Instant) -> (Option<i32>, String) {
    loop {
        if child.try_wait().unwrap().is_some() {
            let o = child.wait_with_output().unwrap();
            return (
                o.status.code(),
                format!(
                    "status {:?}; stdout: {}; stderr: {}",
                    o.status,
                    String::from_utf8_lossy(&o.stdout).trim(),
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
            );
        }
        if started.elapsed() > BOUND {
            let _ = child.kill();
            let _ = child.wait();
            return (None, format!("did not finish within {:?}", BOUND));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether an exclusive `flock` on `path` can be had at once: no other
/// process holds it. The probe's lock is released when the file closes.
fn lock_is_free(path: &Path) -> bool {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    ret == 0
}

#[test]
fn a_deferred_append_holds_no_lock_through_its_after_commit_steps() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: a release build doesn't read the journal test variables");
        return;
    }
    let s3 = FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = mk(tmp.path().to_path_buf());
    mk(root.join("home"));
    mk(root.join("cache"));
    let ws = mk(root.join("ws"));
    let wf = mk(root.join("workflows"));
    std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();

    let (code, out) = run(koto_cmd(
        &s3,
        &root,
        &ws,
        &wf,
        &["init", NAME, "--template", TEMPLATE_FILE],
    ));
    assert_eq!(code, Some(0), "init: {out}");

    // Every journal write from here rotates, so the first after-commit step
    // of the next command reaches the rotation's hold.
    let (code, out) = run(koto_cmd(
        &s3,
        &root,
        &ws,
        &wf,
        &["config", "set", "run_journal.rotate_bytes", "1", "--user"],
    ));
    assert_eq!(code, Some(0), "config set: {out}");

    let session_dir = root.join("home").join(".koto").join("sessions").join(NAME);
    let marker = root.join("rotation-held");
    let started = Instant::now();
    let mut next = koto_cmd(&s3, &root, &ws, &wf, &["next", NAME]);
    next.env(ROTATE_HOLD_ENV, &marker)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = next.spawn().unwrap();
    while !marker.exists() && started.elapsed() < BOUND {
        std::thread::sleep(Duration::from_millis(5));
    }
    let reached = marker.exists();

    // Held inside a deferred append's after-commit steps.
    let append_free = reached && lock_is_free(&session_dir.join("append.lock"));
    let batch_free = reached && lock_is_free(&session_dir.join("cloud-batch.lock"));

    let _ = std::fs::remove_file(&marker);
    let (code, out) = finish(child, started);

    assert!(
        reached,
        "no after-commit step reached the rotation's hold: {out}"
    );
    assert!(
        append_free,
        "the append lock was held through the after-commit steps"
    );
    assert!(
        batch_free,
        "the batch lock was held through the after-commit steps, the reverse of an \
         append's order"
    );
    assert_eq!(code, Some(0), "next: {out}");
}
