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
#[path = "support/host_session.rs"]
mod host_session;
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
        dashboard_data::local_view(&tree),
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
    let feed =
        RemoteFeed::spawn(&bucket, dashboard_data::local_view(&tree), RESCAN_INTERVAL).unwrap();
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
    assert!(
        waited < Duration::from_millis(200),
        "frame after j took {waited:?}"
    );
    assert!(lines[0].contains(" mine "), "focused: {:?}", lines[0]);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("gated") || l.contains("check")),
        "local row drawn: {lines:#?}"
    );

    // After the third Tab: the Session tab, still loading or loaded, and
    // the local row still drawn.
    let (at, lines) = &probe.frames[1];
    let waited = at.duration_since(pressed_at(5));
    assert!(
        waited < Duration::from_millis(200),
        "frame after Tab took {waited:?}"
    );
    let text = lines.join("\n");
    assert!(
        text.contains("Loading session") || text.contains("State:"),
        "the Session tab: {text}"
    );
    assert!(text.contains("check"), "local row drawn: {text}");
}

// ---------------------------------------------------------------------------
// Removed, imported, older-release and same-named sessions on machine B.
// ---------------------------------------------------------------------------

/// A template whose one state takes evidence and moves to its terminal
/// state, so a session can be stopped and imported.
const WORK: &str = r#"---
name: cross-work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// The retention settings an import of a just-stopped session needs.
const RETENTION: &[(&str, &str)] = &[
    ("KOTO_RETENTION_KEEP", "1s"),
    ("KOTO_RETENTION_IDLE_PERIOD", "1h"),
    ("KOTO_RETENTION_STALE_AFTER", "2h"),
];

impl Machine {
    /// `ok`, with `env` added to the machine's environment.
    fn ok_env(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = self.cmd(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let o = cmd.output().unwrap();
        let out = Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        assert_eq!(out.code, Some(0), "koto {:?}: {}", args, out.describe());
        out
    }
}

/// The bucket key of `file` in session `name` under `workspace`.
fn key_of(workspace: &str, name: &str, file: &str) -> String {
    format!("{workspace}/{name}/{file}")
}

fn state_key(workspace: &str, name: &str) -> String {
    key_of(workspace, name, &format!("koto-{name}.state.jsonl"))
}

/// Every row named `name`.
fn rows_named<'a>(rows: &'a [Vec<String>], name: &str) -> Vec<&'a Vec<String>> {
    rows.iter().filter(|r| r[0] == name).collect()
}

/// The dashboard read the bucket and nothing else: listings and GETs only,
/// every GET of a `summary.json` or `migrated.json`, and no request for a
/// key, or a listing of a prefix, under any `ctx/`.
fn assert_read_only(s3: &FakeS3) {
    let requests = s3.requests();
    assert!(!requests.is_empty(), "the bucket was read");
    for r in &requests {
        assert_eq!(r.method, "GET", "only listings and GETs: {r:?}");
        assert!(!r.key.contains("ctx/"), "nothing under ctx/: {r:?}");
        match &r.list_prefix {
            Some(prefix) => assert!(!prefix.contains("ctx/"), "no ctx/ listing: {r:?}"),
            None => assert!(
                r.key.ends_with("/summary.json") || r.key.ends_with("/migrated.json"),
                "only summaries and markers are fetched: {r:?}"
            ),
        }
    }
}

/// B's `--once` with `args` appended, its request log checked read-only.
fn once_on(s3: &FakeS3, b: &Machine, args: &[&str]) -> Out {
    s3.clear_requests();
    let mut all = vec!["dashboard", "--once"];
    all.extend_from_slice(args);
    let out = b.ok(&all);
    assert_read_only(s3);
    out
}

/// Draw B's interactive dashboard in process, its feed rescanning every
/// 200 ms, and call `step` with each frame's lines until it returns true.
/// Panics after 60 s.
fn drive_tui(
    s3: &FakeS3,
    b: &Machine,
    step: &mut dyn FnMut(&[String], &DashboardAppState) -> bool,
) {
    drive_tui_at(&s3.endpoint(), b, step)
}

/// [`drive_tui`] with the feed reading the bucket at `endpoint`.
fn drive_tui_at(
    endpoint: &str,
    b: &Machine,
    step: &mut dyn FnMut(&[String], &DashboardAppState) -> bool,
) {
    let (store, tree) = local_rows(b);
    let feed = RemoteFeed::spawn(
        &bucket_at(endpoint),
        dashboard_data::local_view(&tree),
        Duration::from_millis(200),
    )
    .unwrap();
    let mut state = DashboardAppState::new(500);
    state.tree = tree;
    let mut sources = LoopSources {
        refresh_local: Box::new(move |tree| dashboard_data::refresh(tree, &store)),
        feed: Some(feed),
        sessions: None,
    };
    let shutdown = AtomicBool::new(false);
    let started = Instant::now();
    let mut done = false;
    let mut last: (Vec<String>, Vec<String>, usize, usize) = Default::default();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    run_loop(
        &mut terminal,
        &mut state,
        &mut sources,
        &mut NoInput,
        &SystemClock,
        &shutdown,
        &mut |_, buffer, state| {
            let lines = buffer_lines(buffer);
            if step(&lines, state) {
                done = true;
                shutdown.store(true, Ordering::SeqCst);
            } else if started.elapsed() > Duration::from_secs(60) {
                shutdown.store(true, Ordering::SeqCst);
            }
            let mut keys: Vec<String> = state.tree.sessions.keys().cloned().collect();
            keys.sort();
            last = (
                lines,
                keys,
                state.tree.remote_unreadable,
                state.tree.remote_hidden,
            );
        },
    )
    .unwrap();
    assert!(
        done,
        "the TUI reached every step within 60 s; last frame, rows, unreadable \
         and hidden: {last:#?}"
    );
}

/// After `koto session cleanup` on A, B's `--once --all` no longer prints the
/// session and B's running TUI drops its row on a later snapshot. A session
/// directory holding a summary but no state file prints nothing.
#[test]
fn a_removed_session_drops_from_once_and_the_tui() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    let summary = s3.object(&key_of(&ws, WF, "summary.json")).unwrap();
    s3.put(&key_of(&ws, "orphan", "summary.json"), &summary);

    let rows = once_on(&s3, &b, &["--all"]).rows();
    assert!(row_of(&rows, WF, &ws).is_some(), "{rows:?}");
    assert!(rows_named(&rows, "orphan").is_empty(), "{rows:?}");

    let remote = dashboard_data::remote_key(WF, &ws);
    let mut cleaned = false;
    drive_tui(&s3, &b, &mut |_, state| {
        let shown = state.tree.sessions.contains_key(&remote);
        if !cleaned && shown {
            a.ok(&["session", "cleanup", WF]);
            assert!(s3.keys_under(&format!("{ws}/{WF}/")).is_empty());
            cleaned = true;
        }
        cleaned && !shown
    });

    let rows = once_on(&s3, &b, &["--all"]).rows();
    assert!(rows_named(&rows, WF).is_empty(), "{rows:?}");
    assert!(rows_named(&rows, "orphan").is_empty(), "{rows:?}");
}

/// A failed listing is never read as a removal: the TUI keeps A's row and
/// shows the failure on one status line, keeps every remote row when the
/// top-level listing fails, and on the first snapshot after the listing
/// answers again drops the row of the session that went meanwhile and the
/// status line with it.
#[test]
fn a_failed_listing_keeps_remote_rows_and_shows_one_status_line() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    let remote = dashboard_data::remote_key(WF, &ws);
    let status = |lines: &[String]| -> usize {
        lines
            .iter()
            .filter(|l| l.contains("\u{26a0} bucket:"))
            .count()
    };
    let has = |lines: &[String], text: &str| lines.iter().any(|l| l.contains(text));
    let mut phase = 0;
    drive_tui(&s3, &b, &mut |lines, state| {
        let shown = state.tree.sessions.contains_key(&remote);
        let failure = state.tree.remote_failure.clone();
        let top_level = failure.as_ref().is_some_and(|f| f.top_level);
        match phase {
            0 if shown => {
                assert_eq!(status(lines), 0, "{lines:#?}");
                s3.fail("LIST", &format!("{ws}/"), 403);
                phase = 1;
            }
            1 if failure.is_some() => {
                assert!(shown, "A's row stays while its listing fails");
                assert_eq!(status(lines), 1, "{lines:#?}");
                assert_eq!(
                    status_text(lines),
                    "\u{26a0} bucket: 1 workspace couldn't be read; the first: denied at \
                     127.0.0.1",
                    "{lines:#?}"
                );
                assert!(has(lines, "@machine-a"), "{lines:#?}");
                s3.clear_faults();
                s3.fail("LIST", "", 403);
                phase = 2;
            }
            2 if top_level => {
                assert!(shown, "every remote row stays");
                assert_eq!(status(lines), 1, "{lines:#?}");
                assert_eq!(
                    status_text(lines),
                    "\u{26a0} bucket: denied at 127.0.0.1: the bucket couldn't be listed",
                    "{lines:#?}"
                );
                // The session goes while the listing still fails, so the
                // first snapshot that reads the bucket again is the one that
                // must drop the row and the status line.
                a.ok(&["session", "cleanup", WF]);
                s3.clear_faults();
                phase = 3;
            }
            3 if failure.is_none() => {
                assert!(!shown, "the first clean snapshot drops the row");
                assert_eq!(status(lines), 0, "{lines:#?}");
                return true;
            }
            _ => {}
        }
        false
    });
}

/// After `koto session import` of A's session on B, B lists the copy once,
/// as its local row, never the source, and counts one hidden source on
/// stderr and on the TUI's tally line.
#[test]
fn an_imported_source_is_hidden_and_counted() {
    let s3 = FakeS3::start(BUCKET);
    let a = Machine::new(&s3, "machine-a");
    let b = Machine::new(&s3, "machine-b");
    std::fs::write(a.dir.join("work.md"), WORK).unwrap();
    std::fs::write(b.dir.join("work.md"), WORK).unwrap();
    a.ok(&["init", "imp", "--template", "work.md"]);
    a.ok_env(
        &[
            "next",
            "imp",
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
        RETENTION,
    );
    b.ok(&["template", "compile", "work.md"]);
    // Wait out the one-second retention.
    std::thread::sleep(Duration::from_millis(1300));
    let from = a.dir.to_string_lossy().into_owned();
    b.ok_env(&["session", "import", "imp", "--from", &from], RETENTION);
    let (wa, wb) = (a.workspace(), b.workspace());
    assert!(s3.object(&key_of(&wa, "imp", "migrated.json")).is_some());
    assert!(
        s3.object(&state_key(&wa, "imp")).is_some(),
        "the source stays"
    );

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let imp = rows_named(&rows, "imp");
    assert_eq!(imp.len(), 1, "the copy once: {rows:?}");
    assert_eq!(imp[0][8], "machine-b", "{rows:?}");
    assert_eq!(imp[0][9], wb, "{rows:?}");
    let hidden: Vec<_> = out
        .stderr
        .lines()
        .filter(|l| l.contains("hidden source(s)"))
        .collect();
    assert_eq!(hidden.len(), 1, "{}", out.stderr);
    assert!(
        hidden[0].contains("note: 1 hidden source(s)"),
        "{}",
        out.stderr
    );

    let source = dashboard_data::remote_key("imp", &wa);
    drive_tui(&s3, &b, &mut |lines, state| {
        state.tree.remote_hidden == 1
            && lines.iter().any(|l| l.contains("1 hidden source(s)"))
            && !state.tree.sessions.contains_key(&source)
    });
}

/// A source whose `migrated.json` names a copy that isn't in the bucket,
/// and that no local import names, is listed from its own summary.
#[test]
fn a_marker_naming_an_absent_copy_lists_the_source() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    s3.put(
        &key_of(&ws, WF, "migrated.json"),
        br#"{"schema":1,"target":{"session":"gone","session_id":"x","workspace":"/nowhere","prefix":"0000000000000000"},"machine_id":"m","migrated_at":"2026-10-31T00:00:00Z"}"#,
    );
    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let row = row_of(&rows, WF, &ws).unwrap_or_else(|| panic!("listed: {rows:?}"));
    assert_eq!(row[1], "check");
    assert_eq!(row[3], "blocked");
    assert!(!out.stderr.contains("hidden source"), "{}", out.stderr);
    let marker = key_of(&ws, WF, "migrated.json");
    assert!(
        s3.requests().iter().any(|r| r.key == marker),
        "the marker was read"
    );
}

