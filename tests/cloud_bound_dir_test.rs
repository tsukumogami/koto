//! A command naming a session from another directory under the cloud
//! backend (koto#457, koto#465, koto#466).
//!
//! The cloud backend finds a session in the bucket under a prefix, while
//! every workspace on a host shares one local store. A session records the
//! prefix it was created under (`origin.store.prefix`), and every read,
//! write and delete of it uses the copy under that prefix, its home, from
//! any directory: an older copy another directory's prefix holds under the
//! same name is never read or written. Read from anywhere but its own
//! workspace, the home is read strictly: a bucket that can't answer is an
//! error, not the local copy.
//!
//! A session recorded before koto named the prefix is located from outside
//! its own directory among two candidates, its anchor's prefix and the
//! prefix this host's sync stamp names, and read there once the copy
//! carries its `session_id`.
//!
//! A session koto can't locate refuses with `session_unlocated`: one with
//! no copy on this host and none under the caller's prefix, one whose
//! recorded copy is missing or another session's, and one recorded before
//! koto named the prefix whose candidates hold no copy, two, or one that
//! is another session's.
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
use std::collections::BTreeMap;
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

    /// The JSON the command printed: its whole output, or its last line.
    fn json(&self) -> serde_json::Value {
        let out = self.stdout.trim();
        serde_json::from_str(out)
            .or_else(|_| serde_json::from_str(out.lines().last().unwrap_or_default()))
            .unwrap_or_else(|e| panic!("no JSON ({e}): {}", self.describe()))
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

    fn run(&self, mut cmd: Command) -> Out {
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn koto(&self, dir: &Path, args: &[&str]) -> Out {
        self.run(self.cmd(dir, args))
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

    fn add_out(&self, dir: &Path, key: &str, content: &[u8]) -> Out {
        let mut cmd = self.cmd(dir, &["context", "add", WF, key]);
        cmd.write_stdin(content.to_vec());
        self.run(cmd)
    }

    fn add(&self, dir: &Path, key: &str, content: &[u8]) {
        let out = self.add_out(dir, key, content);
        assert_eq!(out.code, Some(0), "context add: {}", out.describe());
    }

    fn get(&self, dir: &Path, key: &str) -> Out {
        self.koto(dir, &["context", "get", WF, key])
    }

    fn session_dir(&self) -> PathBuf {
        self.home.join(".koto/sessions").join(WF)
    }

    /// Every file of `WF`'s local copy and its bytes' hash, except the
    /// host-local read spool, which a read from outside the session's own
    /// tick appends to by design.
    fn local_files(&self) -> BTreeMap<PathBuf, String> {
        use sha2::{Digest, Sha256};
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.file_name().unwrap() != "context-reads.jsonl" {
                    let bytes = std::fs::read(&path).unwrap();
                    out.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        hex::encode(Sha256::digest(bytes)),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        let dir = self.session_dir();
        walk(&dir, &dir, &mut out);
        out
    }
}

/// The state file of `WF` on host `h`.
fn state_path(h: &Host) -> PathBuf {
    h.session_dir().join(format!("koto-{WF}.state.jsonl"))
}

/// The bucket key of `WF`'s state object under `dir`'s prefix.
fn state_key(dir: &Path) -> String {
    format!("{}/{WF}/koto-{WF}.state.jsonl", prefix_of(dir))
}

/// The bucket objects of `WF` under `dir`'s prefix.
fn objects_under(s3: &FakeS3, dir: &Path) -> Vec<String> {
    s3.keys_under(&format!("{}/{WF}/", prefix_of(dir)))
}

/// Every bucket object and its bytes.
fn bucket(s3: &FakeS3) -> BTreeMap<String, Vec<u8>> {
    s3.keys()
        .into_iter()
        .map(|k| {
            let bytes = s3.object(&k).unwrap();
            (k, bytes)
        })
        .collect()
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

/// The requests a command made under `dir`'s prefix: object requests and
/// listings.
fn asked_under(s3: &FakeS3, dir: &Path) -> Vec<fake_s3::Request> {
    let prefix = prefix_of(dir);
    s3.requests()
        .into_iter()
        .filter(|r| {
            r.key.starts_with(&prefix)
                || r.list_prefix
                    .as_deref()
                    .is_some_and(|p| p.starts_with(&prefix))
        })
        .collect()
}

/// The `session_unlocated` refusal of `command`, exit 2, naming `WF` and
/// `reason`.
fn assert_unlocated(out: &Out, command: &str, reason: &str) {
    assert_eq!(out.code, Some(2), "{command}: {}", out.describe());
    let err = out.json();
    assert_eq!(err["code"], "session_unlocated", "{}", out.describe());
    assert_eq!(err["command"], command, "{}", out.describe());
    assert_eq!(err["session"], WF, "{}", out.describe());
    assert_eq!(err["reason"], reason, "{}", out.describe());
    let message = err["error"].as_str().unwrap();
    assert!(message.starts_with("session_unlocated: "), "{message}");
    assert!(message.contains(&format!("'{WF}'")), "{message}");
}

/// Replace the `session_id` in the header of the bucket's state object at
/// `key`, as another session of the same name would carry.
fn plant_other_session(s3: &FakeS3, key: &str) {
    let text = String::from_utf8(s3.object(key).unwrap()).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    header["session_id"] = "00000000-0000-4000-8000-000000000000".into();
    s3.put(
        key,
        format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest).as_bytes(),
    );
}

/// Rewrite the `origin.store.prefix` of `WF`'s local header, or drop it
/// with `None`, as an older or a damaged koto left it. The bucket's copy
/// gets the same header.
fn set_recorded_prefix(h: &Host, s3: &FakeS3, ws: &Path, prefix: Option<&str>) {
    let path = state_path(h);
    let text = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    let store = header["origin"]["store"].as_object_mut().unwrap();
    assert!(
        store.contains_key("prefix"),
        "a new session records its prefix"
    );
    match prefix {
        Some(p) => {
            store.insert("prefix".to_string(), p.into());
        }
        None => {
            store.remove("prefix");
        }
    }
    let old = format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest);
    std::fs::write(&path, &old).unwrap();
    s3.put(&state_key(ws), old.as_bytes());
}

/// Point this host's sync stamp for `WF` at `dir`'s prefix, as a sync run
/// from `dir` by a koto without the home prefix left it.
fn poison_stamp(h: &Host, dir: &Path) {
    let stamp = h.session_dir().join("cloud-sync.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&stamp).unwrap()).unwrap();
    value["state_key"] = state_key(dir).into();
    std::fs::write(&stamp, serde_json::to_vec(&value).unwrap()).unwrap();
}

/// A session created in A reads from an unrelated directory B, and sees a
/// write A makes after that read.
#[test]
fn a_session_reads_from_an_unrelated_directory_and_sees_later_writes() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"v1");

    assert_eq!(h.ok(&b, &["context", "get", WF, "k.md"]).stdout, "v1");
    h.add(&a, "k.md", b"v2");
    assert_eq!(h.ok(&b, &["context", "get", WF, "k.md"]).stdout, "v2");
    assert!(objects_under(&s3, &b).is_empty(), "nothing went under B");

    // `status`, `context list` and `context exists` print the same from
    // both directories.
    for args in [
        &["status", WF][..],
        &["context", "list", WF][..],
        &["context", "exists", WF, "k.md"][..],
        &["context", "exists", WF, "missing.md"][..],
    ] {
        let from_a = h.koto(&a, args);
        let from_b = h.koto(&b, args);
        assert_eq!(from_a.code, from_b.code, "{args:?}: {}", from_b.describe());
        assert_eq!(from_a.stdout, from_b.stdout, "{args:?}");
    }
}

/// The reported case: an older copy of the session sits under the
/// workspace root's prefix. A read from the root reads the session's own
/// copy, never the older one, and asks the root's prefix nothing.
#[test]
fn a_read_from_another_directory_never_serves_another_prefix_s_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    h.add(&ws, "topic.md", b"old topic");
    copy_session(&s3, &ws, &root);
    h.add(&ws, "topic.md", b"new topic");

    s3.clear_requests();
    assert_eq!(
        h.ok(&root, &["context", "get", WF, "topic.md"]).stdout,
        "new topic"
    );
    let asked = asked_under(&s3, &root);
    assert!(
        asked.is_empty(),
        "the read asked the root's prefix: {asked:?}"
    );
}

