//! `koto-session-list` against `koto dashboard` and `koto context get`, and
//! a hundred of its reads against a cloud host.
//!
//! `koto-session-list` (the workspace member of that name) reads koto's
//! session store through `koto-session-reader` alone. These tests run the
//! built program, so each one is `#[ignore]`d: validate.yml's Session List
//! Checks job builds it in release mode, puts its path in
//! `SESSION_LIST_BIN` and runs this file with `--ignored`. Locally:
//!
//! ```text
//! cargo build --release --locked -p koto-session-list
//! SESSION_LIST_BIN=target/release/koto-session-list \
//!   cargo test --locked --release --test koto_session_list_test -- --ignored
//! ```
//!
//! A test run with the variable unset, or naming something that isn't an
//! executable file, fails naming the variable rather than passing quietly.
//!
//! The parity tests build `tests/support/golden_store.rs`'s store and run
//! both programs on it. The rows must agree on columns 1, 2, 4, 5, 6 and 8
//! (id, current state, status, intent, template, liveness) in printed order,
//! stderr must agree, and the rows must also match
//! `tests/fixtures/dashboard-golden/expected.txt`, so a change to both
//! programs at once still fails. Columns 3 and 7 (elapsed and idle) move
//! with the clock between the two runs and are left out, as the golden test
//! leaves them out.
//!
//! The hundred-read test runs the program against a cloud host whose bucket
//! is a FakeS3, and checks that the host's koto root is byte-for-byte the
//! same afterwards and that the bucket saw no request at all.

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// The variable naming the built program.
const BIN_ENV: &str = "SESSION_LIST_BIN";

const EXPECTED_PATH: &str = "tests/fixtures/dashboard-golden/expected.txt";

/// The `--once` columns both programs print and the comparison keeps,
/// 1-based as the `--once` contract numbers them.
const KEPT_COLUMNS: [(usize, &str); 6] = [
    (1, "id"),
    (2, "current state"),
    (4, "status"),
    (5, "intent"),
    (6, "template"),
    (8, "liveness"),
];

/// The built program, from [`BIN_ENV`].
fn session_list_bin() -> PathBuf {
    let Some(value) = std::env::var_os(BIN_ENV) else {
        panic!(
            "{BIN_ENV} is unset; build the program with \
             `cargo build --release --locked -p koto-session-list` and set {BIN_ENV} \
             to target/release/koto-session-list"
        );
    };
    let path = PathBuf::from(value);
    let meta =
        std::fs::metadata(&path).unwrap_or_else(|e| panic!("{BIN_ENV}={}: {e}", path.display()));
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = true;
    assert!(
        meta.is_file() && executable,
        "{BIN_ENV}={} is not an executable file",
        path.display()
    );
    std::fs::canonicalize(&path).unwrap()
}

/// What one run printed.
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn of(cmd: &mut Command) -> Out {
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8(o.stdout).unwrap(),
            stderr: String::from_utf8(o.stderr).unwrap(),
        }
    }

    fn ok(self, what: &str) -> Out {
        assert_eq!(
            self.code,
            Some(0),
            "{what} exited {:?}\nstdout:\n{}\nstderr:\n{}",
            self.code,
            self.stdout,
            self.stderr
        );
        self
    }

    fn stderr_lines(&self) -> Vec<String> {
        self.stderr.lines().map(str::to_string).collect()
    }
}

// ---------------------------------------------------------------------------
// The golden store
// ---------------------------------------------------------------------------

/// A golden store in a directory of its own, with the environment both
/// programs read it under.
struct Golden {
    dir: tempfile::TempDir,
}

impl Golden {
    fn build() -> Golden {
        let dir = tempfile::TempDir::new().unwrap();
        golden_store::build(&dir.path().join("sessions"), SystemTime::now());
        Golden { dir }
    }

    fn sessions(&self) -> PathBuf {
        self.dir.path().join("sessions")
    }

    fn env(&self, cmd: &mut Command) {
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("KOTO_HOME")
            .current_dir(self.dir.path())
            .env("KOTO_SESSIONS_BASE", self.sessions())
            .env("HOME", self.dir.path())
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"));
    }

    fn koto(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd);
        Out::of(cmd.args(args)).ok(&format!("koto {}", args.join(" ")))
    }

    fn koto_with_stdin(&self, args: &[&str], stdin: &str) -> Out {
        use std::io::Write;
        use std::process::Stdio;
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd);
        let mut child = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        Out {
            code: o.status.code(),
            stdout: String::from_utf8(o.stdout).unwrap(),
            stderr: String::from_utf8(o.stderr).unwrap(),
        }
        .ok(&format!("koto {}", args.join(" ")))
    }

    fn list_cmd(&self, args: &[&str]) -> Out {
        let mut cmd = Command::new(session_list_bin());
        // Only what koto's own rule reads: the program gets nothing else.
        cmd.env_clear();
        self.env(&mut cmd);
        Out::of(cmd.args(args))
    }

    fn list(&self, args: &[&str]) -> Out {
        self.list_cmd(args)
            .ok(&format!("koto-session-list {}", args.join(" ")))
    }
}

