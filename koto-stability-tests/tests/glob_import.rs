//! A consumer that glob-imports the frozen types module still compiles.
//!
//! `koto::engine::types` is re-exported from koto-session-reader, so a
//! glob import reaches the crate's module through koto's path. This test
//! compiles against `use koto::engine::types::*;`, names the frozen
//! function and the error alias by their full paths, and runs nothing that
//! touches a session store.

use koto::engine::types::*;

#[test]
fn the_frozen_types_resolve_through_a_glob_import() {
    // Parsed from a log line rather than built field by field, since the
    // payload may gain fields additively.
    let line = r#"{"seq":1,"timestamp":"2026-05-24T00:00:00Z","type":"transitioned","payload":{"from":"start","to":"done","condition_type":"auto"}}"#;
    let event: Event = serde_json::from_str(line).unwrap();
    assert!(matches!(event.payload, EventPayload::Transitioned { .. }));
    let events: Vec<Event> = vec![event];
    assert_eq!(
        koto::engine::types::derive_state_from_log(&events),
        Some("done".to_string())
    );
    assert_eq!(CURRENT_SCHEMA_VERSION, 1);
}

#[test]
fn the_error_alias_resolves_beside_the_glob_import() {
    fn takes_error(e: Option<koto::error::Error>) -> bool {
        e.is_none()
    }
    assert!(takes_error(None));
    let _ = std::any::type_name::<koto::error::Error>();
}
