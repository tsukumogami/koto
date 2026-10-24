//! `koto session import` on the host the source was created on: one `HOME`,
//! two workspaces, the cloud backend enabled through `KOTO_SESSION_*`
//! variables alone, and the in-process endpoint in `tests/support/fake_s3.rs`.
//!
//! On one host every workspace shares the local store, so the source's own
//! copy holds the session's name. The import takes the name over: it marks
//! the source, then exchanges the staged session with the source's copy in
//! one call. These tests drive the binary through that, through every
//! refusal that must leave the source untouched, and through a stop at each
//! step followed by a re-run of the same command.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use assert_cmd::Command;
use fake_s3::FakeS3;
use migration_carrier::{prefix_of, TEMPLATE, TEMPLATE_FILE};
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-same-host";
const NAME: &str = "coord";

/// Removed from every command's environment before a test sets its own.
const CLEARED: &[&str] = &[
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_CLOUD_ENDPOINT",
    "KOTO_SESSION_CLOUD_BUCKET",
    "KOTO_SESSION_CLOUD_REGION",
    "KOTO_SESSION_CLOUD_PATH_STYLE",
    "KOTO_SESSION_CLOUD_ACCESS_KEY",
    "KOTO_SESSION_CLOUD_SECRET_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "KOTO_SESSIONS_BASE",
    "KOTO_DECIDER",
    "KOTO_DECIDER_API_KEY",
    "KOTO_DECIDER_ENDPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_WORKFLOWS_DIR",
    "KOTO_TEST_IMPORT_FAULT",
    "KOTO_ANCHOR",
    "KOTO_DISPATCHER_ANCHOR",
    "KOTO_RETENTION_KEEP",
    "KOTO_RETENTION_IDLE_PERIOD",
    "KOTO_RETENTION_STALE_AFTER",
];

/// One host: one `HOME` and template cache, two workspaces.
struct Host {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    a: PathBuf,
    b: PathBuf,
    endpoint: String,
}

