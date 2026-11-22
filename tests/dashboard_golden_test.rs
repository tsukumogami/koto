//! `koto dashboard --once` against a store built at run time, compared with
//! a committed expected file.
//!
//! The store comes from `tests/support/golden_store.rs`: one session per
//! liveness class, an abandoned session, a parent with two children, a
//! session holding an event type koto doesn't know, a corrupt state file and
//! a staging directory, every timestamp an offset from this run's clock.
//!
//! The expected file, `tests/fixtures/dashboard-golden/expected.txt`, holds
//! the rows `--once` and `--once --all` print, cut to columns 1, 2, 4, 5, 6
//! and 8 (id, current state, status, intent, template, liveness) in printed
//! order, and the note each run prints on stderr. Columns 3 and 7 (elapsed
//! and idle) move with the clock, so they are left out, and the file holds
//! no date. Nothing here writes the file: when the dashboard's output
//! changes on purpose, the failure message prints the full actual file to
//! copy over it.
//!
//! The test pins what the dashboard shows before its session reader is moved
//! out of koto's crate, so each step of that move has to pass it with the
//! file unchanged.

#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_fs::TempDir;
use std::path::Path;
use std::time::SystemTime;

const EXPECTED_PATH: &str = "tests/fixtures/dashboard-golden/expected.txt";

/// The columns the comparison keeps, 1-based as the `--once` contract
/// numbers them, with the name a failure message gives each.
const KEPT_COLUMNS: [(usize, &str); 6] = [
    (1, "id"),
    (2, "current state"),
    (4, "status"),
    (5, "intent"),
    (6, "template"),
    (8, "liveness"),
];

const ONCE_SECTION: &str = "[once]";
const ALL_SECTION: &str = "[once --all]";
const STDERR_SECTION: &str = "[stderr]";

/// The expected file, split into its three sections.
struct Expected {
    once: Vec<String>,
    all: Vec<String>,
    stderr: Vec<String>,
}

fn read_expected() -> Expected {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(EXPECTED_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut expected = Expected {
        once: Vec::new(),
        all: Vec::new(),
        stderr: Vec::new(),
    };
    let mut section: Option<&mut Vec<String>> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        match line {
            ONCE_SECTION => section = Some(&mut expected.once),
            ALL_SECTION => section = Some(&mut expected.all),
            STDERR_SECTION => section = Some(&mut expected.stderr),
            _ => section
                .as_mut()
                .unwrap_or_else(|| panic!("{EXPECTED_PATH}: line before any section: {line:?}"))
                .push(line.to_string()),
        }
    }
    expected
}

/// A `--once` line cut to the kept columns, tab-separated.
fn project(line: &str) -> String {
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        fields.len(),
        10,
        "a --once line has ten tab-separated columns: {line:?}"
    );
    KEPT_COLUMNS
        .iter()
        .map(|(n, _)| fields[n - 1])
        .collect::<Vec<_>>()
        .join("\t")
}

/// One dashboard run's stdout lines and stderr lines.
struct Run {
    stdout: String,
    rows: Vec<String>,
    stderr: Vec<String>,
}

/// Builds the store in a fresh directory and runs `koto dashboard` there
/// with `args`.
fn run_dashboard(args: &[&str]) -> Run {
    let dir = TempDir::new().unwrap();
    let sessions = dir.path().join("sessions");
    golden_store::build(&sessions, SystemTime::now());

    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(dir.path())
        .env("KOTO_SESSIONS_BASE", &sessions)
        .env("HOME", dir.path())
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .arg("dashboard")
        .args(args);
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "koto dashboard {} exited {:?}; stderr:\n{stderr}",
        args.join(" "),
        output.status.code()
    );
    Run {
        rows: stdout.lines().map(project).collect(),
        stdout,
        stderr: stderr.lines().map(str::to_string).collect(),
    }
}

