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
    assert_eq!(out.code, Some(2), "{}", out.describe());
    let err = out.json();
    let message = err["error"].as_str().unwrap();
    assert!(message.contains(&format!("'{WF}'")), "{message}");
    assert!(message.contains("can't tell"), "{message}");
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

/// A valid declaration planted by hand changes no reply: `status` and
/// `context exists` of a missing name, an older writer's name and an
/// indexed name answer as they did without it. The memo records it as
/// declared, and a second lookup still reads the declaration, lists the
/// top level and sweeps on a miss.
#[test]
fn a_planted_declaration_changes_no_reply_and_skips_no_request() {
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

    s3.put(DECLARATION, &declaration());
    for (args, before) in commands.iter().zip(&undeclared) {
        assert_eq!(&second.koto(&b, args).reply(), before, "koto {args:?}");
    }
    let memo: serde_json::Value =
        serde_json::from_slice(&std::fs::read(second.memo_path()).unwrap()).unwrap();
    assert_eq!(memo["declared"], true, "{memo}");
    assert_eq!(memo["bucket"], BUCKET, "{memo}");

    s3.clear_requests();
    assert_absent(&second.exists(&b, GHOST));
    assert_eq!(listed(&s3, DECLARATION), 1);
    assert_eq!(declaration_gets(&s3), 1);
    assert_eq!(top_level_listings(&s3), 1);
    assert_eq!(listed(&s3, &format!("koto-names/{GHOST}/")), 1);
    assert_swept(&s3, &others, GHOST);
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
