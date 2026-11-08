//! The bucket's name index on the write path (koto#607).
//!
//! Before a session copy's first state object goes up under a key this
//! host hasn't indexed, koto PUTs the empty entry `koto-names/<name>/<home
//! prefix>`, and nothing a session command does removes one. A transient
//! failure of the entry PUT leaves the push owed, as a failed state PUT
//! does. A refused one (403) reads `koto-names/_complete` once: when the
//! bucket holds no valid declaration the push warns and goes on with the
//! entry owed, and otherwise it fails. Nothing reads the index yet, so no
//! command's reply changes.
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

const BUCKET: &str = "koto-names-index";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const DECLARATION: &str = "koto-names/_complete";

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

/// A workflow whose first state writes a context key from its default
/// action, inside the tick's batch, and then waits.
const TICK_TEMPLATE: &str = r#"---
name: tick
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
        printf ticked | koto context add wf ticked.md
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

    /// `koto init WF` from `dir` with `template`, unchecked.
    fn init_with(&self, dir: &Path, template: &str) -> Out {
        std::fs::write(dir.join("flow.md"), template).unwrap();
        self.koto(dir, &["init", WF, "--template", "flow.md"])
    }

    /// `koto init WF` from `dir`.
    fn init(&self, dir: &Path) {
        let out = self.init_with(dir, TEMPLATE);
        assert_eq!(out.code, Some(0), "init: {}", out.describe());
    }

    fn add_out(&self, dir: &Path, key: &str, content: &[u8]) -> Out {
        let mut cmd = self.cmd(dir, &["context", "add", WF, key]);
        cmd.write_stdin(content.to_vec());
        self.run(cmd)
    }

    fn add(&self, dir: &Path, key: &str, content: &[u8]) -> Out {
        let out = self.add_out(dir, key, content);
        assert_eq!(out.code, Some(0), "context add: {}", out.describe());
        out
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.home.join(".koto/sessions").join(name)
    }

    /// The session's `cloud-sync.json`.
    fn stamp(&self, name: &str) -> serde_json::Value {
        let bytes = std::fs::read(self.session_dir(name).join("cloud-sync.json")).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// What the stamp says the bucket holds: the key, its ETag and the
    /// bytes' length and hash. A read may re-save the stamp, with the same
    /// values, in another field order.
    fn stamped(&self, name: &str) -> Vec<serde_json::Value> {
        let stamp = self.stamp(name);
        [
            "state_key",
            "state_etag",
            "state_len",
            "state_sha256",
            "indexed",
        ]
        .iter()
        .map(|field| match (field, stamp.get(field)) {
            (&"indexed", None) => false.into(),
            (_, value) => value.cloned().unwrap_or_default(),
        })
        .collect()
    }

    fn local_state(&self, name: &str) -> Vec<u8> {
        std::fs::read(
            self.session_dir(name)
                .join(format!("koto-{name}.state.jsonl")),
        )
        .unwrap()
    }

    /// Make `WF`'s copy look as an older release left it: its stamp
    /// without the index flag and no entry in the bucket.
    fn make_older(&self, s3: &FakeS3, dir: &Path) {
        let mut stamp = self.stamp(WF);
        stamp.as_object_mut().unwrap().remove("indexed");
        std::fs::write(
            self.session_dir(WF).join("cloud-sync.json"),
            serde_json::to_vec(&stamp).unwrap(),
        )
        .unwrap();
        s3.remove(&entry(WF, dir));
        assert!(s3.keys_under("koto-names/").is_empty());
    }
}

/// The name-index entry of `name` under `dir`'s prefix.
fn entry(name: &str, dir: &Path) -> String {
    format!("koto-names/{name}/{}", prefix_of(dir))
}

/// The bucket key of `name`'s state object under `dir`'s prefix.
fn state_key_of(name: &str, dir: &Path) -> String {
    format!("{}/{name}/koto-{name}.state.jsonl", prefix_of(dir))
}

fn state_key(dir: &Path) -> String {
    state_key_of(WF, dir)
}

/// The keys of every PUT in arrival order.
fn puts(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.key)
        .collect()
}

/// How many PUTs named exactly `key`.
fn puts_of(s3: &FakeS3, key: &str) -> usize {
    puts(s3).iter().filter(|k| *k == key).count()
}

