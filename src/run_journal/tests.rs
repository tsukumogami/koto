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
                machine: None,
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
            exported_fields: None,
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

/// A partial last line, as a crash or a full disk leaves it, costs only
/// that line: the next record starts on a line of its own and parses.
#[test]
fn a_record_after_a_partial_last_line_starts_on_its_own_line() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.home).unwrap();
    let path = f.home.join(JOURNAL_FILE);
    std::fs::write(&path, "{\"kind\":\"session_sta").unwrap();
    write(
        &f.home,
        &[Record::new("cancelled", "torn", "id-torn", Some("id-torn"))],
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text:?}");
    assert_eq!(lines[0], "{\"kind\":\"session_sta");
    let record: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(record["kind"], "cancelled");
    assert!(text.ends_with('\n'));
    // A journal that ends cleanly gets no extra newline.
    write(
        &f.home,
        &[Record::new("cancelled", "torn", "id-torn", Some("id-torn"))],
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
    assert!(warnings_for_test().is_empty());
}

// ----- Driver changes -----

fn evidence(state: &str) -> EventPayload {
    EventPayload::EvidenceSubmitted {
        state: state.into(),
        fields: serde_json::from_str(r#"{"x":"y"}"#).unwrap(),
        submitter_cwd: None,
        source: None,
        exported_fields: None,
    }
}

/// Append `payload` to `session` as a process driven by `driver` would.
fn commit_as(f: &Fixture, driver: Option<&str>, session: &str, payload: &EventPayload) {
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, driver);
    let ts = crate::engine::types::now_iso8601();
    f.backend.append_event(session, payload, &ts).unwrap();
}

fn cached_driver(f: &Fixture, session: &str) -> Option<String> {
    sidecar::read(&f.backend.session_dir(session)).and_then(|s| s.driver)
}

#[test]
fn each_change_of_driver_writes_driver_seen_before_the_payloads_record() {
    let f = Fixture::new();
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("driver-a"));
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    commit_as(&f, Some("driver-b"), "wf", &transitioned(Some("a"), "b"));
    commit_as(&f, Some("driver-b"), "wf", &transitioned(Some("b"), "c"));
    commit_as(&f, Some("driver-a"), "wf", &transitioned(Some("c"), "d"));

    let j = f.journal();
    assert_eq!(
        kinds(&j),
        vec![
            "session_started",
            "state_entered",
            "driver_seen",
            "state_entered",
            "state_entered",
            "driver_seen",
            "state_entered",
        ]
    );
    assert_eq!(j[2]["koto.driver.session.id"], "driver-b");
    assert_eq!(j[3]["koto.state"], "b");
    assert_eq!(j[5]["koto.driver.session.id"], "driver-a");
    assert_eq!(j[6]["koto.state"], "d");
    for r in &j[2..] {
        assert_eq!(r["koto.session.id"], "wf-id");
        assert_eq!(r["koto.run.id"], "wf-id");
    }
    assert_eq!(cached_driver(&f, "wf").as_deref(), Some("driver-a"));
    // The run id stays cached next to the driver.
    assert_eq!(
        sidecar::read(&f.backend.session_dir("wf")).unwrap().run_id,
        Some("wf-id".into())
    );
    assert!(warnings_for_test().is_empty());
}

#[test]
fn a_payload_that_writes_no_record_still_records_a_new_driver() {
    let f = Fixture::new();
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("driver-a"));
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    commit_as(&f, Some("driver-b"), "wf", &evidence("a"));
    commit_as(&f, Some("driver-b"), "wf", &evidence("a"));
    let j = f.journal();
    assert_eq!(
        kinds(&j),
        vec!["session_started", "state_entered", "driver_seen"]
    );
    assert_eq!(j[2]["koto.driver.session.id"], "driver-b");
    assert!(j[2].get("koto.state").is_none());
    assert_eq!(cached_driver(&f, "wf").as_deref(), Some("driver-b"));
}

#[test]
fn no_driver_or_a_malformed_one_is_never_a_change() {
    let f = Fixture::new();
    set_host_env_for_test(CLAUDE_SESSION_ID_ENV, Some("driver-a"));
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    let before = std::fs::read(f.backend.session_dir("wf").join(sidecar::SIDECAR_FILE)).unwrap();
    let long = "d".repeat(200);
    for driver in [None, Some("a\nb"), Some(long.as_str()), Some("")] {
        commit_as(&f, driver, "wf", &evidence("a"));
        commit_as(&f, driver, "wf", &transitioned(Some("a"), "a"));
    }
    let j = f.journal();
    assert!(
        kinds(&j).iter().all(|k| k != "driver_seen"),
        "{:?}",
        kinds(&j)
    );
    let after = std::fs::read(f.backend.session_dir("wf").join(sidecar::SIDECAR_FILE)).unwrap();
    assert_eq!(before, after, "the cache is left as it was");
}

#[test]
fn a_session_created_without_a_driver_records_its_first_one() {
    let f = Fixture::new();
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    assert_eq!(cached_driver(&f, "wf"), None);
    commit_as(&f, None, "wf", &evidence("a"));
    commit_as(&f, Some("driver-a"), "wf", &transitioned(Some("a"), "b"));
    commit_as(&f, Some("driver-a"), "wf", &transitioned(Some("b"), "c"));
    let j = f.journal();
    assert_eq!(
        kinds(&j),
        vec![
            "session_started",
            "state_entered",
            "driver_seen",
            "state_entered",
            "state_entered",
        ]
    );
    assert!(j[0].get("koto.driver.session.id").is_none());
    assert_eq!(j[2]["koto.driver.session.id"], "driver-a");
    assert_eq!(cached_driver(&f, "wf").as_deref(), Some("driver-a"));
}

/// A session with no sidecar at all (an older koto made it, or the cache
/// was lost) gets `driver_seen` for the first driver it meets, and the
/// sidecar written for it never caches an unresolved run id.
#[test]
fn a_lost_sidecar_records_the_driver_again_and_keeps_an_unresolved_run_id_uncached() {
    let f = Fixture::new();
    // An older child whose parent is gone: its run id can't be resolved.
    f.init(
        "gone.kid",
        header("gone.kid", "kid-id", Some("gone")),
        Some("a"),
    );
    std::fs::remove_file(
        f.backend
            .session_dir("gone.kid")
            .join(sidecar::SIDECAR_FILE),
    )
    .unwrap();
    commit_as(&f, Some("driver-a"), "gone.kid", &evidence("a"));
    let j = f.journal();
    let last = j.last().unwrap();
    assert_eq!(last["kind"], "driver_seen");
    assert!(last.get("koto.run.id").is_none(), "{last}");
    assert_eq!(cached_driver(&f, "gone.kid").as_deref(), Some("driver-a"));

    // The parent appears (a root, as an older koto left it): the next
    // record resolves the run id instead of trusting the empty cache.
    f.init("gone", header("gone", "gone-id", None), None);
    commit_as(
        &f,
        Some("driver-a"),
        "gone.kid",
        &transitioned(Some("a"), "b"),
    );
    let j = f.journal();
    let last = j.last().unwrap();
    assert_eq!(last["kind"], "state_entered");
    assert_eq!(last["koto.run.id"], "gone-id");
    let cached = sidecar::read(&f.backend.session_dir("gone.kid")).unwrap();
    assert_eq!(cached.run_id.as_deref(), Some("gone-id"));
    assert_eq!(cached.driver.as_deref(), Some("driver-a"));
}

// ----- Check records -----

use crate::engine::types::{CheckEventFields, PollRecord, RuleCount, RuleCounts};
use std::collections::BTreeSet;

const RELAY_FIELDS: &[&str] = &[
    "kind",
    "v",
    "at",
    "session",
    "koto.session.id",
    "koto.run.id",
    "koto.seq",
    "koto.visit.seq",
];

/// The fields a check kind, or `response`, may carry beyond
/// [`RELAY_FIELDS`].
fn check_kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "check_evaluated" => &[
            "koto.state",
            "koto.check",
            "koto.check.outcome",
            "koto.attempt",
            "koto.visit_attempt",
            "koto.check.rule_count",
            "koto.check.rule_counts_truncated",
            "koto.check.finding_count",
            "koto.check.findings_truncated",
            "koto.check.duration_ms",
            "koto.poll.status",
            "koto.check.unanswered_count",
            "koto.check.error",
            "koto.decider.awaiting_count",
            "koto.decider.agent_answered_count",
            "koto.check.reason",
            "koto.check.not_ready",
            "koto.check.pre_directive",
        ],
        "check_finding" => &[
            "koto.state",
            "koto.check.name",
            "koto.attempt",
            "koto.visit_attempt",
            "koto.rule.id",
            "koto.finding.level",
            "koto.finding.count",
            "koto.finding.source",
        ],
        "check_disputed" => &[
            "koto.state",
            "koto.check.name",
            "koto.attempt",
            "koto.visit_attempt",
            "koto.rule.id",
            "koto.dispute.source",
        ],
        "check_rule" => &[
            "koto.state",
            "koto.check",
            "koto.attempt",
            "koto.visit_attempt",
            "koto.rule.id",
            "koto.rule.visit_count",
            "koto.rule.session_count",
        ],
        "check_pending" => &[
            "koto.state",
            "koto.check",
            "koto.poll.status",
            "koto.poll.evaluations",
            "koto.poll.elapsed_secs",
            "koto.decider.awaiting_count",
            "koto.decider.agent_answered_count",
        ],
        "response" => &[
            "koto.state",
            "koto.response.source",
            "koto.response.directive_bytes",
            "koto.response.details_bytes",
        ],
        other => panic!("not a check or response kind: {other}"),
    }
}

/// The check records in `j`, each checked to carry `v` 1, both sequence
/// numbers, and only its kind's fields.
fn check_records(j: &[serde_json::Value]) -> Vec<serde_json::Value> {
    j.iter()
        .filter(|r| r["kind"].as_str().unwrap().starts_with("check_"))
        .map(|r| {
            let kind = r["kind"].as_str().unwrap();
            let allowed: BTreeSet<&str> = RELAY_FIELDS
                .iter()
                .chain(check_kind_fields(kind))
                .copied()
                .collect();
            for (key, value) in r.as_object().unwrap() {
                assert!(allowed.contains(key.as_str()), "{kind} carries {key}: {r}");
                assert!(!value.is_null() && value != "", "{key}: {r}");
            }
            assert_eq!(r["v"], 1);
            assert!(r["koto.seq"].is_u64(), "{r}");
            assert!(r["koto.visit.seq"].is_u64(), "{r}");
            r.clone()
        })
        .collect()
}

fn counts(rules: &[(&str, u64, u64)]) -> RuleCounts {
    RuleCounts(
        rules
            .iter()
            .map(|(id, visit, session)| {
                (
                    id.to_string(),
                    RuleCount {
                        visit: *visit,
                        session: *session,
                    },
                )
            })
            .collect(),
    )
}

fn stamped(attempt: u64, visit_attempt: u64, rules: Option<RuleCounts>) -> CheckEventFields {
    CheckEventFields {
        attempt: Some(attempt),
        visit_attempt: Some(visit_attempt),
        rule_counts: rules,
        duration_ms: Some(12),
        ..CheckEventFields::default()
    }
}

fn gate_event(
    state: &str,
    gate: &str,
    outcome: &str,
    output: serde_json::Value,
    check: CheckEventFields,
    poll: Option<PollRecord>,
) -> EventPayload {
    EventPayload::GateEvaluated {
        state: state.into(),
        gate: gate.into(),
        output,
        outcome: outcome.into(),
        timestamp: crate::engine::types::now_iso8601(),
        check,
        streams: None,
        poll,
    }
}

fn action_event(exit_code: i32, check: CheckEventFields) -> EventPayload {
    EventPayload::DefaultActionExecuted {
        state: "a".into(),
        command: "PLANTED-COMMAND --flag".into(),
        exit_code,
        stdout: "PLANTED-STDOUT".into(),
        stderr: String::new(),
        truncated: false,
        check,
    }
}

fn poll(status: &str, evaluations: u64, elapsed_secs: u64) -> PollRecord {
    PollRecord {
        status: status.into(),
        evaluations,
        since: "2026-01-01T00:00:00Z".into(),
        elapsed_secs,
    }
}

fn command_output(exit_code: i64) -> serde_json::Value {
    serde_json::json!({"exit_code": exit_code, "error": ""})
}

/// A session `wf` in state `a`: event 1 initializes it and event 2 enters
/// `a`, so its first commit is seq 3 in the visit that opened at 2.
fn session_in_a() -> Fixture {
    let f = Fixture::new();
    f.init("wf", header("wf", "wf-id", None), Some("a"));
    f
}

#[test]
fn a_failed_gate_relays_check_evaluated_then_one_check_rule_per_rule_in_order() {
    let f = session_in_a();
    let rules = counts(&[("Z9", 1, 4), ("E501", 2, 2), ("A1", 1, 1)]);
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "lint",
            "failed",
            command_output(1),
            stamped(3, 2, Some(rules)),
            None,
        ),
    );
    let j = f.journal();
    let c = check_records(&j);
    assert_eq!(
        kinds(&c),
        vec!["check_evaluated", "check_rule", "check_rule", "check_rule"]
    );
    let e = &c[0];
    assert_eq!(e["session"], "wf");
    assert_eq!(e["koto.session.id"], "wf-id");
    assert_eq!(e["koto.run.id"], "wf-id");
    assert_eq!(e["koto.seq"], 3);
    assert_eq!(e["koto.visit.seq"], 2);
    assert_eq!(e["koto.state"], "a");
    assert_eq!(e["koto.check"], "lint");
    assert_eq!(e["koto.check.outcome"], "failed");
    assert_eq!(e["koto.attempt"], 3);
    assert_eq!(e["koto.visit_attempt"], 2);
    assert_eq!(e["koto.check.rule_count"], 3);
    assert_eq!(e["koto.check.duration_ms"], 12);
    for absent in [
        "koto.check.rule_counts_truncated",
        "koto.poll.status",
        "koto.check.unanswered_count",
        "koto.check.error",
    ] {
        assert!(e.get(absent).is_none(), "{absent}: {e}");
    }
    let ids: Vec<&str> = c[1..]
        .iter()
        .map(|r| r["koto.rule.id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["Z9", "E501", "A1"], "rule_counts order");
    for r in &c[1..] {
        assert_eq!(r["koto.seq"], 3, "a rule shares its evaluation's seq");
        assert_eq!(r["koto.visit.seq"], 2);
        assert_eq!(r["koto.state"], "a");
        assert_eq!(r["koto.check"], "lint");
        assert_eq!(r["koto.attempt"], 3);
        assert_eq!(r["koto.visit_attempt"], 2);
    }
    assert_eq!(c[1]["koto.rule.visit_count"], 1);
    assert_eq!(c[1]["koto.rule.session_count"], 4);
    assert_eq!(c[2]["koto.rule.visit_count"], 2);
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("exit_code"), "{raw}");
    assert!(warnings_for_test().is_empty());
}

#[test]
fn gate_outcomes_are_written_as_logged_and_default_actions_map_to_failed_or_passed() {
    let f = session_in_a();
    for outcome in ["passed", "failed", "timed_out", "error"] {
        commit_as(
            &f,
            None,
            "wf",
            &gate_event(
                "a",
                "g",
                outcome,
                command_output(0),
                stamped(1, 1, None),
                None,
            ),
        );
    }
    // A failed action; one that exits 0 but whose capture failed (it
    // carries rule counts); and a passing one.
    commit_as(&f, None, "wf", &action_event(2, stamped(1, 1, None)));
    commit_as(
        &f,
        None,
        "wf",
        &action_event(0, stamped(1, 1, Some(counts(&[("capture", 1, 1)])))),
    );
    commit_as(&f, None, "wf", &action_event(0, stamped(1, 1, None)));
    let c = check_records(&f.journal());
    let evaluated: Vec<(&str, &str)> = c
        .iter()
        .filter(|r| r["kind"] == "check_evaluated")
        .map(|r| {
            (
                r["koto.check"].as_str().unwrap(),
                r["koto.check.outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        evaluated,
        vec![
            ("g", "passed"),
            ("g", "failed"),
            ("g", "timed_out"),
            ("g", "error"),
            ("__action__", "failed"),
            ("__action__", "failed"),
            ("__action__", "passed"),
        ]
    );
    // The action's rule record names its check `__action__`.
    let rule = c.iter().find(|r| r["kind"] == "check_rule").unwrap();
    assert_eq!(rule["koto.check"], "__action__");
    assert_eq!(rule["koto.rule.id"], "capture");
    // Seqs follow the log: init took 1 and 2.
    let seqs: Vec<u64> = c
        .iter()
        .filter(|r| r["kind"] == "check_evaluated")
        .map(|r| r["koto.seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, (3..=9).collect::<Vec<_>>());
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

#[test]
fn no_rule_counts_write_a_zero_count_and_fifty_with_truncation_write_fifty_rules() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "g",
            "failed",
            command_output(1),
            stamped(1, 1, None),
            None,
        ),
    );
    let ids: Vec<String> = (0..50).map(|i| format!("E{:03}", 100 + i)).collect();
    let rules: Vec<(&str, u64, u64)> = ids.iter().map(|id| (id.as_str(), 1, 1)).collect();
    let mut check = stamped(2, 2, Some(counts(&rules)));
    check.rule_counts_truncated = true;
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "g", "failed", command_output(1), check, None),
    );
    let c = check_records(&f.journal());
    assert_eq!(c[0]["kind"], "check_evaluated");
    assert_eq!(c[0]["koto.check.rule_count"], 0);
    assert!(c[0].get("koto.check.rule_counts_truncated").is_none());
    assert_eq!(c[1]["kind"], "check_evaluated");
    assert_eq!(c[1]["koto.check.rule_count"], 50);
    assert_eq!(c[1]["koto.check.rule_counts_truncated"], true);
    let written: Vec<&str> = c[2..]
        .iter()
        .map(|r| r["koto.rule.id"].as_str().unwrap())
        .collect();
    assert_eq!(written.len(), 50);
    assert_eq!(written, ids.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn each_when_field_is_present_exactly_under_its_condition() {
    let f = session_in_a();
    let decider = |unanswered: &[&str], error: &str| serde_json::json!({"failed": [], "unanswered": unanswered, "error": error});
    let no_duration = CheckEventFields {
        duration_ms: None,
        ..stamped(1, 1, None)
    };
    let payloads = [
        // A command gate: duration, no poll, no decider fields.
        gate_event(
            "a",
            "cmd",
            "passed",
            command_output(0),
            stamped(1, 1, None),
            None,
        ),
        // A context gate: no duration.
        gate_event(
            "a",
            "ctx",
            "passed",
            serde_json::json!({"exists": true, "error": ""}),
            no_duration,
            None,
        ),
        // A resolved polling gate.
        gate_event(
            "a",
            "ci",
            "passed",
            command_output(0),
            stamped(1, 1, None),
            Some(poll("done", 3, 40)),
        ),
        // Decider checks with nothing and one criterion unanswered, and
        // one that logged why nothing could be graded.
        gate_event(
            "a",
            "d0",
            "passed",
            decider(&[], ""),
            stamped(1, 1, None),
            None,
        ),
        gate_event(
            "a",
            "d1",
            "passed",
            decider(&["r"], ""),
            stamped(1, 1, None),
            None,
        ),
        gate_event(
            "a",
            "d2",
            "passed",
            decider(&["r", "s"], "log_unreadable"),
            stamped(1, 1, None),
            None,
        ),
        // A gate event from before attempt numbers existed.
        gate_event(
            "a",
            "old",
            "failed",
            command_output(1),
            CheckEventFields::default(),
            None,
        ),
    ];
    for p in &payloads {
        commit_as(&f, None, "wf", p);
    }
    let c = check_records(&f.journal());
    let by = |check: &str| c.iter().find(|r| r["koto.check"] == check).unwrap().clone();
    assert_eq!(by("cmd")["koto.check.duration_ms"], 12);
    assert!(by("ctx").get("koto.check.duration_ms").is_none());
    assert_eq!(by("ci")["koto.poll.status"], "done");
    assert!(by("cmd").get("koto.poll.status").is_none());
    assert_eq!(by("d0")["koto.check.unanswered_count"], 0);
    assert_eq!(by("d1")["koto.check.unanswered_count"], 1);
    assert_eq!(by("d2")["koto.check.unanswered_count"], 2);
    // An output from before the fallback lists no awaiting or answered
    // criteria: both counts are 0, not absent.
    for d in ["d0", "d1", "d2"] {
        assert_eq!(by(d)["koto.decider.awaiting_count"], 0, "{d}");
        assert_eq!(by(d)["koto.decider.agent_answered_count"], 0, "{d}");
    }
    for plain in ["cmd", "ctx", "ci", "old"] {
        assert!(
            by(plain).get("koto.check.unanswered_count").is_none(),
            "{plain}"
        );
        assert!(
            by(plain).get("koto.decider.awaiting_count").is_none(),
            "{plain}"
        );
        assert!(by(plain).get("koto.check.error").is_none(), "{plain}");
    }
    assert!(by("d0").get("koto.check.error").is_none());
    assert!(by("d1").get("koto.check.error").is_none());
    assert_eq!(by("d2")["koto.check.error"], "log_unreadable");
    let old = by("old");
    for absent in [
        "koto.attempt",
        "koto.visit_attempt",
        "koto.check.duration_ms",
    ] {
        assert!(old.get(absent).is_none(), "{absent}: {old}");
    }
    assert_eq!(old["koto.check.rule_count"], 0);
}

#[test]
fn a_pending_evaluation_writes_check_pending_with_the_poll_and_nothing_judged() {
    let f = session_in_a();
    let pending = CheckEventFields {
        duration_ms: Some(5),
        ..CheckEventFields::default()
    };
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "ci",
            "pending",
            serde_json::json!({"exit_code": 75, "error": ""}),
            pending,
            Some(poll("pending", 2, 31)),
        ),
    );
    let c = check_records(&f.journal());
    assert_eq!(kinds(&c), vec!["check_pending"]);
    let p = &c[0];
    assert_eq!(p["koto.state"], "a");
    assert_eq!(p["koto.check"], "ci");
    assert_eq!(p["koto.poll.status"], "pending");
    assert_eq!(p["koto.poll.evaluations"], 2);
    assert_eq!(p["koto.poll.elapsed_secs"], 31);
    assert_eq!(p["koto.seq"], 3);
    assert_eq!(p["koto.visit.seq"], 2);
}

/// A pending event that carries an attempt stamp and rule counts anyway
/// (koto doesn't write one, but a log can hold anything) still relays a
/// bare `check_pending`: nothing judged, and no rule record.
#[test]
fn a_pending_evaluation_carrying_judged_fields_still_writes_none_of_them() {
    let f = session_in_a();
    let mut judged = stamped(
        4,
        2,
        Some(counts(&[("E501", 2, 3), ("no-unused-vars", 1, 1)])),
    );
    judged.rule_counts_truncated = true;
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "ci",
            "pending",
            serde_json::json!({"exit_code": 75, "error": ""}),
            judged,
            Some(poll("pending", 1, 0)),
        ),
    );
    let c = check_records(&f.journal());
    assert_eq!(kinds(&c), vec!["check_pending"]);
    for absent in [
        "koto.check.outcome",
        "koto.attempt",
        "koto.visit_attempt",
        "koto.check.rule_count",
        "koto.check.rule_counts_truncated",
        "koto.check.duration_ms",
        "koto.rule.id",
        "koto.rule.visit_count",
        "koto.rule.session_count",
    ] {
        assert!(c[0].get(absent).is_none(), "{absent}: {}", c[0]);
    }
    assert_eq!(c[0]["koto.poll.evaluations"], 1);
    assert_eq!(c[0]["koto.poll.elapsed_secs"], 0);
}