struct Out {
    code: Option<i32>,
    raw: Vec<u8>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
    /// stdout as JSON: the whole of it (a pretty-printed answer), else its
    /// last non-empty line.
    fn json(&self) -> serde_json::Value {
        if let Ok(v) = serde_json::from_str(self.stdout.trim()) {
            return v;
        }
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line).unwrap_or(serde_json::Value::Null)
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

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        let a = mk(root.path().join("instance-a"));
        let b = mk(root.path().join("instance-b"));
        for ws in [&a, &b] {
            std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        }
        Host {
            _root: root,
            home,
            cache,
            a,
            b,
            endpoint: s3.endpoint(),
        }
    }

    fn cmd(&self, ws: &Path, args: &[&str], extra: &[(&str, &str)]) -> Command {
        let mut cmd = koto_command::koto();
        for k in CLEARED {
            cmd.env_remove(k);
        }
        cmd.current_dir(ws)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.args(args);
        cmd
    }

    fn koto(&self, ws: &Path, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let o = self.cmd(ws, args, extra).output().unwrap();
        Out {
            code: o.status.code(),
            raw: o.stdout.clone(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn add(&self, ws: &Path, key: &str, content: &[u8]) {
        let o = self
            .cmd(ws, &["context", "add", NAME, key], &[])
            .write_stdin(content.to_vec())
            .output()
            .unwrap();
        assert!(o.status.success(), "context add {key}: {:?}", o);
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto").join("sessions")
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    fn pending(&self) -> PathBuf {
        self.sessions().join(format!(".takeover-{}", NAME))
    }

    fn import_from_a(&self, extra: &[(&str, &str)]) -> Out {
        self.koto(
            &self.b,
            &[
                "session",
                "import",
                NAME,
                "--from",
                self.a.to_str().unwrap(),
            ],
            extra,
        )
    }
}

/// The three context keys the source holds, and their bytes.
const KEYS: [(&str, &[u8]); 3] = [
    ("notes.md", b"the notes\n"),
    ("plan/steps.md", b"step one\nstep two\n"),
    ("bin.dat", &[0, 1, 2, 255]),
];

/// A source session in workspace A, advanced into the state that waits for
/// evidence, with three context keys, then left alone.
fn source(s3: &FakeS3) -> Host {
    let h = Host::new(s3);
    let out = h.koto(&h.a, &["init", NAME, "--template", TEMPLATE_FILE], &[]);
    assert!(out.ok(), "init: {}", out.describe());
    for (k, v) in KEYS {
        h.add(&h.a, k, v);
    }
    let out = h.koto(&h.a, &["next", NAME], &[]);
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());
    h
}

fn marker_key(h: &Host) -> String {
    format!("{}/{}/migrated.json", prefix_of(&h.a), NAME)
}

fn holds_import(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).contains("\"session_imported\"")
}

/// Named files and their bytes.
type Files = Vec<(String, Vec<u8>)>;

/// One way to leave the source's local copy ahead of the bucket.
type Ahead = Box<dyn Fn(&Host)>;

/// Everything the source owns: its local directory's files and its
/// objects in the bucket.
fn snapshot(h: &Host, s3: &FakeS3) -> (Files, Files) {
    let mut local = Vec::new();
    let dir = h.sessions().join(NAME);
    for entry in walk(&dir) {
        let rel = entry
            .strip_prefix(&dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if rel.ends_with(".lock") || rel == "run-journal.json" {
            continue;
        }
        local.push((rel, std::fs::read(&entry).unwrap()));
    }
    local.sort();
    let prefix = format!("{}/{}/", prefix_of(&h.a), NAME);
    let mut remote: Files = s3
        .keys_under(&prefix)
        .into_iter()
        .map(|k| {
            let v = s3.object(&k).unwrap();
            (k, v)
        })
        .collect();
    remote.sort();
    (local, remote)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn b_objects(h: &Host, s3: &FakeS3) -> Vec<String> {
    s3.keys_under(&format!("{}/{}/", prefix_of(&h.b), NAME))
}

fn assert_source_untouched(h: &Host, s3: &FakeS3, before: &(Files, Files), what: &str) {
    assert_eq!(&snapshot(h, s3), before, "{what}: the source changed");
    assert!(s3.object(&marker_key(h)).is_none(), "{what}: a marker");
    assert!(b_objects(h, s3).is_empty(), "{what}: pushed under B");
    assert!(!h.pending().exists(), "{what}: a .takeover directory");
}

fn assert_refused(out: &Out, code: &str) {
    assert_eq!(out.code, Some(2), "{}", out.describe());
    assert_eq!(out.json()["error"]["code"], code, "{}", out.describe());
}

/// The replacement continues; the source refuses, naming B.
fn assert_moved(h: &Host) {
    let out = h.koto(
        &h.b,
        &["next", NAME, "--with-data", r#"{"choice":"go"}"#],
        &[],
    );
    assert!(out.ok(), "next in B: {}", out.describe());
    assert_eq!(out.json()["state"], "review", "{}", out.describe());
    for args in [
        vec!["next", NAME],
        vec!["status", NAME],
        vec!["context", "get", NAME, "notes.md"],
    ] {
        let out = h.koto(&h.a, &args, &[]);
        assert_eq!(out.code, Some(2), "{args:?} from A: {}", out.describe());
        let all = format!("{}{}", out.stdout, out.stderr);
        assert!(all.contains("session_migrated"), "{args:?}: {all}");
        assert!(
            all.contains(h.b.to_str().unwrap()),
            "{args:?} names B: {all}"
        );
    }
}

#[test]
fn a_same_host_import_keeps_the_name_and_the_replacement_continues() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let source_log = s3
        .object(&format!(
            "{}/{}/koto-{}.state.jsonl",
            prefix_of(&h.a),
            NAME,
            NAME
        ))
        .unwrap();

    let out = h.import_from_a(&[]);
    assert!(out.ok(), "import: {}", out.describe());
    let j = out.json();
    assert_eq!(j["name"], NAME);
    assert_eq!(j["imported"], true);
    assert_eq!(j["marked"], true);
    assert_eq!(j["keys"], 3);
    assert_eq!(j["from"]["session"], NAME);
    assert_eq!(j["from"]["workspace"], h.a.to_str().unwrap());
    let mut fields: Vec<&String> = j.as_object().unwrap().keys().collect();
    fields.sort();
    assert_eq!(
        fields,
        ["from", "imported", "keys", "marked", "name", "template"]
    );
    assert!(!h.pending().exists(), "the old copy was deleted");
    let local = std::fs::read(h.state_path(NAME)).unwrap();

    for (k, v) in KEYS {
        let out = h.koto(&h.b, &["context", "get", NAME, k], &[]);
        assert!(out.ok(), "get {k}: {}", out.describe());
        assert_eq!(out.raw, v, "{k} byte for byte");
    }

    // B's log, as the import left it, is the source's events plus one
    // session_imported.
    let events = |b: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(b)
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    };
    let (src, here) = (events(&source_log), events(&local));
    assert_eq!(here.len(), src.len() + 1);
    assert_eq!(&here[..src.len()], &src[..]);
    assert!(here.last().unwrap().contains("\"session_imported\""));

    assert_moved(&h);
}

#[test]
fn an_import_refuses_a_source_still_ahead_running_or_with_children() {
    // Ahead: an event appended locally and never pushed, a forked event at
    // a seq both hold, and a context key added locally only.
    let cases: Vec<(&str, Ahead)> = vec![
        (
            "longer",
            Box::new(|h: &Host| {
                let path = h.state_path(NAME);
                let mut log = std::fs::read_to_string(&path).unwrap();
                let last = log.lines().filter(|l| !l.trim().is_empty()).count() - 1;
                log.push_str(&format!(
                    "{{\"seq\":{},\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"intent_updated\",\"payload\":{{\"intent\":\"local\"}}}}\n",
                    last + 1
                ));
                std::fs::write(&path, log).unwrap();
            }),
        ),
        (
            "forked",
            Box::new(|h: &Host| {
                let path = h.state_path(NAME);
                let log = std::fs::read_to_string(&path).unwrap();
                let mut lines: Vec<String> = log.lines().map(str::to_string).collect();
                let n = lines.len();
                lines[n - 1] = lines[n - 1].replace("2026", "2025");
                if lines[n - 1] == log.lines().last().unwrap() {
                    lines[n - 1] = lines[n - 1].replacen("\"seq\"", "\"note\":1,\"seq\"", 1);
                }
                std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            }),
        ),
        (
            "context",
            Box::new(|h: &Host| {
                let ctx = h.sessions().join(NAME).join("ctx");
                std::fs::write(ctx.join("local-only.md"), b"x").unwrap();
                let mpath = ctx.join("manifest.json");
                let mut m: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&mpath).unwrap()).unwrap();
                m["keys"]["local-only.md"] = serde_json::json!({
                    "created_at": "2099-01-01T00:00:00Z",
                    "size": 1,
                    "hash": "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
                });
                std::fs::write(&mpath, serde_json::to_vec(&m).unwrap()).unwrap();
            }),
        ),
    ];
    for (what, make_ahead) in cases {
        let s3 = FakeS3::start(BUCKET);
        let h = source(&s3);
        make_ahead(&h);
        let before = snapshot(&h, &s3);
        let out = h.import_from_a(&[]);
        assert_refused(&out, "import_source_unpushed");
        assert!(
            out.stdout
                .contains("koto session resolve coord --keep local"),
            "{what}: {}",
            out.describe()
        );
        assert_source_untouched(&h, &s3, &before, what);
    }

    // Running: another process holds the state-file lock, or the context
    // manifest's.
    for which in ["state", "manifest"] {
        let s3 = FakeS3::start(BUCKET);
        let h = source(&s3);
        let before = snapshot(&h, &s3);
        let path = match which {
            "state" => h.state_path(NAME),
            _ => h.sessions().join(NAME).join("ctx").join("manifest.lock"),
        };
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: the fd belongs to `lock`, held for the import below.
            let ret = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            assert_eq!(ret, 0);
        }
        let out = h.import_from_a(&[]);
        assert_refused(&out, "import_source_running");
        drop(lock);
        assert_source_untouched(&h, &s3, &before, which);
    }

    // Children: a session naming the source as its parent.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let kid = h.sessions().join("kid");
    std::fs::create_dir_all(&kid).unwrap();
    std::fs::write(
        kid.join("koto-kid.state.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::json!({"schema_version":1,"workflow":"kid","template_hash":"a".repeat(64),"created_at":"2026-01-01T00:00:00Z","parent_workflow":NAME,"session_id":"kid-id"}),
            serde_json::json!({"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"intent_updated","payload":{"intent":"k"}})
        ),
    )
    .unwrap();
    let before = snapshot(&h, &s3);
    let out = h.import_from_a(&[]);
    assert_refused(&out, "import_source_has_children");
    assert!(out.stdout.contains("kid"), "{}", out.describe());
    assert_source_untouched(&h, &s3, &before, "children");
}

#[test]
fn a_local_copy_behind_the_bucket_imports_the_bucket_s_events() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    // Drop the last event locally: the bucket is one event ahead.
    let path = h.state_path(NAME);
    let log = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    std::fs::write(&path, lines[..lines.len() - 1].join("\n") + "\n").unwrap();
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    let here = std::fs::read_to_string(h.state_path(NAME)).unwrap();
    assert_eq!(
        here.lines().filter(|l| !l.trim().is_empty()).count(),
        lines.len() + 1
    );
}

#[test]
fn another_session_at_the_name_and_a_same_workspace_import_keep_today_s_refusal() {
    // Another session holds the name: the source exists only in A's prefix,
    // put there from another host's HOME.
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let other_home = h._root.path().join("other-home");
    std::fs::create_dir_all(&other_home).unwrap();
    let mut init = h.cmd(&h.a, &["init", NAME, "--template", TEMPLATE_FILE], &[]);
    init.env("HOME", &other_home);
    assert!(init.output().unwrap().status.success());
    let out = h.koto(&h.b, &["init", NAME, "--template", TEMPLATE_FILE], &[]);
    assert!(out.ok(), "{}", out.describe());
    let mine = std::fs::read(h.state_path(NAME)).unwrap();
    s3.clear_requests();
    let out = h.import_from_a(&[]);
    assert_refused(&out, "import_name_taken");
    assert!(out.stdout.contains("--as"), "{}", out.describe());
    assert_eq!(std::fs::read(h.state_path(NAME)).unwrap(), mine);
    assert!(s3.requests().iter().all(|r| r.method != "PUT"));

    // The importing workspace is the source's own: today's refusal.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let before = snapshot(&h, &s3);
    let out = h.koto(
        &h.a,
        &["session", "import", NAME, "--from", h.a.to_str().unwrap()],
        &[],
    );
    assert_refused(&out, "import_name_taken");
    assert_source_untouched(&h, &s3, &before, "same workspace");
}

