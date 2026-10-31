//! `koto dashboard --once` on one machine shows the sessions another
//! machine pushed to the same bucket.
//!
//! Machine A creates a session on a two-state template whose first state is
//! gated until context key `go` exists, ticks it once so it stops at the
//! gate, and its push publishes a `summary.json` beside the state file.
//! Machine B, with its own `HOME`, session store and working directory but
//! the same bucket, lists every workspace prefix with a `/ctx/`-delimited
//! listing and prints A's session with its state, A's machine name and A's
//! workspace id as columns nine and ten.
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

const BUCKET: &str = "koto-dashboard";
const WF: &str = "wf";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The fixture template: `check` waits on context key `go`, then `done`.
const TEMPLATE: &str = r#"---
name: gated
version: "1.0"
initial_state: check
states:
  check:
    gates:
      go:
        type: context-exists
        key: go
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Wait for go.

## done

Done.
"#;

/// The bucket prefix koto derives for a directory.
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

    /// The `--once` lines, each split into its tab-separated fields.
    fn rows(&self) -> Vec<Vec<String>> {
        self.stdout
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    }
}

/// One machine: its own `HOME` (and so its own session store), template
/// cache and machine name, and a working directory of its own.
struct Machine {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    dir: PathBuf,
    endpoint: String,
    name: &'static str,
    cloud: bool,
}

impl Machine {
    fn new(s3: &FakeS3, name: &'static str) -> Self {
        Self::with_backend(s3, name, true)
    }

    fn with_backend(s3: &FakeS3, name: &'static str, cloud: bool) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        Machine {
            home: mk(root.path().join("home")),
            cache: mk(root.path().join("cache")),
            dir: mk(root.path().join(format!("work-{name}"))),
            _root: root,
            endpoint: s3.endpoint(),
            name,
            cloud,
        }
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto").join("sessions")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(&self.dir)
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
            .env("KOTO_MACHINE_NAME", self.name)
            // A bucket is configured either way; only the backend differs.
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
            .env(
                "KOTO_SESSION_BACKEND",
                if self.cloud { "cloud" } else { "local" },
            )
            .args(args);
        cmd
    }

    fn koto(&self, args: &[&str]) -> Out {
        let o = self.cmd(args).output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert_eq!(out.code, Some(0), "koto {:?}: {}", args, out.describe());
        out
    }

    /// Create `name` on the fixture template and tick it once, so it stops
    /// at the gate.
    fn start_blocked(&self, name: &str) {
        std::fs::write(self.dir.join("gated.md"), TEMPLATE).unwrap();
        self.ok(&["init", name, "--template", "gated.md"]);
        self.ok(&["next", name]);
    }

    fn workspace(&self) -> String {
        prefix_of(&self.dir)
    }
}

/// The row for `name` under `workspace` in `rows`.
fn row_of<'a>(rows: &'a [Vec<String>], name: &str, workspace: &str) -> Option<&'a Vec<String>> {
    rows.iter().find(|r| {
        r.first().map(String::as_str) == Some(name)
            && r.get(9).map(String::as_str) == Some(workspace)
    })
}

/// Machine A with a session stopped at the fixture's gate, and machine B on
/// the same bucket.
fn two_machines() -> (FakeS3, Machine, Machine) {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    a.start_blocked(WF);
    (s3, a, b)
}