/// The PUTs under `koto-names/`.
fn index_puts(s3: &FakeS3) -> Vec<String> {
    puts(s3)
        .into_iter()
        .filter(|k| k.starts_with("koto-names/"))
        .collect()
}

/// Every DELETE that named a key under `koto-names/`.
fn index_deletes(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "DELETE" && r.key.starts_with("koto-names/"))
        .map(|r| r.key)
        .collect()
}

/// How many listings asked for exactly the declaration's key, and how many
/// GETs named it.
fn declaration_reads(s3: &FakeS3) -> (usize, usize) {
    let requests = s3.requests();
    let lists = requests
        .iter()
        .filter(|r| r.list_prefix.as_deref() == Some(DECLARATION))
        .count();
    let gets = requests
        .iter()
        .filter(|r| r.method == "GET" && r.key == DECLARATION)
        .count();
    (lists, gets)
}

/// What a variant plants or faults in the bucket before its push.
type Setup<'a> = dyn Fn(&FakeS3) + 'a;

/// A declaration body `parse_declaration` accepts.
fn declaration() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "declared_at": "2026-11-01T10:00:00Z",
        "machine": "host-a",
        "first_release": "v0.35.0",
        "audit": {
            "started_at": "2026-10-25T09:00:00Z",
            "finished_at": "2026-10-25T09:05:00Z",
            "prefixes": 1,
            "sessions": 1,
            "entries_written": 1,
            "entries_pruned": 0
        }
    }))
    .unwrap()
}

/// A create PUTs the copy's entry once, empty, before its state object,
/// and stamps the copy indexed.
#[test]
fn a_create_writes_the_name_entry_before_the_state_object() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);

    let puts = puts(&s3);
    let key = entry(WF, &a);
    assert_eq!(puts.iter().filter(|k| **k == key).count(), 1, "{puts:#?}");
    let at_entry = puts.iter().position(|k| *k == key).unwrap();
    let at_state = puts.iter().position(|k| *k == state_key(&a)).unwrap();
    assert!(at_entry < at_state, "{puts:#?}");
    assert_eq!(s3.object(&key), Some(Vec::new()), "the entry is empty");
    assert_eq!(h.stamp(WF)["indexed"], true);
}

/// A later push of an indexed copy sends its state and no entry.
#[test]
fn a_second_push_writes_no_name_entry() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);

    s3.clear_requests();
    h.add(&a, "k.md", b"value");
    assert_eq!(puts_of(&s3, &state_key(&a)), 1, "{:#?}", puts(&s3));
    assert!(index_puts(&s3).is_empty(), "{:#?}", puts(&s3));

    s3.clear_requests();
    h.ok(
        &a,
        &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
    );
    assert!(puts_of(&s3, &state_key(&a)) >= 1, "{:#?}", puts(&s3));
    assert!(index_puts(&s3).is_empty(), "{:#?}", puts(&s3));
}

/// A write from another directory indexes the session's home, never the
/// writing directory's prefix.
#[test]
fn a_push_from_another_directory_indexes_the_home_prefix() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    let b = h.dir("b");
    h.init(&a);
    h.make_older(&s3, &a);

    s3.clear_requests();
    h.add(&b, "k2.md", b"from b");
    assert_eq!(index_puts(&s3), vec![entry(WF, &a)]);
    assert!(s3.object(&entry(WF, &b)).is_none());
    assert_eq!(h.stamp(WF)["indexed"], true);
}

/// The move-aside of `koto init --replace-terminal` renames the finished
/// predecessor to `wf_r1`: the new name's entry goes up before its state
/// object, and the old name's entry stays.
#[test]
fn a_rename_writes_the_new_name_s_entry_and_keeps_the_old() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);
    h.ok(
        &a,
        &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
    );

    s3.clear_requests();
    h.ok(
        &a,
        &["init", WF, "--template", "flow.md", "--replace-terminal"],
    );
    let renamed = format!("{WF}_r1");
    let puts = puts(&s3);
    let at_entry = puts
        .iter()
        .position(|k| *k == entry(&renamed, &a))
        .unwrap_or_else(|| panic!("no entry for {renamed}: {puts:#?}"));
    let at_state = puts
        .iter()
        .position(|k| *k == state_key_of(&renamed, &a))
        .unwrap_or_else(|| panic!("no state for {renamed}: {puts:#?}"));
    assert!(at_entry < at_state, "{puts:#?}");
    assert!(s3.object(&entry(WF, &a)).is_some(), "the old entry is gone");
    assert!(s3.object(&entry(&renamed, &a)).is_some());
    assert!(index_deletes(&s3).is_empty(), "{:#?}", s3.requests());
    assert_eq!(h.stamp(&renamed)["indexed"], true);
}