#[test]
fn the_probe_refuses_before_anything_is_written() {
    // The filesystem can't exchange, and the name is on another device.
    for fault in ["exchange-unsupported", "name-other-device"] {
        let s3 = FakeS3::start(BUCKET);
        let h = source(&s3);
        let before = snapshot(&h, &s3);
        let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", fault)]);
        assert!(!out.ok(), "{fault}: {}", out.describe());
        assert!(out.stdout.contains("--as"), "{fault}: {}", out.describe());
        assert_source_untouched(&h, &s3, &before, fault);
    }
    // A symlink at the name, pointing at the source's real directory.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let real = h.sessions().join("real-coord");
    std::fs::rename(h.sessions().join(NAME), &real).unwrap();
    std::os::unix::fs::symlink(&real, h.sessions().join(NAME)).unwrap();
    let before = snapshot(&h, &s3);
    let out = h.import_from_a(&[]);
    assert!(!out.ok(), "{}", out.describe());
    assert!(
        out.stdout.contains("not a plain directory"),
        "{}",
        out.describe()
    );
    assert_source_untouched(&h, &s3, &before, "symlink");
    assert!(std::fs::symlink_metadata(h.sessions().join(NAME))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_stop_at_any_step_is_finished_by_running_the_same_command_again() {
    for fault in [
        "after-staging",
        "after-push",
        "after-mark",
        "exchange-error",
        "after-exchange",
        "after-journal",
    ] {
        let s3 = FakeS3::start(BUCKET);
        let h = source(&s3);
        let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", fault)]);
        assert!(!out.ok(), "{fault}: {}", out.describe());

        // In between: one directory holds the name, and it is the new
        // session only once the source carries its marker.
        let holder = std::fs::symlink_metadata(h.sessions().join(NAME)).unwrap();
        assert!(holder.is_dir(), "{fault}");
        if holds_import(&std::fs::read(h.state_path(NAME)).unwrap()) {
            assert!(
                s3.object(&marker_key(&h)).is_some(),
                "{fault}: the new session is local but the source is unmarked"
            );
        }

        let out = h.import_from_a(&[]);
        assert!(out.ok(), "{fault} re-run: {}", out.describe());
        assert_eq!(out.json()["name"], NAME, "{fault}");
        assert!(!h.pending().exists(), "{fault}: .takeover left behind");
        assert_moved(&h);
    }
}

#[test]
fn a_marker_that_definitely_failed_takes_everything_back() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let before = snapshot(&h, &s3);
    s3.fail("PUT", &marker_key(&h), 500);
    let out = h.import_from_a(&[]);
    assert!(!out.ok(), "{}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "import_push_failed",
        "{}",
        out.describe()
    );
    assert_source_untouched(&h, &s3, &before, "definite marker failure");
}

#[test]
fn a_same_host_import_makes_no_more_requests_than_a_cross_host_one() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    s3.clear_requests();
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    let same_host = s3.requests().len();

    // The same session imported by another host: its own HOME and cache.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let other = h._root.path().join("host-c");
    let (c_home, c_cache, c_ws) = (other.join("home"), other.join("cache"), other.join("ws"));
    for d in [&c_home, &c_cache, &c_ws] {
        std::fs::create_dir_all(d).unwrap();
    }
    let c_ws = std::fs::canonicalize(&c_ws).unwrap();
    std::fs::write(c_ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
    let mut compile = h.cmd(&c_ws, &["template", "compile", TEMPLATE_FILE], &[]);
    compile.env("HOME", &c_home).env("XDG_CACHE_HOME", &c_cache);
    assert!(compile.output().unwrap().status.success());
    s3.clear_requests();
    let mut import = h.cmd(
        &c_ws,
        &["session", "import", NAME, "--from", h.a.to_str().unwrap()],
        &[],
    );
    import.env("HOME", &c_home).env("XDG_CACHE_HOME", &c_cache);
    let o = import.output().unwrap();
    assert!(o.status.success(), "{:?}", o);
    let cross_host = s3.requests().len();
    assert!(
        same_host <= cross_host,
        "same host {same_host} requests, cross host {cross_host}"
    );
}

#[test]
fn the_source_workspace_can_t_touch_the_imported_copy_even_without_the_marker() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    let b_dir = h.sessions().join(NAME);
    let before: Vec<(PathBuf, Vec<u8>)> = walk(&b_dir)
        .into_iter()
        .filter(|p| !p.to_string_lossy().ends_with(".lock"))
        .map(|p| {
            let v = std::fs::read(&p).unwrap();
            (p, v)
        })
        .collect();
    // The marker listing fails: only the local guard can refuse.
    s3.fail("LIST", &marker_key(&h), 500);
    for args in [
        vec!["next", NAME],
        vec!["session", "cleanup", NAME],
        vec!["session", "resolve", NAME, "--keep", "local"],
    ] {
        let out = h.koto(&h.a, &args, &[]);
        // `next` refuses with exit 2, as documented; cleanup and resolve
        // keep their own failure exit.
        if args[0] == "next" {
            assert_eq!(out.code, Some(2), "{args:?}: {}", out.describe());
        } else {
            assert!(!out.ok(), "{args:?}: {}", out.describe());
        }
        let all = format!("{}{}", out.stdout, out.stderr);
        assert!(all.contains("session_migrated"), "{args:?}: {all}");
        assert!(
            all.contains(h.b.to_str().unwrap()),
            "{args:?} names B: {all}"
        );
    }
    for (p, v) in before {
        assert_eq!(std::fs::read(&p).unwrap(), v, "{}", p.display());
    }
}

/// `log` with one more `intent_updated` event after its last.
fn with_one_more_event(log: &str, intent: &str) -> String {
    let next = log.lines().filter(|l| !l.trim().is_empty()).count();
    format!(
        "{log}{}\n",
        serde_json::json!({
            "seq": next,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "intent_updated",
            "payload": {"intent": intent},
        })
    )
}

#[test]
fn a_re_run_after_the_marker_never_overwrites_a_copy_that_moved_on() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", "after-mark")]);
    assert!(!out.ok(), "{}", out.describe());
    assert!(s3.object(&marker_key(&h)).is_some());

    // B's copy in the bucket gains an event after its session_imported.
    let key = format!("{}/{}/koto-{}.state.jsonl", prefix_of(&h.b), NAME, NAME);
    let log = with_one_more_event(
        &String::from_utf8(s3.object(&key).unwrap()).unwrap(),
        "moved on",
    );
    s3.put(&key, log.as_bytes());
    let local_before = std::fs::read(h.state_path(NAME)).unwrap();

    let out = h.import_from_a(&[]);
    assert_refused(&out, "import_source_migrated");
    assert!(out.stdout.contains("moved on"), "{}", out.describe());
    assert_eq!(s3.object(&key).unwrap(), log.as_bytes());
    assert_eq!(std::fs::read(h.state_path(NAME)).unwrap(), local_before);
}

