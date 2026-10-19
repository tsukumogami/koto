//! Unit tests for the store: the writer, the readers and the report.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::reader::{self, Filter, Status};
use super::record::*;
use super::report;
use super::writer::{assemble_line, open_store_file, repair_torn_tail};
use super::*;

fn store_at(dir: &Path) -> Store {
    Store::new(dir.join("decisions"), crate::redact::Redactor::empty())
}

/// An id for `time`'s month with a recognizable random part.
fn id_for(time: &str, n: u64) -> String {
    let m = time_month(time).unwrap().replace('-', "");
    format!("d-{}-{:032x}", m, n)
}

fn decision(kind: Kind, time: &str, n: u64) -> Decision {
    let (outcome, subject, evidence) = match kind {
        Kind::Judgment => (
            "settled",
            Subject {
                what: Some(format!("call {}", n)),
                ..Default::default()
            },
            vec![],
        ),
        Kind::Accept => (
            "accepted",
            Subject {
                of: Some("document".into()),
                repo: Some("o/r".into()),
                path: Some("notes/a.md".into()),
                commit: Some("c1".into()),
                ..Default::default()
            },
            vec![],
        ),
        Kind::GatePass => (
            "passed",
            Subject {
                workflow: Some(format!("wf-{}", n % 3)),
                state: Some("review".into()),
                gate: Some("panel".into()),
                ..Default::default()
            },
            vec![],
        ),
        Kind::Verdict => (
            "pass",
            Subject {
                what: Some("review".into()),
                ..Default::default()
            },
            vec!["journal:wf#1".to_string()],
        ),
        Kind::Refused => (
            "refused",
            Subject {
                attempted_kind: Some("reverse".into()),
                refusal_reason: Some("not on the record".into()),
                ..Default::default()
            },
            vec![],
        ),
        other => panic!("no fixture for {}", other),
    };
    Decision {
        v: DECISION_FORMAT_VERSION,
        id: id_for(time, n),
        time: time.to_string(),
        kind,
        outcome: outcome.to_string(),
        subject,
        decided_by: DecidedBy::session(vec![Sourced::observed("sess-a")], vec![], false),
        authority: Authority::for_decider(DeciderKind::Session),
        chain: Chain::unknown(),
        basis: vec![],
        evidence,
        reason: None,
        reverses: None,
        original: None,
        repaired_from: None,
        import_source: None,
        recorded_at: None,
    }
}

fn with_decider(mut d: Decision, kind: DeciderKind, defaulted: bool) -> Decision {
    d.decided_by.kind = kind;
    d.decided_by.defaulted = defaulted;
    d.authority = Authority::for_decider(kind);
    d
}

fn reversal_of(original: &Decision, time: &str, n: u64) -> Decision {
    Decision {
        v: DECISION_FORMAT_VERSION,
        id: id_for(time, n),
        time: time.to_string(),
        kind: Kind::Reverse,
        outcome: "reversed".into(),
        subject: Subject {
            decision: Some(original.id.clone()),
            ..Default::default()
        },
        decided_by: DecidedBy::session(vec![Sourced::observed("sess-b")], vec![], false),
        authority: Authority::for_decider(DeciderKind::Session),
        chain: Chain::unknown(),
        basis: vec![],
        evidence: vec![],
        reason: Some("overruled".into()),
        reverses: Some(original.id.clone()),
        original: Some(Original {
            kind: original.kind,
            decider_kind: original.decided_by.kind,
            session: original.decided_by.session.clone(),
            subject_key: None,
        }),
        repaired_from: None,
        import_source: None,
        recorded_at: None,
    }
}

