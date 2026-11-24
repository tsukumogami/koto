//! The session store read surface as a program other than koto sees it:
//! `koto_session_reader`'s public items, on stores built at run time.
//!
//! The layout marker: `check_layout` and `LocalDirSource::new` accept a
//! store with no marker or one naming layout 1, and refuse one naming a
//! newer layout or a marker that can't be read as a number (a FIFO, a
//! symlink, a directory, an over-cap file, text), each with a
//! `LayoutError` naming the marker's path.
//!
//! One session's log and one context value, on a store the koto binary
//! builds: `read_session_events` reads the log, and `read_context` gives
//! each of its four outcomes, answering what `koto context get` prints
//! wherever it answers a value. Every `koto context get` runs after the
//! `read_context` calls it is compared with, since it records the read. A
//! hundred reads leave the session directory exactly as it was, where one
//! `koto context get` adds a line to its read spool. A reason holds a
//! fixed category and a path, never bytes planted in the file it refused.

#![cfg(unix)]

#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use koto_session_reader::engine::types::Event;
use koto_session_reader::source::{
    check_layout, read_context, read_session_events, ContextRead, ContextSource, LayoutError,
    LocalDirSource, SessionReadCategory, SessionReadError, CONTEXT_VALUE_READ_CAP, KOTO_VERSION,
    LAYOUT_MARKER_READ_CAP, STORE_LAYOUT_MARKER, STORE_LAYOUT_VERSION,
};
use tempfile::TempDir;

/// How long one marker check may take before the test calls it blocked.
const DEADLINE: Duration = Duration::from_secs(10);

/// The golden store in `<tmp>/sessions`.
fn store() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let sessions = tmp.path().join("sessions");
    golden_store::build(&sessions, SystemTime::now());
    (tmp, sessions)
}

fn marker(sessions: &Path) -> PathBuf {
    sessions.join(STORE_LAYOUT_MARKER)
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// `check_layout` and `LocalDirSource::new` on `sessions`, on a thread,
/// failing the test if they haven't returned within [`DEADLINE`]. Both must
/// give the same answer.
fn layout_of(sessions: &Path) -> Result<(), LayoutError> {
    let (tx, rx) = mpsc::channel();
    let dir = sessions.to_path_buf();
    std::thread::spawn(move || {
        let checked = check_layout(&dir);
        let opened = LocalDirSource::new(&dir).map(|_| ());
        let _ = tx.send((checked, opened));
    });
    let (checked, opened) = rx.recv_timeout(DEADLINE).expect("the marker check blocked");
    assert_eq!(
        checked, opened,
        "check_layout and LocalDirSource::new disagree"
    );
    checked
}

/// Checks that `sessions` is refused as unusable with `category`, the
/// message naming the marker's path.
fn assert_unusable(sessions: &Path, category: &'static str) {
    let path = marker(sessions);
    let err = layout_of(sessions).expect_err("the store was accepted");
    assert_eq!(
        err,
        LayoutError::Unusable {
            path: path.clone(),
            category,
        }
    );
    assert_eq!(
        err.to_string(),
        format!(
            "session store layout marker {} is unusable: {category}",
            path.display()
        )
    );
}

#[test]
fn a_store_with_no_marker_is_layout_1() {
    let (_tmp, sessions) = store();
    assert_eq!(layout_of(&sessions), Ok(()));
}

#[test]
fn a_sessions_directory_that_does_not_exist_is_layout_1() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(layout_of(&tmp.path().join("missing")), Ok(()));
}

#[test]
fn a_marker_naming_layout_1_is_accepted() {
    let (_tmp, sessions) = store();
    for text in ["1", "1\n", " 1 \n"] {
        std::fs::write(marker(&sessions), text).unwrap();
        assert_eq!(layout_of(&sessions), Ok(()), "{text:?}");
    }
}

#[test]
fn a_marker_of_exactly_the_cap_is_read() {
    let (_tmp, sessions) = store();
    let text = format!("1{}", " ".repeat(LAYOUT_MARKER_READ_CAP as usize - 1));
    assert_eq!(text.len() as u64, LAYOUT_MARKER_READ_CAP);
    std::fs::write(marker(&sessions), text).unwrap();
    assert_eq!(layout_of(&sessions), Ok(()));
}