/// Writes from B go to the session's own copy: A reads them, and the bucket
/// holds nothing for the session under B's prefix.
#[test]
fn writes_from_another_directory_go_to_the_session_s_own_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);

    h.add(&b, "k2.md", b"from b");
    assert_eq!(h.ok(&a, &["context", "get", WF, "k2.md"]).stdout, "from b");
    let manifest = String::from_utf8(
        s3.object(&format!("{}/{WF}/ctx/manifest.json", prefix_of(&a)))
            .unwrap(),
    )
    .unwrap();
    assert!(manifest.contains("k2.md"), "{manifest}");

    h.ok(&b, &["context", "remove", WF, "k2.md"]);
    let exists = h.koto(&a, &["context", "exists", WF, "k2.md"]);
    assert_ne!(exists.code, Some(0), "{}", exists.describe());

    h.ok(
        &b,
        &[
            "decisions",
            "record",
            WF,
            "--with-data",
            r#"{"choice":"a","rationale":"b"}"#,
        ],
    );
    let state = String::from_utf8(s3.object(&state_key(&a)).unwrap()).unwrap();
    assert!(state.contains(r#""choice":"a""#), "{state}");
    assert!(objects_under(&s3, &b).is_empty(), "nothing went under B");
}

/// A tick from a subdirectory of the session's anchor pushes to the
/// session's own copy (koto#465): nothing lands under the subdirectory's
/// prefix, and a read from the anchor sees the tick's event.
#[test]
fn a_tick_from_a_subdirectory_of_the_anchor_pushes_to_the_own_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let sub = h.dir("a/sub");
    h.init(&a, &[]);

    h.ok(
        &sub,
        &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
    );
    assert!(
        objects_under(&s3, &sub).is_empty(),
        "nothing went under sub"
    );
    let state = String::from_utf8(s3.object(&state_key(&a)).unwrap()).unwrap();
    assert!(state.contains(r#""go":"go""#), "{state}");
    let status = h.ok(&a, &["status", WF]).json();
    assert_eq!(status["is_terminal"], true, "{status:#}");
}

/// A tick from a directory outside the session's anchor still refuses, as
/// its anchor check says, and takes nothing from that directory's prefix.
#[test]
fn a_tick_from_outside_the_anchor_refuses_with_the_anchor_mismatch() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("root/ws");
    let root = h.dir("root");
    h.init(&ws, &[]);
    copy_session(&s3, &ws, &root);
    let before = std::fs::read(state_path(&h)).unwrap();

    s3.clear_requests();
    let out = h.koto(&root, &["next", WF]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(
        out.stdout.contains("execution_anchor_mismatch"),
        "{}",
        out.describe()
    );
    let fetched: Vec<_> = asked_under(&s3, &root)
        .into_iter()
        .filter(|r| r.method == "GET")
        .collect();
    assert!(
        fetched.is_empty(),
        "pulled from the root's prefix: {fetched:?}"
    );
    assert_eq!(std::fs::read(state_path(&h)).unwrap(), before);
}

/// Every command that names one session works from another directory as
/// from its own, under its own command label.
#[test]
fn every_guarded_command_works_from_another_directory() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let ws = h.dir("ws");
    let elsewhere = h.dir("elsewhere");
    h.init(&ws, &[]);
    h.add(&ws, "k.md", b"value");

    let cases: &[&[&str]] = &[
        &["context", "get", WF, "k.md"],
        &["context", "exists", WF, "k.md"],
        &["context", "list", WF],
        &["status", WF],
        &["decisions", "list", WF],
        &["overrides", "list", WF],
        &[
            "decisions",
            "record",
            WF,
            "--with-data",
            r#"{"choice":"a","rationale":"b"}"#,
        ],
        &["context", "remove", WF, "k.md"],
    ];
    for args in cases {
        h.ok(&elsewhere, args);
    }
    assert!(objects_under(&s3, &elsewhere).is_empty());
}

/// A session started in `init_dir` with its execution anchor somewhere else
/// reads one copy, the one under the prefix it was created under, from the
/// init directory, the anchor and a third directory alike.
#[test]
fn the_init_directory_the_anchor_and_a_third_directory_read_one_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let init_dir = h.dir("init");
    let anchor = h.dir("anchor");
    let third = h.dir("third");
    h.init(&init_dir, &["--execution-dir", anchor.to_str().unwrap()]);
    h.add(&anchor, "k.md", b"value");

    for dir in [&init_dir, &anchor, &third] {
        assert_eq!(h.ok(dir, &["context", "get", WF, "k.md"]).stdout, "value");
        h.ok(dir, &["status", WF]);
    }
    assert!(objects_under(&s3, &anchor).is_empty());
    assert!(objects_under(&s3, &third).is_empty());
}