/// Write fixture lines straight into the files they belong in.
fn write_fixture(root: &Path, decisions: &[Decision]) {
    std::fs::create_dir_all(root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for d in decisions {
        let path = writer::target_path(root, d).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(f, "{}", assemble_line(d).unwrap()).unwrap();
    }
}

fn append(store: &Store, d: Decision) -> WriteOutcome {
    store.append(d, |_| Ok(Check::Append)).unwrap()
}

fn month_path(store: &Store, time: &str) -> PathBuf {
    store
        .root()
        .join(format!("{}.jsonl", time_month(time).unwrap()))
}

// ===== Writer =====

const T1: &str = "2026-10-05T10:00:00Z";

#[test]
fn a_decision_goes_to_the_month_of_its_own_time() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    append(&s, decision(Kind::Judgment, "2026-09-30T23:59:59Z", 1));
    append(&s, decision(Kind::Judgment, "2026-10-01T00:00:00Z", 2));
    let sep = std::fs::read_to_string(s.root().join("2026-09.jsonl")).unwrap();
    let oct = std::fs::read_to_string(s.root().join("2026-10.jsonl")).unwrap();
    assert!(sep.contains(&id_for("2026-09-01T00:00:00Z", 1)));
    assert!(oct.contains(&id_for("2026-10-01T00:00:00Z", 2)));
    assert_eq!(sep.lines().count(), 1);
    assert_eq!(oct.lines().count(), 1);
}

#[test]
fn an_id_naming_another_month_is_refused() {
    let mut d = decision(Kind::Judgment, T1, 1);
    d.id = id_for("2026-09-01T00:00:00Z", 1);
    assert_eq!(assemble_line(&d).unwrap_err().field, "id");
}

#[test]
fn a_complete_unterminated_tail_gets_its_newline() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    let first = decision(Kind::Judgment, T1, 1);
    writer::ensure_dir(s.root()).unwrap();
    let path = month_path(&s, T1);
    std::fs::write(&path, assemble_line(&first).unwrap()).unwrap();

    let mut f = open_store_file(&path).unwrap();
    assert!(repair_torn_tail(&mut f, &path).unwrap());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.ends_with('\n'));
    assert_eq!(content.lines().count(), 1);

    append(&s, decision(Kind::Judgment, T1, 2));
    let read = reader::read_file(&path, &reader::Needles::none()).unwrap();
    assert_eq!(read.decisions.len(), 2);
    assert_eq!(read.skipped, 0);
}

#[test]
fn a_torn_tail_that_does_not_parse_is_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    append(&s, decision(Kind::Judgment, T1, 1));
    let path = month_path(&s, T1);
    let good = std::fs::read(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"v\":1,\"id\":\"d-2026")
        .unwrap();

    // Readers skip the torn tail before the repair.
    let read = reader::read_file(&path, &reader::Needles::none()).unwrap();
    assert_eq!((read.decisions.len(), read.skipped), (1, 1));

    let mut f = open_store_file(&path).unwrap();
    assert!(repair_torn_tail(&mut f, &path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), good);
    assert!(!repair_torn_tail(&mut f, &path).unwrap());
}

#[test]
fn an_identical_line_is_skipped_and_a_different_one_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    let d = decision(Kind::Judgment, T1, 7);
    assert!(matches!(
        append(&s, d.clone()),
        WriteOutcome::Appended { .. }
    ));
    let before = std::fs::read(month_path(&s, T1)).unwrap();
    assert!(matches!(
        append(&s, d.clone()),
        WriteOutcome::Skipped { .. }
    ));

    let mut changed = d;
    changed.reason = Some("different".into());
    let err = s.append(changed, |_| Ok(Check::Append)).unwrap_err();
    assert!(matches!(err, StoreError::IdConflict { .. }));
    assert_eq!(std::fs::read(month_path(&s, T1)).unwrap(), before);
}

#[test]
fn a_refusing_check_appends_nothing_or_only_its_refused_line() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    append(&s, decision(Kind::Judgment, T1, 1));
    let path = month_path(&s, T1);
    let before = std::fs::read(&path).unwrap();

    let out = s
        .append(decision(Kind::Judgment, T1, 2), |_| Ok(Check::Refuse(None)))
        .unwrap();
    assert_eq!(out, WriteOutcome::Refused { refused_id: None });
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let refused = decision(Kind::Refused, T1, 3);
    let refused_id = refused.id.clone();
    let out = s
        .append(decision(Kind::Judgment, T1, 4), move |_| {
            Ok(Check::Refuse(Some(Box::new(refused))))
        })
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome::Refused {
            refused_id: Some(refused_id.clone())
        }
    );
    let read = reader::read_file(&path, &reader::Needles::none()).unwrap();
    let ids: Vec<&str> = read.decisions.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec![id_for(T1, 1).as_str(), refused_id.as_str()]);
}

