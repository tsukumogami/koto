//! `koto session index audit|declare|revoke` (koto#610).
//!
//! The audit walks every workspace prefix and the name index, backfills the
//! entry each state object lacks, optionally prunes entries whose prefix
//! holds no state for their name, and records the audit at
//! `koto-names/_audit`. `declare` walks again and writes
//! `koto-names/_complete` once the probation, measured in the bucket's own
//! time, has passed and no state object lacks its entry. A later audit that
//! finds an entry-less state object newer than the declaration's audit
//! revokes it; `revoke` deletes it on request.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`; no real bucket is involved.

#![cfg(unix)]

#[path = "support/declaration.rs"]
mod declaration;
#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const BUCKET: &str = "koto-session-index";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const DECLARATION: &str = "koto-names/_complete";
const AUDIT: &str = "koto-names/_audit";
const MACHINE: &str = "ops-host";
const CUT_MARKER: &str = "\u{2026}[cut]";

const P1: &str = "1111111111111111";
const P2: &str = "2222222222222222";
const P3: &str = "3333333333333333";

const DAY: u64 = 24 * 60 * 60;

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

    /// The JSON object on stdout's last line.
    fn json(&self) -> serde_json::Value {
        let last = self.stdout.trim().lines().last().unwrap_or_default();
        serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {}", self.describe()))
    }
}

/// One host: its own `HOME` and template cache, and directories to run
/// koto from, all sharing the host's one local session store.
struct Host {
    root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: String,
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
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
            .env("KOTO_MACHINE_NAME", MACHINE)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .args(args);
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

    /// `koto session index <args>` from the host's `ops` directory.
    fn index(&self, args: &[&str]) -> Out {
        let mut all = vec!["session", "index"];
        all.extend_from_slice(args);
        self.koto(&self.dir("ops"), &all)
    }

    /// [`Host::index`], with `input` on stdin.
    fn index_with_input(&self, args: &[&str], input: &str) -> Out {
        let mut all = vec!["session", "index"];
        all.extend_from_slice(args);
        let mut cmd = self.cmd(&self.dir("ops"), &all);
        cmd.write_stdin(input.as_bytes().to_vec());
        self.run(cmd)
    }

    /// [`Host::index`], expecting exit 0.
    fn index_ok(&self, args: &[&str]) -> Out {
        let out = self.index(args);
        assert_eq!(out.code, Some(0), "index {:?}: {}", args, out.describe());
        out
    }

    /// `koto init WF` from `dir`.
    fn init(&self, dir: &Path) {
        std::fs::write(dir.join("flow.md"), TEMPLATE).unwrap();
        self.ok(dir, &["init", WF, "--template", "flow.md"]);
    }
}

fn state_key(prefix: &str, name: &str) -> String {
    format!("{prefix}/{name}/koto-{name}.state.jsonl")
}

fn entry(name: &str, prefix: &str) -> String {
    format!("koto-names/{name}/{prefix}")
}

/// Store a state object for `name` under `prefix`, as a writer that keeps
/// no index entry would.
fn plant(s3: &FakeS3, prefix: &str, name: &str) -> String {
    let key = state_key(prefix, name);
    s3.put(&key, b"{}\n");
    key
}

/// Every listing, as `(prefix, delimiter)`, in arrival order.
fn listings(s3: &FakeS3) -> Vec<(String, Option<String>)> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.key.is_empty())
        .map(|r| (r.list_prefix.unwrap_or_default(), r.delimiter))
        .collect()
}

fn listed(s3: &FakeS3, prefix: &str, delimiter: Option<&str>) -> bool {
    listings(s3)
        .iter()
        .any(|(p, d)| p == prefix && d.as_deref() == delimiter)
}

/// Every PUT and DELETE, as `(method, key)`, in arrival order.
fn writes(s3: &FakeS3) -> Vec<(String, String)> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "PUT" || r.method == "DELETE")
        .map(|r| (r.method, r.key))
        .collect()
}

