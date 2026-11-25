//! The dashboard scan on a store where a writer planted something a state
//! file or leg pointer can't be, and on a store whose layout marker names a
//! layout this koto doesn't read.
//!
//! Each test builds the golden store (`tests/support/golden_store.rs`), runs
//! `koto dashboard --once` and `--once --all` on it unplanted, plants one
//! thing, and runs both again under a deadline: a FIFO, a symlink to a log
//! outside the store or a sparse file one byte over the state-log cap where
//! a state file belongs; a FIFO where a leg pointer belongs; a session
//! directory moved out of the store and linked back. The same plants go
//! through `LocalDirSource` and `refresh_scan` in-process, on a thread with
//! a timeout.
//!
//! Rows are compared as `tests/dashboard_golden_test.rs` compares them: cut
//! to columns 1, 2, 4, 5, 6 and 8, leaving out the two that move with the
//! clock.

#![cfg(unix)]

#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use koto_session_reader::dashboard::{refresh_scan, SessionTree};
use koto_session_reader::engine::leg_pointer::POINTER_FILENAME;
use koto_session_reader::source::{LocalDirSource, STATE_LOG_READ_CAP, STORE_LAYOUT_MARKER};
use tempfile::TempDir;

const EXPECTED_PATH: &str = "tests/fixtures/dashboard-golden/expected.txt";

/// How long one `koto dashboard` run may take before the test calls it
/// blocked.
const BINARY_DEADLINE: Duration = Duration::from_secs(60);

/// How long one in-process scan may take before the test calls it blocked.
const IN_PROCESS_DEADLINE: Duration = Duration::from_secs(10);

/// The session whose state file each state-file plant replaces. It has no
/// children, so leaving it out changes no other row.
const PLANTED: &str = "golden-idle";

/// The session whose leg pointer path holds a FIFO.
const POINTER_SESSION: &str = "golden-active";

/// The session whose directory is moved out of the store and linked back.
const LINKED_SESSION: &str = "golden-pending";

/// The note `--once` prints on stderr for `n` unreadable sessions.
fn note(n: usize) -> Vec<String> {
    vec![format!("note: {n} unreadable session(s) skipped")]
}

/// The golden store in `<tmp>/sessions`, with `<tmp>/outside` beside it for
/// what a plant puts outside the store.
struct Store {
    tmp: TempDir,
    sessions: PathBuf,
    outside: PathBuf,
}

impl Store {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let sessions = tmp.path().join("sessions");
        golden_store::build(&sessions, SystemTime::now());
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        Store {
            tmp,
            sessions,
            outside,
        }
    }

    fn state_path(&self, id: &str) -> PathBuf {
        self.sessions
            .join(id)
            .join(format!("koto-{id}.state.jsonl"))
    }

    fn marker_path(&self) -> PathBuf {
        self.sessions.join(STORE_LAYOUT_MARKER)
    }
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// One thing a writer could put in the store.
#[derive(Debug, Clone, Copy)]
enum Plant {
    /// A FIFO at [`PLANTED`]'s state file path.
    StateFifo,
    /// A symlink at [`PLANTED`]'s state file path to its log, moved
    /// outside the store.
    StateSymlink,
    /// A sparse file one byte over the state-log cap at [`PLANTED`]'s
    /// state file path.
    StateOverCap,
    /// A FIFO at [`POINTER_SESSION`]'s leg pointer path.
    PointerFifo,
    /// [`LINKED_SESSION`]'s directory moved outside the store and linked
    /// back.
    LinkedDir,
}

fn plant(store: &Store, what: Plant) {
    let state = store.state_path(PLANTED);
    match what {
        Plant::StateFifo => {
            std::fs::remove_file(&state).unwrap();
            mkfifo(&state);
        }
        Plant::StateSymlink => {
            let target = store.outside.join(format!("koto-{PLANTED}.state.jsonl"));
            std::fs::rename(&state, &target).unwrap();
            std::os::unix::fs::symlink(&target, &state).unwrap();
        }
        Plant::StateOverCap => {
            std::fs::remove_file(&state).unwrap();
            std::fs::File::create(&state)
                .unwrap()
                .set_len(STATE_LOG_READ_CAP + 1)
                .unwrap();
        }
        Plant::PointerFifo => {
            mkfifo(&store.sessions.join(POINTER_SESSION).join(POINTER_FILENAME));
        }
        Plant::LinkedDir => {
            let target = store.outside.join(LINKED_SESSION);
            std::fs::rename(store.sessions.join(LINKED_SESSION), &target).unwrap();
            std::os::unix::fs::symlink(&target, store.sessions.join(LINKED_SESSION)).unwrap();
        }
    }
}