#[test]
fn a_skipping_check_leaves_the_file_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    append(&s, decision(Kind::Judgment, T1, 1));
    let path = month_path(&s, T1);
    let before = std::fs::read(&path).unwrap();
    let out = s
        .append(decision(Kind::Judgment, T1, 2), |locked| {
            // The closure reads the record under the lock.
            assert!(locked.find(&id_for(T1, 1)).unwrap().is_some());
            Ok(Check::Skip)
        })
        .unwrap();
    assert!(matches!(out, WriteOutcome::Skipped { .. }));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn oversized_lines_and_too_many_entries_are_refused_whole() {
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());

    let mut big = decision(Kind::Judgment, T1, 1);
    big.reason = Some("x".repeat(MAX_LINE_BYTES));
    let err = s.append(big, |_| Ok(Check::Append)).unwrap_err();
    assert_eq!(err.field(), Some("decision"));

    let mut many = decision(Kind::Judgment, T1, 2);
    many.evidence = (0..=MAX_EVIDENCE).map(|i| format!("e{}", i)).collect();
    assert_eq!(
        s.append(many, |_| Ok(Check::Append)).unwrap_err().field(),
        Some("evidence")
    );

    let mut many = decision(Kind::Judgment, T1, 3);
    many.basis = (0..=MAX_BASIS).map(|i| format!("b{}", i)).collect();
    assert_eq!(
        s.append(many, |_| Ok(Check::Append)).unwrap_err().field(),
        Some("basis")
    );

    // Nothing was written, not even the directory's month file.
    assert!(!month_path(&s, T1).exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_at_any_store_file_makes_the_write_fail() {
    for name in ["2026-10.jsonl", REVERSALS_FILE, IMPORTS_FILE, LOCK_FILE] {
        let dir = tempfile::tempdir().unwrap();
        let s = store_at(dir.path());
        writer::ensure_dir(s.root()).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "").unwrap();
        let link = s.root().join(name);
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        assert!(open_store_file(&link).is_err(), "{} was followed", name);
        if name != IMPORTS_FILE {
            let original = decision(Kind::Judgment, T1, 1);
            let d = if name == REVERSALS_FILE {
                reversal_of(&original, T1, 2)
            } else {
                original
            };
            assert!(
                s.append(d, |_| Ok(Check::Append)).is_err(),
                "a write through {} succeeded",
                name
            );
        }
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "");
    }
}

#[cfg(unix)]
#[test]
fn the_directory_is_created_private_and_an_unsafe_one_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let s = store_at(dir.path());
    append(&s, decision(Kind::Judgment, T1, 1));
    let mode = std::fs::metadata(s.root()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let file_mode = std::fs::metadata(month_path(&s, T1))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);

    std::fs::set_permissions(s.root(), std::fs::Permissions::from_mode(0o720)).unwrap();
    assert!(matches!(
        s.append(decision(Kind::Judgment, T1, 2), |_| Ok(Check::Append)),
        Err(StoreError::UnsafeDir { .. })
    ));
    assert!(reader::list(s.root(), &Filter::default()).is_err());

    // A symlinked store directory is refused too.
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    let linked = dir.path().join("linked");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    assert!(matches!(
        writer::check_dir(&linked),
        Err(StoreError::UnsafeDir { .. })
    ));
}

#[test]
fn the_store_follows_home_not_the_sessions_base() {
    let root = default_root().unwrap();
    assert!(root.ends_with(".koto/decisions"));
    assert_eq!(
        root.parent().unwrap(),
        dirs::home_dir().unwrap().join(".koto")
    );
}

#[test]
fn reasons_links_handles_and_evidence_are_redacted() {
    let secret = "s3cr3t-value-0123456789";
    let red = crate::redact::Redactor::new([("GH_TOKEN".to_string(), secret.to_string())]);
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(dir.path().join("decisions"), red);
    let mut d = decision(Kind::Judgment, T1, 1);
    d.reason = Some(format!("why {}", secret));
    d.evidence = vec![format!("ptr:{}", secret)];
    d.decided_by.link = Some(format!("https://example.com/{}", secret));
    d.decided_by.handle = vec![Sourced::handed(format!("@{}", secret))];
    append(&s, d);
    let content = std::fs::read_to_string(month_path(&s, T1)).unwrap();
    assert!(!content.contains(secret));
    assert_eq!(content.matches("GH_TOKEN").count(), 4, "{}", content);
}