/// A session pushed with no `summary.json` beside its state file, and one
/// whose summary is truncated mid-record, list as `unknown` with only the
/// listing's facts, are counted as unreadable, and read `@unknown` in the
/// TUI.
#[test]
fn a_session_without_a_readable_summary_lists_unknown() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    a.start_blocked("legacy");
    a.start_blocked("trunc");
    s3.remove(&key_of(&ws, "legacy", "summary.json"));
    let summary = s3.object(&key_of(&ws, "trunc", "summary.json")).unwrap();
    s3.put(
        &key_of(&ws, "trunc", "summary.json"),
        &summary[..summary.len() / 2],
    );

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    for name in ["legacy", "trunc"] {
        let row = row_of(&rows, name, &ws).unwrap_or_else(|| panic!("{name}: {rows:?}"));
        assert_eq!(row.len(), 10, "{row:?}");
        assert_eq!(row[3], "unknown", "{row:?}");
        for (i, column) in [
            (1, "current_state"),
            (4, "intent"),
            (5, "template"),
            (8, "machine"),
        ] {
            assert_eq!(row[i], "", "{name}'s {column} is empty: {row:?}");
        }
    }
    assert_eq!(row_of(&rows, WF, &ws).unwrap()[3], "blocked");
    assert!(
        out.stderr
            .contains("note: 2 unreadable session(s), 2 listed as unknown"),
        "{}",
        out.stderr
    );

    drive_tui(&s3, &b, &mut |lines, state| {
        state.tree.remote_unreadable == 2
            && ["legacy", "trunc"].iter().all(|name| {
                lines
                    .iter()
                    .any(|l| l.contains(name) && l.contains("@unknown"))
            })
            && lines.iter().any(|l| l.contains("2 unreadable session(s)"))
    });
}

/// A summary whose `state_etag` isn't the listed state file's ETag reads as
/// `unknown` once the state file is more than 60 seconds old, and from the
/// summary inside that window.
#[test]
fn a_stale_summary_reads_unknown_only_past_60_seconds() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    a.start_blocked("stale");
    a.start_blocked("fresh");
    let now = std::time::SystemTime::now();
    for (name, age) in [("stale", 120), ("fresh", 10)] {
        // An older release's push: the state changes, the summary doesn't.
        let key = state_key(&ws, name);
        let mut state = s3.object(&key).unwrap();
        state.extend_from_slice(b"\n");
        s3.put(&key, &state);
        s3.set_last_modified(&key, now - Duration::from_secs(age));
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let stale = row_of(&rows, "stale", &ws).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!((stale[1].as_str(), stale[3].as_str()), ("", "unknown"));
    let fresh = row_of(&rows, "fresh", &ws).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!((fresh[1].as_str(), fresh[3].as_str()), ("check", "blocked"));
    assert!(
        out.stderr
            .contains("note: 1 unreadable session(s), 1 listed as unknown"),
        "{}",
        out.stderr
    );
}

/// A session B holds a local copy of shows once, as its local row, even when
/// its bucket copy has no summary; a no-summary `review` under A's prefix
/// still prints beside B's own `review`.
#[test]
fn a_local_copy_without_a_summary_shows_once_beside_a_remote_namesake() {
    let (s3, a, b) = two_machines();
    let (wa, wb) = (a.workspace(), b.workspace());
    b.start_blocked("mine");
    s3.remove(&key_of(&wb, "mine", "summary.json"));
    a.start_blocked("review");
    s3.remove(&key_of(&wa, "review", "summary.json"));
    b.start_blocked("review");

    let rows = once_on(&s3, &b, &["--all"]).rows();
    let mine = rows_named(&rows, "mine");
    assert_eq!(mine.len(), 1, "{rows:?}");
    assert_eq!(
        (
            mine[0][3].as_str(),
            mine[0][8].as_str(),
            mine[0][9].as_str()
        ),
        ("blocked", "machine-b", wb.as_str())
    );
    assert_eq!(rows_named(&rows, "review").len(), 2, "{rows:?}");
    let ours = row_of(&rows, "review", &wb).unwrap();
    assert_eq!(
        (ours[3].as_str(), ours[8].as_str()),
        ("blocked", "machine-b")
    );
    let theirs = row_of(&rows, "review", &wa).unwrap();
    assert_eq!((theirs[3].as_str(), theirs[8].as_str()), ("unknown", ""));
}

/// With sessions named `review` on A and on B, B's `--once review` prints
/// both, each with its own machine and workspace.
#[test]
fn once_by_name_prints_both_machines_same_named_sessions() {
    let (s3, a, b) = two_machines();
    a.start_blocked("review");
    b.start_blocked("review");
    let rows = once_on(&s3, &b, &["review"]).rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let mut seen: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            assert_eq!((r.len(), r[0].as_str()), (10, "review"), "{r:?}");
            (r[8].clone(), r[9].clone())
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("machine-a".to_string(), a.workspace()),
            ("machine-b".to_string(), b.workspace()),
        ]
    );
}

// ---------------------------------------------------------------------------
// A slow, failing or read-only bucket (PLAN step 5).
// ---------------------------------------------------------------------------

/// The host of every endpoint these tests stand up.
const HOST: &str = "127.0.0.1";

/// An endpoint that refuses connections: a port bound and let go.
fn refusing() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    endpoint
}

impl Machine {
    /// `--once` with `args` appended, against the bucket at `endpoint`
    /// instead of the fake one, and how long it took.
    fn once_at(&self, endpoint: &str, args: &[&str]) -> (Out, Duration) {
        let mut all = vec!["dashboard", "--once"];
        all.extend_from_slice(args);
        let mut cmd = self.cmd(&all);
        cmd.env("KOTO_SESSION_CLOUD_ENDPOINT", endpoint);
        let started = Instant::now();
        let o = cmd.output().unwrap();
        let took = started.elapsed();
        let out = Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        };
        (out, took)
    }

    /// The names of the entries in this machine's session store.
    fn store_entries(&self) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(self.sessions())
            .map(|d| d.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default()
    }
}

/// The one line `out` wrote to stderr.
fn only_note(out: &Out) -> String {
    let lines: Vec<&str> = out.stderr.lines().collect();
    assert_eq!(lines.len(), 1, "one stderr line: {}", out.describe());
    lines[0].to_string()
}

/// The `--once` stderr line for a failed top-level listing of class
/// `class`.
fn top_level_note(class: &str) -> String {
    format!(
        "note: other machines' sessions weren't read: {class} at {HOST}: the bucket couldn't be \
         listed"
    )
}

/// `--once` printed B's own `mine` and nothing else, exited 0, and wrote
/// the one stderr line for a failed top-level listing of class `class`.
fn assert_local_rows_and_note(out: &Out, class: &str) {
    assert_eq!(out.code, Some(0), "{}", out.describe());
    let rows = out.rows();
    assert_eq!(rows.len(), 1, "B's local row only: {}", out.describe());
    assert_eq!(
        (rows[0][0].as_str(), rows[0][8].as_str()),
        ("mine", "machine-b")
    );
    assert_eq!(only_note(out), top_level_note(class));
}

/// The TUI's status lines in `lines`.
fn status_lines(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| l.contains("\u{26a0} bucket:"))
        .collect()
}

/// The text of the TUI's status line, which wraps inside the list pane:
/// its rows, from the one that starts it to the pane's bottom border,
/// each trimmed and joined with a space. Empty when no status line shows.
fn status_text(lines: &[String]) -> String {
    let Some(start) = lines.iter().position(|l| l.contains("\u{26a0} bucket:")) else {
        return String::new();
    };
    lines[start..]
        .iter()
        .take_while(|l| !l.starts_with('\u{2514}'))
        .filter_map(|l| l.split('\u{2502}').nth(1))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `lines` show B's local row, which is blocked at `check` and
/// carries no `@<machine>` marker.
fn shows_local_row(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|l| l.contains("check") && !l.contains('@'))
}

/// B's TUI, its feed reading `endpoint`, comes to show B's local row and
/// one status line holding `text`.
fn assert_tui_status(endpoint: &str, b: &Machine, text: &str) {
    drive_tui_at(endpoint, b, &mut |lines, _| {
        status_lines(lines).len() == 1
            && status_text(lines).contains(text)
            && shows_local_row(lines)
    });
}

/// With an endpoint that refuses connections, every request answered 403,
/// or every request answered 429 through koto's retries, B's `--once`
/// prints its local rows, writes one stderr line naming the class and the
/// host, and exits 0, and the TUI shows the local rows and one status line
/// with the same two facts. A failed top-level listing is the run's last
/// request: denied, it is asked once; throttled, it is asked koto's usual
/// three times and nothing follows. The TUI's feed, too, asks nothing but
/// top-level listings while they fail.
#[test]
fn a_refused_denied_or_throttled_bucket_leaves_local_rows_and_one_classed_line() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let top_level = |class: &str| format!("{class} at {HOST}: the bucket couldn't be listed");

    let refused = refusing();
    let (out, _) = b.once_at(&refused, &["--all"]);
    assert_local_rows_and_note(&out, "unreachable");
    assert_tui_status(&refused, &b, &top_level("unreachable"));

    for (status, class, asks) in [(403, "denied", 1), (429, "throttled", 3)] {
        s3.clear_faults();
        s3.fail_all(status);
        s3.clear_requests();
        let (out, _) = b.once_at(&s3.endpoint(), &["--all"]);
        assert_local_rows_and_note(&out, class);
        let requests = s3.requests();
        assert_eq!(requests.len(), asks, "{status}: {requests:?}");
        assert!(
            requests
                .iter()
                .all(|r| r.method == "GET" && r.list_prefix.as_deref() == Some("")),
            "only the top-level listing, nothing after it: {requests:?}"
        );
        s3.clear_requests();
        assert_tui_status(&s3.endpoint(), &b, &top_level(class));
        let requests = s3.requests();
        assert!(
            !requests.is_empty()
                && requests
                    .iter()
                    .all(|r| r.method == "GET" && r.list_prefix.as_deref() == Some("")),
            "the TUI's feed made only top-level listings, nothing after one: {requests:?}"
        );
    }
}

/// With an endpoint that accepts connections and never answers, B's
/// `--once` exits 0 within 15 seconds with one stderr line naming `timeout`
/// and the host, having asked once: a timed-out request isn't asked again.
#[test]
fn once_against_a_bucket_that_never_answers_exits_within_15s_with_one_timeout_line() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let (endpoint, dialled) = never_answering();

    let (out, took) = b.once_at(&endpoint, &["--all"]);
    println!("--once against a bucket that never answers took {took:?}");
    assert!(took < Duration::from_secs(15), "took {took:?}");
    assert_local_rows_and_note(&out, "timeout");
    assert_eq!(dialled.load(Ordering::SeqCst), 1, "asked once");
}

/// With one of three workspace prefixes answering 500 on its listing, B's
/// `--once` prints the other two prefixes' sessions and one stderr line
/// saying 1 workspace couldn't be read and naming the class; the TUI's
/// status line says the same and keeps the row it last had for that
/// workspace, and the line is gone after the first snapshot once the
/// listing answers again.
#[test]
fn one_failed_workspace_of_three_leaves_the_others_rows_and_one_line() {
    let s3 = FakeS3::start(BUCKET);
    let machines = ["machine-a1", "machine-a2", "machine-a3"].map(|name| Machine::new(&s3, name));
    for m in &machines {
        m.start_blocked(WF);
    }
    let b = Machine::new(&s3, "machine-b");
    let workspaces: Vec<String> = machines.iter().map(Machine::workspace).collect();
    let failing = workspaces[1].clone();
    let partial = format!("1 workspace couldn't be read; the first: unreachable at {HOST}");

    s3.fail("LIST", &format!("{failing}/"), 500);
    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    for (ws, listed) in workspaces.iter().zip([true, false, true]) {
        assert_eq!(row_of(&rows, WF, ws).is_some(), listed, "{ws}: {rows:?}");
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        only_note(&out),
        format!("note: other machines' sessions weren't all read: {partial}")
    );
    s3.clear_faults();

    let keys: Vec<String> = workspaces
        .iter()
        .map(|ws| dashboard_data::remote_key(WF, ws))
        .collect();
    let mut phase = 0;
    drive_tui(&s3, &b, &mut |lines, state| {
        let all_shown = keys.iter().all(|k| state.tree.sessions.contains_key(k));
        let failed = state.tree.remote_failure.is_some();
        match phase {
            0 if all_shown && !failed => {
                assert!(status_lines(lines).is_empty(), "{lines:#?}");
                s3.fail("LIST", &format!("{failing}/"), 500);
                phase = 1;
            }
            1 if failed => {
                assert!(all_shown, "the failed workspace's row stays");
                assert_eq!(status_lines(lines).len(), 1, "{lines:#?}");
                assert_eq!(
                    status_text(lines),
                    format!("\u{26a0} bucket: {partial}"),
                    "{lines:#?}"
                );
                s3.clear_faults();
                phase = 2;
            }
            2 if !failed => {
                assert!(all_shown);
                assert!(status_lines(lines).is_empty(), "{lines:#?}");
                return true;
            }
            _ => {}
        }
        false
    });
}

/// A session whose listing succeeds but whose `summary.json` GET answers
/// 500 prints with status `unknown` and is counted in the unreadable tally;
/// no workspace counts as unread.
#[test]
fn a_summary_get_that_fails_lists_unknown_and_is_counted() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    s3.fail("GET", &key_of(&ws, WF, "summary.json"), 500);

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let row = row_of(&rows, WF, &ws).unwrap_or_else(|| panic!("{}", out.describe()));
    assert_eq!((row[1].as_str(), row[3].as_str()), ("", "unknown"));
    assert_eq!(
        only_note(&out),
        "note: 1 unreadable session(s), 1 listed as unknown"
    );
}