#[test]
fn a_marker_naming_layout_2_is_refused_naming_both_numbers() {
    let (_tmp, sessions) = store();
    std::fs::write(marker(&sessions), "2\n").unwrap();
    let err = layout_of(&sessions).expect_err("a layout 2 store was accepted");
    assert_eq!(
        err,
        LayoutError::Newer {
            path: marker(&sessions),
            found: 2,
            supported: STORE_LAYOUT_VERSION,
        }
    );
    assert_eq!(STORE_LAYOUT_VERSION, 1);
    assert_eq!(
        err.to_string(),
        format!(
            "session store layout marker {} says layout 2; this reader supports layout 1",
            marker(&sessions).display()
        )
    );
}

#[test]
fn a_fifo_marker_is_refused_without_blocking() {
    let (_tmp, sessions) = store();
    mkfifo(&marker(&sessions));
    assert_unusable(&sessions, "not a regular file");
}

#[test]
fn a_symlinked_marker_is_refused() {
    let (tmp, sessions) = store();
    let target = tmp.path().join("layout-1");
    std::fs::write(&target, "1\n").unwrap();
    std::os::unix::fs::symlink(&target, marker(&sessions)).unwrap();
    assert_unusable(&sessions, "a symlink");
}

#[test]
fn a_directory_marker_is_refused() {
    let (_tmp, sessions) = store();
    std::fs::create_dir(marker(&sessions)).unwrap();
    assert_unusable(&sessions, "not a regular file");
}

#[test]
fn a_marker_over_64_bytes_is_refused() {
    let (_tmp, sessions) = store();
    // A number, so only the size refuses it.
    let text = format!("1{}", " ".repeat(LAYOUT_MARKER_READ_CAP as usize));
    assert_eq!(text.len(), 65);
    std::fs::write(marker(&sessions), text).unwrap();
    assert_unusable(&sessions, "larger than 64 bytes");
}

#[test]
fn a_marker_that_is_not_a_number_is_refused() {
    let (_tmp, sessions) = store();
    std::fs::write(marker(&sessions), "abc\n").unwrap();
    assert_unusable(&sessions, "not a layout number");
}

// ===== One session's log and one context value, on a store koto builds =====

/// A template whose first transition assigns [`ASSIGNED_KEY`].
const TEMPLATE: &str = r#"---
name: reader-surface
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: middle
        when:
          step: go
        context_assignments:
          assigned: from-transition
  middle:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: end
        when:
          step: go
  end:
    terminal: true
---

## start

Start.

## middle

Middle.

## end

End.
"#;

/// The session every [`KotoStore`] builds.
const SESSION: &str = "wf";
/// The key the template's transition assigns, and the value it assigns.
const ASSIGNED_KEY: &str = "assigned";
const ASSIGNED_VALUE: &str = "from-transition";
/// A hierarchical key `koto context add` stores.
const NESTED_KEY: &str = "notes/plan";
const NESTED_VALUE: &str = "the plan\nin two lines\n";
/// A flat key `koto context add` stores.
const PLAIN_KEY: &str = "plain";
const PLAIN_VALUE: &str = "a plain value";
/// The host-local key `koto workflows publish` stores.
const PUBLISH_KEY: &str = "workflows/publish-location";

/// The string the reason tests plant in the files they make unreadable.
const PLANTED: &str = "ZQX-PLANTED-7F3A";

/// A scratch directory holding a sessions directory that the koto binary
/// wrote: session [`SESSION`] past the assigning transition, with
/// [`NESTED_KEY`] and [`PLAIN_KEY`] added and a `/workflows` location
/// published.
struct KotoStore {
    tmp: TempDir,
}

