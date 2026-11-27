//! Integration tests for the wake an answer rings: `koto decisions answer`
//! appends one opaque line to `<koto home>/wakes/<raiser>` (and to the
//! newest session continuing a superseded raiser), which `koto request
//! watch` notices. A wake carries nothing: the asker's next tick reads the
//! store, so the answer is found whatever state the wake file is in, a
//! hand-written line is never taken as an answer, and a ring that fails
//! never fails the answer.
//!
//! Every test runs the real binary with `HOME` and `KOTO_HOME` pointed
//! into its own temporary directory and every variable koto reads an
//! identity or a session backend from removed. The wake files are under
//! `home/wakes/`.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

use assert_fs::TempDir;
use serde_json::Value;

/// Variables koto reads an identity, a run mode, an authority or a session
/// backend from.
const CLEARED_ENV: [&str; 9] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_ID",
    "KOTO_SESSION_NAME",
];

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
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

## done

Done.
"#;

/// The warning an answer prints when its ring fails.
const RING_WARNING: &str = "warning: could not deliver a wake to";

struct Env {
    _tmp: TempDir,
    root: PathBuf,
    /// Where sessions live: `KOTO_SESSIONS_BASE` when the env sets it, else
    /// the koto root's own `sessions/`.
    sessions: PathBuf,
    sessions_base: bool,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

/// A recorded escalation: its session, id and the session's id.
#[derive(Clone)]
struct Rec {
    session: String,
    id: String,
    session_id: String,
}

impl Rec {
    fn identity(&self) -> String {
        format!("{}/{}", self.session_id, self.id)
    }
}

impl Env {
    /// The layout `tests/decisions_question_test.rs` uses: sessions under
    /// `KOTO_SESSIONS_BASE`.
    fn new() -> Self {
        Self::build(true)
    }

    /// Sessions under the koto root (`KOTO_HOME/sessions`), with
    /// `KOTO_SESSIONS_BASE` removed. The ring finds the newest member of a
    /// superseded raiser's chain in the koto root's sessions directory, as
    /// `tests/supersede_ring_test.rs` does for a request principal.
    fn at_root() -> Self {
        Self::build(false)
    }

    fn build(sessions_base: bool) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        let sessions = if sessions_base {
            root.join("sessions")
        } else {
            root.join("home/sessions")
        };
        Env {
            _tmp: tmp,
            root,
            sessions,
            sessions_base,
        }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn wakes(&self) -> PathBuf {
        self.home().join("wakes")
    }

    fn wake_file(&self, name: &str) -> PathBuf {
        self.wakes().join(name)
    }

    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.home());
        cmd.env("KOTO_HOME", self.home());
        if self.sessions_base {
            cmd.env("KOTO_SESSIONS_BASE", &self.sessions);
        } else {
            cmd.env_remove("KOTO_SESSIONS_BASE");
        }
        cmd.args(args);
        cmd
    }

    fn run(&self, args: &[&str]) -> Out {
        let output = self.command(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn escalate_with_text(&self, name: &str, question: &str, text: &str, recipient: &str) -> Rec {
        let q = format!("question={question}");
        let t = format!("text={text}");
        let r = format!("recipient={recipient}");
        let out = self
            .ok(&[
                "decisions",
                "record",
                name,
                "--kind",
                "escalate",
                "--outcome",
                "escalated",
                "--subject",
                &q,
                "--subject",
                &t,
                "--subject",
                &r,
                "--by",
                "session",
            ])
            .json();
        Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        }
    }

    fn escalate(&self, name: &str, question: &str, recipient: &str) -> Rec {
        self.escalate_with_text(
            name,
            question,
            &format!("what about {question}?"),
            recipient,
        )
    }

    fn answer_run(&self, q: &Rec, in_session: &str, text: &str) -> Out {
        self.run(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--text",
            text,
            "--by",
            "session",
        ])
    }

    /// Answer `q` from `in_session`; returns the answer's id.
    fn answer(&self, q: &Rec, in_session: &str, text: &str) -> String {
        let out = self.answer_run(q, in_session, text);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        out.json()["id"].as_str().unwrap().to_string()
    }

    fn open(&self, person: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--person", person]).json();
        out["questions"].as_array().unwrap().clone()
    }

    /// The open questions `koto decisions open --at <position>` lists.
    fn at(&self, position: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--at", position]).json();
        out["questions"].as_array().unwrap().clone()
    }

    /// `koto next <name> --no-cleanup`, so a terminal session stays.
    fn next(&self, name: &str) -> Value {
        self.ok(&["next", name, "--no-cleanup"]).json()
    }

    /// `koto request watch` on `session`; returns `(woke, cursor)`.
    fn watch(&self, session: &str, timeout: &str, since: Option<&str>) -> (bool, String) {
        let mut args = vec![
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            timeout,
        ];
        if let Some(c) = since {
            args.extend_from_slice(&["--since", c]);
        }
        let v = self.ok(&args).json();
        (
            v["woke"].as_bool().unwrap(),
            v["cursor"].as_str().unwrap().to_string(),
        )
    }

    fn wake_lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.wake_file(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The modification time and size of every entry under `home/wakes/`,
    /// symlinks not followed.
    fn wakes_state(&self) -> BTreeMap<PathBuf, (SystemTime, u64)> {
        let mut out = BTreeMap::new();
        let Ok(rd) = std::fs::read_dir(self.wakes()) else {
            return out;
        };
        for entry in rd {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.insert(path, (meta.modified().unwrap(), meta.len()));
        }
        out
    }

    /// Every file under a session's directory with its bytes, lock files
    /// aside.
    fn snapshot(&self, name: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_tree(&self.sessions.join(name), &[])
    }

    /// The answer `answer_id` recorded in `answerer` for a question of the
    /// session whose id is `raiser_id` is stored where the open reads find
    /// it.
    fn answer_stored(&self, answerer: &str, raiser_id: &str, answer_id: &str) -> bool {
        self.sessions
            .join(answerer)
            .join("ctx/koto/aimed")
            .join(raiser_id)
            .join(answer_id)
            .is_file()
    }
}