/// `line` cut to [`KEPT_COLUMNS`], after checking it has `width` columns.
fn project(program: &str, line: &str, width: usize) -> Vec<String> {
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        fields.len(),
        width,
        "a {program} line has {width} tab-separated columns: {line:?}"
    );
    KEPT_COLUMNS
        .iter()
        .map(|(n, _)| fields[n - 1].to_string())
        .collect()
}

/// The `[once]` or `[once --all]` rows of the expected file.
fn expected_rows(section: &str) -> Vec<Vec<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(EXPECTED_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut current = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            current = Some(line);
            continue;
        }
        if current == Some(section) {
            rows.push(line.split('\t').map(str::to_string).collect());
        }
    }
    assert!(!rows.is_empty(), "{EXPECTED_PATH} has no {section} rows");
    rows
}

/// Fails naming `view`, the row's session and each column that differs.
fn compare(view: &str, left: (&str, &[Vec<String>]), right: (&str, &[Vec<String>])) {
    let (lname, lrows) = left;
    let (rname, rrows) = right;
    for (i, (l, r)) in lrows.iter().zip(rrows).enumerate() {
        if l == r {
            continue;
        }
        let diffs: Vec<String> = KEPT_COLUMNS
            .iter()
            .zip(l.iter().zip(r))
            .filter(|(_, (a, b))| a != b)
            .map(|((n, name), (a, b))| {
                format!("  column {n} ({name}): {lname} {a:?}, {rname} {b:?}")
            })
            .collect();
        panic!(
            "{view}: row {} differs for session {:?} ({rname}'s row is {:?}):\n{}",
            i + 1,
            l[0],
            r[0],
            diffs.join("\n")
        );
    }
    assert_eq!(
        lrows.len(),
        rrows.len(),
        "{view}: {lname} printed {} rows, {rname} {}:\n{lname}: {:?}\n{rname}: {:?}",
        lrows.len(),
        rrows.len(),
        lrows.iter().map(|r| &r[0]).collect::<Vec<_>>(),
        rrows.iter().map(|r| &r[0]).collect::<Vec<_>>()
    );
}

fn check_parity(once_args: &[&str], section: &str) {
    let store = Golden::build();
    let mut dash_args = vec!["dashboard", "--once"];
    dash_args.extend(once_args);
    let mut list_args = vec!["once"];
    list_args.extend(once_args);

    let dash = store.koto(&dash_args);
    let list = store.list(&list_args);
    let view = format!(
        "once{}",
        once_args
            .iter()
            .map(|a| format!(" {a}"))
            .collect::<String>()
    );

    let dash_rows: Vec<Vec<String>> = dash
        .stdout
        .lines()
        .map(|l| {
            let fields: Vec<&str> = l.split('\t').collect();
            assert_eq!(
                fields.get(8..),
                Some(&["", ""][..]),
                "koto dashboard's columns 9 and 10 are empty on the local backend: {l:?}"
            );
            project("koto dashboard --once", l, 10)
        })
        .collect();
    let list_rows: Vec<Vec<String>> = list
        .stdout
        .lines()
        .map(|l| project("koto-session-list once", l, 8))
        .collect();

    compare(
        &view,
        ("koto dashboard", &dash_rows),
        ("koto-session-list", &list_rows),
    );
    compare(
        &view,
        (EXPECTED_PATH, &expected_rows(section)),
        ("koto-session-list", &list_rows),
    );
    assert_eq!(
        list.stderr_lines(),
        dash.stderr_lines(),
        "{view}: koto-session-list's stderr differs from koto dashboard's"
    );
    assert_eq!(
        list.stderr_lines(),
        vec!["note: 1 unreadable session(s) skipped".to_string()],
        "{view}: the golden store holds one unreadable session"
    );
}

#[test]
#[ignore = "needs SESSION_LIST_BIN; run by validate.yml's Session List Checks job"]
fn once_matches_the_dashboard_default_view() {
    check_parity(&[], "[once]");
}

#[test]
#[ignore = "needs SESSION_LIST_BIN; run by validate.yml's Session List Checks job"]
fn once_all_matches_the_dashboard_all_view() {
    check_parity(&["--all"], "[once --all]");
}

const PARENT: &str = "golden-parent";
const CHILD_A: &str = "golden-parent-child-a";
const CHILD_B: &str = "golden-parent-child-b";
const KEY: &str = "status/breadcrumb";