/// With an empty bucket, B's `--once` prints its local rows and no stderr
/// line, and the TUI shows them with no status line after its scans.
#[test]
fn an_empty_bucket_shows_local_rows_and_no_status_line() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    for key in s3.keys() {
        s3.remove(&key);
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_eq!(rows.len(), 1, "{}", out.describe());
    assert_eq!(rows[0][0], "mine");
    assert!(out.stderr.trim().is_empty(), "{}", out.describe());

    s3.clear_requests();
    drive_tui(&s3, &b, &mut |lines, state| {
        // Three top-level listings: at least two scans' snapshots are in.
        let scans = s3
            .requests()
            .iter()
            .filter(|r| r.list_prefix.as_deref() == Some(""))
            .count();
        if scans < 3 {
            return false;
        }
        assert!(state.tree.remote_failure.is_none());
        assert!(status_lines(lines).is_empty(), "{lines:#?}");
        assert!(shows_local_row(lines), "{lines:#?}");
        true
    });
}

/// With every PUT and DELETE refused with 403, as keys that grant only list
/// and get would, B's `--once` and TUI list A's session, the request log
/// shows no write, and B's session store gains no entry.
#[test]
fn keys_that_grant_only_list_and_get_still_list_other_machines_sessions() {
    let (s3, a, b) = two_machines();
    let ws = a.workspace();
    s3.fail_writes(403);
    let before = b.store_entries();

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let row = row_of(&rows, WF, &ws).unwrap_or_else(|| panic!("{}", out.describe()));
    assert_eq!((row[1].as_str(), row[3].as_str()), ("check", "blocked"));
    assert!(out.stderr.trim().is_empty(), "{}", out.describe());

    s3.clear_requests();
    let remote = dashboard_data::remote_key(WF, &ws);
    drive_tui(&s3, &b, &mut |_, state| {
        state.tree.sessions.contains_key(&remote) && state.tree.remote_failure.is_none()
    });
    assert_read_only(&s3);
    assert_eq!(b.store_entries(), before, "B's session store is unchanged");
}

// --- PLAN step 6: the feed's ETag cache, its prefix budget and freshness ---

use koto::session::remote_feed::{FeedCache, LocalView, RemoteReader, RemoteSnapshot, RowStatus};

/// Satisfy the fixture's gate on `machine` and tick `name` past it, so its
/// state moves from `check` to `done` (kept, not cleaned up, at the terminal).
fn advance(machine: &Machine, name: &str) {
    let o = machine
        .cmd(&["context", "add", name, "go"])
        .write_stdin(b"go".to_vec())
        .output()
        .unwrap();
    assert!(o.status.success(), "context add: {o:?}");
    machine.ok(&["next", name, "--no-cleanup"]);
}

/// Every object GET in the request log (listings left out).
fn object_gets(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.list_prefix.is_none())
        .map(|r| r.key)
        .collect()
}

/// The workspace prefixes listed in the request log (the top level left out).
fn listed_prefixes(s3: &FakeS3) -> Vec<String> {
    s3.requests()
        .into_iter()
        .filter_map(|r| r.list_prefix)
        .filter_map(|p| p.strip_suffix('/').map(str::to_string))
        .collect()
}

/// The current state `snapshot` shows for `name` under `workspace`; `None`
/// when the row is missing or unknown.
fn shown_state(snapshot: &RemoteSnapshot, workspace: &str, name: &str) -> Option<String> {
    snapshot
        .rows
        .iter()
        .find(|r| r.workspace == workspace && r.name == name)
        .and_then(|r| match &r.status {
            RowStatus::Shown(s) => s.current_state.clone(),
            RowStatus::Unknown { .. } => None,
        })
}

/// One cached scan of the fake bucket as the feed thread runs it, its
/// request log starting empty.
fn cached_scan(s3: &FakeS3, reader: &RemoteReader, cache: &mut FeedCache) -> RemoteSnapshot {
    s3.clear_requests();
    reader.scan_cached(&LocalView::default(), cache)
}

/// With nothing changed on A, a cached scan after the first reads listings
/// only; after a tick on A changes one session, the next scan GETs that
/// session's summary and no other, and shows the new state.
#[test]
fn remote_feed_cache_reads_listings_only_until_a_session_changes() {
    let (s3, a, _b) = two_machines();
    a.start_blocked("wf2");
    let ws = a.workspace();
    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();

    let first = cached_scan(&s3, &reader, &mut cache);
    assert_eq!(shown_state(&first, &ws, WF).as_deref(), Some("check"));
    assert_eq!(
        object_gets(&s3).len(),
        2,
        "the first scan reads both summaries"
    );
    for _ in 0..2 {
        let again = cached_scan(&s3, &reader, &mut cache);
        assert!(object_gets(&s3).is_empty(), "{:?}", s3.requests());
        assert!(!listed_prefixes(&s3).is_empty(), "a later scan still lists");
        assert_eq!(shown_state(&again, &ws, WF).as_deref(), Some("check"));
        assert_eq!(shown_state(&again, &ws, "wf2").as_deref(), Some("check"));
    }

    advance(&a, WF);
    let after = cached_scan(&s3, &reader, &mut cache);
    assert_eq!(object_gets(&s3), vec![key_of(&ws, WF, "summary.json")]);
    assert_eq!(shown_state(&after, &ws, WF).as_deref(), Some("done"));
    assert_eq!(shown_state(&after, &ws, "wf2").as_deref(), Some("check"));
}

/// A summary whose GET failed, or whose body didn't parse, is asked for
/// again on the next scan, and shows once a GET succeeds.
#[test]
fn remote_feed_cache_asks_a_failed_or_unparsed_summary_again() {
    let (s3, a, _b) = two_machines();
    let ws = a.workspace();
    let key = key_of(&ws, WF, "summary.json");
    let good = s3.object(&key).unwrap();
    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();

    s3.fail("GET", &key, 500);
    for _ in 0..2 {
        let snapshot = cached_scan(&s3, &reader, &mut cache);
        assert_eq!(shown_state(&snapshot, &ws, WF), None);
        let gets = object_gets(&s3);
        assert!(
            !gets.is_empty() && gets.iter().all(|k| *k == key),
            "{gets:?}"
        );
    }
    s3.clear_faults();
    let snapshot = cached_scan(&s3, &reader, &mut cache);
    assert_eq!(shown_state(&snapshot, &ws, WF).as_deref(), Some("check"));

    s3.put(&key, b"{not json");
    for _ in 0..2 {
        let snapshot = cached_scan(&s3, &reader, &mut cache);
        assert_eq!(shown_state(&snapshot, &ws, WF), None);
        let gets = object_gets(&s3);
        assert!(
            !gets.is_empty() && gets.iter().all(|k| *k == key),
            "{gets:?}"
        );
    }
    s3.put(&key, &good);
    let snapshot = cached_scan(&s3, &reader, &mut cache);
    assert_eq!(shown_state(&snapshot, &ws, WF).as_deref(), Some("check"));
    cached_scan(&s3, &reader, &mut cache);
    assert!(object_gets(&s3).is_empty(), "a parsed summary is cached");
}

/// While A's workspace listing fails, the cache keeps A's session and no
/// GET is asked for it; after `koto session cleanup` on A, the next scan
/// drops the row and its cache entry.
#[test]
fn remote_feed_cache_keeps_a_failed_workspace_and_drops_a_cleaned_session() {
    let (s3, a, _b) = two_machines();
    let ws = a.workspace();
    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();
    cached_scan(&s3, &reader, &mut cache);
    assert!(cache.sessions(&ws).is_some_and(|s| s.contains_key(WF)));

    s3.fail("LIST", &format!("{ws}/"), 500);
    let failed = cached_scan(&s3, &reader, &mut cache);
    assert!(failed.keeps(&ws), "the holder keeps A's row");
    assert!(object_gets(&s3).is_empty(), "{:?}", s3.requests());
    assert!(cache.sessions(&ws).is_some_and(|s| s.contains_key(WF)));

    s3.clear_faults();
    a.ok(&["session", "cleanup", WF]);
    let cleaned = cached_scan(&s3, &reader, &mut cache);
    assert!(
        cleaned.rows.iter().all(|r| r.name != WF),
        "{:?}",
        cleaned.rows
    );
    assert!(!cache.sessions(&ws).is_some_and(|s| s.contains_key(WF)));
}

/// A 16-hex workspace prefix, the `i`th.
fn prefix_n(i: usize) -> String {
    format!("{:016x}", 0xa000_0000_0000_0000u64 + i as u64)
}

/// With 250 workspace prefixes, idle for two hours, no cached scan lists
/// more than 200; a prefix created after the first scan is listed on the
/// next one; every prefix is listed in any two consecutive scans; and a
/// prefix left out of a scan keeps its row.
#[test]
fn remote_feed_budget_lists_at_most_200_prefixes_and_rotates() {
    let s3 = FakeS3::start(BUCKET);
    let idle = std::time::SystemTime::now() - Duration::from_secs(2 * 3600);
    let seed = |ws: &str| {
        let key = state_key(ws, "s");
        s3.put(&key, b"{}\n");
        s3.set_last_modified(&key, idle);
    };
    let mut all: Vec<String> = (0..250).map(prefix_n).collect();
    all.iter().for_each(|ws| seed(ws));
    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();

    let mut listed = Vec::new();
    for scan in 0..3 {
        if scan == 1 {
            all.push(prefix_n(999));
            seed(&all[250]);
        }
        let snapshot = cached_scan(&s3, &reader, &mut cache);
        let these: std::collections::HashSet<String> = listed_prefixes(&s3).into_iter().collect();
        assert!(these.len() <= 200, "scan {scan} listed {}", these.len());
        if scan == 1 {
            assert!(these.contains(&all[250]), "a new prefix is listed at once");
        }
        if scan > 0 {
            assert_eq!(
                snapshot.rows.len(),
                all.len(),
                "a prefix left out keeps its row"
            );
        }
        listed.push(these);
    }
    for pair in listed.windows(2) {
        for ws in &all[..250] {
            assert!(
                pair[0].contains(ws) || pair[1].contains(ws),
                "{ws} unlisted two scans running"
            );
        }
    }
}

/// `--once` keeps nothing: two runs against an unchanged bucket each GET
/// the summary.
#[test]
fn remote_feed_once_reads_every_summary_on_every_run() {
    let (s3, a, b) = two_machines();
    let key = key_of(&a.workspace(), WF, "summary.json");
    for _ in 0..2 {
        once_on(&s3, &b, &["--all"]);
        assert_eq!(object_gets(&s3), vec![key.clone()]);
    }
}

/// B's TUI, its feed rescanning every 200 ms, shows A's new state within
/// one interval of the tick that PUT the summary, plus one scan's time.
#[test]
fn remote_feed_tui_shows_a_change_within_one_interval() {
    let (s3, a, b) = two_machines();
    let remote = dashboard_data::remote_key(WF, &a.workspace());
    let mut ticked: Option<Instant> = None;
    let mut elapsed = Duration::ZERO;
    drive_tui(&s3, &b, &mut |_, state| {
        let current = state
            .tree
            .sessions
            .get(&remote)
            .and_then(|s| s.current_state.clone());
        match ticked {
            None if current.as_deref() == Some("check") => {
                advance(&a, WF);
                ticked = Some(Instant::now());
                false
            }
            Some(at) if current.as_deref() == Some("done") => {
                elapsed = at.elapsed();
                true
            }
            _ => false,
        }
    });
    println!("A's change showed on B {elapsed:?} after its summary PUT");
    assert!(elapsed < Duration::from_millis(200 + 1000), "{elapsed:?}");
}

/// With 200 prefixes holding 2,000 sessions and every request answered
/// after 50 ms, B's `--once --all` prints all 2,000 rows within 30 s.
#[test]
fn remote_feed_once_at_200_prefixes_and_2000_sessions() {
    let (s3, a, b) = two_machines();
    let template: serde_json::Value = serde_json::from_slice(
        &s3.object(&key_of(&a.workspace(), WF, "summary.json"))
            .unwrap(),
    )
    .unwrap();
    for keyp in s3.keys_under(&format!("{}/", a.workspace())) {
        s3.remove(&keyp);
    }
    for p in 0..200 {
        let ws = prefix_n(p);
        for n in 0..10 {
            let name = format!("s{n}");
            let state = state_key(&ws, &name);
            s3.put(&state, b"{}\n");
            let mut summary = template.clone();
            summary["home"] = ws.clone().into();
            summary["name"] = name.clone().into();
            summary["session_id"] = format!("id-{p}-{n}").into();
            summary["state_etag"] = s3.etag_of(&state).unwrap().into();
            s3.put(
                &key_of(&ws, &name, "summary.json"),
                summary.to_string().as_bytes(),
            );
        }
    }
    s3.set_latency(Duration::from_millis(50));
    let started = Instant::now();
    let out = b.ok(&["dashboard", "--once", "--all"]);
    let elapsed = started.elapsed();
    println!("--once read 200 prefixes and 2,000 sessions in {elapsed:?}");
    let rows = out.rows();
    let shown = rows
        .iter()
        .filter(|r| r.get(1).is_some_and(|s| s == "check"))
        .count();
    assert_eq!(shown, 2000, "{}", out.describe());
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
}

