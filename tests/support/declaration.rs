//! Declaration bodies for tests that plant `koto-names/_complete` in the
//! bucket fake by hand, so a test can make a bucket read declared (or
//! carry a declaration koto must reject) without a command that writes
//! one.
//!
//! Include it with `#[path = "support/declaration.rs"] mod declaration;`
//! and plant a body with `FakeS3::put(declaration::KEY, &...)`.

#![allow(dead_code)]

/// The key the declaration lives at.
pub const KEY: &str = "koto-names/_complete";

/// The most bytes of the declaration koto reads.
pub const MAX_BYTES: usize = 4096;

/// The fields of a declaration `parse_declaration` accepts.
pub fn fields() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "declared_at": "2026-11-01T10:00:00Z",
        "machine": "host-a",
        "first_release": "v0.35.0",
        "audit": {
            "started_at": "2026-10-25T09:00:00Z",
            "finished_at": "2026-10-25T09:05:00Z",
            "prefixes": 1,
            "sessions": 1,
            "entries_written": 1,
            "entries_pruned": 0
        }
    })
}

/// A valid declaration body.
pub fn valid() -> Vec<u8> {
    serde_json::to_vec(&fields()).unwrap()
}

/// Bodies koto must read as undeclared, each with a label: a valid one
/// padded past [`MAX_BYTES`], one that isn't JSON, one missing a required
/// field, one whose `version` is a string, and one whose timestamp isn't a
/// timestamp.
pub fn rejected() -> Vec<(&'static str, Vec<u8>)> {
    let with = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut body = fields();
        edit(&mut body);
        serde_json::to_vec(&body).unwrap()
    };
    vec![
        (
            "oversized",
            with(&|body| body["padding"] = "x".repeat(MAX_BYTES).into()),
        ),
        ("not JSON", b"declared".to_vec()),
        (
            "missing field",
            with(&|body| {
                body.as_object_mut().unwrap().remove("machine");
            }),
        ),
        ("string version", with(&|body| body["version"] = "1".into())),
        (
            "bad timestamp",
            with(&|body| body["declared_at"] = "yesterday".into()),
        ),
    ]
}