/// PLAN step 1's check: B's `--once` prints A's blocked session with A's
/// columns, `machine-a` and A's workspace, reading only listings and the
/// summary.
#[test]
fn once_prints_other_machines_blocked_session() {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    let ws = a.workspace();

    std::fs::write(a.dir.join("gated.md"), TEMPLATE).unwrap();
    a.ok(&["init", WF, "--template", "gated.md"]);
    s3.clear_requests();
    a.ok(&["next", WF]);

    // A's tick pushed the state file and then the summary beside it.
    let state_key = format!("{ws}/{WF}/koto-{WF}.state.jsonl");
    let summary_key = format!("{ws}/{WF}/summary.json");
    let ticked = s3.requests();
    let state_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == state_key)
        .unwrap_or_else(|| panic!("the tick pushed no state: {ticked:?}"));
    let summary_put = ticked
        .iter()
        .rposition(|r| r.method == "PUT" && r.key == summary_key)
        .unwrap_or_else(|| panic!("the tick published no summary: {ticked:?}"));
    assert!(
        summary_put > state_put,
        "the summary follows the state push"
    );
    // The summary PUT's latency after the state push, against koto's 30 s
    // action limit.
    let latency = ticked[summary_put].at - ticked[state_put].at;
    eprintln!("summary.json PUT latency after the state push: {latency:?}");
    assert!(latency < Duration::from_secs(30), "latency {latency:?}");

    let summary: serde_json::Value =
        serde_json::from_slice(&s3.object(&summary_key).expect("summary in the bucket")).unwrap();
    assert_eq!(summary["current_state"], "check");
    assert_eq!(summary["is_blocked"], true);
    assert_eq!(summary["machine"], "machine-a");
    assert_eq!(summary["schema"], 1);

    let a_rows = a.ok(&["dashboard", "--once"]).rows();
    let a_row = row_of(&a_rows, WF, &ws).unwrap_or_else(|| panic!("A's own row: {a_rows:?}"));
    assert_eq!(a_row[8], "machine-a", "A's own row names A in column nine");

    s3.clear_requests();
    let out = b.ok(&["dashboard", "--once"]);
    let rows = out.rows();
    let row = row_of(&rows, WF, &ws).unwrap_or_else(|| panic!("A's row on B: {}", out.describe()));
    assert_eq!(row.len(), 10, "ten tab-separated fields: {row:?}");
    for (i, column) in [
        (1, "current_state"),
        (3, "status"),
        (4, "intent"),
        (5, "template_name"),
    ] {
        assert_eq!(row[i], a_row[i], "{column} on B equals A's own");
    }
    assert_eq!(row[1], "check");
    assert_eq!(row[3], "blocked");
    assert_eq!(row[8], "machine-a");
    assert_eq!(row[9], ws);

    // B read the bucket and wrote nothing: only listings and GETs, none of
    // them of a state file, and no directory in B's session store.
    let requests = s3.requests();
    assert!(
        requests.iter().all(|r| r.method == "GET"),
        "only listings and GETs: {requests:?}"
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.list_prefix.is_none() && r.key.ends_with(".state.jsonl")),
        "no state file fetched: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|r| r.list_prefix.as_deref() == Some(&format!("{ws}/"))
                && r.delimiter.as_deref() == Some("/ctx/")),
        "A's workspace listed with the /ctx/ delimiter: {requests:?}"
    );
    let created: Vec<_> = std::fs::read_dir(b.sessions())
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        created.is_empty(),
        "B's session store stays empty: {created:?}"
    );
}

/// A session B created itself shows once, as B's own row with B's machine,
/// though its copy and summary are in the bucket too.
#[test]
fn once_shows_own_session_once_with_own_machine() {
    let (_s3, a, b) = two_machines();
    b.start_blocked("mine");

    let rows = b.ok(&["dashboard", "--once"]).rows();
    let mine: Vec<_> = rows.iter().filter(|r| r[0] == "mine").collect();
    assert_eq!(mine.len(), 1, "B's own session once: {rows:?}");
    assert_eq!(mine[0][8], "machine-b");
    assert_eq!(mine[0][9], b.workspace());
    assert!(
        row_of(&rows, WF, &a.workspace()).is_some(),
        "A's row too: {rows:?}"
    );
}