// --- PLAN step 7: text and objects another machine wrote ---------------

use koto::cli::dashboard_render::render_frame;
use koto::cli::dashboard_state::DashboardTab;

/// A workspace prefix no machine here derives, for planted sessions.
const PLANTED: &str = "00000000000000aa";

/// The characters the bucket-text rule replaces, written out again here so
/// the tests don't take the rule from the code under test.
fn is_hazard(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1F}'
            | '\u{7F}'..='\u{9F}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200B}'..='\u{200F}'
            | '\u{061C}'
            | '\u{FEFF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Every hazard in `text`, as `U+XXXX`.
fn hazards(text: &str) -> Vec<String> {
    text.chars()
        .filter(|c| is_hazard(*c))
        .map(|c| format!("U+{:04X}", c as u32))
        .collect()
}

/// Every character of every range the rule names.
fn every_hazard() -> Vec<char> {
    let ranges: [(u32, u32); 10] = [
        (0x0, 0x1F),
        (0x7F, 0x7F),
        (0x80, 0x9F),
        (0x2028, 0x2029),
        (0x202A, 0x202E),
        (0x2066, 0x2069),
        (0x200B, 0x200F),
        (0x061C, 0x061C),
        (0xFEFF, 0xFEFF),
        (0xE0000, 0xE007F),
    ];
    ranges
        .iter()
        .flat_map(|&(lo, hi)| (lo..=hi).filter_map(char::from_u32))
        .collect()
}

/// Put a state file for `name` under `ws`, as another machine's push
/// would; returns its ETag.
fn plant_state(s3: &FakeS3, ws: &str, name: &str) -> String {
    let key = state_key(ws, name);
    s3.put(&key, b"{\"planted\":true}\n");
    s3.etag_of(&key).unwrap()
}

/// A schema-1 summary of a blocked session `name` under `ws` in state
/// `check`, naming the state file [`plant_state`] put there.
fn summary_of(s3: &FakeS3, ws: &str, name: &str) -> serde_json::Value {
    let etag = plant_state(s3, ws, name);
    serde_json::json!({
        "schema": 1,
        "session_id": format!("sid-{name}"),
        "name": name,
        "home": ws,
        "kind": null,
        "parent": null,
        "created_at": "2026-01-01T00:00:00Z",
        "template_name": "gated",
        "current_state": "check",
        "is_terminal": false,
        "is_blocked": true,
        "failed": false,
        "intent": null,
        "salient_var": null,
        "is_batch_task": false,
        "is_request_leg": false,
        // Recent, so the blocked row sits in the needs-you band.
        "last_event_at": fake_s3::rfc3339_millis(std::time::SystemTime::now()),
        "last_seq": 1,
        "state_etag": etag,
        "machine": "machine-a",
        "sources": [],
    })
}

/// Put `body` as the summary of `name` under `ws`.
fn put_summary(s3: &FakeS3, ws: &str, name: &str, body: &[u8]) {
    s3.put(&key_of(ws, name, "summary.json"), body);
}

/// Plant session `name` under `ws`: its state file and a summary with
/// `edit` applied.
fn plant(s3: &FakeS3, ws: &str, name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut summary = summary_of(s3, ws, name);
    edit(&mut summary);
    put_summary(s3, ws, name, summary.to_string().as_bytes());
}

/// B's interactive dashboard over one feed scan of the bucket, as the feed
/// thread runs it, drawn on a [`TestBackend`] with the receded set shown.
struct TuiView {
    state: DashboardAppState,
}

impl TuiView {
    fn scan(s3: &FakeS3, b: &Machine) -> Self {
        let (_store, mut tree) = local_rows(b);
        let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
        let snapshot =
            reader.scan_cached(&dashboard_data::local_view(&tree), &mut FeedCache::new());
        dashboard_data::apply_snapshot(&mut tree, &snapshot);
        let mut state = DashboardAppState::new(500);
        state.tree = tree;
        state.show_receded = true;
        TuiView { state }
    }

    /// The row key of the one remote row named `name` (as shown) under `ws`.
    /// A remote row is keyed by its name as listed, so the key is found by
    /// the name [`dashboard_data::row_name`] shows.
    fn key(&self, name: &str, ws: &str) -> String {
        let found: Vec<&String> = self
            .state
            .tree
            .sessions
            .iter()
            .filter(|(key, s)| {
                s.remote
                    && s.workspace.as_deref() == Some(ws)
                    && dashboard_data::row_name(key, s) == name
            })
            .map(|(key, _)| key)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{name:?}@{ws} among {:?}",
            self.state.tree.sessions.keys().collect::<Vec<_>>()
        );
        found[0].clone()
    }

    /// The frame's lines: the list, and with `focus` that row's Session tab.
    fn draw(&mut self, focus: Option<&str>, width: u16, height: u16) -> Vec<String> {
        match focus {
            Some(key) => {
                self.state.focused_id = Some(key.to_string());
                self.state.active_tab = DashboardTab::Session;
            }
            None => self.state.focused_id = None,
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render_frame(f, &self.state)).unwrap();
        buffer_lines(terminal.backend().buffer())
    }
}

/// Assert no hazard is anywhere in `lines`.
fn assert_no_hazard(lines: &[String], what: &str) {
    let text = lines.join("\n");
    let found = hazards(&text.replace('\n', ""));
    assert!(found.is_empty(), "{what}: {found:?} in {text}");
}

/// The tally line for `n` remote rows listed as unknown and no local one.
fn unknown_tally(n: usize) -> String {
    format!("note: {n} unreadable session(s), {n} listed as unknown")
}

/// Plan step 7, criterion 1: a name, state, intent and template holding a
/// tab, a newline, an ESC, a U+0085 and a U+2028 print on one ten-field
/// line with each replaced by one space, and still show the state; the TUI
/// list row and Session tab carry none of them; a no-summary session with
/// such a name prints the same way as `unknown`.
#[test]
fn bucket_text_with_controls_prints_on_one_line_with_each_spaced() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let mixed = |head: &str| format!("{head}\ta\nb\u{1B}c\u{85}d\u{2028}e");
    let spaced = |head: &str| format!("{head} a b c d e");
    let (shown, bare) = (mixed("n"), mixed("u"));
    plant(&s3, PLANTED, &shown, |s| {
        s["current_state"] = mixed("s").into();
        s["intent"] = mixed("i").into();
        s["template_name"] = mixed("t").into();
        s["session_id"] = mixed("id").into();
    });
    plant_state(&s3, PLANTED, &bare);

    let out = once_on(&s3, &b, &["--all"]);
    assert_eq!(out.stdout.lines().count(), 2, "{}", out.describe());
    let rows = out.rows();
    for row in &rows {
        assert_eq!(row.len(), 10, "ten fields: {row:?}");
        assert!(hazards(&row.join("")).is_empty(), "{row:?}");
    }
    let row = row_of(&rows, &spaced("n"), PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(row[1], spaced("s"), "{row:?}");
    assert_eq!(row[3], "blocked", "the state still shows: {row:?}");
    assert_eq!(row[4], spaced("i"), "{row:?}");
    assert_eq!(row[5], spaced("t"), "{row:?}");
    assert_eq!(row[8], "machine-a", "{row:?}");
    assert!(
        !out.stdout.contains("\\u{"),
        "no visible escape: {}",
        out.stdout
    );
    let unknown = row_of(&rows, &spaced("u"), PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(unknown[3], "unknown", "{unknown:?}");
    assert!(out.stderr.contains(&unknown_tally(1)), "{}", out.stderr);

    // The interactive dashboard, its feed on a short interval.
    drive_tui(&s3, &b, &mut |lines, state| {
        assert_no_hazard(lines, "the TUI's list");
        state.tree.remote_unreadable == 1
            && lines
                .iter()
                .any(|l| l.contains("i a b c") && l.contains(&spaced("s")))
            && lines.iter().any(|l| l.contains("u a b c"))
    });
    let mut tui = TuiView::scan(&s3, &b);
    let key = tui.key(&spaced("n"), PLANTED);
    let tab = tui.draw(Some(&key), 160, 40);
    assert_no_hazard(&tab, "the Session tab");
    let text = tab.join("\n");
    for fact in [
        format!("Session: {}", spaced("n")),
        format!("State: {}", spaced("s")),
        format!("Session id: {}", spaced("id")),
    ] {
        assert!(text.contains(&fact), "{fact:?} in {text}");
    }
    assert!(!text.contains("\\u{"), "no visible escape: {text}");
    let key = tui.key(&spaced("u"), PLANTED);
    assert_no_hazard(&tui.draw(Some(&key), 160, 40), "the unknown row's tab");
}

/// Plan step 7, criterion 2: each character of every range the rule names,
/// between two letters of an intent and a state, prints as those letters
/// with one space between them, and reaches no cell of the TUI; `café`
/// beside it is unchanged.
#[test]
fn every_hazard_character_prints_as_one_space_and_reaches_no_cell() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let all = every_hazard();
    assert_eq!(all.len(), 32 + 1 + 32 + 2 + 5 + 4 + 5 + 1 + 1 + 128);
    let name = |c: char| format!("c{:05x}", c as u32);
    for &c in &all {
        plant(&s3, PLANTED, &name(c), |s| {
            s["intent"] = format!("caf\u{e9} a{c}b").into();
            s["current_state"] = format!("s{c}t").into();
        });
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_eq!(rows.len(), all.len(), "{}", out.describe());
    for &c in &all {
        let row = row_of(&rows, &name(c), PLANTED)
            .unwrap_or_else(|| panic!("U+{:04X}: {rows:?}", c as u32));
        assert_eq!(row.len(), 10, "U+{:04X}: {row:?}", c as u32);
        assert_eq!(row[1], "s t", "U+{:04X}: {row:?}", c as u32);
        assert_eq!(row[4], "caf\u{e9} a b", "U+{:04X}: {row:?}", c as u32);
    }
    assert!(hazards(&out.stdout.replace(['\t', '\n'], "")).is_empty());

    let mut tui = TuiView::scan(&s3, &b);
    let list = tui.draw(None, 79, all.len() as u16 + 10);
    assert_no_hazard(&list, "the TUI's list");
    assert_eq!(
        list.iter()
            .filter(|l| l.contains("caf\u{e9} a b @machine-a"))
            .count(),
        all.len(),
        "{list:#?}"
    );
    for &c in &all {
        let key = tui.key(&name(c), PLANTED);
        let tab = tui.draw(Some(&key), 160, 30);
        assert_no_hazard(&tab, &format!("U+{:04X}'s Session tab", c as u32));
        let text = tab.join("\n");
        assert!(text.contains("Intent: caf\u{e9} a b"), "{text}");
        assert!(text.contains("State: s t"), "{text}");
    }
}

/// Plan step 7, criterion 3: CSI and OSC sequences in an intent leave no
/// ESC or BEL in `--once` or the TUI; in `created_at`, `kind`, `parent` and
/// `salient_var` they reach no row, and the Session tab shows `created_at`
/// with each one a space.
#[test]
fn terminal_sequences_in_bucket_text_reach_no_row() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let csi = "\u{1B}[2J";
    let osc = "\u{1B}]0;owned\u{7}";
    let hostile = format!("x{csi}y{osc}z");
    let created = format!("2026-01-01T00:00:00Z{csi}{osc}");
    for (name, intent) in [("loud", Some(hostile.clone())), ("quiet", None)] {
        plant(&s3, PLANTED, name, |s| {
            s["intent"] = intent.into();
            s["created_at"] = created.clone().into();
            s["kind"] = hostile.clone().into();
            s["parent"] = hostile.clone().into();
            s["salient_var"] = hostile.clone().into();
        });
    }

    let out = once_on(&s3, &b, &["--all"]);
    assert!(
        !out.stdout.contains('\u{1B}') && !out.stdout.contains('\u{7}'),
        "{:?}",
        out.stdout
    );
    let rows = out.rows();
    let loud = row_of(&rows, "loud", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(loud[4], "x [2Jy ]0;owned z", "{loud:?}");
    assert_eq!(loud[3], "blocked", "{loud:?}");
    assert!(row_of(&rows, "quiet", PLANTED).is_some(), "{rows:?}");

    drive_tui(&s3, &b, &mut |lines, _| {
        assert_no_hazard(lines, "the TUI's list");
        lines.iter().any(|l| l.contains("x [2Jy"))
    });
    let mut tui = TuiView::scan(&s3, &b);
    // The intent-less row's label folds in its salient variable.
    let list = tui.draw(None, 79, 20);
    assert_no_hazard(&list, "the TUI's list");
    assert!(
        list.iter().any(|l| l.contains("gated \u{b7} x [2Jy")),
        "{list:#?}"
    );
    for name in ["loud", "quiet"] {
        let key = tui.key(name, PLANTED);
        let tab = tui.draw(Some(&key), 160, 40);
        assert_no_hazard(&tab, name);
        let text = tab.join("\n");
        assert!(
            text.contains("Created: 2026-01-01T00:00:00Z [2J ]0;owned"),
            "{name}: {text}"
        );
    }
}

/// Plan step 7, criterion 4: a machine that breaks the machine-name rules
/// reads as no recorded machine, with the row's other facts from its
/// summary.
#[test]
fn a_machine_name_that_breaks_the_rules_reads_as_none_recorded() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let machines = [
        ("spaced", "bad name!".to_string()),
        ("literal", "unknown".to_string()),
        ("long", "m".repeat(65)),
        ("tabbed", "tab\there".to_string()),
        ("escaped", "esc\u{1B}here".to_string()),
    ];
    for (name, machine) in &machines {
        plant(&s3, PLANTED, name, |s| {
            s["machine"] = machine.clone().into();
            s["intent"] = format!("about {name}").into();
        });
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    for (name, _) in &machines {
        let row = row_of(&rows, name, PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
        assert_eq!(row.len(), 10, "{row:?}");
        assert_eq!(row[8], "", "{name}: no recorded machine: {row:?}");
        assert_eq!(row[1], "check", "{name}: {row:?}");
        assert_eq!(row[3], "blocked", "{name}: {row:?}");
        assert_eq!(row[4], format!("about {name}"), "{name}: {row:?}");
    }
    assert!(!out.stderr.contains("unreadable"), "{}", out.stderr);

    let mut tui = TuiView::scan(&s3, &b);
    let list = tui.draw(None, 79, 20);
    for (name, _) in &machines {
        let line = list
            .iter()
            .find(|l| l.contains(&format!("about {name}")))
            .unwrap_or_else(|| panic!("{name}: {list:#?}"));
        assert!(line.contains("@unknown"), "{name}: {line:?}");
        assert!(line.contains("check"), "{name}: {line:?}");
        let key = tui.key(name, PLANTED);
        let text = tui.draw(Some(&key), 160, 30).join("\n");
        assert!(text.contains("Machine: unknown"), "{name}: {text}");
        assert!(text.contains("State: check"), "{name}: {text}");
    }
}

/// `body` padded with trailing spaces, which JSON allows, to `len` bytes.
fn padded(body: &serde_json::Value, len: usize) -> Vec<u8> {
    let mut bytes = body.to_string().into_bytes();
    assert!(bytes.len() <= len);
    bytes.resize(len, b' ');
    bytes
}

/// Every GET of `key` in the request log.
fn gets_of(s3: &FakeS3, key: &str) -> Vec<fake_s3::Request> {
    s3.requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.list_prefix.is_none() && r.key == key)
        .collect()
}

/// Assert row `name` under [`PLANTED`] prints `unknown` with nothing from a
/// summary: no state, intent, template or machine.
fn assert_unknown_row(rows: &[Vec<String>], name: &str) {
    let row = row_of(rows, name, PLANTED).unwrap_or_else(|| panic!("{name}: {rows:?}"));
    assert_eq!(row.len(), 10, "{row:?}");
    assert_eq!(row[3], "unknown", "{name}: {row:?}");
    for (i, column) in [
        (1, "current_state"),
        (4, "intent"),
        (5, "template"),
        (8, "machine"),
    ] {
        assert_eq!(row[i], "", "{name}'s {column} is empty: {row:?}");
    }
}

/// Plan step 7, criterion 5: a summary listed at 64 KiB and one byte is
/// never fetched and lists as `unknown`, counted; one padded to exactly
/// 64 KiB shows from its fields.
#[test]
fn a_summary_listed_over_64_kib_is_never_fetched() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let mut over = summary_of(&s3, PLANTED, "over");
    over["intent"] = "too big".into();
    put_summary(&s3, PLANTED, "over", &padded(&over, 64 * 1024 + 1));
    let mut at = summary_of(&s3, PLANTED, "at");
    at["intent"] = "just fits".into();
    put_summary(&s3, PLANTED, "at", &padded(&at, 64 * 1024));

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_unknown_row(&rows, "over");
    let fits = row_of(&rows, "at", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        (fits[1].as_str(), fits[3].as_str(), fits[4].as_str()),
        ("check", "blocked", "just fits"),
        "{fits:?}"
    );
    assert!(out.stderr.contains(&unknown_tally(1)), "{}", out.stderr);
    assert!(
        gets_of(&s3, &key_of(PLANTED, "over", "summary.json")).is_empty(),
        "no GET of the oversized summary"
    );
    let read = gets_of(&s3, &key_of(PLANTED, "at", "summary.json"));
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(read[0].range.as_deref(), Some("bytes=0-65536"), "{read:?}");
    assert_eq!(read[0].body_sent, 64 * 1024, "{read:?}");
}

