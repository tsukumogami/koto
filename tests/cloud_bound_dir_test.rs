//! A command naming a session from another directory under the cloud
//! backend (koto#457).
//!
//! The cloud backend finds a session in the bucket under a prefix derived
//! from the directory a command runs in, while every workspace on a host
//! shares one local store. A `koto context get` run from another directory
//! used to sync the shared copy against that directory's prefix, and when
//! that prefix held an older session of the same name the read returned the
//! older value with nothing saying so. Now such a command refuses, naming
//! the directory the session is bound to, before it makes any request.
//!
//! And a read whose sync step fails after the bucket said its copy changed
//! is an error, not a quiet fall-back to the local copy.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`; no real bucket is involved.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-bound";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

const TEMPLATE: &str = r#"---
name: flow
version: "1.0"
initial_state: wait
states:
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

## wait

Wait.

## done

Done.
"#;

/// The remote prefix koto derives for a directory.
fn prefix_of(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(dir).unwrap();
    hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))[..16].to_string()
}

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
}

/// One host: its own `HOME` and template cache, and directories to run
/// koto from, all sharing the host's one local session store.
struct Host {
    root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: String,
    cloud: bool,
}

impl Host {
    fn new(s3: &FakeS3, cloud: bool) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Host {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            root,
            endpoint: s3.endpoint(),
            cloud,
        }
    }

    /// A directory on this host, created and canonical.
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(p).unwrap()
    }

    fn cmd(&self, dir: &Path, args: &[&str]) -> Command {
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
            .env("XDG_CACHE_HOME", &self.cache)
            .args(args);
        if self.cloud {
            cmd.env("KOTO_SESSION_BACKEND", "cloud")
                .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
                .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
                .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
                .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
                .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
                .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
                .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0");
        } else {
            cmd.env("KOTO_SESSION_BACKEND", "local");
        }
        cmd
    }

    fn koto(&self, dir: &Path, args: &[&str]) -> Out {
        let o = self.cmd(dir, args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> Out {
        let out = self.koto(dir, args);
        assert_eq!(out.code, Some(0), "koto {:?}: {}", args, out.describe());
        out
    }

    /// `koto init WF` from `dir`, with any extra flags.
    fn init(&self, dir: &Path, extra: &[&str]) {
        std::fs::write(dir.join("flow.md"), TEMPLATE).unwrap();
        let mut args = vec!["init", WF, "--template", "flow.md"];
        args.extend_from_slice(extra);
        self.ok(dir, &args);
    }

    fn add(&self, dir: &Path, key: &str, content: &[u8]) {
        let out = self
            .cmd(dir, &["context", "add", WF, key])
            .write_stdin(content.to_vec())
            .output()
            .unwrap();
        assert!(out.status.success(), "context add: {:?}", out);
    }

    fn get(&self, dir: &Path, key: &str) -> Out {
        self.koto(dir, &["context", "get", WF, key])
    }
}

/// Copy every object of `WF` under `from`'s prefix to the same name under
/// `to`'s: the older copy another directory's prefix holds in the reported
/// case.
fn copy_session(s3: &FakeS3, from: &Path, to: &Path) {
    let src = format!("{}/{WF}/", prefix_of(from));
    let dst = format!("{}/{WF}/", prefix_of(to));
    for key in s3.keys_under(&src) {
        let bytes = s3.object(&key).unwrap();
        s3.put(&format!("{dst}{}", key.strip_prefix(&src).unwrap()), &bytes);
    }
}

/// The refusal every guarded command prints for a session bound to `anchor`.
fn assert_refused(out: &Out, command: &str, anchor: &Path) {
    assert_eq!(out.code, Some(2), "{command}: {}", out.describe());
    let err: serde_json::Value = serde_json::from_str(out.stdout.trim())
        .unwrap_or_else(|e| panic!("{command}: no JSON error ({e}): {}", out.describe()));
    assert_eq!(err["command"], command, "{}", out.describe());
    let anchor = anchor.display();
    assert_eq!(
        err["error"].as_str().unwrap(),
        format!(
            "session_bound_elsewhere: session '{WF}' is bound to {anchor}, not to the directory \
             this command ran in; under the cloud backend a session is read only from its own \
             workspace, so run the command from {anchor}"
        ),
        "{command}"
    );
}

/// The reported case: a session in `ws` whose key changed after an older
/// copy of it landed under the workspace root's prefix. A read from the
/// root refuses, naming `ws`, and never prints the old value; nothing is
/// asked of the root's prefix; the session's own directory still reads
/// the current value.
#[test]
fn a_read_from_another_directory_refuses_instead_of_serving_an_older_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    h.add(&ws, "topic.md", b"old topic");
    copy_session(&s3, &ws, &root);
    h.add(&ws, "topic.md", b"new topic");

    s3.clear_requests();
    let out = h.get(&root, "topic.md");
    assert_refused(&out, "context get", &ws);
    assert!(!out.stdout.contains("old topic"), "{}", out.describe());
    let root_prefix = prefix_of(&root);
    let asked: Vec<_> = s3
        .requests()
        .into_iter()
        .filter(|r| {
            r.key.starts_with(&root_prefix)
                || r.list_prefix
                    .as_deref()
                    .is_some_and(|p| p.starts_with(&root_prefix))
        })
        .collect();
    assert!(asked.is_empty(), "the refusal synced: {asked:?}");

    assert_eq!(
        h.ok(&ws, &["context", "get", WF, "topic.md"]).stdout,
        "new topic"
    );
    // The refused read left the shared local copy alone, so a read from the
    // session's directory needs nothing from the bucket to say so.
    let state = h
        .home
        .join(".koto/sessions")
        .join(WF)
        .join(format!("koto-{WF}.state.jsonl"));
    assert!(std::fs::read_to_string(state).unwrap().contains("topic.md"));
}

