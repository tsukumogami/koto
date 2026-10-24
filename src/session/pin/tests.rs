use super::*;
use crate::session::hierarchy::{display_text, pin_fields, pin_text, SessionFacts};
use crate::session::links::LINKS_PREFIX;

fn pin(reason: &str) -> Pin {
    Pin {
        v: RECORD_VERSION,
        id: "0123456789abcdef0123456789abcdef".to_string(),
        reason: reason.to_string(),
        set_by: None,
        set_at: "2026-10-24T00:00:00.000Z".to_string(),
    }
}

/// A session directory holding a header line, with `ctx/` and a manifest
/// listing `listed`.
fn session(base: &Path, name: &str, listed: &[&str]) -> std::path::PathBuf {
    let dir = base.join(name);
    fs::create_dir_all(dir.join("ctx/koto/links")).unwrap();
    let header = serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "h",
        "created_at": "2026-10-24T00:00:00.000Z",
        "session_id": format!("id-{name}"),
    });
    fs::write(dir.join(state_file_name(name)), format!("{header}\n")).unwrap();
    let mut keys = serde_json::Map::new();
    for key in listed {
        keys.insert(
            key.to_string(),
            serde_json::json!({"created_at": "2026-10-24T00:00:00.000Z", "size": 1, "hash": "h"}),
        );
    }
    fs::write(
        dir.join("ctx/manifest.json"),
        serde_json::to_vec(&serde_json::json!({ "keys": keys })).unwrap(),
    )
    .unwrap();
    dir
}

#[test]
fn reasons_are_checked_by_scalar_count_blankness_and_controls() {
    assert!(validate_reason("demo").is_ok());
    assert!(validate_reason(&"é".repeat(MAX_REASON)).is_ok());
    assert!(validate_reason(&"é".repeat(MAX_REASON + 1)).is_err());
    for bad in ["", " ", "\u{3000}", "a\tb", "a\nb", "a\u{7}b", "a\u{85}b"] {
        assert!(validate_reason(bad).is_err(), "{bad:?}");
    }
    // Format characters aren't Cc: allowed, and escaped when printed.
    assert!(validate_reason("a\u{202E}b").is_ok());
}

#[test]
fn the_pin_key_fails_the_link_key_grammar_older_builds_read() {
    // koto v0.23.0 reads a key under koto/links/ as <kind>/<peer>; one with
    // no peer is a problem that keeps the session, never an edge.
    let rest = PIN_KEY.strip_prefix(LINKS_PREFIX).unwrap();
    assert!(rest.split_once('/').is_none());
    assert!(crate::session::import_links::validate_link(PIN_KEY, b"{}").is_err());
    // The release record sits outside the links area altogether.
    assert!(!RELEASE_KEY.starts_with(LINKS_PREFIX));
}

#[test]
fn pin_keys_are_the_key_and_what_lies_under_it() {
    assert!(is_pin_key("koto/links/pin"));
    assert!(is_pin_key("koto/links/pin/x"));
    assert!(!is_pin_key("koto/links/pin-old"));
    assert!(!is_pin_key("koto/links/pinned/x"));
    assert!(!is_pin_key("koto/pin-release"));
}