/// Plan step 7, criterion 6: a summary and a `migrated.json` replaced by
/// 64 MiB bodies between the listing and the GET are read no further than
/// 64 KiB and one byte; the summary lists as `unknown`, counted, and the
/// marker hides nothing.
#[test]
fn a_body_swapped_after_the_listing_is_read_no_further_than_the_bound() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    plant(&s3, PLANTED, "swapped", |s| {
        s["intent"] = "first body".into()
    });
    plant(&s3, PLANTED, "source", |s| {
        s["intent"] = "the source".into()
    });
    plant(&s3, PLANTED, "copy", |s| s["intent"] = "the copy".into());
    let marker = serde_json::json!({
        "prefix": PLANTED,
        "session": "copy",
        "session_id": "sid-copy",
    });
    let marker_key = key_of(PLANTED, "source", "migrated.json");
    s3.put(&marker_key, marker.to_string().as_bytes());
    // Both swapped once the workspace's listing has been answered: a valid
    // body followed by 64 MiB of padding, which a full read would accept.
    let huge = |body: &serde_json::Value| padded(body, 64 * 1024 * 1024);
    let summary_key = key_of(PLANTED, "swapped", "summary.json");
    let mut second = summary_of(&s3, PLANTED, "swapped");
    second["intent"] = "second body".into();
    let listing = format!("{PLANTED}/");
    s3.put_after("LIST", &listing, &summary_key, &huge(&second));
    s3.put_after("LIST", &listing, &marker_key, &huge(&marker));

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_unknown_row(&rows, "swapped");
    assert!(!out.stdout.contains("body"), "{}", out.stdout);
    let source = row_of(&rows, "source", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        source[4], "the source",
        "the marker hid nothing: {source:?}"
    );
    assert!(row_of(&rows, "copy", PLANTED).is_some(), "{rows:?}");
    assert!(out.stderr.contains(&unknown_tally(1)), "{}", out.stderr);
    assert!(!out.stderr.contains("hidden"), "{}", out.stderr);
    for key in [&summary_key, &marker_key] {
        let read = gets_of(&s3, key);
        assert_eq!(read.len(), 1, "{key}: {read:?}");
        assert_eq!(read[0].range.as_deref(), Some("bytes=0-65536"), "{read:?}");
        assert!(
            read[0].body_sent <= 64 * 1024 + 1,
            "{key}: {} body bytes sent",
            read[0].body_sent
        );
    }
}

/// Plan step 7, criterion 7: a field over its byte cap makes the summary
/// unreadable, with none of its text in the row; at the cap it shows; and
/// the caps count bytes.
#[test]
fn a_summary_field_over_its_byte_cap_reads_unknown() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let over_name = "n".repeat(257);
    let at_name = "a".repeat(256);
    let leak = |s: &mut serde_json::Value| {
        s["intent"] = "leaked intent".into();
        s["template_name"] = "leaked-template".into();
    };
    plant(&s3, PLANTED, "state-257", |s| {
        leak(s);
        s["current_state"] = "s".repeat(257).into();
    });
    plant(&s3, PLANTED, "template-257", |s| {
        s["intent"] = "leaked intent".into();
        s["template_name"] = "t".repeat(257).into();
    });
    plant(&s3, PLANTED, "salient-257", |s| {
        leak(s);
        s["salient_var"] = "v".repeat(257).into();
    });
    plant(&s3, PLANTED, &over_name, leak);
    plant(&s3, PLANTED, "intent-1025", |s| {
        s["template_name"] = "leaked-template".into();
        s["intent"] = "i".repeat(1025).into();
    });
    plant(&s3, PLANTED, "wide-state", |s| {
        leak(s);
        s["current_state"] = "\u{e9}".repeat(200).into();
    });
    plant(&s3, PLANTED, "state-256", |s| {
        s["current_state"] = "s".repeat(256).into();
        s["template_name"] = "t".repeat(256).into();
        s["salient_var"] = "v".repeat(256).into();
    });
    plant(&s3, PLANTED, &at_name, |s| {
        s["intent"] = "i".repeat(1024).into();
    });

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    for name in [
        "state-257",
        "template-257",
        "salient-257",
        over_name.as_str(),
        "intent-1025",
        "wide-state",
    ] {
        assert_unknown_row(&rows, name);
    }
    assert!(!out.stdout.contains("leaked"), "{}", out.stdout);
    assert!(!out.stdout.contains(&"s".repeat(257)), "no over-cap state");
    assert!(
        !out.stdout.contains(&"i".repeat(1025)),
        "no over-cap intent"
    );
    assert!(
        !out.stdout.contains(&"v".repeat(257)),
        "no over-cap variable"
    );
    assert!(out.stderr.contains(&unknown_tally(6)), "{}", out.stderr);

    let at = row_of(&rows, "state-256", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(at[1], "s".repeat(256), "{at:?}");
    assert_eq!(at[5], "t".repeat(256), "{at:?}");
    assert_eq!(at[3], "blocked", "{at:?}");
    let named = row_of(&rows, &at_name, PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(named[4], "i".repeat(1024), "{named:?}");
    assert_eq!(named[1], "check", "{named:?}");
}

/// Plan step 7, criterion 8: a summary that isn't JSON, isn't UTF-8, lacks
/// `is_blocked`, has an unknown field, carries schema 2, names another
/// session or workspace, or has an empty `session_id` lists as `unknown`,
/// counted, with none of its text in the row.
#[test]
fn a_malformed_summary_reads_unknown_with_none_of_its_text() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let leaky = |name: &str| {
        let mut s = summary_of(&s3, PLANTED, name);
        s["intent"] = format!("leaked from {name}").into();
        s["current_state"] = "leaked-state".into();
        s
    };
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let text = leaky("not-json").to_string();
    cases.push((
        "not-json",
        format!("{} trailing", text.trim_end_matches('}')).into_bytes(),
    ));
    let mut bytes = leaky("not-utf8").to_string().into_bytes();
    let at = bytes.windows(6).position(|w| w == b"leaked").unwrap();
    bytes[at] = 0xff;
    cases.push(("not-utf8", bytes));
    let mut s = leaky("no-blocked");
    s.as_object_mut().unwrap().remove("is_blocked");
    cases.push(("no-blocked", s.to_string().into_bytes()));
    let mut s = leaky("extra-field");
    s["owner"] = "someone".into();
    cases.push(("extra-field", s.to_string().into_bytes()));
    let mut s = leaky("schema-2");
    s["schema"] = 2.into();
    cases.push(("schema-2", s.to_string().into_bytes()));
    let mut s = leaky("other-name");
    s["name"] = "someone-else".into();
    cases.push(("other-name", s.to_string().into_bytes()));
    let mut s = leaky("other-home");
    s["home"] = "00000000000000bb".into();
    cases.push(("other-home", s.to_string().into_bytes()));
    let mut s = leaky("empty-id");
    s["session_id"] = "".into();
    cases.push(("empty-id", s.to_string().into_bytes()));
    for (name, body) in &cases {
        put_summary(&s3, PLANTED, name, body);
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_eq!(rows.len(), cases.len(), "{}", out.describe());
    for (name, _) in &cases {
        assert_unknown_row(&rows, name);
    }
    assert!(!out.stdout.contains("leaked"), "{}", out.stdout);
    assert!(
        out.stderr.contains(&unknown_tally(cases.len())),
        "{}",
        out.stderr
    );
}

/// A second planted workspace prefix.
const NAMED: &str = "00000000000000bb";

/// A summary's `sources` entry naming `name` under `prefix` by `session_id`.
fn source_entry(prefix: &str, name: &str, session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "relation": "imported",
        "prefix": prefix,
        "name": name,
        "session_id": session_id,
    })
}

/// The stderr lines counting hidden sources.
fn hidden_notes(out: &Out) -> Vec<&str> {
    out.stderr
        .lines()
        .filter(|l| l.contains("hidden source(s)"))
        .collect()
}