/// The `.kept-takeover-<name>-*` directories in the store.
fn kept_copies(h: &Host) -> Vec<PathBuf> {
    let prefix = format!(".kept-takeover-{}-", NAME);
    std::fs::read_dir(h.sessions())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect()
}

/// After a stop past the exchange, change the old copy with `change`, run
/// the import again, and return the one place it was kept.
fn kept_after(h: &Host, change: impl Fn(&Path)) -> PathBuf {
    let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", "after-exchange")]);
    assert!(!out.ok(), "{}", out.describe());
    change(&h.pending());
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    let kept = kept_copies(h);
    assert_eq!(kept.len(), 1, "{:?}: {}", kept, out.describe());
    assert!(
        out.stderr.contains(kept[0].to_str().unwrap()),
        "the warning names the kept copy: {}",
        out.describe()
    );
    // Moved aside, it no longer holds the name: the new session goes on.
    assert!(std::fs::symlink_metadata(h.pending()).is_err());
    assert_moved(h);
    kept[0].clone()
}

#[test]
fn an_old_copy_that_changed_is_moved_aside_and_the_name_stays_usable() {
    // An event appended to the old copy after the exchange.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let state = format!("koto-{}.state.jsonl", NAME);
    let log = std::cell::RefCell::new(String::new());
    let kept = kept_after(&h, |old| {
        let grown =
            with_one_more_event(&std::fs::read_to_string(old.join(&state)).unwrap(), "late");
        std::fs::write(old.join(&state), &grown).unwrap();
        *log.borrow_mut() = grown;
    });
    assert_eq!(
        std::fs::read_to_string(kept.join(&state)).unwrap(),
        *log.borrow()
    );

    // A context key added to the old copy and never pushed: the re-run
    // compares the old copy's manifest with the bucket's.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let kept = kept_after(&h, |old| {
        let ctx = old.join("ctx");
        std::fs::write(ctx.join("late.md"), b"x").unwrap();
        let mpath = ctx.join("manifest.json");
        let mut m: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&mpath).unwrap()).unwrap();
        m["keys"]["late.md"] = serde_json::json!({
            "created_at": "2099-01-01T00:00:00Z",
            "size": 1,
            "hash": "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        });
        std::fs::write(&mpath, serde_json::to_vec(&m).unwrap()).unwrap();
    });
    assert_eq!(
        std::fs::read(kept.join("ctx").join("late.md")).unwrap(),
        b"x"
    );

    // A symlink at .takeover-<name> is moved aside, never followed.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let real = h.sessions().join("elsewhere");
    let kept = kept_after(&h, |old| {
        std::fs::rename(old, &real).unwrap();
        std::os::unix::fs::symlink(&real, old).unwrap();
    });
    assert!(std::fs::symlink_metadata(&kept)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(real.join(&state).exists());
}

#[test]
fn a_stop_while_staging_or_deleting_leaves_nothing_a_re_run_trips_on() {
    // Stopped with the session staged but not yet in .takeover-<name>.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", "after-staging")]);
    assert!(!out.ok(), "{}", out.describe());
    assert!(
        !h.pending().exists(),
        "only a complete session is renamed in"
    );
    let out = h.koto(&h.a, &["status", NAME], &[]);
    assert!(
        out.ok(),
        "the source isn't held by a staging: {}",
        out.describe()
    );
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    assert_moved(&h);

    // A .takeover-<name> with no state file: a deletion that stopped.
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    std::fs::create_dir_all(h.pending().join("ctx")).unwrap();
    std::fs::write(h.pending().join("ctx").join("leftover"), b"x").unwrap();
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    assert!(!h.pending().exists());
    assert_moved(&h);
}

#[test]
fn an_unknown_marker_outcome_keeps_the_source_refusing_until_a_re_run() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    s3.fail("PUT", &marker_key(&h), 500);
    let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", "marker-unknown")]);
    assert_eq!(out.code, Some(1), "{}", out.describe());
    assert_eq!(
        out.json()["error"]["code"],
        "import_unmarked",
        "{}",
        out.describe()
    );
    // The staged session stays, and with it the source's refusal.
    assert!(h
        .pending()
        .join(format!("koto-{}.state.jsonl", NAME))
        .exists());
    let out = h.koto(&h.a, &["next", NAME], &[]);
    assert!(!out.ok(), "{}", out.describe());
    assert!(
        format!("{}{}", out.stdout, out.stderr).contains("being imported"),
        "{}",
        out.describe()
    );

    s3.clear_faults();
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    assert!(!h.pending().exists());
    assert_moved(&h);
}

#[test]
fn a_pending_takeover_refuses_commands_on_the_name() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    std::fs::create_dir_all(h.pending()).unwrap();
    let before = snapshot(&h, &s3);
    for args in [
        vec!["status", NAME],
        vec!["next", NAME],
        vec!["context", "add", NAME, "late.md"],
    ] {
        let out = h.koto(&h.a, &args, &[]);
        assert!(!out.ok(), "{args:?}: {}", out.describe());
        let all = format!("{}{}", out.stdout, out.stderr);
        assert!(all.contains("being imported"), "{args:?}: {all}");
        assert!(
            all.contains(&format!("koto session import {}", NAME)),
            "{args:?} names the import to run: {all}"
        );
    }
    assert_eq!(snapshot(&h, &s3), before);
    assert!(std::fs::read_dir(h.pending()).unwrap().next().is_none());
}

#[test]
fn a_takeover_removes_the_name_s_finished_record() {
    use std::os::unix::fs::DirBuilderExt;
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    // A stale record of an earlier session that finished under the name.
    let finished = h.sessions().join(".finished");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&finished)
        .unwrap();
    let record = finished.join(format!("{}.json", NAME));
    std::fs::write(&record, br#"{"name":"coord","status":"done"}"#).unwrap();

    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    assert!(
        !record.exists(),
        "the finished record of the name was removed"
    );
    assert_moved(&h);
}

#[test]
fn a_re_run_after_a_stop_past_the_exchange_removes_the_finished_record() {
    use std::os::unix::fs::DirBuilderExt;
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let finished = h.sessions().join(".finished");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&finished)
        .unwrap();
    let record = finished.join(format!("{}.json", NAME));
    std::fs::write(&record, br#"{"name":"coord","status":"done"}"#).unwrap();

    let out = h.import_from_a(&[("KOTO_TEST_IMPORT_FAULT", "after-exchange")]);
    assert!(!out.ok(), "{}", out.describe());
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    assert!(!record.exists(), "the re-run removed the finished record");
    assert_moved(&h);
}

