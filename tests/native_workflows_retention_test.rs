//! The retention bound on a `/workflows` directory.
//!
//! Every koto write into a Claude Code `/workflows` directory ends with a
//! sweep that keeps at most ten koto-owned entries, removing finished ones
//! oldest `startTime` first and never a live entry or the one just written.
//! The first tests drive real sessions with the koto binary (`koto init`,
//! then `koto next` to the terminal state) into one `KOTO_WORKFLOWS_DIR`, so
//! the bound is shown across the whole commit-to-disk path; the rest call
//! the sweep directly on seeded directories to pin its ordering and its
//! protections. Nothing here needs niwa or Claude Code.

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use koto::workflows_surface::retention::{self, Limits, KEEP};
use koto::workflows_surface::CONTRACT_VERSION;
use serde_json::{json, Value};

/// A one-step workflow: `koto next` takes a session from `start` straight to
/// its terminal state.
const TEMPLATE: &str = r#"---
name: retention-check
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Begin.

## done

Finished.
"#;

/// Variables that would point a koto run at a store or session outside the
/// test's scratch directory.
const AMBIENT_ENV: [&str; 7] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_DISPATCHER_ANCHOR",
    "KOTO_AUTHORITY",
    "KOTO_TICK_SESSION",
    "KOTO_WORKFLOWS_HOST_SESSION",
    "KOTO_SESSIONS_BASE",
];

/// A scratch koto home, session store and `/workflows` directory.
struct Scratch {
    _root: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Scratch {
        let root_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path().to_path_buf();
        std::fs::write(root.join("retention.md"), TEMPLATE).unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        Scratch {
            _root: root_dir,
            root,
        }
    }

    fn workflows(&self) -> PathBuf {
        self.root.join("workflows")
    }

    fn koto(&self, args: &[&str]) {
        let mut cmd = koto_command::koto();
        for name in AMBIENT_ENV {
            cmd.env_remove(name);
        }
        let out = cmd
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("KOTO_HOME", self.root.join("koto-home"))
            .env("KOTO_WORKFLOWS_DIR", self.workflows())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Drive session `name` from `koto init` to its terminal state.
    fn run_session(&self, name: &str) {
        let template = self.root.join("retention.md");
        self.koto(&["init", name, "--template", template.to_str().unwrap()]);
        self.koto(&["next", name]);
    }
}

/// The koto-owned entries in `dir`, by file name: files named
/// `koto-<id>.json` whose content carries koto's marker.
fn owned_entries(dir: &Path) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("koto-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<Value>(&std::fs::read(entry.path()).unwrap()) else {
            continue;
        };
        if v["id"] == format!("koto-{id}") && v["koto"]["sessionId"] == id {
            out.insert(name, v);
        }
    }
    out
}