/// Press `code` on `state`, as the loop hands it a key.
fn press(state: &mut DashboardAppState, code: KeyCode) {
    state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// The index of the first of `lines` holding `needle`.
fn line_at(lines: &[String], needle: &str) -> Option<usize> {
    lines.iter().position(|l| l.contains(needle))
}

/// Plan step 7, criterion 9: a `sources` entry in X's summary naming Y by
/// `session_id` takes Y out of the default `--once`, `--needs-you` and the
/// TUI's needs-you band; Y still prints with its own state under `--all`
/// and shows after the `a` key, X is listed as before, and nothing is
/// counted hidden or unknown.
#[test]
fn a_sources_entry_only_recedes_the_row_it_names() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    plant(&s3, PLANTED, "x", |s| {
        s["intent"] = "intent-x".into();
        s["sources"] = serde_json::json!([source_entry(NAMED, "y", "sid-y")]);
    });
    plant(&s3, NAMED, "y", |s| s["intent"] = "intent-y".into());

    for args in [&[][..], &["--needs-you"][..]] {
        let out = once_on(&s3, &b, args);
        let rows = out.rows();
        let x = row_of(&rows, "x", PLANTED).unwrap_or_else(|| panic!("{args:?}: {rows:?}"));
        assert_eq!(
            (x[1].as_str(), x[3].as_str(), x[4].as_str(), x[8].as_str()),
            ("check", "blocked", "intent-x", "machine-a"),
            "{args:?}: {x:?}"
        );
        assert!(row_of(&rows, "y", NAMED).is_none(), "{args:?}: {rows:?}");
        assert_eq!(rows.len(), 1, "{args:?}: {rows:?}");
        assert!(hidden_notes(&out).is_empty(), "{args:?}: {}", out.stderr);
        assert!(!out.stderr.contains("unknown"), "{args:?}: {}", out.stderr);
    }

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let y = row_of(&rows, "y", NAMED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(y.len(), 10, "{y:?}");
    assert_eq!(
        (y[1].as_str(), y[3].as_str(), y[4].as_str(), y[8].as_str()),
        ("check", "blocked", "intent-y", "machine-a"),
        "Y with its own state, not unknown: {y:?}"
    );
    let x = row_of(&rows, "x", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!((x[3].as_str(), x[4].as_str()), ("blocked", "intent-x"));
    assert!(hidden_notes(&out).is_empty(), "{}", out.stderr);
    assert!(!out.stderr.contains("unknown"), "{}", out.stderr);

    // The interactive dashboard, its feed on a short interval: Y's row is
    // held but not drawn, and nothing is counted.
    let y_key = dashboard_data::remote_key("y", NAMED);
    drive_tui(&s3, &b, &mut |lines, state| {
        if !(state.tree.sessions.contains_key(&y_key) && line_at(lines, "intent-x").is_some()) {
            return false;
        }
        assert!(line_at(lines, "intent-y").is_none(), "{lines:#?}");
        assert_eq!(
            (state.tree.remote_hidden, state.tree.remote_unreadable),
            (0, 0)
        );
        assert!(!lines.iter().any(|l| l.contains("hidden source")));
        true
    });

    // The needs-you band holds X only; the `a` key brings Y back, in the
    // receded band after it.
    let mut tui = TuiView::scan(&s3, &b);
    tui.state.show_receded = false;
    let rows: Vec<String> = tui
        .state
        .visible_rows()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(rows, vec![dashboard_data::remote_key("x", PLANTED)]);
    let lines = tui.draw(None, 110, 20);
    assert!(line_at(&lines, "intent-x").is_some(), "{lines:#?}");
    assert!(line_at(&lines, "intent-y").is_none(), "{lines:#?}");
    press(&mut tui.state, KeyCode::Char('a'));
    let lines = tui.draw(None, 110, 20);
    let (x_at, y_at) = (line_at(&lines, "intent-x"), line_at(&lines, "intent-y"));
    assert!(x_at.is_some() && y_at > x_at, "{lines:#?}");
    let y_line = &lines[y_at.unwrap()];
    assert!(
        y_line.contains("@machine-a") && y_line.contains("check"),
        "{y_line:?}"
    );
}

/// Plan step 7, criterion 10: an entry naming a session B holds locally,
/// one naming its own summary's session and one with Y's prefix and name
/// but another `session_id` recede nothing; a session many summaries name
/// stays listed with no marker of its own; a forged marker in a session's
/// own directory, naming a copy in the same scan, hides it, counted on the
/// tally line and in the stderr note.
#[test]
fn a_sources_entry_recedes_no_local_self_or_other_id_row_and_only_a_marker_hides() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let wb = b.workspace();
    let pushed = s3.object(&key_of(&wb, "mine", "summary.json")).unwrap();
    let pushed: serde_json::Value = serde_json::from_slice(&pushed).unwrap();
    let mine_id = pushed["session_id"].as_str().unwrap().to_string();
    let sources = |entries: Vec<serde_json::Value>| serde_json::Value::Array(entries);
    plant(&s3, PLANTED, "x-local", |s| {
        s["sources"] = sources(vec![source_entry(&wb, "mine", &mine_id)]);
    });
    plant(&s3, PLANTED, "x-self", |s| {
        s["sources"] = sources(vec![source_entry(PLANTED, "x-self", "sid-x-self")]);
    });
    plant(&s3, PLANTED, "x-other-id", |s| {
        s["sources"] = sources(vec![source_entry(NAMED, "y", "not-sid-y")]);
    });
    plant(&s3, NAMED, "y", |_| {});
    // Named by five summaries, with no marker of its own.
    for i in 0..5 {
        plant(&s3, PLANTED, &format!("n{i}"), |s| {
            s["sources"] = sources(vec![source_entry(NAMED, "w", "sid-w")]);
        });
    }
    plant(&s3, NAMED, "w", |_| {});
    // A forged marker in h's own directory, naming a copy in the scan.
    plant(&s3, NAMED, "h", |_| {});
    plant(&s3, PLANTED, "h-copy", |_| {});
    let marker = serde_json::json!({
        "schema": 1,
        "target": {
            "session": "h-copy",
            "session_id": "sid-h-copy",
            "workspace": "/elsewhere",
            "prefix": PLANTED,
        },
        "machine_id": "m",
        "migrated_at": "2026-10-31T00:00:00Z",
    });
    s3.put(
        &key_of(NAMED, "h", "migrated.json"),
        marker.to_string().as_bytes(),
    );

    let out = once_on(&s3, &b, &[]);
    let rows = out.rows();
    let mine = rows_named(&rows, "mine");
    assert_eq!(mine.len(), 1, "B's local row once: {rows:?}");
    assert_eq!(
        (mine[0][1].as_str(), mine[0][8].as_str()),
        ("check", "machine-b")
    );
    assert_eq!(mine[0][9], wb, "{rows:?}");
    for (name, ws) in [
        ("x-local", PLANTED),
        ("x-self", PLANTED),
        ("x-other-id", PLANTED),
        ("y", NAMED),
        ("h-copy", PLANTED),
    ] {
        let row = row_of(&rows, name, ws).unwrap_or_else(|| panic!("{name}: {rows:?}"));
        assert_eq!(row[3], "blocked", "{name}: {row:?}");
    }
    assert!(row_of(&rows, "w", NAMED).is_none(), "w recedes: {rows:?}");
    assert!(row_of(&rows, "h", NAMED).is_none(), "h is hidden: {rows:?}");
    assert_eq!(
        hidden_notes(&out),
        vec!["note: 1 hidden source(s): imported into a copy that is listed"],
        "{}",
        out.stderr
    );

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let w = row_of(&rows, "w", NAMED).unwrap_or_else(|| panic!("w stays listed: {rows:?}"));
    assert_eq!((w[1].as_str(), w[3].as_str()), ("check", "blocked"));
    assert!(row_of(&rows, "h", NAMED).is_none(), "{rows:?}");
    assert_eq!(rows_named(&rows, "mine").len(), 1, "{rows:?}");
    assert_eq!(hidden_notes(&out).len(), 1, "{}", out.stderr);

    let (h_key, w_key) = (
        dashboard_data::remote_key("h", NAMED),
        dashboard_data::remote_key("w", NAMED),
    );
    drive_tui(&s3, &b, &mut |lines, state| {
        state.tree.remote_hidden == 1
            && lines.iter().any(|l| l.contains("1 hidden source(s)"))
            && state.tree.sessions.contains_key(&w_key)
            && !state.tree.sessions.contains_key(&h_key)
            && state.tree.sessions.contains_key("mine")
    });
}

/// Plan step 7, criterion 11: a summary with 17 `sources` entries prints as
/// `unknown`, counted, and recedes nothing; one with 16 shows from its
/// summary.
#[test]
fn a_summary_with_17_sources_reads_unknown_and_recedes_nothing() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let entries = |target: &str, n: usize| {
        let mut all = vec![source_entry(NAMED, target, &format!("sid-{target}"))];
        all.extend(
            (1..n).map(|i| source_entry(PLANTED, &format!("gone{i}"), &format!("sid-gone{i}"))),
        );
        serde_json::Value::Array(all)
    };
    plant(&s3, PLANTED, "seventeen", |s| {
        s["intent"] = "leaked from seventeen".into();
        s["sources"] = entries("t17", 17);
    });
    plant(&s3, PLANTED, "sixteen", |s| {
        s["intent"] = "intent-16".into();
        s["sources"] = entries("t16", 16);
    });
    plant(&s3, NAMED, "t17", |_| {});
    plant(&s3, NAMED, "t16", |_| {});

    let out = once_on(&s3, &b, &[]);
    let rows = out.rows();
    assert_unknown_row(&rows, "seventeen");
    assert!(!out.stdout.contains("leaked"), "{}", out.stdout);
    assert!(out.stderr.contains(&unknown_tally(1)), "{}", out.stderr);
    let sixteen = row_of(&rows, "sixteen", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(
        (
            sixteen[1].as_str(),
            sixteen[3].as_str(),
            sixteen[4].as_str()
        ),
        ("check", "blocked", "intent-16"),
        "{sixteen:?}"
    );
    let t17 = row_of(&rows, "t17", NAMED).unwrap_or_else(|| panic!("nothing receded: {rows:?}"));
    assert_eq!(t17[3], "blocked", "{t17:?}");
    assert!(row_of(&rows, "t16", NAMED).is_none(), "{rows:?}");

    let rows = once_on(&s3, &b, &["--all"]).rows();
    assert!(row_of(&rows, "t16", NAMED).is_some(), "{rows:?}");
}

/// Plan step 7, criterion 12: a summary whose `parent` names itself, and
/// two whose parents name each other, show each row in B's TUI, as they do
/// in `--once`.
#[test]
fn a_parent_cycle_still_shows_each_row_in_the_tui_as_in_once() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    for (name, parent) in [("loop", "loop"), ("ping", "pong"), ("pong", "ping")] {
        plant(&s3, PLANTED, name, |s| {
            s["parent"] = parent.into();
            s["intent"] = format!("i-{name}").into();
        });
    }
    let intents = ["i-loop", "i-ping", "i-pong"];

    let out = once_on(&s3, &b, &[]);
    let rows = out.rows();
    assert_eq!(rows.len(), 3, "{}", out.describe());
    for name in ["loop", "ping", "pong"] {
        let row = row_of(&rows, name, PLANTED).unwrap_or_else(|| panic!("{name}: {rows:?}"));
        assert_eq!(
            (row[1].as_str(), row[3].as_str()),
            ("check", "blocked"),
            "{row:?}"
        );
        assert_eq!(row[4], format!("i-{name}"));
    }

    drive_tui(&s3, &b, &mut |lines, state| {
        state.tree.sessions.len() == 3
            && intents.iter().all(|i| line_at(lines, i).is_some())
            && (state.tree.remote_hidden, state.tree.remote_unreadable) == (0, 0)
    });

    // Each row once, expanded or not.
    let mut tui = TuiView::scan(&s3, &b);
    tui.state.show_receded = false;
    for expand in [false, true] {
        if expand {
            let keys: Vec<String> = tui.state.tree.sessions.keys().cloned().collect();
            tui.state.expanded.extend(keys);
        }
        let lines = tui.draw(None, 110, 20);
        for intent in intents {
            assert_eq!(
                lines.iter().filter(|l| l.contains(intent)).count(),
                1,
                "{intent}, expanded {expand}: {lines:#?}"
            );
        }
    }
}

/// Every path under `dir`, depth first; empty when it doesn't exist.
fn paths_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        out.push(path.clone());
        if path.is_dir() {
            out.extend(paths_under(&path));
        }
    }
    out.sort();
    out
}

/// The log read nothing but listings of the top level and of `ws`, and
/// GETs of `name`'s own `summary.json` and `migrated.json`; no write.
fn assert_reads_only_its_own(s3: &FakeS3, ws: &str, name: &str) {
    assert_read_only(s3);
    let own = [
        key_of(ws, name, "summary.json"),
        key_of(ws, name, "migrated.json"),
    ];
    for r in s3.requests() {
        match &r.list_prefix {
            Some(prefix) => assert!(
                prefix.is_empty() || *prefix == format!("{ws}/"),
                "only the top level and {ws}/ are listed: {r:?}"
            ),
            None => assert!(own.contains(&r.key), "only its own objects: {r:?}"),
        }
    }
}

/// Input that presses `keys` one at a time once the log is `ready`, each
/// after the frame drawn for the one before it.
struct Presses {
    keys: Vec<KeyCode>,
    log: Arc<Mutex<PressLog>>,
}

#[derive(Default)]
struct PressLog {
    ready: bool,
    pressed: usize,
    awaiting: bool,
    /// The frame drawn after each press.
    frames: Vec<Vec<String>>,
}