/// Every command that names one session and syncs it refuses from another
/// directory with the same message, under its own command label.
#[test]
fn every_guarded_command_names_the_bound_directory() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("ws");
    let elsewhere = h.dir("elsewhere");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"value");

    let cases: &[(&[&str], &str)] = &[
        (&["context", "get", WF, "k.md"], "context get"),
        (&["context", "exists", WF, "k.md"], "context exists"),
        (&["context", "list", WF], "context list"),
        (&["context", "remove", WF, "k.md"], "context remove"),
        (&["status", WF], "status"),
        (&["decisions", "list", WF], "decisions list"),
        (&["overrides", "list", WF], "overrides list"),
        (
            &["session", "resolve", WF, "--keep", "local"],
            "session resolve",
        ),
        (
            &[
                "decisions",
                "record",
                WF,
                "--with-data",
                r#"{"choice":"a","rationale":"b"}"#,
            ],
            "decisions record",
        ),
        (
            &["overrides", "record", WF, "--gate", "g", "--rationale", "r"],
            "overrides record",
        ),
        (&["rewind", WF], "rewind"),
        (&["cancel", WF], "cancel"),
    ];
    for (args, command) in cases {
        assert_refused(&h.koto(&elsewhere, args), command, &ws);
    }
    let added = h
        .cmd(&elsewhere, &["context", "add", WF, "k.md"])
        .write_stdin(b"from elsewhere".to_vec())
        .output()
        .unwrap();
    assert_refused(
        &Out {
            code: added.status.code(),
            stdout: String::from_utf8_lossy(&added.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&added.stderr).into_owned(),
        },
        "context add",
        &ws,
    );

    // A subdirectory of the session's directory is another prefix too.
    let sub = h.dir("ws/sub");
    assert_refused(&h.get(&sub, "k.md"), "context get", &ws);

    // Nothing above changed the session.
    assert_eq!(h.ok(&ws, &["context", "get", WF, "k.md"]).stdout, "value");
    h.ok(&ws, &["status", WF]);
}

