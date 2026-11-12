//! A lookup of a name with no local copy reads the bucket's name index
//! first (koto#608).
//!
//! For a name with no copy on this host and no state under the caller's
//! prefix, koto asks three things at once: the index listing of
//! `koto-names/<name>/`, the declaration `koto-names/_complete`, and the
//! bucket's top-level listing. Each prefix the index names is confirmed by
//! listing its `<prefix>/<name>/`; a confirmed hit answers at once, and a
//! miss sweeps every other prefix the top-level listing names, less those
//! the confirmation already found without the session. Every bucket reads
//! undeclared in this release, so no reply changes: a planted declaration,
//! a failed index listing and a forged or stale entry each leave every
//! reply as it was. A sweep slower than a few seconds prints a hint on
//! stderr naming `koto session index audit`.
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
use std::time::Duration;

const BUCKET: &str = "koto-index-lookup";
const WF: &str = "wf";
/// A name no prefix holds.
const GHOST: &str = "ghost";
/// A name only an older writer's bare PUT holds, with no entry.
const OLD: &str = "old";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const DECLARATION: &str = "koto-names/_complete";
const MEMO: &str = "cloud-index.json";
const HINT: &str = "koto session index audit";

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

#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// What a reply is: the exit code, stdout byte for byte, and the
    /// `code` and `reason` of a JSON error body when it has one.
    fn reply(&self) -> (Option<i32>, String, String, String) {
        let body: Option<serde_json::Value> = serde_json::from_str(self.stdout.trim()).ok();
        let field = |name: &str| {
            body.as_ref()
                .and_then(|b| b.get(name))
                .map(|v| v.to_string())
                .unwrap_or_default()
        };
        (
            self.code,
            self.stdout.clone(),
            field("code"),
            field("reason"),
        )
    }
}

/// One host: its own `HOME` and template cache, and directories to run
/// koto from.
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

    /// `koto init WF` from `dir`, then a context key, so the session has
    /// its entry and its state in the bucket.
    fn init(&self, dir: &Path) {
        std::fs::write(dir.join("flow.md"), TEMPLATE).unwrap();
        self.ok(dir, &["init", WF, "--template", "flow.md"]);
        let mut cmd = self.cmd(dir, &["context", "add", WF, "k.md"]);
        cmd.write_stdin(b"value".to_vec());
        let o = cmd.output().unwrap();
        assert!(o.status.success(), "context add: {o:?}");
    }

    /// `context exists <name> k.md` from `dir`.
    fn exists(&self, dir: &Path, name: &str) -> Out {
        self.koto(dir, &["context", "exists", name, "k.md"])
    }

    /// The host-wide declaration memo.
    fn memo_path(&self) -> PathBuf {
        self.home.join(".koto").join(MEMO)
    }

    /// Create the koto root, as any earlier koto run on this host would
    /// have: the memo is written beside the local store, and a read never
    /// creates the root itself.
    fn make_root(&self) {
        std::fs::create_dir_all(self.home.join(".koto")).unwrap();
    }
}

/// `count` workspace prefixes of the shape koto derives, each holding
/// another session, as other directories' sessions leave the bucket.
fn plant_prefixes(s3: &FakeS3, count: u32) -> Vec<String> {
    (0..count)
        .map(|i| format!("{i:016x}"))
        .inspect(|p| s3.put(&format!("{p}/other/koto-other.state.jsonl"), b"{}\n"))
        .collect()
}

/// `name`'s state object under `prefix`, put there bare with no entry, as
/// a release before the name index left it.
fn plant_state(s3: &FakeS3, prefix: &str, name: &str) {
    s3.put(&format!("{prefix}/{name}/koto-{name}.state.jsonl"), b"{}\n");
}

/// The entry `koto-names/<name>/<suffix>`, empty.
fn plant_entry(s3: &FakeS3, name: &str, suffix: &str) {
    s3.put(&format!("koto-names/{name}/{suffix}"), b"");
}

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

/// The prefix of every listing, in arrival order.
fn listings(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter_map(|r| r.list_prefix)
        .collect()
}

/// How many listings asked for exactly `prefix`.
fn listed(s3: &FakeS3, prefix: &str) -> usize {
    listings(s3).iter().filter(|p| *p == prefix).count()
}

/// How many top-level listings (prefix `""`, delimiter `/`) were made.
fn top_level_listings(s3: &FakeS3) -> usize {
    s3.requests()
        .iter()
        .filter(|r| r.list_prefix.as_deref() == Some("") && r.delimiter.as_deref() == Some("/"))
        .count()
}

/// How many GETs named the declaration.
fn declaration_gets(s3: &FakeS3) -> usize {
    s3.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.key == DECLARATION)
        .count()
}

/// Every request as one line, sorted: the lookup's listings run on
/// several threads, so their order isn't fixed, but their set is.
fn shapes(s3: &FakeS3) -> Vec<String> {
    let mut shapes: Vec<String> = s3
        .requests()
        .iter()
        .map(|r| match &r.list_prefix {
            Some(prefix) => format!("LIST {prefix:?} {:?}", r.delimiter),
            None => format!("{} {}", r.method, r.key),
        })
        .collect();
    shapes.sort();
    shapes
}

/// A listing's line in [`shapes`].
fn list_shape(prefix: &str, delimiter: Option<&str>) -> String {
    format!("LIST {prefix:?} {:?}", delimiter.map(str::to_string))
}

/// The listings of `<prefix>/<name>/` for every prefix in `prefixes`, each
/// asked exactly once.
fn assert_swept(s3: &FakeS3, prefixes: &[String], name: &str) {
    for p in prefixes {
        assert_eq!(
            listed(s3, &format!("{p}/{name}/")),
            1,
            "{p}/{name}/ was not listed exactly once: {:?}",
            listings(s3)
        );
    }
}

/// The `session_unlocated` refusal of `command`, exit 2, naming `name` and
/// `reason`.
fn assert_unlocated(out: &Out, command: &str, name: &str, reason: &str) {
    assert_eq!(out.code, Some(2), "{command}: {}", out.describe());
    let err = out.json();
    assert_eq!(err["code"], "session_unlocated", "{}", out.describe());
    assert_eq!(err["command"], command, "{}", out.describe());
    assert_eq!(err["session"], name, "{}", out.describe());
    assert_eq!(err["reason"], reason, "{}", out.describe());
}

/// The `session_lookup_failed` refusal of `command`, exit 3, naming `name`:
/// the lookup couldn't finish, so koto can't tell whether the name is in
/// use (koto#638). Returns the message.
fn assert_lookup_failed(out: &Out, command: &str, name: &str) -> String {
    assert_eq!(out.code, Some(3), "{command}: {}", out.describe());
    let err = out.json();
    assert_eq!(err["code"], "session_lookup_failed", "{}", out.describe());
    assert_eq!(err["command"], command, "{}", out.describe());
    assert_eq!(err["session"], name, "{}", out.describe());
    let message = err["error"].as_str().unwrap().to_string();
    assert!(message.contains(&format!("'{name}'")), "{message}");
    assert!(message.contains("can't tell"), "{message}");
    message
}

/// `context exists` of a name read as absent: exit 1, nothing printed.
fn assert_absent(out: &Out) {
    assert_eq!(out.code, Some(1), "context exists: {}", out.describe());
    assert!(out.stdout.trim().is_empty(), "{}", out.describe());
}

// -- the named tests --