impl EventSource for Presses {
    fn next_event(&mut self, timeout: Duration) -> std::io::Result<Option<Event>> {
        let mut log = self.log.lock().unwrap();
        if log.ready && !log.awaiting && log.pressed < self.keys.len() {
            let code = self.keys[log.pressed];
            log.pressed += 1;
            log.awaiting = true;
            return Ok(Some(key(code)));
        }
        drop(log);
        std::thread::sleep(timeout);
        Ok(None)
    }
}

/// Plan step 7, criterion 13: a `migrated.json` and a `parent` holding a
/// parent-directory path cause no GET outside the session's own summary
/// and marker, no file or directory in B's session store and no write;
/// Enter, Tab, `r`, `a`, `j` and `k` on the remote row add no write and
/// nothing to the store, and show only the Session tab's facts and the
/// other tabs' one line.
#[test]
fn a_path_in_a_marker_or_parent_reads_and_writes_nothing_else() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let up = ["..", "..", "x"].join("/");
    plant(&s3, PLANTED, "esc", |s| {
        s["parent"] = up.clone().into();
        s["intent"] = "i-esc".into();
    });
    let marker = serde_json::json!({
        "schema": 1,
        "target": {"session": up, "session_id": up, "workspace": up, "prefix": up},
        "machine_id": up,
        "migrated_at": "2026-10-31T00:00:00Z",
    });
    s3.put(
        &key_of(PLANTED, "esc", "migrated.json"),
        marker.to_string().as_bytes(),
    );
    let root = b.home.parent().unwrap().to_path_buf();
    let store = paths_under(&b.sessions());
    let no_x = |paths: &[PathBuf]| {
        let found: Vec<_> = paths
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "x"))
            .collect();
        assert!(found.is_empty(), "{found:?}");
    };

    let out = once_on(&s3, &b, &["--all"]);
    assert_reads_only_its_own(&s3, PLANTED, "esc");
    assert_eq!(
        gets_of(&s3, &key_of(PLANTED, "esc", "migrated.json")).len(),
        1,
        "the marker was read"
    );
    let rows = out.rows();
    assert_eq!(rows.len(), 1, "{}", out.describe());
    let esc = row_of(&rows, "esc", PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!((esc[3].as_str(), esc[4].as_str()), ("blocked", "i-esc"));
    assert!(hidden_notes(&out).is_empty(), "{}", out.stderr);
    assert_eq!(paths_under(&b.sessions()), store, "nothing in B's store");
    no_x(&paths_under(&root));

    // The interactive dashboard, with the Session tab's loader on the
    // bucket as `koto dashboard` builds it.
    s3.clear_requests();
    let before = paths_under(&root);
    let (local, tree) = local_rows(&b);
    let bucket = bucket_at(&s3.endpoint());
    let feed = RemoteFeed::spawn(
        &bucket,
        dashboard_data::local_view(&tree),
        Duration::from_millis(200),
    )
    .unwrap();
    let cloud = koto::session::cloud::CloudBackend::with_parts(
        LocalBackend::with_base_dir(b.sessions()),
        bucket,
        b.workspace(),
    );
    let loader = SessionLoader::spawn(Arc::new(koto::session::Backend::Cloud(cloud))).unwrap();
    let mut state = DashboardAppState::new(500);
    state.tree = tree;
    let mut sources = LoopSources {
        refresh_local: Box::new(move |tree| dashboard_data::refresh(tree, &local)),
        feed: Some(feed),
        sessions: Some(loader),
    };
    use KeyCode::{Char, Enter, Tab};
    let keys = vec![
        Char('j'),
        Enter,
        Tab,
        Tab,
        Tab,
        Char('r'),
        Char('a'),
        Char('j'),
        Char('k'),
        Enter,
    ];
    let count = keys.len();
    let log = Arc::new(Mutex::new(PressLog::default()));
    let mut input = Presses {
        keys,
        log: Arc::clone(&log),
    };
    let remote = dashboard_data::remote_key("esc", PLANTED);
    let shutdown = AtomicBool::new(false);
    let started = Instant::now();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    run_loop(
        &mut terminal,
        &mut state,
        &mut sources,
        &mut input,
        &SystemClock,
        &shutdown,
        &mut |_, buffer, state| {
            let mut log = log.lock().unwrap();
            let lines = buffer_lines(buffer);
            if log.awaiting {
                log.frames.push(lines);
                log.awaiting = false;
                if log.frames.len() == count {
                    shutdown.store(true, Ordering::SeqCst);
                }
            } else if !log.ready
                && state.tree.sessions.contains_key(&remote)
                && line_at(&lines, "i-esc").is_some()
            {
                log.ready = true;
            }
            if started.elapsed() > Duration::from_secs(60) {
                shutdown.store(true, Ordering::SeqCst);
            }
        },
    )
    .unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.frames.len(), count, "every key drew a frame");
    let one_line = "This session's detail lives on its own machine, machine-a.";
    let workspace = format!("Workspace: {PLANTED}");
    let session_facts = [
        "Machine: machine-a",
        workspace.as_str(),
        "Session: esc",
        "State: check",
        "Intent: i-esc",
        "Session id: sid-esc",
        "Context keys and history live on that machine.",
    ];
    // After Enter (Summary), then History and Remaining: the one line.
    for (i, tab) in [(1, "Summary"), (2, "History"), (3, "Remaining")] {
        let text = log.frames[i].join("\n");
        assert!(text.contains(one_line), "{tab}: {text}");
        assert!(!text.contains("Session id:"), "{tab}: {text}");
    }
    // The Session tab, then r, a, j, k and Enter on it: its facts only.
    for (i, frame) in log.frames.iter().enumerate().skip(4) {
        let text = frame.join("\n");
        for fact in session_facts {
            assert!(text.contains(fact), "frame {i}: {fact:?} in {text}");
        }
        assert!(!text.contains(one_line), "frame {i}: {text}");
        assert!(!text.contains("Loading session"), "frame {i}: {text}");
    }
    assert_reads_only_its_own(&s3, PLANTED, "esc");
    let after = paths_under(&root);
    assert_eq!(after, before, "nothing written under B's root");
    assert_eq!(paths_under(&b.sessions()), store, "nothing in B's store");
    no_x(&after);
}

// --- Plan step 7: times another machine wrote, and the listing ceilings ---

use std::time::SystemTime;

/// The seconds a `format_duration` token (`45s`, `2m5s`, `1h1m`) reads;
/// `None` for any other token.
fn duration_token(token: &str) -> Option<u64> {
    let (mut total, mut digits, mut any) = (0u64, String::new(), false);
    for c in token.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let n: u64 = digits.parse().ok()?;
        digits.clear();
        total += n * match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        any = true;
    }
    (any && digits.is_empty()).then_some(total)
}

/// The idle, in seconds, the TUI's list row labelled `name` and marked
/// with a machine shows; `None` when no such row is drawn.
fn tui_idle(lines: &[String], name: &str) -> Option<u64> {
    let marker = format!("{name} @");
    let line = lines.iter().find(|l| l.contains(&marker))?;
    line.split(|c: char| c.is_whitespace() || c == '\u{2502}')
        .rev()
        .find_map(duration_token)
}

/// How many top-level listings the request log holds: one a scan, while
/// the top level fits one page.
fn top_level_listings(s3: &FakeS3) -> usize {
    s3.requests()
        .iter()
        .filter(|r| r.list_prefix.as_deref() == Some(""))
        .count()
}

/// The stderr lines of `out` that hold `listing truncated`.
fn truncation_notes(out: &Out) -> Vec<&str> {
    out.stderr
        .lines()
        .filter(|l| l.contains("listing truncated"))
        .collect()
}

/// Plan step 7, criterion 14: a `last_event_at` an hour ahead prints idle
/// `0s` in `--once`, and in the TUI, across scans that find the summary's
/// ETag unchanged, reads at least 2 s on a frame 2 s after the first; one
/// 4 minutes ahead isn't clamped and reads `0s` on both frames. An unknown
/// row whose state file is listed an hour ahead ages the same way. A
/// `created_at` an hour ahead prints elapsed `0s`, and the Session tab
/// shows no time later than B's clock.
#[test]
fn a_far_future_time_is_clamped_to_the_reader_clock_and_the_row_still_ages() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let now = SystemTime::now();
    let hour = Duration::from_secs(3600);
    // Each labelled by its name: the list shows a row's intent.
    plant(&s3, PLANTED, "ahead", |s| {
        s["intent"] = "ahead".into();
        s["last_event_at"] = fake_s3::rfc3339_millis(now + hour).into();
    });
    plant(&s3, PLANTED, "near", |s| {
        s["intent"] = "near".into();
        s["last_event_at"] = fake_s3::rfc3339_millis(now + Duration::from_secs(240)).into();
    });
    plant(&s3, PLANTED, "born", |s| {
        s["created_at"] = fake_s3::rfc3339_millis(now + hour).into();
    });
    plant_state(&s3, PLANTED, "bare");
    s3.set_last_modified(&state_key(PLANTED, "bare"), now + hour);

    let out = once_on(&s3, &b, &["--all"]);
    let rows = out.rows();
    let row = |name: &str| row_of(&rows, name, PLANTED).unwrap_or_else(|| panic!("{rows:?}"));
    for name in ["ahead", "near", "bare"] {
        assert_eq!(row(name)[6], "0s", "{name}'s idle: {:?}", row(name));
    }
    assert_eq!(row("bare")[3], "unknown", "{:?}", row("bare"));
    assert_eq!(row("born")[2], "0s", "born's elapsed: {:?}", row("born"));

    // The TUI: a first frame with the rows, then one 2 s later with a scan
    // between them, which GETs no summary again.
    let summary_gets = |s3: &FakeS3| {
        object_gets(s3)
            .into_iter()
            .filter(|k| k.ends_with("/summary.json"))
            .count()
    };
    s3.clear_requests();
    let mut first: Option<(Instant, usize, usize)> = None;
    let mut seen = (None, None, None);
    drive_tui(&s3, &b, &mut |lines, _| {
        let idle = (
            tui_idle(lines, "ahead"),
            tui_idle(lines, "near"),
            tui_idle(lines, "bare"),
        );
        let (Some(_), Some(near), Some(_)) = idle else {
            return false;
        };
        match first {
            None => {
                assert_eq!(near, 0, "4 minutes ahead reads 0s: {lines:#?}");
                first = Some((Instant::now(), top_level_listings(&s3), summary_gets(&s3)));
                false
            }
            Some((at, scans, _)) => {
                seen = idle;
                at.elapsed() >= Duration::from_secs(2) && top_level_listings(&s3) > scans
            }
        }
    });
    let (_, _, gets) = first.unwrap();
    let (ahead, near, bare) = seen;
    assert!(ahead.unwrap() >= 2, "ahead's idle grew: {ahead:?}");
    assert!(bare.unwrap() >= 2, "bare's idle grew: {bare:?}");
    assert_eq!(near, Some(0), "4 minutes ahead still reads 0s");
    assert_eq!(
        summary_gets(&s3),
        gets,
        "the later scans found every summary's ETag unchanged"
    );

    let mut tui = TuiView::scan(&s3, &b);
    let key = tui.key("born", PLANTED);
    let tab = tui.draw(Some(&key), 160, 40);
    let created = tab
        .iter()
        .find_map(|l| l.split("Created: ").nth(1))
        .map(|v| v.trim_end_matches([' ', '\u{2502}']).to_string())
        .unwrap_or_else(|| panic!("{tab:#?}"));
    let clock = fake_s3::rfc3339_millis(SystemTime::now());
    assert!(
        created.as_str() <= clock.as_str(),
        "{created} after {clock}"
    );
}

/// B's TUI, the receded set shown (a row with no readable last event reads
/// as long idle): wait until the frame shows a row labelled with each of
/// `names`, then `j`, the frame after it, three Tabs, the frame after them,
/// and `q`. Each frame, with how long after its key press it was drawn.
fn press_once_rows_show(
    s3: &FakeS3,
    b: &Machine,
    names: &[String],
) -> Vec<(Duration, Vec<String>)> {
    let (store, tree) = local_rows(b);
    let feed = RemoteFeed::spawn(
        &bucket_at(&s3.endpoint()),
        dashboard_data::local_view(&tree),
        Duration::from_millis(200),
    )
    .unwrap();
    let mut state = DashboardAppState::new(500);
    state.tree = tree;
    state.show_receded = true;
    let mut sources = LoopSources {
        refresh_local: Box::new(move |tree| dashboard_data::refresh(tree, &store)),
        feed: Some(feed),
        sessions: None,
    };
    let probe = Arc::new(Mutex::new(Probe::default()));
    let ready = Arc::new(AtomicUsize::new(0));
    let mut script = Script {
        probe: Arc::clone(&probe),
        dialled: Arc::clone(&ready),
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
            let lines = buffer_lines(buffer);
            let mut probe = recorder.lock().unwrap();
            if probe.step == 0
                && names
                    .iter()
                    .all(|n| lines.iter().any(|l| l.contains(&format!("{n} @"))))
            {
                ready.store(1, Ordering::SeqCst);
            }
            if probe.step == 1 || probe.step == 5 {
                probe.frames.push((at, lines));
                probe.step += 1;
            }
        },
    )
    .unwrap();
    let probe = probe.lock().unwrap();
    let pressed = |step| probe.pressed.iter().find(|(s, _)| *s == step).unwrap().1;
    probe
        .frames
        .iter()
        .zip([1, 5])
        .map(|((at, lines), step)| (at.duration_since(pressed(step)), lines.clone()))
        .collect()
}