#[test]
fn link_validation() {
    assert!(validate_link("link", "https://example.com/a#b").is_ok());
    for bad in [
        "http://example.com/",
        "https://a:b@example.com/",
        "https://a@example.com/",
        "https://example.com/?q=1",
        "/relative",
        "ftp://example.com/",
    ] {
        assert!(validate_link("link", bad).is_err(), "{} passed", bad);
    }
}

#[test]
fn time_normalization() {
    assert_eq!(normalize_time("2026-10").unwrap(), "2026-10-01T00:00:00Z");
    assert_eq!(
        normalize_time("2026-10-05").unwrap(),
        "2026-10-05T00:00:00Z"
    );
    assert_eq!(
        normalize_time("2026-10-05T01:02:03.456Z").unwrap(),
        "2026-10-05T01:02:03Z"
    );
    assert!(normalize_time("2026-13").is_none());
    assert!(normalize_time("yesterday").is_none());
    assert_eq!(now_to_second().len(), "2026-10-05T01:02:03Z".len());
    assert!(id_month(&new_id(T1).unwrap()).is_some());
}

// ===== Readers =====

/// A fixture of known decisions across two months.
fn fixture() -> Vec<Decision> {
    vec![
        decision(Kind::Judgment, "2026-09-10T00:00:00Z", 1),
        with_decider(
            decision(Kind::GatePass, "2026-09-20T00:00:00Z", 2),
            DeciderKind::Session,
            true,
        ),
        with_decider(
            decision(Kind::Accept, "2026-10-01T00:00:00Z", 3),
            DeciderKind::Person,
            false,
        ),
        with_decider(
            decision(Kind::GatePass, "2026-10-02T00:00:00Z", 4),
            DeciderKind::Session,
            true,
        ),
        with_decider(
            decision(Kind::GatePass, "2026-10-02T00:00:00Z", 5),
            DeciderKind::Check,
            false,
        ),
        with_decider(
            decision(Kind::Verdict, "2026-10-03T00:00:00Z", 6),
            DeciderKind::Check,
            false,
        ),
        decision(Kind::Judgment, "2026-10-31T23:59:59Z", 7),
        decision(Kind::Refused, "2026-10-15T00:00:00Z", 8),
        decision(Kind::Judgment, "2026-11-01T00:00:00Z", 9),
    ]
}

fn ids(listing: &reader::Listing) -> Vec<String> {
    listing
        .items
        .iter()
        .map(|i| i.decision.id.clone())
        .collect()
}

fn expect_ids(all: &[Decision], ns: &[u64]) -> Vec<String> {
    ns.iter()
        .map(|n| {
            all.iter()
                .find(|d| d.id.ends_with(&format!("{:032x}", n)))
                .unwrap()
                .id
                .clone()
        })
        .collect()
}

#[test]
fn filters_return_exactly_the_matching_decisions_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    let all = fixture();
    write_fixture(&root, &all);

    let list = |f: Filter| reader::list(&root, &f).unwrap();

    assert_eq!(
        ids(&list(Filter {
            kind: Some(Kind::GatePass),
            ..Default::default()
        })),
        expect_ids(&all, &[2, 4, 5])
    );
    assert_eq!(
        ids(&list(Filter {
            decider_kind: Some(DeciderKind::Check),
            ..Default::default()
        })),
        expect_ids(&all, &[5, 6])
    );
    assert_eq!(
        ids(&list(Filter {
            subject: vec![("workflow".into(), "wf-1".into())],
            ..Default::default()
        })),
        expect_ids(&all, &[4])
    );
    assert_eq!(
        ids(&list(Filter {
            defaulted: Some(true),
            ..Default::default()
        })),
        expect_ids(&all, &[2, 4])
    );
    // The window includes its start and excludes its end.
    let october = list(Filter {
        since: Some("2026-10-01T00:00:00Z".into()),
        until: Some("2026-11-01T00:00:00Z".into()),
        ..Default::default()
    });
    assert_eq!(ids(&october), expect_ids(&all, &[3, 4, 5, 6, 8, 7]));
    // Ties on time break by identifier.
    assert!(october.items[1].decision.id < october.items[2].decision.id);
    assert!(october.items.iter().all(|i| i.status == Status::InEffect));
}