#[test]
fn a_prune_in_the_source_workspace_never_removes_the_imported_session() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    let out = h.import_from_a(&[]);
    assert!(out.ok(), "{}", out.describe());
    // Finish the imported session in B, so a prune would pick it up.
    for data in [r#"{"choice":"go"}"#, r#"{"verdict":"approve"}"#] {
        let out = h.koto(
            &h.b,
            &["next", NAME, "--with-data", data, "--no-cleanup"],
            &[],
        );
        assert!(out.ok(), "{}", out.describe());
    }
    let state = std::fs::read(h.state_path(NAME)).unwrap();
    for args in [
        vec!["session", "prune", "--apply", "--all"],
        vec![
            "session",
            "prune",
            "--apply",
            "--all",
            "--execution-dir",
            h.b.to_str().unwrap(),
        ],
    ] {
        let _ = h.koto(&h.a, &args, &[]);
        assert_eq!(
            std::fs::read(h.state_path(NAME)).unwrap(),
            state,
            "{args:?} from A removed or changed the imported session"
        );
    }
}

#[test]
fn a_marker_from_another_import_is_never_taken_for_this_one() {
    let s3 = FakeS3::start(BUCKET);
    let h = source(&s3);
    // Our marker PUT fails, but another import's marker is there when the
    // run looks.
    let other = serde_json::json!({
        "target": {"workspace": "/srv/elsewhere", "session": NAME, "session_id": "someone-else"},
    });
    s3.fail("PUT", &marker_key(&h), 500);
    s3.put_after(
        "PUT",
        &marker_key(&h),
        &marker_key(&h),
        other.to_string().as_bytes(),
    );
    let out = h.import_from_a(&[]);
    assert_refused(&out, "import_source_migrated");
    assert!(out.stdout.contains("another import"), "{}", out.describe());
    // Everything this run pushed or staged was taken back.
    assert!(b_objects(&h, &s3).is_empty());
    assert!(!h.pending().exists());
}

// ===== Settling koto's own keys (DESIGN-session-retention.md, Decision 6) =====
//
// Each scenario runs on this one host, with the cloud backend configured
// per command in the environment. A scenario that needs a second host's
// store gives it a second `Host` (its own HOME and template cache) over the
// same bucket.

/// A command that must succeed, in workspace `ws`, with `env` added.
fn ok(h: &Host, ws: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let out = h.koto(ws, args, env);
    assert!(out.ok(), "koto {args:?}: {}", out.describe());
    out
}

/// Open the anchor of harness session `harness` (`anchor-<harness>`), with
/// an observed marker when `key` is given.
fn open_anchor(h: &Host, ws: &Path, harness: &str, key: Option<&str>) -> String {
    let mut args = vec![
        "session",
        "anchor",
        "open",
        "--harness-session",
        harness,
        "--source",
        "startup",
    ];
    if let Some(key) = key {
        args.extend(["--observed-key", key]);
    }
    ok(h, ws, &args, &[]);
    format!("anchor-{harness}")
}

/// Start session `name` in `ws` with `extra` arguments, `notes.md` added.
fn start(h: &Host, ws: &Path, name: &str, extra: &[&str], env: &[(&str, &str)]) {
    let mut args = vec!["init", name, "--template", TEMPLATE_FILE];
    args.extend_from_slice(extra);
    ok(h, ws, &args, env);
    let file = h.home.join(format!("notes-{name}"));
    std::fs::write(&file, b"the notes\n").unwrap();
    ok(
        h,
        ws,
        &[
            "context",
            "add",
            name,
            "notes.md",
            "--from-file",
            file.to_str().unwrap(),
        ],
        &[],
    );
}

/// Tick `name` in `ws` to its terminal, without `--no-cleanup`; returns the
/// terminal response.
fn finish(h: &Host, ws: &Path, name: &str, env: &[(&str, &str)]) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    for data in [
        None,
        Some(r#"{"choice":"go"}"#),
        Some(r#"{"verdict":"approve"}"#),
    ] {
        let mut args = vec!["next", name];
        if let Some(d) = data {
            args.extend(["--with-data", d]);
        }
        last = ok(h, ws, &args, env).json();
    }
    assert_eq!(last["action"], "done", "{last:#}");
    last
}

/// Import `name` from workspace `from` into `ws`, as `as_name` when given.
fn import(
    h: &Host,
    ws: &Path,
    name: &str,
    from: &Path,
    as_name: Option<&str>,
    env: &[(&str, &str)],
) -> Out {
    let mut args = vec!["session", "import", name, "--from", from.to_str().unwrap()];
    if let Some(n) = as_name {
        args.extend(["--as", n]);
    }
    ok(h, ws, &args, env)
}

fn session_file(h: &Host, name: &str, rel: &str) -> PathBuf {
    h.sessions().join(name).join(rel)
}

/// The keys `name`'s local manifest lists.
fn manifest_keys(h: &Host, name: &str) -> Vec<String> {
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(session_file(h, name, "ctx/manifest.json")).unwrap())
            .unwrap();
    m["keys"].as_object().unwrap().keys().cloned().collect()
}

/// The parsed body of `name`'s link `koto/links/<kind>/<peer>`, or `None`.
fn link(h: &Host, name: &str, kind: &str, peer: &str) -> Option<serde_json::Value> {
    let rel = format!("ctx/koto/links/{kind}/{peer}");
    std::fs::read(session_file(h, name, &rel))
        .ok()
        .map(|b| serde_json::from_slice(&b).unwrap())
}

/// Every object under `name`'s key prefix in workspace `ws`, with its bytes.
fn objects(s3: &FakeS3, ws: &Path, name: &str) -> Files {
    let prefix = format!("{}/{}/", prefix_of(ws), name);
    let mut out: Files = s3
        .keys_under(&prefix)
        .into_iter()
        .map(|k| {
            let v = s3.object(&k).unwrap();
            (k, v)
        })
        .collect();
    out.sort();
    out
}

/// Every file of `name`'s local directory, but the locks and the run
/// journal's sidecar.
fn local_files(h: &Host, name: &str) -> Files {
    let dir = h.sessions().join(name);
    let mut out: Files = walk(&dir)
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned();
            (!rel.ends_with(".lock") && rel != "run-journal.json")
                .then(|| (rel, std::fs::read(&p).unwrap()))
        })
        .collect();
    out.sort();
    out
}

/// `before` and `after` are the same objects but the migration marker the
/// import adds beside the source.
fn only_marked(before: &Files, after: &Files, ws: &Path, name: &str) {
    let marker = format!("{}/{}/migrated.json", prefix_of(ws), name);
    let rest: Files = after
        .iter()
        .filter(|(k, _)| *k != marker)
        .cloned()
        .collect();
    assert_eq!(&rest, before, "the source changed beyond its marker");
    assert!(
        after.iter().any(|(k, _)| *k == marker),
        "no marker on {name}"
    );
}

/// The kept entry for `name` in a prune's output, as its `detail`.
fn kept_detail(prune: &serde_json::Value, name: &str) -> String {
    prune["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {prune:#}"))["detail"]
        .as_str()
        .unwrap()
        .to_string()
}

fn removed_names(prune: &serde_json::Value) -> Vec<String> {
    prune["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.get("name")
                .and_then(serde_json::Value::as_str)
                .or_else(|| r.as_str())
                .unwrap()
                .to_string()
        })
        .collect()
}