/// The session names (`koto.workflow`) of the owned entries in `dir`, sorted.
fn session_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = owned_entries(dir)
        .values()
        .map(|v| v["koto"]["workflow"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

/// An entry in the current contract's shape for session `id`.
fn entry(id: &str, status: &str, start_time: Value) -> Value {
    json!({
        "id": format!("koto-{id}"),
        "name": id,
        "workflowName": id,
        "summary": format!("koto \u{b7} {id} \u{b7} done"),
        "status": status,
        "startTime": start_time,
        "koto": {
            "sessionId": id,
            "workflow": id,
            "currentState": "done",
            "contractVersion": CONTRACT_VERSION,
            "status": status,
        },
    })
}

/// An entry in the first contract version's shape: no `workflowName`,
/// `summary` or `koto.status`.
fn v1_entry(id: &str, status: &str, start_time: u64) -> Value {
    json!({
        "id": format!("koto-{id}"),
        "name": id,
        "status": status,
        "startTime": start_time,
        "koto": {
            "sessionId": id,
            "workflow": id,
            "currentState": "done",
            "contractVersion": 1,
        },
    })
}

/// Write `value` as session `id`'s entry in `dir` and return its path.
fn seed(dir: &Path, id: &str, value: &Value) -> PathBuf {
    let path = dir.join(format!("koto-{id}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}

/// Every file in `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

/// koto#190: thirty sessions driven to completion through the koto binary
/// into one directory leave at most ten koto entries after each one, the
/// ten newest at the end, and neither Claude Code's own file nor a
/// lookalike without koto's marker is touched.
#[test]
fn thirty_sessions_through_the_binary_leave_the_ten_newest() {
    let scratch = Scratch::new();
    let dir = scratch.workflows();
    let claude_file = dir.join("wf_test.json");
    let claude_bytes = br#"{"id":"wf_test","status":"completed","startTime":1}"#.to_vec();
    std::fs::write(&claude_file, &claude_bytes).unwrap();
    let lookalike = dir.join("koto-notes.json");
    let lookalike_bytes = br#"{"id":"koto-notes","status":"completed","startTime":1}"#.to_vec();
    std::fs::write(&lookalike, &lookalike_bytes).unwrap();

    let names: Vec<String> = (1..=3 * KEEP).map(|n| format!("r{n:02}")).collect();
    for (i, name) in names.iter().enumerate() {
        scratch.run_session(name);
        let owned = owned_entries(&dir);
        assert!(
            owned.len() <= KEEP,
            "{} koto entries after session {}: {:?}",
            owned.len(),
            i + 1,
            owned.keys().collect::<Vec<_>>()
        );
        assert_eq!(owned.len(), (i + 1).min(KEEP));
        let mine = owned
            .values()
            .find(|v| v["koto"]["workflow"] == name.as_str())
            .unwrap_or_else(|| panic!("session {name}'s own entry was written and kept"));
        assert_eq!(
            mine["status"], "completed",
            "{name} ran to its terminal state"
        );
    }

    assert_eq!(session_names(&dir), names[2 * KEEP..].to_vec());
    assert_eq!(std::fs::read(&claude_file).unwrap(), claude_bytes);
    assert_eq!(std::fs::read(&lookalike).unwrap(), lookalike_bytes);
}

/// The oldest entry goes whatever its status word: completed, failed, or a
/// cancelled session's `failed`.
#[test]
fn the_oldest_finished_entry_goes_whatever_its_outcome() {
    let outcomes = [
        ("completed", "completed"),
        ("failed", "failed"),
        ("cancelled", "failed"),
    ];
    for (outcome, oldest_status) in outcomes {
        let scratch = Scratch::new();
        let dir = scratch.workflows();
        let mut oldest = entry("oldest", oldest_status, json!(1_000));
        if outcome == "cancelled" {
            oldest["koto"]["status"] = json!("failed");
            oldest["summary"] = json!("koto \u{b7} oldest \u{b7} cancelled");
        }
        seed(&dir, "oldest", &oldest);
        for n in 1..KEEP {
            let status = outcomes[n % outcomes.len()].1;
            let id = format!("seed{n:02}");
            seed(&dir, &id, &entry(&id, status, json!(2_000 + n as u64)));
        }
        assert_eq!(owned_entries(&dir).len(), KEEP);

        scratch.run_session("newest");

        let names = session_names(&dir);
        assert_eq!(names.len(), KEEP, "{outcome}: {names:?}");
        assert!(
            !names.contains(&"oldest".to_string()),
            "{outcome}: the oldest entry was kept: {names:?}"
        );
        assert!(names.contains(&"newest".to_string()));
    }
}

/// The first koto write after an upgrade drains a backlog in one sweep,
/// first-version entries included.
#[test]
fn one_write_drains_a_backlog_of_thirty_including_a_version_one_entry() {
    let scratch = Scratch::new();
    let dir = scratch.workflows();
    // Session ids in file names are UUIDs, which can start with a digit.
    let legacy = "0a1b2c3d-legacy";
    let v1 = seed(&dir, legacy, &v1_entry(legacy, "completed", 1_000));
    for n in 1..3 * KEEP {
        let id = format!("seed{n:02}");
        seed(&dir, &id, &entry(&id, "completed", json!(2_000 + n as u64)));
    }
    assert_eq!(owned_entries(&dir).len(), 3 * KEEP);

    scratch.run_session("upgrade");

    assert_eq!(owned_entries(&dir).len(), KEEP);
    assert!(!v1.exists(), "the first-version entry was not removed");
    assert!(session_names(&dir).contains(&"upgrade".to_string()));
}

/// The entry just written is never removed, even when its `startTime` is the
/// oldest.
#[test]
fn the_entry_just_written_survives_a_bound_of_one() {
    let dir = tempfile::tempdir().unwrap();
    let older = seed(
        dir.path(),
        "older",
        &entry("older", "completed", json!(2_000)),
    );
    let written = seed(
        dir.path(),
        "written",
        &entry("written", "completed", json!(1_000)),
    );

    let sweep = retention::apply(
        dir.path(),
        &written,
        SystemTime::now(),
        &Limits {
            keep: 1,
            ..Limits::default()
        },
    );

    assert_eq!(sweep.removed, 1);
    assert!(!older.exists());
    assert!(written.exists());
    assert_eq!(owned_entries(dir.path()).len(), 1);
}

/// A live entry is never removed to make room, however old it is.
#[test]
fn a_live_entry_older_than_every_finished_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let live = seed(dir.path(), "live", &entry("live", "running", json!(1)));
    for n in 1..=KEEP {
        let id = format!("seed{n:02}");
        seed(
            dir.path(),
            &id,
            &entry(&id, "completed", json!(1_000 + n as u64)),
        );
    }
    let written = seed(
        dir.path(),
        "written",
        &entry("written", "running", json!(9_999)),
    );

    let sweep = retention::apply(dir.path(), &written, SystemTime::now(), &Limits::default());

    assert_eq!(sweep.removed, 2);
    assert!(live.exists());
    let names = session_names(dir.path());
    assert_eq!(names.len(), KEEP);
    assert!(!names.contains(&"seed01".to_string()));
    assert!(!names.contains(&"seed02".to_string()));
}

/// More live entries than the bound: every finished entry goes, every live
/// one stays, and the directory holds more than ten until they finish.
#[test]
fn live_entries_past_the_bound_all_stay() {
    let dir = tempfile::tempdir().unwrap();
    for n in 1..=KEEP + 2 {
        let id = format!("live{n:02}");
        seed(dir.path(), &id, &entry(&id, "running", json!(n as u64)));
    }
    for n in 1..=3 {
        let id = format!("done{n}");
        seed(
            dir.path(),
            &id,
            &entry(&id, "completed", json!(100 + n as u64)),
        );
    }
    let written = seed(
        dir.path(),
        "written",
        &entry("written", "running", json!(9_999)),
    );

    let sweep = retention::apply(dir.path(), &written, SystemTime::now(), &Limits::default());

    assert_eq!(sweep.removed, 3);
    let owned = owned_entries(dir.path());
    assert_eq!(owned.len(), KEEP + 3);
    assert!(owned.values().all(|v| v["status"] == "running"));
    assert!(written.exists());
}

/// An entry with no numeric `startTime` is ordered by its modification time,
/// and equal keys are removed in ascending filename order.
#[test]
fn ordering_falls_back_to_modification_time_and_then_the_filename() {
    let dir = tempfile::tempdir().unwrap();
    let a = seed(
        dir.path(),
        "a-late",
        &entry("a-late", "completed", json!(5_000)),
    );
    let b = seed(
        dir.path(),
        "b-nostart",
        &entry("b-nostart", "completed", json!("soon")),
    );
    filetime::set_file_mtime(&b, filetime::FileTime::from_unix_time(3, 0)).unwrap();
    let c = seed(dir.path(), "c-tie", &entry("c-tie", "failed", json!(4_000)));
    let d = seed(
        dir.path(),
        "d-tie",
        &entry("d-tie", "completed", json!(4_000)),
    );
    let written = seed(
        dir.path(),
        "written",
        &entry("written", "running", json!(9_999)),
    );

    // Five owned entries; each lower bound removes exactly the next oldest.
    retention::apply(
        dir.path(),
        &written,
        SystemTime::now(),
        &Limits {
            keep: 4,
            ..Limits::default()
        },
    );
    assert!(
        !b.exists(),
        "the entry ordered by its 3000 ms mtime goes first"
    );
    assert!(c.exists() && d.exists() && a.exists());

    retention::apply(
        dir.path(),
        &written,
        SystemTime::now(),
        &Limits {
            keep: 3,
            ..Limits::default()
        },
    );
    assert!(!c.exists(), "c-tie sorts before d-tie");
    assert!(d.exists() && a.exists());

    retention::apply(
        dir.path(),
        &written,
        SystemTime::now(),
        &Limits {
            keep: 2,
            ..Limits::default()
        },
    );
    assert!(!d.exists());
    assert!(a.exists() && written.exists());
}

/// Each directory is bounded on its own: a write into one leaves another's
/// entries alone.
#[test]
fn a_write_into_one_directory_leaves_another_alone() {
    let scratch = Scratch::new();
    let dir = scratch.workflows();
    for n in 1..=KEEP {
        let id = format!("here{n:02}");
        seed(&dir, &id, &entry(&id, "completed", json!(1_000 + n as u64)));
    }
    let other = tempfile::tempdir().unwrap();
    for n in 1..=KEEP + 5 {
        let id = format!("there{n:02}");
        seed(other.path(), &id, &entry(&id, "completed", json!(n as u64)));
    }
    let before = snapshot(other.path());

    scratch.run_session("here-new");

    assert_eq!(owned_entries(&dir).len(), KEEP);
    assert_eq!(snapshot(other.path()), before);
}

/// A directory within the bound costs one listing: the sweep opens nothing.
#[test]
fn a_directory_within_the_bound_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    for n in 1..KEEP {
        let id = format!("seed{n:02}");
        seed(dir.path(), &id, &entry(&id, "completed", json!(n as u64)));
    }
    let written = seed(
        dir.path(),
        "written",
        &entry("written", "running", json!(9_999)),
    );

    let sweep = retention::apply(dir.path(), &written, SystemTime::now(), &Limits::default());

    assert_eq!(sweep.matched, KEEP);
    assert_eq!(sweep.read, 0);
    assert_eq!(sweep.removed, 0);
}