/// A `--once` line cut to columns 1, 2, 4, 5, 6 and 8.
fn project(line: &str) -> String {
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        fields.len(),
        10,
        "a --once line has ten tab-separated columns: {line:?}"
    );
    [1, 2, 4, 5, 6, 8]
        .iter()
        .map(|n| fields[n - 1])
        .collect::<Vec<_>>()
        .join("\t")
}

/// One `koto dashboard` run.
struct Run {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn rows(&self) -> Vec<String> {
        self.stdout.lines().map(project).collect()
    }

    fn stderr_lines(&self) -> Vec<String> {
        self.stderr.lines().map(str::to_string).collect()
    }
}

/// Runs `koto dashboard <args>` on the store, failing the test if it
/// hasn't exited within [`BINARY_DEADLINE`].
fn run_dashboard(store: &Store, args: &[&str]) -> Run {
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(store.tmp.path())
        .env("KOTO_SESSIONS_BASE", &store.sessions)
        .env("HOME", store.tmp.path())
        .env("XDG_CACHE_HOME", store.tmp.path().join("cache"))
        .arg("dashboard")
        .args(args)
        .timeout(BINARY_DEADLINE);
    let started = Instant::now();
    let output = cmd.output().unwrap();
    assert!(
        started.elapsed() < BINARY_DEADLINE,
        "koto dashboard {} didn't exit within {BINARY_DEADLINE:?}",
        args.join(" ")
    );
    Run {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Runs a view that must succeed.
fn run_ok(store: &Store, args: &[&str]) -> Run {
    let run = run_dashboard(store, args);
    assert!(
        run.success,
        "koto dashboard {} exited {:?}; stderr:\n{}",
        args.join(" "),
        run.code,
        run.stderr
    );
    run
}

const VIEWS: [&[&str]; 2] = [&["--once"], &["--once", "--all"]];

/// What one in-process scan's tree holds of each row, keyed by row id.
type Rows = BTreeMap<String, String>;

/// Runs `LocalDirSource::new` and `refresh_scan` on a thread, failing the
/// test if they haven't returned within [`IN_PROCESS_DEADLINE`]. Returns
/// each row's facts and the unreadable tally.
fn scan_in_process(sessions: &Path) -> (Rows, usize) {
    let (tx, rx) = mpsc::channel();
    let dir = sessions.to_path_buf();
    std::thread::spawn(move || {
        let scanned = (|| -> anyhow::Result<(Rows, usize)> {
            let source = LocalDirSource::new(&dir)?;
            let mut tree = SessionTree::<()>::new();
            refresh_scan(&mut tree, &source)?;
            let rows = tree
                .sessions
                .iter()
                .map(|(id, s)| {
                    let facts = format!(
                        "state={:?} terminal={} blocked={} intent={:?} unreadable={} leg={} \
                         workflow={:?} parent={:?}",
                        s.current_state,
                        s.is_terminal,
                        s.is_blocked,
                        s.intent,
                        s.is_unreadable,
                        s.is_request_leg,
                        s.header.workflow,
                        s.header.parent_workflow,
                    );
                    (id.clone(), facts)
                })
                .collect();
            Ok((rows, tree.unreadable))
        })();
        let _ = tx.send(scanned.map_err(|e| e.to_string()));
    });
    rx.recv_timeout(IN_PROCESS_DEADLINE)
        .expect("the in-process scan blocked")
        .expect("the in-process scan failed")
}

/// Plants `what` and checks the binary and the in-process scan against
/// the same store unplanted: `gone` is the row that leaves, and `extra`
/// how many more sessions the note counts.
fn check_plant(what: Plant, gone: Option<&str>, extra: usize) {
    let store = Store::new();
    let baseline: Vec<Run> = VIEWS.iter().map(|v| run_ok(&store, v)).collect();
    let (base_rows, base_unreadable) = scan_in_process(&store.sessions);
    assert_eq!(baseline[0].stderr_lines(), note(1), "the unplanted store");
    assert_eq!(base_unreadable, 1, "the unplanted store");

    plant(&store, what);

    for (view, before) in VIEWS.iter().zip(&baseline) {
        let after = run_ok(&store, view);
        let view = view.join(" ");
        let want: Vec<String> = before
            .rows()
            .into_iter()
            .filter(|row| Some(row.split('\t').next().unwrap()) != gone)
            .collect();
        if let Some(gone) = gone {
            assert!(
                before
                    .rows()
                    .iter()
                    .any(|r| r.starts_with(&format!("{gone}\t"))),
                "{gone} is a row of the unplanted `koto dashboard {view}`"
            );
            assert!(
                !after.stdout.contains(gone),
                "{what:?}: {gone} reached `koto dashboard {view}` stdout:\n{}",
                after.stdout
            );
        }
        assert_eq!(
            after.rows(),
            want,
            "{what:?}: `koto dashboard {view}` rows differ from the unplanted run's"
        );
        assert_eq!(
            after.stderr_lines(),
            note(1 + extra),
            "{what:?}: `koto dashboard {view}` stderr"
        );
    }

    let (rows, unreadable) = scan_in_process(&store.sessions);
    let mut want = base_rows.clone();
    if let Some(gone) = gone {
        assert!(
            want.remove(gone).is_some(),
            "{gone} is in the unplanted tree"
        );
    }
    assert_eq!(rows, want, "{what:?}: the in-process tree's rows");
    assert_eq!(
        unreadable,
        base_unreadable + extra,
        "{what:?}: the in-process unreadable tally"
    );
}

#[test]
fn a_fifo_state_file_is_unreadable_and_the_scan_goes_on() {
    check_plant(Plant::StateFifo, Some(PLANTED), 1);
}

#[test]
fn a_symlinked_state_file_is_unreadable_and_the_scan_goes_on() {
    check_plant(Plant::StateSymlink, Some(PLANTED), 1);
}

#[test]
fn an_over_cap_state_file_is_unreadable_and_the_scan_goes_on() {
    check_plant(Plant::StateOverCap, Some(PLANTED), 1);
}

#[test]
fn a_fifo_leg_pointer_reads_as_no_pointer() {
    check_plant(Plant::PointerFifo, None, 0);
}

#[test]
fn a_symlinked_session_directory_is_neither_a_row_nor_counted() {
    check_plant(Plant::LinkedDir, Some(LINKED_SESSION), 0);
}

#[test]
fn a_newer_schema_version_is_counted_unreadable_in_process() {
    let store = Store::new();
    let (base_rows, base_unreadable) = scan_in_process(&store.sessions);

    let path = store.state_path(PLANTED);
    let log = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = log.split_once('\n').unwrap();
    let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
    header["schema_version"] = serde_json::json!(2);
    std::fs::write(&path, format!("{header}\n{rest}")).unwrap();

    let (rows, unreadable) = scan_in_process(&store.sessions);
    let mut want = base_rows;
    assert!(want.remove(PLANTED).is_some());
    assert_eq!(rows, want, "the other rows stay");
    assert_eq!(unreadable, base_unreadable + 1);
}

/// The rows the golden test expects of `--once`, read from its file.
fn expected_once_rows() -> (Vec<String>, Vec<String>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(EXPECTED_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut once = Vec::new();
    let mut stderr = Vec::new();
    let mut section = "";
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line;
            continue;
        }
        match section {
            "[once]" => once.push(line.to_string()),
            "[stderr]" => stderr.push(line.to_string()),
            _ => {}
        }
    }
    assert!(!once.is_empty(), "{EXPECTED_PATH} has a [once] section");
    (once, stderr)
}

#[test]
fn a_newer_layout_marker_refuses_the_store_and_its_absence_reads_as_layout_1() {
    let store = Store::new();
    std::fs::write(store.marker_path(), "2\n").unwrap();

    let refused = run_dashboard(&store, &["--once"]);
    assert!(
        !refused.success,
        "koto dashboard --once read a store marked layout 2; stdout:\n{}",
        refused.stdout
    );
    let message = format!(
        "session store layout marker {} says layout 2; this reader supports layout 1",
        store.marker_path().display()
    );
    assert!(
        refused.stderr.contains(&message),
        "stderr lacks {message:?}:\n{}",
        refused.stderr
    );
    assert_eq!(refused.stdout, "", "no row is printed");

    std::fs::remove_file(store.marker_path()).unwrap();
    let (once, stderr) = expected_once_rows();
    let read = run_ok(&store, &["--once"]);
    assert_eq!(
        read.rows(),
        once,
        "rows differ from {EXPECTED_PATH}'s [once]"
    );
    assert_eq!(read.stderr_lines(), stderr);
}