fn puts(s3: &FakeS3) -> Vec<String> {
    writes(s3)
        .into_iter()
        .filter(|(m, _)| m == "PUT")
        .map(|(_, k)| k)
        .collect()
}

fn deletes(s3: &FakeS3) -> Vec<String> {
    writes(s3)
        .into_iter()
        .filter(|(m, _)| m == "DELETE")
        .map(|(_, k)| k)
        .collect()
}

/// The PUTs of index entries, the two records left out.
fn entry_puts(s3: &FakeS3) -> Vec<String> {
    puts(s3)
        .into_iter()
        .filter(|k| k.starts_with("koto-names/") && k != AUDIT && k != DECLARATION)
        .collect()
}

fn audit_record(s3: &FakeS3) -> serde_json::Value {
    serde_json::from_slice(&s3.object(AUDIT).expect("an audit record")).unwrap()
}

/// Set every state object's LastModified to `now`, the bucket's own clock,
/// and the audit record's to `audit_age` before it.
fn age(s3: &FakeS3, now: SystemTime, audit_age: Duration) {
    for key in s3.keys() {
        if key.ends_with(".state.jsonl") {
            s3.set_last_modified(&key, now);
        }
    }
    s3.set_last_modified(AUDIT, now - audit_age);
}

/// Six days and 23 hours: inside the probation.
fn inside() -> Duration {
    Duration::from_secs(6 * DAY + 23 * 3600)
}

/// Seven days and one minute: past the probation.
fn past() -> Duration {
    Duration::from_secs(7 * DAY + 60)
}

/// No byte in 0x00-0x1F or 0x7F other than a newline.
fn assert_clean(text: &str) {
    let bad: Vec<u8> = text
        .bytes()
        .filter(|b| (*b < 0x20 && *b != b'\n') || *b == 0x7f)
        .collect();
    assert!(bad.is_empty(), "control bytes {bad:?} in {text:?}");
}

// ---------------------------------------------------------------------------
// Command surface
// ---------------------------------------------------------------------------

/// Under the local backend each subcommand refuses, exit 2, and makes no
/// request to any bucket.
#[test]
fn each_subcommand_refuses_under_the_local_backend_with_no_request() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    for args in [
        &["audit", "--yes"][..],
        &["audit", "--prune", "--revoke", "--yes"],
        &["declare", "--no-probation", "--yes"],
        &["revoke", "--yes"],
    ] {
        let mut all = vec!["session", "index"];
        all.extend_from_slice(args);
        let mut cmd = h.cmd(&h.dir("ops"), &all);
        cmd.env("KOTO_SESSION_BACKEND", "local");
        let out = h.run(cmd);
        assert_eq!(out.code, Some(2), "{args:?}: {}", out.describe());
        assert!(out.json()["error"]
            .as_str()
            .unwrap()
            .contains("cloud backend"));
    }
    assert!(s3.requests().is_empty(), "{:?}", s3.requests());
}

/// `--revoke` is accepted only together with `--prune`.
#[test]
fn revoke_is_accepted_only_with_prune() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let out = h.index(&["audit", "--revoke", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(out.stderr.contains("--prune"), "{}", out.describe());
    assert!(s3.requests().is_empty());
}

/// Without `--yes`, each subcommand prints its plan, asks `[y/N]`, and on
/// end of input or anything but `y`/`yes` exits non-zero with no PUT and
/// no DELETE. A `y` writes.
#[test]
fn a_declined_or_unanswered_prompt_writes_nothing() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    s3.put(&entry("gone", P1), b"");

    for (args, input) in [
        (&["audit", "--prune"][..], ""),
        (&["audit"], "n\n"),
        (&["audit"], "maybe\n"),
        (&["revoke"], ""),
    ] {
        s3.clear_requests();
        let out = h.index_with_input(args, input);
        assert_ne!(out.code, Some(0), "{args:?}: {}", out.describe());
        assert!(out.stdout.contains("[y/N]"), "{}", out.describe());
        assert!(writes(&s3).is_empty(), "{args:?}: {:?}", writes(&s3));
    }

    let out = h.index_with_input(&["audit", "--prune"], "n\n");
    assert!(out.stdout.contains(&entry("one", P1)), "{}", out.describe());
    assert!(
        out.stdout.contains(&entry("gone", P1)),
        "{}",
        out.describe()
    );

    s3.clear_requests();
    let out = h.index_with_input(&["audit", "--prune"], "yes\n");
    assert_eq!(out.code, Some(0), "{}", out.describe());
    assert_eq!(
        writes(&s3),
        vec![
            ("PUT".to_string(), entry("one", P1)),
            ("DELETE".to_string(), entry("gone", P1)),
            ("PUT".to_string(), AUDIT.to_string()),
        ]
    );

    // declare, with every check passed, asks too.
    age(&s3, SystemTime::now(), past());
    s3.clear_requests();
    let out = h.index_with_input(&["declare"], "");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    assert!(out.stdout.contains("[y/N]"), "{}", out.describe());
    assert!(writes(&s3).is_empty(), "{:?}", writes(&s3));
}