/// Plan step 7, criterion 15: a `last_event_at` or `created_at` holding a
/// year or an hour of `u64::MAX`, a fraction of two-byte characters or a
/// word prints on ten fields in `--once --all`, which exits 0 within 30 s
/// with no panic, and the TUI draws its frame after a key press within
/// 200 ms with those rows present.
#[test]
fn a_timestamp_that_cannot_be_read_neither_hangs_nor_panics_the_reader() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    let bad = [
        "18446744073709551615-01-01T00:00:00Z",
        "2026-01-01T18446744073709551615:00:00Z",
        "2026-01-01T00:00:00.\u{e9}\u{e9}Z",
        "yesterday",
    ];
    let mut names = Vec::new();
    for (i, value) in bad.iter().enumerate() {
        for field in ["last_event_at", "created_at"] {
            let name = format!("{}{i}", &field[..2]);
            plant(&s3, PLANTED, &name, |s| {
                s[field] = (*value).into();
                s["intent"] = name.clone().into();
            });
            names.push(name);
        }
    }

    let started = Instant::now();
    let out = b.koto(&["dashboard", "--once", "--all"]);
    let took = started.elapsed();
    println!("--once with unreadable timestamps took {took:?}");
    assert_eq!(out.code, Some(0), "{}", out.describe());
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert!(!out.stderr.contains("panicked"), "{}", out.describe());
    let rows = out.rows();
    for name in &names {
        let row = row_of(&rows, name, PLANTED).unwrap_or_else(|| panic!("{name}: {rows:?}"));
        assert_eq!(row.len(), 10, "{row:?}");
        assert_eq!(row[1], "check", "{row:?}");
    }

    let frames = press_once_rows_show(&s3, &b, &names);
    assert_eq!(frames.len(), 2, "a frame after j and after the tabs");
    for (waited, lines) in &frames {
        assert!(*waited < Duration::from_millis(200), "{waited:?}");
        let text = lines.join("\n");
        assert!(!text.contains("panicked"), "{text}");
    }
    let list = frames[0].1.join("\n");
    for name in &names {
        assert!(
            list.contains(&format!("{name} @machine-a")),
            "{name}: {list}"
        );
    }
}

/// Plan step 7, criteria 16 and 19: with 5,001 workspace prefixes beside
/// B's, `--once` lists 5,000, not the last in key order, writes one stderr
/// line holding `listing truncated` and prints B's local row and a remote
/// one; the TUI shows the notice on its status line beside its local row.
/// Once two prefixes go, the next snapshot has no notice, and the TUI's
/// clears.
#[test]
fn a_bucket_past_5000_prefixes_lists_5000_with_one_notice() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    for i in 0..5001 {
        s3.put(&format!("{}/pad", prefix_n(i)), b"x");
    }
    plant(&s3, &prefix_n(0), "far", |_| {});
    let mut every: Vec<String> = (0..5001).map(prefix_n).collect();
    every.push(b.workspace());
    every.sort();

    let started = Instant::now();
    let out = once_on(&s3, &b, &[]);
    println!("--once over 5,002 prefixes took {:?}", started.elapsed());
    let listed: std::collections::HashSet<String> = listed_prefixes(&s3).into_iter().collect();
    assert_eq!(listed.len(), 5000, "5,000 prefixes listed");
    assert!(
        !listed.contains(every.last().unwrap()),
        "the last one isn't"
    );
    assert!(top_level_listings(&s3) <= 100);
    assert_eq!(truncation_notes(&out).len(), 1, "{}", out.describe());
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.describe());
    let rows = out.rows();
    assert!(row_of(&rows, "mine", &b.workspace()).is_some(), "{rows:?}");
    assert!(row_of(&rows, "far", &prefix_n(0)).is_some(), "{rows:?}");

    assert_tui_status(&s3.endpoint(), &b, "listing truncated");

    // Down to 5,000 prefixes: the next snapshot, and the TUI, say nothing.
    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();
    assert!(cached_scan(&s3, &reader, &mut cache).truncated);
    s3.remove(&format!("{}/pad", prefix_n(5000)));
    s3.remove(&format!("{}/pad", prefix_n(4999)));
    assert!(!cached_scan(&s3, &reader, &mut cache).truncated);
    drive_tui(&s3, &b, &mut |lines, state| {
        assert!(
            !status_text(lines).contains("listing truncated"),
            "{lines:#?}"
        );
        !state.tree.sessions.values().all(|s| !s.remote) && shows_local_row(lines)
    });
}

/// Plan step 7, criteria 17 and 19: a prefix holding more than 100 pages is
/// listed 100 times, the rows from those pages and from another prefix
/// print beside B's own with the notice, and the session after the
/// hundredth page doesn't. Once the prefix fits 100 pages again, the next
/// snapshot has no notice.
#[test]
fn a_prefix_past_100_pages_lists_100_with_the_notice() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    plant(&s3, PLANTED, "a-first", |_| {});
    for i in 0..100_500 {
        s3.put(&key_of(PLANTED, "m-pad", &format!("p{i:06}")), b"");
    }
    plant(&s3, PLANTED, "z-last", |_| {});
    plant(&s3, NAMED, "other", |_| {});

    let started = Instant::now();
    let out = once_on(&s3, &b, &["--all"]);
    println!("--once over a 100-page prefix took {:?}", started.elapsed());
    let pages = listed_prefixes(&s3)
        .into_iter()
        .filter(|p| p == PLANTED)
        .count();
    assert_eq!(pages, 100, "100 listing requests for the padded prefix");
    let rows = out.rows();
    let state = |name: &str, ws: &str| row_of(&rows, name, ws).map(|r| r[1].clone());
    assert_eq!(
        state("a-first", PLANTED).as_deref(),
        Some("check"),
        "{rows:?}"
    );
    assert_eq!(state("other", NAMED).as_deref(), Some("check"), "{rows:?}");
    assert!(state("mine", &b.workspace()).is_some(), "{rows:?}");
    assert_eq!(
        state("z-last", PLANTED),
        None,
        "past the 100th page: {rows:?}"
    );
    assert_eq!(truncation_notes(&out).len(), 1, "{}", out.describe());
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.describe());

    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();
    let over = cached_scan(&s3, &reader, &mut cache);
    assert!(over.truncated);
    assert!(over.rows.iter().all(|r| r.name != "z-last"));
    for i in 0..600 {
        s3.remove(&key_of(PLANTED, "m-pad", &format!("p{i:06}")));
    }
    let under = cached_scan(&s3, &reader, &mut cache);
    assert!(!under.truncated, "the notice clears on the next snapshot");
    assert!(under.rows.iter().any(|r| r.name == "z-last"));
}

/// A workspace prefix that sorts after every one a machine derives.
const NEWEST: &str = "ffffffffffffffff";

/// Plan step 7, criterion 18: with 20,500 sessions, the 500 changed most
/// recently in the prefix last in key order, `--once --all` prints exactly
/// 20,000 remote rows, those 500 among them, the oldest prefix's 500 oldest
/// left out, plus B's local row, writes the notice, and GETs no summary of
/// a session it left out.
#[test]
fn the_session_ceiling_keeps_the_20000_most_recent_and_every_local_row() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let base = SystemTime::now() - Duration::from_secs(10 * 86400);
    for p in 0..20u64 {
        let ws = prefix_n(p as usize);
        for n in 0..1000u64 {
            let key = state_key(&ws, &format!("s{n:03}"));
            s3.put(&key, b"{}\n");
            s3.set_last_modified(
                &key,
                base + Duration::from_secs(p * 3600) - Duration::from_secs(n),
            );
        }
    }
    // Summaries no read should reach: the oldest prefix's oldest sessions.
    let oldest = prefix_n(0);
    for n in 990..1000 {
        put_summary(&s3, &oldest, &format!("s{n:03}"), b"{}");
    }
    for n in 0..500 {
        plant(&s3, NEWEST, &format!("n{n:03}"), |_| {});
    }

    let started = Instant::now();
    let out = once_on(&s3, &b, &["--all"]);
    println!("--once over 20,501 sessions took {:?}", started.elapsed());
    let rows = out.rows();
    let own = b.workspace();
    let remote: Vec<&Vec<String>> = rows.iter().filter(|r| r[9] != own).collect();
    assert_eq!(remote.len(), 20_000, "{}", out.stderr);
    let newest: Vec<_> = remote.iter().filter(|r| r[9] == NEWEST).collect();
    assert_eq!(newest.len(), 500);
    assert!(newest.iter().all(|r| r[1] == "check"), "{newest:?}");
    let mut kept: Vec<&str> = remote
        .iter()
        .filter(|r| r[9] == oldest)
        .map(|r| r[0].as_str())
        .collect();
    kept.sort();
    let expected: Vec<String> = (0..500).map(|n| format!("s{n:03}")).collect();
    assert_eq!(kept, expected, "the oldest prefix keeps its 500 newest");
    for p in 1..20 {
        let ws = prefix_n(p);
        assert_eq!(remote.iter().filter(|r| r[9] == ws).count(), 1000, "{ws}");
    }
    assert!(row_of(&rows, "mine", &own).is_some(), "B's local row");
    assert_eq!(truncation_notes(&out).len(), 1, "{}", out.stderr);
    let gets: Vec<String> = object_gets(&s3)
        .into_iter()
        .filter(|k| k.ends_with("/summary.json"))
        .collect();
    assert!(gets.len() <= 20_000, "{}", gets.len());
    assert!(
        gets.iter()
            .all(|k| !k.starts_with(&format!("{oldest}/s99"))),
        "no summary of a session left out is read"
    );
}

/// Plan step 7, criterion 18 in the TUI: 250 prefixes of 81 sessions each,
/// more than the 200-prefix budget lists in one scan, over enough scans to
/// list every prefix, never put more than 20,000 remote rows in B's tree,
/// which comes to hold exactly 20,000 with the notice on the status line
/// and B's local row.
#[test]
fn the_tui_never_holds_more_than_20000_remote_rows_across_a_rotation() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let idle = SystemTime::now() - Duration::from_secs(2 * 3600);
    let all: Vec<String> = (0..250).map(prefix_n).collect();
    for (p, ws) in all.iter().enumerate() {
        for n in 0..81u64 {
            let key = state_key(ws, &format!("s{n:02}"));
            s3.put(&key, b"{}\n");
            s3.set_last_modified(&key, idle - Duration::from_secs(p as u64 * 100 + n));
        }
    }
    s3.clear_requests();
    let mut most = 0usize;
    let started = Instant::now();
    drive_tui(&s3, &b, &mut |lines, state| {
        let remote = state.tree.sessions.values().filter(|s| s.remote).count();
        assert!(remote <= 20_000, "{remote} remote rows");
        most = most.max(remote);
        top_level_listings(&s3) >= 3
            && remote == 20_000
            && status_text(lines).contains("listing truncated")
            && shows_local_row(lines)
    });
    println!(
        "the TUI rotated through 250 prefixes and 20,250 sessions in {:?}",
        started.elapsed()
    );
    let listed: std::collections::HashSet<String> = listed_prefixes(&s3).into_iter().collect();
    assert!(
        all.iter().all(|ws| listed.contains(ws)),
        "every prefix listed"
    );
    assert_eq!(most, 20_000);
}

/// Plan step 7, criterion 19: a bucket below every ceiling, here 250
/// prefixes, which only passes the TUI's 200-prefix budget, shows no
/// notice in `--once`, in any cached snapshot or on any frame of the TUI.
#[test]
fn a_bucket_below_every_ceiling_shows_no_truncation_notice() {
    let s3 = FakeS3::start(BUCKET);
    let b = Machine::new(&s3, "machine-b");
    b.start_blocked("mine");
    let idle = SystemTime::now() - Duration::from_secs(2 * 3600);
    for i in 0..250 {
        let key = state_key(&prefix_n(i), "s");
        s3.put(&key, b"{}\n");
        s3.set_last_modified(&key, idle);
    }
    let out = once_on(&s3, &b, &["--all"]);
    assert!(truncation_notes(&out).is_empty(), "{}", out.describe());
    assert_eq!(out.rows().len(), 251, "{}", out.describe());

    let reader = RemoteReader::new(&bucket_at(&s3.endpoint())).unwrap();
    let mut cache = FeedCache::new();
    for _ in 0..3 {
        assert!(!cached_scan(&s3, &reader, &mut cache).truncated);
    }
    s3.clear_requests();
    drive_tui(&s3, &b, &mut |lines, state| {
        assert!(!lines.join("").contains("listing truncated"), "{lines:#?}");
        top_level_listings(&s3) >= 3
            && state.tree.sessions.values().filter(|s| s.remote).count() == 250
            && shows_local_row(lines)
    });
}