/// Every regular file under `dir` with its bytes, lock files and anything
/// under `skip` aside. Symlinks are recorded by their target and other
/// special files by their type, never opened.
fn snapshot_tree(dir: &Path, skip: &[PathBuf]) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, base: &Path, skip: &[PathBuf], out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if skip.iter().any(|s| path.starts_with(s)) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                out.insert(rel, target.to_string_lossy().into_owned().into_bytes());
            } else if meta.is_dir() {
                walk(&path, base, skip, out);
            } else if !meta.is_file() {
                // A FIFO or other special file: opening it could block.
                out.insert(rel, format!("special {:?}", meta.file_type()).into_bytes());
            } else if !path.to_string_lossy().ends_with(".lock") {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, skip, &mut out);
    out
}

fn questions(next: &Value) -> Vec<Value> {
    next.get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn identities(qs: &[Value]) -> Vec<String> {
    qs.iter()
        .map(|q| q["identity"].as_str().unwrap().to_string())
        .collect()
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn an_answer_appends_one_line_and_wakes_a_watch() {
    let env = Env::new();
    env.init("asker", &[]);
    env.init("helper", &[]);
    let q1 = env.escalate("asker", "q1", "session:helper");
    let q2 = env.escalate("asker", "q2", "session:helper");
    assert!(!env.wake_file("asker").exists());

    let (woke, before) = env.watch("asker", "0", None);
    assert!(!woke);

    // A watch armed before the answer, from the same cursor.
    let mut child = env
        .command(&[
            "request",
            "watch",
            "--session",
            "asker",
            "--timeout-secs",
            "10",
            "--since",
            &before,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the watch exited before any answer"
    );

    env.answer(&q1, "helper", "yes");
    let returned = Instant::now();

    // The file was created with exactly one line by the time the answer
    // returned.
    assert_eq!(env.wake_lines("asker").len(), 1);

    let exited_after = loop {
        if child.try_wait().unwrap().is_some() {
            break returned.elapsed();
        }
        if returned.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("the armed watch didn't exit after the answer");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["woke"], true, "{v}");
    assert!(
        exited_after < Duration::from_secs(1),
        "the armed watch took {exited_after:?} after the answer returned"
    );

    // A watch started after the answer, from the cursor taken before it.
    let (woke, after) = env.watch("asker", "5", Some(&before));
    assert!(woke);
    assert_ne!(after, before);

    // Each answer appends exactly one line.
    env.answer(&q2, "helper", "no");
    assert_eq!(env.wake_lines("asker").len(), 2);
    let (woke, _) = env.watch("asker", "5", Some(&after));
    assert!(woke);
}

#[test]
fn an_answer_writes_nothing_in_the_asker_and_its_wake_carries_nothing() {
    let env = Env::new();
    env.init("asker", &[]);
    env.init("zed-answerer", &[]);
    let q = env.escalate_with_text(
        "asker",
        "storage-engine",
        "which storage engine do we pick",
        "session:zed-answerer",
    );
    // The answering session's own directory may change; nothing else but
    // the asker's wake file may.
    let skip = [env.sessions.join("zed-answerer")];
    let before_all = snapshot_tree(&env.root, &skip);
    let before = env.snapshot("asker");

    let answer_id = env.answer(&q, "zed-answerer", "PostgreSQL-with-WAL");

    assert_eq!(env.snapshot("asker"), before, "the asker's session changed");
    let after_all = snapshot_tree(&env.root, &skip);
    let changed: BTreeSet<PathBuf> = after_all
        .keys()
        .chain(before_all.keys())
        .filter(|k| before_all.get(*k) != after_all.get(*k))
        .cloned()
        .collect();
    assert_eq!(
        changed,
        BTreeSet::from([PathBuf::from("home/wakes/asker")]),
        "paths outside the answering session that changed"
    );

    let wake = std::fs::read_to_string(env.wake_file("asker")).unwrap();
    assert_eq!(wake.lines().count(), 1);
    for secret in [
        "PostgreSQL-with-WAL",
        "storage-engine",
        "which storage engine do we pick",
        q.identity().as_str(),
        q.session_id.as_str(),
        "zed-answerer",
        answer_id.as_str(),
    ] {
        assert!(
            !wake.contains(secret),
            "the wake line {wake:?} carries {secret:?}"
        );
    }
}

#[test]
fn an_answer_to_a_superseded_raiser_rings_its_newest_successor_too() {
    let env = Env::at_root();
    env.init("raiser", &[]);
    env.init("helper", &[]);
    let q = env.escalate("raiser", "q1", "session:helper");
    env.init("succ", &["--continues", "raiser", "--supersede"]);
    let (raiser0, succ0) = (env.wake_lines("raiser"), env.wake_lines("succ"));
    let bystander0 = env.wake_lines("helper");

    env.answer(&q, "helper", "yes");

    assert_eq!(env.wake_lines("raiser").len(), raiser0.len() + 1);
    assert_eq!(env.wake_lines("succ").len(), succ0.len() + 1);
    assert_eq!(env.wake_lines("helper"), bystander0);
}

#[test]
fn a_refused_answer_a_readdress_and_a_record_ring_nothing() {
    let env = Env::new();
    env.init("coord", &[]);
    env.init("w", &["--dispatched-by", "coord"]);
    env.init("helper", &[]);
    let q1 = env.escalate("w", "q1", "session:helper");
    let q2 = env.escalate("w", "q2", "session:coord");
    let q3 = env.escalate("helper", "q3", "session:coord");
    // Give the wakes directory files to watch: one ring for `w`, one for
    // `helper`.
    env.answer(&q1, "helper", "yes");
    env.answer(&q3, "coord", "yes");
    let before = env.wakes_state();
    assert_eq!(before.len(), 2, "{before:?}");
    // Coarse filesystem clocks: let any later write land on a later tick.
    std::thread::sleep(Duration::from_millis(50));

    // Refused: an id the raiser doesn't hold.
    let missing = Rec {
        id: "000000000099".to_string(),
        ..q2.clone()
    };
    let out = env.answer_run(&missing, "helper", "x");
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.wakes_state(), before, "a refused answer rang");

    // Refused: answering from the raiser itself.
    let out = env.answer_run(&q2, "w", "x");
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.wakes_state(), before, "a refused answer rang");

    // An accepted pass-up.
    env.ok(&[
        "decisions",
        "readdress",
        "w",
        &q2.id,
        "--in",
        "coord",
        "--to",
        "person:owner",
        "--by",
        "session",
    ]);
    assert_eq!(env.wakes_state(), before, "a readdress rang");

    // Plain records, a question among them.
    env.ok(&[
        "decisions",
        "record",
        "w",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=x",
        "--by",
        "session",
    ]);
    env.escalate("helper", "q4", "session:w");
    assert_eq!(env.wakes_state(), before, "a decisions record rang");

    // The raiser withdrawing one question and settling another: accepted,
    // and neither rings.
    let q5 = env.escalate("w", "q5", "session:coord");
    let q6 = env.escalate("w", "q6", "session:coord");
    let bytes_before = snapshot_tree(&env.wakes(), &[]);
    env.ok(&[
        "decisions",
        "answer",
        "w",
        &q5.id,
        "--in",
        "w",
        "--outcome",
        "withdrawn",
        "--reason",
        "not needed",
        "--by",
        "session",
    ]);
    env.ok(&[
        "decisions",
        "answer",
        "w",
        &q6.id,
        "--in",
        "w",
        "--outcome",
        "settled",
        "--text",
        "settled",
        "--evidence",
        "event:w:1",
        "--by",
        "session",
    ]);
    let closed = identities(&env.at("w"));
    assert!(
        !closed.contains(&q5.identity()) && !closed.contains(&q6.identity()),
        "{closed:?}"
    );
    assert_eq!(env.wakes_state(), before, "a withdrawal or a settle rang");
    assert_eq!(snapshot_tree(&env.wakes(), &[]), bytes_before);

    // The pass-up stood: q2 now waits on the owner.
    let open = env.open("owner");
    assert!(identities(&open).contains(&q2.identity()), "{open:?}");
}

/// Answer `q` from `answerer` with a ring that is expected to fail: exits
/// 0, warns, and stores the answer.
fn answer_despite_failed_ring(env: &Env, q: &Rec, answerer: &str) {
    let out = env.answer_run(q, answerer, "answered anyway");
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(out.stderr.contains(RING_WARNING), "{}", out.stderr);
    let id = out.json()["id"].as_str().unwrap().to_string();
    assert!(
        env.answer_stored(answerer, &q.session_id, &id),
        "the answer to {} wasn't stored",
        q.session
    );
    assert!(
        !identities(&env.at(&q.session)).contains(&q.identity()),
        "{} is still open",
        q.session
    );
    let reported = questions(&env.next(&q.session));
    assert_eq!(identities(&reported), vec![q.identity()]);
    assert_eq!(reported[0]["answer"]["text"], "answered anyway");
}

#[test]
fn a_ring_that_fails_never_fails_the_answer() {
    let env = Env::new();
    env.init("helper", &[]);
    let mut qs = BTreeMap::new();
    for name in ["dir-at-path", "fifo-at-path", "link-at-path", "unwritable"] {
        env.init(name, &[]);
        qs.insert(name, env.escalate(name, "q1", "session:helper"));
    }
    std::fs::create_dir_all(env.wakes()).unwrap();
    std::fs::set_permissions(env.wakes(), std::fs::Permissions::from_mode(0o700)).unwrap();

    // A directory at the wake path, holding a file.
    let dir = env.wake_file("dir-at-path");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("keep"), b"kept").unwrap();
    answer_despite_failed_ring(&env, &qs["dir-at-path"], "helper");
    assert!(std::fs::symlink_metadata(&dir).unwrap().is_dir());
    assert_eq!(snapshot_tree(&dir, &[]).len(), 1);
    assert_eq!(std::fs::read(dir.join("keep")).unwrap(), b"kept");

    // A FIFO at the wake path.
    let fifo = env.wake_file("fifo-at-path");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let fifo_before = std::fs::symlink_metadata(&fifo).unwrap();
    answer_despite_failed_ring(&env, &qs["fifo-at-path"], "helper");
    let fifo_after = std::fs::symlink_metadata(&fifo).unwrap();
    assert!(fifo_after.file_type().is_fifo());
    assert_eq!(
        fifo_after.modified().unwrap(),
        fifo_before.modified().unwrap()
    );

    // A symlink at the wake path, to a file outside the wakes directory.
    let target = env.root.join("link-target");
    std::fs::write(&target, b"target bytes\n").unwrap();
    let link = env.wake_file("link-at-path");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    answer_despite_failed_ring(&env, &qs["link-at-path"], "helper");
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), b"target bytes\n");

    // An unwritable wakes directory. Root writes through any mode, so the
    // case proves nothing there and is skipped.
    if !is_root() {
        let listing = snapshot_tree(&env.wakes(), &[]);
        std::fs::set_permissions(env.wakes(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let out = env.answer_run(&qs["unwritable"], "helper", "answered anyway");
        // Restore before asserting, so the temporary directory can go.
        std::fs::set_permissions(env.wakes(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        assert!(out.stderr.contains(RING_WARNING), "{}", out.stderr);
        let id = out.json()["id"].as_str().unwrap().to_string();
        assert!(env.answer_stored("helper", &qs["unwritable"].session_id, &id));
        assert!(!env.wake_file("unwritable").exists());
        assert_eq!(snapshot_tree(&env.wakes(), &[]), listing);
        let reported = questions(&env.next("unwritable"));
        assert_eq!(identities(&reported), vec![qs["unwritable"].identity()]);
    }
}

#[test]
fn the_answer_is_found_whatever_state_the_wake_file_is_in() {
    if is_root() {
        // The never-written case needs a ring that fails on permissions.
        return;
    }
    let env = Env::new();
    env.init("asker", &[]);
    env.init("first", &[]);
    env.init("second", &[]);
    let q = env.escalate("asker", "q1", "session:first");
    // A question is answered once, by its addressee: the later answer that
    // rings is `second`'s, to a second question.
    let q2 = env.escalate("asker", "q2", "session:second");
    let first_of = |next: Value| -> Vec<Value> {
        questions(&next)
            .into_iter()
            .filter(|e| e["identity"] == q.identity().as_str())
            .collect()
    };

    // Never written: the first answer's ring fails.
    std::fs::create_dir_all(env.wakes()).unwrap();
    std::fs::set_permissions(env.wakes(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = env.answer_run(&q, "first", "the answer");
    std::fs::set_permissions(env.wakes(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(!env.wake_file("asker").exists());
    let never = questions(&env.next("asker"));
    assert_eq!(identities(&never), vec![q.identity()]);
    assert_eq!(never[0]["answer"]["text"], "the answer");

    // Intact: a later answer rings, and the earlier one stays in effect.
    env.answer(&q2, "second", "a later answer");
    assert_eq!(env.wake_lines("asker").len(), 1);
    let intact = first_of(env.next("asker"));
    assert_eq!(intact, never);

    // Deleted.
    std::fs::remove_file(env.wake_file("asker")).unwrap();
    assert_eq!(first_of(env.next("asker")), intact);

    // Replaced by 64 KiB of arbitrary bytes, newlines and NULs included.
    let mut junk = Vec::with_capacity(64 * 1024);
    let mut x: u32 = 0x2545_f491;
    while junk.len() < 64 * 1024 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        junk.push((x & 0xff) as u8);
    }
    std::fs::write(env.wake_file("asker"), &junk).unwrap();
    assert_eq!(first_of(env.next("asker")), intact);
    assert_eq!(std::fs::read(env.wake_file("asker")).unwrap(), junk);
}

#[test]
fn a_hand_written_wake_is_never_taken_as_an_answer() {
    let env = Env::new();
    env.init("asker", &[]);
    let q = env.escalate("asker", "q1", "person:owner");
    let before = env.next("asker");
    assert!(before.get("questions").is_none(), "{before}");

    std::fs::create_dir_all(env.wakes()).unwrap();
    let mut line = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(env.wake_file("asker"))
        .unwrap();
    std::io::Write::write_all(&mut line, b"1795713895233960250.4242.0\n").unwrap();
    drop(line);

    let after = env.next("asker");
    assert_eq!(after, before);
    assert!(after.get("questions").is_none());
    assert_eq!(identities(&env.open("owner")), vec![q.identity()]);
}

#[test]
fn the_wake_file_stays_bounded_across_two_thousand_answers() {
    const ANSWERS: usize = 2_000;
    const THREADS: usize = 8;
    // Answers are spread over several answering sessions, since each
    // answer reads its own session's decisions to allocate an id.
    const ANSWERERS_PER_THREAD: usize = 5;

    let env = Env::new();
    env.init("asker", &[]);
    let answerers: Vec<Vec<String>> = (0..THREADS)
        .map(|t| {
            (0..ANSWERERS_PER_THREAD)
                .map(|a| format!("ans-{t}-{a}"))
                .collect()
        })
        .collect();
    for name in answerers.iter().flatten() {
        env.init(name, &[]);
    }

    // All but the last answer, from several threads at once. A question is
    // answered once, by the session it waits on, so each answer closes a
    // question of its own, raised in the asker just before it.
    let per_thread = (ANSWERS - 1).div_ceil(THREADS);
    std::thread::scope(|s| {
        for (t, names) in answerers.iter().enumerate() {
            let env = &env;
            s.spawn(move || {
                let start = t * per_thread;
                let end = ((t + 1) * per_thread).min(ANSWERS - 1);
                for i in start..end {
                    let to = &names[i % names.len()];
                    let q = env.escalate("asker", &format!("q{i}"), &format!("session:{to}"));
                    env.answer(&q, to, &format!("answer {i}"));
                }
            });
        }
    });

    let last = &answerers[0][0];
    let q = env.escalate("asker", "q-last", &format!("session:{last}"));
    let (_, cursor) = env.watch("asker", "0", None);
    env.answer(&q, last, "the last answer");

    let len = std::fs::metadata(env.wake_file("asker")).unwrap().len();
    assert!(len < 64 * 1024, "the wake file grew to {len} bytes");
    // The file was truncated along the way: it holds fewer lines than
    // there were answers.
    assert!(env.wake_lines("asker").len() < ANSWERS);
    let (woke, _) = env.watch("asker", "5", Some(&cursor));
    assert!(woke, "a cursor from before the last answer didn't wake");
}