/// The local backend's one store answers a name the same from every
/// directory.
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

/// From another directory the session's own copy is read strictly. With
/// its listing failing, a read and an `exists` fail rather than answer from
/// this host's copy, while from the session's own directory the read warns
/// and answers locally, as before. A GET failing after a good listing fails
/// the read from B too.
#[test]
fn a_bucket_failure_fails_a_read_from_another_directory() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"local value");
    let listing = format!("{}/{WF}/", prefix_of(&a));

    s3.fail("LIST", &listing, 500);
    let out = h.get(&b, "k.md");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!out.stdout.contains("local value"), "{}", out.describe());
    let exists = h.koto(&b, &["context", "exists", WF, "k.md"]);
    assert_ne!(exists.code, Some(0), "{}", exists.describe());
    assert_ne!(
        exists.code,
        Some(1),
        "not answered as absent: {}",
        exists.describe()
    );
    let own = h.get(&a, "k.md");
    assert_eq!(own.code, Some(0), "{}", own.describe());
    assert_eq!(own.stdout, "local value");
    assert!(own.stderr.contains("warning"), "{}", own.describe());
    s3.clear_faults();

    // Another host moves the session past this host's copy, and fetching
    // the state object fails after a good listing.
    let mut bytes = s3.object(&state_key(&a)).unwrap();
    bytes.extend_from_slice(b"\n");
    s3.put(&state_key(&a), &bytes);
    s3.fail("GET", &state_key(&a), 500);
    let out = h.get(&b, "k.md");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(!out.stdout.contains("local value"), "{}", out.describe());
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
    let state = state_key(&ws);
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

