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