#[test]
#[ignore = "needs SESSION_LIST_BIN; run by validate.yml's Session List Checks job"]
fn list_prints_a_tree_with_the_values_koto_holds() {
    let store = Golden::build();
    let parent_value = "coordinating two halves";
    let child_value = "first half\tstep 2\nwaiting on review";
    store.koto_with_stdin(&["context", "add", PARENT, KEY], parent_value);
    store.koto_with_stdin(&["context", "add", CHILD_A, KEY], child_value);

    let list = store.list(&["list", "--root", PARENT, "--key", KEY]);
    assert_eq!(list.stderr, "", "list wrote to stderr");
    let rows: Vec<Vec<&str>> = list
        .stdout
        .lines()
        .map(|l| {
            let fields: Vec<&str> = l.split('\t').collect();
            assert_eq!(fields.len(), 5, "a list line has five fields: {l:?}");
            fields
        })
        .collect();
    let ids: Vec<&str> = rows.iter().map(|r| r[0]).collect();
    assert_eq!(
        ids,
        [PARENT, CHILD_A, CHILD_B],
        "list prints the root, then its children in id order"
    );
    let parents: Vec<&str> = rows.iter().map(|r| r[1]).collect();
    assert_eq!(parents, ["", PARENT, PARENT], "each row's parent field");

    // State and liveness agree with the dashboard's columns 2 and 8.
    let dash = store.koto(&["dashboard", "--once", "--all"]);
    let dash_by_id: BTreeMap<&str, Vec<&str>> = dash
        .stdout
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f[0], f)
        })
        .collect();
    for row in &rows {
        let dash_row = dash_by_id
            .get(row[0])
            .unwrap_or_else(|| panic!("koto dashboard --once --all has no row for {}", row[0]));
        assert_eq!(
            (row[2], row[3]),
            (dash_row[1], dash_row[7]),
            "list's state and liveness for {} differ from the dashboard's columns 2 and 8",
            row[0]
        );
    }

    // Values agree with `koto context get`, tabs and line breaks read as
    // spaces; a session with nothing under the key prints an empty field
    // and koto refuses the get.
    for (row, written) in rows.iter().zip([parent_value, child_value]) {
        let got = store.koto(&["context", "get", row[0], KEY]).stdout;
        assert_eq!(got, written, "koto context get {} {KEY}", row[0]);
        let flat = got.replace(['\t', '\n', '\r'], " ");
        assert_eq!(row[4], flat, "list's value field for {}", row[0]);
    }
    assert_eq!(rows[2][4], "", "{CHILD_B} has no value under {KEY}");
    let mut get_b = koto_command::koto_std();
    store.env(&mut get_b);
    let get_b = Out::of(get_b.args(["context", "get", CHILD_B, KEY]));
    assert_ne!(
        get_b.code,
        Some(0),
        "koto context get {CHILD_B} {KEY} succeeded with {:?}",
        get_b.stdout
    );

    // A leaf root prints itself alone.
    let leaf = store.list(&["list", "--root", CHILD_A, "--key", KEY]);
    let leaf_ids: Vec<&str> = leaf
        .stdout
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(leaf_ids, [CHILD_A], "a leaf root prints itself alone");

    // An unknown root is a usage error naming it.
    let unknown = store.list_cmd(&["list", "--root", "golden-nobody", "--key", KEY]);
    assert_eq!(unknown.code, Some(2), "an unknown root exits 2");
    assert!(
        unknown.stderr.contains("golden-nobody"),
        "the unknown root's error names it: {:?}",
        unknown.stderr
    );
}

// ---------------------------------------------------------------------------
// A hundred reads on a cloud host
// ---------------------------------------------------------------------------

const BUCKET: &str = "koto-session-list";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const ROOT: &str = "list-root";
const CHILD: &str = "list-child";
const NOTE: &str = "note";

/// `check` waits on context key `go`, which nothing sets, so a session
/// ticked once stops blocked there.
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

/// A host whose koto runs the cloud backend against a FakeS3 bucket, with a
/// koto root of its own.
struct CloudHost {
    _root: tempfile::TempDir,
    home: PathBuf,
    koto_home: PathBuf,
    cache: PathBuf,
    work: PathBuf,
    endpoint: String,
}

impl CloudHost {
    fn new(s3: &FakeS3) -> CloudHost {
        let root = tempfile::TempDir::new().unwrap();
        let mk = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        let koto_home = mk(root.path().join("koto-home"));
        // With KOTO_HOME set, koto takes the backend from the root's
        // config.toml and ignores KOTO_SESSION_BACKEND.
        std::fs::write(
            koto_home.join("config.toml"),
            "[session]\nbackend = \"cloud\"\n",
        )
        .unwrap();
        CloudHost {
            home: mk(root.path().join("home")),
            koto_home,
            cache: mk(root.path().join("cache")),
            work: mk(root.path().join("work")),
            _root: root,
            endpoint: s3.endpoint(),
        }
    }