impl KotoStore {
    fn build() -> Self {
        let store = KotoStore {
            tmp: TempDir::new().unwrap(),
        };
        std::fs::create_dir_all(store.sessions()).unwrap();
        let template = store.dir().join("reader-surface.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        store.koto_ok(&["init", SESSION, "--template", template.to_str().unwrap()]);
        let next = store.koto_ok(&[
            "next",
            SESSION,
            "--no-cleanup",
            "--with-data",
            r#"{"step": "go"}"#,
        ]);
        let next: serde_json::Value = serde_json::from_slice(&next.stdout).unwrap();
        assert_eq!(next["state"], "middle", "got: {next}");
        store.add(NESTED_KEY, NESTED_VALUE);
        store.add(PLAIN_KEY, PLAIN_VALUE);
        let published = store.published_dir();
        store.koto_ok(&[
            "workflows",
            "publish",
            "--session",
            SESSION,
            "--dir",
            published.to_str().unwrap(),
        ]);
        store
    }

    fn dir(&self) -> &Path {
        self.tmp.path()
    }

    fn sessions(&self) -> PathBuf {
        self.dir().join("sessions")
    }

    fn session_dir(&self) -> PathBuf {
        self.sessions().join(SESSION)
    }

    fn ctx(&self) -> PathBuf {
        self.session_dir().join("ctx")
    }

    fn log(&self) -> PathBuf {
        self.session_dir()
            .join(format!("koto-{SESSION}.state.jsonl"))
    }

    fn published_dir(&self) -> PathBuf {
        self.dir().join("workflows-dir")
    }

    /// A koto run on this store, with no reader's anchor and no hosting
    /// Claude Code session, so a read is anchorless and nothing else is
    /// published.
    fn koto(&self, args: &[&str]) -> Output {
        let mut cmd = koto_command::koto();
        cmd.current_dir(self.dir())
            .env("KOTO_SESSIONS_BASE", self.sessions())
            .env("KOTO_HOME", self.dir().join("koto-home"))
            .env("HOME", self.dir());
        for var in [
            "KOTO_ANCHOR",
            "KOTO_DISPATCHER_ANCHOR",
            "KOTO_TICK_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_WORKFLOWS_HOST_SESSION",
        ] {
            cmd.env_remove(var);
        }
        cmd.args(args).output().unwrap()
    }

    fn koto_ok(&self, args: &[&str]) -> Output {
        let out = self.koto(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn add(&self, key: &str, value: &str) {
        let file = self.dir().join("add-input.txt");
        std::fs::write(&file, value).unwrap();
        self.koto_ok(&[
            "context",
            "add",
            SESSION,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ]);
    }

    /// What `koto context get` prints for `key`. Records a read, so call
    /// it only after the `read_context` calls it is compared with.
    fn context_get(&self, key: &str) -> Vec<u8> {
        self.koto_ok(&["context", "get", SESSION, key]).stdout
    }

    /// The store as a dependent opens it.
    fn source(&self) -> LocalDirSource {
        LocalDirSource::new(self.sessions()).unwrap()
    }

    /// The session's events, read through the hardened surface.
    fn events(&self) -> Vec<Event> {
        read_session_events(&self.source(), SESSION).unwrap().1
    }

    fn read(&self, key: &str, events: Option<&[Event]>) -> ContextRead {
        read_context(&self.session_dir(), key, events)
    }
}

/// Run `f` on a thread, failing the test if it hasn't returned within
/// [`DEADLINE`].
fn within_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(DEADLINE).expect("the read blocked")
}

fn present(bytes: &[u8], from: ContextSource) -> ContextRead {
    ContextRead::Present {
        bytes: bytes.to_vec(),
        from,
    }
}

/// Checks that `read` is `Unreadable` with reason `<category>: <path>`.
fn assert_unreadable(read: &ContextRead, category: &str, path: &Path) {
    assert_eq!(
        read,
        &ContextRead::Unreadable(format!("{category}: {}", path.display())),
    );
}

#[test]
fn read_session_events_reads_the_log_koto_wrote() {
    let store = KotoStore::build();
    let (header, events) = read_session_events(&store.source(), SESSION).unwrap();
    assert_eq!(header.workflow, SESSION);
    let log = std::fs::read_to_string(store.log()).unwrap();
    assert_eq!(events.len(), log.lines().count() - 1);
    assert!(events
        .iter()
        .any(|e| e.payload.type_name() == "transitioned"));
}

#[test]
fn read_session_events_refuses_what_is_not_a_regular_log_inside_the_store() {
    let store = KotoStore::build();
    let source = store.source();
    let refusal = |id: &str| read_session_events(&source, id).unwrap_err();

    assert_eq!(refusal("../wf").category, SessionReadCategory::InvalidId);
    assert_eq!(refusal("missing").category, SessionReadCategory::NotFound);

    // A symlink at a state file's path, to a regular log outside the store.
    let outside = store.dir().join("outside.state.jsonl");
    std::fs::copy(store.log(), &outside).unwrap();
    let linked = store.sessions().join("linked");
    std::fs::create_dir(&linked).unwrap();
    std::os::unix::fs::symlink(&outside, linked.join("koto-linked.state.jsonl")).unwrap();
    assert_eq!(refusal("linked").category, SessionReadCategory::Symlink);

    // A FIFO at a state file's path, refused without blocking.
    let fifo = store.sessions().join("fifo");
    std::fs::create_dir(&fifo).unwrap();
    mkfifo(&fifo.join("koto-fifo.state.jsonl"));
    let sessions = store.sessions();
    let err = within_deadline(move || {
        read_session_events(&LocalDirSource::new(sessions).unwrap(), "fifo").unwrap_err()
    });
    assert_eq!(err.category, SessionReadCategory::NotRegular);
    assert_eq!(
        err.to_string(),
        format!(
            "state log is not a regular file: {}",
            fifo.join("koto-fifo.state.jsonl").display()
        )
    );
}

#[test]
fn unusable_keys_are_refused() {
    let store = KotoStore::build();
    let events = store.events();
    for key in ["../x", "a//b", "manifest.json", "manifest.lock"] {
        for given in [None, Some(&events[..])] {
            let read = store.read(key, given);
            assert!(
                matches!(&read, ContextRead::Unusable(reason) if reason.contains(&format!("{key:?}"))),
                "{key:?}: {read:?}"
            );
        }
    }
}

#[test]
fn an_assigned_value_whose_file_was_removed_reads_from_the_log() {
    let store = KotoStore::build();
    let events = store.events();
    std::fs::remove_file(store.ctx().join(ASSIGNED_KEY)).unwrap();

    let from_log = store.read(ASSIGNED_KEY, Some(&events));
    assert_eq!(
        from_log,
        present(ASSIGNED_VALUE.as_bytes(), ContextSource::Log)
    );
    // Without the events only the store answers, and it has no file.
    assert_eq!(store.read(ASSIGNED_KEY, None), ContextRead::Absent);

    assert_eq!(store.context_get(ASSIGNED_KEY), ASSIGNED_VALUE.as_bytes());
}

#[test]
fn an_assigned_value_whose_file_was_changed_reads_from_the_log() {
    let store = KotoStore::build();
    let events = store.events();
    std::fs::write(store.ctx().join(ASSIGNED_KEY), "changed by hand").unwrap();

    let from_log = store.read(ASSIGNED_KEY, Some(&events));
    assert_eq!(
        from_log,
        present(ASSIGNED_VALUE.as_bytes(), ContextSource::Log)
    );
    assert_eq!(
        store.read(ASSIGNED_KEY, None),
        present(b"changed by hand", ContextSource::Store)
    );

    assert_eq!(store.context_get(ASSIGNED_KEY), ASSIGNED_VALUE.as_bytes());
}

#[test]
fn a_missing_ctx_directory_intermediate_or_file_reads_absent() {
    let store = KotoStore::build();
    let events = store.events();
    // The intermediate `nothere/` and the leaves `notes/missing` and
    // `missing` don't exist.
    assert_eq!(store.read("nothere/x", Some(&events)), ContextRead::Absent);
    assert_eq!(
        store.read("notes/missing", Some(&events)),
        ContextRead::Absent
    );
    assert_eq!(store.read("missing", Some(&events)), ContextRead::Absent);

    // Nor does `ctx/` itself, once moved away.
    std::fs::rename(store.ctx(), store.session_dir().join("ctx-away")).unwrap();
    assert_eq!(store.read(PLAIN_KEY, None), ContextRead::Absent);
    assert_eq!(store.read(NESTED_KEY, Some(&events)), ContextRead::Absent);
}

#[test]
fn a_planted_value_reads_unreadable() {
    let store = KotoStore::build();
    let events = store.events();
    let ctx = store.ctx();

    // A symlinked leaf, to a regular file outside the store.
    let outside = store.dir().join("outside-value");
    std::fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, ctx.join("linked")).unwrap();
    assert_unreadable(
        &store.read("linked", Some(&events)),
        "context value path holds a symlink",
        &ctx.join("linked"),
    );

    // A symlinked intermediate directory, to a directory outside the store.
    let outside_dir = store.dir().join("outside-dir");
    std::fs::create_dir(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("x"), "outside").unwrap();
    std::os::unix::fs::symlink(&outside_dir, ctx.join("linkdir")).unwrap();
    assert_unreadable(
        &store.read("linkdir/x", Some(&events)),
        "context value path holds a symlink",
        &ctx.join("linkdir"),
    );

    // A FIFO leaf, refused without blocking.
    mkfifo(&ctx.join("fifo"));
    let session_dir = store.session_dir();
    let fifo = within_deadline(move || read_context(&session_dir, "fifo", None));
    assert_unreadable(
        &fifo,
        "context value is not a regular file",
        &ctx.join("fifo"),
    );

    // A directory where the value's file belongs: `notes/` holds a value.
    assert_unreadable(
        &store.read("notes", Some(&events)),
        "context value is a directory",
        &ctx.join("notes"),
    );

    // A sparse file one byte over the cap.
    let big = ctx.join("big");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(CONTEXT_VALUE_READ_CAP + 1)
        .unwrap();
    assert_unreadable(
        &store.read("big", Some(&events)),
        "context value is larger than 16 MiB",
        &big,
    );
    std::fs::remove_file(&big).unwrap();
}

#[test]
fn a_regular_value_reads_from_the_store_as_koto_context_get_prints_it() {
    let store = KotoStore::build();
    let events = store.events();
    let published = store.published_dir();
    let expected = [
        (NESTED_KEY, NESTED_VALUE.as_bytes()),
        (PLAIN_KEY, PLAIN_VALUE.as_bytes()),
        (PUBLISH_KEY, published.to_str().unwrap().as_bytes()),
    ];

    let mut reads = Vec::new();
    for (key, value) in expected {
        for given in [None, Some(&events[..])] {
            let read = store.read(key, given);
            assert_eq!(read, present(value, ContextSource::Store), "{key}");
            reads.push((key, read));
        }
    }
    // The transition's own value, whose file koto stored, reads from the
    // log when the events are given and from the store when not.
    assert_eq!(
        store.read(ASSIGNED_KEY, Some(&events)),
        present(ASSIGNED_VALUE.as_bytes(), ContextSource::Log)
    );
    assert_eq!(
        store.read(ASSIGNED_KEY, None),
        present(ASSIGNED_VALUE.as_bytes(), ContextSource::Store)
    );

    // Debug holds the length, never the bytes.
    let debug = format!("{:?}", reads[0].1);
    assert_eq!(
        debug,
        format!("Present {{ len: {}, from: Store }}", NESTED_VALUE.len())
    );
    assert!(!debug.contains("the plan"));

    // koto, after every read it is compared with.
    for (key, read) in reads {
        let ContextRead::Present { bytes, .. } = read else {
            unreachable!("asserted Present above")
        };
        assert_eq!(store.context_get(key), bytes, "{key}");
    }
    assert_eq!(store.context_get(ASSIGNED_KEY), ASSIGNED_VALUE.as_bytes());
}

/// One path's entry in a [`snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    file_type: String,
    size: u64,
    modified: SystemTime,
    bytes: Option<Vec<u8>>,
}