/// Host A creates `wf`, so its entry exists, among dozens of other
/// prefixes. Host B, from another directory, refuses with `no_local_copy`
/// having listed `<p>/wf/` only for its own prefix and A's: the index
/// answered and no sweep ran.
#[test]
fn an_indexed_session_refuses_from_another_prefix_without_a_sweep() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    assert!(s3
        .object(&format!("koto-names/{WF}/{}", prefix_of(&a)))
        .is_some());
    plant_prefixes(&s3, 40);
    let b = second.dir("elsewhere");

    s3.clear_requests();
    let out = second.exists(&b, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");

    let mut session_listings: Vec<String> = listings(&s3)
        .into_iter()
        .filter(|p| p.ends_with(&format!("/{WF}/")) && !p.starts_with("koto-names/"))
        .collect();
    session_listings.sort();
    let mut expected = vec![
        format!("{}/{WF}/", prefix_of(&b)),
        format!("{}/{WF}/", prefix_of(&a)),
    ];
    expected.sort();
    assert_eq!(session_listings, expected);
}

/// `wf`'s state object, planted bare under one of several prefixes with no
/// entry, is found by the sweep: the index listing was made and named
/// nothing, and the sweep listed the holder.
#[test]
fn an_older_writer_s_session_is_found_by_the_sweep_on_an_undeclared_bucket() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 12);
    plant_state(&s3, &planted[7], WF);
    let dir = h.dir("elsewhere");

    s3.clear_requests();
    let out = h.exists(&dir, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
    assert_eq!(listed(&s3, &format!("koto-names/{WF}/")), 1);
    assert!(s3.keys_under(&format!("koto-names/{WF}/")).is_empty());
    assert_eq!(top_level_listings(&s3), 1);
    assert_eq!(listed(&s3, &format!("{}/{WF}/", planted[7])), 1);
}

/// An entry naming a prefix that holds other sessions but not this name is
/// confirmed absent, and the name reads as absent everywhere: `<p>/<name>/`
/// is listed once, by the confirmation, and the sweep skips it.
#[test]
fn a_stale_entry_is_confirmed_and_read_as_absent() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 8);
    plant_entry(&s3, GHOST, &planted[2]);
    let dir = h.dir("elsewhere");
    let stale = format!("{}/{GHOST}/", planted[2]);

    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    assert_absent(&out);
    assert_eq!(listed(&s3, &stale), 1, "{:?}", listings(&s3));
    assert_swept(&s3, &planted, GHOST);

    s3.clear_requests();
    let out = h.koto(&dir, &["status", GHOST]);
    assert_ne!(out.code, Some(0), "status: {}", out.describe());
    assert!(
        !out.stdout.contains("session_unlocated"),
        "{}",
        out.describe()
    );
    assert_eq!(listed(&s3, &stale), 1, "{:?}", listings(&s3));
}

/// An all-stale index hit still sweeps: with a stale entry for `<p1>` and
/// `wf`'s state planted bare under `<p2>` with no entry, a lookup from a
/// third directory refuses with `no_local_copy`. `<p1>/wf/` is listed once,
/// by the confirmation, and `<p2>/wf/` by the sweep.
#[test]
fn a_stale_entry_still_sweeps_and_finds_an_older_writer_s_session() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 8);
    plant_entry(&s3, WF, &planted[1]);
    plant_state(&s3, &planted[5], WF);
    let dir = h.dir("third");

    s3.clear_requests();
    let out = h.exists(&dir, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
    assert_eq!(top_level_listings(&s3), 1);
    assert_eq!(listed(&s3, &format!("{}/{WF}/", planted[1])), 1);
    assert_eq!(listed(&s3, &format!("{}/{WF}/", planted[5])), 1);
}

/// Entries with forged suffixes never build a key: no request's key or
/// list prefix carries their text, the caller's own `<own>/<name>/` is
/// listed only by the probe, and every reply is the one given with no
/// entries planted.
#[test]
fn a_forged_entry_never_builds_a_key() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 6);
    plant_state(&s3, &planted[4], WF);
    let dir = h.dir("elsewhere");
    let own = prefix_of(&dir);

    let before: Vec<Out> = [GHOST, WF].iter().map(|n| h.exists(&dir, n)).collect();
    assert_absent(&before[0]);
    assert_unlocated(&before[1], "context exists", WF, "no_local_copy");

    let forged = [
        "ABCDEF0123456789".to_string(),
        "bbbbbbbbbbbbbbb".to_string(),
        "ccccccccccccccccc".to_string(),
        "..".to_string(),
        "dddddddddddddddd/more".to_string(),
        own.clone(),
        "eeeeeeeeeeeeeee%65".to_string(),
        "%2e%2e".to_string(),
    ];
    for name in [GHOST, WF] {
        for suffix in &forged {
            plant_entry(&s3, name, suffix);
        }
    }
    let banned = [
        "ABCDEF0123456789",
        "abcdef0123456789",
        "bbbbbbbbbbbbbbb",
        "ccccccccccccccc",
        "dddddddddddddddd",
        "eeeeeeeeeeeeeee",
        "..",
        "%",
    ];

    for (name, earlier) in [GHOST, WF].iter().zip(&before) {
        s3.clear_requests();
        let out = h.exists(&dir, name);
        assert_eq!(out.reply(), earlier.reply(), "{name}: {}", out.describe());
        for r in s3.requests() {
            let text = format!("{} {}", r.key, r.list_prefix.as_deref().unwrap_or(""));
            for bad in banned {
                assert!(!text.contains(bad), "a request built from {bad:?}: {r:?}");
            }
        }
        assert_eq!(
            listed(&s3, &format!("{own}/{name}/")),
            1,
            "{:?}",
            listings(&s3)
        );
    }
}

/// One fault scenario of
/// [`a_failed_index_listing_changes_nothing_on_an_undeclared_bucket`]:
/// what it plants and fails before the lookups.
type Fault = fn(&FakeS3);

/// A failed index listing, one answered with something that isn't a
/// listing, and a failed or invalid declaration read each leave every
/// reply as it is with no index, and the sweep covering every other prefix
/// on a miss. No reply says "can't tell".
#[test]
fn a_failed_index_listing_changes_nothing_on_an_undeclared_bucket() {
    let faults: [(&str, Fault); 6] = [
        ("index listing 503", |s3| {
            for name in [GHOST, OLD, WF] {
                s3.fail("LIST", &format!("koto-names/{name}/"), 503);
            }
        }),
        ("index listing garbled", |s3| {
            for name in [GHOST, OLD, WF] {
                s3.garble_listing(&format!("koto-names/{name}/"));
            }
        }),
        ("declaration listing 503", |s3| {
            s3.fail("LIST", DECLARATION, 503)
        }),
        ("declaration GET 500", |s3| {
            s3.put(DECLARATION, &declaration());
            s3.fail("GET", DECLARATION, 500);
        }),
        ("declaration listing and GET fail", |s3| {
            s3.put(DECLARATION, &declaration());
            s3.fail("LIST", DECLARATION, 503);
            s3.fail("GET", DECLARATION, 500);
        }),
        ("declaration invalid", |s3| {
            s3.put(DECLARATION, b"not a declaration")
        }),
    ];

    std::thread::scope(|scope| {
        for (label, fault) in faults {
            scope.spawn(move || {
                let s3 = FakeS3::start(BUCKET);
                let first = Host::new(&s3);
                let second = Host::new(&s3);
                let a = first.dir("a");
                first.init(&a);
                let mut others = plant_prefixes(&s3, 10);
                plant_state(&s3, &others[6], OLD);
                others.push(prefix_of(&a));
                let dir = second.dir("elsewhere");
                fault(&s3);
                s3.clear_requests();

                let outs: Vec<(&str, Out)> = std::thread::scope(|inner| {
                    let runs: Vec<_> = [GHOST, OLD, WF]
                        .into_iter()
                        .map(|name| {
                            let (second, dir) = (&second, &dir);
                            inner.spawn(move || (name, second.exists(dir, name)))
                        })
                        .collect();
                    runs.into_iter().map(|r| r.join().unwrap()).collect()
                });
                for (name, out) in &outs {
                    assert!(
                        !out.stdout.contains("can't tell"),
                        "{label}, {name}: {}",
                        out.describe()
                    );
                    match *name {
                        GHOST => assert_absent(out),
                        _ => assert_unlocated(out, "context exists", name, "no_local_copy"),
                    }
                }
                assert_swept(&s3, &others, GHOST);
            });
        }
    });
}