/// Not a mismatch: a session started in `init_dir` with its execution
/// anchor somewhere else reads from both, and refuses only from a third
/// directory, naming the anchor.
#[test]
fn the_init_directory_and_the_anchor_both_still_read() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let init_dir = h.dir("init");
    let anchor = h.dir("anchor");
    let third = h.dir("third");
    h.init(&init_dir, &["--execution-dir", anchor.to_str().unwrap()]);
    h.add(&init_dir, "k.md", b"value");

    assert_eq!(
        h.ok(&init_dir, &["context", "get", WF, "k.md"]).stdout,
        "value"
    );
    assert_eq!(
        h.ok(&anchor, &["context", "get", WF, "k.md"]).stdout,
        "value"
    );
    h.ok(&init_dir, &["status", WF]);
    h.ok(&anchor, &["status", WF]);
    assert_refused(&h.get(&third, "k.md"), "context get", &anchor);
}

/// The local backend's one store answers a name the same from every
/// directory, so nothing there refuses.
#[test]
fn the_local_backend_reads_by_name_from_any_directory() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, false);
    let ws = h.dir("ws");
    let elsewhere = h.dir("elsewhere");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"old");
    h.add(&ws, "k.md", b"new");
    assert_eq!(
        h.ok(&elsewhere, &["context", "get", WF, "k.md"]).stdout,
        "new"
    );
    h.ok(&elsewhere, &["status", WF]);
}

/// Another host's write of `k.md`, made straight into the bucket: the
/// manifest names the new hash. With `object`, the key's object is written
/// too; without it, the manifest names an object the bucket lacks, the 404
/// of the reported case.
fn change_key_elsewhere(s3: &FakeS3, ws: &Path, value: &[u8], object: bool) {
    use sha2::{Digest, Sha256};
    let key = |rest: &str| format!("{}/{WF}/{rest}", prefix_of(ws));
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&key("ctx/manifest.json")).unwrap()).unwrap();
    manifest["keys"]["k.md"]["hash"] = hex::encode(Sha256::digest(value)).into();
    if object {
        s3.put(&key("ctx/k.md"), value);
    } else {
        s3.remove(&key("ctx/k.md"));
    }
    s3.put(
        &key("ctx/manifest.json"),
        &serde_json::to_vec(&manifest).unwrap(),
    );
}

/// The bucket's manifest says the key changed but its object answers 404:
/// the read fails and says the local copy may be stale, instead of printing
/// the local value.
#[test]
fn a_key_whose_fetch_fails_is_an_error_not_the_local_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("ws");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"local value");
    h.ok(&ws, &["context", "get", WF, "k.md"]);
    change_key_elsewhere(&s3, &ws, b"newer value", false);

    let out = h.get(&ws, "k.md");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!out.stdout.contains("local value"), "{}", out.describe());
    assert!(out.stdout.contains("may be stale"), "{}", out.describe());

    // Once the object is there the same read returns it.
    change_key_elsewhere(&s3, &ws, b"newer value", true);
    assert_eq!(
        h.ok(&ws, &["context", "get", WF, "k.md"]).stdout,
        "newer value"
    );
}

/// The listing shows a changed manifest that can't be fetched: the read
/// fails rather than answer from the local manifest.
#[test]
fn a_listed_manifest_whose_fetch_fails_is_an_error() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("ws");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"local value");
    h.ok(&ws, &["context", "get", WF, "k.md"]);
    change_key_elsewhere(&s3, &ws, b"newer value", true);
    let manifest = format!("{}/{WF}/ctx/manifest.json", prefix_of(&ws));
    s3.fail("GET", &manifest, 500);

    let out = h.get(&ws, "k.md");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!out.stdout.contains("local value"), "{}", out.describe());
    assert!(out.stdout.contains("may be stale"), "{}", out.describe());

    s3.clear_faults();
    assert_eq!(
        h.ok(&ws, &["context", "get", WF, "k.md"]).stdout,
        "newer value"
    );
}

/// The bucket's state file moved past this host's copy and its fetch
/// fails: `koto status` fails and says so instead of reading the local log.
#[test]
fn a_state_whose_fetch_fails_is_an_error_not_the_local_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("ws");
    h.init(&ws, &[]);
    h.ok(&ws, &["status", WF]);
    let state = format!("{}/{WF}/koto-{WF}.state.jsonl", prefix_of(&ws));
    let mut bytes = s3.object(&state).unwrap();
    bytes.extend_from_slice(b"\n");
    s3.put(&state, &bytes);
    s3.fail("GET", &state, 404);

    let out = h.koto(&ws, &["status", WF]);
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(out.stdout.contains("may be stale"), "{}", out.describe());

    s3.clear_faults();
    h.ok(&ws, &["status", WF]);
}