/// The flush follows the records actually written: an identity record
/// dropped for the line cap doesn't make the check records beside it
/// flush.
#[test]
fn a_batch_whose_only_identity_record_is_dropped_is_not_flushed() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.home).unwrap();
    let relay = Relay {
        session: "wf",
        session_id: "wf-id",
        run_id: Some("wf-id"),
        seq: 3,
        visit_seq: Some(2),
    };
    let check = relayed(
        &gate_event(
            "a",
            "g",
            "passed",
            command_output(0),
            stamped(1, 1, None),
            None,
        ),
        &relay,
    );
    // A state entry under a long session name, longer than the check.
    let long_name = "w".repeat(255);
    let big = Record::new("state_entered", &long_name, "wf-id", Some("wf-id"))
        .name("koto.state", Some(&"s".repeat(128)[..]));
    let cap = check[0].to_line().len() + 1;
    assert!(
        big.to_line().len() + 1 > cap,
        "the identity record is the longer one"
    );
    set_max_line_for_test(cap);
    let mut records = vec![big];
    records.extend(check);
    write(&f.home, &records);
    assert_eq!(
        batches_for_test(),
        (1, 0),
        "only the check record was written"
    );
    assert_eq!(kinds(&f.journal()), vec!["check_evaluated"]);
    assert_eq!(warnings_for_test().len(), 1);
}

#[test]
fn rule_ids_outside_the_name_shape_are_digested_the_same_way_every_time() {
    use sha2::{Digest, Sha256};
    let digest = |s: &str| {
        let hex = hex::encode(Sha256::digest(s.as_bytes()));
        format!("sha256:{}", &hex[..32])
    };
    let odd = "no explicit any [x]";
    assert_eq!(name_or_digest(odd), digest(odd));
    assert_eq!(name_or_digest(odd), name_or_digest(odd));
    assert_eq!(name_or_digest(odd).len(), "sha256:".len() + 32);
    assert!(is_name(&name_or_digest(odd)));
    let longest = "r".repeat(128);
    for kept in [
        "@typescript-eslint/no-explicit-any",
        "E501",
        longest.as_str(),
    ] {
        assert_eq!(name_or_digest(kept), kept);
    }
    for digested in [
        "r".repeat(129),
        "règle".to_string(),
        "/abs/rule".to_string(),
        String::new(),
    ] {
        assert_eq!(name_or_digest(&digested), digest(&digested), "{digested}");
    }

    // Through a commit, in two sessions.
    let f = Fixture::new();
    for name in ["one", "two"] {
        f.init(name, header(name, &format!("{name}-id"), None), Some("a"));
        commit_as(
            &f,
            None,
            name,
            &gate_event(
                "a",
                "lint",
                "failed",
                command_output(1),
                stamped(1, 1, Some(counts(&[(odd, 1, 1)]))),
                None,
            ),
        );
    }
    let rules: Vec<serde_json::Value> = check_records(&f.journal())
        .into_iter()
        .filter(|r| r["kind"] == "check_rule")
        .collect();
    assert_eq!(rules.len(), 2);
    for r in &rules {
        assert_eq!(r["koto.rule.id"], digest(odd).as_str());
    }
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("explicit"), "{raw}");
}

/// A commit whose records are all check records is written in one batch
/// and not flushed; a state entry's batch is. A `context_read` writes no
/// batch at all.
#[test]
fn a_batch_with_no_identity_record_is_not_flushed() {
    let f = session_in_a();
    reset_for_test();
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "lint",
            "failed",
            command_output(1),
            stamped(
                1,
                1,
                Some(counts(&[("E501", 1, 1), ("no-unused-vars", 1, 1)])),
            ),
            None,
        ),
    );
    assert_eq!(
        batches_for_test(),
        (1, 0),
        "three records, one batch, no flush"
    );
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::ContextRead {
            key: "notes.md".into(),
            reader: "gate".into(),
            state: "a".into(),
            present: false,
            hash: None,
            access: None,
            gate: Some("lint".into()),
            session: None,
        },
    );
    assert_eq!(batches_for_test(), (1, 0), "a context read writes nothing");
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b"));
    assert_eq!(batches_for_test(), (2, 1), "a state entry is flushed");
    // A check event under a new driver carries `driver_seen`, so its batch
    // is flushed with it.
    commit_as(
        &f,
        Some("driver-z"),
        "wf",
        &gate_event(
            "b",
            "g",
            "passed",
            command_output(0),
            stamped(1, 1, None),
            None,
        ),
    );
    assert_eq!(batches_for_test(), (3, 2));
    let j = f.journal();
    let tail: Vec<String> = kinds(&j).into_iter().rev().take(2).rev().collect();
    assert_eq!(tail, vec!["driver_seen", "check_evaluated"]);
    assert!(warnings_for_test().is_empty());
}

/// The check records of a commit come after the identity records of the
/// same batch, and the visit follows arrivals: a self-transition keeps it,
/// an arrival elsewhere and a rewind open new ones.
#[test]
fn the_visit_seq_follows_arrivals_and_survives_a_self_transition() {
    let f = session_in_a();
    let check = |f: &Fixture, state: &str| {
        commit_as(
            f,
            None,
            "wf",
            &gate_event(
                state,
                "g",
                "passed",
                command_output(0),
                stamped(1, 1, None),
                None,
            ),
        )
    };
    check(&f, "a"); // 3, visit 2
    commit_as(&f, None, "wf", &transitioned(Some("a"), "a")); // 4
    check(&f, "a"); // 5, visit 2
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 6
    check(&f, "b"); // 7, visit 6
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::Rewound {
            from: "b".into(),
            to: "a".into(),
            rationale: None,
        },
    ); // 8
    check(&f, "a"); // 9, visit 8
    let visits: Vec<(u64, u64)> = check_records(&f.journal())
        .iter()
        .map(|r| {
            (
                r["koto.seq"].as_u64().unwrap(),
                r["koto.visit.seq"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(visits, vec![(3, 2), (5, 2), (7, 6), (9, 8)]);
}

/// With the longest names koto keeps, a 255-byte session name and fifty
/// 128-byte rule ids, every check record fits the line cap and every rule
/// is written.
#[test]
fn check_records_fit_the_line_cap_at_the_longest_names() {
    let session = "s".repeat(255);
    let state = "t".repeat(128);
    let gate = "g".repeat(128);
    let ids: Vec<String> = (0..50)
        .map(|i| format!("{i:02}{}", "r".repeat(126)))
        .collect();
    let rules: Vec<(&str, u64, u64)> = ids
        .iter()
        .map(|id| (id.as_str(), u64::MAX, u64::MAX))
        .collect();
    let mut check = CheckEventFields {
        attempt: Some(u64::MAX),
        visit_attempt: Some(u64::MAX),
        rule_counts: Some(counts(&rules)),
        duration_ms: Some(u64::MAX),
        ..CheckEventFields::default()
    };
    check.rule_counts_truncated = true;
    let id = "i".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let unanswered = vec!["x"; 50];
    let decider_output = serde_json::json!({
        "failed": [],
        "unanswered": unanswered,
        "error": "e".repeat(128),
    });
    let mut lines: Vec<String> = Vec::new();
    for (outcome, output, poll_record) in [
        (
            "failed",
            decider_output.clone(),
            Some(poll("timed_out", u64::MAX, u64::MAX)),
        ),
        (
            "pending",
            command_output(75),
            Some(poll("pending", u64::MAX, u64::MAX)),
        ),
    ] {
        let payload = gate_event(&state, &gate, outcome, output, check.clone(), poll_record);
        lines.extend(relayed(&payload, &relay).iter().map(Record::to_line));
    }
    assert_eq!(lines.len(), 1 + 50 + 1);
    for line in &lines {
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes: {line}", line.len());
    }
    for id in &ids {
        assert!(lines.iter().any(|l| l.contains(id.as_str())), "{id}");
    }
}

// ----- Response records -----

/// The `response` records in `j`, each checked to carry `v` 1, both
/// sequence numbers, and exactly its kind's fields.
fn response_records(j: &[serde_json::Value]) -> Vec<serde_json::Value> {
    j.iter()
        .filter(|r| r["kind"] == "response")
        .map(|r| {
            let expected: BTreeSet<&str> = RELAY_FIELDS
                .iter()
                .chain(check_kind_fields("response"))
                .copied()
                .collect();
            let got: BTreeSet<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(got, expected, "{r}");
            assert_eq!(r["v"], 1);
            r.clone()
        })
        .collect()
}

/// The log of `session` as it stands, read the way `response` reads it.
fn log_of(f: &Fixture, session: &str) -> String {
    std::fs::read_to_string(
        f.backend
            .session_dir(session)
            .join(crate::session::state_file_name(session)),
    )
    .unwrap()
}

/// A response record carries the log's highest seq and its visit, the
/// UTF-8 byte lengths of what was printed (0 for no details), and nothing
/// of the text. It is written in its own batch without a flush.
#[test]
fn a_response_records_the_printed_byte_lengths_and_the_logs_position() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &evidence("a")); // 3
    reset_for_test();
    let directive = "Corrija o código — ✓ PLANTED-DIRECTIVE";
    let details = "Détails PLANTED-DETAILS";
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Next,
        directive,
        Some(details),
    );
    assert_eq!(batches_for_test(), (1, 0), "one batch, no flush");
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        directive,
        None,
    );
    let r = response_records(&f.journal());
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["session"], "wf");
    assert_eq!(r[0]["koto.session.id"], "wf-id");
    assert_eq!(r[0]["koto.run.id"], "wf-id");
    assert_eq!(r[0]["koto.state"], "a");
    assert_eq!(r[0]["koto.response.source"], "next");
    assert_eq!(r[0]["koto.response.directive_bytes"], directive.len());
    assert_ne!(directive.len(), directive.chars().count());
    assert_eq!(r[0]["koto.response.details_bytes"], details.len());
    assert_eq!(r[1]["koto.response.source"], "status");
    assert_eq!(r[1]["koto.response.details_bytes"], 0);
    let log = log_of(&f, "wf");
    for rec in &r {
        assert_eq!(rec["koto.seq"], 3);
        assert_eq!(
            rec["koto.seq"].as_u64(),
            crate::engine::persistence::last_seq(&log)
        );
        assert_eq!(rec["koto.visit.seq"], 2);
    }
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
    assert!(warnings_for_test().is_empty());
}

/// A response and the check records of the same visit carry the same
/// `koto.visit.seq`, since both come from `persistence::visit_start`: a
/// self-transition keeps the visit, and an arrival elsewhere or a rewind
/// opens a new one.
#[test]
fn a_response_shares_the_visit_rule_with_the_check_records() {
    let f = session_in_a();
    let respond = |f: &Fixture, state: &str| {
        response(
            Some(&f.home),
            &f.backend,
            "wf",
            state,
            ResponseSource::Next,
            "d",
            None,
        )
    };
    let check = |f: &Fixture, state: &str| {
        commit_as(
            f,
            None,
            "wf",
            &gate_event(
                state,
                "g",
                "passed",
                command_output(0),
                stamped(1, 1, None),
                None,
            ),
        )
    };
    check(&f, "a"); // 3, visit 2
    respond(&f, "a");
    commit_as(&f, None, "wf", &transitioned(Some("a"), "a")); // 4
    respond(&f, "a");
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 5
    check(&f, "b"); // 6, visit 5
    respond(&f, "b");
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::Rewound {
            from: "b".into(),
            to: "a".into(),
            rationale: None,
        },
    ); // 7
    respond(&f, "a");
    let j = f.journal();
    let got: Vec<(String, u64, u64)> = j
        .iter()
        .filter(|r| r["kind"] == "response" || r["kind"] == "check_evaluated")
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["koto.seq"].as_u64().unwrap(),
                r["koto.visit.seq"].as_u64().unwrap(),
            )
        })
        .collect();
    let pair = |k: &str, seq: u64, visit: u64| (k.to_string(), seq, visit);
    assert_eq!(
        got,
        vec![
            pair("check_evaluated", 3, 2),
            pair("response", 3, 2),
            pair("response", 4, 2),
            pair("check_evaluated", 6, 5),
            pair("response", 6, 5),
            pair("response", 7, 7),
        ]
    );
    assert_eq!(response_records(&j).len(), 4);
}

/// No journal root writes nothing. A session whose log can't be read
/// writes nothing and warns once.
#[test]
fn a_response_without_a_journal_or_a_log_writes_nothing() {
    let f = session_in_a();
    let before = f.journal().len();
    response(None, &f.backend, "wf", "a", ResponseSource::Next, "d", None);
    assert_eq!(f.journal().len(), before);
    assert!(warnings_for_test().is_empty());
    for _ in 0..2 {
        response(
            Some(&f.home),
            &f.backend,
            "gone",
            "a",
            ResponseSource::Status,
            "d",
            None,
        );
    }
    assert_eq!(f.journal().len(), before);
    assert_eq!(warnings_for_test().len(), 1);
}

/// With a 255-byte session name, a 128-byte state and the largest counts,
/// a response record fits the line cap.
#[test]
fn a_response_record_fits_the_line_cap_at_the_longest_names() {
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let state = "t".repeat(128);
    let long = "x".repeat(1 << 20);
    let record = response_record(&relay, &state, ResponseSource::Status, &long, Some(&long));
    let line = record.to_line();
    assert!(line.len() < MAX_LINE_BYTES, "{} bytes: {line}", line.len());
    assert!(line.contains(&state));
    assert!(line.contains(&format!("\"koto.response.directive_bytes\":{}", 1 << 20)));
}

/// When the log no longer stands in the printed state (another command
/// advanced the session between the print and the record), no record is
/// written and nothing warns.
#[test]
fn a_response_for_a_state_the_log_has_left_is_skipped() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 3
    let before = f.journal().len();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "d",
        None,
    );
    assert_eq!(f.journal().len(), before, "printed a, the log is in b");
    assert!(warnings_for_test().is_empty());
    // The state the log is in is still recorded.
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "b",
        ResponseSource::Status,
        "d",
        None,
    );
    let r = response_records(&f.journal());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["koto.state"], "b");
    assert_eq!(r[0]["koto.seq"], 3);
    assert_eq!(r[0]["koto.visit.seq"], 3);
}

/// The hook writes only the journal: a session with no sidecar gets its
/// run id derived from the header, and still has no sidecar afterwards.
#[test]
fn a_response_derives_the_run_id_without_writing_the_sidecar() {
    let f = session_in_a();
    let sidecar_path = f.backend.session_dir("wf").join(sidecar::SIDECAR_FILE);
    std::fs::remove_file(&sidecar_path).unwrap();
    let files_before: Vec<_> = std::fs::read_dir(f.backend.session_dir("wf"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "d",
        None,
    );
    let r = response_records(&f.journal());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["koto.run.id"], "wf-id");
    assert!(!sidecar_path.exists(), "the hook wrote the sidecar");
    let files_after: Vec<_> = std::fs::read_dir(f.backend.session_dir("wf"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files_after, files_before);
}

/// With the journal's lock held by another writer, a `koto status`
/// response gives up after its short wait: no record, one warning, and no
/// hang.
#[test]
fn a_response_skips_its_record_while_another_writer_holds_the_lock() {
    use std::os::unix::io::AsRawFd;
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    let holder = std::fs::File::open(&journal).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let started = std::time::Instant::now();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "d",
        None,
    );
    let took = started.elapsed();
    assert!(took >= RESPONSE_LOCK_WAIT, "{took:?}");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    let warnings = warnings_for_test();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("held by another writer"),
        "{warnings:?}"
    );
    drop(holder);
}

/// `koto next`'s response waits for the journal's lock as long as every
/// other journal write: held well past the status bound but released within
/// the journal's, the lock is waited out, and the record is written once
/// it's released, with no warning.
#[test]
fn a_next_response_waits_for_a_held_lock_and_is_written() {
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let holder = std::fs::File::open(&journal).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let hold = RESPONSE_LOCK_WAIT * 5;
    let started = Instant::now();
    let release = std::thread::spawn(move || {
        std::thread::sleep(hold);
        drop(holder);
    });
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Next,
        "d",
        None,
    );
    let took = started.elapsed();
    release.join().unwrap();
    assert!(took >= hold, "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let r = response_records(&f.journal());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["koto.response.source"], "next");
    assert!(warnings_for_test().is_empty(), "{:?}", warnings_for_test());
}

/// How long [`hold_journal_lock`] holds the lock at most. Far past every
/// bound these tests set, so a write that waited without a bound would
/// fail their timing checks rather than hang the test run.
#[cfg(unix)]
const HOLD_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(10);

/// The journal's lock, held by a thread of its own as a stopped writer
/// would hold it, until this is dropped or [`HOLD_WATCHDOG`] passes.
#[cfg(unix)]
struct LockHolder {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for LockHolder {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// Take `journal`'s lock from another thread (see [`LockHolder`]) and
/// return once it is held.
#[cfg(unix)]
fn hold_journal_lock(journal: &Path) -> LockHolder {
    use std::os::unix::io::AsRawFd;
    let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let path = journal.to_path_buf();
    let thread = std::thread::spawn(move || {
        let holder = std::fs::File::open(&path).unwrap();
        // SAFETY: flock on a descriptor `holder` owns until it is dropped.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        locked_tx.send(()).unwrap();
        // Released when the test drops its `LockHolder`, or by the watchdog.
        let _ = release_rx.recv_timeout(HOLD_WATCHDOG);
        drop(holder);
    });
    locked_rx.recv().unwrap();
    LockHolder {
        release: Some(release_tx),
        thread: Some(thread),
    }
}

/// Every journal write has a bound, not only `koto status`'s: with the
/// lock held past it, a `koto next` response gives up after about the
/// bound, writes nothing, and warns once.
#[cfg(unix)]
#[test]
fn a_next_response_skips_its_record_once_the_journal_bound_passes() {
    use std::time::{Duration, Instant};
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    let bound = Duration::from_millis(50);
    set_lock_wait_for_test(bound);
    let holder = hold_journal_lock(&journal);
    let started = Instant::now();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Next,
        "d",
        None,
    );
    let took = started.elapsed();
    drop(holder);
    assert!(took >= bound, "{took:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    let warnings = warnings_for_test();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("held by another writer"),
        "{warnings:?}"
    );
}

/// A `koto status` response waits no longer than the journal's own bound
/// when that is the tighter one.
#[cfg(unix)]
#[test]
fn a_status_response_takes_the_tighter_of_its_wait_and_the_journal_bound() {
    use std::time::Duration;
    let f = session_in_a();
    set_lock_wait_for_test(Duration::from_millis(5));
    assert_eq!(journal_lock_wait(), Duration::from_millis(5));
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    let holder = hold_journal_lock(&journal);
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "d",
        None,
    );
    drop(holder);
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    assert_eq!(warnings_for_test().len(), 1);
    // Outside tests the journal's bound is the five seconds, and a status
    // response keeps its 30 milliseconds.
    reset_for_test();
    assert_eq!(journal_lock_wait(), JOURNAL_LOCK_WAIT);
    assert_eq!(JOURNAL_LOCK_WAIT, Duration::from_secs(5));
    assert_eq!(
        RESPONSE_LOCK_WAIT.min(JOURNAL_LOCK_WAIT),
        RESPONSE_LOCK_WAIT
    );
}

/// A held lock costs a process one wait: once a write has given up on it,
/// the next write in the same process tries the lock once and returns at
/// once, and the two skipped batches print one warning between them. With
/// the lock free again, a later write takes it on that one try and is
/// written.
#[cfg(unix)]
#[test]
fn after_one_write_gives_up_later_writes_in_the_process_do_not_wait() {
    use std::time::{Duration, Instant};
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    let bound = Duration::from_millis(200);
    set_lock_wait_for_test(bound);
    let holder = hold_journal_lock(&journal);
    let next = || {
        let started = Instant::now();
        response(
            Some(&f.home),
            &f.backend,
            "wf",
            "a",
            ResponseSource::Next,
            "d",
            None,
        );
        started.elapsed()
    };

    let first = next();
    assert!(first >= bound, "{first:?}");
    assert!(gave_up());
    let second = next();
    assert!(second < bound / 4, "{second:?}");
    drop(holder);

    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    let warnings = warnings_for_test();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("held by another writer"),
        "{warnings:?}"
    );

    // The lock is free: the single try takes it, and clears the state.
    let third = next();
    assert!(third < bound / 4, "{third:?}");
    assert_eq!(response_records(&f.journal()).len(), 1);
    assert_eq!(warnings_for_test().len(), 1);
    assert!(!gave_up());
}

/// The process stops waiting only until a write takes the lock again: after
/// a write gives up, one that finds the lock free clears the state, and the
/// next contended write waits its full bound again instead of skipping at
/// once. A long-running process recovers from one stall.
#[cfg(unix)]
#[test]
fn a_write_that_takes_the_lock_restores_the_full_wait() {
    use std::time::{Duration, Instant};
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let bound = Duration::from_millis(150);
    set_lock_wait_for_test(bound);
    let next = || {
        let started = Instant::now();
        response(
            Some(&f.home),
            &f.backend,
            "wf",
            "a",
            ResponseSource::Next,
            "d",
            None,
        );
        started.elapsed()
    };

    // A stall: the write gives up at its bound.
    let holder = hold_journal_lock(&journal);
    let stalled = next();
    drop(holder);
    assert!(stalled >= bound, "{stalled:?}");
    assert!(gave_up());

    // The stall passes: a write takes the lock and clears the state.
    let free = next();
    assert!(free < bound / 2, "{free:?}");
    assert!(!gave_up());
    assert_eq!(response_records(&f.journal()).len(), 1);

    // The next contended write waits its full bound again, and gets the
    // lock once it is released within that bound.
    let holder = hold_journal_lock(&journal);
    let release = std::thread::spawn(move || {
        std::thread::sleep(bound / 3);
        drop(holder);
    });
    let contended = next();
    release.join().unwrap();
    assert!(contended >= bound / 3, "{contended:?}");
    assert!(contended < bound * 2, "{contended:?}");
    assert_eq!(response_records(&f.journal()).len(), 2);
    assert!(!gave_up());
    // The one warning, from the stall.
    assert_eq!(warnings_for_test().len(), 1, "{:?}", warnings_for_test());
}