/// A stamp left naming another prefix's state key never lets a read skip
/// the fetch of the session's own copy as fresh.
#[test]
fn a_stamp_naming_another_prefix_never_skips_the_own_copy_s_fetch() {
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

    s3.clear_requests();
    assert_eq!(
        h.ok(&root, &["context", "get", WF, "topic.md"]).stdout,
        "new topic"
    );
    let fetched_own = s3
        .requests()
        .into_iter()
        .any(|r| r.method == "GET" && r.key == state_key(&ws));
    assert!(fetched_own, "the read skipped the own copy's fetch");
    assert!(asked_under(&s3, &root).is_empty());
}

/// The prefixes of the delimiter listings in the request log: the listings
/// that locate a session recording no prefix, one per candidate. Every
/// other listing koto makes of a session reads its whole prefix.
fn candidate_listings(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter(|r| r.delimiter.is_some())
        .filter_map(|r| r.list_prefix)
        .collect()
}

/// A session recorded before koto named the prefix, and one whose recorded
/// prefix isn't of the shape koto derives, read as before from their own
/// directory, and from another one too once the copy under the anchor's
/// prefix carries their `session_id`: a stamp naming a prefix that holds no
/// copy doesn't stop it.
#[test]
fn a_session_without_a_usable_recorded_prefix_reads_from_another_directory_once_located() {
    for recorded in [None, Some("NOT-A-PREFIX")] {
        let s3 = FakeS3::start(BUCKET);
        let h = Host::new(&s3, true);
        let ws = h.dir("root/ws");
        let root = h.dir("root");
        let b = h.dir("b");
        h.init(&ws, &[]);
        h.add(&ws, "k.md", b"value");
        set_recorded_prefix(&h, &s3, &ws, recorded);
        assert_eq!(h.ok(&ws, &["context", "get", WF, "k.md"]).stdout, "value");

        assert_eq!(h.ok(&b, &["context", "get", WF, "k.md"]).stdout, "value");
        poison_stamp(&h, &root);
        assert_eq!(h.ok(&root, &["context", "get", WF, "k.md"]).stdout, "value");
        assert!(objects_under(&s3, &b).is_empty());
        assert!(objects_under(&s3, &root).is_empty());
        assert_eq!(h.ok(&ws, &["context", "get", WF, "k.md"]).stdout, "value");
    }
}

