//! Unit tests for the run journal. Each test builds its store with
//! `LocalBackend::with_base_dir_and_journal` on a temporary directory, so
//! nothing here reads or writes the invoking user's journal.

use super::*;
use crate::engine::types::{Event, EventPayload, StateFileHeader};
use crate::host_env::{set_host_env_for_test, CLAUDE_SESSION_ID_ENV};
use crate::session::local::LocalBackend;
use crate::session::SessionBackend;
use std::path::PathBuf;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    home: PathBuf,
    backend: LocalBackend,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("koto-home");
        let backend =
            LocalBackend::with_base_dir_and_journal(tmp.path().join("sessions"), home.clone());
        reset_for_test();
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, None);
        Fixture {
            _tmp: tmp,
            home,
            backend,
        }
    }

    fn journal(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.home.join(JOURNAL_FILE))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("every journal line is JSON"))
            .collect()
    }

    fn init(&self, name: &str, header: StateFileHeader, initial_state: Option<&str>) {
        let mut events = vec![event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: String::new(),
                variables: Default::default(),
                spawn_entry: None,
            },
        )];
        if let Some(state) = initial_state {
            events.push(event(2, transitioned(None, state)));
        }
        self.backend.init_state_file(name, header, events).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        reset_for_test();
        set_host_env_for_test(CLAUDE_SESSION_ID_ENV, None);
    }
}

fn event(seq: u64, payload: EventPayload) -> Event {
    Event {
        seq,
        timestamp: crate::engine::types::now_iso8601(),
        event_type: payload.type_name().to_string(),
        payload,
        idempotency_hash: None,
    }
}

fn transitioned(from: Option<&str>, to: &str) -> EventPayload {
    EventPayload::Transitioned {
        from: from.map(str::to_string),
        to: to.to_string(),
        condition_type: "auto".to_string(),
        skip_if_matched: None,
        context_assignments: None,
        vars_matched: None,
    }
}

fn header(workflow: &str, session_id: &str, parent: Option<&str>) -> StateFileHeader {
    let mut h: StateFileHeader = serde_json::from_str(
        r#"{"schema_version":1,"workflow":"x","template_hash":"","created_at":"2026-01-01T00:00:00.000Z"}"#,
    )
    .unwrap();
    h.workflow = workflow.to_string();
    h.session_id = session_id.to_string();
    h.parent_workflow = parent.map(str::to_string);
    h.template_name = Some("demo".to_string());
    h.template_hash = "ab".repeat(32);
    h
}