/// The named member of a `koto session hierarchy --json` answer.
fn member(h: &serde_json::Value, name: &str) -> serde_json::Value {
    h["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} isn't a member: {h:#}"))
}

/// Milliseconds since the epoch of a UTC timestamp as koto writes them.
fn millis(ts: &str) -> i64 {
    let n = |r: std::ops::Range<usize>| ts[r].parse::<i64>().unwrap();
    let (y, m, d) = (n(0..4), n(5..7), n(8..10));
    let (hh, mm, ss) = (n(11..13), n(14..16), n(17..19));
    let ms = if ts.as_bytes().get(19) == Some(&b'.') {
        n(20..23)
    } else {
        0
    };
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000 + ms
}

fn sleep_until(at_ms: i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    if at_ms > now {
        std::thread::sleep(std::time::Duration::from_millis((at_ms - now) as u64));
    }
}

/// The timestamp of the last event in `name`'s local log, in milliseconds.
fn last_event_ms(h: &Host, name: &str) -> i64 {
    let log = std::fs::read_to_string(h.state_path(name)).unwrap();
    let line = log.lines().rfind(|l| !l.trim().is_empty()).unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    millis(v["timestamp"].as_str().unwrap())
}

/// A coordinator `coord` opened by an agent session whose anchor has since
/// closed, with a finished `b` it dispatched, moved to workspace B by
/// another agent session: the copy's held-by is rewritten to the importer's
/// anchor, the copy is kept at its terminal step, a prune names the
/// importer's anchor as why the copy and `b` are kept, the migrated source
/// keeps nothing and refuses reads, and `b`, which didn't move, is read from
/// its own workspace A and refused from B.
#[test]
fn an_import_from_an_agent_session_holds_the_moved_copy_and_its_hierarchy() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let old = open_anchor(&h, &h.a, "OLD", Some("K-OLD"));
    let under_old = [("KOTO_ANCHOR", old.as_str())];
    start(&h, &h.a, "coord", &[], &under_old);
    start(&h, &h.a, "b", &["--dispatched-by", "coord"], &under_old);
    let v = finish(&h, &h.a, "b", &under_old);
    assert_eq!(v["retention"]["retained"], true, "{v:#}");
    // The old agent session ends: its anchor reads closed by its observed
    // close.
    ok(
        &h,
        &h.a,
        &[
            "session",
            "anchor",
            "close",
            "--key=K-OLD",
            "--harness-session=OLD",
        ],
        &[],
    );
    let imp = open_anchor(&h, &h.b, "IMP", None);
    let as_importer = [("KOTO_ANCHOR", imp.as_str())];
    let source_objects = objects(&s3, &h.a, "coord");
    let source_local = local_files(&h, "coord");

    let out = import(&h, &h.b, "coord", &h.a, Some("coord-b"), &as_importer);
    assert_eq!(out.json()["name"], "coord-b");
    // The held-by link now names the importer's anchor, and says which it
    // replaced.
    assert!(link(&h, "coord-b", "held-by", &old).is_none());
    let held = link(&h, "coord-b", "held-by", &imp).expect("a held-by to the importer");
    assert_eq!(held["session"], imp.as_str());
    assert_eq!(held["was"], old.as_str());
    // The source changed only by its marker, locally and in the bucket.
    only_marked(&source_objects, &objects(&s3, &h.a, "coord"), &h.a, "coord");
    assert_eq!(local_files(&h, "coord"), source_local);

    // The copy reaches its terminal step without --no-cleanup and is kept.
    let v = finish(&h, &h.b, "coord-b", &as_importer);
    assert_eq!(v["retention"]["retained"], true, "{v:#}");
    assert_eq!(v["retention"]["reason"], "hierarchy", "{v:#}");
    assert!(h.sessions().join("coord-b").is_dir());

    // A prune names the importer's anchor as why the copy and b are kept.
    for args in [
        &["session", "prune"][..],
        &["session", "prune", "--apply", "--all"][..],
    ] {
        let out = ok(&h, &h.b, args, &[]).json();
        for name in ["coord-b", "b"] {
            let why = kept_detail(&out, name);
            assert!(why.contains(&imp), "{name}: {why}");
        }
    }
    // The migrated source keeps nothing and refuses reads.
    let hierarchy = ok(&h, &h.b, &["session", "hierarchy", "b", "--json"], &[]).json();
    let source = member(&hierarchy, "coord");
    assert_eq!(source["migrated"], true, "{hierarchy:#}");
    assert_eq!(source["alive"], false);
    let refused = h.koto(&h.a, &["context", "get", "coord", "notes.md"], &[]);
    assert_eq!(refused.code, Some(2), "{}", refused.describe());
    assert!(format!("{}{}", refused.stdout, refused.stderr).contains("session_migrated"));

    // Only coord moved: b is still bound to A, where it was started, so the
    // reviewer reads b's log and keys from there. From B, whose prefix isn't
    // b's, the read refuses rather than sync against it (koto#457).
    let out = ok(&h, &h.a, &["context", "list", "b"], &as_importer);
    assert!(out.stdout.contains("notes.md"), "{}", out.describe());
    let out = ok(&h, &h.a, &["status", "b"], &as_importer);
    assert_eq!(out.json()["is_terminal"], true, "{}", out.describe());
    let out = ok(&h, &h.a, &["context", "get", "b", "notes.md"], &as_importer);
    assert_eq!(out.raw, b"the notes\n");
    let refused = h.koto(&h.b, &["context", "get", "b", "notes.md"], &as_importer);
    assert_eq!(refused.code, Some(2), "{}", refused.describe());
    assert!(
        refused.stdout.contains("session_bound_elsewhere"),
        "{}",
        refused.describe()
    );
}

/// A held-by link whose anchor is open here is kept as it is, although the
/// anchor's own workspace is another one than the importing workspace.
#[test]
fn a_held_by_naming_an_open_anchor_here_is_kept_whatever_its_workspace() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let anchor = open_anchor(&h, &h.a, "A", None);
    start(&h, &h.a, "s", &[], &[("KOTO_ANCHOR", anchor.as_str())]);
    let body = std::fs::read(session_file(
        &h,
        "s",
        &format!("ctx/koto/links/held-by/{anchor}"),
    ))
    .unwrap();
    let header: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(h.state_path(&anchor))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(header["execution_dir"], h.a.to_str().unwrap());
    assert_ne!(h.a, h.b);

    import(&h, &h.b, "s", &h.a, Some("s-b"), &[]);
    assert_eq!(
        std::fs::read(session_file(
            &h,
            "s-b",
            &format!("ctx/koto/links/held-by/{anchor}")
        ))
        .unwrap(),
        body
    );
}