/// A failed top-level listing is "can't tell" even when the index names
/// the holder and its state is there; once the listing answers, the lookup
/// refuses with `no_local_copy`.
#[test]
fn a_failed_top_level_listing_is_cant_tell_even_with_an_index_hit() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    plant_prefixes(&s3, 6);
    let b = second.dir("elsewhere");

    s3.fail("LIST", "", 503);
    let out = second.exists(&b, WF);
    let message = assert_lookup_failed(&out, "context exists", WF);
    assert!(message.contains("503"), "{message}");
    assert!(
        !out.stdout.contains("session_unlocated"),
        "{}",
        out.describe()
    );

    s3.clear_faults();
    let out = second.exists(&b, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
}

/// The index listing of `foo` is `koto-names/foo/`, never `foobar`'s: an
/// entry `koto-names/foobar/<p>`, for a prefix holding nothing else, builds
/// no listing of `<p>/foo/`.
#[test]
fn an_index_listing_for_a_name_never_returns_a_longer_name_s_entries() {
    const P: &str = "0123456789abcdef";
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    plant_prefixes(&s3, 3);
    plant_entry(&s3, "foobar", P);
    let dir = h.dir("elsewhere");

    s3.clear_requests();
    let out = h.exists(&dir, "foo");
    assert_absent(&out);
    let index: Vec<String> = listings(&s3)
        .into_iter()
        .filter(|p| p.starts_with("koto-names/foo"))
        .collect();
    assert_eq!(index, vec!["koto-names/foo/".to_string()]);
    assert_eq!(listed(&s3, &format!("{P}/foo/")), 0, "{:?}", listings(&s3));
    assert!(
        listings(&s3).iter().all(|p| !p.starts_with(P)),
        "{:?}",
        listings(&s3)
    );
}

// -- the cap --

/// A name with more than a page of entries is treated as polluted: the
/// lookup reads one index page, confirms none of its entries, sweeps every
/// other prefix, and replies as it would with no index.
#[test]
fn an_index_past_the_cap_reads_one_page_and_sweeps() {
    const ENTRIES: u32 = 1_100;
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 6);
    plant_state(&s3, &planted[3], WF);
    let flooded: Vec<String> = (0..ENTRIES)
        .map(|i| format!("{:016x}", 0x1000_0000_u64 + u64::from(i)))
        .collect();
    for name in [GHOST, WF] {
        for p in &flooded {
            plant_entry(&s3, name, p);
        }
    }
    let dir = h.dir("elsewhere");
    let from_flood = |s3: &FakeS3| {
        listings(s3)
            .into_iter()
            .filter(|l| flooded.iter().any(|p| l.starts_with(p.as_str())))
            .collect::<Vec<_>>()
    };

    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    assert_absent(&out);
    assert_eq!(listed(&s3, &format!("koto-names/{GHOST}/")), 1);
    assert_eq!(from_flood(&s3), Vec::<String>::new());
    assert_swept(&s3, &planted, GHOST);

    s3.clear_requests();
    let out = h.exists(&dir, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
    assert_eq!(listed(&s3, &format!("koto-names/{WF}/")), 1);
    assert_eq!(from_flood(&s3), Vec::<String>::new());
    assert_eq!(listed(&s3, &format!("{}/{WF}/", planted[3])), 1);

    // A page throttled for one whole call (rust-s3's request and its own
    // retry) is asked again: still one page per attempt.
    s3.fail_times("LIST", &format!("koto-names/{GHOST}/"), 503, 2);
    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    assert_absent(&out);
    assert_eq!(listed(&s3, &format!("koto-names/{GHOST}/")), 3);
    assert_eq!(from_flood(&s3), Vec::<String>::new());
}

// -- the declaration and its memo --

/// A valid declaration planted by hand turns the declared branch on and
/// changes one reply only. `status` and `context exists` of a missing name
/// and of an indexed name answer as they did without it, while an older
/// writer's session, held under another prefix with no entry, now answers
/// as a missing name does: the reply change the design accepts on a
/// declared bucket. The memo records the bucket as declared, and the next
/// lookup of the missing name trusts it: no top-level listing, no sweep
/// and no listing of the declaration, only one GET of it (the window is
/// 0).
#[test]
fn a_planted_declaration_changes_only_an_unindexed_session_s_reply() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    let mut others = plant_prefixes(&s3, 6);
    plant_state(&s3, &others[2], OLD);
    others.push(prefix_of(&a));
    let b = second.dir("elsewhere");
    second.make_root();

    let commands: Vec<Vec<&str>> = [GHOST, OLD, WF]
        .iter()
        .flat_map(|name| {
            [
                vec!["status", *name],
                vec!["context", "exists", *name, "k.md"],
            ]
        })
        .collect();
    let undeclared: Vec<_> = commands
        .iter()
        .map(|args| second.koto(&b, args).reply())
        .collect();
    let memo: serde_json::Value =
        serde_json::from_slice(&std::fs::read(second.memo_path()).unwrap()).unwrap();
    assert_eq!(memo["declared"], false, "{memo}");

    // What the missing name answered to `args`, asked of `name` instead.
    let as_missing = |args: &[&str], name: &str| {
        let ghost: Vec<&str> = args
            .iter()
            .map(|a| if *a == name { GHOST } else { *a })
            .collect();
        let at = commands.iter().position(|c| *c == ghost).unwrap();
        let (code, stdout, field_code, reason) = undeclared[at].clone();
        let stdout = stdout.replace(&format!("'{GHOST}'"), &format!("'{name}'"));
        (code, stdout, field_code, reason)
    };

    s3.put(DECLARATION, &declaration());
    for (args, before) in commands.iter().zip(&undeclared) {
        let after = second.koto(&b, args).reply();
        if args.contains(&OLD) {
            assert_ne!(&after, before, "koto {args:?} kept its undeclared reply");
            assert_eq!(after, as_missing(args, OLD), "koto {args:?}");
        } else {
            assert_eq!(&after, before, "koto {args:?}");
        }
    }
    assert_absent(&second.exists(&b, OLD));
    let memo: serde_json::Value =
        serde_json::from_slice(&std::fs::read(second.memo_path()).unwrap()).unwrap();
    assert_eq!(memo["declared"], true, "{memo}");
    assert_eq!(memo["bucket"], BUCKET, "{memo}");

    s3.clear_requests();
    assert_absent(&second.exists(&b, GHOST));
    assert_eq!(top_level_listings(&s3), 0, "{:?}", shapes(&s3));
    assert_eq!(listed(&s3, DECLARATION), 0, "{:?}", shapes(&s3));
    assert_eq!(declaration_gets(&s3), 1, "{:?}", shapes(&s3));
    assert_eq!(listed(&s3, &format!("koto-names/{GHOST}/")), 1);
    for p in &others {
        let sweep = format!("{p}/{GHOST}/");
        assert_eq!(listed(&s3, &sweep), 0, "{sweep}: {:?}", listings(&s3));
    }
}