/// The state file of `WF` on this host.
fn state_path(h: &Host) -> PathBuf {
    h.home
        .join(".koto/sessions")
        .join(WF)
        .join(format!("koto-{WF}.state.jsonl"))
}

/// Point this host's sync stamp for `WF` at `dir`'s prefix, as a sync run
/// from `dir` by a koto without the guard left it.
fn poison_stamp(h: &Host, dir: &Path) {
    let stamp = h
        .home
        .join(".koto/sessions")
        .join(WF)
        .join("cloud-sync.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&stamp).unwrap()).unwrap();
    value["state_key"] = format!("{}/{WF}/koto-{WF}.state.jsonl", prefix_of(dir)).into();
    std::fs::write(&stamp, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// A stamp that a wrong-directory run rewrote to name that directory
/// doesn't let a read from there through: the prefix the session was
/// created under, in its header, decides.
#[test]
fn a_stamp_rewritten_by_a_wrong_directory_run_lets_nothing_through() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    h.add(&ws, "topic.md", b"old topic");
    copy_session(&s3, &ws, &root);
    h.add(&ws, "topic.md", b"new topic");
    h.ok(&ws, &["context", "get", WF, "topic.md"]);
    poison_stamp(&h, &root);

    let out = h.get(&root, "topic.md");
    assert_refused(&out, "context get", &ws);
    assert!(!out.stdout.contains("old topic"), "{}", out.describe());
    assert_eq!(
        h.ok(&ws, &["context", "get", WF, "topic.md"]).stdout,
        "new topic"
    );
}

/// A session recorded before koto named the prefix in its origin is bound
/// to its anchor alone, so a stamp naming another directory doesn't let a
/// read from there through either, and the anchor still reads.
#[test]
fn a_session_without_a_recorded_prefix_is_bound_to_its_anchor() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"value");
    // Drop the prefix from the header, locally and in the bucket, as an
    // older koto wrote it.
    let path = state_path(&h);
    let text = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    header["origin"]["store"]
        .as_object_mut()
        .unwrap()
        .remove("prefix")
        .expect("a new session records its prefix");
    let old = format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest);
    std::fs::write(&path, &old).unwrap();
    s3.put(
        &format!("{}/{WF}/koto-{WF}.state.jsonl", prefix_of(&ws)),
        old.as_bytes(),
    );
    assert_eq!(h.ok(&ws, &["context", "get", WF, "k.md"]).stdout, "value");
    poison_stamp(&h, &root);

    assert_refused(&h.get(&root, "k.md"), "context get", &ws);
    assert_eq!(h.ok(&ws, &["context", "get", WF, "k.md"]).stdout, "value");
}

/// koto's own read of a session bound elsewhere, here the state read
/// `koto next` makes before its execution-anchor check refuses, takes
/// nothing from this directory's prefix: the older copy planted there
/// never reaches the local log.
#[test]
fn an_unguarded_read_never_pulls_another_directory_s_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    copy_session(&s3, &ws, &root);
    // A write from the session's own directory moves its log past the copy
    // planted under the root's prefix.
    h.ok(
        &ws,
        &[
            "decisions",
            "record",
            WF,
            "--with-data",
            r#"{"choice":"a","rationale":"b"}"#,
        ],
    );
    let before = std::fs::read(state_path(&h)).unwrap();

    s3.clear_requests();
    let out = h.koto(&root, &["next", WF]);
    assert_ne!(out.code, Some(0), "{}", out.describe());
    let root_prefix = prefix_of(&root);
    let fetched: Vec<_> = s3
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.key.starts_with(&root_prefix))
        .collect();
    assert!(
        fetched.is_empty(),
        "pulled from the root's prefix: {fetched:?}"
    );
    assert_eq!(std::fs::read(state_path(&h)).unwrap(), before);
    let decisions = h.ok(&ws, &["decisions", "list", WF]);
    assert!(
        decisions.stdout.contains(r#""choice":"a""#),
        "{}",
        decisions.describe()
    );
}