/// `--needs-you`, `--status` and a session name apply to a remote row as to
/// a local one.
#[test]
fn once_filters_apply_to_remote_rows() {
    let (_s3, a, b) = two_machines();
    let ws = a.workspace();

    let needs_you = b.ok(&["dashboard", "--once", "--needs-you"]).rows();
    assert!(row_of(&needs_you, WF, &ws).is_some(), "{needs_you:?}");

    let running = b.ok(&["dashboard", "--once", "--status", "running"]).rows();
    assert!(row_of(&running, WF, &ws).is_none(), "{running:?}");

    let blocked = b
        .ok(&["dashboard", "--once", "--status", "needs-you-blocked"])
        .rows();
    assert!(row_of(&blocked, WF, &ws).is_some(), "{blocked:?}");

    let named = b.ok(&["dashboard", "--once", WF]).rows();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0][9], ws);
    assert_eq!(
        named[0][7], "needs-you-blocked",
        "column eight keeps its meaning"
    );
}

/// With the local backend selected, even with a bucket configured, the two
/// new columns are empty and the run asks the bucket nothing.
#[test]
fn once_on_the_local_backend_prints_empty_columns_and_makes_no_request() {
    let (s3, _a, _b) = two_machines();
    let local = Machine::with_backend(&s3, "machine-c", false);
    local.start_blocked("here");

    s3.clear_requests();
    let rows = local.ok(&["dashboard", "--once"]).rows();
    assert_eq!(rows.len(), 1, "only the local session: {rows:?}");
    assert_eq!(rows[0].len(), 10);
    assert_eq!(rows[0][0], "here");
    assert_eq!((rows[0][8].as_str(), rows[0][9].as_str()), ("", ""));
    assert!(s3.requests().is_empty(), "no request: {:?}", s3.requests());
}

// ---------------------------------------------------------------------------
// The interactive dashboard (PLAN step 2)
// ---------------------------------------------------------------------------

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use koto::cli::dashboard::{run_loop, EventSource, LoopSources, SessionLoader, SystemClock};
use koto::cli::dashboard_data::{self, SessionTree};
use koto::cli::dashboard_state::DashboardAppState;
use koto::session::local::LocalBackend;
use koto::session::remote_feed::{RemoteFeed, RESCAN_INTERVAL};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A client of the bucket at `endpoint`, as koto builds one from the
/// machines' environment.
fn bucket_at(endpoint: &str) -> Box<s3::Bucket> {
    let region = s3::Region::Custom {
        region: "us-east-1".to_string(),
        endpoint: endpoint.to_string(),
    };
    let credentials = s3::creds::Credentials::new(
        Some("test-access-key"),
        Some("test-secret-key"),
        None,
        None,
        None,
    )
    .unwrap();
    s3::Bucket::new(BUCKET, region, credentials)
        .unwrap()
        .with_path_style()
}

/// Every row of `buffer`, as text.
fn buffer_lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
                .collect()
        })
        .collect()
}

/// `machine`'s local rows, read the way the interactive dashboard reads
/// them: from its session store only.
fn local_rows(machine: &Machine) -> (LocalBackend, SessionTree) {
    let store = LocalBackend::with_base_dir(machine.sessions());
    let mut tree = SessionTree::new();
    dashboard_data::refresh(&mut tree, &store).unwrap();
    (store, tree)
}

/// An event source with no input: each call waits out its timeout.
struct NoInput;