/// A memo that doesn't parse, and one recorded for another bucket, are
/// ignored: the lookup makes the same requests and gives the same reply
/// as with no memo, and leaves a memo for this bucket behind.
#[test]
fn a_garbage_or_foreign_memo_changes_neither_requests_nor_reply() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 6);
    plant_state(&s3, &planted[1], OLD);
    let dir = h.dir("elsewhere");
    h.make_root();

    let run = |memo: Option<&[u8]>| {
        let path = h.memo_path();
        match memo {
            Some(bytes) => std::fs::write(&path, bytes).unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        s3.clear_requests();
        let ghost = h.exists(&dir, GHOST).reply();
        let requests = shapes(&s3);
        let old = h.exists(&dir, OLD).reply();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["bucket"], BUCKET, "{written}");
        assert_eq!(written["declared"], false, "{written}");
        (ghost, requests, old)
    };

    let none = run(None);
    let foreign = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "endpoint": s3.endpoint(),
        "bucket": "another-bucket",
        "declared": true,
        "read_at": "2026-11-01T10:00:00Z",
    }))
    .unwrap();
    for memo in [&b"{not json"[..], &b""[..], &foreign[..]] {
        assert_eq!(
            run(Some(memo)),
            none,
            "memo {}",
            String::from_utf8_lossy(memo)
        );
    }
}

// -- request counts and timing --

/// A missing name with no local copy costs exactly the lookup before the
/// index plus two requests: the index listing and the declaration's.
#[test]
fn a_missing_name_costs_two_more_requests_than_before_the_index() {
    const N: u32 = 10;
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, N);
    let dir = h.dir("elsewhere");
    let own = prefix_of(&dir);

    s3.clear_requests();
    assert_absent(&h.exists(&dir, GHOST));
    let all = shapes(&s3);
    let index_requests: Vec<String> = all
        .iter()
        .filter(|s| s.contains("koto-names/"))
        .cloned()
        .collect();
    assert_eq!(
        index_requests,
        vec![
            list_shape(DECLARATION, None),
            list_shape(&format!("koto-names/{GHOST}/"), None),
        ]
    );

    // What the command asked before the index: the own-prefix probe, the
    // top-level listing, one listing per other prefix, and the HEAD of the
    // own state key a not-found makes (rust-s3 asks a 404 HEAD twice).
    let mut before: Vec<String> = planted
        .iter()
        .map(|p| list_shape(&format!("{p}/{GHOST}/"), Some("/")))
        .collect();
    before.push(list_shape(&format!("{own}/{GHOST}/"), None));
    before.push(list_shape("", Some("/")));
    let own_state = format!("HEAD {own}/{GHOST}/koto-{GHOST}.state.jsonl");
    before.push(own_state.clone());
    before.push(own_state);
    before.sort();
    let rest: Vec<String> = all
        .iter()
        .filter(|s| !s.contains("koto-names/"))
        .cloned()
        .collect();
    assert_eq!(rest, before);
    assert_eq!(all.len(), before.len() + 2);
}

/// A hit through the index costs the own-prefix probe, the three step-1
/// requests and one listing of the holder's `<p>/<name>/`.
#[test]
fn an_index_hit_costs_the_probe_three_requests_and_one_listing() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    plant_prefixes(&s3, 20);
    let b = second.dir("elsewhere");

    s3.clear_requests();
    assert_unlocated(
        &second.exists(&b, WF),
        "context exists",
        WF,
        "no_local_copy",
    );
    let mut expected = vec![
        list_shape(&format!("{}/{WF}/", prefix_of(&b)), None),
        list_shape(&format!("koto-names/{WF}/"), None),
        list_shape(DECLARATION, None),
        list_shape("", Some("/")),
        list_shape(&format!("{}/{WF}/", prefix_of(&a)), Some("/")),
    ];
    expected.sort();
    assert_eq!(shapes(&s3), expected);
}

/// The index listing, the declaration's listing and the top-level listing
/// go out together: against a slow bucket all three arrive within one
/// latency of each other, and before any other prefix's `<p>/<name>/`.
#[test]
fn the_first_step_s_three_requests_share_one_round_trip() {
    let latency = Duration::from_millis(300);
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, 10);
    let dir = h.dir("elsewhere");

    s3.set_latency(latency);
    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    assert_absent(&out);

    let requests = s3.requests();
    let at = |prefix: &str, delimiter: Option<&str>| {
        let found: Vec<_> = requests
            .iter()
            .filter(|r| {
                r.list_prefix.as_deref() == Some(prefix) && r.delimiter.as_deref() == delimiter
            })
            .map(|r| r.at)
            .collect();
        assert_eq!(found.len(), 1, "one listing of {prefix:?}: {requests:?}");
        found[0]
    };
    let step_one = [
        at(&format!("koto-names/{GHOST}/"), None),
        at(DECLARATION, None),
        at("", Some("/")),
    ];
    let first = *step_one.iter().min().unwrap();
    let last = *step_one.iter().max().unwrap();
    assert!(
        last - first < latency,
        "the first step's requests went out over {:?}",
        last - first
    );
    let sweep_starts = requests
        .iter()
        .filter(|r| {
            r.list_prefix
                .as_deref()
                .is_some_and(|l| planted.iter().any(|p| l == format!("{p}/{GHOST}/")))
        })
        .map(|r| r.at)
        .min()
        .unwrap();
    assert!(
        last < sweep_starts,
        "a sweep listing went out before step 1"
    );
}

// -- the stderr hint --

/// How many prefixes and how slow a bucket the hint tests use: two rounds
/// of 64 listings at just over a second each make the sweep take longer
/// than the hint's two-second threshold.
const SLOW_PREFIXES: u32 = 70;
const SLOW_LATENCY: Duration = Duration::from_millis(1_050);

/// A sweep slower than the threshold prints the hint naming
/// `koto session index audit` on stderr, with no bucket text, and leaves
/// stdout and the exit code as a fast sweep gives them; a fast sweep
/// prints no hint.
#[test]
fn a_slow_sweep_prints_the_index_audit_hint() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let planted = plant_prefixes(&s3, SLOW_PREFIXES);
    let dir = h.dir("elsewhere");

    let fast = h.exists(&dir, GHOST);
    assert_absent(&fast);
    assert!(!fast.stderr.contains(HINT), "{}", fast.describe());

    s3.set_latency(SLOW_LATENCY);
    let slow = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    assert_eq!(
        (slow.code, &slow.stdout),
        (fast.code, &fast.stdout),
        "{}",
        slow.describe()
    );
    let hint: Vec<&str> = slow.stderr.lines().filter(|l| l.contains(HINT)).collect();
    assert_eq!(hint.len(), 1, "{}", slow.describe());
    assert!(!hint[0].contains(BUCKET), "{}", hint[0]);
    assert!(!hint[0].contains(GHOST), "{}", hint[0]);
    assert!(
        planted.iter().all(|p| !hint[0].contains(p.as_str())),
        "{}",
        hint[0]
    );
}

/// A lookup the index answers prints no hint, however slow the bucket:
/// it never sweeps.
#[test]
fn an_index_hit_prints_no_hint_however_slow() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let second = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    plant_prefixes(&s3, SLOW_PREFIXES);
    let b = second.dir("elsewhere");

    s3.set_latency(SLOW_LATENCY);
    let out = second.exists(&b, WF);
    s3.set_latency(Duration::ZERO);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
    assert!(!out.stderr.contains(HINT), "{}", out.describe());
}

// -- the declared branch (koto#609) --

/// The check window the declared tests set when they need one above 0.
const WINDOW_SECS: &str = "1";