#[test]
fn every_shape_of_pin_file_reads_as_its_fact() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path();
    let good = serde_json::to_vec(&pin("demo")).unwrap();

    // No pin.
    let dir = session(base, "none", &[]);
    assert_eq!(read_at(&dir, false), PinFact::None);

    // A pin, listed or not (R19): readable either way.
    let dir = session(base, "listed", &[PIN_KEY]);
    fs::write(dir.join("ctx").join(PIN_KEY), &good).unwrap();
    assert_eq!(read_local(base, "listed"), PinFact::Present(pin("demo")));
    let dir = session(base, "bare", &[]);
    fs::write(dir.join("ctx").join(PIN_KEY), &good).unwrap();
    assert_eq!(read_local(base, "bare"), PinFact::Present(pin("demo")));

    // An empty unlisted file is a lock file, not a pin; listed, it is one
    // that can't be parsed.
    let dir = session(base, "empty", &[]);
    fs::write(dir.join("ctx").join(PIN_KEY), b"").unwrap();
    assert_eq!(read_local(base, "empty"), PinFact::None);
    let dir = session(base, "empty-listed", &[PIN_KEY]);
    fs::write(dir.join("ctx").join(PIN_KEY), b"").unwrap();
    assert_eq!(
        read_local(base, "empty-listed"),
        PinFact::Unreadable(PinProblem::Unparseable)
    );

    // Listed with nothing there.
    session(base, "missing", &[PIN_KEY]);
    assert_eq!(
        read_local(base, "missing"),
        PinFact::Unreadable(PinProblem::MissingContent)
    );

    // Too large, malformed, an unknown version.
    let dir = session(base, "large", &[PIN_KEY]);
    fs::write(dir.join("ctx").join(PIN_KEY), vec![b' '; 5 * 1024]).unwrap();
    assert_eq!(
        read_local(base, "large"),
        PinFact::Unreadable(PinProblem::TooLarge)
    );
    let dir = session(base, "bad", &[PIN_KEY]);
    fs::write(dir.join("ctx").join(PIN_KEY), b"not json").unwrap();
    assert_eq!(
        read_local(base, "bad"),
        PinFact::Unreadable(PinProblem::Unparseable)
    );
    let dir = session(base, "v2", &[PIN_KEY]);
    let mut future = serde_json::to_value(pin("demo")).unwrap();
    future["v"] = serde_json::json!(2);
    fs::write(
        dir.join("ctx").join(PIN_KEY),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    assert_eq!(
        read_local(base, "v2"),
        PinFact::Unreadable(PinProblem::Unparseable)
    );

    // A symlink and a directory are not files.
    let dir = session(base, "link", &[PIN_KEY]);
    let target = base.join("elsewhere");
    fs::write(&target, &good).unwrap();
    std::os::unix::fs::symlink(&target, dir.join("ctx").join(PIN_KEY)).unwrap();
    assert_eq!(
        read_local(base, "link"),
        PinFact::Unreadable(PinProblem::NotAFile)
    );
    let dir = session(base, "dir", &["koto/links/pin/x"]);
    fs::create_dir_all(dir.join("ctx").join(PIN_KEY)).unwrap();
    fs::write(dir.join("ctx").join(PIN_KEY).join("x"), &good).unwrap();
    assert_eq!(
        read_local(base, "dir"),
        PinFact::Unreadable(PinProblem::NotAFile)
    );
}

#[test]
fn the_scan_reads_a_pin_as_a_fact_and_never_as_a_link_or_a_problem() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path();
    let dir = session(base, "p", &[PIN_KEY]);
    fs::write(
        dir.join("ctx").join(PIN_KEY),
        serde_json::to_vec(&pin("demo")).unwrap(),
    )
    .unwrap();
    let facts = SessionFacts::read(base, "p");
    assert_eq!(facts.pin, PinFact::Present(pin("demo")));
    assert!(facts.links.is_empty(), "{:?}", facts.links);
    assert!(facts.problems.is_empty(), "{:?}", facts.problems);

    // An unreadable pin (a directory, with an entry under it listed) is
    // still only a fact of its session: no link, no problem.
    let dir = session(base, "q", &["koto/links/pin/peer"]);
    fs::create_dir_all(dir.join("ctx").join(PIN_KEY)).unwrap();
    fs::write(dir.join("ctx").join(PIN_KEY).join("peer"), b"{}").unwrap();
    let facts = SessionFacts::read(base, "q");
    assert_eq!(facts.pin, PinFact::Unreadable(PinProblem::NotAFile));
    assert!(facts.links.is_empty(), "{:?}", facts.links);
    assert!(facts.problems.is_empty(), "{:?}", facts.problems);

    // A symlink at the pin's path: the same.
    let dir = session(base, "s", &[PIN_KEY]);
    std::os::unix::fs::symlink(base.join("nowhere"), dir.join("ctx").join(PIN_KEY)).unwrap();
    let facts = SessionFacts::read(base, "s");
    assert_eq!(facts.pin, PinFact::Unreadable(PinProblem::NotAFile));
    assert!(facts.problems.is_empty(), "{:?}", facts.problems);
}