/// Only a lock wait that ran out sets the process to stop waiting: a write
/// that failed for another reason (a record over the line cap) leaves the
/// next write its full wait.
#[cfg(unix)]
#[test]
fn a_write_that_fails_for_another_reason_keeps_the_wait() {
    let f = session_in_a();
    set_max_line_for_test(16);
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Next,
        "d",
        None,
    );
    assert_eq!(warnings_for_test().len(), 1);
    assert!(!gave_up());

    // A symlink at the journal's path fails the open, not the lock.
    set_max_line_for_test(MAX_LINE_BYTES);
    let journal = f.home.join(JOURNAL_FILE);
    std::fs::remove_file(&journal).unwrap();
    std::os::unix::fs::symlink(f.home.join("elsewhere"), &journal).unwrap();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Next,
        "d",
        None,
    );
    assert!(!gave_up());
}

/// The terminal tick's record is bounded like the rest: with the lock held
/// past the bound, `terminal` returns after about the bound with no record
/// and one warning, so the tick's cleanup that follows it still runs.
#[cfg(unix)]
#[test]
fn a_terminal_record_is_skipped_and_returns_once_the_bound_passes() {
    use std::time::{Duration, Instant};
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    let batches = batches_for_test();
    let bound = Duration::from_millis(50);
    set_lock_wait_for_test(bound);
    let holder = hold_journal_lock(&journal);
    let started = Instant::now();
    terminal(
        Some(&f.home),
        &f.backend,
        "wf",
        &header("wf", "wf-id", None),
        "done",
    );
    let took = started.elapsed();
    drop(holder);
    assert!(took >= bound, "{took:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    assert_eq!(warnings_for_test().len(), 1, "{:?}", warnings_for_test());
    assert_eq!(batches_for_test(), batches);

    // With the lock free again the next terminal is written.
    terminal(
        Some(&f.home),
        &f.backend,
        "wf",
        &header("wf", "wf-id", None),
        "done",
    );
    let last = f.journal().pop().unwrap();
    assert_eq!(last["kind"], "terminal");
    assert_eq!(last["koto.terminal"], "done");
}

/// Rotation with a bound every journal is past.
#[cfg(unix)]
fn rotate_at_one_byte() {
    rotate::set_settings_for_test(rotate::Settings {
        rotate_bytes: 1,
        retain_days: 30,
        reader_stale_days: 90,
    });
}

/// With rotation on and the journal past its bound, a response rotates the
/// journal before writing: the rotated file keeps every byte it had, and
/// the record lands whole, alone, in the fresh journal.
#[cfg(unix)]
#[test]
fn a_response_lands_whole_in_the_fresh_journal_after_a_rotation() {
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    assert!(!before.is_empty());
    rotate_at_one_byte();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "directive",
        Some("details"),
    );
    let rotated = f.home.join(rotate::rotated_name(1));
    assert_eq!(std::fs::read_to_string(&rotated).unwrap(), before);
    let fresh = std::fs::read_to_string(&journal).unwrap();
    assert!(fresh.ends_with('\n'), "{fresh:?}");
    let j = f.journal();
    assert_eq!(j.len(), 1, "{fresh}");
    let r = response_records(&j);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["session"], "wf");
    assert_eq!(r[0]["koto.state"], "a");
    assert_eq!(r[0]["koto.response.source"], "status");
    assert_eq!(r[0]["koto.response.directive_bytes"], 9);
    assert_eq!(r[0]["koto.response.details_bytes"], 7);
    assert!(warnings_for_test().is_empty(), "{:?}", warnings_for_test());
    assert!(
        rotate::warnings_for_test().is_empty(),
        "{:?}",
        rotate::warnings_for_test()
    );
}

/// Rotation doesn't lengthen a response's wait for a held lock, and nothing
/// is rotated while it's held: the journal past its bound stays where it
/// is, unchanged, and the record is skipped with the one warning.
#[cfg(unix)]
#[test]
fn a_response_skips_its_record_while_the_lock_is_held_with_rotation_on() {
    use std::os::unix::io::AsRawFd;
    let f = session_in_a();
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    rotate_at_one_byte();
    let holder = std::fs::File::open(&journal).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let started = std::time::Instant::now();
    response(
        Some(&f.home),
        &f.backend,
        "wf",
        "a",
        ResponseSource::Status,
        "d",
        None,
    );
    let took = started.elapsed();
    drop(holder);
    assert!(took >= RESPONSE_LOCK_WAIT, "{took:?}");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), before);
    assert!(!f.home.join(rotate::rotated_name(1)).exists());
    let warnings = warnings_for_test();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("held by another writer"),
        "{warnings:?}"
    );
}

// ----- Decider and override records -----

use crate::decider::check::{DeciderCheck, RecordedOutcome, UnansweredReason};
use crate::decider::types::{ErrorClass, SettingOrigin};
use crate::decider::{ConsultationOutcome, DeciderConsultation, FieldConsultation};
use crate::template::decider_check::CheckMode;

/// The fields a decider or override kind may carry beyond [`RELAY_FIELDS`].
fn decider_kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "decider_consulted" => &[
            "koto.state",
            "koto.decider.provider",
            "koto.decider.model",
            "koto.decider.outcome",
            "koto.decider.error_class",
            "koto.decider.input_tokens",
            "koto.decider.output_tokens",
            "koto.decider.unread_usage_attempts",
            "koto.decider.latency_ms",
            "koto.decider.directive_bytes",
            "koto.decider.reason",
            "koto.decider.input_label",
            "koto.decider.input_bytes",
            "koto.decider.field_count",
            "koto.decider.field",
            "koto.decider.answer",
            "koto.decider.mode",
            "koto.decider.threshold_met",
        ],
        "decider_field" => &[
            "koto.state",
            "koto.decider.consulted_seq",
            "koto.decider.field",
            "koto.decider.answer",
            "koto.decider.mode",
            "koto.decider.threshold_met",
        ],
        "decider_answered" => &[
            "koto.state",
            "koto.decider.field",
            "koto.decider.agent_answer",
            "koto.decider.consulted_seq",
        ],
        "decider_check_answered" => &[
            "koto.state",
            "koto.check",
            "koto.rule.id",
            "koto.unit.key",
            "koto.unit.document",
            "koto.unit.section",
            "koto.decider.answer",
            "koto.decider.reason",
            "koto.decider.fallback_seq",
        ],
        "decider_checked" => &[
            "koto.state",
            "koto.check",
            "koto.rule.id",
            "koto.unit.key",
            "koto.unit.document",
            "koto.unit.section",
            "koto.decider.provider",
            "koto.decider.model",
            "koto.decider.mode",
            "koto.decider.outcome",
            "koto.decider.confidence_bp",
            "koto.decider.reason",
            "koto.decider.blocked",
            "koto.decider.error_class",
            "koto.decider.input_tokens",
            "koto.decider.output_tokens",
            "koto.decider.unread_usage_attempts",
            "koto.decider.attempts",
            "koto.decider.latency_ms",
            "koto.decider.input_bytes",
        ],
        "decider_fallback" => &[
            "koto.state",
            "koto.check",
            "koto.rule.id",
            "koto.unit.key",
            "koto.unit.document",
            "koto.unit.section",
            "koto.decider.mode",
            "koto.decider.reason",
            "koto.decider.input_bytes",
        ],
        "rule_delivered" => &[
            "koto.state",
            "koto.check.name",
            "koto.rule.id",
            "koto.rule.extent",
            "koto.rule.unit_count",
            "koto.rule.via",
            "koto.rule.delivery",
        ],
        "check_overridden" => &["koto.state", "koto.check"],
        other => panic!("not a decider or override kind: {other}"),
    }
}

/// The decider and override records in `j`, each checked to carry `v` 1,
/// both sequence numbers, and only its kind's fields.
fn decider_records(j: &[serde_json::Value]) -> Vec<serde_json::Value> {
    j.iter()
        .filter(|r| {
            matches!(
                r["kind"].as_str().unwrap(),
                "decider_consulted"
                    | "decider_field"
                    | "decider_answered"
                    | "decider_checked"
                    | "decider_fallback"
                    | "decider_check_answered"
                    | "rule_delivered"
                    | "check_overridden"
            )
        })
        .map(|r| {
            let kind = r["kind"].as_str().unwrap();
            let allowed: BTreeSet<&str> = RELAY_FIELDS
                .iter()
                .chain(decider_kind_fields(kind))
                .copied()
                .collect();
            for (key, value) in r.as_object().unwrap() {
                assert!(allowed.contains(key.as_str()), "{kind} carries {key}: {r}");
                assert!(!value.is_null() && value != "", "{key}: {r}");
            }
            assert_eq!(r["v"], 1);
            assert!(r["koto.seq"].is_u64(), "{r}");
            assert!(r["koto.visit.seq"].is_u64(), "{r}");
            r.clone()
        })
        .collect()
}

/// A routing consultation of state `a` whose own `visit_seq` (99) is not
/// koto's visit, with a planted hash and a probability in its field detail.
fn consultation() -> DeciderConsultation {
    let mut probabilities = std::collections::BTreeMap::new();
    probabilities.insert("proceed".to_string(), 0.4321);
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "verdict".to_string(),
        FieldConsultation {
            declaration_hash: "PLANTED-DECLARATION-HASH".into(),
            modes: Default::default(),
            probabilities,
            winning: Some("proceed".into()),
            confidence: Some(0.4321),
            threshold: Some(0.875),
            at_threshold: false,
            outcome: None,
        },
    );
    DeciderConsultation {
        state: "a".into(),
        visit_seq: 99,
        provider: "jev".into(),
        model: "jev-test-1.2.3".into(),
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        outcome: ConsultationOutcome::NotApplied,
        error_class: None,
        latency_ms: 321,
        directive_bytes: 4567,
        endpoint_origin: SettingOrigin::Env,
        fields,
        input_tokens: Some(812),
        output_tokens: Some(9),
        unread_usage_attempts: 0,
        reason: None,
        input_label: None,
        input_bytes: None,
    }
}

/// One criterion's verdict on gate `comments` in state `a`, whose own
/// `visit_seq` (99) is not koto's visit, with a planted rule reference,
/// hashes, threshold and probabilities. Only the highest probability, fail's
/// 0.5432, reaches the journal, as `koto.decider.confidence_bp` 5432; the
/// planted 0.4321 and 0.0247 reach it in no form.
fn criterion_check(rule_id: &str) -> DeciderCheck {
    let mut probabilities = std::collections::BTreeMap::new();
    probabilities.insert("pass".to_string(), 0.4321);
    probabilities.insert("fail".to_string(), 0.5432);
    probabilities.insert("unclear".to_string(), 0.0247);
    DeciderCheck {
        state: "a".into(),
        visit_seq: 99,
        gate: "comments".into(),
        rule_id: rule_id.into(),
        rule_ref: "https://example.org/PLANTED-RULE-REF".into(),
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        mode: CheckMode::Veto,
        threshold: 0.875,
        outcome: RecordedOutcome::Fail,
        reason: None,
        blocked: true,
        provider: "jev".into(),
        model: "jev-test-1.2.3".into(),
        probabilities,
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        input_bytes: Some(34),
        input_tokens: Some(100),
        output_tokens: Some(3),
        unread_usage_attempts: 0,
        attempts: 1,
        latency_ms: 12,
        error_class: None,
        endpoint_origin: Some(SettingOrigin::Env),
        unit: None,
        unit_labels: None,
    }
}

/// Nothing the decider events carry beyond the record's fields reaches the
/// journal. The field's name, `verdict`, is one of them
/// (`koto.decider.field`); its winning value, `proceed`, isn't, since the
/// fixture's field got no answer.
///
/// No probability reaches it in any form either: not as written, and not
/// scaled as basis points. The one value derived from one is
/// `koto.decider.confidence_bp`, the highest probability of a decider-check
/// criterion, which the fixtures plant as 0.5432 and so is allowed as 5432;
/// every other planted probability (0.4321, 0.0247) is checked absent, in
/// either form, from every field.
fn assert_no_decider_content(f: &Fixture) {
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for absent in [
        "PLANTED",
        "0.4321",
        "0.5432",
        "0.0247",
        "0.875",
        "probabilit",
        "\"threshold\"",
        "endpoint",
        "rule_ref",
        "proceed",
        "\"env\"",
    ] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
    for record in f.journal() {
        for (key, value) in record.as_object().unwrap() {
            if let Some(n) = value.as_f64() {
                for planted in [4321.0, 247.0, 0.4321, 0.0247, 0.5432] {
                    assert!(n != planted, "{key} carries {n}: {record}");
                }
                if n == 5432.0 {
                    assert_eq!(key, CONFIDENCE_FIELD, "{record}");
                }
            }
        }
    }
}

#[test]
fn a_consultation_relays_decider_consulted_with_the_events_values() {
    let f = session_in_a();
    reset_for_test();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderConsulted(consultation()),
    );
    // An error with no usage read: tokens absent, the error class present,
    // the unread count as logged.
    let failed = DeciderConsultation {
        outcome: ConsultationOutcome::Error,
        error_class: Some(ErrorClass::Timeout),
        model: "unknown".into(),
        input_tokens: None,
        output_tokens: None,
        unread_usage_attempts: 2,
        ..consultation()
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderConsulted(failed));
    assert_eq!(
        batches_for_test(),
        (2, 0),
        "relayed, so never flushed alone"
    );

    let d = decider_records(&f.journal());
    assert_eq!(kinds(&d), vec!["decider_consulted", "decider_consulted"]);
    let r = &d[0];
    assert_eq!(r["session"], "wf");
    assert_eq!(r["koto.session.id"], "wf-id");
    assert_eq!(r["koto.run.id"], "wf-id");
    assert_eq!(r["koto.seq"], 3);
    assert_eq!(r["koto.visit.seq"], 2, "koto's visit, not the event's 99");
    assert_eq!(r["koto.state"], "a");
    assert_eq!(r["koto.decider.provider"], "jev");
    assert_eq!(r["koto.decider.model"], "jev-test-1.2.3");
    assert_eq!(r["koto.decider.outcome"], "not_applied");
    assert_eq!(r["koto.decider.input_tokens"], 812);
    assert_eq!(r["koto.decider.output_tokens"], 9);
    assert_eq!(r["koto.decider.unread_usage_attempts"], 0);
    assert_eq!(r["koto.decider.latency_ms"], 321);
    assert_eq!(r["koto.decider.directive_bytes"], 4567);
    assert!(r.get("koto.decider.error_class").is_none(), "{r}");
    // One field, which got no answer and declares no values.
    assert_eq!(r["koto.decider.field_count"], 1);
    assert_eq!(r["koto.decider.field"], "verdict");
    assert_eq!(r["koto.decider.answer"], "no_answer");
    for absent in [
        "koto.decider.mode",
        "koto.decider.threshold_met",
        "koto.decider.reason",
        "koto.decider.input_label",
        "koto.decider.input_bytes",
    ] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }

    let r = &d[1];
    assert_eq!(r["koto.seq"], 4);
    assert_eq!(r["koto.visit.seq"], 2);
    assert_eq!(r["koto.decider.outcome"], "error");
    assert_eq!(r["koto.decider.error_class"], "timeout");
    assert_eq!(r["koto.decider.model"], "unknown");
    assert_eq!(r["koto.decider.unread_usage_attempts"], 2);
    for absent in ["koto.decider.input_tokens", "koto.decider.output_tokens"] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }
    assert_no_decider_content(&f);
    assert!(warnings_for_test().is_empty());
}

#[test]
fn a_criterion_relays_decider_checked_with_the_events_values() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderChecked(criterion_check("comment_reason")),
    );
    // A shadow criterion left unanswered: the reason and error class
    // present, no tokens, nothing sent.
    let unanswered = DeciderCheck {
        mode: CheckMode::Shadow,
        outcome: RecordedOutcome::Unanswered,
        reason: Some(UnansweredReason::Busy),
        blocked: false,
        model: "unknown".into(),
        probabilities: Default::default(),
        input_tokens: None,
        output_tokens: None,
        unread_usage_attempts: 1,
        attempts: 0,
        latency_ms: 0,
        error_class: Some(ErrorClass::HttpStatus),
        ..criterion_check("no reason [x]")
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderChecked(unanswered));

    let d = decider_records(&f.journal());
    assert_eq!(kinds(&d), vec!["decider_checked", "decider_checked"]);
    let r = &d[0];
    assert_eq!(r["koto.seq"], 3);
    assert_eq!(r["koto.visit.seq"], 2, "koto's visit, not the event's 99");
    assert_eq!(r["koto.state"], "a");
    assert_eq!(r["koto.check"], "comments");
    assert_eq!(r["koto.rule.id"], "comment_reason");
    assert_eq!(r["koto.decider.provider"], "jev");
    assert_eq!(r["koto.decider.model"], "jev-test-1.2.3");
    assert_eq!(r["koto.decider.mode"], "veto");
    assert_eq!(r["koto.decider.outcome"], "fail");
    assert_eq!(r["koto.decider.blocked"], true);
    assert_eq!(r["koto.decider.input_tokens"], 100);
    assert_eq!(r["koto.decider.output_tokens"], 3);
    assert_eq!(r["koto.decider.unread_usage_attempts"], 0);
    assert_eq!(r["koto.decider.attempts"], 1);
    assert_eq!(r["koto.decider.latency_ms"], 12);
    for absent in ["koto.decider.reason", "koto.decider.error_class"] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }
    // An answer was read: its highest probability, as basis points.
    assert_eq!(r["koto.decider.confidence_bp"], 5432);
    // A check without units writes no unit field.
    for absent in ["koto.unit.key", "koto.unit.document", "koto.unit.section"] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }

    let r = &d[1];
    assert_eq!(r["koto.seq"], 4);
    assert_eq!(
        r["koto.rule.id"],
        name_or_digest("no reason [x]").as_str(),
        "a criterion id outside the name shape is digested"
    );
    assert_eq!(r["koto.decider.mode"], "shadow");
    assert_eq!(r["koto.decider.outcome"], "unanswered");
    assert_eq!(r["koto.decider.reason"], "busy");
    assert_eq!(r["koto.decider.blocked"], false);
    assert_eq!(r["koto.decider.error_class"], "http_status");
    assert_eq!(r["koto.decider.unread_usage_attempts"], 1);
    assert_eq!(r["koto.decider.attempts"], 0);
    assert_eq!(r["koto.decider.latency_ms"], 0);
    for absent in ["koto.decider.input_tokens", "koto.decider.output_tokens"] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }
    // No answer was read, so no confidence.
    assert!(r.get("koto.decider.confidence_bp").is_none(), "{r}");
    assert_no_decider_content(&f);
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("no reason"), "{raw}");
}

/// A unit-mode criterion's records: the unit's key as its digest, the
/// check's declared labels where it declares them, the digest of what the
/// extraction printed where it doesn't, and nothing where neither exists.
/// No raw key, document or section reaches the journal.
#[test]
fn a_unit_mode_check_relays_the_units_digests_and_declared_labels() {
    use crate::decider::check::{
        AgentAnswer, DeciderCheckAnswered, DeciderFallback, UnitLabels, UnitRef,
    };
    let f = session_in_a();
    let unit = UnitRef {
        key: "SENTINEL-KEY-ac-3".into(),
        // Not path-shaped, so the doc-names check doesn't read it as a
        // repository path.
        document: Some("SENTINEL-DOC".into()),
        section: Some("SENTINEL-SECTION".into()),
    };
    let declared = UnitLabels {
        document: Some("prd".into()),
        section: Some("acceptance-criteria".into()),
    };
    // Declared labels win over the printed values.
    let labelled = DeciderCheck {
        unit: Some(unit.clone()),
        unit_labels: Some(declared.clone()),
        ..criterion_check("ac_binary")
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderChecked(labelled));
    // No labels: the printed document and section, digested.
    let printed = DeciderCheck {
        unit: Some(unit.clone()),
        ..criterion_check("ac_binary")
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderChecked(printed));
    // Only a declared document, and a unit that printed no section.
    let partial = DeciderFallback {
        unit: Some(UnitRef {
            section: None,
            ..unit.clone()
        }),
        unit_labels: Some(UnitLabels {
            document: Some("prd".into()),
            section: None,
        }),
        ..fallback("ac_binary")
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderFallback(partial));
    let answered = DeciderCheckAnswered {
        state: "a".into(),
        visit_seq: 2,
        gate: "comments".into(),
        rule_id: "ac_binary".into(),
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        answer: AgentAnswer::Pass,
        fallback_seq: 5,
        unit: Some(unit.clone()),
        unit_labels: Some(declared),
    };
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderCheckAnswered(answered),
    );

    let d = decider_records(&f.journal());
    assert_eq!(
        kinds(&d),
        vec![
            "decider_checked",
            "decider_checked",
            "decider_fallback",
            "decider_check_answered"
        ]
    );
    let key = digest("SENTINEL-KEY-ac-3");
    assert!(key.starts_with("sha256:") && key.len() == 7 + 32, "{key}");
    assert!(is_id(&key), "{key}");
    for r in &d {
        assert_eq!(r["koto.unit.key"], key.as_str(), "{r}");
    }
    assert_eq!(d[0]["koto.unit.document"], "prd");
    assert_eq!(d[0]["koto.unit.section"], "acceptance-criteria");
    assert_eq!(d[0]["koto.decider.confidence_bp"], 5432);
    assert_eq!(d[1]["koto.unit.document"], digest("SENTINEL-DOC").as_str());
    assert_eq!(
        d[1]["koto.unit.section"],
        digest("SENTINEL-SECTION").as_str()
    );
    assert_eq!(d[2]["koto.unit.document"], "prd");
    assert!(d[2].get("koto.unit.section").is_none(), "{}", d[2]);
    assert_eq!(d[3]["koto.unit.document"], "prd");
    assert_eq!(d[3]["koto.unit.section"], "acceptance-criteria");
    assert_eq!(d[3]["koto.decider.answer"], "pass");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("SENTINEL"), "{raw}");
    assert_no_decider_content(&f);
}

/// A unit-mode fallback and answer carry the three unit fields; the same
/// events without a unit are written exactly as before units existed.
#[test]
fn records_of_a_check_without_units_are_unchanged() {
    let lines = |payload: &EventPayload| -> Vec<serde_json::Value> {
        let relay = Relay {
            session: "wf",
            session_id: "wf-id",
            run_id: Some("wf-id"),
            seq: 7,
            visit_seq: Some(2),
        };
        relayed(payload, &relay)
            .iter()
            .chain(paired(payload, &relay, "").iter())
            .map(|r| {
                // The time is the only field that moves between two writes.
                let mut v: serde_json::Value = serde_json::from_str(&r.to_line()).unwrap();
                v.as_object_mut().unwrap().remove("at");
                v
            })
            .collect()
    };
    let fb = fallback_event(UnansweredReason::OverBudget);
    let answered = check_answered(AgentAnswer::Fail, 3);
    for (payload, expected) in [
        (
            &fb,
            r#"{"kind":"decider_fallback","koto.check":"comments","koto.decider.input_bytes":34,"koto.decider.mode":"veto","koto.decider.reason":"over_budget","koto.rule.id":"comment_reason","koto.run.id":"wf-id","koto.seq":7,"koto.session.id":"wf-id","koto.state":"a","koto.visit.seq":2,"session":"wf","v":1}"#,
        ),
        (
            &answered,
            r#"{"kind":"decider_check_answered","koto.check":"comments","koto.decider.answer":"fail","koto.decider.fallback_seq":3,"koto.rule.id":"comment_reason","koto.run.id":"wf-id","koto.seq":7,"koto.session.id":"wf-id","koto.state":"a","koto.visit.seq":2,"session":"wf","v":1}"#,
        ),
    ] {
        let expected: serde_json::Value = serde_json::from_str(expected).unwrap();
        assert_eq!(lines(payload), vec![expected]);
    }
}