#[test]
fn an_unwindowed_filter_parses_only_lines_holding_its_literal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    let all = fixture();
    write_fixture(&root, &all);
    // A line that doesn't parse, and doesn't hold the literal, is never
    // parsed, so it isn't counted as skipped.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join("2026-09.jsonl"))
        .unwrap();
    writeln!(f, "not json at all").unwrap();
    let narrow = reader::list(
        &root,
        &Filter {
            kind: Some(Kind::Verdict),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(narrow.count, 1);
    assert_eq!(narrow.skipped_lines, 0);
    let everything = reader::list(&root, &Filter::default()).unwrap();
    assert_eq!(everything.skipped_lines, 1);
    assert_eq!(everything.count, all.len());
}

#[test]
fn reversed_decisions_read_as_reversed_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    let all = fixture();
    let rev = reversal_of(&all[2], "2026-10-20T00:00:00Z", 100);
    let mut lines = all.clone();
    lines.push(rev.clone());
    write_fixture(&root, &lines);

    let listing = reader::list(
        &root,
        &Filter {
            kind: Some(Kind::Accept),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(listing.items[0].status, Status::Reversed);
    assert_eq!(
        listing.items[0].reversed_by.as_deref(),
        Some(rev.id.as_str())
    );

    let shown = reader::show(&root, &all[2].id).unwrap().unwrap();
    assert_eq!(shown.listed.status, Status::Reversed);
    assert_eq!(shown.reversal.unwrap().id, rev.id);

    // The reversal itself shows from the reversal log.
    let shown = reader::show(&root, &rev.id).unwrap().unwrap();
    assert_eq!(shown.listed.decision.kind, Kind::Reverse);
}

#[cfg(unix)]
#[test]
fn show_reads_only_the_named_month_and_the_reversal_log() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    let all = fixture();
    write_fixture(&root, &all);
    // Make every other month unreadable: show must not open them.
    for m in ["2026-09.jsonl", "2026-11.jsonl"] {
        std::fs::set_permissions(root.join(m), std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let shown = reader::show(&root, &all[3].id).unwrap();
    for m in ["2026-09.jsonl", "2026-11.jsonl"] {
        std::fs::set_permissions(root.join(m), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(shown.unwrap().listed.decision.id, all[3].id);
}

// ===== Report =====

#[test]
fn the_report_tables_a_month_against_a_two_month_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    let all = fixture();
    // all[0] (September judgment) is reversed in October; all[3] (October
    // defaulted gate pass) is reversed in October; all[2] (October accept)
    // is reversed in November.
    let r_sep = reversal_of(&all[0], "2026-10-10T00:00:00Z", 101);
    let r_oct = reversal_of(&all[3], "2026-10-12T00:00:00Z", 102);
    let r_nov = reversal_of(&all[2], "2026-11-02T00:00:00Z", 103);
    let mut lines = all.clone();
    lines.extend([r_sep.clone(), r_oct.clone(), r_nov.clone()]);
    write_fixture(&root, &lines);

    let r = report::report(&root, "2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z").unwrap();
    // October decisions: accept(person), gate-pass(session, defaulted),
    // gate-pass(check), verdict(check), judgment(session). The refused line
    // and November's judgment don't count.
    assert_eq!(r.decisions.total, 5);
    assert_eq!(r.decisions.reversed, 2);
    let cell = |k: Kind, dk: DeciderKind| {
        r.decisions
            .table
            .iter()
            .find(|c| c.kind == k && c.decider_kind == dk)
            .map(|c| (c.count, c.reversed))
    };
    assert_eq!(cell(Kind::Accept, DeciderKind::Person), Some((1, 1)));
    assert_eq!(cell(Kind::GatePass, DeciderKind::Session), Some((1, 1)));
    assert_eq!(cell(Kind::GatePass, DeciderKind::Check), Some((1, 0)));
    assert_eq!(cell(Kind::Verdict, DeciderKind::Check), Some((1, 0)));
    assert_eq!(cell(Kind::Judgment, DeciderKind::Session), Some((1, 0)));
    assert_eq!(cell(Kind::Refused, DeciderKind::Session), None);
    assert_eq!(r.decisions.table.len(), 5);

    // Reversals recorded in October: the September judgment's and the
    // October gate pass's, each in its original's cell.
    assert_eq!(r.reversals.total, 2);
    let rcell = |k: Kind, dk: DeciderKind| {
        r.reversals
            .table
            .iter()
            .find(|c| c.kind == k && c.decider_kind == dk)
            .map(|c| c.count)
    };
    assert_eq!(rcell(Kind::Judgment, DeciderKind::Session), Some(1));
    assert_eq!(rcell(Kind::GatePass, DeciderKind::Session), Some(1));
    assert_eq!(r.reversals.table.len(), 2);

    // Each reversed decision with its reversal: the two October decisions
    // reversed (one in November), and September's reversed in October.
    let pairs: Vec<(&str, &str)> = r
        .reversed
        .iter()
        .map(|p| (p.decision.id.as_str(), p.reversal.id.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            (all[0].id.as_str(), r_sep.id.as_str()),
            (all[2].id.as_str(), r_nov.id.as_str()),
            (all[3].id.as_str(), r_oct.id.as_str()),
        ]
    );
    assert_eq!(r.gate_pass_defaulted, 1);

    // The end is exclusive: a window ending at all[6]'s second leaves it out.
    let r = report::report(&root, "2026-10-01T00:00:00Z", "2026-10-31T23:59:59Z").unwrap();
    assert_eq!(r.decisions.total, 4);
    let r = report::report(&root, "2026-10-31T23:59:59Z", "2026-11-01T00:00:00Z").unwrap();
    assert_eq!(r.decisions.total, 1);
}

// ===== Performance =====

/// 100,000 decisions over twelve months: a one-month listing and a
/// one-month report each finish in under five seconds.
#[test]
fn a_month_of_a_hundred_thousand_decisions_lists_and_reports_in_time() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("decisions");
    std::fs::create_dir_all(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let kinds = [Kind::Judgment, Kind::GatePass, Kind::Accept, Kind::Verdict];
    let per_month = 100_000 / 12 + 1;
    let mut reversals = String::new();
    for month in 1..=12u32 {
        let mut buf = String::with_capacity(per_month * 900);
        for i in 0..per_month {
            let n = (month as u64) << 32 | i as u64;
            let time = format!(
                "2025-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                month,
                1 + (i % 28),
                (i / 3600) % 24,
                (i / 60) % 60,
                i % 60
            );
            let mut d = decision(kinds[i % kinds.len()], &time, n);
            d.reason = Some("a reason of ordinary length for a decision line".repeat(4));
            d.decided_by.defaulted = i % 2 == 0;
            buf.push_str(&serde_json::to_string(&d).unwrap());
            buf.push('\n');
            if i % 20 == 0 {
                let r = reversal_of(&d, &time, n | 1 << 63);
                reversals.push_str(&serde_json::to_string(&r).unwrap());
                reversals.push('\n');
            }
        }
        std::fs::write(root.join(format!("2025-{:02}.jsonl", month)), buf).unwrap();
    }
    std::fs::write(root.join(REVERSALS_FILE), reversals).unwrap();

    let started = std::time::Instant::now();
    let listing = reader::list(
        &root,
        &Filter {
            since: Some("2025-06-01T00:00:00Z".into()),
            until: Some("2025-07-01T00:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let listed_in = started.elapsed();
    let june_reversals = (0..per_month).filter(|i| i % 20 == 0).count();
    assert_eq!(listing.count, per_month + june_reversals);

    let started = std::time::Instant::now();
    let r = report::report(&root, "2025-06-01T00:00:00Z", "2025-07-01T00:00:00Z").unwrap();
    let reported_in = started.elapsed();
    assert_eq!(r.decisions.total, per_month);

    assert!(
        listed_in.as_secs_f64() < 5.0,
        "one-month listing took {:?}",
        listed_in
    );
    assert!(
        reported_in.as_secs_f64() < 5.0,
        "one-month report took {:?}",
        reported_in
    );
}

// ===== No network =====

/// The store and its readers name no network client: recording and reading
/// never make a network call.
#[test]
fn the_store_has_no_network_path() {
    let sources = [
        include_str!("mod.rs"),
        include_str!("record.rs"),
        include_str!("writer.rs"),
        include_str!("reader.rs"),
        include_str!("report.rs"),
        include_str!("../cli/decisions.rs"),
    ];
    for src in sources {
        for client in ["attohttpc", "TcpStream", "UdpSocket", "s3::", "reqwest"] {
            assert!(!src.contains(client), "the store references {}", client);
        }
    }
}