/// A session with no recorded prefix whose copy under its anchor's prefix
/// is missing, and separately carries another `session_id`, refuses from B
/// with `session_unlocated`, naming the session and the reason, prints no
/// value and changes no local file.
#[test]
fn an_unrecorded_session_s_missing_or_mismatched_copy_refuses() {
    for (plant, reason) in [
        ("missing", "no_candidate_copy"),
        ("other", "candidate_copy_mismatch"),
    ] {
        let s3 = FakeS3::start(BUCKET);
        let h = Host::new(&s3, true);
        let a = h.dir("a");
        let b = h.dir("b");
        h.init(&a, &[]);
        h.add(&a, "k.md", b"value");
        set_recorded_prefix(&h, &s3, &a, None);
        assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
        match plant {
            "missing" => s3.remove(&state_key(&a)),
            _ => plant_other_session(&s3, &state_key(&a)),
        }
        let before = h.local_files();

        let out = h.get(&b, "k.md");
        assert_unlocated(&out, "context get", reason);
        assert!(!out.stdout.contains("value"), "{plant}: {}", out.describe());
        assert_eq!(out.json()["locations"], serde_json::json!([prefix_of(&a)]));
        let message = out.json()["error"].as_str().unwrap().to_string();
        assert!(message.contains(a.to_str().unwrap()), "{message}");
        assert_eq!(h.local_files(), before, "{plant}: a local file changed");
    }
}

/// With a copy carrying the session's `session_id` under both its anchor's
/// prefix and the prefix the sync stamp names, the read from B refuses and
/// names both.
#[test]
fn matching_copies_under_both_candidates_refuse_and_name_both() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let c = h.dir("c");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    set_recorded_prefix(&h, &s3, &a, None);
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
    copy_session(&s3, &a, &c);
    poison_stamp(&h, &c);
    let before = h.local_files();

    let out = h.get(&b, "k.md");
    assert_unlocated(&out, "context get", "ambiguous_copies");
    assert!(!out.stdout.contains("value"), "{}", out.describe());
    assert_eq!(
        out.json()["locations"],
        serde_json::json!([prefix_of(&a), prefix_of(&c)])
    );
    let message = out.json()["error"].as_str().unwrap().to_string();
    assert!(message.contains(&prefix_of(&a)), "{message}");
    assert!(message.contains(&prefix_of(&c)), "{message}");
    assert_eq!(h.local_files(), before, "a local file changed");
}

/// When the anchor's prefix and the sync stamp's are the same, the read
/// from B lists it once and succeeds, rather than refusing as ambiguous.
#[test]
fn one_prefix_for_the_anchor_and_the_stamp_is_listed_once() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    set_recorded_prefix(&h, &s3, &a, None);
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");

    s3.clear_requests();
    assert_eq!(h.ok(&b, &["context", "get", WF, "k.md"]).stdout, "value");
    assert_eq!(
        candidate_listings(&s3),
        vec![format!("{}/{WF}/", prefix_of(&a))]
    );
}