#[test]
fn a_rule_delivery_relays_rule_delivered_with_names_and_counts_only() {
    use crate::decider::check::RuleDelivered;
    use crate::template::decider_check::RuleLevel;
    let f = session_in_a();
    reset_for_test();
    let unit_mode = RuleDelivered {
        state: "a".into(),
        gate: "ac_binary".into(),
        rule_id: "ac_binary".into(),
        level: RuleLevel::Summary,
        unit_count: Some(20),
        via: None,
        delivery: None,
    };
    commit_as(&f, None, "wf", &EventPayload::RuleDelivered(unit_mode));
    let single = RuleDelivered {
        state: "a".into(),
        gate: "comments".into(),
        rule_id: "no reason [x]".into(),
        level: RuleLevel::Full,
        unit_count: None,
        via: None,
        delivery: None,
    };
    commit_as(&f, None, "wf", &EventPayload::RuleDelivered(single));
    let reported = RuleDelivered::from_check_output(
        "a",
        "pr_body",
        &crate::findings::PrintedRuleDelivery {
            rule_id: "pr-body/one-separator".into(),
            extent: RuleLevel::Full,
            delivery: crate::findings::RuleDeliveryOutcome::Refused,
        },
    );
    commit_as(&f, None, "wf", &EventPayload::RuleDelivered(reported));
    assert_eq!(
        batches_for_test(),
        (3, 0),
        "relayed, so never flushed alone"
    );

    let d = decider_records(&f.journal());
    assert_eq!(
        kinds(&d),
        vec!["rule_delivered", "rule_delivered", "rule_delivered"]
    );
    // A decider check's delivery carries no via or delivery.
    for r in &d[..2] {
        assert!(r.get("koto.rule.via").is_none(), "{r}");
        assert!(r.get("koto.rule.delivery").is_none(), "{r}");
    }
    let r = &d[2];
    assert_eq!(r["koto.check.name"], "pr_body");
    assert_eq!(r["koto.rule.id"], "pr-body/one-separator");
    assert_eq!(r["koto.rule.extent"], "full");
    assert_eq!(r["koto.rule.via"], "check_output");
    assert_eq!(r["koto.rule.delivery"], "refused");
    assert!(r.get("koto.rule.unit_count").is_none(), "{r}");
    let r = &d[0];
    assert_eq!(r["koto.seq"], 3);
    assert_eq!(r["koto.visit.seq"], 2);
    assert_eq!(r["koto.state"], "a");
    assert_eq!(r["koto.check.name"], "ac_binary");
    assert!(r.get("koto.check").is_none(), "{r}");
    assert_eq!(r["koto.rule.id"], "ac_binary");
    assert_eq!(r["koto.rule.extent"], "summary");
    assert_eq!(r["koto.rule.unit_count"], 20);
    let r = &d[1];
    assert_eq!(r["koto.seq"], 4);
    assert_eq!(r["koto.rule.extent"], "full");
    assert_eq!(r["koto.rule.id"], name_or_digest("no reason [x]").as_str());
    assert!(r.get("koto.rule.unit_count").is_none(), "{r}");
}

#[test]
fn unit_and_rule_records_with_the_longest_names_fit_a_line() {
    use crate::decider::check::{RuleDelivered, UnitLabels, UnitRef};
    use crate::template::decider_check::RuleLevel;
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    // Three 128-byte names (the state, the check and a declared label) and
    // a 128-byte rule id, on the widest decider-check record.
    let checked = DeciderCheck {
        state: "t".repeat(128),
        gate: "g".repeat(128),
        provider: "p".repeat(128),
        model: "m".repeat(128),
        input_bytes: Some(u64::MAX),
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        latency_ms: u64::MAX,
        unit: Some(UnitRef {
            key: "k".repeat(128),
            document: Some("d".repeat(128)),
            section: Some("x".repeat(128)),
        }),
        unit_labels: Some(UnitLabels {
            document: Some("l".repeat(128)),
            section: None,
        }),
        ..criterion_check(&"r".repeat(128))
    };
    let delivered = RuleDelivered {
        state: "t".repeat(128),
        gate: "g".repeat(128),
        rule_id: "r".repeat(128),
        level: RuleLevel::Summary,
        unit_count: Some(32),
        via: Some(crate::decider::check::RuleVia::CheckOutput),
        delivery: Some(crate::findings::RuleDeliveryOutcome::Refused),
    };
    for payload in [
        EventPayload::DeciderChecked(checked),
        EventPayload::RuleDelivered(delivered),
    ] {
        let lines: Vec<String> = relayed(&payload, &relay)
            .iter()
            .map(Record::to_line)
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].len() < MAX_LINE_BYTES, "{} bytes", lines[0].len());
        let r: serde_json::Value = serde_json::from_str(lines[0].trim_end()).unwrap();
        assert_eq!(r["koto.rule.id"], "r".repeat(128));
        assert_eq!(r["koto.state"], "t".repeat(128));
    }
}

#[test]
fn an_override_relays_check_overridden_with_its_state_and_gate_only() {
    let f = session_in_a();
    reset_for_test();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::GateOverrideRecorded {
            state: "a".into(),
            gate: "lint".into(),
            rationale: "PLANTED-RATIONALE".into(),
            override_applied: serde_json::json!({"exit_code": 0, "note": "PLANTED-VALUE"}),
            actual_output: serde_json::json!({"exit_code": 1, "error": "PLANTED-OUTPUT"}),
            timestamp: crate::engine::types::now_iso8601(),
        },
    );
    assert_eq!(batches_for_test(), (1, 0));
    let d = decider_records(&f.journal());
    assert_eq!(kinds(&d), vec!["check_overridden"]);
    let keys: BTreeSet<&str> = d[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = RELAY_FIELDS
        .iter()
        .copied()
        .chain(["koto.state", "koto.check"])
        .collect();
    assert_eq!(keys, expected);
    assert_eq!(d[0]["koto.seq"], 3);
    assert_eq!(d[0]["koto.visit.seq"], 2);
    assert_eq!(d[0]["koto.state"], "a");
    assert_eq!(d[0]["koto.check"], "lint");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for absent in ["PLANTED", "exit_code", "rationale"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
}

/// With the longest names koto keeps, a 255-byte session name, 128-byte
/// provider, model, state, gate and criterion names, and the largest
/// numbers, every decider and override record fits the line cap, and a
/// 129-byte criterion id is digested.
#[test]
fn decider_and_override_records_fit_the_line_cap_at_the_longest_names() {
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let name = "n".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let consulted = DeciderConsultation {
        state: name.clone(),
        provider: name.clone(),
        model: name.clone(),
        error_class: Some(ErrorClass::HttpStatus),
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        unread_usage_attempts: u32::MAX,
        latency_ms: u64::MAX,
        directive_bytes: u64::MAX,
        outcome: ConsultationOutcome::InputUnavailable,
        ..consultation()
    };
    let checked = |rule_id: &str| DeciderCheck {
        state: name.clone(),
        gate: name.clone(),
        provider: name.clone(),
        model: name.clone(),
        outcome: RecordedOutcome::Unanswered,
        reason: Some(UnansweredReason::UnreadableResponse),
        error_class: Some(ErrorClass::HttpStatus),
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        unread_usage_attempts: u32::MAX,
        attempts: u32::MAX,
        latency_ms: u64::MAX,
        ..criterion_check(rule_id)
    };
    let longest = "r".repeat(128);
    let over = "r".repeat(129);
    let overridden = EventPayload::GateOverrideRecorded {
        state: name.clone(),
        gate: name.clone(),
        rationale: "x".repeat(8192),
        override_applied: serde_json::json!({"v": "x".repeat(8192)}),
        actual_output: serde_json::json!({"v": "x".repeat(8192)}),
        timestamp: crate::engine::types::now_iso8601(),
    };
    let mut records = Vec::new();
    for payload in [
        EventPayload::DeciderConsulted(consulted),
        EventPayload::DeciderChecked(checked(&longest)),
        EventPayload::DeciderChecked(checked(&over)),
        overridden,
    ] {
        records.extend(relayed(&payload, &relay));
    }
    assert_eq!(records.len(), 4);
    let lines: Vec<String> = records.iter().map(Record::to_line).collect();
    for line in &lines {
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes: {line}", line.len());
    }
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let parsed = decider_records(&parsed);
    assert_eq!(parsed[0]["koto.decider.model"], name.as_str());
    assert_eq!(parsed[1]["koto.rule.id"], longest.as_str());
    assert_eq!(parsed[2]["koto.rule.id"], name_or_digest(&over).as_str());
    assert!(parsed[2]["koto.rule.id"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(parsed[3]["koto.check"], name.as_str());
}

// ----- Decider answers -----

use crate::decider::check::{AgentAnswer, DeciderCheckAnswered, DeciderFallback};
use crate::decider::{FieldOutcome, UnavailableReason};
use crate::template::decider::DeciderMode;

/// A relay for records built straight from a payload, at `seq` 7 in visit 2.
fn answer_relay() -> Relay<'static> {
    Relay {
        session: "wf",
        session_id: "wf-id",
        run_id: Some("wf-id"),
        seq: 7,
        visit_seq: Some(2),
    }
}

/// A field declaring `modes`, answered `winning` with `outcome`.
fn answered_field(
    modes: &[(&str, DeciderMode)],
    winning: Option<&str>,
    outcome: Option<FieldOutcome>,
    at_threshold: bool,
) -> FieldConsultation {
    FieldConsultation {
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        modes: modes.iter().map(|(v, m)| (v.to_string(), *m)).collect(),
        probabilities: Default::default(),
        winning: winning.map(str::to_string),
        confidence: Some(0.4321),
        threshold: Some(0.875),
        at_threshold,
        outcome,
    }
}

/// [`consultation`] of state `a` in koto's visit 2 with `fields`.
fn consultation_of(fields: Vec<(&str, FieldConsultation)>) -> DeciderConsultation {
    DeciderConsultation {
        visit_seq: 2,
        fields: fields
            .into_iter()
            .map(|(name, f)| (name.to_string(), f))
            .collect(),
        ..consultation()
    }
}

/// The records `payload` relays, parsed and checked by [`decider_records`].
fn relayed_values(payload: &EventPayload) -> Vec<serde_json::Value> {
    let lines: Vec<serde_json::Value> = relayed(payload, &answer_relay())
        .iter()
        .map(|r| serde_json::from_str(&r.to_line()).unwrap())
        .collect();
    decider_records(&lines)
}

const SHADOW: DeciderMode = DeciderMode::Shadow;
const AUTO: DeciderMode = DeciderMode::Auto;
const NEVER: DeciderMode = DeciderMode::Never;

#[test]
fn a_consultation_of_one_field_carries_its_answer_mode_and_threshold() {
    let both_shadow = [("proceed", SHADOW), ("exit", SHADOW)];
    let mixed = [("proceed", SHADOW), ("exit", AUTO)];
    // (field, answer, mode, threshold_met)
    let cases: Vec<(FieldConsultation, &str, Option<&str>, Option<bool>)> = vec![
        (
            answered_field(
                &both_shadow,
                Some("proceed"),
                Some(FieldOutcome::Shadow),
                true,
            ),
            "proceed",
            Some("shadow"),
            Some(true),
        ),
        (
            answered_field(&mixed, Some("exit"), Some(FieldOutcome::Qualified), true),
            "exit",
            Some("auto"),
            Some(true),
        ),
        (
            answered_field(
                &[("proceed", SHADOW), ("exit", NEVER)],
                Some("exit"),
                Some(FieldOutcome::Never),
                true,
            ),
            "exit",
            Some("never"),
            Some(true),
        ),
        // Below its threshold: koto's word, and the near-winner's mode.
        (
            answered_field(
                &mixed,
                Some("exit"),
                Some(FieldOutcome::BelowThreshold),
                false,
            ),
            "below_threshold",
            Some("auto"),
            Some(false),
        ),
        // The escape isn't a declared value; the modes differ, so none, and
        // no declared value won, so no threshold either.
        (
            answered_field(&mixed, Some("unclear"), Some(FieldOutcome::Escape), false),
            "escape",
            None,
            None,
        ),
        // No answer: every declared value shares one mode, so that one.
        (
            answered_field(&both_shadow, None, None, false),
            "no_answer",
            Some("shadow"),
            None,
        ),
    ];
    for (field, answer, mode, met) in cases {
        let d = relayed_values(&EventPayload::DeciderConsulted(consultation_of(vec![(
            "verdict", field,
        )])));
        assert_eq!(kinds(&d), vec!["decider_consulted"]);
        let r = &d[0];
        assert_eq!(r["koto.decider.field_count"], 1, "{r}");
        assert_eq!(r["koto.decider.field"], "verdict", "{r}");
        assert_eq!(r["koto.decider.answer"], answer, "{r}");
        assert_eq!(r["koto.decider.mode"].as_str(), mode, "{r}");
        assert_eq!(r["koto.decider.threshold_met"].as_bool(), met, "{r}");
        let raw = r.to_string();
        for absent in ["PLANTED", "0.4321", "0.875"] {
            assert!(!raw.contains(absent), "{absent} in {raw}");
        }
    }
}

/// The exported answer is the report's own column for the field, including
/// the cases a compiled template can't produce, so the export and
/// `koto decider report` never read one consultation two ways.
#[test]
fn the_answer_is_the_reports_column_for_every_field() {
    let modes = [("proceed", SHADOW), ("exit", SHADOW)];
    let cases = [
        (
            ConsultationOutcome::NotApplied,
            answered_field(&modes, Some("proceed"), Some(FieldOutcome::Shadow), true),
            "proceed",
        ),
        // A winner that isn't declared, under a qualifying outcome.
        (
            ConsultationOutcome::NotApplied,
            answered_field(&modes, Some("maybe"), Some(FieldOutcome::Shadow), true),
            "escape",
        ),
        // A qualifying outcome whose winner missed its threshold.
        (
            ConsultationOutcome::NotApplied,
            answered_field(&modes, Some("exit"), Some(FieldOutcome::Shadow), false),
            "below_threshold",
        ),
        // A field outcome on a consultation that got no answer.
        (
            ConsultationOutcome::Error,
            answered_field(&modes, Some("exit"), Some(FieldOutcome::Shadow), true),
            "no_answer",
        ),
    ];
    for (outcome, field, answer) in cases {
        let column = crate::decider::report::ledger_column(outcome, &field);
        assert_eq!(column, answer);
        let d = relayed_values(&EventPayload::DeciderConsulted(DeciderConsultation {
            outcome,
            ..consultation_of(vec![("verdict", field)])
        }));
        assert_eq!(d[0]["koto.decider.answer"], answer, "{}", d[0]);
        assert!(!d[0].to_string().contains("maybe"), "{}", d[0]);
    }
}

#[test]
fn a_consultation_of_several_fields_writes_one_decider_field_per_field() {
    let d = relayed_values(&EventPayload::DeciderConsulted(consultation_of(vec![
        (
            "verdict",
            answered_field(
                &[("proceed", SHADOW), ("exit", SHADOW)],
                Some("proceed"),
                Some(FieldOutcome::Shadow),
                true,
            ),
        ),
        (
            "size",
            answered_field(
                &[("small", SHADOW), ("large", SHADOW)],
                Some("large"),
                Some(FieldOutcome::BelowThreshold),
                false,
            ),
        ),
    ])));
    assert_eq!(
        kinds(&d),
        vec!["decider_consulted", "decider_field", "decider_field"]
    );
    let c = &d[0];
    assert_eq!(c["koto.decider.field_count"], 2);
    for absent in [
        "koto.decider.field",
        "koto.decider.answer",
        "koto.decider.mode",
        "koto.decider.threshold_met",
    ] {
        assert!(c.get(absent).is_none(), "{absent}: {c}");
    }
    // The tokens and timings stay on the consultation alone.
    assert_eq!(c["koto.decider.input_tokens"], 812);
    for f in &d[1..] {
        assert_eq!(f["koto.decider.consulted_seq"], c["koto.seq"], "{f}");
        assert_eq!(f["koto.seq"], c["koto.seq"], "{f}");
        assert_eq!(f["koto.visit.seq"], c["koto.visit.seq"], "{f}");
        assert_eq!(f["koto.state"], "a");
        assert!(f.get("koto.decider.input_tokens").is_none(), "{f}");
    }
    // In field-name order.
    assert_eq!(d[1]["koto.decider.field"], "size");
    assert_eq!(d[1]["koto.decider.answer"], "below_threshold");
    assert_eq!(d[1]["koto.decider.threshold_met"], false);
    assert_eq!(d[2]["koto.decider.field"], "verdict");
    assert_eq!(d[2]["koto.decider.answer"], "proceed");
    assert_eq!(d[2]["koto.decider.mode"], "shadow");
}

#[test]
fn an_unavailable_consultation_carries_its_reason_and_the_inputs_label_and_size() {
    let unavailable = |reason, label: &str, bytes| DeciderConsultation {
        outcome: ConsultationOutcome::InputUnavailable,
        input_tokens: None,
        output_tokens: None,
        reason: Some(reason),
        input_label: Some(label.to_string()),
        input_bytes: bytes,
        ..consultation()
    };
    for (c, reason, label, bytes) in [
        (
            unavailable(UnavailableReason::Unset, "issue", None),
            "unset",
            "issue".to_string(),
            None,
        ),
        (
            unavailable(UnavailableReason::OverBudget, "diff", Some(18_000)),
            "over_budget",
            "diff".to_string(),
            Some(18_000),
        ),
        (
            unavailable(UnavailableReason::NotText, "notes", Some(3)),
            "not_text",
            "notes".to_string(),
            Some(3),
        ),
        // A declared label outside the name shape is digested.
        (
            unavailable(UnavailableReason::Unset, "the issue body", None),
            "unset",
            name_or_digest("the issue body"),
            None,
        ),
    ] {
        let d = relayed_values(&EventPayload::DeciderConsulted(c));
        let r = &d[0];
        assert_eq!(r["koto.decider.outcome"], "input_unavailable");
        assert_eq!(r["koto.decider.reason"], reason, "{r}");
        assert_eq!(r["koto.decider.input_label"], label.as_str(), "{r}");
        assert_eq!(r["koto.decider.input_bytes"].as_u64(), bytes, "{r}");
        assert_eq!(r["koto.decider.answer"], "no_answer");
        assert!(!r.to_string().contains("the issue body"), "{r}");
    }
}

/// Evidence for state `a` from the agent.
fn agent_evidence(fields: &[(&str, serde_json::Value)]) -> EventPayload {
    EventPayload::EvidenceSubmitted {
        state: "a".into(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        submitter_cwd: Some("/PLANTED/host/path".into()),
        source: None,
        exported_fields: None,
    }
}

/// A one-field consultation of `verdict` (`proceed` or `exit`) in koto's
/// visit 2 that ended `outcome`.
fn verdict_consultation(outcome: ConsultationOutcome) -> EventPayload {
    EventPayload::DeciderConsulted(DeciderConsultation {
        outcome,
        ..consultation_of(vec![(
            "verdict",
            answered_field(
                &[("proceed", SHADOW), ("exit", SHADOW)],
                Some("proceed"),
                Some(FieldOutcome::Shadow),
                true,
            ),
        )])
    })
}

/// The journal finds a visit's consultation in the raw log by the rule the
/// engine reads the parsed log by (`prior_consultation`, which the ledger's
/// `answered` record pairs with), on every shape of log that rule turns on.
#[test]
fn the_journal_pairs_with_the_consultation_the_engine_finds() {
    let consulted = |visit_seq: u64, model: &str| {
        EventPayload::DeciderConsulted(DeciderConsultation {
            visit_seq,
            model: model.into(),
            ..consultation()
        })
    };
    let other_state = EventPayload::DeciderConsulted(DeciderConsultation {
        state: "b".into(),
        visit_seq: 2,
        ..consultation()
    });
    let logs: Vec<(&str, Vec<EventPayload>)> = vec![
        ("one consultation", vec![consulted(2, "m1")]),
        (
            "two in one visit: the first",
            vec![consulted(2, "m1"), consulted(2, "m2")],
        ),
        (
            "one naming another visit, then this one",
            vec![consulted(1, "stale"), consulted(2, "m1")],
        ),
        ("another state's only", vec![other_state.clone()]),
        (
            "a self-loop after it",
            vec![consulted(2, "m1"), transitioned(Some("a"), "a")],
        ),
        (
            "a self-loop, then the new lap's",
            vec![
                consulted(2, "m1"),
                transitioned(Some("a"), "a"),
                consulted(4, "m2"),
            ],
        ),
        (
            "left for another state",
            vec![consulted(2, "m1"), transitioned(Some("a"), "b")],
        ),
        ("no consultation", vec![]),
    ];
    for (name, payloads) in logs {
        let mut events = vec![crate::engine::types::Event {
            seq: 1,
            timestamp: crate::engine::types::now_iso8601(),
            event_type: "workflow_initialized".into(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "t.json".into(),
                variables: Default::default(),
                spawn_entry: None,
                machine: None,
            },
            idempotency_hash: None,
        }];
        for (i, payload) in std::iter::once(transitioned(None, "a"))
            .chain(payloads)
            .enumerate()
        {
            events.push(crate::engine::types::Event {
                seq: i as u64 + 2,
                timestamp: crate::engine::types::now_iso8601(),
                event_type: payload.type_name().to_string(),
                payload,
                idempotency_hash: None,
            });
        }
        let log: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        let engine = crate::engine::decider::prior_consultation(&events, "a");
        let journal = super::pairing::consultation(&log, "a");
        assert_eq!(
            journal.as_ref().map(|(_, c)| c),
            engine,
            "{name}: the journal and the engine disagree"
        );
        if let Some((seq, c)) = &journal {
            let at = events
                .iter()
                .find(|e| matches!(&e.payload, EventPayload::DeciderConsulted(x) if x == c))
                .unwrap();
            assert_eq!(*seq, at.seq, "{name}");
        }
    }
}

#[test]
fn an_agent_answer_to_a_consulted_visit_writes_decider_answered() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &verdict_consultation(ConsultationOutcome::NotApplied),
    );
    commit_as(
        &f,
        None,
        "wf",
        &agent_evidence(&[
            ("verdict", serde_json::json!("exit")),
            ("notes", serde_json::json!("PLANTED free text")),
        ]),
    );
    let d = decider_records(&f.journal());
    assert_eq!(kinds(&d), vec!["decider_consulted", "decider_answered"]);
    let (c, a) = (&d[0], &d[1]);
    assert_eq!(c["koto.seq"], 3);
    assert_eq!(a["koto.seq"], 4);
    assert_eq!(a["koto.visit.seq"], c["koto.visit.seq"]);
    assert_eq!(a["koto.decider.consulted_seq"], c["koto.seq"]);
    assert_eq!(a["koto.state"], "a");
    assert_eq!(a["koto.decider.field"], "verdict");
    assert_eq!(a["koto.decider.agent_answer"], "exit");
    assert_eq!(c["koto.decider.answer"], "proceed");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for absent in ["PLANTED", "notes", "host/path"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
}

#[test]
fn an_agent_value_outside_the_declared_values_is_written_other() {
    let flag = answered_field(
        &[("true", SHADOW), ("false", SHADOW)],
        Some("true"),
        Some(FieldOutcome::Shadow),
        true,
    );
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderConsulted(consultation_of(vec![
            (
                "verdict",
                answered_field(&[("proceed", SHADOW), ("exit", SHADOW)], None, None, false),
            ),
            ("ready", flag.clone()),
            ("count", flag),
        ])),
    );
    commit_as(
        &f,
        None,
        "wf",
        &agent_evidence(&[
            ("verdict", serde_json::json!("PLANTED free text")),
            ("ready", serde_json::json!(false)),
            ("count", serde_json::json!(3)),
        ]),
    );
    let answers: Vec<(String, String)> = decider_records(&f.journal())
        .iter()
        .filter(|r| r["kind"] == "decider_answered")
        .map(|r| {
            (
                r["koto.decider.field"].as_str().unwrap().to_string(),
                r["koto.decider.agent_answer"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        answers,
        vec![
            ("count".to_string(), "other".to_string()),
            ("ready".to_string(), "false".to_string()),
            ("verdict".to_string(), "other".to_string()),
        ]
    );
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

#[test]
fn evidence_with_no_unapplied_consultation_of_its_visit_writes_no_decider_answered() {
    let answer = || agent_evidence(&[("verdict", serde_json::json!("exit"))]);
    let answered = |f: &Fixture| {
        f.journal()
            .iter()
            .filter(|r| r["kind"] == "decider_answered")
            .count()
    };

    // No consultation at all.
    let f = session_in_a();
    commit_as(&f, None, "wf", &answer());
    assert_eq!(answered(&f), 0);

    // An applied consultation, and the decider's own evidence.
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &verdict_consultation(ConsultationOutcome::Applied),
    );
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::EvidenceSubmitted {
            state: "a".into(),
            fields: [("verdict".to_string(), serde_json::json!("proceed"))]
                .into_iter()
                .collect(),
            submitter_cwd: None,
            source: Some(crate::engine::decider::DECIDER_SOURCE.to_string()),
            exported_fields: None,
        },
    );
    commit_as(&f, None, "wf", &answer());
    assert_eq!(answered(&f), 0);

    // A consultation of an earlier lap: a self-loop began a new decider
    // visit, which nothing consulted.
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &verdict_consultation(ConsultationOutcome::NotApplied),
    );
    commit_as(&f, None, "wf", &transitioned(Some("a"), "a"));
    commit_as(&f, None, "wf", &answer());
    assert_eq!(answered(&f), 0);

    // Evidence that holds no consulted field.
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &verdict_consultation(ConsultationOutcome::NotApplied),
    );
    commit_as(
        &f,
        None,
        "wf",
        &agent_evidence(&[("other_field", serde_json::json!("exit"))]),
    );
    assert_eq!(answered(&f), 0);
}

/// A fallback of criterion `comment_reason` on gate `comments`.
fn fallback_event(reason: UnansweredReason) -> EventPayload {
    EventPayload::DeciderFallback(DeciderFallback {
        state: "a".into(),
        visit_seq: 2,
        gate: "comments".into(),
        rule_id: "comment_reason".into(),
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        mode: CheckMode::Veto,
        reason,
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        input_bytes: Some(34),
        unit: None,
        unit_labels: None,
    })
}

/// The agent's `answer` to the fallback at `fallback_seq`.
fn check_answered(answer: AgentAnswer, fallback_seq: u64) -> EventPayload {
    EventPayload::DeciderCheckAnswered(DeciderCheckAnswered {
        state: "a".into(),
        visit_seq: 2,
        gate: "comments".into(),
        rule_id: "comment_reason".into(),
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        answer,
        fallback_seq,
        unit: None,
        unit_labels: None,
    })
}

#[test]
fn an_answered_fallback_relays_decider_check_answered_with_the_fallbacks_reason() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &fallback_event(UnansweredReason::OverBudget),
    );
    commit_as(&f, None, "wf", &check_answered(AgentAnswer::Fail, 3));
    // An answer whose fallback the log doesn't hold carries no reason.
    commit_as(&f, None, "wf", &check_answered(AgentAnswer::Unclear, 99));
    let d = decider_records(&f.journal());
    assert_eq!(
        kinds(&d),
        vec![
            "decider_fallback",
            "decider_check_answered",
            "decider_check_answered"
        ]
    );
    let a = &d[1];
    assert_eq!(a["koto.seq"], 4);
    assert_eq!(a["koto.visit.seq"], 2);
    assert_eq!(a["koto.state"], "a");
    assert_eq!(a["koto.check"], "comments");
    assert_eq!(a["koto.rule.id"], "comment_reason");
    assert_eq!(a["koto.decider.answer"], "fail");
    assert_eq!(a["koto.decider.reason"], "over_budget");
    assert_eq!(a["koto.decider.fallback_seq"], d[0]["koto.seq"]);
    let b = &d[2];
    assert_eq!(b["koto.decider.answer"], "unclear");
    assert_eq!(b["koto.decider.fallback_seq"], 99);
    assert!(b.get("koto.decider.reason").is_none(), "{b}");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