/// When the anchor a held-by link names is closed or migrated here, an
/// import from an agent session rewrites the link to that session's anchor
/// with `"was"` holding the old name; the source changes only by its mark.
#[test]
fn a_held_by_naming_a_closed_or_migrated_anchor_is_rewritten_to_the_importers() {
    // Closed: the anchor was imported first, from a bare shell, and has
    // idled out since its import.
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let x = open_anchor(&h, &h.a, "X", Some("K-X"));
    start(&h, &h.a, "s", &[], &[("KOTO_ANCHOR", x.as_str())]);
    import(&h, &h.b, &x, &h.a, None, &[]);
    let imported_at = last_event_ms(&h, &x);
    let imp = open_anchor(&h, &h.b, "IMP", None);
    sleep_until(imported_at + 1300);
    let before = objects(&s3, &h.a, "s");
    import(
        &h,
        &h.b,
        "s",
        &h.a,
        None,
        &[
            ("KOTO_ANCHOR", imp.as_str()),
            ("KOTO_RETENTION_IDLE_PERIOD", "1s"),
        ],
    );
    assert!(link(&h, "s", "held-by", &x).is_none());
    let held = link(&h, "s", "held-by", &imp).expect("rewritten to the importer");
    assert_eq!(held["was"], x.as_str());
    only_marked(&before, &objects(&s3, &h.a, "s"), &h.a, "s");

    // Migrated: the anchor was imported under another name, so the one
    // here is the migrated source.
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let x = open_anchor(&h, &h.a, "X", None);
    start(&h, &h.a, "s", &[], &[("KOTO_ANCHOR", x.as_str())]);
    import(&h, &h.b, &x, &h.a, Some("anchor-X2"), &[]);
    let imp = open_anchor(&h, &h.b, "IMP", None);
    let before = (objects(&s3, &h.a, "s"), local_files(&h, "s"));
    import(
        &h,
        &h.b,
        "s",
        &h.a,
        Some("s-b"),
        &[("KOTO_ANCHOR", imp.as_str())],
    );
    let hierarchy = ok(&h, &h.b, &["session", "hierarchy", &x, "--json"], &[]).json();
    assert_eq!(member(&hierarchy, &x)["migrated"], true, "{hierarchy:#}");
    assert!(link(&h, "s-b", "held-by", &x).is_none());
    let held = link(&h, "s-b", "held-by", &imp).expect("rewritten to the importer");
    assert_eq!(held["was"], x.as_str());
    only_marked(&before.0, &objects(&s3, &h.a, "s"), &h.a, "s");
    assert_eq!(local_files(&h, "s"), before.1);
}

/// From a bare shell on another host, a finished session's held-by link is
/// dropped (its anchor isn't in that host's store) and its dispatched-by
/// link is carried unchanged; a prune there removes it once the retention
/// from its finish has passed. Its original anchor stays open throughout.
#[test]
fn an_import_with_no_anchor_drops_the_held_by_and_the_session_is_released_later() {
    let s3 = FakeS3::start(BUCKET);
    let h1 = Host::new(&s3);
    let h2 = Host::new(&s3);
    let anchor = open_anchor(&h1, &h1.a, "O", None);
    let under = [("KOTO_ANCHOR", anchor.as_str())];
    start(&h1, &h1.a, "coord", &[], &[]);
    start(&h1, &h1.a, "b", &["--dispatched-by", "coord"], &under);
    finish(&h1, &h1.a, "b", &under);
    let dispatched =
        std::fs::read(session_file(&h1, "b", "ctx/koto/links/dispatched-by/coord")).unwrap();
    let finished_at = last_event_ms(&h1, "b");

    let template = h2.a.join(TEMPLATE_FILE);
    ok(
        &h2,
        &h2.a,
        &["template", "compile", template.to_str().unwrap()],
        &[],
    );
    import(&h2, &h2.a, "b", &h1.a, None, &[]);
    assert!(manifest_keys(&h2, "b")
        .iter()
        .all(|k| !k.starts_with("koto/links/held-by/")));
    assert!(link(&h2, "b", "held-by", &anchor).is_none());
    assert_eq!(
        std::fs::read(session_file(&h2, "b", "ctx/koto/links/dispatched-by/coord")).unwrap(),
        dispatched
    );

    // Inside the retention from its finish the prune keeps it; past it,
    // the prune removes it.
    let keep_long = [("KOTO_RETENTION_KEEP", "1h")];
    let out = ok(
        &h2,
        &h2.a,
        &["session", "prune", "--apply", "--all"],
        &keep_long,
    )
    .json();
    assert!(kept_detail(&out, "b").contains("retention"), "{out:#}");
    sleep_until(finished_at + 2300);
    let keep_short = [("KOTO_RETENTION_KEEP", "2s")];
    let out = ok(
        &h2,
        &h2.a,
        &["session", "prune", "--apply", "--all"],
        &keep_short,
    )
    .json();
    assert!(removed_names(&out).contains(&"b".to_string()), "{out:#}");
    assert!(!h2.sessions().join("b").exists());

    // The original anchor is open on its own host.
    let hierarchy = ok(
        &h1,
        &h1.a,
        &["session", "hierarchy", &anchor, "--json"],
        &[],
    )
    .json();
    assert_eq!(member(&hierarchy, &anchor)["alive"], true, "{hierarchy:#}");
}

/// An anchor imported from a bare shell, then a finished session it holds:
/// the session's held-by still names the anchor, which reads here with no
/// observed marker and its import as its last activity. Inside the idle
/// period from the import the prune keeps the session naming the anchor;
/// past it the anchor reads closed (idle); inside the retention after that
/// close the session is still kept, and past it the prune removes it,
/// naming the idle close.
#[test]
fn an_imported_anchor_holds_what_it_held_until_it_idles_out() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let anchor = open_anchor(&h, &h.a, "A", Some("K-A"));
    assert!(manifest_keys(&h, &anchor).contains(&"koto/anchor/observed".to_string()));
    let under = [("KOTO_ANCHOR", anchor.as_str())];
    start(&h, &h.a, "b", &[], &under);
    finish(&h, &h.a, "b", &under);
    let body = std::fs::read(session_file(
        &h,
        "b",
        &format!("ctx/koto/links/held-by/{anchor}"),
    ))
    .unwrap();

    import(&h, &h.b, &anchor, &h.a, None, &[]);
    import(&h, &h.b, "b", &h.a, None, &[]);
    let imported_at = {
        let log = std::fs::read_to_string(h.state_path(&anchor)).unwrap();
        let last: serde_json::Value =
            serde_json::from_str(log.lines().rfind(|l| !l.trim().is_empty()).unwrap()).unwrap();
        assert_eq!(last["type"], "session_imported", "{last}");
        millis(last["timestamp"].as_str().unwrap())
    };
    // The anchor reads here, unobserved and imported; b still names it.
    assert!(!manifest_keys(&h, &anchor).contains(&"koto/anchor/observed".to_string()));
    assert!(!session_file(&h, &anchor, "ctx/koto/anchor/observed").exists());
    assert_eq!(
        std::fs::read(session_file(
            &h,
            "b",
            &format!("ctx/koto/links/held-by/{anchor}")
        ))
        .unwrap(),
        body
    );
    let fast = [
        ("KOTO_RETENTION_IDLE_PERIOD", "3s"),
        ("KOTO_RETENTION_KEEP", "3s"),
        ("KOTO_RETENTION_STALE_AFTER", "1h"),
    ];
    let hierarchy = ok(&h, &h.b, &["session", "hierarchy", "b", "--json"], &fast).json();
    let a = member(&hierarchy, &anchor);
    assert_eq!(a["alive"], true, "{hierarchy:#}");
    assert_eq!(a["observed"], false);
    assert_eq!(a["imported"], true);

    // Inside the idle period: kept, naming the anchor.
    let out = ok(&h, &h.b, &["session", "prune", "--apply", "--all"], &fast).json();
    assert!(kept_detail(&out, "b").contains(&anchor), "{out:#}");

    // Past it: the anchor reads closed, idle, from its import; b is kept
    // through the retention after that close.
    sleep_until(imported_at + 3400);
    let hierarchy = ok(&h, &h.b, &["session", "hierarchy", "b", "--json"], &fast).json();
    let a = member(&hierarchy, &anchor);
    assert_eq!(a["alive"], false, "{hierarchy:#}");
    assert_eq!(a["close_kind"], "idle");
    assert_eq!(
        millis(a["stopped_at"].as_str().unwrap()),
        imported_at + 3000
    );
    let out = ok(&h, &h.b, &["session", "prune", "--apply", "--all"], &fast).json();
    let why = kept_detail(&out, "b");
    assert!(why.contains("idle"), "{why}");

    // Past the retention: removed, naming the idle close.
    sleep_until(imported_at + 6400);
    let hierarchy = ok(&h, &h.b, &["session", "hierarchy", "b", "--json"], &fast).json();
    assert_eq!(hierarchy["kept"], false, "{hierarchy:#}");
    assert_eq!(hierarchy["retention"]["release"]["member"], anchor.as_str());
    assert_eq!(hierarchy["retention"]["release"]["close_kind"], "idle");
    let out = ok(&h, &h.b, &["session", "prune", "--apply", "--all"], &fast).json();
    assert!(removed_names(&out).contains(&"b".to_string()), "{out:#}");
    let removed_anchor = out["removed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == anchor.as_str())
        .unwrap_or_else(|| panic!("the anchor wasn't removed with b: {out:#}"));
    assert_eq!(
        millis(removed_anchor["stopped_at"].as_str().unwrap()),
        imported_at + 3000,
        "the prune names the idle close as the anchor's stop"
    );
    assert!(!h.sessions().join("b").exists());
}