/// Run `cmd` and capture what it printed.
fn run(mut cmd: Command) -> Out {
    let o = cmd.output().unwrap();
    Out {
        code: o.status.code(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// `context exists <name> k.md` from `dir` with the check window above 0.
fn exists_windowed(h: &Host, dir: &Path, name: &str) -> Out {
    let mut cmd = h.cmd(dir, &["context", "exists", name, "k.md"]);
    cmd.env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", WINDOW_SECS);
    run(cmd)
}

/// The host's memo, parsed.
fn memo(h: &Host) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(h.memo_path()).unwrap()).unwrap()
}

/// Write the memo for `endpoint` and this bucket by hand: `declared`, its
/// last answered read `age` ago.
fn write_memo(h: &Host, endpoint: &str, declared: bool, age: Duration) {
    let read_at = fake_s3::rfc3339_millis(std::time::SystemTime::now() - age);
    let body = serde_json::json!({
        "version": 1,
        "endpoint": endpoint,
        "bucket": BUCKET,
        "declared": declared,
        "read_at": read_at,
    });
    std::fs::write(h.memo_path(), serde_json::to_vec(&body).unwrap()).unwrap();
}

/// Move the memo's last answered read to `age` ago, keeping the rest.
fn age_memo(h: &Host, age: Duration) {
    let mut body = memo(h);
    body["read_at"] = fake_s3::rfc3339_millis(std::time::SystemTime::now() - age).into();
    std::fs::write(h.memo_path(), serde_json::to_vec(&body).unwrap()).unwrap();
}

/// A bucket carrying a valid declaration, and a host whose memo says
/// "declared" from an earlier lookup of a missing name from `dir_name`.
fn declared_host(s3: &FakeS3, dir_name: &str) -> (Host, PathBuf) {
    s3.put(declaration::KEY, &declaration::valid());
    let h = Host::new(s3);
    h.make_root();
    let dir = h.dir(dir_name);
    assert_absent(&h.exists(&dir, "earlier"));
    assert_eq!(memo(&h)["declared"], true, "{}", memo(&h));
    (h, dir)
}

/// Every request but the HEADs of `name`'s state key under `own`, which
/// `context exists` makes after the lookup for a name it reads as absent.
fn lookup_requests(s3: &FakeS3, own: &str, name: &str) -> Vec<fake_s3::Request> {
    let own_state = format!("{own}/{name}/koto-{name}.state.jsonl");
    s3.requests()
        .into_iter()
        .filter(|r| !(r.method == "HEAD" && r.key == own_state))
        .collect()
}

/// [`shapes`] of [`lookup_requests`].
fn lookup_shapes(s3: &FakeS3, own: &str, name: &str) -> Vec<String> {
    let mut shapes: Vec<String> = lookup_requests(s3, own, name)
        .iter()
        .map(|r| match &r.list_prefix {
            Some(prefix) => format!("LIST {prefix:?} {:?}", r.delimiter),
            None => format!("{} {}", r.method, r.key),
        })
        .collect();
    shapes.sort();
    shapes
}

/// The listings of `<p>/<name>/` made for a `p` other than `own`.
fn other_prefix_listings(s3: &FakeS3, own: &str, name: &str) -> Vec<String> {
    let suffix = format!("/{name}/");
    let own_listing = format!("{own}{suffix}");
    listings(s3)
        .into_iter()
        .filter(|l| l.ends_with(&suffix) && !l.starts_with("koto-names/") && *l != own_listing)
        .collect()
}

/// A missing name on a declared bucket costs the own-prefix probe, the
/// index listing and the declaration GET, whatever the prefix count, and
/// the last two share one round trip after the first.
#[test]
fn a_missing_name_on_a_declared_bucket_costs_three_requests_at_any_prefix_count() {
    let s3 = FakeS3::start(BUCKET);
    let (h, dir) = declared_host(&s3, "elsewhere");
    let own = prefix_of(&dir);
    let mut expected = vec![
        list_shape(&format!("{own}/{GHOST}/"), None),
        list_shape(&format!("koto-names/{GHOST}/"), None),
        format!("GET {}", declaration::KEY),
    ];
    expected.sort();

    for count in [1, 64, 300, 2_000] {
        plant_prefixes(&s3, count);
        s3.clear_requests();
        assert_absent(&h.exists(&dir, GHOST));
        assert_eq!(
            lookup_shapes(&s3, &own, GHOST),
            expected,
            "{count} prefixes"
        );
    }

    let latency = Duration::from_millis(300);
    s3.set_latency(latency);
    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    assert_absent(&out);
    let requests = lookup_requests(&s3, &own, GHOST);
    assert_eq!(requests.len(), 3, "{requests:?}");
    let at = |pred: &dyn Fn(&fake_s3::Request) -> bool| {
        requests.iter().find(|r| pred(r)).map(|r| r.at).unwrap()
    };
    let probe_listing = format!("{own}/{GHOST}/");
    let index_listing = format!("koto-names/{GHOST}/");
    let probe = at(&|r| r.list_prefix.as_deref() == Some(probe_listing.as_str()));
    let index = at(&|r| r.list_prefix.as_deref() == Some(index_listing.as_str()));
    let get = at(&|r| r.method == "GET" && r.key == declaration::KEY);
    let (early, late) = if index < get {
        (index, get)
    } else {
        (get, index)
    };
    assert!(
        late - early < latency / 2,
        "index and GET {:?} apart",
        late - early
    );
    assert!(probe < early, "the index or GET went out before the probe");
    assert!(
        late - probe < latency * 2,
        "the lookup took {:?}",
        late - probe
    );

    // A "declared" younger than the window skips the GET as well.
    write_memo(&h, &s3.endpoint(), true, Duration::ZERO);
    s3.clear_requests();
    let mut cmd = h.cmd(&dir, &["context", "exists", GHOST, "k.md"]);
    cmd.env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "60");
    assert_absent(&run(cmd));
    let listings_only: Vec<String> = expected
        .iter()
        .filter(|s| s.starts_with("LIST"))
        .cloned()
        .collect();
    assert_eq!(lookup_shapes(&s3, &own, GHOST), listings_only, "fresh memo");
}

/// Throttles that clear within the retries cost the same at 64 and 300
/// prefixes, ask each key a bounded number of times, and never sweep. A
/// declaration GET that keeps failing, with the memo's last answered read
/// under 10 minutes old and the window above 0, stays bounded too.
#[test]
fn a_missing_name_on_a_degraded_declared_bucket_asks_a_bounded_number_of_listings() {
    let mut totals = Vec::new();
    for count in [64, 300] {
        let s3 = FakeS3::start(BUCKET);
        let (h, dir) = declared_host(&s3, "elsewhere");
        let own = prefix_of(&dir);
        plant_prefixes(&s3, count);
        let probe = format!("{own}/{GHOST}/");
        let index = format!("koto-names/{GHOST}/");
        s3.fail_times("LIST", &probe, 503, 2);
        s3.fail_times("LIST", &index, 503, 2);
        s3.fail_times("GET", declaration::KEY, 503, 2);
        s3.clear_requests();
        assert_absent(&h.exists(&dir, GHOST));
        let requests = lookup_requests(&s3, &own, GHOST);
        totals.push(requests.len());
        // Each of LIST_ATTEMPTS asks is at most rust-s3's request and its
        // one retry.
        let max = 3 * 2;
        assert!(listed(&s3, &probe) <= max, "{:?}", listings(&s3));
        assert!(listed(&s3, &index) <= max, "{:?}", listings(&s3));
        assert!(declaration_gets(&s3) <= max, "{requests:?}");
        assert_eq!(top_level_listings(&s3), 0, "{count}: {:?}", listings(&s3));
        assert!(other_prefix_listings(&s3, &own, GHOST).is_empty());

        // The GET fails on every ask; the carried "declared" still holds.
        s3.clear_faults();
        s3.fail("GET", declaration::KEY, 503);
        age_memo(&h, Duration::from_secs(120));
        s3.clear_requests();
        assert_absent(&exists_windowed(&h, &dir, GHOST));
        assert!(declaration_gets(&s3) > 0, "the GET was skipped");
        assert_eq!(top_level_listings(&s3), 0, "{count}: {:?}", listings(&s3));
        assert!(other_prefix_listings(&s3, &own, GHOST).is_empty());

        // With the window at 0 nothing is carried: the same failing GET
        // reads undeclared and the lookup sweeps every other prefix.
        s3.clear_requests();
        assert_absent(&h.exists(&dir, GHOST));
        assert_eq!(top_level_listings(&s3), 1, "{count}: {:?}", listings(&s3));
        assert_eq!(
            other_prefix_listings(&s3, &own, GHOST).len(),
            count as usize
        );
    }
    assert_eq!(totals[0], totals[1], "requests at 64 and 300 prefixes");
}