/// Every value the answer and reason keys carry is one of the template's
/// declared names (a field, a declared value, an input label, or the
/// digest of one outside the name shape), one of koto's own fixed words, a
/// boolean, or a number. The test feeds them free text wherever the
/// session could put it and checks none of it comes out.
#[test]
fn every_decider_answer_value_is_declared_fixed_a_boolean_or_a_number() {
    let declared: BTreeSet<String> = [
        "verdict",
        "proceed",
        "exit",
        "ready",
        "true",
        "false",
        "diff",
        "comment_reason",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([name_or_digest("odd value"), name_or_digest("odd label")])
    .collect();
    let fixed: BTreeSet<&str> = [
        "no_answer",
        "other",
        "below_threshold",
        "escape",
        "unset",
        "over_budget",
        "not_text",
        "pass",
        "fail",
        "unclear",
        "off",
        "shadow",
        "auto",
        "never",
    ]
    .into_iter()
    .chain(
        [
            UnansweredReason::ProviderError,
            UnansweredReason::UnreadableResponse,
            UnansweredReason::OverBudget,
            UnansweredReason::ExtractionFailed,
            UnansweredReason::CapSpent,
            UnansweredReason::Busy,
        ]
        .map(|r| r.as_str()),
    )
    .collect();

    let f = session_in_a();
    // A consultation answered with a declared value outside the name shape,
    // one that stopped at an odd label, and the agent's free text.
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderConsulted(consultation_of(vec![
            (
                "verdict",
                answered_field(
                    &[("proceed", SHADOW), ("odd value", AUTO)],
                    Some("odd value"),
                    Some(FieldOutcome::Qualified),
                    true,
                ),
            ),
            (
                "ready",
                answered_field(&[("true", SHADOW), ("false", SHADOW)], None, None, false),
            ),
        ])),
    );
    commit_as(
        &f,
        None,
        "wf",
        &agent_evidence(&[
            ("verdict", serde_json::json!("PLANTED free text")),
            ("ready", serde_json::json!(true)),
        ]),
    );
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderConsulted(DeciderConsultation {
            outcome: ConsultationOutcome::InputUnavailable,
            reason: Some(UnavailableReason::OverBudget),
            input_label: Some("odd label".into()),
            input_bytes: Some(18_000),
            ..consultation_of(vec![(
                "verdict",
                answered_field(&[("proceed", SHADOW), ("exit", NEVER)], None, None, false),
            )])
        }),
    );
    commit_as(&f, None, "wf", &fallback_event(UnansweredReason::Busy));
    commit_as(&f, None, "wf", &check_answered(AgentAnswer::Pass, 6));

    // The kinds this checks are the ones that carry answers; the
    // `decider_fallback` between them has its own test.
    let records: Vec<serde_json::Value> = decider_records(&f.journal())
        .into_iter()
        .filter(|r| r["kind"] != "decider_fallback")
        .collect();
    assert_eq!(
        kinds(&records),
        vec![
            "decider_consulted",
            "decider_field",
            "decider_field",
            "decider_answered",
            "decider_answered",
            "decider_consulted",
            "decider_check_answered"
        ]
    );
    let mut seen = 0;
    for r in &records {
        for (key, value) in r.as_object().unwrap() {
            if !key.starts_with("koto.decider.") {
                continue;
            }
            let suffix = &key["koto.decider.".len()..];
            match suffix {
                "provider" | "model" | "outcome" | "error_class" => continue,
                "field" | "answer" | "agent_answer" | "mode" | "reason" | "input_label" => {
                    let v = value.as_str().unwrap_or_else(|| panic!("{key}: {r}"));
                    assert!(
                        declared.contains(v) || fixed.contains(v),
                        "{key} = {v} is neither declared nor fixed: {r}"
                    );
                    seen += 1;
                }
                _ => assert!(
                    value.is_boolean() || value.is_u64(),
                    "{key} = {value} is not a boolean or a number: {r}"
                ),
            }
        }
    }
    assert!(seen >= 12, "{seen} values checked: {records:?}");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for absent in ["PLANTED", "odd value", "odd label", "host/path"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
}

/// With 128-byte names everywhere a declared name goes, the largest
/// numbers, and a 255-byte session name, the new decider records fit the
/// line cap.
#[test]
fn decider_answer_records_fit_the_line_cap_at_the_longest_names() {
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let name = "n".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let field = |value: &str| {
        answered_field(
            &[(value, SHADOW), ("x", AUTO)],
            Some(value),
            Some(FieldOutcome::Shadow),
            true,
        )
    };
    let wide = DeciderConsultation {
        state: name.clone(),
        provider: name.clone(),
        model: name.clone(),
        error_class: Some(ErrorClass::HttpStatus),
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        unread_usage_attempts: u32::MAX,
        latency_ms: u64::MAX,
        directive_bytes: u64::MAX,
        outcome: ConsultationOutcome::InputUnavailable,
        reason: Some(UnavailableReason::OverBudget),
        input_label: Some(name.clone()),
        input_bytes: Some(u64::MAX),
        ..consultation_of(vec![(name.as_str(), field(&name))])
    };
    let several = consultation_of(vec![
        (name.as_str(), field(&name)),
        ("m".repeat(128).as_str(), field(&name)),
    ]);
    let mut records = relayed(&EventPayload::DeciderConsulted(wide), &relay);
    records.extend(relayed(&EventPayload::DeciderConsulted(several), &relay));
    let log = |payload: &EventPayload, seq: u64| {
        serde_json::to_string(&crate::engine::types::Event {
            seq,
            timestamp: crate::engine::types::now_iso8601(),
            event_type: payload.type_name().to_string(),
            payload: payload.clone(),
            idempotency_hash: None,
        })
        .unwrap()
    };
    let entry = log(&transitioned(None, &name), 1);
    let consulted = log(
        &EventPayload::DeciderConsulted(DeciderConsultation {
            state: name.clone(),
            visit_seq: 1,
            ..consultation_of(vec![(name.as_str(), field(&name))])
        }),
        2,
    );
    let evidence = EventPayload::EvidenceSubmitted {
        state: name.clone(),
        fields: [(name.clone(), serde_json::json!(name.clone()))]
            .into_iter()
            .collect(),
        submitter_cwd: None,
        source: None,
        exported_fields: None,
    };
    records.extend(paired(
        &evidence,
        &relay,
        &format!("{entry}\n{consulted}\n"),
    ));
    let fallback = log(
        &EventPayload::DeciderFallback(DeciderFallback {
            state: name.clone(),
            gate: name.clone(),
            rule_id: name.clone(),
            reason: UnansweredReason::UnreadableResponse,
            ..match fallback_event(UnansweredReason::Busy) {
                EventPayload::DeciderFallback(f) => f,
                _ => unreachable!(),
            }
        }),
        u64::MAX - 1,
    );
    let answer = EventPayload::DeciderCheckAnswered(DeciderCheckAnswered {
        state: name.clone(),
        gate: name.clone(),
        rule_id: name.clone(),
        ..match check_answered(AgentAnswer::Unclear, u64::MAX - 1) {
            EventPayload::DeciderCheckAnswered(a) => a,
            _ => unreachable!(),
        }
    });
    records.extend(paired(&answer, &relay, &format!("{fallback}\n")));

    let lines: Vec<String> = records.iter().map(Record::to_line).collect();
    for line in &lines {
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes: {line}", line.len());
    }
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let parsed = decider_records(&parsed);
    assert_eq!(
        kinds(&parsed),
        vec![
            "decider_consulted",
            "decider_consulted",
            "decider_field",
            "decider_field",
            "decider_answered",
            "decider_check_answered"
        ]
    );
    assert_eq!(parsed[0]["koto.decider.input_label"], name.as_str());
    assert_eq!(parsed[0]["koto.decider.field"], name.as_str());
    assert_eq!(parsed[4]["koto.decider.agent_answer"], name.as_str());
    assert_eq!(parsed[4]["koto.decider.consulted_seq"], 2);
    assert_eq!(parsed[5]["koto.decider.reason"], "unreadable_response");
}

// ----- Context reads -----

/// The fields a `context_reads` record may carry beyond [`RELAY_FIELDS`].
const CONTEXT_READS_FIELDS: &[&str] = &[
    "koto.state",
    "koto.context.key",
    "koto.context.reader",
    "koto.context.access",
    "koto.context.overflow",
    "koto.context.read_count",
    "koto.context.present_count",
    "koto.context.first_hash",
    "koto.context.last_hash",
];

/// The `context_reads` records in `j`, each checked to carry `v` 1, both
/// sequence numbers, only the kind's fields, and each field exactly when
/// the reference says: the tuple's names unless it is the overflow record,
/// the hashes only when a read found the key.
fn context_records(j: &[serde_json::Value]) -> Vec<serde_json::Value> {
    j.iter()
        .filter(|r| r["kind"] == "context_reads")
        .map(|r| {
            let allowed: BTreeSet<&str> = RELAY_FIELDS
                .iter()
                .chain(CONTEXT_READS_FIELDS)
                .copied()
                .collect();
            for (key, value) in r.as_object().unwrap() {
                assert!(
                    allowed.contains(key.as_str()),
                    "context_reads carries {key}: {r}"
                );
                assert!(!value.is_null() && value != "", "{key}: {r}");
            }
            assert_eq!(r["v"], 1);
            assert!(r["koto.seq"].is_u64(), "{r}");
            assert!(r["koto.visit.seq"].is_u64(), "{r}");
            assert!(r["koto.state"].is_string(), "{r}");
            assert!(r["koto.context.read_count"].is_u64(), "{r}");
            assert!(r["koto.context.present_count"].is_u64(), "{r}");
            let overflow = r.get("koto.context.overflow").is_some();
            if overflow {
                assert_eq!(r["koto.context.overflow"], true, "{r}");
            }
            for named in [
                "koto.context.key",
                "koto.context.reader",
                "koto.context.access",
            ] {
                assert_eq!(r.get(named).is_some(), !overflow, "{named}: {r}");
            }
            let found = r["koto.context.present_count"].as_u64().unwrap() > 0 && !overflow;
            for hash in ["koto.context.first_hash", "koto.context.last_hash"] {
                assert_eq!(r.get(hash).is_some(), found, "{hash}: {r}");
            }
            r.clone()
        })
        .collect()
}

/// A logged read of `key` by `reader` in `state`: present with `content`'s
/// hash, or absent.
fn read_of(
    state: &str,
    key: &str,
    reader: &str,
    access: &str,
    content: Option<&str>,
) -> EventPayload {
    EventPayload::ContextRead {
        key: key.into(),
        reader: reader.into(),
        state: state.into(),
        present: content.is_some(),
        hash: content.map(|c| crate::cache::sha256_hex(c.as_bytes())),
        access: Some(access.into()),
        gate: (reader == "gate").then(|| "g".to_string()),
        session: None,
    }
}

fn gate_read(state: &str, key: &str, content: Option<&str>) -> EventPayload {
    read_of(state, key, "gate", "content", content)
}

fn cli_read(state: &str, key: &str, content: Option<&str>) -> EventPayload {
    read_of(state, key, "cli", "content", content)
}

fn hash_of(content: &str) -> String {
    crate::cache::sha256_hex(content.as_bytes())
}

fn rewound(from: &str, to: &str) -> EventPayload {
    EventPayload::Rewound {
        from: from.into(),
        to: to.into(),
        rationale: None,
    }
}

fn cancelled(state: &str) -> EventPayload {
    EventPayload::WorkflowCancelled {
        state: state.into(),
        reason: "PLANTED-REASON".into(),
    }
}

fn store_result() -> EventPayload {
    EventPayload::RequestStoreResult {
        result: crate::engine::types::WorkflowResult::with_string_payload(
            crate::engine::types::TerminalOutcome::Success,
            "PLANTED-SUMMARY",
            [("pr", "PLANTED-RESULT-VALUE")],
        ),
        failure_reason: None,
    }
}

/// The session's last event's `seq`, read from its log.
fn last_seq(f: &Fixture, session: &str) -> u64 {
    f.backend
        .read_events(session)
        .unwrap()
        .1
        .last()
        .unwrap()
        .seq
}

/// (key, reader, access, read count, present count) of each record.
fn tuples(c: &[serde_json::Value]) -> Vec<(String, String, String, u64, u64)> {
    c.iter()
        .map(|r| {
            let s = |k: &str| r[k].as_str().unwrap_or("-").to_string();
            (
                s("koto.context.key"),
                s("koto.context.reader"),
                s("koto.context.access"),
                r["koto.context.read_count"].as_u64().unwrap(),
                r["koto.context.present_count"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// Within one visit: two keys a gate reads and one `koto context get`
/// reads give three records, in first-read order; a key read once absent
/// and twice present counts 3 and 2, with the second and third reads'
/// hashes. They are written at the arrival that ends the visit, before its
/// `state_entered`, in the same flushed batch, carrying the arrival's seq
/// (the highest in the log when they're written) and the closing visit's.
#[test]
fn a_visits_reads_fold_into_one_record_per_tuple_before_the_arrival_that_ends_it() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &gate_read("a", "notes", None)); // 3
    commit_as(
        &f,
        None,
        "wf",
        &read_of("a", "spec", "gate", "presence", Some("s1")),
    ); // 4
    commit_as(&f, None, "wf", &gate_read("a", "notes", Some("n1"))); // 5
    commit_as(&f, None, "wf", &cli_read("a", "plan", Some("p1"))); // 6
    commit_as(&f, None, "wf", &gate_read("a", "notes", Some("n2"))); // 7
    reset_for_test();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 8
    assert_eq!(batches_for_test(), (1, 1), "one flushed batch");
    assert_eq!(last_seq(&f, "wf"), 8);

    let j = f.journal();
    let tail: Vec<String> = kinds(&j).into_iter().rev().take(4).rev().collect();
    assert_eq!(
        tail,
        vec![
            "context_reads",
            "context_reads",
            "context_reads",
            "state_entered"
        ]
    );
    let c = context_records(&j);
    let t = |k: &str, r: &str, a: &str, n, p| (k.into(), r.into(), a.into(), n, p);
    assert_eq!(
        tuples(&c),
        vec![
            t("notes", "gate", "content", 3, 2),
            t("spec", "gate", "presence", 1, 1),
            t("plan", "cli", "content", 1, 1),
        ]
    );
    assert_eq!(c[0]["koto.context.first_hash"], hash_of("n1"));
    assert_eq!(c[0]["koto.context.last_hash"], hash_of("n2"));
    assert_eq!(c[1]["koto.context.first_hash"], hash_of("s1"));
    assert_eq!(c[1]["koto.context.last_hash"], hash_of("s1"));
    for r in &c {
        assert_eq!(r["koto.seq"], 8);
        assert_eq!(r["koto.visit.seq"], 2);
        assert_eq!(r["koto.state"], "a");
        assert_eq!(r["koto.run.id"], "wf-id");
    }
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for content in ["n1", "n2", "s1", "p1"] {
        assert!(!raw.contains(&format!("\"{content}\"")), "{raw}");
    }

    // The next visit's arrival writes that visit's reads only.
    commit_as(&f, None, "wf", &gate_read("b", "notes", Some("n2"))); // 9
    commit_as(&f, None, "wf", &transitioned(Some("b"), "c")); // 10
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 4);
    assert_eq!(tuples(&c[3..]), vec![t("notes", "gate", "content", 1, 1)]);
    assert_eq!(c[3]["koto.visit.seq"], 8);
    assert_eq!(c[3]["koto.seq"], 10);
    assert_eq!(c[3]["koto.state"], "b");
}

/// A self-transition stays in the visit, so it writes no reads, and the
/// reads on both sides of it are one visit's.
#[test]
fn a_self_transition_writes_no_reads_and_keeps_counting_the_visit() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &gate_read("a", "k", None)); // 3
    commit_as(&f, None, "wf", &transitioned(Some("a"), "a")); // 4
    assert!(context_records(&f.journal()).is_empty());
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("v"))); // 5
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 6
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["koto.context.read_count"], 2);
    assert_eq!(c[0]["koto.visit.seq"], 2);
}

/// 52 distinct keys read in one visit write 50 records and one overflow
/// record, which sums the reads of the other two and names none of them.
/// A tuple already kept goes on counting in its own record.
#[test]
fn fifty_two_keys_write_fifty_records_and_one_overflow_record() {
    let f = session_in_a();
    for i in 0..52 {
        commit_as(&f, None, "wf", &gate_read("a", &format!("k{i:02}"), None));
    }
    commit_as(&f, None, "wf", &gate_read("a", "k50", Some("x")));
    commit_as(&f, None, "wf", &gate_read("a", "k51", Some("y")));
    commit_as(&f, None, "wf", &gate_read("a", "k00", Some("z")));
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b"));
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 51);
    let keys: Vec<String> = c[..50]
        .iter()
        .map(|r| r["koto.context.key"].as_str().unwrap().to_string())
        .collect();
    let expected: Vec<String> = (0..50).map(|i| format!("k{i:02}")).collect();
    assert_eq!(keys, expected);
    assert_eq!(c[0]["koto.context.read_count"], 2);
    assert_eq!(c[0]["koto.context.present_count"], 1);
    let overflow = &c[50];
    assert_eq!(overflow["koto.context.overflow"], true);
    assert_eq!(overflow["koto.context.read_count"], 4);
    assert_eq!(overflow["koto.context.present_count"], 2);
    assert_eq!(overflow["koto.state"], "a");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("k50") && !raw.contains("k51"), "{raw}");
}