    /// The host's environment, the same for koto and the program: the
    /// program is handed the bucket's settings too, and must not use them.
    fn env(&self, cmd: &mut Command) {
        cmd.env_clear()
            .current_dir(&self.work)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    koto_command::koto_bin().parent().unwrap().display(),
                    SYSTEM_PATH
                ),
            )
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_MACHINE_NAME", "list-host")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0");
    }

    fn koto(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd);
        Out::of(cmd.args(args)).ok(&format!("koto {}", args.join(" ")))
    }

    fn list(&self, args: &[&str]) -> Out {
        let mut cmd = Command::new(session_list_bin());
        self.env(&mut cmd);
        Out::of(cmd.args(args))
    }
}

/// Every entry under `root`: its relative path, its type, and for a file its
/// size, modification time and bytes, for a link its target.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            let desc = if meta.file_type().is_symlink() {
                format!("link -> {}", std::fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                stack.push(path.clone());
                format!("dir mtime {:?}", meta.modified().unwrap())
            } else {
                let bytes = std::fs::read(&path).unwrap();
                format!(
                    "file size {} mtime {:?} bytes {:?}",
                    meta.len(),
                    meta.modified().unwrap(),
                    bytes
                )
            };
            out.insert(rel, desc);
        }
    }
    out
}

#[test]
#[ignore = "needs SESSION_LIST_BIN; run by validate.yml's Session List Checks job"]
fn a_hundred_reads_leave_a_cloud_host_unchanged_and_send_no_request() {
    let s3 = FakeS3::start(BUCKET);
    let host = CloudHost::new(&s3);
    std::fs::write(host.work.join("gated.md"), TEMPLATE).unwrap();
    host.koto(&["init", ROOT, "--template", "gated.md"]);
    host.koto(&["next", ROOT]);
    host.koto(&["init", CHILD, "--template", "gated.md", "--parent", ROOT]);
    host.koto(&["next", CHILD]);
    for (name, value) in [(ROOT, "root note"), (CHILD, "child note")] {
        let path = host.work.join(format!("{name}.txt"));
        std::fs::write(&path, value).unwrap();
        host.koto(&[
            "context",
            "add",
            name,
            NOTE,
            "--from-file",
            path.to_str().unwrap(),
        ]);
    }
    let before = snapshot(&host.koto_home);
    assert!(
        !s3.requests().is_empty(),
        "the host's koto reached the bucket while setting up; koto root: {:?}",
        before.keys().collect::<Vec<_>>()
    );
    assert!(
        before.keys().any(|p| p.starts_with("sessions")),
        "the host's koto root holds its sessions mirror: {:?}",
        before.keys().collect::<Vec<_>>()
    );
    s3.clear_requests();

    let modes: [&[&str]; 3] = [
        &["once"],
        &["once", "--all"],
        &["list", "--root", ROOT, "--key", NOTE],
    ];
    for i in 0..100 {
        let args = modes[i % modes.len()];
        let out = host.list(args);
        let what = format!("run {} of koto-session-list {}", i + 1, args.join(" "));
        assert_eq!(
            out.code,
            Some(0),
            "{what} exited {:?}: {}",
            out.code,
            out.stderr
        );
        assert_eq!(out.stderr, "", "{what} wrote to stderr");
        assert!(
            out.stdout
                .lines()
                .any(|l| l.starts_with(&format!("{ROOT}\t"))),
            "{what} printed no row for {ROOT}:\n{}",
            out.stdout
        );
        if args[0] == "list" {
            assert!(
                out.stdout
                    .lines()
                    .any(|l| l.starts_with(&format!("{CHILD}\t{ROOT}\t"))
                        && l.ends_with("\tchild note")),
                "{what} printed no row for {CHILD} under {ROOT} with its note:\n{}",
                out.stdout
            );
        }
    }

    let after = snapshot(&host.koto_home);
    if before != after {
        let changed: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|p| before.get(*p) != after.get(*p))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        panic!("a hundred reads changed the host's koto root: {changed:?}");
    }
    let requests = s3.requests();
    assert!(
        requests.is_empty(),
        "a hundred reads sent {} request(s) to the bucket: {:?}",
        requests.len(),
        requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.key))
            .collect::<Vec<_>>()
    );

    // The control: koto's own dashboard on the same host does reach the
    // bucket, so a FakeS3 that recorded nothing above wasn't deaf.
    host.koto(&["dashboard", "--once"]);
    assert!(
        !s3.requests().is_empty(),
        "koto dashboard --once on the cloud host sent no request; the zero above proves nothing"
    );
}