/// `koto session import` PUTs the target's entry under the importing
/// directory's prefix before anything of the copy, after the source's
/// back-reference. A failed import keeps the entry.
#[test]
fn an_import_writes_the_target_s_entry_first() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    let b = second.dir("b");
    first.init(&a);
    first.add(&a, "k.md", b"value");
    let from = a.to_string_lossy().into_owned();
    let import = || {
        second.koto(
            &b,
            &["session", "import", WF, "--from", &from, "--trust-template"],
        )
    };

    // A state PUT that fails rolls the copy back and leaves the entry.
    let copy_state = state_key(&b);
    s3.fail("PUT", &copy_state, 500);
    s3.clear_requests();
    let out = import();
    assert_eq!(out.code, Some(1), "{}", out.describe());
    assert!(
        out.stdout.contains("import_push_failed"),
        "{}",
        out.describe()
    );
    assert!(s3
        .keys_under(&format!("{}/{WF}/", prefix_of(&b)))
        .is_empty());
    assert_eq!(s3.object(&entry(WF, &b)), Some(Vec::new()));
    assert!(index_deletes(&s3).is_empty(), "{:#?}", s3.requests());

    s3.clear_faults();
    s3.clear_requests();
    let out = import();
    assert_eq!(out.code, Some(0), "{}", out.describe());
    let puts = puts(&s3);
    let copy = format!("{}/{WF}/", prefix_of(&b));
    let at_entry = puts
        .iter()
        .position(|k| *k == entry(WF, &b))
        .unwrap_or_else(|| panic!("no entry: {puts:#?}"));
    let copy_puts: Vec<usize> = puts
        .iter()
        .enumerate()
        .filter(|(_, k)| k.starts_with(&copy))
        .map(|(i, _)| i)
        .collect();
    for want in [
        format!("{copy}koto-{WF}.state.jsonl"),
        format!("{copy}template.json"),
        format!("{copy}ctx/k.md"),
        format!("{copy}ctx/manifest.json"),
        format!("{copy}version.json"),
    ] {
        assert!(puts.contains(&want), "{want} not pushed: {puts:#?}");
    }
    assert!(
        copy_puts.iter().all(|i| at_entry < *i),
        "the entry is not first: {puts:#?}"
    );
    let backref = format!("{}/{WF}/ctx/koto/continued-by/", prefix_of(&a));
    let at_backref = puts
        .iter()
        .position(|k| k.starts_with(&backref))
        .unwrap_or_else(|| panic!("no back-reference: {puts:#?}"));
    assert!(at_backref < copy_puts[0], "{puts:#?}");
    assert!(
        s3.object(&entry(WF, &a)).is_some(),
        "the source's entry stays"
    );
}

/// A copy an older release created, stamped without the flag and with no
/// entry, writes its entry once on its next push, then replaces the state
/// object on its stamped ETag as before.
#[test]
fn a_first_push_of_an_older_copy_writes_its_entry() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);
    h.make_older(&s3, &a);
    let stamped = h.stamp(WF)["state_etag"].as_str().unwrap().to_string();

    s3.clear_requests();
    h.add(&a, "k.md", b"value");
    let requests = s3.requests();
    let key = entry(WF, &a);
    assert_eq!(index_puts(&s3), vec![key.clone()]);
    let at_entry = requests
        .iter()
        .position(|r| r.method == "PUT" && r.key == key)
        .unwrap();
    let states: Vec<&fake_s3::Request> = requests
        .iter()
        .filter(|r| r.method == "PUT" && r.key == state_key(&a))
        .collect();
    assert_eq!(states.len(), 1, "{requests:#?}");
    assert!(
        requests
            .iter()
            .position(|r| r.method == "PUT" && r.key == state_key(&a))
            .unwrap()
            > at_entry
    );
    assert_eq!(
        states[0].if_match.as_deref().map(|e| e.trim_matches('"')),
        Some(stamped.as_str()),
        "{:?}",
        states[0]
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.method == "GET" && r.key == state_key(&a)),
        "{requests:#?}"
    );
    assert_eq!(h.stamp(WF)["indexed"], true);
}