/// With no copy under the anchor's prefix and one carrying the session's
/// `session_id` under the prefix the sync stamp names, the read from B uses
/// the stamp's.
#[test]
fn the_stamp_s_copy_is_read_when_the_anchor_s_prefix_holds_none() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let c = h.dir("c");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    set_recorded_prefix(&h, &s3, &a, None);
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
    copy_session(&s3, &a, &c);
    for key in objects_under(&s3, &a) {
        s3.remove(&key);
    }
    change_key_elsewhere(&s3, &c, b"the stamp's copy", true);
    poison_stamp(&h, &c);

    assert_eq!(
        h.ok(&b, &["context", "get", WF, "k.md"]).stdout,
        "the stamp's copy"
    );
    assert!(objects_under(&s3, &a).is_empty());
    assert!(objects_under(&s3, &b).is_empty());
}

/// A candidate whose listing fails refuses the read from B with
/// `candidate_unreadable`, printing no value, while the read from the
/// session's own directory keeps its local copy and warns, as before.
#[test]
fn a_failed_candidate_listing_refuses_the_read_from_another_directory() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    set_recorded_prefix(&h, &s3, &a, None);
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
    s3.fail("LIST", &format!("{}/{WF}/", prefix_of(&a)), 500);

    let out = h.get(&b, "k.md");
    assert_unlocated(&out, "context get", "candidate_unreadable");
    assert!(!out.stdout.contains("value"), "{}", out.describe());
    let own = h.get(&a, "k.md");
    assert_eq!(own.code, Some(0), "{}", own.describe());
    assert_eq!(own.stdout, "value");
}

/// The read from B makes one listing per distinct candidate beyond the
/// same read from the session's own directory, with the same cache state
/// on both sides: one when only the anchor names a prefix, two when the
/// stamp names another.
#[test]
fn a_located_read_costs_one_listing_per_distinct_candidate() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let c = h.dir("c");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    set_recorded_prefix(&h, &s3, &a, None);
    let cold = || {
        for file in ["cloud-sync.json", "cloud-manifest.json", "cloud-check.json"] {
            let _ = std::fs::remove_file(h.session_dir().join(file));
        }
    };
    let count = |dir: &Path| {
        s3.clear_requests();
        assert_eq!(h.ok(dir, &["context", "get", WF, "k.md"]).stdout, "value");
        (s3.requests().len(), candidate_listings(&s3))
    };

    cold();
    let (from_a, listed) = count(&a);
    assert!(listed.is_empty(), "from A: {listed:?}");
    cold();
    let (from_b, listed) = count(&b);
    assert_eq!(listed, vec![format!("{}/{WF}/", prefix_of(&a))]);
    assert!(
        from_b <= from_a + 1,
        "cold: {from_b} from B, {from_a} from A"
    );

    // Warm, with the stamp naming a third prefix that holds no copy.
    h.ok(&a, &["context", "get", WF, "k.md"]);
    poison_stamp(&h, &c);
    let (from_a, listed) = count(&a);
    assert!(listed.is_empty(), "from A: {listed:?}");
    poison_stamp(&h, &c);
    let (from_b, listed) = count(&b);
    assert_eq!(
        listed,
        vec![
            format!("{}/{WF}/", prefix_of(&a)),
            format!("{}/{WF}/", prefix_of(&c))
        ]
    );
    assert!(
        from_b <= from_a + 2,
        "warm: {from_b} from B, {from_a} from A"
    );
}

/// The same session read from its own directory makes no candidate
/// listing, and the same requests as before its prefix was dropped.
#[test]
fn an_unrecorded_session_read_from_its_own_directory_is_unchanged() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    let cold = || {
        for file in ["cloud-sync.json", "cloud-manifest.json", "cloud-check.json"] {
            let _ = std::fs::remove_file(h.session_dir().join(file));
        }
    };
    let asked = || {
        s3.requests()
            .into_iter()
            .map(|r| (r.method, r.key, r.list_prefix, r.delimiter))
            .collect::<Vec<_>>()
    };

    cold();
    s3.clear_requests();
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
    let recorded = asked();
    set_recorded_prefix(&h, &s3, &a, None);
    cold();
    s3.clear_requests();
    assert_eq!(h.ok(&a, &["context", "get", WF, "k.md"]).stdout, "value");
    assert_eq!(asked(), recorded);
    assert!(candidate_listings(&s3).is_empty());
}

