use super::*;
use crate::session::state_file_name;
use std::path::PathBuf;

/// A session `name` in the store at `base`, written directly: a header
/// (with `kind` when given, and `execution_dir`), then one `anchor_activity`
/// or `transitioned` event at `at`, then `extra` event lines.
fn session(
    base: &Path,
    name: &str,
    id: &str,
    kind: Option<&str>,
    exec: &str,
    at: &str,
    extra: &[String],
) {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut header = serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "ab".repeat(32),
        "created_at": at,
        "session_id": id,
        "execution_dir": exec,
    });
    if let Some(kind) = kind {
        header["kind"] = serde_json::Value::from(kind);
    }
    let first = if kind == Some("anchor") {
        format!(
            r#"{{"seq":1,"timestamp":"{at}","type":"anchor_activity","payload":{{"session":"x","command":"init"}}}}"#
        )
    } else {
        format!(
            r#"{{"seq":1,"timestamp":"{at}","type":"transitioned","payload":{{"from":null,"to":"work","condition_type":"auto"}}}}"#
        )
    };
    let mut text = format!("{header}\n{first}\n");
    for line in extra {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(dir.join(state_file_name(name)), text).unwrap();
}

fn now() -> String {
    crate::engine::types::now_iso8601()
}

/// A staged copy's `ctx/` holding `keys`, and its manifest.
fn staged(ctx: &Path, keys: &[(&str, &[u8])]) -> Manifest {
    let mut manifest = Manifest::default();
    for (key, body) in keys {
        let path = ctx.join(key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        manifest.keys.insert(
            key.to_string(),
            KeyMeta {
                created_at: "2026-01-01T00:00:00Z".to_string(),
                size: body.len() as u64,
                hash: crate::cache::sha256_hex(body),
                writer: Some("koto".to_string()),
            },
        );
    }
    manifest
}

fn body(peer: &str, workspace: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "v": 1,
        "session": peer,
        "session_id": "sid",
        "workspace": workspace,
        "at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

struct Store {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    ctx: PathBuf,
}

/// A local store with: an open anchor whose `execution_dir` is another
/// workspace than the one its links record, an anchor idle since 2020, an
/// anchor migrated to a local copy, the importer's own anchor, and an
/// ordinary session; plus a staging `ctx/` beside it.
fn store() -> Store {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    let ctx = tmp.path().join("staging").join("ctx");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::create_dir_all(&ctx).unwrap();
    let t = now();
    session(
        &base,
        "anchor-open",
        "open-id",
        Some("anchor"),
        "/elsewhere",
        &t,
        &[],
    );
    session(
        &base,
        "anchor-idle",
        "idle-id",
        Some("anchor"),
        "/ws",
        "2020-01-01T00:00:00.000Z",
        &[],
    );
    session(
        &base,
        "anchor-moved",
        "moved-id",
        Some("anchor"),
        "/ws",
        "2026-01-01T00:00:00.000Z",
        &[],
    );
    // The copy that makes `anchor-moved` migrated: its imported-from names
    // it with its id, and its log's import comes after the source's last
    // event.
    session(
        &base,
        "anchor-moved-2",
        "copy-id",
        Some("anchor"),
        "/ws2",
        "2026-01-01T00:00:00.000Z",
        &[format!(
            r#"{{"seq":2,"timestamp":"{t}","type":"session_imported","payload":{{"from_workspace":"/ws","from_session":"anchor-moved","from_session_id":"moved-id","machine_id":"m"}}}}"#
        )],
    );
    let copy_ctx = base.join("anchor-moved-2").join("ctx");
    let mut m = staged(
        &copy_ctx,
        &[(
            IMPORTED_FROM_KEY,
            br#"{"from_session":"anchor-moved","from_session_id":"moved-id"}"#,
        )],
    );
    m.keys.get_mut(IMPORTED_FROM_KEY).unwrap().writer = Some("koto".into());
    std::fs::write(
        copy_ctx.join("manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    session(
        &base,
        "anchor-imp",
        "imp-id",
        Some("anchor"),
        "/here",
        &t,
        &[],
    );
    session(&base, "plain", "plain-id", None, "/ws", &t, &[]);
    Store {
        _tmp: tmp,
        base,
        ctx,
    }
}

fn settle(s: &Store, importer: Option<&str>, manifest: &mut Manifest) -> Settled {
    Settlement {
        local_base: &s.base,
        importer_anchor: importer,
        target: "copy",
        from_session: "src",
        from_session_id: "src-id",
    }
    .apply(&s.ctx, manifest)
    .unwrap()
}

/// Every key in `manifest` has a file in `ctx` whose size and hash match,
/// and no file under `ctx/koto/` is unlisted.
fn assert_consistent(ctx: &Path, manifest: &Manifest) {
    for (key, meta) in &manifest.keys {
        let bytes = std::fs::read(ctx.join(key)).unwrap();
        assert_eq!(bytes.len() as u64, meta.size, "{key}");
        assert_eq!(crate::cache::sha256_hex(&bytes), meta.hash, "{key}");
    }
    let mut stack = vec![ctx.join("koto")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let key = p.strip_prefix(ctx).unwrap().to_string_lossy().into_owned();
                assert!(manifest.keys.contains_key(&key), "unlisted {key}");
            }
        }
    }
}

#[test]
fn the_reserved_keys_an_import_leaves_behind() {
    for key in [
        "koto/anchor/observed",
        "koto/anchor-close",
        "koto/links/closes/anchor-x",
        "koto/imported-from",
        "KOTO/links/held-by/anchor-x",
        "Koto/anchor/observed",
    ] {
        assert!(dropped_on_import(key), "{key}");
    }
    for key in [
        "koto/links/held-by/anchor-x",
        "koto/links/dispatched-by/coord",
        "notes.md",
        "kotox/a",
    ] {
        assert!(!dropped_on_import(key), "{key}");
    }
    let mut manifest = staged(
        &tempfile::TempDir::new().unwrap().path().join("ctx"),
        &[
            ("koto/anchor/observed", b"{}"),
            ("koto/links/closes/a", b"{}"),
            ("koto/links/held-by/a", b"{}"),
            ("notes.md", b"n"),
        ],
    );
    assert!(drop_from_manifest(&mut manifest));
    assert_eq!(
        manifest.keys.keys().collect::<Vec<_>>(),
        ["koto/links/held-by/a", "notes.md"]
    );
    assert!(!drop_from_manifest(&mut manifest));
}

#[test]
fn a_link_body_is_valid_only_when_it_names_the_peer_its_key_names() {
    let key = "koto/links/held-by/anchor-a";
    assert!(validate_link(key, &body("anchor-a", "/w")).is_ok());
    let with_was = br#"{"v":1,"session":"anchor-a","session_id":null,"workspace":null,"at":"t","was":"anchor-old"}"#;
    assert_eq!(
        validate_link(key, with_was).unwrap().was.as_deref(),
        Some("anchor-old")
    );
    for (key, bytes, why) in [
        (key, b"not json".to_vec(), "doesn't parse"),
        (key, br#"{"v":1,"session":7}"#.to_vec(), "doesn't parse"),
        (key, body("anchor-b", "/w"), "names session"),
        (
            key,
            br#"{"v":0,"session":"anchor-a","session_id":null,"workspace":null,"at":"t"}"#.to_vec(),
            "version 0",
        ),
        (key, vec![b' '; MAX_LINK_BODY_BYTES + 1], "limit"),
        (
            "koto/links/read-by/anchor-a",
            body("anchor-a", "/w"),
            "unknown",
        ),
        (
            "koto/links/held-by",
            body("anchor-a", "/w"),
            "names no peer",
        ),
        ("koto/links/held-by/a~1", body("a~1", "/w"), "invalid peer"),
    ] {
        let err = validate_link(key, &bytes).unwrap_err();
        assert!(err.contains(why), "{key}: {err}");
    }
}

/// With an importer anchor: the open anchor's link is kept although its
/// header's `execution_dir` differs from the link's workspace; the idle,
/// migrated and absent anchors' links are rewritten to the importer's
/// anchor once, with `was` naming the first; a malformed link is dropped;
/// the dispatched-by link is kept; `koto/imported-from` is written; and
/// the manifest matches the files.
#[test]
fn held_by_links_are_kept_rewritten_or_dropped_by_the_anchor_they_name() {
    let s = store();
    let mut manifest = staged(
        &s.ctx,
        &[
            (
                "koto/links/held-by/anchor-open",
                &body("anchor-open", "/ws"),
            ),
            (
                "koto/links/held-by/anchor-idle",
                &body("anchor-idle", "/ws"),
            ),
            (
                "koto/links/held-by/anchor-moved",
                &body("anchor-moved", "/ws"),
            ),
            (
                "koto/links/held-by/anchor-gone",
                &body("anchor-gone", "/ws"),
            ),
            ("koto/links/held-by/plain", &body("plain", "/ws")),
            ("koto/links/dispatched-by/coord", &body("coord", "/ws")),
            (
                "koto/links/dispatched-by/evil",
                &body("someone-else", "/ws"),
            ),
            ("notes.md", b"n"),
        ],
    );
    let settled = settle(&s, Some("anchor-imp"), &mut manifest);
    assert_eq!(settled.kept, vec!["anchor-open"]);
    assert_eq!(
        settled.rewritten,
        vec![
            ("anchor-gone".to_string(), "anchor-imp".to_string()),
            ("anchor-idle".to_string(), "anchor-imp".to_string()),
            ("anchor-moved".to_string(), "anchor-imp".to_string()),
            ("plain".to_string(), "anchor-imp".to_string()),
        ]
    );
    assert!(settled.dropped.is_empty());
    assert_eq!(settled.malformed.len(), 1);
    assert_eq!(settled.malformed[0].0, "koto/links/dispatched-by/evil");

    assert_eq!(
        manifest.keys.keys().collect::<Vec<_>>(),
        [
            "koto/imported-from",
            "koto/links/dispatched-by/coord",
            "koto/links/held-by/anchor-imp",
            "koto/links/held-by/anchor-open",
            "notes.md",
        ]
    );
    assert_consistent(&s.ctx, &manifest);
    let rewritten: LinkBody = serde_json::from_slice(
        &std::fs::read(s.ctx.join("koto/links/held-by/anchor-imp")).unwrap(),
    )
    .unwrap();
    assert_eq!(rewritten.session, "anchor-imp");
    assert_eq!(rewritten.session_id.as_deref(), Some("imp-id"));
    assert_eq!(rewritten.workspace.as_deref(), Some("/here"));
    assert_eq!(rewritten.was.as_deref(), Some("anchor-gone"));
    assert_eq!(
        manifest.keys["koto/links/held-by/anchor-imp"]
            .writer
            .as_deref(),
        Some("koto")
    );
    // The kept link is byte-for-byte the source's.
    assert_eq!(
        std::fs::read(s.ctx.join("koto/links/held-by/anchor-open")).unwrap(),
        body("anchor-open", "/ws")
    );
    let from: serde_json::Value =
        serde_json::from_slice(&std::fs::read(s.ctx.join(IMPORTED_FROM_KEY)).unwrap()).unwrap();
    assert_eq!(
        from,
        serde_json::json!({"from_session": "src", "from_session_id": "src-id"})
    );
}

/// With no importer anchor, or one naming no anchor here, every held-by
/// link whose anchor isn't open is dropped, and the dispatched-by link is
/// unchanged.
#[test]
fn with_no_importer_anchor_unresolved_held_by_links_are_dropped() {
    for importer in [None, Some("no-such-anchor"), Some("plain")] {
        let s = store();
        let coord = body("coord", "/ws");
        let mut manifest = staged(
            &s.ctx,
            &[
                (
                    "koto/links/held-by/anchor-open",
                    &body("anchor-open", "/ws"),
                ),
                (
                    "koto/links/held-by/anchor-idle",
                    &body("anchor-idle", "/ws"),
                ),
                (
                    "koto/links/held-by/anchor-gone",
                    &body("anchor-gone", "/ws"),
                ),
                ("koto/links/dispatched-by/coord", &coord),
            ],
        );
        let settled = settle(&s, importer, &mut manifest);
        assert_eq!(settled.kept, vec!["anchor-open"], "{importer:?}");
        assert_eq!(settled.dropped, vec!["anchor-gone", "anchor-idle"]);
        assert!(settled.rewritten.is_empty());
        assert_eq!(
            manifest.keys.keys().collect::<Vec<_>>(),
            [
                "koto/imported-from",
                "koto/links/dispatched-by/coord",
                "koto/links/held-by/anchor-open",
            ]
        );
        assert_eq!(
            std::fs::read(s.ctx.join("koto/links/dispatched-by/coord")).unwrap(),
            coord
        );
        assert_consistent(&s.ctx, &manifest);
    }
}

/// The importer's anchor is never linked to the copy when the copy is that
/// anchor; its own unresolved links are dropped instead.
#[test]
fn an_import_of_the_importers_own_anchor_never_links_it_to_itself() {
    let s = store();
    let mut manifest = staged(
        &s.ctx,
        &[(
            "koto/links/held-by/anchor-gone",
            &body("anchor-gone", "/ws"),
        )],
    );
    let settled = Settlement {
        local_base: &s.base,
        importer_anchor: Some("copy"),
        target: "copy",
        from_session: "src",
        from_session_id: "src-id",
    }
    .apply(&s.ctx, &mut manifest)
    .unwrap();
    assert_eq!(settled.dropped, vec!["anchor-gone"]);
    assert_eq!(
        manifest.keys.keys().collect::<Vec<_>>(),
        ["koto/imported-from"]
    );
}