/// A key outside the name shape (longer than 128 bytes) is written as its
/// digest, and one at 128 bytes as itself.
#[test]
fn a_key_outside_the_name_shape_is_digested() {
    let f = session_in_a();
    let long = format!("k{}", "e".repeat(200));
    let edge = "q".repeat(128);
    commit_as(&f, None, "wf", &gate_read("a", &long, None));
    commit_as(&f, None, "wf", &gate_read("a", &edge, None));
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b"));
    let c = context_records(&f.journal());
    assert_eq!(c[0]["koto.context.key"], name_or_digest(&long).as_str());
    assert!(c[0]["koto.context.key"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(c[1]["koto.context.key"], edge.as_str());
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains(&long), "{raw}");

    // A reader or access outside the shape is digested the same way, so
    // two odd values stay two records.
    let g = session_in_a();
    for reader in ["odd reader", "odd  reader"] {
        commit_as(
            &g,
            None,
            "wf",
            &read_of("a", "k", reader, "odd access", None),
        );
    }
    commit_as(&g, None, "wf", &transitioned(Some("a"), "b"));
    let c = context_records(&g.journal());
    assert_eq!(c.len(), 2);
    for (r, reader) in c.iter().zip(["odd reader", "odd  reader"]) {
        assert_eq!(r["koto.context.reader"], name_or_digest(reader).as_str());
        assert_eq!(
            r["koto.context.access"],
            name_or_digest("odd access").as_str()
        );
    }
}

/// The tuple is the key, the reader and the access together: one key read
/// in one visit by `koto context get` (cli, content), a context-exists
/// gate (gate, presence) and a context-matches gate (gate, content) gives
/// three records, in first-read order, each with its own counts.
#[test]
fn one_key_read_by_two_readers_and_two_accesses_gives_one_record_per_tuple() {
    let f = session_in_a();
    let exists = |content| read_of("a", "notes", "gate", "presence", Some(content));
    commit_as(&f, None, "wf", &cli_read("a", "notes", Some("n1"))); // 3
    commit_as(&f, None, "wf", &exists("n1")); // 4
    commit_as(&f, None, "wf", &gate_read("a", "notes", None)); // 5
    commit_as(&f, None, "wf", &exists("n2")); // 6
    commit_as(&f, None, "wf", &gate_read("a", "notes", Some("n2"))); // 7
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 8
    let c = context_records(&f.journal());
    let t = |r: &str, a: &str, n, p| ("notes".into(), r.into(), a.into(), n, p);
    assert_eq!(
        tuples(&c),
        vec![
            t("cli", "content", 1, 1),
            t("gate", "presence", 2, 2),
            t("gate", "content", 2, 1),
        ]
    );
    assert_eq!(c[0]["koto.context.last_hash"], hash_of("n1"));
    assert_eq!(c[1]["koto.context.first_hash"], hash_of("n1"));
    assert_eq!(c[1]["koto.context.last_hash"], hash_of("n2"));
    assert_eq!(c[2]["koto.context.first_hash"], hash_of("n2"));
}

/// A `request_store.result` whose visit has no read left to write writes
/// nothing at all.
#[test]
fn a_result_with_no_reads_to_write_writes_nothing() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "done"));
    reset_for_test();
    commit_as(&f, None, "wf", &store_result());
    assert_eq!(batches_for_test(), (0, 0));
    assert!(warnings_for_test().is_empty());
}

/// A rewind ends the visit it leaves: its reads are written before the
/// rewind's `state_entered`, and the visit it opens counts from zero.
#[test]
fn a_rewind_writes_the_visit_it_leaves_once() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 3
    commit_as(&f, None, "wf", &gate_read("b", "k", Some("v"))); // 4
    commit_as(&f, None, "wf", &rewound("b", "a")); // 5
    commit_as(&f, None, "wf", &gate_read("a", "k", None)); // 6
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 7
    let j = f.journal();
    let c = context_records(&j);
    let seqs: Vec<(u64, u64, &str, u64)> = c
        .iter()
        .map(|r| {
            (
                r["koto.seq"].as_u64().unwrap(),
                r["koto.visit.seq"].as_u64().unwrap(),
                r["koto.state"].as_str().unwrap(),
                r["koto.context.read_count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(seqs, vec![(5, 3, "b", 1), (7, 5, "a", 1)]);
    // Each before the `state_entered` of the event that ended its visit.
    let k = kinds(&j);
    let positions: Vec<usize> = k
        .iter()
        .enumerate()
        .filter(|(_, k)| *k == "context_reads")
        .map(|(i, _)| i)
        .collect();
    for i in positions {
        assert_eq!(k[i + 1], "state_entered");
    }
}

/// A cancel writes the open visit's reads before `cancelled`. Reads after
/// it aren't journaled: a second cancel and a later rewind write no reads.
#[test]
fn a_cancel_writes_the_open_visit_and_a_second_cancel_writes_nothing_more() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("v"))); // 3
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("v"))); // 4
    reset_for_test();
    commit_as(&f, None, "wf", &cancelled("a")); // 5
    assert_eq!(batches_for_test(), (1, 1));
    let tail: Vec<String> = kinds(&f.journal())
        .into_iter()
        .rev()
        .take(2)
        .rev()
        .collect();
    assert_eq!(tail, vec!["context_reads", "cancelled"]);
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["koto.context.read_count"], 2);
    assert_eq!(c[0]["koto.seq"], 5);
    assert_eq!(c[0]["koto.visit.seq"], 2);

    // A read after the cancel, a second cancel (another process racing the
    // first, or a respawn fallback), then a rewind out of it.
    commit_as(&f, None, "wf", &cli_read("a", "k", Some("v"))); // 6
    commit_as(&f, None, "wf", &cancelled("a")); // 7
    commit_as(&f, None, "wf", &rewound("a", "a")); // 8
    let j = f.journal();
    assert_eq!(context_records(&j).len(), 1, "{j:?}");
    let tail: Vec<String> = kinds(&j).into_iter().rev().take(2).rev().collect();
    assert_eq!(tail, vec!["cancelled", "state_entered"]);
    // The rewind opened a visit of its own, which counts its own reads.
    commit_as(&f, None, "wf", &gate_read("a", "k", None)); // 9
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 10
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 2);
    assert_eq!(c[1]["koto.visit.seq"], 8);
    assert_eq!(c[1]["koto.context.read_count"], 1);
}

/// The terminal tick's `request_store.result` ends the terminal state's
/// visit: its batch holds the visit's reads alone, unflushed, and the
/// `terminal` record's flushed write follows. A read on the kept terminal
/// afterwards isn't journaled, and a rewind out of it writes no reads.
#[test]
fn request_store_result_writes_the_terminal_visit_and_a_rewind_out_of_it_writes_nothing() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("v"))); // 3
    commit_as(&f, None, "wf", &transitioned(Some("a"), "done")); // 4
    commit_as(
        &f,
        None,
        "wf",
        &read_of("done", "k", "result", "content", Some("v")),
    ); // 5
    commit_as(
        &f,
        None,
        "wf",
        &read_of("done", "gone", "result", "content", None),
    ); // 6
    reset_for_test();
    commit_as(&f, None, "wf", &store_result()); // 7
    assert_eq!(batches_for_test(), (1, 0), "context reads alone, unflushed");
    let header = f.backend.read_header("wf").unwrap();
    terminal(Some(&f.home), &f.backend, "wf", &header, "done");
    assert_eq!(batches_for_test(), (2, 1), "the terminal is flushed");

    let j = f.journal();
    let tail: Vec<String> = kinds(&j).into_iter().rev().take(3).rev().collect();
    assert_eq!(tail, vec!["context_reads", "context_reads", "terminal"]);
    let c = context_records(&j);
    let t = |k: &str, n, p| {
        (
            k.to_string(),
            "result".to_string(),
            "content".to_string(),
            n,
            p,
        )
    };
    assert_eq!(tuples(&c[1..]), vec![t("k", 1, 1), t("gone", 1, 0)]);
    for r in &c[1..] {
        assert_eq!(r["koto.seq"], 7);
        assert_eq!(r["koto.visit.seq"], 4);
        assert_eq!(r["koto.state"], "done");
    }

    // `koto context get` on the kept terminal, then a rewind out of it.
    commit_as(&f, None, "wf", &cli_read("done", "k", Some("v"))); // 8
    commit_as(&f, None, "wf", &rewound("done", "a")); // 9
    let j = f.journal();
    assert_eq!(context_records(&j).len(), 3, "nothing more: {j:?}");
    assert_eq!(kinds(&j).last().unwrap(), "state_entered");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

/// When the terminal tick's `request_store.result` failed to commit, the
/// terminal visit stays open: its reads stay in the log, a read made after
/// the `terminal` record joins them (the one exception to reads after a
/// terminal going unjournaled), and the next arrival writes them once.
#[test]
fn a_failed_request_store_result_leaves_the_visit_to_the_next_arrival() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "failed")); // 3
    let reason = read_of("failed", "failure_reason", "result", "content", Some("r"));
    commit_as(&f, None, "wf", &reason); // 4

    // No `request_store.result`: its append failed. The terminal is still
    // recorded.
    let header = f.backend.read_header("wf").unwrap();
    terminal(Some(&f.home), &f.backend, "wf", &header, "failed");
    assert!(context_records(&f.journal()).is_empty());
    // `koto context get` on the kept failure terminal.
    let late = cli_read("failed", "failure_reason", Some("r"));
    commit_as(&f, None, "wf", &late); // 5
    commit_as(&f, None, "wf", &rewound("failed", "a")); // 6
    let j = f.journal();
    let c = context_records(&j);
    assert_eq!(c.len(), 2);
    let t = |r: &str, n, p| ("failure_reason".into(), r.into(), "content".into(), n, p);
    assert_eq!(tuples(&c), vec![t("result", 1, 1), t("cli", 1, 1)]);
    for r in &c {
        assert_eq!(r["koto.state"], "failed");
        assert_eq!(r["koto.seq"], 6);
        assert_eq!(r["koto.visit.seq"], 3);
    }
    let tail: Vec<String> = kinds(&j).into_iter().rev().take(3).rev().collect();
    assert_eq!(
        tail,
        vec!["context_reads", "context_reads", "state_entered"]
    );
    // Another arrival writes nothing more for that visit.
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 7
    assert_eq!(context_records(&f.journal()).len(), 2);
}

/// An imported session's log carries the source's reads in its open visit,
/// before its `session_imported`. Only the reads after the import are
/// written when that visit ends.
#[test]
fn an_imported_sessions_visit_counts_only_the_reads_after_the_import() {
    let f = Fixture::new();
    let events = vec![
        event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: String::new(),
                variables: Default::default(),
                spawn_entry: None,
                machine: None,
            },
        ),
        event(2, transitioned(None, "a")),
        event(3, gate_read("a", "k", Some("v"))),
        event(4, gate_read("a", "k", Some("v"))),
        event(
            5,
            EventPayload::SessionImported {
                from_workspace: "PLANTED-WORKSPACE".into(),
                from_session: "wf".into(),
                from_session_id: "src-id".into(),
                machine_id: "m".into(),
            },
        ),
    ];
    f.backend
        .init_state_file("wf", header("wf", "wf-id", None), events)
        .unwrap();
    commit_as(&f, None, "wf", &gate_read("a", "k", None)); // 6
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 7
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["koto.context.read_count"], 1);
    assert_eq!(c[0]["koto.context.present_count"], 0);
    assert_eq!(c[0]["koto.visit.seq"], 2);
    assert_eq!(c[0]["koto.seq"], 7);

    // An import with no read after it writes nothing.
    let g = Fixture::new();
    let mut events = vec![
        event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: String::new(),
                variables: Default::default(),
                spawn_entry: None,
                machine: None,
            },
        ),
        event(2, transitioned(None, "a")),
        event(3, gate_read("a", "k", Some("v"))),
    ];
    events.push(event(
        4,
        EventPayload::SessionImported {
            from_workspace: "w".into(),
            from_session: "wf".into(),
            from_session_id: "src-id".into(),
            machine_id: "m".into(),
        },
    ));
    g.backend
        .init_state_file("wf", header("wf", "wf-id", None), events)
        .unwrap();
    commit_as(&g, None, "wf", &transitioned(Some("a"), "b"));
    assert!(context_records(&g.journal()).is_empty());
}

/// Under a new driver, an arrival's batch is `driver_seen`, the visit's
/// reads, then `state_entered`.
#[test]
fn a_new_drivers_record_comes_before_the_visits_reads() {
    let f = session_in_a();
    commit_as(&f, None, "wf", &gate_read("a", "k", None));
    commit_as(&f, Some("driver-z"), "wf", &transitioned(Some("a"), "b"));
    let tail: Vec<String> = kinds(&f.journal())
        .into_iter()
        .rev()
        .take(3)
        .rev()
        .collect();
    assert_eq!(tail, vec!["driver_seen", "context_reads", "state_entered"]);
}

/// A visit's `context_reads`, its check records and the responses printed
/// in it carry the same `koto.visit.seq`, since all of them come from
/// `persistence::current_visit`'s rule: a self-transition keeps the visit,
/// and the reads are written, with the visit that ended, at the arrival
/// elsewhere or the rewind that opens the next one.
#[test]
fn context_reads_share_the_visit_rule_with_check_and_response_records() {
    let f = session_in_a();
    let respond = |state: &str| {
        response(
            Some(&f.home),
            &f.backend,
            "wf",
            state,
            ResponseSource::Next,
            "d",
            None,
        )
    };
    let check = |state: &str| {
        commit_as(
            &f,
            None,
            "wf",
            &gate_event(
                state,
                "g",
                "passed",
                command_output(0),
                stamped(1, 1, None),
                None,
            ),
        )
    };
    check("a"); // 3, visit 2
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("one"))); // 4
    respond("a");
    commit_as(&f, None, "wf", &transitioned(Some("a"), "a")); // 5
    commit_as(&f, None, "wf", &gate_read("a", "k", Some("two"))); // 6
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 7
    check("b"); // 8, visit 7
    commit_as(&f, None, "wf", &gate_read("b", "k", None)); // 9
    respond("b");
    commit_as(&f, None, "wf", &rewound("b", "a")); // 10
    respond("a");
    let j = f.journal();
    let got: Vec<(String, u64, u64)> = j
        .iter()
        .filter(|r| r["koto.visit.seq"].is_u64())
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["koto.seq"].as_u64().unwrap(),
                r["koto.visit.seq"].as_u64().unwrap(),
            )
        })
        .collect();
    let row = |k: &str, seq: u64, visit: u64| (k.to_string(), seq, visit);
    assert_eq!(
        got,
        vec![
            row("check_evaluated", 3, 2),
            row("response", 4, 2),
            row("context_reads", 7, 2),
            row("check_evaluated", 8, 7),
            row("response", 9, 7),
            row("context_reads", 10, 7),
            row("response", 10, 10),
        ]
    );
    let c = context_records(&j);
    assert_eq!(c[0]["koto.context.read_count"], 2, "{}", c[0]);
    assert_eq!(c[0]["koto.context.last_hash"], hash_of("two"));
    assert_eq!(c[1]["koto.state"], "b");
}

/// With rotation on and the journal past its bound, the commit that ends a
/// visit rotates the journal once, before its batch: the rotated file keeps
/// every byte it had, and the visit's `context_reads` and the arrival's
/// `state_entered` land whole, together, in the fresh journal, as one
/// flushed batch.
#[cfg(unix)]
#[test]
fn a_closing_batch_lands_whole_in_the_fresh_journal_after_a_rotation() {
    let f = session_in_a();
    for key in ["k1", "k2", "k3"] {
        commit_as(&f, None, "wf", &gate_read("a", key, Some(key))); // 3, 4, 5
    }
    let journal = f.home.join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&journal).unwrap();
    assert!(!before.is_empty());
    rotate_at_one_byte();
    reset_for_test();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b")); // 6
    assert_eq!(batches_for_test(), (1, 1));
    let rotated = f.home.join(rotate::rotated_name(1));
    assert_eq!(std::fs::read_to_string(&rotated).unwrap(), before);
    assert!(!f.home.join(rotate::rotated_name(2)).exists());
    let j = f.journal();
    assert_eq!(
        kinds(&j),
        vec![
            "context_reads",
            "context_reads",
            "context_reads",
            "state_entered"
        ]
    );
    let c = context_records(&j);
    assert_eq!(
        c.iter()
            .map(|r| r["koto.context.key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["k1", "k2", "k3"]
    );
    for r in &c {
        assert_eq!(r["koto.seq"], 6, "{r}");
        assert_eq!(r["koto.visit.seq"], 2, "{r}");
    }
    assert_eq!(j[3]["koto.state"], "b");
    assert!(warnings_for_test().is_empty(), "{:?}", warnings_for_test());
    assert!(
        rotate::warnings_for_test().is_empty(),
        "{:?}",
        rotate::warnings_for_test()
    );
}

/// A session whose current visit holds 10,000 events (reads of 60 keys,
/// failed check events with 2 KB of output, evidence): a 4 MB log.
fn ten_thousand_event_visit() -> Fixture {
    let f = Fixture::new();
    let mut events = vec![
        event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: String::new(),
                variables: Default::default(),
                spawn_entry: None,
                machine: None,
            },
        ),
        event(2, transitioned(None, "a")),
    ];
    let output = serde_json::json!({"exit_code": 1, "error": "", "stdout": "x".repeat(2000)});
    for i in 0..10_000u64 {
        let payload = match i % 10 {
            0 => gate_event(
                "a",
                "lint",
                "failed",
                output.clone(),
                stamped(i + 1, i + 1, None),
                None,
            ),
            1 => evidence("a"),
            n => gate_read("a", &format!("key-{}", (i / 10 * 8 + n) % 60), Some("v")),
        };
        events.push(event(3 + i, payload));
    }
    f.backend
        .init_state_file("wf", header("wf", "wf-id", None), events)
        .unwrap();
    f
}

/// The arrival that ends a 10,000-event visit writes 51 lines (50 of the
/// 60 tuples and the overflow) whose counts add up to the visit's 8,000
/// reads.
#[test]
fn a_ten_thousand_event_visit_writes_fifty_one_lines() {
    let f = ten_thousand_event_visit();
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b"));
    let c = context_records(&f.journal());
    assert_eq!(c.len(), 51);
    assert_eq!(c[50]["koto.context.overflow"], true);
    let total: u64 = c
        .iter()
        .map(|r| r["koto.context.read_count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 8_000);
    for r in &c {
        assert_eq!(r["koto.seq"], 10_003);
        assert_eq!(r["koto.visit.seq"], 2);
    }
}

/// The work the fold adds to the arrival that ends a 10,000-event visit
/// (finding the visit and folding its reads) stays within the 15 ms the
/// hot path allows a tick. The budget is for release builds on an idle
/// machine, so the test is left out of the default run. Run it with
/// `cargo test --release --lib a_ten_thousand -- --ignored`.
#[test]
#[ignore = "wall-clock budget for release builds"]
fn a_ten_thousand_event_visit_folds_within_budget() {
    let f = ten_thousand_event_visit();
    let path = f
        .backend
        .session_dir("wf")
        .join(crate::session::state_file_name("wf"));
    let log = std::fs::read_to_string(&path).unwrap();
    let relay = Relay {
        session: "wf",
        session_id: "wf-id",
        run_id: Some("wf-id"),
        seq: 10_003,
        visit_seq: Some(2),
    };
    let mut times = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let visit = crate::engine::persistence::visit_start(&log).unwrap();
        let lines = context_reads::records(&log, visit, &relay).len();
        times.push(started.elapsed());
        assert_eq!(lines, 51);
    }
    times.sort();
    let median = times[2];
    assert!(
        median < std::time::Duration::from_millis(15),
        "the fold took {median:?}"
    );
}

// ----- Decider fallback records -----

fn fallback(rule_id: &str) -> crate::decider::check::DeciderFallback {
    crate::decider::check::DeciderFallback {
        state: "a".into(),
        visit_seq: 99,
        gate: "comments".into(),
        rule_id: rule_id.into(),
        declaration_hash: "PLANTED-DECLARATION-HASH".into(),
        mode: CheckMode::Veto,
        reason: UnansweredReason::OverBudget,
        input_sha256: Some("PLANTED-INPUT-HASH".into()),
        input_bytes: Some(2561),
        unit: None,
        unit_labels: None,
    }
}

#[test]
fn a_fallback_relays_decider_fallback_with_the_events_values() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::DeciderFallback(fallback("r")),
    );
    let no_slice = crate::decider::check::DeciderFallback {
        mode: CheckMode::Shadow,
        reason: UnansweredReason::ExtractionFailed,
        input_sha256: None,
        input_bytes: None,
        ..fallback("no reason [x]")
    };
    commit_as(&f, None, "wf", &EventPayload::DeciderFallback(no_slice));

    let d = decider_records(&f.journal());
    assert_eq!(kinds(&d), vec!["decider_fallback", "decider_fallback"]);
    let r = &d[0];
    assert_eq!(r["koto.state"], "a");
    assert_eq!(r["koto.check"], "comments");
    assert_eq!(r["koto.rule.id"], "r");
    assert_eq!(r["koto.decider.mode"], "veto");
    assert_eq!(r["koto.decider.reason"], "over_budget");
    assert_eq!(r["koto.decider.input_bytes"], 2561);
    let r = &d[1];
    assert_eq!(r["koto.decider.mode"], "shadow");
    assert_eq!(r["koto.decider.reason"], "extraction_failed");
    assert!(r.get("koto.decider.input_bytes").is_none(), "{r}");
    // A rule id outside the name shape is digested, as on decider_checked.
    assert!(
        r["koto.rule.id"].as_str().unwrap().starts_with("sha256:"),
        "{r}"
    );
    let text = serde_json::to_string(&f.journal()).unwrap();
    for planted in ["PLANTED-DECLARATION-HASH", "PLANTED-INPUT-HASH"] {
        assert!(!text.contains(planted), "{planted} reached the journal");
    }
}

#[test]
fn a_fallback_record_with_the_longest_names_fits_a_line() {
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let p = crate::decider::check::DeciderFallback {
        state: "t".repeat(128),
        gate: "g".repeat(128),
        input_bytes: Some(u64::MAX),
        ..fallback(&"r".repeat(128))
    };
    let lines: Vec<String> = relayed(&EventPayload::DeciderFallback(p), &relay)
        .iter()
        .map(Record::to_line)
        .collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].len() < MAX_LINE_BYTES, "{} bytes", lines[0].len());
    let r: serde_json::Value = serde_json::from_str(lines[0].trim_end()).unwrap();
    assert_eq!(r["koto.check"], "g".repeat(128));
    assert_eq!(r["koto.rule.id"], "r".repeat(128));
}