/// The prompts of `declare --no-probation` and `audit --prune --revoke`
/// each carry exactly one line saying what the run gives up.
#[test]
fn the_give_up_prompts_say_what_is_given_up_in_one_line() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);

    let out = h.index_with_input(&["declare", "--no-probation"], "");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    let lines: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| l.contains("gives up"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", out.describe());
    assert!(lines[0].contains("probation"), "{}", lines[0]);
    assert!(lines[0].contains("older"), "{}", lines[0]);
    assert!(s3.object(DECLARATION).is_none());

    s3.put(DECLARATION, &declaration::valid());
    let out = h.index_with_input(&["audit", "--prune", "--revoke"], "");
    assert_ne!(out.code, Some(0), "{}", out.describe());
    let lines: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| l.contains("gives up"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", out.describe());
    assert!(lines[0].contains("declaration"), "{}", lines[0]);
    assert!(lines[0].contains("full sweep"), "{}", lines[0]);
    assert!(s3.object(DECLARATION).is_some());

    // Without --prune --revoke or --no-probation, no such line.
    let out = h.index_with_input(&["audit"], "");
    assert!(!out.stdout.contains("gives up"), "{}", out.describe());
}

// ---------------------------------------------------------------------------
// Audit walk and backfill
// ---------------------------------------------------------------------------

/// On three prefixes the audit asks one top-level listing, one listing of
/// each prefix, one of each session (`<p>/ctx/` only where the prefix's
/// listing named it) and one of `koto-names/`, all in the `url` encoding.
/// It PUTs each missing entry, empty, and none that exists; a second run
/// PUTs no entry and writes only the audit record.
#[test]
fn the_audit_walks_each_listing_and_backfills_missing_entries() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    plant(&s3, P1, "ctx");
    plant(&s3, P2, "one");
    plant(&s3, P3, "two");
    s3.put(&format!("{P3}/two/ctx/k.md"), b"x");
    s3.put(&entry("one", P2), b"");

    let out = h.index_ok(&["audit", "--yes"]);
    assert!(listed(&s3, "", Some("/")));
    for p in [P1, P2, P3] {
        assert!(listed(&s3, &format!("{p}/"), Some("/")), "{p}");
    }
    for (p, name) in [(P1, "one"), (P1, "ctx"), (P2, "one"), (P3, "two")] {
        assert!(
            listed(&s3, &format!("{p}/{name}/"), Some("/")),
            "{p}/{name}"
        );
    }
    assert!(listed(&s3, "koto-names/", None));
    for p in [P2, P3] {
        assert!(!listed(&s3, &format!("{p}/ctx/"), Some("/")), "{p}/ctx/");
    }
    assert!(s3
        .requests()
        .iter()
        .all(|r| r.method != "GET" || !r.key.is_empty() || r.url_encoded));

    let mut written = entry_puts(&s3);
    written.sort();
    assert_eq!(
        written,
        vec![entry("ctx", P1), entry("one", P1), entry("two", P3)]
    );
    for key in &written {
        assert_eq!(s3.object(key).unwrap(), b"");
    }
    assert!(out.stdout.contains(
        "prefixes 3, sessions 4, entries written 3, entries pruned 0, stale entries 0, keys skipped 0"
    ), "{}", out.describe());

    s3.clear_requests();
    h.index_ok(&["audit", "--yes"]);
    assert_eq!(writes(&s3), vec![("PUT".to_string(), AUDIT.to_string())]);
}