/// Every path under `dir`, relative to it (`""` for `dir` itself), with its
/// type, size, modification time and, for a file, its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        let meta = std::fs::symlink_metadata(path).unwrap();
        let file_type = meta.file_type();
        let entry = Entry {
            file_type: format!("{file_type:?}"),
            size: meta.len(),
            modified: meta.modified().unwrap(),
            bytes: file_type.is_file().then(|| std::fs::read(path).unwrap()),
        };
        out.insert(path.strip_prefix(root).unwrap().to_path_buf(), entry);
        if file_type.is_dir() {
            for child in std::fs::read_dir(path).unwrap() {
                walk(root, &child.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The lines in the file at `path`, 0 when there is no file.
fn line_count(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(text) => text.lines().count(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

#[test]
fn a_hundred_reads_change_nothing_where_one_koto_context_get_records_a_read() {
    let store = KotoStore::build();
    let listed = store.koto_ok(&["context", "list", SESSION]);
    let keys: Vec<String> = serde_json::from_slice(&listed.stdout).unwrap();
    for key in [ASSIGNED_KEY, NESTED_KEY, PLAIN_KEY, PUBLISH_KEY] {
        assert!(keys.iter().any(|k| k == key), "{key} not in {keys:?}");
    }

    let session_dir = store.session_dir();
    let spool = session_dir.join("context-reads.jsonl");
    let before = snapshot(&session_dir);
    let log_lines = line_count(&store.log());
    let spool_lines = line_count(&spool);

    let source = store.source();
    for _ in 0..100 {
        let (_, events) = read_session_events(&source, SESSION).unwrap();
        for key in &keys {
            for given in [None, Some(&events[..])] {
                let read = read_context(&session_dir, key, given);
                assert!(
                    matches!(read, ContextRead::Present { .. }),
                    "{key}: {read:?}"
                );
            }
        }
    }
    assert_eq!(snapshot(&session_dir), before);
    assert_eq!(line_count(&store.log()), log_lines);

    // One koto read, with no reader's anchor, records itself in the spool
    // and leaves the log alone.
    store.context_get(PLAIN_KEY);
    assert_eq!(line_count(&store.log()), log_lines);
    assert_eq!(line_count(&spool), spool_lines + 1);
    let after = snapshot(&session_dir);
    assert_ne!(after, before);
    let rest = |snap: &BTreeMap<PathBuf, Entry>| {
        let mut snap = snap.clone();
        snap.remove(Path::new(""));
        snap.remove(Path::new("context-reads.jsonl"));
        snap
    };
    assert_eq!(rest(&after), rest(&before), "only the spool may change");
}

/// Checks that neither `Display` nor `Debug` of a reason holds
/// [`PLANTED`], and that `Display` names `path`.
fn assert_no_planted_bytes(display: &str, debug: &str, path: &Path) {
    assert!(
        !display.contains(PLANTED),
        "Display quotes the file: {display}"
    );
    assert!(!debug.contains(PLANTED), "Debug quotes the file: {debug}");
    assert!(
        display.contains(&path.display().to_string()),
        "{display} doesn't name {}",
        path.display()
    );
}

#[test]
fn reasons_carry_no_bytes_from_the_files_they_refuse() {
    let store = KotoStore::build();
    assert!(!store.dir().to_string_lossy().contains(PLANTED));

    // A malformed, non-final log line shaped so a parser would quote it.
    let log = std::fs::read_to_string(store.log()).unwrap();
    let mut lines: Vec<&str> = log.lines().collect();
    let planted_line =
        format!(r#"{{"seq":"{PLANTED}","timestamp":"2026-05-24T00:00:00Z","type":"x"}}"#);
    lines.insert(1, &planted_line);
    std::fs::write(store.log(), lines.join("\n") + "\n").unwrap();
    let err: SessionReadError = read_session_events(&store.source(), SESSION).unwrap_err();
    assert_eq!(err.category, SessionReadCategory::Malformed);
    assert_eq!(
        err.to_string(),
        format!("state log is malformed: {}", store.log().display())
    );
    assert_no_planted_bytes(&err.to_string(), &format!("{err:?}"), &store.log());

    // A marker holding the string, under the marker's cap.
    let marker_path = marker(&store.sessions());
    std::fs::write(&marker_path, PLANTED).unwrap();
    let err = check_layout(&store.sessions()).unwrap_err();
    assert_eq!(
        err,
        LayoutError::Unusable {
            path: marker_path.clone(),
            category: "not a layout number",
        }
    );
    assert_no_planted_bytes(&err.to_string(), &format!("{err:?}"), &marker_path);
    std::fs::remove_file(&marker_path).unwrap();

    // An over-cap context value that starts with the string.
    let big = store.ctx().join("big");
    std::fs::write(&big, PLANTED).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(CONTEXT_VALUE_READ_CAP + 1)
        .unwrap();
    let read = store.read("big", None);
    let ContextRead::Unreadable(reason) = &read else {
        panic!("the over-cap value read as {read:?}");
    };
    assert_no_planted_bytes(reason, &format!("{read:?}"), &big);
    std::fs::remove_file(&big).unwrap();

    // A symlinked leaf whose target starts with the string.
    let target = store.dir().join("planted-target");
    std::fs::write(&target, format!("{PLANTED} and more")).unwrap();
    let linked = store.ctx().join("linked");
    std::os::unix::fs::symlink(&target, &linked).unwrap();
    let read = store.read("linked", None);
    let ContextRead::Unreadable(reason) = &read else {
        panic!("the symlinked value read as {read:?}");
    };
    assert_no_planted_bytes(reason, &format!("{read:?}"), &linked);
}

#[test]
fn koto_version_is_the_koto_release_the_crate_ships_with() {
    assert_eq!(KOTO_VERSION, env!("CARGO_PKG_VERSION"));
}