/// A second host with no local copy of the session reads it only from a
/// directory whose prefix is the session's: from any other it refuses with
/// `session_unlocated` and prints no value.
#[test]
fn a_host_with_no_local_copy_refuses_from_another_prefix() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3, true);
    let second = Host::new(&s3, true);
    let a = first.dir("a");
    first.init(&a, &[]);
    first.add(&a, "k.md", b"value");

    let elsewhere = second.dir("elsewhere");
    let out = second.get(&elsewhere, "k.md");
    assert_unlocated(&out, "context get", "no_local_copy");
    assert!(!out.stdout.contains("\nvalue"), "{}", out.describe());
    assert_eq!(out.json()["locations"][0], prefix_of(&elsewhere));
}

/// A recorded copy that is missing from the bucket, and one that is another
/// session's, read from B refuse with `session_unlocated` and the reason
/// `recorded_copy_mismatch`, print no value and leave every local file as
/// it was.
#[test]
fn a_recorded_copy_that_is_missing_or_another_session_s_refuses() {
    for plant in ["missing", "other"] {
        let s3 = FakeS3::start(BUCKET);
        let h = Host::new(&s3, true);
        let a = h.dir("a");
        let b = h.dir("b");
        h.init(&a, &[]);
        h.add(&a, "k.md", b"value");
        match plant {
            "missing" => s3.remove(&state_key(&a)),
            _ => plant_other_session(&s3, &state_key(&a)),
        }
        let before = h.local_files();

        let out = h.get(&b, "k.md");
        assert_unlocated(&out, "context get", "recorded_copy_mismatch");
        assert!(
            !out.stdout.contains("\nvalue"),
            "{plant}: {}",
            out.describe()
        );
        assert_eq!(out.json()["locations"][0], prefix_of(&a));
        assert_eq!(h.local_files(), before, "{plant}: a local file changed");
    }
}

/// A rebind to C moves nothing in the bucket: the session's keys are the
/// same before and after, and a write from C adds no object under C's
/// prefix. A rebind run from a third directory does the same.
#[test]
fn a_rebind_keeps_the_session_s_copy_where_it_is() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    let c = h.dir("c");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    let keys = s3.keys();

    h.ok(&b, &["session", "rebind", WF, "--to", c.to_str().unwrap()]);
    assert_eq!(s3.keys(), keys, "the rebind moved the copy");
    let state = String::from_utf8(s3.object(&state_key(&a)).unwrap()).unwrap();
    assert!(
        state.contains(c.to_str().unwrap()),
        "the header went up: {state}"
    );

    h.add(&c, "k2.md", b"from c");
    assert!(
        objects_under(&s3, &c).is_empty(),
        "a write from C went under C"
    );
    assert!(objects_under(&s3, &b).is_empty());
    assert_eq!(h.ok(&a, &["context", "get", WF, "k2.md"]).stdout, "from c");
}

/// The reads of a finished session from B change none of its local files
/// and none of its bucket objects.
#[test]
fn reads_from_another_directory_leave_a_finished_session_unchanged() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    h.ok(
        &a,
        &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
    );
    // A read from A first, so the host's copy and stamp are settled.
    h.ok(&a, &["context", "get", WF, "k.md"]);
    let local = h.local_files();
    let objects = bucket(&s3);

    h.ok(&b, &["context", "get", WF, "k.md"]);
    h.ok(&b, &["context", "exists", WF, "k.md"]);
    h.ok(&b, &["context", "list", WF]);
    h.ok(&b, &["status", WF]);
    assert_eq!(h.local_files(), local, "a local file changed");
    assert_eq!(bucket(&s3), objects, "a bucket object changed");
}