/// The audit record is JSON under 4 KiB with the six fields, replaced by
/// each run. One that is oversized, isn't JSON or lacks a field reads as no
/// audit, and `declare` refuses on it.
#[test]
fn the_audit_record_reads_back_strictly() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);
    let bytes = s3.object(AUDIT).unwrap();
    assert!(bytes.len() < 4096);
    let record = audit_record(&s3);
    for field in [
        "started_at",
        "finished_at",
        "prefixes",
        "sessions",
        "entries_written",
        "entries_pruned",
    ] {
        assert!(record.get(field).is_some(), "{field}: {record}");
    }
    assert_eq!(record["prefixes"], 1);
    assert_eq!(record["sessions"], 1);
    assert_eq!(record["entries_written"], 1);

    let mut missing = record.clone();
    missing.as_object_mut().unwrap().remove("entries_pruned");
    for bad in [
        serde_json::to_vec(&missing).unwrap(),
        b"not json".to_vec(),
        vec![b' '; 4097],
    ] {
        s3.put(AUDIT, &bad);
        s3.clear_requests();
        let out = h.index(&["declare", "--no-probation", "--yes"]);
        assert_eq!(out.code, Some(2), "{}", out.describe());
        assert!(out.json()["error"].as_str().unwrap().contains(AUDIT));
        assert!(!puts(&s3).contains(&DECLARATION.to_string()));
    }
}

/// `declare` with no audit record refuses and sends no PUT.
#[test]
fn declare_refuses_with_no_audit_record() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    s3.put(&entry("one", P1), b"");
    let out = h.index(&["declare", "--no-probation", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(puts(&s3).is_empty(), "{:?}", puts(&s3));
}

// ---------------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------------

/// On an undeclared bucket `--prune` deletes exactly the entries whose
/// prefix holds no state for their name; every other key is untouched.
#[test]
fn prune_deletes_only_stale_entries() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    plant(&s3, P2, "two");
    s3.put(&format!("{P2}/two/ctx/k.md"), b"value");
    s3.put(&entry("one", P1), b"");
    s3.put(&entry("two", P2), b"");
    s3.put(&entry("one", P2), b"");
    s3.put(&entry("gone", P3), b"");
    let before: Vec<(String, Vec<u8>)> = s3
        .keys()
        .into_iter()
        .map(|k| (k.clone(), s3.object(&k).unwrap()))
        .collect();

    let out = h.index_ok(&["audit", "--prune", "--yes"]);
    let mut gone = deletes(&s3);
    gone.sort();
    assert_eq!(gone, vec![entry("gone", P3), entry("one", P2)]);
    assert!(
        out.stdout.contains("entries pruned 2"),
        "{}",
        out.describe()
    );
    for (key, bytes) in before {
        if gone.contains(&key) {
            assert!(s3.object(&key).is_none(), "{key}");
        } else {
            assert_eq!(s3.object(&key).as_ref(), Some(&bytes), "{key}");
        }
    }
}