/// An indexed session under one prefix still refuses from another on a
/// declared bucket, listing only its holder besides the own prefix, with
/// no top-level listing once the memo says "declared".
#[test]
fn a_declared_bucket_still_refuses_an_indexed_session_from_another_prefix() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    plant_prefixes(&s3, 300);
    let (second, b) = declared_host(&s3, "elsewhere");
    let own = prefix_of(&b);

    s3.clear_requests();
    let out = second.exists(&b, WF);
    assert_unlocated(&out, "context exists", WF, "no_local_copy");
    assert_eq!(out.json()["locations"][0], own, "{}", out.describe());
    assert!(!out.stdout.contains("value"), "{}", out.describe());
    assert_eq!(
        other_prefix_listings(&s3, &own, WF),
        vec![format!("{}/{WF}/", prefix_of(&a))]
    );
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
}

/// `koto status` of a session another workspace's prefix holds, with its
/// index entry, on a declared bucket is never answered absent: it refuses
/// with `session_unlocated` and `no_local_copy`, from the index alone.
#[test]
fn status_on_a_declared_bucket_refuses_an_indexed_session_as_unlocated() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    assert!(s3
        .object(&format!("koto-names/{WF}/{}", prefix_of(&a)))
        .is_some());
    plant_prefixes(&s3, 30);
    let (second, b) = declared_host(&s3, "elsewhere");

    s3.clear_requests();
    let out = second.koto(&b, &["status", WF]);
    assert_unlocated(&out, "status", WF, "no_local_copy");
    assert!(!out.stdout.contains("not found"), "{}", out.describe());
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
}

/// On a declared bucket a failed index listing, or a failed confirming
/// listing, is "can't tell" (`session_lookup_failed`, exit 3), never
/// not-found.
#[test]
fn a_failed_index_listing_on_a_declared_bucket_is_cant_tell() {
    let s3 = FakeS3::start(BUCKET);
    let first = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    plant_prefixes(&s3, 10);
    let (second, b) = declared_host(&s3, "elsewhere");
    let holder = format!("{}/{WF}/", prefix_of(&a));

    let assert_cant_tell = |name: &str, listing: &str| {
        let out = second.exists(&b, name);
        assert_lookup_failed(&out, "context exists", name);
        let text = format!("{}{}", out.stdout, out.stderr);
        let needles = ["503".to_string(), listing.into()];
        for needle in needles {
            assert!(
                text.contains(&needle),
                "{name}: no {needle:?}: {}",
                out.describe()
            );
        }
    };

    for name in [GHOST, WF] {
        s3.fail("LIST", &format!("koto-names/{name}/"), 503);
    }
    for name in [GHOST, WF] {
        assert_cant_tell(name, &format!("koto-names/{name}/"));
    }
    s3.clear_faults();
    s3.fail("LIST", &holder, 503);
    assert_cant_tell(WF, &holder);

    s3.clear_faults();
    assert_absent(&second.exists(&b, GHOST));
    assert_unlocated(
        &second.exists(&b, WF),
        "context exists",
        WF,
        "no_local_copy",
    );
}

/// A host whose memo says "declared", pointed at a closed port, can't tell
/// whether a never-used name is in use (koto#638): after the own-prefix
/// probe alone, `context exists` and `status` refuse with
/// `session_lookup_failed`, exit 3, saying the bucket couldn't be reached.
#[test]
fn an_unreachable_declared_bucket_refuses_a_never_used_name_as_lookup_failed() {
    const NEVER: &str = "never_used";
    let s3 = FakeS3::start(BUCKET);
    let (mut h, dir) = declared_host(&s3, "offline");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    h.endpoint = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let endpoint = h.endpoint.clone();
    write_memo(&h, &endpoint, true, Duration::from_secs(120));

    s3.clear_requests();
    let message = assert_lookup_failed(&h.exists(&dir, NEVER), "context exists", NEVER);
    assert!(message.contains("couldn't be reached"), "{message}");
    let message = assert_lookup_failed(&h.koto(&dir, &["status", NEVER]), "status", NEVER);
    assert!(message.contains("couldn't be reached"), "{message}");
    assert!(s3.requests().is_empty(), "{:?}", s3.requests());
}

/// Every rejected declaration body reads as undeclared: an entry-less
/// session under another prefix refuses with `no_local_copy`, and a
/// missing name sweeps every other prefix. With a memo that said
/// "declared", the same body, or no object at all, clears it in that
/// lookup, which then lists the top level and sweeps.
#[test]
fn an_oversized_or_malformed_declaration_reads_as_undeclared() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    h.make_root();
    let others = plant_prefixes(&s3, 6);
    plant_state(&s3, &others[3], OLD);
    let dir = h.dir("elsewhere");
    let endpoint = s3.endpoint();

    let mut bodies: Vec<(&str, Option<Vec<u8>>)> = declaration::rejected()
        .into_iter()
        .map(|(label, body)| (label, Some(body)))
        .collect();
    bodies.push(("removed", None));
    for (label, body) in &bodies {
        match body {
            Some(body) => s3.put(declaration::KEY, body),
            None => s3.remove(declaration::KEY),
        }
        for remembered in [false, true] {
            if remembered {
                write_memo(&h, &endpoint, true, Duration::from_secs(1));
            }
            s3.clear_requests();
            let out = h.exists(&dir, OLD);
            assert_unlocated(&out, "context exists", OLD, "no_local_copy");
            assert_eq!(memo(&h)["declared"], false, "{label}, memo {remembered}");
            if remembered {
                assert_eq!(top_level_listings(&s3), 1, "{label}: {:?}", listings(&s3));
                write_memo(&h, &endpoint, true, Duration::from_secs(1));
            }
            s3.clear_requests();
            assert_absent(&h.exists(&dir, GHOST));
            assert_swept(&s3, &others, GHOST);
        }
    }
}

/// A name the index lists more entries for than the cap confirms none of
/// them on a declared bucket and sweeps instead, so the sweep finds an
/// entry-less session; the index listing stops at the first page. Under
/// the cap, the same bucket reads that session as not-found, with no
/// top-level listing.
#[test]
fn a_name_with_more_entries_than_the_cap_falls_back_to_the_sweep() {
    let s3 = FakeS3::start(BUCKET);
    let mut others = plant_prefixes(&s3, 300);
    let holder = "f".repeat(16);
    plant_state(&s3, &holder, OLD);
    others.push(holder);
    let (h, dir) = declared_host(&s3, "elsewhere");
    let own = prefix_of(&dir);
    let index = format!("koto-names/{OLD}/");

    for p in others.iter().take(200) {
        plant_entry(&s3, OLD, p);
    }
    s3.clear_requests();
    assert_absent(&h.exists(&dir, OLD));
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
    assert_eq!(other_prefix_listings(&s3, &own, OLD).len(), 200);

    for p in others.iter().take(300) {
        plant_entry(&s3, OLD, p);
    }
    s3.clear_requests();
    let out = h.exists(&dir, OLD);
    assert_unlocated(&out, "context exists", OLD, "no_local_copy");
    assert_eq!(top_level_listings(&s3), 1, "{:?}", listings(&s3));
    assert_swept(&s3, &others, OLD);

    for i in 0..1_500u32 {
        plant_entry(&s3, OLD, &format!("{:016x}", 0x1000_0000 + i));
    }
    s3.clear_requests();
    let out = h.exists(&dir, OLD);
    assert_unlocated(&out, "context exists", OLD, "no_local_copy");
    assert_eq!(listed(&s3, &index), 1, "{:?}", listings(&s3));
    assert_swept(&s3, &others, OLD);
}