impl EventSource for NoInput {
    fn next_event(&mut self, timeout: Duration) -> std::io::Result<Option<Event>> {
        std::thread::sleep(timeout);
        Ok(None)
    }
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// PLAN step 2's check: on machine B the interactive dashboard lists A's
/// session in the needs-you band, blocked in the gate's state, its name cell
/// ending in `@machine-a`; B's own session, whose copy is in the bucket too,
/// shows once and unmarked.
#[test]
fn tui_lists_other_machines_blocked_session_marked_with_its_machine() {
    let (s3, a, b) = two_machines();
    b.start_blocked("mine");
    let ws = a.workspace();
    assert!(
        s3.object(&format!("{}/mine/summary.json", b.workspace()))
            .is_some(),
        "B's own session is in the bucket too"
    );

    // B's dashboard, in process: its own session store, the shared bucket.
    let (store, tree) = local_rows(&b);
    let feed = RemoteFeed::spawn(
        &bucket_at(&s3.endpoint()),
        dashboard_data::local_session_ids(&tree),
        RESCAN_INTERVAL,
    )
    .unwrap();
    let mut state = DashboardAppState::new(500);
    state.tree = tree;
    let mut sources = LoopSources {
        refresh_local: Box::new(move |tree| dashboard_data::refresh(tree, &store)),
        feed: Some(feed),
        sessions: None,
    };

    let remote_key = dashboard_data::remote_key(WF, &ws);
    let shutdown = AtomicBool::new(false);
    let started = Instant::now();
    let mut seen: Option<(Buffer, Vec<String>)> = None;
    let mut terminal = Terminal::new(TestBackend::new(78, 24)).unwrap();
    run_loop(
        &mut terminal,
        &mut state,
        &mut sources,
        &mut NoInput,
        &SystemClock,
        &shutdown,
        &mut |_, buffer, state| {
            if state.tree.sessions.contains_key(&remote_key) {
                let mut keys: Vec<String> = state.tree.sessions.keys().cloned().collect();
                keys.sort();
                seen = Some((buffer.clone(), keys));
                shutdown.store(true, Ordering::SeqCst);
            } else if started.elapsed() > Duration::from_secs(30) {
                shutdown.store(true, Ordering::SeqCst);
            }
        },
    )
    .unwrap();

    let (buffer, keys) = seen.expect("the feed brought A's session within 30 s");
    assert_eq!(
        keys,
        vec!["mine".to_string(), remote_key.clone()],
        "B's own session once, as its local row, and A's as a remote row"
    );
    let lines = buffer_lines(&buffer);
    // Both sessions are blocked at the gate, so the needs-you band holds
    // both list rows and the all-clear row is absent.
    assert!(
        !lines.iter().any(|l| l.contains("Nothing needs you")),
        "{lines:#?}"
    );
    let rows: Vec<&String> = lines[2..].iter().filter(|l| l.contains("check")).collect();
    assert_eq!(rows.len(), 2, "two rows in the band: {lines:#?}");
    let theirs = rows
        .iter()
        .find(|l| l.contains("@machine-a"))
        .unwrap_or_else(|| panic!("A's row marked @machine-a: {lines:#?}"));
    // The marker ends the name cell: the State column, showing the gate's
    // state, follows it.
    let after = theirs.split("@machine-a").nth(1).unwrap();
    assert!(
        after.trim_start().starts_with("check"),
        "the name cell ends in @machine-a, then the state: {theirs:?}"
    );
    assert!(theirs.contains('\u{25cf}'), "blocked glyph: {theirs:?}");
    let ours: Vec<_> = rows.iter().filter(|l| !l.contains('@')).collect();
    assert_eq!(ours.len(), 1, "B's own session once, unmarked: {lines:#?}");
}

/// A bucket endpoint that accepts connections and never answers: it reads
/// nothing and writes nothing, and holds every connection open.
fn never_answering() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (endpoint, accepted)
}

/// What the scripted input below has pressed and what the loop drew after.
#[derive(Default)]
struct Probe {
    /// When each key was handed to the loop, with the step it ended.
    pressed: Vec<(usize, Instant)>,
    /// The first frame drawn after each press, with when it was drawn.
    frames: Vec<(Instant, Vec<String>)>,
    step: usize,
}

/// Input that waits until the bucket has been dialled, then presses `j`
/// (focus the first row), waits for the frame after it, presses Tab three
/// times to reach the Session tab, waits for that frame, and quits.
struct Script {
    probe: Arc<Mutex<Probe>>,
    dialled: Arc<AtomicUsize>,
    started: Instant,
}