/// Run the tick of an older copy on a fresh bucket with `fault` set
/// before it; returns the bucket, host, directory and the tick's output.
fn owed_tick(fault: impl Fn(&FakeS3, &Path)) -> (FakeS3, Host, PathBuf, Out) {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    let out = h.init_with(&a, TICK_TEMPLATE);
    assert_eq!(out.code, Some(0), "init: {}", out.describe());
    h.make_older(&s3, &a);
    fault(&s3, &a);
    s3.clear_requests();
    let out = h.koto(&a, &["next", WF]);
    (s3, h, a, out)
}

/// A transient failure of the entry PUT answers as a failed state PUT
/// does, sends no state, and leaves the push owed; the flush that follows
/// writes the entry and then the state.
#[test]
fn a_failed_entry_put_leaves_the_push_owed() {
    let (_base_s3, _base_h, _base_dir, today) =
        owed_tick(|s3, dir| s3.fail("PUT", &state_key(dir), 503));
    let (s3, h, a, out) = owed_tick(|s3, dir| s3.fail("PUT", &entry(WF, dir), 503));

    assert_eq!(
        out.code,
        today.code,
        "{} against {}",
        out.describe(),
        today.describe()
    );
    assert_eq!(
        out.stdout,
        today.stdout,
        "{} against {}",
        out.describe(),
        today.describe()
    );
    assert_eq!(puts_of(&s3, &state_key(&a)), 0, "{:#?}", puts(&s3));
    assert!(
        !h.stamp(WF)["state_sha256"]
            .as_str()
            .is_some_and(|s| s == sha256_hex(&h.local_state(WF))),
        "the stamp names the unpushed bytes"
    );
    assert_ne!(h.stamp(WF)["indexed"], true);

    s3.clear_faults();
    s3.clear_requests();
    h.ok(&a, &["session", "flush", WF]);
    let puts = puts(&s3);
    let at_entry = puts
        .iter()
        .position(|k| *k == entry(WF, &a))
        .unwrap_or_else(|| panic!("no entry: {puts:#?}"));
    let at_state = puts
        .iter()
        .position(|k| *k == state_key(&a))
        .unwrap_or_else(|| panic!("no state: {puts:#?}"));
    assert!(at_entry < at_state, "{puts:#?}");
    assert_eq!(h.stamp(WF)["indexed"], true);
    assert_eq!(
        h.stamp(WF)["state_sha256"].as_str().unwrap(),
        sha256_hex(&h.local_state(WF))
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// A refused entry PUT on a bucket with no declaration warns, reads the
/// declaration once, and pushes the state as it would without the
/// refusal; the copy stays unindexed until a later push writes the entry.
#[test]
fn a_refused_entry_put_warns_and_pushes_on_an_undeclared_bucket() {
    let base_s3 = FakeS3::start(BUCKET);
    let base = Host::new(&base_s3);
    let base_dir = base.dir("a");
    let today = base.init_with(&base_dir, TEMPLATE);
    assert_eq!(today.code, Some(0), "{}", today.describe());

    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    s3.fail("PUT", &entry(WF, &a), 403);
    let out = h.init_with(&a, TEMPLATE);
    assert_eq!(out.code, today.code, "{}", out.describe());
    assert_eq!(
        out.stdout,
        today.stdout,
        "{} against {}",
        out.describe(),
        today.describe()
    );
    assert!(
        out.stderr.contains("warning:") && out.stderr.contains(&entry(WF, &a)),
        "{}",
        out.describe()
    );
    assert!(out.stderr.contains("owed"), "{}", out.describe());
    assert!(s3.object(&state_key(&a)).is_some(), "the state went up");
    assert!(s3.object(&entry(WF, &a)).is_none());
    assert_eq!(declaration_reads(&s3), (1, 0), "read once, by listing");
    assert_eq!(h.stamp(WF)["indexed"], false);

    s3.clear_faults();
    s3.clear_requests();
    h.add(&a, "k.md", b"value");
    assert_eq!(index_puts(&s3), vec![entry(WF, &a)]);
    assert_eq!(h.stamp(WF)["indexed"], true);
}

/// With a valid declaration planted by hand, or one that can't be read,
/// a refused entry PUT fails the push: no state goes up, the stamp stays
/// as it was, and the error names the entry and the permission it needs.
#[test]
fn a_refused_entry_put_fails_the_push_on_a_planted_declaration() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);
    h.make_older(&s3, &a);
    let key = entry(WF, &a);

    let variants: [(&str, &Setup<'_>); 3] = [
        ("declared", &|s3| s3.put(DECLARATION, &declaration())),
        ("listing refused", &|s3| s3.fail("LIST", DECLARATION, 403)),
        ("listing failed", &|s3| s3.fail("LIST", DECLARATION, 503)),
    ];
    for (i, (what, setup)) in variants.iter().enumerate() {
        s3.clear_faults();
        s3.remove(DECLARATION);
        setup(&s3);
        s3.fail("PUT", &key, 403);
        let stamp = h.stamped(WF);
        s3.clear_requests();
        let out = h.add_out(&a, &format!("k{i}.md"), b"value");
        assert_eq!(puts_of(&s3, &state_key(&a)), 0, "{what}: {:#?}", puts(&s3));
        assert_eq!(h.stamped(WF), stamp, "{what}: the stamp moved");
        assert!(
            out.stderr.contains("koto-names/") && out.stderr.contains(&key),
            "{what}: {}",
            out.describe()
        );
        assert!(
            out.stderr.contains("PUT permission"),
            "{what}: {}",
            out.describe()
        );
        assert!(
            !out.stderr.contains("test-secret-key"),
            "{}",
            out.describe()
        );
    }
}

/// A planted declaration `parse_declaration` rejects reads as undeclared:
/// one over 4 KiB by its listed size, one that isn't JSON, and one that
/// grew past 4 KiB between the listing and the GET, whose read stops at
/// the cap. Each refused entry PUT warns and the push goes on.
#[test]
fn a_refused_entry_put_pushes_on_when_the_planted_declaration_is_rejected() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let a = h.dir("a");
    h.init(&a);
    h.make_older(&s3, &a);
    let key = entry(WF, &a);
    let mut padded = declaration();
    padded.resize(5000, b' ');
    let huge = vec![b' '; 64 * 1024];

    let variants: [(&str, &Setup<'_>); 3] = [
        ("oversized", &|s3| s3.put(DECLARATION, &padded)),
        ("not JSON", &|s3| s3.put(DECLARATION, b"declared")),
        ("grown", &|s3| {
            s3.put(DECLARATION, &declaration());
            s3.put_after("LIST", DECLARATION, DECLARATION, &huge);
        }),
    ];
    for (i, (what, setup)) in variants.iter().enumerate() {
        s3.clear_faults();
        s3.remove(DECLARATION);
        setup(&s3);
        s3.fail("PUT", &key, 403);
        s3.clear_requests();
        let out = h.add(&a, &format!("k{i}.md"), b"value");
        assert_eq!(puts_of(&s3, &state_key(&a)), 1, "{what}: {:#?}", puts(&s3));
        assert!(
            out.stderr.contains("warning:") && out.stderr.contains(&key),
            "{what}: {}",
            out.describe()
        );
        assert_eq!(h.stamp(WF)["indexed"], false, "{what}");
        let gets: Vec<fake_s3::Request> = s3
            .requests()
            .into_iter()
            .filter(|r| r.method == "GET" && r.key == DECLARATION)
            .collect();
        match *what {
            "oversized" => assert!(gets.is_empty(), "{gets:#?}"),
            _ => {
                assert_eq!(gets.len(), 1, "{what}: {gets:#?}");
                assert_eq!(gets[0].range.as_deref(), Some("bytes=0-4096"), "{what}");
                assert!(gets[0].body_sent <= 4097, "{what}: {:?}", gets[0]);
            }
        }
    }
}

/// Deleting a session, by `session cleanup` or by `session prune`, removes
/// its state objects and leaves its entry, with no DELETE under
/// `koto-names/`.
#[test]
fn a_delete_leaves_the_name_entry() {
    for prune in [false, true] {
        let s3 = FakeS3::start(BUCKET);
        let h = Host::new(&s3);
        let a = h.dir("a");
        h.init(&a);
        h.ok(
            &a,
            &["next", WF, "--with-data", r#"{"go":"go"}"#, "--no-cleanup"],
        );
        assert!(s3.object(&entry(WF, &a)).is_some());

        s3.clear_requests();
        if prune {
            std::thread::sleep(std::time::Duration::from_millis(1300));
            let mut cmd = h.cmd(
                &a,
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
            assert_eq!(out.code, Some(0), "prune: {}", out.describe());
            assert!(out.stdout.contains("\"removed\""), "{}", out.describe());
        } else {
            h.ok(&a, &["session", "cleanup", WF]);
        }
        assert!(
            s3.object(&state_key(&a)).is_none(),
            "prune {prune}: the state is still there"
        );
        assert_eq!(s3.object(&entry(WF, &a)), Some(Vec::new()), "prune {prune}");
        assert!(
            index_deletes(&s3).is_empty(),
            "prune {prune}: {:#?}",
            s3.requests()
        );
    }
}

/// The two top-level listings drop `koto-names/` as they drop every common
/// prefix that isn't a workspace's: a lookup of a missing name from another
/// directory makes as many requests, with the same reply, as on the same
/// bucket without the index, and asks no `koto-names/` listing; the
/// dashboard's remote scan shows the same rows and lists nothing under
/// `koto-names/`.
#[test]
fn top_level_listings_ignore_the_name_index_prefix() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    for i in 0..6u32 {
        s3.put(&format!("{i:016x}/other/koto-other.state.jsonl"), b"{}\n");
    }
    let a = first.dir("a");
    first.init(&a);
    assert!(s3.object(&entry(WF, &a)).is_some());
    let probe = second.dir("probe");

    let lookup = || {
        s3.clear_requests();
        let out = second.koto(&probe, &["context", "exists", "missing", "k.md"]);
        let requests = s3.requests();
        (out, requests)
    };
    let dashboard = || {
        s3.clear_requests();
        let out = second.ok(&probe, &["dashboard", "--once", "--all"]);
        let rows: Vec<Vec<String>> = out
            .stdout
            .lines()
            .map(|l| {
                let cols: Vec<&str> = l.split('\t').collect();
                vec![
                    cols.first().copied().unwrap_or_default().to_string(),
                    cols.last().copied().unwrap_or_default().to_string(),
                ]
            })
            .collect();
        (rows, s3.requests())
    };
    let index_listings = |requests: &[fake_s3::Request]| -> Vec<fake_s3::Request> {
        requests
            .iter()
            .filter(|r| {
                r.list_prefix
                    .as_deref()
                    .is_some_and(|p| p.starts_with("koto-names"))
            })
            .cloned()
            .collect()
    };

    let (indexed, indexed_requests) = lookup();
    let (indexed_rows, indexed_scan) = dashboard();
    for key in s3.keys_under("koto-names/") {
        s3.remove(&key);
    }
    let (bare, bare_requests) = lookup();
    let (bare_rows, bare_scan) = dashboard();

    assert_eq!(indexed.code, Some(1), "{}", indexed.describe());
    assert_eq!(indexed.code, bare.code);
    assert_eq!(indexed.stdout, bare.stdout);
    assert_eq!(
        indexed_requests.len(),
        bare_requests.len(),
        "{indexed_requests:#?} against {bare_requests:#?}"
    );
    assert!(index_listings(&indexed_requests).is_empty());
    assert_eq!(indexed_rows, bare_rows);
    assert!(indexed_rows.iter().any(|r| r[0] == WF), "{indexed_rows:?}");
    assert!(
        index_listings(&indexed_scan).is_empty(),
        "{indexed_scan:#?}"
    );
    assert_eq!(indexed_scan.len(), bare_scan.len());
}