/// A prune run from B removes the session's copy under its own prefix. When
/// the copy there carries another `session_id`, the prune removes neither
/// copy and reports the session as skipped.
#[test]
fn a_prune_from_another_directory_removes_only_the_own_copy() {
    for plant in [false, true] {
        let s3 = FakeS3::start(BUCKET);
        let h = Host::new(&s3, true);
        let a = h.dir("a");
        let b = h.dir("b");
        h.init(&a, &[]);
        h.ok(
            &a,
            &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
        );
        copy_session(&s3, &a, &b);
        if plant {
            plant_other_session(&s3, &state_key(&a));
        }
        let under_b = objects_under(&s3, &b);
        std::thread::sleep(std::time::Duration::from_millis(1300));

        let mut cmd = h.cmd(
            &b,
            &[
                "session",
                "prune",
                "--apply",
                "--all",
                "--confirm-short-retention",
            ],
        );
        cmd.env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        let out = h.run(cmd);
        assert_eq!(out.code, Some(0), "{}", out.describe());
        let summary = out.json();
        assert_eq!(
            objects_under(&s3, &b),
            under_b,
            "the copy under B was touched"
        );
        if plant {
            let skipped = summary["skipped"].as_array().unwrap();
            assert!(
                skipped.iter().any(|s| s["name"] == WF
                    && s["reason"]
                        .as_str()
                        .is_some_and(|r| r.contains("another session's"))),
                "{summary:#}"
            );
            assert!(state_path(&h).exists(), "the local copy was removed");
            assert!(
                !objects_under(&s3, &a).is_empty(),
                "the copy under A was removed"
            );
        } else {
            let removed = summary["removed"].as_array().unwrap();
            assert!(removed.iter().any(|r| r["name"] == WF), "{summary:#}");
            assert!(!state_path(&h).exists());
            assert!(
                s3.object(&state_key(&a)).is_none(),
                "the own copy is still there"
            );
        }
    }
}

/// A push from B to a home whose bucket copy turned out to be another
/// session's refuses with a `session conflict` and replaces nothing.
#[test]
fn a_push_from_another_directory_never_replaces_another_session_s_copy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    // Another session's copy lands under A's prefix once B's write has
    // pushed its key's manifest, before the state push.
    let text = String::from_utf8(s3.object(&state_key(&a)).unwrap()).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    header["session_id"] = "00000000-0000-4000-8000-000000000000".into();
    let other = format!("{}\n{}", serde_json::to_string(&header).unwrap(), rest);
    let manifest = format!("{}/{WF}/ctx/manifest.json", prefix_of(&a));
    s3.put_after("PUT", &manifest, &state_key(&a), other.as_bytes());

    let out = h.add_out(&b, "k.md", b"from b");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(
        format!("{}{}", out.stdout, out.stderr).contains("session conflict"),
        "{}",
        out.describe()
    );
    assert_eq!(s3.object(&state_key(&a)).unwrap(), other.into_bytes());
}

/// A read from B costs no more requests than the same read from A, with the
/// same cache state on both sides: cold (no stamp or cached manifest beside
/// the session), then warm.
#[test]
fn a_read_from_another_directory_costs_no_more_requests() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3, true);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a, &[]);
    h.add(&a, "k.md", b"value");
    let cold = || {
        for file in ["cloud-sync.json", "cloud-manifest.json", "cloud-check.json"] {
            let _ = std::fs::remove_file(h.session_dir().join(file));
        }
    };
    let count = |dir: &Path| {
        s3.clear_requests();
        assert_eq!(h.ok(dir, &["context", "get", WF, "k.md"]).stdout, "value");
        s3.requests().len()
    };

    cold();
    let from_a = count(&a);
    cold();
    let from_b = count(&b);
    assert!(from_b <= from_a, "cold: {from_b} from B, {from_a} from A");

    let from_a = count(&a);
    let from_b = count(&b);
    assert!(from_b <= from_a, "warm: {from_b} from B, {from_a} from A");
}