#[test]
fn a_waiting_decider_check_writes_check_pending_with_its_counts_and_no_poll() {
    let f = session_in_a();
    let output = serde_json::json!({"failed": [], "unanswered": ["r"],
        "awaiting": ["r", "s"], "agent_answered": ["t"], "error": ""});
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "comments",
            "pending",
            output,
            CheckEventFields::default(),
            None,
        ),
    );
    let c = check_records(&f.journal());
    assert_eq!(kinds(&c), vec!["check_pending"]);
    let r = &c[0];
    assert_eq!(r["koto.decider.awaiting_count"], 2);
    assert_eq!(r["koto.decider.agent_answered_count"], 1);
    for absent in [
        "koto.poll.status",
        "koto.poll.evaluations",
        "koto.poll.elapsed_secs",
    ] {
        assert!(r.get(absent).is_none(), "{absent}: {r}");
    }
}

// ----- Finding and dispute records -----

/// A finding as a check event logs it, with a planted message, path and
/// rule reference that must never reach the journal.
fn finding(rule_id: &str, level: &str) -> crate::findings::Finding {
    serde_json::from_value(serde_json::json!({
        "rule_id": rule_id,
        "level": level,
        "message": "PLANTED-MESSAGE",
        "path": "PLANTED/path.rs",
        "line": 3,
        "rule_ref": "https://example.org/PLANTED",
        "message_source": "check",
    }))
    .unwrap()
}

fn with_findings_logged(
    mut check: CheckEventFields,
    findings: Vec<crate::findings::Finding>,
    truncated: bool,
) -> CheckEventFields {
    check.findings = findings;
    check.findings_truncated = truncated;
    check
}

fn assert_no_finding_content(f: &Fixture) {
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for planted in ["PLANTED", "message", "rule_ref"] {
        assert!(!raw.contains(planted), "{planted} in {raw}");
    }
}

#[test]
fn a_passing_gate_writes_one_check_finding_per_rule_and_level_and_no_check_rule() {
    let f = session_in_a();
    reset_for_test();
    let check = with_findings_logged(
        stamped(3, 2, None),
        vec![
            finding("A", "error"),
            finding("B", "warning"),
            finding("A", "error"),
        ],
        false,
    );
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "passed", command_output(0), check, None),
    );
    // The evaluation and its findings are one batch.
    assert_eq!(batches_for_test(), (1, 0));
    let c = check_records(&f.journal());
    assert_eq!(
        kinds(&c),
        vec!["check_evaluated", "check_finding", "check_finding"]
    );
    assert_eq!(c[0]["koto.check.outcome"], "passed");
    assert_eq!(c[0]["koto.check.finding_count"], 3);
    assert_eq!(c[0]["koto.check.rule_count"], 0);
    assert!(c[0].get("koto.check.findings_truncated").is_none());
    let pairs: Vec<(&str, &str, u64)> = c[1..]
        .iter()
        .map(|r| {
            (
                r["koto.rule.id"].as_str().unwrap(),
                r["koto.finding.level"].as_str().unwrap(),
                r["koto.finding.count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(pairs, vec![("A", "error", 2), ("B", "warning", 1)]);
    for r in &c[1..] {
        assert_eq!(r["koto.seq"], c[0]["koto.seq"], "{r}");
        assert_eq!(r["koto.visit.seq"], c[0]["koto.visit.seq"], "{r}");
        assert_eq!(r["koto.state"], "a");
        assert_eq!(r["koto.check.name"], "lint");
        assert!(r.get("koto.check").is_none(), "{r}");
        assert_eq!(r["koto.attempt"], 3);
        assert_eq!(r["koto.visit_attempt"], 2);
    }
    assert_no_finding_content(&f);
}

#[test]
fn a_failing_gate_keeps_its_check_rule_and_adds_a_check_finding() {
    let f = session_in_a();
    let check = with_findings_logged(
        stamped(1, 1, Some(counts(&[("E501", 1, 1)]))),
        vec![finding("E501", "error")],
        false,
    );
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "failed", command_output(1), check, None),
    );
    let c = check_records(&f.journal());
    assert_eq!(
        kinds(&c),
        vec!["check_evaluated", "check_rule", "check_finding"]
    );
    let keys: BTreeSet<String> = c[1].as_object().unwrap().keys().cloned().collect();
    let expected: BTreeSet<String> = RELAY_FIELDS
        .iter()
        .chain(check_kind_fields("check_rule"))
        .map(|s| s.to_string())
        .collect();
    assert_eq!(keys, expected, "check_rule's fields are unchanged");
    assert_eq!(c[1]["koto.rule.id"], "E501");
    assert_eq!(c[1]["koto.rule.visit_count"], 1);
    assert_eq!(c[2]["koto.rule.id"], "E501");
    assert_eq!(c[2]["koto.finding.level"], "error");
    assert_eq!(c[2]["koto.finding.count"], 1);
    assert_eq!(c[0]["koto.check.finding_count"], 1);
}

#[test]
fn a_passing_gate_with_no_findings_writes_a_zero_count_and_no_check_finding() {
    let f = session_in_a();
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "lint",
            "passed",
            command_output(0),
            stamped(1, 1, None),
            None,
        ),
    );
    let c = check_records(&f.journal());
    assert_eq!(kinds(&c), vec!["check_evaluated"]);
    assert_eq!(c[0]["koto.check.finding_count"], 0);
    assert!(c[0].get("koto.check.findings_truncated").is_none());
}

/// Sixty findings reported, ten `error` and fifty `info`: the event logs
/// fifty (koto's log cap keeps the errors), and the journal writes at most
/// fifty `check_finding` records with every error pair first.
#[test]
fn findings_cut_at_the_log_cap_keep_every_error_pair_and_say_so() {
    let f = session_in_a();
    let reported: Vec<crate::findings::Finding> = (0..50)
        .map(|i| finding(&format!("I{i:02}"), "info"))
        .chain((0..10).map(|i| finding(&format!("E{i:02}"), "error")))
        .collect();
    let (logged, truncated) =
        crate::findings::cap_findings(&reported, None, crate::findings::LOG_FINDINGS_CAP);
    assert!(truncated);
    let check = with_findings_logged(stamped(1, 1, None), logged.clone(), truncated);
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "passed", command_output(0), check, None),
    );
    let c = check_records(&f.journal());
    assert_eq!(c[0]["koto.check.finding_count"], logged.len() as u64);
    assert_eq!(c[0]["koto.check.findings_truncated"], true);
    let found: Vec<&serde_json::Value> =
        c.iter().filter(|r| r["kind"] == "check_finding").collect();
    assert!(found.len() <= 50, "{}", found.len());
    for i in 0..10 {
        let id = format!("E{i:02}");
        assert!(
            found
                .iter()
                .any(|r| r["koto.rule.id"] == id.as_str() && r["koto.finding.level"] == "error"),
            "{id} missing"
        );
    }
    assert!(found[..10]
        .iter()
        .all(|r| r["koto.finding.level"] == "error"));
}

/// An event holding more distinct pairs than the record cap still writes
/// fifty records, levels in order and each level in first-seen order.
#[test]
fn more_pairs_than_the_record_cap_write_fifty_by_level() {
    let f = session_in_a();
    let findings: Vec<crate::findings::Finding> = (0..30)
        .map(|i| finding(&format!("I{i:02}"), "info"))
        .chain((0..5).map(|i| finding(&format!("X{i:02}"), "notice")))
        .chain((0..20).map(|i| finding(&format!("W{i:02}"), "warning")))
        .chain((0..10).map(|i| finding(&format!("E{i:02}"), "error")))
        .collect();
    let check = with_findings_logged(stamped(1, 1, None), findings, false);
    commit_as(&f, None, "wf", &action_event(0, check));
    let c = check_records(&f.journal());
    let found: Vec<&serde_json::Value> =
        c.iter().filter(|r| r["kind"] == "check_finding").collect();
    assert_eq!(found.len(), 50);
    let levels: Vec<&str> = found
        .iter()
        .map(|r| r["koto.finding.level"].as_str().unwrap())
        .collect();
    assert_eq!(&levels[..10], &["error"; 10]);
    assert_eq!(&levels[10..30], &["warning"; 20]);
    assert_eq!(&levels[30..], &["info"; 20]);
    assert_eq!(found[10]["koto.rule.id"], "W00");
    assert_eq!(found[49]["koto.rule.id"], "I19");
    assert!(found.iter().all(|r| r["koto.check.name"] == "__action__"));
    assert_eq!(c[0]["koto.check.finding_count"], 65);
}

/// koto's own fallback finding, whose rule id is the check's name, is a
/// finding like any other; a pending evaluation writes none.
#[test]
fn the_fallback_finding_is_journaled_and_a_pending_evaluation_writes_none() {
    let f = session_in_a();
    let mut fallback = finding("lint", "error");
    fallback.message_source = crate::findings::MessageSource::Koto;
    let check = with_findings_logged(
        stamped(1, 1, Some(counts(&[("lint", 1, 1)]))),
        vec![fallback],
        false,
    );
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "failed", command_output(1), check, None),
    );
    let pending = with_findings_logged(
        CheckEventFields::default(),
        vec![finding("ci", "warning")],
        false,
    );
    commit_as(
        &f,
        None,
        "wf",
        &gate_event(
            "a",
            "ci",
            "pending",
            command_output(75),
            pending,
            Some(poll("pending", 1, 0)),
        ),
    );
    let c = check_records(&f.journal());
    assert_eq!(
        kinds(&c),
        vec![
            "check_evaluated",
            "check_rule",
            "check_finding",
            "check_pending"
        ]
    );
    assert_eq!(c[2]["koto.rule.id"], "lint");
    assert!(c[3].get("koto.check.finding_count").is_none());
}

/// A rule id outside the name shape is digested on `check_finding` as on
/// `check_rule`, and an evaluation from before attempt numbers leaves them
/// out of its findings too.
#[test]
fn finding_records_digest_odd_rule_ids_and_follow_their_evaluations_attempts() {
    let f = session_in_a();
    let odd = "no explicit any [x]";
    let check = with_findings_logged(
        CheckEventFields::default(),
        vec![finding(odd, "warning")],
        false,
    );
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "passed", command_output(0), check, None),
    );
    let c = check_records(&f.journal());
    assert_eq!(c[1]["koto.rule.id"], name_or_digest(odd).as_str());
    assert!(c[1].get("koto.attempt").is_none());
    assert!(c[1].get("koto.visit_attempt").is_none());
}

fn dispute(state: &str, gate: &str, rule_id: &str, source: &str) -> EventPayload {
    disputed_attempt(state, gate, rule_id, source, Some(2), Some(1))
}

fn disputed_attempt(
    state: &str,
    gate: &str,
    rule_id: &str,
    source: &str,
    attempt: Option<u64>,
    visit_attempt: Option<u64>,
) -> EventPayload {
    EventPayload::FindingDisputed(crate::engine::types::FindingDispute {
        state: state.into(),
        gate: gate.into(),
        rule_id: rule_id.into(),
        reason: "PLANTED-REASON the rule misreads generated code".into(),
        source: source.into(),
        attempt,
        visit_attempt,
    })
}

#[test]
fn a_dispute_relays_check_disputed_without_its_reason() {
    let f = session_in_a();
    reset_for_test();
    commit_as(&f, None, "wf", &dispute("a", "lint", "E501", "agent"));
    assert_eq!(batches_for_test(), (1, 0), "relayed records aren't flushed");
    let c = check_records(&f.journal());
    assert_eq!(kinds(&c), vec!["check_disputed"]);
    let keys: BTreeSet<&str> = c[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = RELAY_FIELDS
        .iter()
        .chain(check_kind_fields("check_disputed"))
        .copied()
        .collect();
    assert_eq!(keys, expected);
    assert_eq!(c[0]["koto.seq"], 3);
    assert_eq!(c[0]["koto.visit.seq"], 2);
    assert_eq!(c[0]["koto.state"], "a");
    assert_eq!(c[0]["koto.check.name"], "lint");
    assert_eq!(c[0]["koto.attempt"], 2);
    assert_eq!(c[0]["koto.visit_attempt"], 1);
    assert_eq!(c[0]["koto.rule.id"], "E501");
    assert_eq!(c[0]["koto.dispute.source"], "agent");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    for absent in ["PLANTED", "reason", "misreads"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
}

/// A dispute carries the disputed evaluation's attempt numbers, so it joins
/// to exactly that `check_evaluated` (same state, `koto.check` equal to its
/// `koto.check.name`, same attempt) and not to the gate's other attempts. A
/// dispute of an evaluation from before attempt numbers leaves them out.
#[test]
fn a_dispute_joins_the_evaluation_it_names_by_attempt() {
    let f = session_in_a();
    for attempt in 1..=3 {
        commit_as(
            &f,
            None,
            "wf",
            &gate_event(
                "a",
                "lint",
                "failed",
                command_output(1),
                stamped(attempt, attempt, None),
                None,
            ),
        );
    }
    commit_as(
        &f,
        None,
        "wf",
        &disputed_attempt("a", "lint", "E501", "agent", Some(2), Some(2)),
    );
    commit_as(
        &f,
        None,
        "wf",
        &disputed_attempt("a", "lint", "E501", "person", None, None),
    );
    let c = check_records(&f.journal());
    let disputes: Vec<&serde_json::Value> =
        c.iter().filter(|r| r["kind"] == "check_disputed").collect();
    assert_eq!(disputes.len(), 2);
    let d = disputes[0];
    let joined: Vec<&serde_json::Value> = c
        .iter()
        .filter(|r| {
            r["kind"] == "check_evaluated"
                && r["koto.state"] == d["koto.state"]
                && r["koto.check"] == d["koto.check.name"]
                && r["koto.attempt"] == d["koto.attempt"]
                && r["koto.visit_attempt"] == d["koto.visit_attempt"]
        })
        .collect();
    assert_eq!(joined.len(), 1, "{joined:?}");
    assert_eq!(joined[0]["koto.attempt"], 2);
    let old = disputes[1];
    assert!(old.get("koto.attempt").is_none(), "{old}");
    assert!(old.get("koto.visit_attempt").is_none(), "{old}");
    assert_eq!(old["koto.dispute.source"], "person");
}

/// With the longest names koto keeps (a 255-byte session name, 128-byte
/// state, gate, rule id and level) and the largest numbers, every finding
/// and dispute record and the evaluation carrying the finding fields fit
/// the line cap.
#[test]
fn finding_and_dispute_records_fit_the_line_cap_at_the_longest_names() {
    let session = "s".repeat(255);
    let id = "i".repeat(128);
    let state = "t".repeat(128);
    let gate = "g".repeat(128);
    let level = "l".repeat(128);
    let relay = Relay {
        session: &session,
        session_id: &id,
        run_id: Some(&id),
        seq: u64::MAX,
        visit_seq: Some(u64::MAX),
    };
    let ids: Vec<String> = (0..50)
        .map(|i| format!("{i:02}{}", "r".repeat(126)))
        .collect();
    let findings: Vec<crate::findings::Finding> =
        ids.iter().map(|id| finding(id, &level)).collect();
    let rules: Vec<(&str, u64, u64)> = ids
        .iter()
        .map(|id| (id.as_str(), u64::MAX, u64::MAX))
        .collect();
    let mut check = CheckEventFields {
        attempt: Some(u64::MAX),
        visit_attempt: Some(u64::MAX),
        rule_counts: Some(counts(&rules)),
        duration_ms: Some(u64::MAX),
        ..CheckEventFields::default()
    };
    check.rule_counts_truncated = true;
    let check = with_findings_logged(check, findings, true);
    let mut records = relayed(
        &gate_event(
            &state,
            &gate,
            "failed",
            command_output(1),
            check,
            Some(poll("timed_out", u64::MAX, u64::MAX)),
        ),
        &relay,
    );
    records.extend(relayed(
        &disputed_attempt(
            &state,
            &gate,
            &"r".repeat(128),
            &level,
            Some(u64::MAX),
            Some(u64::MAX),
        ),
        &relay,
    ));
    let lines: Vec<String> = records.iter().map(Record::to_line).collect();
    assert_eq!(lines.len(), 1 + 50 + 50 + 1);
    for line in &lines {
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes: {line}", line.len());
    }
    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let c = check_records(&parsed);
    assert_eq!(c[0]["koto.check.findings_truncated"], true);
    assert_eq!(c[51]["koto.finding.level"], level.as_str());
    assert_eq!(c[51]["koto.check.name"], gate.as_str());
    assert_eq!(c[51]["koto.finding.count"], 1);
    assert_eq!(c[101]["koto.rule.id"], "r".repeat(128));
    assert_eq!(c[101]["koto.dispute.source"], level.as_str());
    assert_eq!(c[101]["koto.check.name"], gate.as_str());
    assert_eq!(c[101]["koto.attempt"], u64::MAX);
}

/// A read the reader's anchor recorded for another session (a
/// `context_read` carrying `session`) writes one `context_reads` record when
/// the anchor commits it, attributed to the session it names, with the
/// anchor as `koto.context.read_by` and no sequence numbers. The session
/// read's own visit fold never counts it: its log holds no such event, and
/// an anchor's fold skips one.
#[test]
fn an_anchors_read_record_is_journaled_against_the_session_it_names() {
    let f = Fixture::new();
    f.init("b", header("b", "b-id", None), Some("work"));
    f.init(
        "anchor-r",
        anchor_header("anchor-r", "anchor-id"),
        Some("open"),
    );
    let b_dir = f.backend.session_dir("b");
    let sidecar_before = std::fs::read(b_dir.join("run-journal.json")).ok();

    commit_as(&f, None, "b", &gate_read("work", "own", Some("x")));
    let mut other = cli_read("work", "notes.md", Some("v"));
    if let EventPayload::ContextRead { session, .. } = &mut other {
        *session = Some("b".to_string());
    }
    commit_as(&f, None, "anchor-r", &other);
    let mut absent = cli_read("work", "gone", None);
    if let EventPayload::ContextRead {
        session, access, ..
    } = &mut absent
    {
        *session = Some("b".to_string());
        *access = Some("presence".to_string());
    }
    commit_as(&f, None, "anchor-r", &absent);
    // An anchor that moves on folds none of the reads it recorded for b.
    commit_as(&f, None, "anchor-r", &transitioned(Some("open"), "later"));
    commit_as(&f, None, "b", &transitioned(Some("work"), "next"));

    let j = f.journal();
    let reads: Vec<&serde_json::Value> =
        j.iter().filter(|r| r["kind"] == "context_reads").collect();
    assert_eq!(reads.len(), 3, "{j:?}");

    let anchored = reads[0];
    assert_eq!(anchored["session"], "b");
    assert_eq!(anchored["koto.session.id"], "b-id");
    assert_eq!(anchored["koto.run.id"], "b-id");
    assert_eq!(anchored["koto.context.read_by"], "anchor-r");
    assert_eq!(anchored["koto.state"], "work");
    assert_eq!(anchored["koto.context.key"], "notes.md");
    assert_eq!(anchored["koto.context.reader"], "cli");
    assert_eq!(anchored["koto.context.access"], "content");
    assert_eq!(anchored["koto.context.read_count"], 1);
    assert_eq!(anchored["koto.context.present_count"], 1);
    assert_eq!(anchored["koto.context.first_hash"], hash_of("v"));
    assert_eq!(anchored["koto.context.last_hash"], hash_of("v"));
    assert!(anchored.get("koto.seq").is_none(), "{anchored}");
    assert!(anchored.get("koto.visit.seq").is_none(), "{anchored}");

    let missing = reads[1];
    assert_eq!(missing["session"], "b");
    assert_eq!(missing["koto.context.key"], "gone");
    assert_eq!(missing["koto.context.access"], "presence");
    assert_eq!(missing["koto.context.present_count"], 0);
    assert!(
        missing.get("koto.context.first_hash").is_none(),
        "{missing}"
    );

    // b's own visit fold: only its own read, never the anchor's.
    let own = reads[2];
    assert_eq!(own["session"], "b");
    assert_eq!(own["koto.context.key"], "own");
    assert!(own.get("koto.context.read_by").is_none(), "{own}");
    assert!(j
        .iter()
        .all(|r| !(r["session"] == "anchor-r" && r["kind"] == "context_reads")));

    // Journaling the anchor's read wrote nothing of b's.
    if let Some(before) = sidecar_before {
        assert_eq!(
            std::fs::read(b_dir.join("run-journal.json")).unwrap(),
            before
        );
    }
}

fn anchor_header(workflow: &str, session_id: &str) -> StateFileHeader {
    let mut h = header(workflow, session_id, None);
    h.kind = Some(crate::engine::types::SessionKind::ANCHOR.to_string());
    h
}

/// A `context_read` carrying `session` in a log that isn't an anchor's is
/// not a read record: koto writes the field only into a reader's anchor, so
/// nothing is journaled against the session it names.
#[test]
fn a_session_field_outside_an_anchor_is_not_journaled() {
    let f = Fixture::new();
    f.init("b", header("b", "b-id", None), Some("work"));
    f.init("c", header("c", "c-id", None), Some("work"));
    let mut forged = cli_read("work", "notes.md", Some("v"));
    if let EventPayload::ContextRead { session, .. } = &mut forged {
        *session = Some("b".to_string());
    }
    commit_as(&f, None, "c", &forged);
    commit_as(&f, None, "c", &transitioned(Some("work"), "next"));

    let j = f.journal();
    assert!(
        j.iter()
            .all(|r| !(r["kind"] == "context_reads" && r["session"] == "b")),
        "{j:?}"
    );
}

// ----- Check reasons, readiness, directive timing and evidence fields -----

fn logged_finding(rule: &str, level: crate::findings::FindingLevel) -> crate::findings::Finding {
    crate::findings::Finding {
        rule_id: rule.into(),
        level,
        message: "PLANTED-MESSAGE".into(),
        path: None,
        line: None,
        column: None,
        rule_ref: None,
        effect_landed: None,
        message_source: crate::findings::MessageSource::Check,
        unit: None,
    }
}

fn delivered_payload(state: &str, details: Option<bool>) -> EventPayload {
    EventPayload::InstructionsDelivered {
        state: state.into(),
        details_included: details,
    }
}

#[test]
fn a_failed_check_carries_its_reason_and_every_check_its_readiness() {
    use crate::findings::FindingLevel;
    let f = session_in_a();
    let mut failed = stamped(1, 1, None);
    failed.reason = Some("rule_failed".into());
    failed.not_ready = true;
    failed.findings = vec![logged_finding("E1", FindingLevel::Error)];
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "failed", command_output(1), failed, None),
    );
    // A reason outside the closed set is left out; a passing check never
    // carries one.
    let mut odd = stamped(2, 2, None);
    odd.reason = Some("PLANTED reason".into());
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "odd", "failed", command_output(1), odd, None),
    );
    let mut passed = stamped(3, 3, None);
    passed.reason = Some("no_finding".into());
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "ok", "passed", command_output(0), passed, None),
    );
    let c = check_records(&f.journal());
    let by = |check: &str| {
        c.iter()
            .find(|r| r["kind"] == "check_evaluated" && r["koto.check"] == check)
            .unwrap()
            .clone()
    };
    assert_eq!(by("lint")["koto.check.reason"], "rule_failed");
    assert_eq!(by("lint")["koto.check.not_ready"], true);
    assert!(by("odd").get("koto.check.reason").is_none());
    assert!(by("ok").get("koto.check.reason").is_none());
    assert_eq!(by("odd")["koto.check.not_ready"], false);
    assert_eq!(by("ok")["koto.check.not_ready"], false);
    let finding = c.iter().find(|r| r["kind"] == "check_finding").unwrap();
    assert_eq!(finding["koto.finding.source"], "check");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