/// On a declared bucket, `--prune` without `--revoke` refuses with no
/// write; without `--prune` the audit counts stale entries and keeps them.
#[test]
fn prune_on_a_declared_bucket_needs_revoke() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    s3.put(&entry("one", P1), b"");
    s3.put(&entry("gone", P1), b"");
    s3.put(DECLARATION, &declaration::valid());

    let out = h.index(&["audit", "--prune", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(writes(&s3).is_empty(), "{:?}", writes(&s3));
    assert!(s3.object(DECLARATION).is_some());

    let out = h.index_ok(&["audit", "--yes"]);
    assert!(deletes(&s3).is_empty(), "{:?}", deletes(&s3));
    assert!(out.stdout.contains("stale entries 1"), "{}", out.describe());
    assert!(s3.object(&entry("gone", P1)).is_some());
    assert!(s3.object(DECLARATION).is_some());
}

/// On a declared bucket, `--prune --revoke` deletes the declaration before
/// any entry.
#[test]
fn prune_with_revoke_deletes_the_declaration_first() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    s3.put(&entry("one", P1), b"");
    s3.put(&entry("gone", P1), b"");
    s3.put(&entry("gone", P2), b"");
    s3.put(DECLARATION, &declaration::valid());

    h.index_ok(&["audit", "--prune", "--revoke", "--yes"]);
    let gone = deletes(&s3);
    assert_eq!(gone.first().map(String::as_str), Some(DECLARATION));
    assert_eq!(gone.len(), 3, "{gone:?}");
    assert!(s3.object(DECLARATION).is_none());
}

// ---------------------------------------------------------------------------
// Revocation on audit
// ---------------------------------------------------------------------------

/// An entry-less state object older than the declaration's audit start is
/// backfilled and doesn't revoke.
#[test]
fn an_entry_less_object_older_than_the_declared_audit_does_not_revoke() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let key = plant(&s3, P2, "old");
    s3.set_last_modified(&key, SystemTime::now() - Duration::from_secs(30 * DAY));
    // The declaration's audit started 7 d 1 h (host time) before it was
    // declared, and the bucket stamps it now.
    s3.put(DECLARATION, &declaration::valid());

    let out = h.index_ok(&["audit", "--yes"]);
    assert!(s3.object(DECLARATION).is_some(), "{}", out.describe());
    assert!(deletes(&s3).is_empty());
    assert_eq!(entry_puts(&s3), vec![entry("old", P2)]);
}