#[test]
fn stored_text_is_escaped_wherever_it_is_printed() {
    let p = Pin {
        reason: "a\u{1b}[31mred\u{202E}x\u{200B}\u{2028}\\".to_string(),
        set_by: Some("anchor\u{7}".to_string()),
        ..pin("demo")
    };
    let shown = display_text(&p.reason);
    assert_eq!(shown, "a\\u{001B}[31mred\\u{202E}x\\u{200B}\\u{2028}\\\\");
    let fact = PinFact::Present(p);
    let text = pin_text(&fact).unwrap();
    assert!(!text.chars().any(|c| c.is_control()), "{text:?}");
    assert!(text.contains("set by anchor\\u{0007} at"), "{text}");
    let fields = pin_fields(&fact, "local");
    assert_eq!(fields["kept_by_pin"], serde_json::json!(true));
    assert_eq!(fields["pin"]["reason"], serde_json::json!(shown));
    assert_eq!(fields["pin"]["source"], serde_json::json!("local"));
    assert!(pin_fields(&PinFact::None, "local").is_empty());
    assert_eq!(
        pin_text(&PinFact::Unreadable(PinProblem::TooLarge)).unwrap(),
        "pin unreadable: too_large"
    );
    let unreadable = pin_fields(&PinFact::Unreadable(PinProblem::MissingContent), "local");
    assert_eq!(
        unreadable["pin_unreadable"],
        serde_json::json!("missing_content")
    );
    assert!(unreadable.get("pin").is_none());
}

#[test]
fn pin_and_unpin_write_only_their_records_and_log_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    fs::create_dir_all(&base).unwrap();
    session(&base, "a", &[]);
    let backend = Backend::Local(crate::session::local::LocalBackend::with_base_dir(
        base.clone(),
    ));
    let log = fs::read(base.join("a").join(state_file_name("a"))).unwrap();

    let set = super::pin(&backend, "a", "keep me").unwrap();
    let PinOutcome::Set(first) = set else {
        panic!("{set:?}")
    };
    assert_eq!(first.id.len(), 32);
    let manifest: crate::session::context::Manifest =
        serde_json::from_slice(&fs::read(base.join("a/ctx/manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest.keys[PIN_KEY].writer.as_deref(),
        Some(crate::session::context_log::WRITER_KOTO)
    );
    // No per-key lock file next to the pin, which a scan would trip on.
    assert!(!base.join("a/ctx/koto/links/pin.lock").exists());

    let again = super::pin(&backend, "a", "another").unwrap();
    assert_eq!(again, PinOutcome::Existing(first.clone()));

    let record = super::unpin(&backend, "a", Some("done with it"))
        .unwrap()
        .unwrap();
    assert_eq!(record.pin.id.as_deref(), Some(first.id.as_str()));
    assert_eq!(record.reason.as_deref(), Some("done with it"));
    assert_eq!(read_local(&base, "a"), PinFact::None);
    assert_eq!(read_release(&base, "a"), Some(record));
    let manifest: crate::session::context::Manifest =
        serde_json::from_slice(&fs::read(base.join("a/ctx/manifest.json")).unwrap()).unwrap();
    assert!(!manifest.keys.contains_key(PIN_KEY));
    assert!(manifest.keys.contains_key(RELEASE_KEY));
    assert_eq!(
        fs::read(base.join("a").join(state_file_name("a"))).unwrap(),
        log
    );

    // A new pin gets a new id, so it never reads as the one released.
    let PinOutcome::Set(second) = super::pin(&backend, "a", "again").unwrap() else {
        panic!()
    };
    assert_ne!(second.id, first.id);
}

#[test]
fn an_unreadable_pin_refuses_pinning_and_is_cleared_by_unpin() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    fs::create_dir_all(&base).unwrap();
    let dir = session(&base, "a", &[PIN_KEY]);
    fs::write(dir.join("ctx").join(PIN_KEY), b"garbage").unwrap();
    let backend = Backend::Local(crate::session::local::LocalBackend::with_base_dir(
        base.clone(),
    ));
    let err = super::pin(&backend, "a", "r").unwrap_err();
    assert_eq!((err.code, err.exit), ("pin_unreadable", 2));
    let record = super::unpin(&backend, "a", None).unwrap().unwrap();
    assert_eq!(
        record.pin,
        ReleasedPin {
            id: None,
            reason: None,
            set_by: None,
            set_at: None
        }
    );
    assert_eq!(read_local(&base, "a"), PinFact::None);
}