/// A bucket that can't be trusted for the reserved area: a link whose body
/// names another session than its key, an observed marker, a close link and
/// a forged imported-from record, each listed in the manifest with its
/// right hash. The import drops all four, warns about the malformed link,
/// keeps the well-formed one, and writes its own imported-from.
#[test]
fn an_import_drops_the_reserved_keys_a_bucket_must_not_carry() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    start(&h, &h.a, "s", &[], &[]);
    let prefix = format!("{}/s/", prefix_of(&h.a));
    let manifest_key = format!("{prefix}ctx/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&s3.object(&manifest_key).unwrap()).unwrap();
    let body = |peer: &str| {
        format!(
            r#"{{"v":1,"session":"{peer}","session_id":null,"workspace":null,"at":"2026-01-01T00:00:00.000Z"}}"#
        )
        .into_bytes()
    };
    let planted: Vec<(&str, Vec<u8>)> = vec![
        ("koto/links/dispatched-by/good", body("good")),
        ("koto/links/dispatched-by/evil", body("someone-else")),
        (
            "koto/anchor/observed",
            br#"{"v":1,"key":"K","at":"2026-01-01T00:00:00.000Z"}"#.to_vec(),
        ),
        ("koto/links/closes/anchor-x", body("anchor-x")),
        (
            "koto/imported-from",
            br#"{"from_session":"forged","from_session_id":"forged-id"}"#.to_vec(),
        ),
    ];
    for (key, bytes) in &planted {
        s3.put(&format!("{prefix}ctx/{key}"), bytes);
        manifest["keys"][key] = serde_json::json!({
            "created_at": "2026-01-01T00:00:00.000Z",
            "size": bytes.len(),
            "hash": migration_carrier::sha256_hex(bytes),
            "writer": "koto",
        });
    }
    s3.put(&manifest_key, &serde_json::to_vec(&manifest).unwrap());
    let source_id = {
        let log =
            String::from_utf8(s3.object(&format!("{prefix}koto-s.state.jsonl")).unwrap()).unwrap();
        let header: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        header["session_id"].as_str().unwrap().to_string()
    };

    let out = import(&h, &h.b, "s", &h.a, Some("s-b"), &[]);
    assert!(
        out.stderr.contains("koto/links/dispatched-by/evil"),
        "the malformed link isn't reported: {}",
        out.describe()
    );
    assert_eq!(out.json()["keys"], 2, "notes.md and the good link");
    assert_eq!(
        manifest_keys(&h, "s-b"),
        [
            "koto/imported-from",
            "koto/links/dispatched-by/good",
            "notes.md"
        ]
    );
    for gone in [
        "koto/links/dispatched-by/evil",
        "koto/anchor/observed",
        "koto/links/closes/anchor-x",
    ] {
        assert!(
            !session_file(&h, "s-b", &format!("ctx/{gone}")).exists(),
            "{gone}"
        );
    }
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(session_file(&h, "s-b", "ctx/koto/imported-from")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record,
        serde_json::json!({"from_session": "s", "from_session_id": source_id})
    );
    // The pushed manifest is the copy's, so the bucket copy verifies.
    let pushed = s3
        .object(&format!("{}/s-b/ctx/manifest.json", prefix_of(&h.b)))
        .unwrap();
    assert_eq!(
        pushed,
        std::fs::read(session_file(&h, "s-b", "ctx/manifest.json")).unwrap()
    );
    let pushed: serde_json::Value = serde_json::from_slice(&pushed).unwrap();
    for (key, meta) in pushed["keys"].as_object().unwrap() {
        let bytes = s3
            .object(&format!("{}/s-b/ctx/{key}", prefix_of(&h.b)))
            .unwrap_or_else(|| panic!("{key} wasn't pushed"));
        assert_eq!(meta["hash"], migration_carrier::sha256_hex(&bytes), "{key}");
    }
}

/// An anchor's close session is never a source: the import refuses it
/// before anything is written.
#[test]
fn an_import_refuses_an_anchor_close_session() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    start(&h, &h.a, "s", &[], &[]);
    let key = format!("{}/s/koto-s.state.jsonl", prefix_of(&h.a));
    let log = String::from_utf8(s3.object(&key).unwrap()).unwrap();
    let mut lines: Vec<String> = log.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header["kind"] = serde_json::Value::from("anchor-close");
    lines[0] = header.to_string();
    s3.put(&key, (lines.join("\n") + "\n").as_bytes());
    let before = objects(&s3, &h.a, "s");

    let out = h.koto(
        &h.b,
        &[
            "session",
            "import",
            "s",
            "--from",
            h.a.to_str().unwrap(),
            "--as",
            "s-b",
        ],
        &[],
    );
    assert_refused(&out, "import_source_is_anchor_close");
    assert!(out.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("anchor-close"));
    assert_eq!(objects(&s3, &h.a, "s"), before);
    assert!(s3
        .keys_under(&format!("{}/s-b/", prefix_of(&h.b)))
        .is_empty());
    assert!(!h.sessions().join("s-b").exists());
}