/// The issue's first scenario: an older writer's session under a declared
/// bucket reads not-found from another prefix, the next audit revokes the
/// declaration and indexes it, and then it refuses as unlocated.
#[test]
fn an_older_writer_s_session_under_a_declared_bucket_revokes_on_the_next_audit() {
    let s3 = FakeS3::start(BUCKET);
    let writer = Host::new(&s3);
    let b = writer.dir("b");
    writer.init(&b);
    let pb = prefix_of(&b);
    s3.remove(&entry(WF, &pb));
    assert!(s3.keys_under("koto-names/").is_empty());
    s3.put(DECLARATION, &declaration::valid());
    // The session's state object is later than the declaration's audit
    // start (7 d 1 h before the declaration's LastModified).

    let reader = Host::new(&s3);
    let a = reader.dir("a");
    let out = reader.koto(&a, &["status", WF]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(
        !out.stdout.contains("session_unlocated"),
        "{}",
        out.describe()
    );

    s3.clear_requests();
    let out = reader.index_ok(&["audit", "--yes"]);
    assert!(deletes(&s3).contains(&DECLARATION.to_string()));
    assert!(s3.object(DECLARATION).is_none());
    assert!(
        out.stdout.contains(&format!("{pb}/{WF}")),
        "{}",
        out.describe()
    );
    assert!(s3.object(&entry(WF, &pb)).is_some());

    let out = reader.koto(&a, &["status", WF]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    let err = out.json();
    assert_eq!(err["code"], "session_unlocated", "{}", out.describe());
    assert_eq!(err["reason"], "no_local_copy", "{}", out.describe());
}

/// `revoke` sends one DELETE, of the declaration, and nothing else; a later
/// lookup sweeps the prefixes and refuses as unlocated.
#[test]
fn a_revoked_declaration_returns_lookups_to_the_sweep() {
    let s3 = FakeS3::start(BUCKET);
    let writer = Host::new(&s3);
    let b = writer.dir("b");
    writer.init(&b);
    let pb = prefix_of(&b);
    s3.remove(&entry(WF, &pb));
    s3.put(DECLARATION, &declaration::valid());

    let reader = Host::new(&s3);
    s3.clear_requests();
    reader.index_ok(&["revoke", "--yes"]);
    let requests: Vec<(String, String)> = s3
        .requests()
        .into_iter()
        .map(|r| (r.method, r.key))
        .collect();
    assert_eq!(
        requests,
        vec![("DELETE".to_string(), DECLARATION.to_string())]
    );
    assert!(s3.object(DECLARATION).is_none());

    let a = reader.dir("a");
    s3.clear_requests();
    let out = reader.koto(&a, &["status", WF]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    let err = out.json();
    assert_eq!(err["code"], "session_unlocated", "{}", out.describe());
    assert_eq!(err["reason"], "no_local_copy", "{}", out.describe());
    assert!(listed(&s3, "", Some("/")), "{:?}", listings(&s3));
    assert!(
        listed(&s3, &format!("{pb}/{WF}/"), Some("/")),
        "{:?}",
        listings(&s3)
    );
}

// ---------------------------------------------------------------------------
// Declare
// ---------------------------------------------------------------------------

/// `declare` walks the bucket again and PUTs one declaration that parses,
/// carrying the audit record's six fields.
#[test]
fn declare_walks_again_and_writes_a_declaration_that_parses() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    plant(&s3, P2, "two");
    h.index_ok(&["audit", "--yes"]);
    age(&s3, SystemTime::now(), past());

    s3.clear_requests();
    h.index_ok(&["declare", "--yes"]);
    assert!(listed(&s3, "", Some("/")));
    assert!(listed(&s3, &format!("{P1}/"), Some("/")));
    assert!(listed(&s3, &format!("{P2}/"), Some("/")));
    assert!(listed(&s3, "koto-names/", None));
    assert_eq!(puts(&s3), vec![DECLARATION.to_string()]);

    let bytes = s3.object(DECLARATION).unwrap();
    assert!(bytes.len() < 4096);
    let declared: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(declared["version"], 1);
    assert_eq!(declared["machine"], MACHINE);
    assert!(declared["first_release"].as_str().unwrap().starts_with('v'));
    assert!(declared["declared_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(declared["audit"], audit_record(&s3));

    // A lookup reads it as declared: a name nowhere reads absent without
    // sweeping the other prefixes.
    s3.clear_requests();
    let out = h.koto(&h.dir("ops"), &["status", "nowhere"]);
    assert_ne!(out.code, Some(0), "{}", out.describe());
    for p in [P1, P2] {
        assert!(
            !listed(&s3, &format!("{p}/nowhere/"), Some("/")),
            "{:?}",
            listings(&s3)
        );
    }
}

#[test]
fn declare_refuses_inside_the_probation_window() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);
    let now = SystemTime::now();

    age(&s3, now, inside());
    s3.clear_requests();
    let out = h.index(&["declare", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("probation hasn't passed"),
        "{}",
        out.describe()
    );
    assert!(!puts(&s3).contains(&DECLARATION.to_string()));

    age(&s3, now, past());
    s3.clear_requests();
    h.index_ok(&["declare", "--yes"]);
    assert_eq!(puts(&s3), vec![DECLARATION.to_string()]);
}

#[test]
fn declare_refuses_and_names_an_entry_less_state_object_newer_than_the_audit() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);
    let now = SystemTime::now();
    age(&s3, now, past());
    let late = plant(&s3, P2, WF);
    s3.set_last_modified(&late, now);

    for args in [
        &["declare", "--yes"][..],
        &["declare", "--no-probation", "--yes"],
    ] {
        s3.clear_requests();
        let out = h.index(args);
        assert_eq!(out.code, Some(2), "{args:?}: {}", out.describe());
        assert!(out.stdout.contains(&late), "{}", out.describe());
        assert!(!puts(&s3).contains(&DECLARATION.to_string()));
    }
}

/// A listing in declare's walk that still fails after its retries refuses
/// the declaration, with or without `--no-probation`, naming the listing.
#[test]
fn declare_refuses_when_a_listing_still_fails() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    plant(&s3, P2, "two");
    h.index_ok(&["audit", "--yes"]);
    age(&s3, SystemTime::now(), past());
    let listing = format!("{P2}/");
    s3.fail("LIST", &listing, 503);

    for args in [
        &["declare", "--yes"][..],
        &["declare", "--no-probation", "--yes"],
    ] {
        s3.clear_requests();
        let out = h.index(args);
        assert_eq!(out.code, Some(3), "{args:?}: {}", out.describe());
        assert!(out.stdout.contains(&listing), "{}", out.describe());
        assert!(!puts(&s3).contains(&DECLARATION.to_string()));
    }
}