#[test]
fn the_guard_keeps_a_pinned_session_and_clears_the_rest() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    fs::create_dir_all(&base).unwrap();
    session(&base, "p", &[]);
    session(&base, "u", &[]);
    session(&base, "bad", &[PIN_KEY]);
    fs::write(base.join("bad/ctx").join(PIN_KEY), b"garbage").unwrap();
    let backend = Backend::Local(crate::session::local::LocalBackend::with_base_dir(
        base.clone(),
    ));
    let set = super::pin(&backend, "p", "keep me").unwrap();

    assert_eq!(guard_removal(&backend, "u"), Guard::Clear);
    assert_eq!(guard_removal(&backend, "nosuch"), Guard::Clear);
    assert_eq!(
        guard_removal(&backend, "p"),
        Guard::Pinned {
            pin: set.pin().clone(),
            source: SOURCE_LOCAL
        }
    );
    let bad = guard_removal(&backend, "bad");
    assert_eq!(
        bad,
        Guard::PinUnreadable {
            problem: PinProblem::Unparseable,
            source: SOURCE_LOCAL
        }
    );

    // The refusal names the reason, the setter and the time, and says to
    // unpin first; it carries the pin's fields.
    let refusal = guard_removal(&backend, "p").refusal("p").unwrap();
    assert_eq!((refusal.code, refusal.exit), ("pinned", 2));
    assert!(refusal.message.contains("keep me"));
    assert!(refusal.message.contains("set by hand"));
    assert!(refusal.message.contains(&set.pin().set_at));
    assert!(refusal.message.contains("koto session unpin p"));
    let json = refusal.to_json();
    assert_eq!(json["error"]["kept_by_pin"], true);
    assert_eq!(json["error"]["pin"]["source"], "local");
    let refusal = bad.refusal("bad").unwrap();
    assert_eq!((refusal.code, refusal.exit), ("pin_unreadable", 2));
    assert_eq!(refusal.to_json()["error"]["pin_unreadable"], "unparseable");
    assert!(Guard::Clear.refusal("u").is_none());
    assert!(Guard::Clear.fields().is_empty());

    // A guarded removal deletes only what the guard clears.
    assert!(matches!(
        remove_guarded(&backend, "p").unwrap(),
        Removal::Kept(Guard::Pinned { .. })
    ));
    assert!(base.join("p").is_dir());
    assert!(matches!(
        remove_guarded(&backend, "bad").unwrap(),
        Removal::Kept(Guard::PinUnreadable { .. })
    ));
    assert!(base.join("bad").is_dir());
    assert_eq!(remove_guarded(&backend, "u").unwrap(), Removal::Removed);
    assert!(!base.join("u").exists());
    assert_eq!(
        remove_guarded(&backend, "nosuch").unwrap(),
        Removal::Removed
    );

    // A caller holding the lock deletes its own way, only when clear.
    session(&base, "v", &[]);
    let lock = backend.lock_state_file("v").unwrap();
    let mut ran = false;
    let removal = remove_guarded_with(&backend, "v", &lock, || {
        ran = true;
        Ok::<(), ()>(())
    });
    assert_eq!(removal, Ok(Removal::Removed));
    assert!(ran);
    let lock = backend.lock_state_file("p").unwrap();
    let removal = remove_guarded_with(&backend, "p", &lock, || -> Result<(), ()> {
        panic!("a pinned session's delete must not run")
    });
    assert!(matches!(removal, Ok(Removal::Kept(Guard::Pinned { .. }))));
}