#[test]
fn the_fallback_finding_is_sourced_to_koto_and_the_checks_own_to_the_check() {
    use crate::findings::FindingLevel;
    let f = session_in_a();
    let mut check = stamped(1, 1, None);
    check.reason = Some("no_finding".into());
    check.fallback_logged = true;
    check.findings = vec![
        logged_finding("W1", FindingLevel::Warning),
        // A warning the check printed under the gate's own name shares the
        // rule id with the fallback but not its level.
        logged_finding("lint", FindingLevel::Warning),
        logged_finding("lint", FindingLevel::Error),
    ];
    commit_as(
        &f,
        None,
        "wf",
        &gate_event("a", "lint", "failed", command_output(3), check, None),
    );
    let c = check_records(&f.journal());
    let sources: Vec<(String, String, String)> = c
        .iter()
        .filter(|r| r["kind"] == "check_finding")
        .map(|r| {
            (
                r["koto.rule.id"].as_str().unwrap().to_string(),
                r["koto.finding.level"].as_str().unwrap().to_string(),
                r["koto.finding.source"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        sources,
        vec![
            ("lint".into(), "error".into(), "fallback".into()),
            ("W1".into(), "warning".into(), "check".into()),
            ("lint".into(), "warning".into(), "check".into()),
        ]
    );
}

#[test]
fn an_evaluation_before_any_directive_in_its_visit_is_pre_directive() {
    let f = session_in_a();
    let gate = |n: u64| {
        gate_event(
            "a",
            "g",
            "passed",
            command_output(0),
            stamped(n, n, None),
            None,
        )
    };
    // The first tick after init: no directive printed yet.
    commit_as(&f, None, "wf", &gate(1));
    // A directive printed for this visit, details suppressed or not.
    commit_as(&f, None, "wf", &delivered_payload("a", Some(false)));
    commit_as(&f, None, "wf", &gate(2));
    // A loop back opens a new visit with no directive shown.
    commit_as(&f, None, "wf", &transitioned(Some("a"), "b"));
    commit_as(&f, None, "wf", &delivered_payload("b", Some(true)));
    commit_as(&f, None, "wf", &transitioned(Some("b"), "a"));
    commit_as(&f, None, "wf", &gate(3));
    // An older log's record, without `details_included`, counts as shown.
    commit_as(&f, None, "wf", &delivered_payload("a", None));
    commit_as(&f, None, "wf", &gate(4));
    // A rewind opens a visit too.
    commit_as(&f, None, "wf", &rewound("a", "a"));
    commit_as(&f, None, "wf", &gate(5));
    let pre: Vec<bool> = check_records(&f.journal())
        .iter()
        .filter(|r| r["kind"] == "check_evaluated")
        .map(|r| r["koto.check.pre_directive"].as_bool().unwrap())
        .collect();
    assert_eq!(pre, vec![true, false, true, false, true]);
}

#[test]
fn exported_fields_keep_declared_enums_digest_odd_ones_and_drop_free_text() {
    use crate::template::types::FieldSchema;
    let schema = |field_type: &str, values: &[&str]| FieldSchema {
        field_type: field_type.into(),
        required: false,
        values: values.iter().map(|v| v.to_string()).collect(),
        description: String::new(),
        decider: None,
    };
    let accepts: std::collections::BTreeMap<String, FieldSchema> = [
        (
            "status".to_string(),
            schema("enum", &["completed", "needs work"]),
        ),
        ("odd".to_string(), schema("enum", &["needs work"])),
        ("verdict".to_string(), schema("enum", &["pass"])),
        ("ok".to_string(), schema("boolean", &[])),
        ("no".to_string(), schema("boolean", &[])),
        ("note".to_string(), schema("string", &[])),
        ("count".to_string(), schema("number", &[])),
        ("absent".to_string(), schema("enum", &["x"])),
    ]
    .into_iter()
    .collect();
    let submitted: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({
            "status": "completed",
            "odd": "needs work",
            "verdict": "PLANTED-UNDECLARED",
            "ok": true,
            "no": false,
            "note": "PLANTED-TEXT",
            "count": 7,
        }))
        .unwrap();
    let got = exported_fields(&accepts, &submitted).unwrap();
    let want: std::collections::BTreeMap<String, String> = [
        ("status", "completed".to_string()),
        ("odd", name_or_digest("needs work")),
        ("verdict", "other".to_string()),
        ("ok", "true".to_string()),
        ("no", "false".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(got, want);
    assert!(got["odd"].starts_with("sha256:"));
    // Nothing exportable: no map at all.
    let only_text: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({"note": "x"})).unwrap();
    assert_eq!(exported_fields(&accepts, &only_text), None);
}

#[test]
fn evidence_with_exported_fields_relays_one_evidence_field_per_entry() {
    let f = session_in_a();
    let exported: std::collections::BTreeMap<String, String> = [
        ("approved".to_string(), "true".to_string()),
        ("status".to_string(), "other".to_string()),
    ]
    .into_iter()
    .collect();
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::EvidenceSubmitted {
            state: "a".into(),
            fields: serde_json::from_value(serde_json::json!({
                "status": "PLANTED-VALUE",
                "approved": true,
                "note": "PLANTED-NOTE"
            }))
            .unwrap(),
            submitter_cwd: None,
            source: None,
            exported_fields: Some(exported),
        },
    );
    // Evidence without the map relays nothing.
    commit_as(&f, None, "wf", &evidence("a"));
    let j = f.journal();
    let e: Vec<&serde_json::Value> = j.iter().filter(|r| r["kind"] == "evidence_field").collect();
    assert_eq!(e.len(), 2);
    for r in &e {
        let keys: BTreeSet<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = RELAY_FIELDS
            .iter()
            .copied()
            .chain(["koto.state", "koto.evidence.field", "koto.evidence.value"])
            .collect();
        assert_eq!(keys, expected, "{r}");
        assert_eq!(r["koto.seq"], 3);
        assert_eq!(r["koto.visit.seq"], 2);
        assert_eq!(r["koto.state"], "a");
        assert!(r.to_string().len() < MAX_LINE_BYTES);
    }
    assert_eq!(e[0]["koto.evidence.field"], "approved");
    assert_eq!(e[0]["koto.evidence.value"], "true");
    assert_eq!(e[1]["koto.evidence.field"], "status");
    assert_eq!(e[1]["koto.evidence.value"], "other");
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("PLANTED"), "{raw}");
}

// ----- Review records -----

/// The fields each review kind carries beyond [`RELAY_FIELDS`].
fn review_kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "review_round" => &[
            "koto.doc.type",
            "koto.doc.schema",
            "koto.review.round",
            "koto.review.seat_count",
            "koto.review.fallback",
        ],
        "review_verdict" => &[
            "koto.doc.type",
            "koto.doc.schema",
            "koto.review.round",
            "koto.review.seat",
            "koto.rule.id",
            "koto.review.section",
            "koto.review.verdict",
        ],
        "review_decision" => &[
            "koto.doc.type",
            "koto.doc.schema",
            "koto.review.round",
            "koto.review.decision",
        ],
        "doc_checked" => &[
            "koto.doc.type",
            "koto.doc.schema",
            "koto.rule.id",
            "koto.doc.check_outcome",
        ],
        "doc_rule_checked" => &[
            "koto.state",
            "koto.doc.format",
            "koto.rule.id",
            "koto.doc.rule_outcome",
            "koto.doc.check_moment",
        ],
        "doc_section_written" => &[
            "koto.state",
            "koto.doc.format",
            "koto.doc.section",
            "koto.doc.section_write",
        ],
        "doc_fix_applied" => &[
            "koto.state",
            "koto.doc.format",
            "koto.rule.id",
            "koto.doc.section",
            "koto.doc.fix_answers",
            "koto.doc.fix_round",
            "koto.doc.fix_check_rule",
        ],
        other => panic!("not a review kind: {other}"),
    }
}

fn review_verdict(seat: &str, rule_id: &str, section: &str, verdict: &str) -> EventPayload {
    EventPayload::ReviewVerdictRecorded(crate::engine::types::ReviewVerdictRecorded {
        doc_type: Some("prd".into()),
        doc_format: None,
        schema: "prd/v1".into(),
        round: 2,
        seat: seat.into(),
        rule_id: rule_id.into(),
        section: section.into(),
        verdict: verdict.into(),
    })
}

/// Each review event relays one record of its kind holding exactly that
/// kind's fields, each on its own `koto.seq`, and a verdict record differs
/// from its sibling only in its own attributes.
#[test]
fn each_review_event_relays_one_record_with_exactly_its_fields() {
    use crate::engine::types::{DocCheckRecorded, ReviewDecisionRecorded, ReviewRoundStarted};
    let f = session_in_a();
    reset_for_test();
    let payloads = [
        EventPayload::ReviewRoundStarted(ReviewRoundStarted {
            doc_type: Some("prd".into()),
            doc_format: None,
            schema: "prd/v1".into(),
            round: 2,
            seat_count: 3,
            fallback: true,
        }),
        review_verdict("clarity", "rs-001", "goals", "pass"),
        review_verdict("other", "rs-002", "other", "fail"),
        EventPayload::ReviewDecisionRecorded(ReviewDecisionRecorded {
            doc_type: Some("prd".into()),
            doc_format: None,
            schema: "prd/v1".into(),
            round: 2,
            decision: "fix_and_proceed".into(),
        }),
        EventPayload::DocCheckRecorded(DocCheckRecorded {
            doc_type: "prd".into(),
            schema: "absent".into(),
            rule_id: "r-frontmatter".into(),
            outcome: "error".into(),
        }),
    ];
    for p in &payloads {
        commit_as(&f, None, "wf", p);
    }
    assert_eq!(batches_for_test(), (5, 0), "relayed records aren't flushed");
    let j = f.journal();
    let r: Vec<&serde_json::Value> = j
        .iter()
        .filter(|r| {
            let k = r["kind"].as_str().unwrap();
            k.starts_with("review_") || k == "doc_checked"
        })
        .collect();
    let kinds: Vec<&str> = r.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "review_round",
            "review_verdict",
            "review_verdict",
            "review_decision",
            "doc_checked"
        ]
    );
    for rec in &r {
        let kind = rec["kind"].as_str().unwrap();
        let keys: BTreeSet<&str> = rec
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected: BTreeSet<&str> = RELAY_FIELDS
            .iter()
            .chain(review_kind_fields(kind))
            .copied()
            .collect();
        assert_eq!(keys, expected, "{rec}");
        assert_eq!(rec["v"], 1);
    }
    let seqs: Vec<u64> = r.iter().map(|r| r["koto.seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, vec![3, 4, 5, 6, 7], "each event has its own koto.seq");

    assert_eq!(r[0]["koto.doc.type"], "prd");
    assert_eq!(r[0]["koto.doc.schema"], "prd/v1");
    assert_eq!(r[0]["koto.review.round"], 2);
    assert_eq!(r[0]["koto.review.seat_count"], 3);
    assert_eq!(r[0]["koto.review.fallback"], true);
    assert_eq!(r[1]["koto.review.seat"], "clarity");
    assert_eq!(r[1]["koto.rule.id"], "rs-001");
    assert_eq!(r[1]["koto.review.section"], "goals");
    assert_eq!(r[1]["koto.review.verdict"], "pass");
    assert_eq!(r[3]["koto.review.decision"], "fix_and_proceed");
    assert_eq!(r[4]["koto.doc.schema"], "absent");
    assert_eq!(r[4]["koto.rule.id"], "r-frontmatter");
    assert_eq!(r[4]["koto.doc.check_outcome"], "error");

    // The two verdicts differ only in their own attributes (and their
    // sequence number and time).
    let own: BTreeSet<&str> = [
        "at",
        "koto.seq",
        "koto.review.seat",
        "koto.rule.id",
        "koto.review.section",
        "koto.review.verdict",
    ]
    .into_iter()
    .collect();
    for (key, value) in r[1].as_object().unwrap() {
        if !own.contains(key.as_str()) {
            assert_eq!(&r[2][key], value, "{key}");
        }
    }
}

/// A verdict with the longest seat, section and rule id koto stores still
/// fits the journal's line limit.
#[test]
fn a_review_verdict_of_the_longest_values_fits_one_line() {
    let f = session_in_a();
    let long = |c: char| c.to_string().repeat(128);
    commit_as(
        &f,
        None,
        "wf",
        &review_verdict(&long('s'), &long('r'), &long('x'), "fail"),
    );
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    let line = raw
        .lines()
        .find(|l| l.contains("\"review_verdict\""))
        .expect("the verdict was written");
    assert!(line.len() < MAX_LINE_BYTES, "{} bytes", line.len());
    let rec: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(rec["koto.review.seat"], long('s'));
    assert_eq!(rec["koto.rule.id"], long('r'));
    assert_eq!(rec["koto.review.section"], long('x'));
}

/// The drafting events relay one record per entry, every record of one
/// event under that event's `koto.seq`, each holding exactly its kind's
/// fields; a `check_fail` fix carries no round.
#[test]
fn each_drafting_event_relays_one_record_per_entry_under_one_seq() {
    use crate::engine::types::{
        DocFix, DocFixRecorded, DocRuleCheck, DocRuleChecksRecorded, DocSectionRecorded,
    };
    let f = session_in_a();
    let check = |rule: &str, outcome: &str| DocRuleCheck {
        rule_id: rule.into(),
        outcome: outcome.into(),
    };
    let fix = |rule: &str, section: &str, held: &str| DocFix {
        rule_id: rule.into(),
        section: section.into(),
        check_rule: held.into(),
    };
    let payloads = [
        EventPayload::DocRuleChecksRecorded(DocRuleChecksRecorded {
            state: "a".into(),
            doc_format: "vision".into(),
            moment: "pre_jury".into(),
            entries: vec![
                check("doc-required-sections", "fail"),
                check("doc-frontmatter", "not_ready"),
            ],
        }),
        EventPayload::DocSectionRecorded(DocSectionRecorded {
            state: "a".into(),
            doc_format: "vision".into(),
            section: "other".into(),
            write: "rewrite".into(),
        }),
        EventPayload::DocFixRecorded(DocFixRecorded {
            state: "a".into(),
            doc_format: "vision".into(),
            answers: "jury_fail".into(),
            round: Some(2),
            entries: vec![
                fix("rs-001", "goals", "doc-required-sections"),
                fix("rs-002", "other", "none"),
            ],
        }),
        EventPayload::DocFixRecorded(DocFixRecorded {
            state: "a".into(),
            doc_format: "vision".into(),
            answers: "check_fail".into(),
            round: None,
            entries: vec![fix(
                "doc-required-sections",
                "goals",
                "doc-required-sections",
            )],
        }),
    ];
    for p in &payloads {
        commit_as(&f, None, "wf", p);
    }
    let j = f.journal();
    let r: Vec<&serde_json::Value> = j
        .iter()
        .filter(|r| r["kind"].as_str().unwrap().starts_with("doc_"))
        .collect();
    let kinds: Vec<(&str, u64)> = r
        .iter()
        .map(|r| (r["kind"].as_str().unwrap(), r["koto.seq"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("doc_rule_checked", 3),
            ("doc_rule_checked", 3),
            ("doc_section_written", 4),
            ("doc_fix_applied", 5),
            ("doc_fix_applied", 5),
            ("doc_fix_applied", 6),
        ]
    );
    for rec in &r {
        let kind = rec["kind"].as_str().unwrap();
        let keys: BTreeSet<&str> = rec
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected: BTreeSet<&str> = RELAY_FIELDS
            .iter()
            .chain(review_kind_fields(kind))
            .copied()
            .collect();
        if rec["koto.doc.fix_answers"] == "check_fail" {
            expected.remove("koto.doc.fix_round");
        }
        assert_eq!(keys, expected, "{rec}");
        assert_eq!(rec["koto.state"], "a");
        assert_eq!(rec["koto.doc.format"], "vision");
        assert!(rec.get("koto.doc.type").is_none());
    }
    assert_eq!(r[0]["koto.rule.id"], "doc-required-sections");
    assert_eq!(r[0]["koto.doc.rule_outcome"], "fail");
    assert_eq!(r[0]["koto.doc.check_moment"], "pre_jury");
    assert_eq!(r[1]["koto.doc.rule_outcome"], "not_ready");
    assert_eq!(r[2]["koto.doc.section"], "other");
    assert_eq!(r[2]["koto.doc.section_write"], "rewrite");
    assert_eq!(r[3]["koto.rule.id"], "rs-001");
    assert_eq!(r[3]["koto.doc.section"], "goals");
    assert_eq!(r[3]["koto.doc.fix_answers"], "jury_fail");
    assert_eq!(r[3]["koto.doc.fix_round"], 2);
    assert_eq!(r[3]["koto.doc.fix_check_rule"], "doc-required-sections");
    assert_eq!(r[4]["koto.doc.fix_check_rule"], "none");
    assert_eq!(r[5]["koto.doc.fix_answers"], "check_fail");
}

/// With a format, the three review kinds carry `koto.doc.format`, and for a
/// charter format no `koto.doc.type`.
#[test]
fn a_review_round_with_a_format_carries_it_and_a_charter_format_drops_the_type() {
    use crate::engine::types::ReviewRoundStarted;
    let f = session_in_a();
    for (doc_type, doc_format) in [(None, "roadmap"), (Some("plan"), "plan")] {
        commit_as(
            &f,
            None,
            "wf",
            &EventPayload::ReviewRoundStarted(ReviewRoundStarted {
                doc_type: doc_type.map(str::to_string),
                doc_format: Some(doc_format.into()),
                schema: "absent".into(),
                round: 1,
                seat_count: 2,
                fallback: false,
            }),
        );
    }
    let j = f.journal();
    let r: Vec<&serde_json::Value> = j.iter().filter(|r| r["kind"] == "review_round").collect();
    assert_eq!(r[0]["koto.doc.format"], "roadmap");
    assert!(r[0].get("koto.doc.type").is_none());
    assert_eq!(r[1]["koto.doc.format"], "plan");
    assert_eq!(r[1]["koto.doc.type"], "plan");
}

// ----- Code review records -----

/// A code review round relays one `code_review_round` record and then one
/// `code_review_verdict` per seat, in order, all under the event's one
/// `koto.seq`, each holding exactly its kind's fields and none of a
/// document review kind's.
#[test]
fn a_code_review_round_relays_one_round_and_one_verdict_per_seat_under_one_seq() {
    use crate::engine::types::{CodeReviewRoundRecorded, CodeReviewSeat};
    let f = session_in_a();
    let head = digest("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    let seat = |seat: &str, model: &str, verdict: &str| CodeReviewSeat {
        seat: seat.into(),
        model: model.into(),
        verdict: verdict.into(),
    };
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::CodeReviewRoundRecorded(CodeReviewRoundRecorded {
            state: "a".into(),
            round: 2,
            head: head.clone(),
            entries: vec![
                seat("correctness", "claude-opus-4-5", "pass"),
                seat("security", "gpt-5.1", "fail"),
                seat("other", "claude-sonnet-4-5@20250929", "pass"),
            ],
        }),
    );
    let j = f.journal();
    let r: Vec<&serde_json::Value> = j
        .iter()
        .filter(|r| r["kind"].as_str().unwrap().starts_with("code_review_"))
        .collect();
    let kinds: Vec<(&str, u64)> = r
        .iter()
        .map(|r| (r["kind"].as_str().unwrap(), r["koto.seq"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("code_review_round", 3),
            ("code_review_verdict", 3),
            ("code_review_verdict", 3),
            ("code_review_verdict", 3),
        ]
    );
    for rec in &r {
        let keys: BTreeSet<&str> = rec
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let own: &[&str] = match rec["kind"].as_str().unwrap() {
            "code_review_round" => &[
                "koto.state",
                "koto.code_review.round",
                "koto.code_review.seat_count",
                "koto.code_review.head",
            ],
            _ => &[
                "koto.state",
                "koto.code_review.round",
                "koto.code_review.seat",
                "koto.code_review.model",
                "koto.code_review.verdict",
            ],
        };
        let expected: BTreeSet<&str> = RELAY_FIELDS.iter().chain(own).copied().collect();
        assert_eq!(keys, expected, "{rec}");
        assert_eq!(rec["v"], 1);
        assert_eq!(rec["koto.state"], "a");
        assert_eq!(rec["koto.code_review.round"], 2);
        assert_eq!(rec["koto.visit.seq"], 2);
    }
    assert_eq!(r[0]["koto.code_review.seat_count"], 3);
    assert_eq!(r[0]["koto.code_review.head"], head.as_str());
    let seats: Vec<(&str, &str, &str)> = r[1..]
        .iter()
        .map(|r| {
            (
                r["koto.code_review.seat"].as_str().unwrap(),
                r["koto.code_review.model"].as_str().unwrap(),
                r["koto.code_review.verdict"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seats,
        vec![
            ("correctness", "claude-opus-4-5", "pass"),
            ("security", "gpt-5.1", "fail"),
            ("other", "claude-sonnet-4-5@20250929", "pass"),
        ]
    );
    // No document review record was written for the round.
    assert!(!j.iter().any(|r| {
        let k = r["kind"].as_str().unwrap();
        k.starts_with("review_") || k.starts_with("doc_")
    }));
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    assert!(!raw.contains("4b825dc642cb6eb9a060e54bf8d69288fbee4904"));
}

/// A round of the most seats, each with the longest seat and model koto
/// stores, keeps every record within the journal's line limit.
#[test]
fn a_code_review_round_of_the_longest_values_fits_one_line_per_record() {
    use crate::engine::types::{CodeReviewRoundRecorded, CodeReviewSeat};
    let f = session_in_a();
    let long = |c: char| c.to_string().repeat(128);
    commit_as(
        &f,
        None,
        "wf",
        &EventPayload::CodeReviewRoundRecorded(CodeReviewRoundRecorded {
            state: long('s'),
            round: u64::MAX,
            head: digest("h"),
            entries: vec![
                CodeReviewSeat {
                    seat: long('x'),
                    model: long('m'),
                    verdict: "fail".into(),
                };
                200
            ],
        }),
    );
    let raw = std::fs::read_to_string(f.home.join(JOURNAL_FILE)).unwrap();
    let lines: Vec<&str> = raw
        .lines()
        .filter(|l| l.contains("\"code_review_"))
        .collect();
    assert_eq!(lines.len(), 201);
    for line in lines {
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes", line.len());
    }
}