/// A prune that races a create: the creator's state object lands right
/// after the listing that showed the prefix holding no session of that
/// name, so the prune deletes a live session's entry. The next declare
/// sees the entry-less object and refuses.
#[test]
fn a_prune_racing_a_create_ends_in_a_refused_declaration() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    plant(&s3, P2, "other");
    s3.put(&entry(WF, P2), b"");
    let created = state_key(P2, WF);
    let listing = format!("{P2}/");
    s3.put_after("LIST", &listing, &created, b"{}\n");

    h.index_ok(&["audit", "--prune", "--yes"]);
    let requests = s3.requests();
    let listed_at = requests
        .iter()
        .position(|r| {
            r.method == "GET"
                && r.key.is_empty()
                && r.list_prefix.as_deref() == Some(listing.as_str())
        })
        .expect("the prefix's listing");
    let deleted_at = requests
        .iter()
        .position(|r| r.method == "DELETE" && r.key == entry(WF, P2))
        .expect("the entry's DELETE");
    assert!(listed_at < deleted_at);
    assert!(s3.object(&created).is_some());
    assert!(s3.object(&entry(WF, P2)).is_none());

    s3.clear_requests();
    let out = h.index(&["declare", "--no-probation", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(out.stdout.contains(&created), "{}", out.describe());
    assert!(!puts(&s3).contains(&DECLARATION.to_string()));
}

#[test]
fn rerunning_the_audit_restarts_the_probation() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);
    let first = audit_record(&s3);
    age(&s3, SystemTime::now(), past());

    s3.clear_requests();
    h.index_ok(&["audit", "--yes"]);
    assert_eq!(puts(&s3), vec![AUDIT.to_string()]);
    let second = audit_record(&s3);
    assert!(
        second["started_at"].as_str().unwrap() > first["started_at"].as_str().unwrap(),
        "{first} then {second}"
    );

    s3.clear_requests();
    let out = h.index(&["declare", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    let error = out.json()["error"].as_str().unwrap().to_string();
    assert!(error.contains("probation hasn't passed"), "{error}");
    // The record's LastModified is fresh: no state object is newer.
    assert!(
        error.contains("no state object is newer than the audit record"),
        "{error}"
    );
    assert!(!puts(&s3).contains(&DECLARATION.to_string()));
}

/// The bucket's clock, not the host's, decides the probation and the
/// freshness of an entry-less object.
#[test]
fn declare_ignores_a_skewed_host_clock() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "one");
    h.index_ok(&["audit", "--yes"]);
    let host_now = SystemTime::now();
    let all = |at: SystemTime| {
        for key in s3.keys() {
            s3.set_last_modified(&key, at);
        }
    };

    // The bucket runs 30 days behind: the host clock puts the audit 30
    // days back, but in the bucket's time it is 6 d 23 h old.
    let behind = host_now - Duration::from_secs(30 * DAY);
    all(behind);
    s3.set_last_modified(AUDIT, behind - inside());
    s3.clear_requests();
    let out = h.index(&["declare", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(out.json()["error"]
        .as_str()
        .unwrap()
        .contains("probation hasn't passed"));
    assert!(!puts(&s3).contains(&DECLARATION.to_string()));

    // Still behind: an entry-less object later than the audit record
    // refuses even without the probation.
    let late = plant(&s3, P2, WF);
    s3.set_last_modified(&late, behind - Duration::from_secs(3600));
    s3.clear_requests();
    let out = h.index(&["declare", "--no-probation", "--yes"]);
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert!(out.stdout.contains(&late), "{}", out.describe());
    assert!(!puts(&s3).contains(&DECLARATION.to_string()));
    s3.remove(&late);

    // The bucket runs 30 days ahead: the host clock puts the audit in the
    // future, but in the bucket's time it is 7 d 1 min old.
    let ahead = host_now + Duration::from_secs(30 * DAY);
    all(ahead);
    s3.set_last_modified(AUDIT, ahead - past());
    s3.clear_requests();
    h.index_ok(&["declare", "--yes"]);
    assert_eq!(puts(&s3), vec![DECLARATION.to_string()]);
}

// ---------------------------------------------------------------------------
// Bucket text
// ---------------------------------------------------------------------------

#[test]
fn audit_skips_and_reports_keys_that_fail_validation() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "good");
    plant(&s3, P2, "fine");
    let planted = [
        format!("{P1}/9bad/koto-9bad.state.jsonl"),
        format!("{P1}/has space/koto-has space.state.jsonl"),
        "koto-names/good/not-a-prefix".to_string(),
        format!("koto-names/good/{P1}/extra"),
        format!("koto-names/9bad/{P1}"),
    ];
    for key in &planted {
        s3.put(key, b"");
    }

    let out = h.index_ok(&["audit", "--prune", "--yes"]);
    assert!(
        out.stdout
            .contains(&format!("keys skipped {}", planted.len())),
        "{}",
        out.describe()
    );
    let mut written = entry_puts(&s3);
    written.sort();
    assert_eq!(written, vec![entry("fine", P2), entry("good", P1)]);
    for (_, key) in writes(&s3) {
        for bad in ["9bad", "has space", "not-a-prefix", "extra"] {
            assert!(!key.contains(bad), "{key}");
        }
    }
    for key in &planted {
        assert!(s3.object(key).is_some(), "{key}");
    }
}