impl EventSource for Script {
    fn next_event(&mut self, timeout: Duration) -> std::io::Result<Option<Event>> {
        let mut probe = self.probe.lock().unwrap();
        let press = |probe: &mut Probe, code| {
            probe.step += 1;
            probe.pressed.push((probe.step, Instant::now()));
            Ok(Some(key(code)))
        };
        match probe.step {
            // The feed thread is in a request that will never answer.
            0 if self.dialled.load(Ordering::SeqCst) > 0 => press(&mut probe, KeyCode::Char('j')),
            0 if self.started.elapsed() > Duration::from_secs(10) => {
                panic!("the feed never dialled the bucket")
            }
            2..=4 => press(&mut probe, KeyCode::Tab),
            6 => press(&mut probe, KeyCode::Char('q')),
            _ => {
                drop(probe);
                std::thread::sleep(timeout);
                Ok(None)
            }
        }
    }
}

/// With a bucket that never answers, the frame after a key press reflects
/// it within 200 ms, the local rows stay drawn, and neither the feed nor the
/// Session tab's reads hold the input and render thread.
#[test]
fn tui_frame_after_a_key_press_arrives_within_200ms_when_the_bucket_never_answers() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let (endpoint, dialled) = never_answering();

    let (store, tree) = local_rows(&b);
    assert_eq!(tree.sessions.len(), 1);
    let bucket = bucket_at(&endpoint);
    let feed = RemoteFeed::spawn(
        &bucket,
        dashboard_data::local_session_ids(&tree),
        RESCAN_INTERVAL,
    )
    .unwrap();
    // The Session tab's reads go to the dead endpoint too.
    let cloud = koto::session::cloud::CloudBackend::with_parts(
        LocalBackend::with_base_dir(b.sessions()),
        bucket,
        b.workspace(),
    );
    let loader = SessionLoader::spawn(Arc::new(koto::session::Backend::Cloud(cloud))).unwrap();
    let mut state = DashboardAppState::new(500);
    state.tree = tree;
    let mut sources = LoopSources {
        refresh_local: Box::new(move |tree| dashboard_data::refresh(tree, &store)),
        feed: Some(feed),
        sessions: Some(loader),
    };

    let probe = Arc::new(Mutex::new(Probe::default()));
    let mut script = Script {
        probe: Arc::clone(&probe),
        dialled: Arc::clone(&dialled),
        started: Instant::now(),
    };
    let shutdown = AtomicBool::new(false);
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let recorder = Arc::clone(&probe);
    run_loop(
        &mut terminal,
        &mut state,
        &mut sources,
        &mut script,
        &SystemClock,
        &shutdown,
        &mut |at, buffer, _| {
            let mut probe = recorder.lock().unwrap();
            // The frame drawn right after `j` (step 1) and after the third
            // Tab (step 5).
            if probe.step == 1 || probe.step == 5 {
                probe.frames.push((at, buffer_lines(buffer)));
                probe.step += 1;
            }
        },
    )
    .unwrap();

    let probe = probe.lock().unwrap();
    assert!(dialled.load(Ordering::SeqCst) > 0, "the bucket was dialled");
    let pressed_at = |step| {
        probe
            .pressed
            .iter()
            .find(|(s, _)| *s == step)
            .map(|(_, at)| *at)
            .unwrap()
    };
    assert_eq!(probe.frames.len(), 2, "a frame after j and after the tabs");

    // After `j`: the first row is focused, so the detail pane is titled
    // with it; the local row is drawn.
    let (at, lines) = &probe.frames[0];
    let waited = at.duration_since(pressed_at(1));
    assert!(waited < Duration::from_millis(200), "frame after j took {waited:?}");
    assert!(lines[0].contains(" mine "), "focused: {:?}", lines[0]);
    assert!(
        lines.iter().any(|l| l.contains("gated") || l.contains("check")),
        "local row drawn: {lines:#?}"
    );

    // After the third Tab: the Session tab, still loading or loaded, and
    // the local row still drawn.
    let (at, lines) = &probe.frames[1];
    let waited = at.duration_since(pressed_at(5));
    assert!(waited < Duration::from_millis(200), "frame after Tab took {waited:?}");
    let text = lines.join("\n");
    assert!(
        text.contains("Loading session") || text.contains("State:"),
        "the Session tab: {text}"
    );
    assert!(text.contains("check"), "local row drawn: {text}");
}