/// The full expected file a run of each view would produce, for a person
/// who means to replace the committed one.
fn render_actual(once: &Run, all: &Run) -> String {
    let mut out = String::new();
    out.push_str(ONCE_SECTION);
    out.push('\n');
    for row in &once.rows {
        out.push_str(row);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(ALL_SECTION);
    out.push('\n');
    for row in &all.rows {
        out.push_str(row);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(STDERR_SECTION);
    out.push('\n');
    for line in &once.stderr {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Compares one view's rows with the expected rows line for line. On the
/// first difference it fails naming the row's session id and, for each
/// column that differs, the expected and actual values.
fn compare_rows(view: &str, expected: &[String], actual: &[String], hint: &dyn Fn() -> String) {
    for (i, (want, got)) in expected.iter().zip(actual).enumerate() {
        if want == got {
            continue;
        }
        let want_cols: Vec<&str> = want.split('\t').collect();
        let got_cols: Vec<&str> = got.split('\t').collect();
        let mut diffs = Vec::new();
        for (j, (n, name)) in KEPT_COLUMNS.iter().enumerate() {
            let w = want_cols.get(j).copied().unwrap_or("<missing>");
            let g = got_cols.get(j).copied().unwrap_or("<missing>");
            if w != g {
                diffs.push(format!(
                    "  column {n} ({name}): expected {w:?}, actual {g:?}"
                ));
            }
        }
        panic!(
            "`koto dashboard {view}` row {} differs for session {:?} (actual row's session {:?}):\n{}\n\
             expected row: {want:?}\nactual row:   {got:?}\n\n{}",
            i + 1,
            want_cols[0],
            got_cols[0],
            diffs.join("\n"),
            hint()
        );
    }
    if expected.len() != actual.len() {
        let (more, side) = if expected.len() > actual.len() {
            (&expected[actual.len()..], "expected but not printed")
        } else {
            (&actual[expected.len()..], "printed but not expected")
        };
        panic!(
            "`koto dashboard {view}` printed {} rows, expected {}; rows {side}:\n{}\n\n{}",
            actual.len(),
            expected.len(),
            more.join("\n"),
            hint()
        );
    }
}

fn hint_for(once: &Run, all: &Run) -> String {
    format!(
        "If the dashboard's output changed on purpose, {EXPECTED_PATH} would read:\n{}",
        render_actual(once, all)
    )
}

/// Checks a run's stderr against the expected note and that neither the
/// corrupt session nor the staging directory reached stdout.
fn check_skipped(view: &str, run: &Run, expected: &Expected) {
    assert_eq!(
        run.stderr, expected.stderr,
        "`koto dashboard {view}` stderr differs from {EXPECTED_PATH}'s [stderr] section"
    );
    assert!(
        !run.stdout.contains(golden_store::CORRUPT_SESSION),
        "the corrupt session's id reached `koto dashboard {view}` stdout:\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains(golden_store::STAGING_DIR),
        "the staging directory's name reached `koto dashboard {view}` stdout:\n{}",
        run.stdout
    );
}

#[test]
fn once_default_view_matches_the_expected_file() {
    let expected = read_expected();
    let once = run_dashboard(&["--once"]);
    let all = run_dashboard(&["--once", "--all"]);
    let hint = || hint_for(&once, &all);
    compare_rows("--once", &expected.once, &once.rows, &hint);
    check_skipped("--once", &once, &expected);
}

#[test]
fn once_all_view_matches_the_expected_file() {
    let expected = read_expected();
    let once = run_dashboard(&["--once"]);
    let all = run_dashboard(&["--once", "--all"]);
    let hint = || hint_for(&once, &all);
    compare_rows("--once --all", &expected.all, &all.rows, &hint);
    check_skipped("--once --all", &all, &expected);
}

/// The expected file covers what the store is built to show, so a
/// regenerated file that lost a case fails here rather than passing.
#[test]
fn expected_file_covers_every_case_the_store_holds() {
    let expected = read_expected();
    let id = |row: &String| row.split('\t').next().unwrap().to_string();
    let col = |row: &String, j: usize| row.split('\t').nth(j).unwrap().to_string();

    for row in expected.once.iter().chain(&expected.all) {
        assert_eq!(
            row.split('\t').count(),
            KEPT_COLUMNS.len(),
            "{EXPECTED_PATH}: a row holds exactly the six kept columns: {row:?}"
        );
    }

    let tokens: Vec<String> = expected
        .once
        .iter()
        .chain(&expected.all)
        .map(|r| col(r, 5))
        .collect();
    for token in [
        "needs-you-blocked",
        "needs-you-failed",
        "needs-you-stalled",
        "active",
        "idle",
        "pending",
        "done",
    ] {
        assert!(
            tokens.iter().any(|t| t == token),
            "{EXPECTED_PATH}: no row shows liveness {token}"
        );
    }

    let once_ids: Vec<String> = expected.once.iter().map(id).collect();
    let all_ids: Vec<String> = expected.all.iter().map(id).collect();
    for receded in ["golden-abandoned", "golden-done"] {
        assert!(
            !once_ids.iter().any(|i| i == receded),
            "{EXPECTED_PATH}: the default view leaves {receded} out"
        );
        assert!(
            all_ids.iter().any(|i| i == receded),
            "{EXPECTED_PATH}: the --all view shows {receded}"
        );
    }
    for member in [
        "golden-parent",
        "golden-parent-child-a",
        "golden-parent-child-b",
    ] {
        assert!(
            once_ids.iter().any(|i| i == member),
            "{EXPECTED_PATH}: the parent and its two children are three rows; {member} is missing"
        );
    }
    let unknown = expected
        .once
        .iter()
        .find(|r| id(r) == "golden-unknown-event")
        .unwrap_or_else(|| panic!("{EXPECTED_PATH}: the unknown-event session is a row"));
    assert_eq!(
        col(unknown, 1),
        "review",
        "{EXPECTED_PATH}: the unknown-event session shows the state it reached"
    );

    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(EXPECTED_PATH)).unwrap();
    let has_date = text.as_bytes().windows(10).any(|w| {
        w[..4].iter().all(u8::is_ascii_digit)
            && w[4] == b'-'
            && w[5..7].iter().all(u8::is_ascii_digit)
            && w[7] == b'-'
            && w[8..].iter().all(u8::is_ascii_digit)
    });
    assert!(!has_date, "{EXPECTED_PATH} holds a calendar date");
    assert_eq!(
        expected.stderr,
        vec!["note: 1 unreadable session(s) skipped".to_string()],
        "{EXPECTED_PATH}: one unreadable session, the corrupt one; the staging directory isn't counted"
    );
}