/// A remembered "declared" carries a lookup through declaration reads that
/// keep failing only for 10 minutes after the last answered read, and the
/// failed reads don't move that limit. Past it, lookups sweep; an answered
/// read restores the bounded path.
#[test]
fn a_carried_declaration_expires_while_reads_keep_failing() {
    let s3 = FakeS3::start(BUCKET);
    let others = plant_prefixes(&s3, 8);
    plant_state(&s3, &others[5], OLD);
    let (h, dir) = declared_host(&s3, "elsewhere");

    s3.fail("GET", declaration::KEY, 503);
    age_memo(&h, Duration::from_secs(5 * 60));
    let read_at = memo(&h)["read_at"].clone();
    for _ in 0..2 {
        s3.clear_requests();
        assert_absent(&exists_windowed(&h, &dir, GHOST));
        assert!(declaration_gets(&s3) > 0, "the GET was skipped");
        assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
        assert_absent(&exists_windowed(&h, &dir, OLD));
        assert_eq!(memo(&h)["read_at"], read_at, "a failed read moved the memo");
    }

    age_memo(&h, Duration::from_secs(11 * 60));
    s3.clear_requests();
    assert_absent(&exists_windowed(&h, &dir, GHOST));
    assert_eq!(top_level_listings(&s3), 1, "{:?}", listings(&s3));
    assert_swept(&s3, &others, GHOST);
    let out = exists_windowed(&h, &dir, OLD);
    assert_unlocated(&out, "context exists", OLD, "no_local_copy");

    s3.clear_faults();
    s3.clear_requests();
    let expired_at = memo(&h)["read_at"].clone();
    assert_absent(&exists_windowed(&h, &dir, GHOST));
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
    assert_ne!(
        memo(&h)["read_at"],
        expired_at,
        "the answered read wasn't recorded"
    );
    s3.clear_requests();
    assert_absent(&exists_windowed(&h, &dir, OLD));
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
}

/// `HELD_ELSEWHERE_FANOUT` in `src/session/cloud.rs`: how many confirming
/// listings a lookup keeps in flight at once.
const FANOUT: usize = 64;

/// K leftover entries for a name (prefixes the index names that hold no
/// state for it) cost exactly K confirming listings on a declared bucket,
/// at most `FANOUT` in flight at once. With K = 100 against a slow bucket
/// they arrive in two waves: a worker sends its next listing only once its
/// last one has answered, so nothing reaches the bucket within one latency
/// of the first confirming listing but the first wave. The lookup answers
/// not-found and makes no top-level listing.
#[test]
fn k_leftover_entries_on_a_declared_bucket_cost_k_confirming_listings_in_two_waves() {
    const K: u32 = 100;
    let latency = Duration::from_millis(400);
    let s3 = FakeS3::start(BUCKET);
    let (h, dir) = declared_host(&s3, "elsewhere");
    let own = prefix_of(&dir);
    let leftovers = plant_prefixes(&s3, K);
    for p in &leftovers {
        plant_entry(&s3, GHOST, p);
    }

    s3.set_latency(latency);
    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    assert_absent(&out);
    assert_eq!(top_level_listings(&s3), 0, "{:?}", listings(&s3));
    assert_eq!(
        other_prefix_listings(&s3, &own, GHOST).len(),
        K as usize,
        "{:?}",
        listings(&s3)
    );
    assert_swept(&s3, &leftovers, GHOST);

    let confirming: Vec<String> = leftovers.iter().map(|p| format!("{p}/{GHOST}/")).collect();
    let mut at: Vec<_> = s3
        .requests()
        .iter()
        .filter(|r| {
            r.list_prefix
                .as_deref()
                .is_some_and(|l| confirming.iter().any(|c| c == l))
        })
        .map(|r| r.at)
        .collect();
    at.sort();
    let first = at[0];
    let first_wave = at.iter().filter(|t| **t - first < latency).count();
    assert_eq!(
        first_wave, FANOUT,
        "confirming listings within {latency:?} of the first"
    );
    assert!(
        at[FANOUT] - first >= latency,
        "the second wave started {:?} after the first",
        at[FANOUT] - first
    );
}

/// With no memo (`cloud-index.json` absent), a lookup on a declared bucket
/// starts the index listing, the declaration read and the top-level
/// listing in one round trip, and answers from the declared branch: an
/// entry-less session under another prefix reads not-found, and nothing
/// sweeps. The declaration read opens with a listing of its key
/// (`name_index::read_declaration`, which spares an absent declaration
/// rust-s3's retried 404 GET), so that listing is what joins the round
/// trip, and the GET follows it. A failed top-level listing changes no
/// answer.
#[test]
fn a_declared_bucket_with_no_memo_reads_in_one_round_trip_whatever_the_top_level_listing_does() {
    let latency = Duration::from_millis(300);
    let s3 = FakeS3::start(BUCKET);
    s3.put(declaration::KEY, &declaration::valid());
    let first = Host::new(&s3);
    let a = first.dir("a");
    first.init(&a);
    let planted = plant_prefixes(&s3, 10);
    plant_state(&s3, &planted[0], OLD);
    let h = Host::new(&s3);
    let dir = h.dir("elsewhere");
    let own = prefix_of(&dir);

    assert!(!h.memo_path().exists(), "a memo before the first lookup");
    s3.set_latency(latency);
    s3.clear_requests();
    let out = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    assert_absent(&out);

    let requests = lookup_requests(&s3, &own, GHOST);
    let at = |pred: &dyn Fn(&fake_s3::Request) -> bool, what: &str| {
        let found: Vec<_> = requests.iter().filter(|r| pred(r)).map(|r| r.at).collect();
        assert_eq!(found.len(), 1, "one {what}: {requests:?}");
        found[0]
    };
    let index_listing = format!("koto-names/{GHOST}/");
    let index = at(
        &|r| r.list_prefix.as_deref() == Some(index_listing.as_str()),
        "index listing",
    );
    let declaration_listing = at(
        &|r| r.list_prefix.as_deref() == Some(DECLARATION),
        "declaration listing",
    );
    let top_level = at(
        &|r| r.list_prefix.as_deref() == Some("") && r.delimiter.as_deref() == Some("/"),
        "top-level listing",
    );
    let get = at(
        &|r| r.method == "GET" && r.key == declaration::KEY,
        "declaration GET",
    );
    let step_one = [index, declaration_listing, top_level];
    let earliest = *step_one.iter().min().unwrap();
    let latest = *step_one.iter().max().unwrap();
    assert!(
        latest - earliest < latency,
        "the first step's requests went out over {:?}",
        latest - earliest
    );
    assert!(
        declaration_listing < get,
        "the GET went out before the listing"
    );
    assert!(
        other_prefix_listings(&s3, &own, GHOST).is_empty(),
        "{:?}",
        listings(&s3)
    );

    // A failed top-level listing is asked again (`list_retrying`), so it
    // is asked at least once rather than exactly once.
    let check = |what: &str, failed: bool| {
        assert!(!h.memo_path().exists(), "{what}: a memo was written");
        s3.clear_requests();
        assert_absent(&h.exists(&dir, OLD));
        let asked = top_level_listings(&s3);
        assert!(
            if failed { asked >= 1 } else { asked == 1 },
            "{what}: {:?}",
            listings(&s3)
        );
        assert!(
            other_prefix_listings(&s3, &own, OLD).is_empty(),
            "{what}: {:?}",
            listings(&s3)
        );
        assert_absent(&h.exists(&dir, GHOST));
        let out = h.exists(&dir, WF);
        assert_unlocated(&out, "context exists", WF, "no_local_copy");
    };
    check("top-level listing answered", false);

    s3.fail("LIST", "", 503);
    check("top-level listing failed", true);
    s3.clear_faults();
}

// -- the lookup's end-to-end deadline (koto#597) --