fn kinds(records: &[serde_json::Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn shapes_accept_128_characters_and_refuse_129() {
    assert!(is_id(&"a".repeat(128)));
    assert!(!is_id(&"a".repeat(129)));
    assert!(!is_id(""));
    assert!(is_id("A-z.0_9:x"));
    assert!(!is_id("a/b"));
    assert!(!is_id("a b"));
    assert!(!is_id("a\nb"));

    assert!(is_name(&"n".repeat(128)));
    assert!(!is_name(&"n".repeat(129)));
    assert!(is_name("team/work-on@2"));
    assert!(!is_name("/etc/passwd"));
    assert!(!is_name("~/x"));
    assert!(!is_name("a b"));
    assert!(!is_name("é"));
}

#[test]
fn init_writes_session_started_then_the_initial_state_and_a_sidecar() {
    let f = Fixture::new();
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("driver-a"));
    f.init("root", header("root", "root-id", None), Some("start"));

    let j = f.journal();
    assert_eq!(kinds(&j), vec!["session_started", "state_entered"]);
    let s = &j[0];
    assert_eq!(s["v"], 1);
    assert_eq!(s["session"], "root");
    assert_eq!(s["koto.session.id"], "root-id");
    assert_eq!(s["koto.run.id"], "root-id");
    assert_eq!(s["koto.driver.session.id"], "driver-a");
    assert_eq!(s["koto.template.name"], "demo");
    assert_eq!(s["koto.template.hash"], "ab".repeat(32));
    assert_eq!(s["koto.fixture"], false);
    assert!(s.get("koto.parent.session.id").is_none());
    assert!(s.get("koto.imported_from.session.id").is_none());
    let at = s["at"].as_str().unwrap();
    assert_eq!(at.len(), 24, "{at}");
    assert!(at.ends_with('Z') && at.as_bytes()[19] == b'.', "{at}");
    assert_eq!(j[1]["koto.state"], "start");

    assert_eq!(
        sidecar::read(&f.backend.session_dir("root")),
        Some(sidecar::Sidecar {
            run_id: Some("root-id".into()),
            driver: Some("driver-a".into()),
        })
    );
}

#[test]
fn a_session_without_a_driver_or_template_leaves_those_fields_out() {
    let f = Fixture::new();
    let mut h = header("bare", "bare-id", None);
    h.template_name = None;
    h.template_hash = String::new();
    f.init("bare", h, None);
    let j = f.journal();
    assert_eq!(kinds(&j), vec!["session_started"]);
    for key in [
        "koto.driver.session.id",
        "koto.template.name",
        "koto.template.hash",
    ] {
        assert!(j[0].get(key).is_none(), "{key} should be left out");
    }
    for value in j[0].as_object().unwrap().values() {
        assert!(!value.is_null() && value != "", "no null or empty values");
    }
}

#[test]
fn commits_map_to_records_and_other_payloads_write_nothing() {
    let f = Fixture::new();
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    let b = &f.backend;
    let ts = crate::engine::types::now_iso8601();
    b.append_event("wf", &transitioned(Some("a"), "b"), &ts)
        .unwrap();
    b.append_event(
        "wf",
        &EventPayload::DirectedTransition {
            from: "b".into(),
            to: "c".into(),
            rationale: Some("PLANTED-RATIONALE".into()),
        },
        &ts,
    )
    .unwrap();
    b.append_event(
        "wf",
        &EventPayload::Rewound {
            from: "c".into(),
            to: "b".into(),
            rationale: None,
        },
        &ts,
    )
    .unwrap();
    b.append_event("wf", &transitioned(Some("b"), "b"), &ts)
        .unwrap();
    b.append_event(
        "wf",
        &EventPayload::EvidenceSubmitted {
            state: "b".into(),
            fields: serde_json::from_str(r#"{"x":"PLANTED-EVIDENCE"}"#).unwrap(),
            submitter_cwd: None,
            source: None,
        },
        &ts,
    )
    .unwrap();
    b.append_event(
        "wf",
        &EventPayload::WorkflowCancelled {
            state: "b".into(),
            reason: "PLANTED-REASON".into(),
        },
        &ts,
    )
    .unwrap();

    let j = f.journal();
    assert_eq!(
        kinds(&j),
        vec![
            "session_started",
            "state_entered",
            "state_entered",
            "state_entered",
            "state_entered",
            "state_entered",
            "cancelled"
        ]
    );
    let states: Vec<&str> = j[1..6]
        .iter()
        .map(|r| r["koto.state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["a", "b", "c", "b", "b"]);
    for r in &j {
        assert_eq!(r["koto.run.id"], "wf-id");
    }
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

#[test]
fn a_failed_commit_writes_no_record() {
    let f = Fixture::new();
    let ts = crate::engine::types::now_iso8601();
    // No such session: the log append fails before the hook runs.
    assert!(f
        .backend
        .append_event("ghost", &transitioned(None, "a"), &ts)
        .is_err());
    // A session whose log can't be appended to.
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    let before = f.journal().len();
    let state = f
        .backend
        .session_dir("wf")
        .join(crate::session::state_file_name("wf"));
    std::fs::remove_file(&state).unwrap();
    assert!(f
        .backend
        .append_event("wf", &transitioned(Some("a"), "b"), &ts)
        .is_err());
    assert_eq!(f.journal().len(), before);
    assert!(warnings_for_test().is_empty());
}

#[test]
fn an_oversized_record_is_dropped_with_one_warning_and_the_rest_are_written() {
    let f = Fixture::new();
    // A long, valid template name makes session_started larger than the
    // state_entered that follows it; a cap between the two drops only the
    // first.
    let mut h = header("big", "big-id", None);
    h.template_name = Some("t".repeat(128));
    let probe_started = session_started("big", &h, Some("big-id"), None, None)
        .to_line()
        .len();
    let probe_state = Record::new("state_entered", "big", "big-id", Some("big-id"))
        .name("koto.state", Some("start"))
        .to_line()
        .len();
    assert!(probe_started > probe_state + 100);
    set_max_line_for_test(probe_state + 20);

    f.init("big", h, Some("start"));
    f.backend
        .append_event(
            "big",
            &transitioned(Some("start"), "next"),
            &crate::engine::types::now_iso8601(),
        )
        .unwrap();

    let j = f.journal();
    assert_eq!(kinds(&j), vec!["state_entered", "state_entered"]);
    let warnings = warnings_for_test();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].starts_with("warning: run journal write failed ("));
    assert!(warnings[0].contains("exceeds"), "{}", warnings[0]);
}

#[test]
fn values_outside_their_shape_are_left_out_and_the_record_is_kept() {
    let f = Fixture::new();
    let mut h = header("odd", "odd-id", None);
    h.template_name = Some("/home/me/templates/x".into());
    h.template_hash = "not a hash".into();
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some(&"d".repeat(129)));
    f.init("odd", h, Some(&"s".repeat(129)));
    let j = f.journal();
    assert_eq!(kinds(&j), vec!["session_started", "state_entered"]);
    assert!(j[0].get("koto.template.name").is_none());
    assert!(j[0].get("koto.template.hash").is_none());
    assert!(j[0].get("koto.driver.session.id").is_none());
    assert!(j[1].get("koto.state").is_none());
    assert_eq!(j[1]["koto.run.id"], "odd-id");
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_journal_path_is_refused_with_one_warning() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.home).unwrap();
    let elsewhere = f.home.parent().unwrap().join("elsewhere.txt");
    std::fs::write(&elsewhere, "keep\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.home.join(JOURNAL_FILE)).unwrap();
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    f.backend
        .append_event(
            "wf",
            &transitioned(Some("a"), "b"),
            &crate::engine::types::now_iso8601(),
        )
        .unwrap();
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "keep\n");
    assert_eq!(warnings_for_test().len(), 1, "{:?}", warnings_for_test());
}

#[test]
fn a_child_records_its_lineage_and_carries_the_roots_run_id() {
    let f = Fixture::new();
    f.init("root", header("root", "root-id", None), Some("a"));
    let (root, parent) = child_lineage(&f.backend, "root");
    assert_eq!(
        (root.as_deref(), parent.as_deref()),
        (Some("root-id"), Some("root-id"))
    );

    let mut child = header("root.c", "child-id", Some("root"));
    child.root_session_id = root;
    child.parent_session_id = parent;
    f.init("root.c", child, Some("work"));

    let (root2, parent2) = child_lineage(&f.backend, "root.c");
    assert_eq!(
        (root2.as_deref(), parent2.as_deref()),
        (Some("root-id"), Some("child-id"))
    );

    let j = f.journal();
    let started = &j[2];
    assert_eq!(started["session"], "root.c");
    assert_eq!(started["koto.run.id"], "root-id");
    assert_eq!(started["koto.parent.session.id"], "root-id");
}

#[test]
fn an_older_child_walks_to_its_root_and_omits_the_run_id_when_the_root_is_gone() {
    let f = Fixture::new();
    f.init("root", header("root", "root-id", None), Some("a"));
    // A child from an older koto: no lineage fields in its header.
    f.init(
        "root.old",
        header("root.old", "old-id", Some("root")),
        Some("work"),
    );
    std::fs::remove_file(
        f.backend
            .session_dir("root.old")
            .join(sidecar::SIDECAR_FILE),
    )
    .unwrap();
    assert_eq!(
        run_id::run_id(&f.backend, &f.backend.read_header("root.old").unwrap()).as_deref(),
        Some("root-id")
    );

    // A grandchild spawned under it now records the true root.
    let (root, parent) = child_lineage(&f.backend, "root.old");
    assert_eq!(
        (root.as_deref(), parent.as_deref()),
        (Some("root-id"), Some("old-id"))
    );

    // With the root gone and no cache, nothing resolves, and the
    // child's own id is never used in its place.
    f.backend.cleanup("root").unwrap();
    let (root, parent) = child_lineage(&f.backend, "root.old");
    assert_eq!((root, parent.as_deref()), (None, Some("old-id")));
    assert_eq!(
        run_id::run_id(&f.backend, &f.backend.read_header("root.old").unwrap()),
        None
    );
}

#[test]
fn a_store_on_an_explicit_base_journals_inside_its_base() {
    let f = Fixture::new();
    let base = f._tmp.path().join("elsewhere");
    let other = LocalBackend::with_base_dir(base.clone());
    other
        .init_state_file(
            "quiet",
            header("quiet", "quiet-id", None),
            vec![event(2, transitioned(None, "a"))],
        )
        .unwrap();
    other
        .append_event(
            "quiet",
            &transitioned(Some("a"), "b"),
            &crate::engine::types::now_iso8601(),
        )
        .unwrap();
    // Not in the fixture's journal: the store journals inside its own base.
    assert!(f.journal().is_empty());
    let own: Vec<serde_json::Value> = std::fs::read_to_string(base.join(JOURNAL_FILE))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = own.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec!["session_started", "state_entered", "state_entered"]
    );
    assert!(other
        .session_dir("quiet")
        .join(sidecar::SIDECAR_FILE)
        .exists());
    // The journal file at the base level is never listed as a session.
    let listed: Vec<String> = other.list().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(listed, vec!["quiet".to_string()]);
    assert!(warnings_for_test().is_empty());
}
