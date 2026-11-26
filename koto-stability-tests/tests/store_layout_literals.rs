//! The session store's layout names, pinned as literals.
//!
//! docs/STABILITY.md's "Session store read surface" section promises these
//! names to every reader of the store. Each is defined once, in
//! koto-session-reader, and koto's writer imports it; this test names them
//! through the crate a dependent names and compares each with the literal
//! older readers were built against, so a rename fails here instead of
//! silently orphaning them. Nothing here touches a session store.

use koto_session_reader::engine::leg_pointer::POINTER_FILENAME;
use koto_session_reader::engine::types::{Event, EventPayload};
use koto_session_reader::session::state_file_name;
use koto_session_reader::source::{
    CONTEXT_KEY_EVENTS, CTX_DIR, STAGING_PREFIX, STORE_LAYOUT_MARKER, STORE_LAYOUT_VERSION,
};

const SECTION: &str = "docs/STABILITY.md \"Session store read surface\"";

#[test]
fn the_state_file_name_is_pinned() {
    assert_eq!(
        state_file_name("x"),
        "koto-x.state.jsonl",
        "the state file name is promised by {SECTION}"
    );
}

#[test]
fn the_staging_prefix_is_pinned() {
    assert_eq!(
        STAGING_PREFIX, ".koto-staging-",
        "the staging prefix is promised by {SECTION}"
    );
    assert_eq!(
        koto::session::anchor::STAGING_PREFIX,
        ".koto-staging-",
        "koto's writer must stage under the prefix promised by {SECTION}"
    );
}

#[test]
fn the_context_directory_is_pinned() {
    assert_eq!(
        CTX_DIR, "ctx",
        "the context directory is promised by {SECTION}"
    );
}

#[test]
fn the_leg_pointer_file_name_is_pinned() {
    assert_eq!(
        POINTER_FILENAME, "request-leg.toml",
        "the leg pointer file name is promised by {SECTION}"
    );
}

#[test]
fn the_layout_marker_is_pinned() {
    assert_eq!(
        STORE_LAYOUT_MARKER, ".koto-store-layout",
        "the layout marker name is promised by {SECTION}"
    );
    assert_eq!(
        STORE_LAYOUT_VERSION, 1,
        "the store layout version is promised by {SECTION}"
    );
}

#[test]
fn the_context_key_events_are_pinned() {
    assert_eq!(
        CONTEXT_KEY_EVENTS,
        [
            "transitioned",
            "context_added",
            "context_removed",
            "context_cleared",
        ],
        "the events that set or supersede a context key are promised by {SECTION}"
    );
}

/// One log line of each key-setting type, parsed as a reader parses a state
/// log. Parsed rather than built field by field, since payloads may gain
/// fields additively.
#[test]
fn each_context_key_event_parses_to_its_variant() {
    let lines = [
        r#"{"seq":1,"timestamp":"2026-05-24T00:00:00Z","type":"transitioned","payload":{"from":"a","to":"b","condition_type":"auto","context_assignments":{"k":"v"}}}"#,
        r#"{"seq":2,"timestamp":"2026-05-24T00:00:00Z","type":"context_added","payload":{"key":"k","hash":"00","size":0}}"#,
        r#"{"seq":3,"timestamp":"2026-05-24T00:00:00Z","type":"context_removed","payload":{"key":"k"}}"#,
        r#"{"seq":4,"timestamp":"2026-05-24T00:00:00Z","type":"context_cleared","payload":{"state":"b","keys":["k"],"entry_seq":1}}"#,
    ];
    for (literal, line) in CONTEXT_KEY_EVENTS.iter().zip(lines) {
        let event: Event = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("a {literal} line must parse ({SECTION}): {e}"));
        let variant_matches = match *literal {
            "transitioned" => matches!(event.payload, EventPayload::Transitioned { .. }),
            "context_added" => matches!(event.payload, EventPayload::ContextAdded { .. }),
            "context_removed" => matches!(event.payload, EventPayload::ContextRemoved { .. }),
            "context_cleared" => matches!(event.payload, EventPayload::ContextCleared { .. }),
            other => panic!("{other} is not one of the events promised by {SECTION}"),
        };
        assert!(
            variant_matches,
            "a {literal} line must parse to its own variant, as promised by {SECTION}"
        );
        assert_eq!(
            event.payload.type_name(),
            *literal,
            "the {literal} variant's type name is promised by {SECTION}"
        );
    }
}