#[test]
fn audit_output_escapes_control_characters_in_bucket_keys() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "good");
    let long = "y".repeat(400);
    for key in [
        format!("{P1}/\u{1b}[2Jevil/koto-x.state.jsonl"),
        format!("koto-names/\u{1b}[2Jevil/{P1}"),
        format!("{P1}/9{long}/koto-x.state.jsonl"),
        format!("koto-names/{long}/not-a-prefix"),
    ] {
        s3.put(&key, b"");
    }

    let out = h.index_ok(&["audit", "--yes"]);
    assert_clean(&out.stdout);
    assert_clean(&out.stderr);
    assert!(out.stdout.contains("\\u{1B}[2Jevil"), "{}", out.describe());
    assert!(out.stdout.contains("keys skipped 4"), "{}", out.describe());
    assert_eq!(
        out.stdout.matches(CUT_MARKER).count(),
        2,
        "{}",
        out.describe()
    );
    assert!(!out.stdout.contains(&long), "{}", out.describe());
}

#[test]
fn audit_backfills_a_session_named_ctx() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant(&s3, P1, "ctx");
    plant(&s3, P1, "other");
    plant(&s3, P2, "other");

    h.index_ok(&["audit", "--yes"]);
    assert!(listed(&s3, &format!("{P1}/ctx/"), Some("/")));
    assert!(!listed(&s3, &format!("{P2}/ctx/"), Some("/")));
    let mut written = entry_puts(&s3);
    written.sort();
    assert_eq!(
        written,
        vec![entry("ctx", P1), entry("other", P1), entry("other", P2)]
    );
    assert_eq!(s3.object(&entry("ctx", P1)).unwrap(), b"");
    assert!(s3.object(&entry("ctx", P2)).is_none());

    s3.clear_requests();
    h.index_ok(&["audit", "--yes"]);
    assert!(entry_puts(&s3).is_empty(), "{:?}", entry_puts(&s3));
}