/// How long a `koto status` whose lookup is cut at the 6-second deadline
/// may take: the deadline, plus the process's start, its other requests
/// and slack for a loaded test host.
const LOOKUP_BOUND: Duration = Duration::from_secs(12);

/// On a declared bucket of 2,000 workspace prefixes across a slow network,
/// four `koto status` lookups of missing names, run one after another, each
/// answer not-found within the bound and all four together in under 30
/// seconds: the index answers, so no lookup sweeps.
#[test]
fn four_missing_names_on_a_large_declared_bucket_answer_not_found_quickly() {
    const PREFIXES: u32 = 2_000;
    let latency = Duration::from_millis(100);
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, PREFIXES);
    let (h, dir) = declared_host(&s3, "elsewhere");

    s3.set_latency(latency);
    let all = std::time::Instant::now();
    for name in ["missing-1", "missing-2", "missing-3", "missing-4"] {
        let started = std::time::Instant::now();
        let out = h.koto(&dir, &["status", name]);
        let took = started.elapsed();
        assert!(took < LOOKUP_BOUND, "{name} took {took:?}");
        assert_eq!(out.code, Some(2), "koto status {name}: {}", out.describe());
        let err = out.json();
        assert_eq!(
            err["error"],
            format!("workflow '{name}' not found"),
            "{}",
            out.describe()
        );
        assert!(err.get("code").is_none(), "{}", out.describe());
        println!("koto status {name}: {took:?}");
    }
    let total = all.elapsed();
    s3.set_latency(Duration::ZERO);
    println!("four lookups: {total:?}");
    assert!(
        total < Duration::from_secs(30),
        "four lookups took {total:?}"
    );
}

/// On an undeclared bucket whose sweep can't finish inside the deadline
/// (2,000 workspace prefixes, each listing answered only after 300 ms, so
/// about 32 rounds of 64), `koto status` of a missing name refuses with
/// `session_lookup_failed`, exit 3, within the bound, naming the prefixes
/// it didn't get to ask, and never answers not-found.
#[test]
fn a_sweep_cut_at_the_deadline_is_cant_tell_never_not_found() {
    const PREFIXES: u32 = 2_000;
    let latency = Duration::from_millis(300);
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, PREFIXES);
    let h = Host::new(&s3);
    let dir = h.dir("elsewhere");

    s3.set_latency(latency);
    let started = std::time::Instant::now();
    let out = h.koto(&dir, &["status", GHOST]);
    let took = started.elapsed();
    s3.set_latency(Duration::ZERO);
    assert!(took < LOOKUP_BOUND, "took {took:?}: {}", out.describe());
    let message = assert_lookup_failed(&out, "status", GHOST);
    assert!(!message.contains("not found"), "{message}");
    assert!(
        message.contains("not asked before koto stopped waiting at the lookup's 6s deadline"),
        "{message}"
    );
    println!("cut sweep: {took:?}");
}

/// The remedy a failed lookup on an undeclared bucket names (koto#675).
const DECLARE_REMEDY: [&str; 2] = ["`koto session index audit`", "`koto session index declare`"];

/// `session_lookup_failed`'s `reason` and `index` fields (koto#675).
fn assert_reason(out: &Out, reason: &str, index: &str) {
    let err = out.json();
    assert_eq!(err["reason"], reason, "{}", out.describe());
    assert_eq!(err["index"], index, "{}", out.describe());
}

/// Whether `message` names both commands of the undeclared remedy.
fn names_declare_remedy(message: &str) -> bool {
    DECLARE_REMEDY
        .iter()
        .all(|command| message.contains(command))
}

/// On an undeclared bucket whose top-level listing never answers, a lookup
/// of a missing name refuses within the bound with `reason: timed_out` and
/// `index: undeclared`, says asking again can succeed rather than only
/// "once the bucket answers", and names `koto session index audit` and
/// `koto session index declare` as what lets the lookup finish (koto#675).
#[test]
fn a_stalled_top_level_listing_on_an_undeclared_bucket_is_timed_out_and_names_the_remedy() {
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, 6);
    let h = Host::new(&s3);
    let dir = h.dir("elsewhere");

    // Far past the deadline, so no ask of the top-level listing is answered.
    s3.stall("LIST", "", Duration::from_secs(300));
    let started = std::time::Instant::now();
    let out = h.koto(&dir, &["status", GHOST]);
    let took = started.elapsed();
    assert!(took < LOOKUP_BOUND, "took {took:?}: {}", out.describe());
    let message = assert_lookup_failed(&out, "status", GHOST);
    assert_reason(&out, "timed_out", "undeclared");
    assert!(
        message.contains("listing the bucket's workspace prefixes failed"),
        "{message}"
    );
    assert!(message.contains("asking again can succeed"), "{message}");
    assert!(!message.contains("once the bucket answers"), "{message}");
    assert!(names_declare_remedy(&message), "{message}");
}

/// On an undeclared bucket whose sweep is cut at the deadline, the refusal
/// is `reason: timed_out`, `index: undeclared`, with the remedy (koto#675).
#[test]
fn a_sweep_cut_at_the_deadline_is_timed_out_on_an_undeclared_bucket() {
    const PREFIXES: u32 = 2_000;
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, PREFIXES);
    let h = Host::new(&s3);
    let dir = h.dir("elsewhere");

    s3.set_latency(Duration::from_millis(300));
    let out = h.exists(&dir, GHOST);
    s3.set_latency(Duration::ZERO);
    let message = assert_lookup_failed(&out, "context exists", GHOST);
    assert_reason(&out, "timed_out", "undeclared");
    assert!(names_declare_remedy(&message), "{message}");
}

/// On an undeclared bucket whose top-level listing the bucket keeps
/// failing, the lookup ends with time left: `reason: listing_failed`, and
/// the message still says to run the command again once the bucket
/// answers, with the remedy, since the bucket reads undeclared (koto#675).
#[test]
fn a_top_level_listing_the_bucket_keeps_failing_is_listing_failed() {
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, 6);
    let h = Host::new(&s3);
    let dir = h.dir("elsewhere");

    s3.fail("LIST", "", 503);
    let out = h.koto(&dir, &["status", GHOST]);
    s3.clear_faults();
    let message = assert_lookup_failed(&out, "status", GHOST);
    assert_reason(&out, "listing_failed", "undeclared");
    assert!(message.contains("503"), "{message}");
    assert!(message.contains("once the bucket answers"), "{message}");
    assert!(names_declare_remedy(&message), "{message}");
}

/// On a declared bucket a failed index listing is `reason:
/// listing_failed`, `index: declared`, and names no remedy: the index is
/// already declared (koto#675).
#[test]
fn a_failed_index_listing_on_a_declared_bucket_names_no_remedy() {
    let s3 = FakeS3::start(BUCKET);
    plant_prefixes(&s3, 6);
    let (h, dir) = declared_host(&s3, "elsewhere");

    s3.fail("LIST", &format!("koto-names/{GHOST}/"), 503);
    let out = h.exists(&dir, GHOST);
    s3.clear_faults();
    let message = assert_lookup_failed(&out, "context exists", GHOST);
    assert_reason(&out, "listing_failed", "declared");
    assert!(!names_declare_remedy(&message), "{message}");
}

/// A bucket that can't be reached is `reason: unreachable`, `index:
/// unknown`: the lookup never read the declaration (koto#675).
#[test]
fn an_unreachable_bucket_is_reason_unreachable_with_index_unknown() {
    let s3 = FakeS3::start(BUCKET);
    let (mut h, dir) = declared_host(&s3, "offline");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    h.endpoint = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);

    let out = h.koto(&dir, &["status", GHOST]);
    let message = assert_lookup_failed(&out, "status", GHOST);
    assert_reason(&out, "unreachable", "unknown");
    assert!(!names_declare_remedy(&message), "{message}");
}
