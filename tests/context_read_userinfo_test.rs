//! A failed `koto context get` or `koto context list` on the cloud backend
//! prints its error with the userinfo (`user:password@`) of every URL in it
//! taken out, with and without `--workspace` (koto#469), as koto's other
//! cloud errors are.
//!
//! The cloud backend's own errors already leave credentials out where they
//! name the endpoint, so the URL here reaches the error another way: through
//! a path that holds one. The session store lives under a `HOME` whose path
//! holds `http://user:secret@host`, and the `--workspace` reader's scratch
//! copy under a `TMPDIR` that does, so an error naming either names the URL.
//! The bucket is the in-process endpoint of `tests/support/fake_s3.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use std::path::{Path, PathBuf};

use fake_s3::FakeS3;
use migration_carrier::{Cloud, Host, Run};

const BUCKET: &str = "koto-context-read-userinfo";
const NAME: &str = "record";
/// A URL with credentials, written into a directory's path.
const URL: &str = "http://user:secret@host";
/// What must never be printed.
const USERINFO: &str = "user:secret@";
/// What the URL reads as once its userinfo is taken out.
const STRIPPED: &str = "http://host/";

fn cloud(s3: &FakeS3) -> Cloud {
    Cloud {
        endpoint: s3.endpoint(),
        bucket: s3.bucket().to_string(),
        region: "us-east-1".to_string(),
        path_style: true,
        credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
    }
}

/// `dir/http://user:secret@host/leaf`, created. The path keeps the URL as
/// written, `//` and all: nothing canonicalizes it on the way to koto.
fn under_url(dir: &Path, leaf: &str) -> PathBuf {
    let path = PathBuf::from(format!("{}/{}/{}", dir.display(), URL, leaf));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Run koto on `host` with `env` set over the host's own environment.
fn koto(host: &Host, env: &[(&str, &Path)], args: &[&str]) -> Run {
    let mut cmd = host.cmd();
    for (name, value) in env {
        cmd.env(name, value);
    }
    let output = cmd.args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null);
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        json,
    }
}

fn ok(run: &Run, what: &str) {
    assert!(run.ok(), "{}: {}", what, run.describe());
}

/// `run` is `command`'s failed read: exit 3, an error envelope that still
/// names the URL, and the URL's credentials nowhere in what it printed.
fn failed_without_credentials(run: &Run, command: &str) {
    assert_eq!(run.code, Some(3), "{}: {}", command, run.describe());
    assert_eq!(run.json["command"], command, "{}", run.describe());
    assert!(
        !run.stdout.contains(USERINFO) && !run.stderr.contains(USERINFO),
        "{} printed the URL's credentials: {}",
        command,
        run.describe()
    );
    let error = run.json["error"].as_str().unwrap_or_default();
    assert!(
        error.contains(STRIPPED),
        "{}'s error no longer names the URL: {}",
        command,
        run.describe()
    );
}

/// Without `--workspace`: a key the session doesn't hold, and a manifest
/// that doesn't parse, each fail the read with an error naming the store's
/// path, which holds the URL.
#[test]
fn a_failed_read_prints_no_url_credentials() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let host = Host::new(root.path(), "a", &cloud(&s3));
    let home = under_url(root.path(), "home");
    let env = [("HOME", home.as_path())];

    let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
    ok(
        &koto(
            &host,
            &env,
            &["init", NAME, "--template", template.to_str().unwrap()],
        ),
        "init",
    );

    let run = koto(&host, &env, &["context", "get", NAME, "missing.md"]);
    failed_without_credentials(&run, "context get");

    let dir = koto(&host, &env, &["session", "dir", NAME]);
    ok(&dir, "session dir");
    let dir = PathBuf::from(dir.stdout.trim());
    assert!(dir.starts_with(&home), "session dir {}", dir.display());
    std::fs::create_dir_all(dir.join("ctx")).unwrap();
    std::fs::write(dir.join("ctx").join("manifest.json"), b"{not json").unwrap();

    let run = koto(&host, &env, &["context", "list", NAME]);
    failed_without_credentials(&run, "context list");
}

/// With `--workspace`: a key the session doesn't hold fails the read with
/// an error naming the reader's scratch copy, which sits under `TMPDIR`.
/// `context list` through `--workspace` goes through the same envelope; no
/// failure of it can be caused from here, since a listing the bucket
/// refuses leaves the remote keys out rather than failing, and the scratch
/// copy holds no manifest to break.
#[test]
fn a_failed_workspace_read_prints_no_url_credentials() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let a = Host::new(root.path(), "a", &cloud(&s3));
    let b = Host::new(root.path(), "b", &cloud(&s3));
    let template = a.ws.join(migration_carrier::TEMPLATE_FILE);
    ok(
        &a.koto(&["init", NAME, "--template", template.to_str().unwrap()]),
        "init",
    );
    let notes = a.home.join("notes.md");
    std::fs::write(&notes, b"what the run did\n").unwrap();
    ok(
        &a.koto(&[
            "context",
            "add",
            NAME,
            "notes.md",
            "--from-file",
            notes.to_str().unwrap(),
        ]),
        "context add",
    );

    let tmp = under_url(root.path(), "tmp");
    let run = koto(
        &b,
        &[("TMPDIR", tmp.as_path())],
        &[
            "--workspace",
            &a.prefix(),
            "context",
            "get",
            NAME,
            "missing.md",
        ],
    );
    failed_without_credentials(&run, "context get");
}
