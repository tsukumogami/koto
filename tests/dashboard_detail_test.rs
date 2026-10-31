//! `koto dashboard <NAME> --detail` at the binary boundary: the one-shot
//! JSON answer for a mixed local fixture, the error and migrated exits, the
//! preview ceiling, hostile content, and the feed and help surfaces it must
//! leave untouched (DESIGN-session-view Decision 4).

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use assert_cmd::Command;
use fake_s3::FakeS3;
use koto::session::context::ContextStore;
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use migration_carrier::{Cloud, Host};
use std::path::Path;

const BUCKET: &str = "koto-dashboard-detail";

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd.env("HOME", dir);
    cmd.env("XDG_CACHE_HOME", dir.join("cache"));
    cmd
}

fn header_of(workflow: &str) -> koto::engine::types::StateFileHeader {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": workflow,
        "session_id": "detail-fixture-uuid",
        "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn initialized() -> koto::engine::types::Event {
    serde_json::from_value(serde_json::json!({
        "seq": 1,
        "timestamp": "2026-01-01T00:00:00Z",
        "type": "workflow_initialized",
        "payload": {"template_path": "", "variables": {}},
    }))
    .unwrap()
}

/// A local session named `wf` under `<dir>/sessions` holding a short text
/// key, a text key over the excerpt bound, and a binary key.
fn local_fixture(dir: &Path) -> LocalBackend {
    let local = LocalBackend::with_base_dir(dir.join("sessions"));
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    local.add("wf", "notes.md", b"short notes").unwrap();
    let long = "long line of text\n".repeat(400); // > 4096 bytes
    local
        .add_with_writer("wf", "research/long.md", long.as_bytes(), "agent")
        .unwrap();
    let blob: Vec<u8> = (0u32..4096).map(|i| (i * 31 % 256) as u8).collect();
    assert!(blob.contains(&0));
    local.add("wf", "data/blob.bin", &blob).unwrap();
    local
}

fn detail_json(dir: &Path, name: &str) -> serde_json::Value {
    let output = koto_cmd(dir)
        .args(["dashboard", name, "--detail"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--detail should exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("one JSON object on stdout")
}

// -- the answer ---------------------------------------------------------------

/// PRD AC1/AC12: every key appears in lexicographic order with metadata; the
/// long key is truncated at the bound; the binary key shows type, recorded
/// size and hash and carries no content bytes.
#[test]
fn detail_lists_every_key_with_bounded_typed_previews() {
    let dir = tempfile::TempDir::new().unwrap();
    local_fixture(dir.path());

    let v = detail_json(dir.path(), "wf");
    assert_eq!(v["name"], "wf");
    assert_eq!(v["session_id"], "detail-fixture-uuid");
    assert_eq!(v["contentOmittedCount"], 0);
    assert_eq!(v["unlistedKeys"], 0);

    let keys = v["keys"].as_array().unwrap();
    let names: Vec<&str> = keys.iter().map(|k| k["key"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["data/blob.bin", "notes.md", "research/long.md"],
        "lexicographic and complete"
    );
    for k in keys {
        assert!(k["size"].as_u64().unwrap() > 0, "recorded size present");
        assert_eq!(k["locality"], "local");
        assert_eq!(k["stale"], false);
        assert!(k["created_at"].is_string(), "manifest created_at present");
    }

    let short = &keys[1]["content"];
    assert_eq!(short["type"], "text");
    assert_eq!(short["excerpt"], "short notes");
    assert_eq!(short["truncated"], false);

    let long = &keys[2]["content"];
    assert_eq!(long["type"], "text");
    assert_eq!(long["truncated"], true, "the explicit truncation marker");
    let excerpt = long["excerpt"].as_str().unwrap();
    assert!(excerpt.len() <= 4096, "excerpt bounded at EXCERPT_BOUND");
    assert_eq!(keys[2]["writer"], "agent");

    let binary = &keys[0]["content"];
    assert_eq!(binary["type"], "binary");
    assert_eq!(binary["size"], 4096);
    let hash = binary["hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64, "recorded hash, 64 hex characters");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(binary.get("excerpt").is_none(), "no content bytes");
}

// -- the exits ----------------------------------------------------------------

/// PRD AC6: a missing session exits 2 with `{error, command}` and nothing
/// else on stdout.
#[test]
fn detail_missing_session_exits_two_with_the_error_object() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();

    let output = koto_cmd(dir.path())
        .args(["dashboard", "no-such-session", "--detail"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("exactly one JSON object on stdout");
    assert_eq!(v["command"], "dashboard");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("'no-such-session' not found"));
    assert_eq!(
        v.as_object().unwrap().len(),
        2,
        "the error object and nothing else"
    );
}

/// PRD AC6: an unreadable record is an infrastructure failure, exit 3.
#[test]
fn detail_corrupt_session_exits_three() {
    let dir = tempfile::TempDir::new().unwrap();
    let session_dir = dir.path().join("sessions").join("wf");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("koto-wf.state.jsonl"),
        "not a valid header line\n",
    )
    .unwrap();

    let output = koto_cmd(dir.path())
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["command"], "dashboard");
    assert!(v["error"].is_string());
}

/// PRD AC6/AC13: a migrated session is an exit-0 answer naming its
/// successor, and the parsed JSON has no `keys` member at all.
#[test]
fn detail_migrated_session_answers_exit_zero_with_no_keys_member() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = Cloud {
        endpoint: s3.endpoint(),
        bucket: s3.bucket().to_string(),
        region: "us-east-1".to_string(),
        path_style: true,
        credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
    };
    let host = Host::new(root.path(), "a", &cloud);

    // A cloud session with one local key, then a migration marker in the
    // bucket saying it moved.
    let local = LocalBackend::with_base_dir(host.home.join(".koto").join("sessions"));
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    local.add("wf", "notes.md", b"old copy").unwrap();
    s3.put(
        &format!("{}/wf/migrated.json", host.prefix()),
        &serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "target": {
                "session": "wf-elsewhere",
                "session_id": "fixture-session-id",
                "workspace": "/srv/other-workspace",
                "prefix": "0123456789abcdef",
            },
            "machine_id": "another-machine",
            "migrated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
    );

    let run = host.koto(&["dashboard", "wf", "--detail"]);
    assert_eq!(
        run.code,
        Some(0),
        "an answer, not an error: {}",
        run.describe()
    );
    assert_eq!(run.json["migrated"]["target"], "wf-elsewhere");
    assert_eq!(run.json["migrated"]["workspace"], "/srv/other-workspace");
    assert!(
        run.json.get("keys").is_none(),
        "no key list masquerades as content"
    );
    assert_eq!(run.json["name"], "wf");
}

/// PRD AC5/AC8 (cloud half): a key only the remote manifest knows renders
/// its recorded size and hash and the not-local state, nothing fetched.
#[test]
fn detail_remote_only_key_renders_not_local() {
    let s3 = FakeS3::start(BUCKET);
    let root = tempfile::TempDir::new().unwrap();
    let cloud = Cloud {
        endpoint: s3.endpoint(),
        bucket: s3.bucket().to_string(),
        region: "us-east-1".to_string(),
        path_style: true,
        credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
    };
    let host = Host::new(root.path(), "a", &cloud);

    let sessions = host.home.join(".koto").join("sessions");
    let local = LocalBackend::with_base_dir(sessions.clone());
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    local.add("wf", "local.md", b"here").unwrap();

    // The remote manifest carries the local key plus one another host added.
    let local_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sessions.join("wf/ctx/manifest.json")).unwrap())
            .unwrap();
    let remote_manifest = serde_json::json!({
        "keys": {
            "local.md": local_manifest["keys"]["local.md"],
            "remote-only.md": {
                "created_at": "2026-01-02T00:00:00Z",
                "size": 42,
                "hash": "a".repeat(64),
            },
        }
    });
    s3.put(
        &format!("{}/wf/ctx/manifest.json", host.prefix()),
        &serde_json::to_vec(&remote_manifest).unwrap(),
    );

    s3.clear_requests();
    let run = host.koto(&["dashboard", "wf", "--detail"]);
    // The render's remote traffic is metadata only: the one manifest read
    // that tells a remote-only key apart, and the migration-marker listing.
    // No write, no pull of the state object, no key content.
    for r in s3.requests() {
        assert!(
            r.method != "PUT" && r.method != "DELETE",
            "--detail never writes: {} {}",
            r.method,
            r.key
        );
        if r.method == "GET" && r.list_prefix.is_none() {
            assert!(
                !r.key.ends_with(".state.jsonl"),
                "--detail never pulls the state object: {}",
                r.key
            );
            assert!(
                !r.key.contains("/ctx/") || r.key.ends_with("/ctx/manifest.json"),
                "--detail never fetches key content: {}",
                r.key
            );
        }
    }
    assert_eq!(run.code, Some(0), "{}", run.describe());
    let keys = run.json["keys"].as_array().unwrap();
    let remote = keys
        .iter()
        .find(|k| k["key"] == "remote-only.md")
        .expect("remote-only key listed");
    assert_eq!(remote["locality"], "remote-only");
    assert_eq!(remote["size"], 42);
    assert_eq!(remote["content"]["type"], "not-local");
    assert_eq!(remote["content"]["size"], 42);
    assert_eq!(remote["content"]["hash"], "a".repeat(64));
}

// -- bounds and hostility -----------------------------------------------------

/// PRD AC3: previews stop at the ceiling with an explicit per-key marker
/// and a top-level count, and the output is byte-identical across runs.
#[test]
fn detail_preview_ceiling_marks_omissions_and_output_is_deterministic() {
    let dir = tempfile::TempDir::new().unwrap();
    let local = LocalBackend::with_base_dir(dir.path().join("sessions"));
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    // 1,001 keys; zero-padded names make manifest order lexicographic.
    for i in 0..1001 {
        local
            .add("wf", &format!("k{i:04}.md"), format!("v{i}").as_bytes())
            .unwrap();
    }

    let first = koto_cmd(dir.path())
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert!(first.status.success());
    let v: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let keys = v["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1001, "listing stays complete past the ceiling");
    assert_eq!(
        keys[999]["content"]["type"], "text",
        "key 1,000 still carries a preview"
    );
    assert!(keys[999].get("contentOmitted").is_none());
    assert_eq!(
        keys[1000]["contentOmitted"], "key-limit",
        "key 1,001 is metadata-only"
    );
    assert!(keys[1000].get("content").is_none());
    assert_eq!(v["contentOmittedCount"], 1);

    let second = koto_cmd(dir.path())
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert_eq!(
        first.stdout, second.stdout,
        "byte-identical across two runs with no session activity"
    );
}

/// PRD AC5 (local half): an unreadable key renders an explicit marker with
/// a sanitized reason while every other key still renders.
#[test]
fn detail_unreadable_key_marks_itself_and_spares_the_rest() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    let local = local_fixture(dir.path());
    local.add("wf", "locked.md", b"cannot be read").unwrap();
    let content = dir.path().join("sessions/wf/ctx/locked.md");
    std::fs::set_permissions(&content, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&content).is_ok() {
        // Running as root: permission bits don't stop the read, so this
        // fixture can't make the key unreadable. The symlink and FIFO test
        // covers the unreadable marker without relying on them.
        std::fs::set_permissions(&content, std::fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    let v = detail_json(dir.path(), "wf");
    let keys = v["keys"].as_array().unwrap();
    let locked = keys.iter().find(|k| k["key"] == "locked.md").unwrap();
    assert_eq!(locked["content"]["type"], "unreadable");
    assert!(locked["content"]["reason"].is_string());
    let readable = keys.iter().find(|k| k["key"] == "notes.md").unwrap();
    assert_eq!(readable["content"]["type"], "text");

    std::fs::set_permissions(&content, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// PRD AC10: a value holding ESC and C1 characters reaches the JSON stream
/// only in escaped form -- no raw control byte on stdout.
#[test]
fn detail_hostile_value_reaches_stdout_only_escaped() {
    let dir = tempfile::TempDir::new().unwrap();
    let local = LocalBackend::with_base_dir(dir.path().join("sessions"));
    local
        .init_state_file("wf", header_of("wf"), vec![initialized()])
        .unwrap();
    local
        .add("wf", "hostile.md", "pre\u{1B}[2J\u{9B}31mpost".as_bytes())
        .unwrap();

    let output = koto_cmd(dir.path())
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !output.stdout.contains(&0x1B),
        "no raw ESC byte on the stream"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains('\u{9B}'),
        "no raw 8-bit CSI on the stream"
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let excerpt = v["keys"][0]["content"]["excerpt"].as_str().unwrap();
    assert_eq!(
        excerpt, "pre\\u{1B}[2J\\u{9B}31mpost",
        "the visible escape forms, not the control characters"
    );
}

// -- the surfaces it must not touch -------------------------------------------

/// PRD AC4/AC9: the feed row for a fixture session keeps its exact
/// tab-separated shape and stable columns, and the top-level verb list is
/// unchanged.
#[test]
fn detail_leaves_the_feed_and_the_verb_list_untouched() {
    let dir = tempfile::TempDir::new().unwrap();
    local_fixture(dir.path());

    let output = koto_cmd(dir.path())
        .args(["dashboard", "--once", "--all"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let row = stdout
        .lines()
        .find(|l| l.starts_with("wf\t"))
        .expect("fixture session in the feed");
    // The whole row against a golden, byte for byte, with the two clock
    // columns (elapsed, idle) normalized: they tick with wall time, so they
    // are checked for their compact-duration shape and then replaced. Every
    // other byte of the row, tabs included, must match exactly.
    let mut cols: Vec<String> = row.split('\t').map(str::to_string).collect();
    assert_eq!(cols.len(), 10, "the ten-column contract");
    for i in [2, 6] {
        assert!(
            !cols[i].is_empty()
                && cols[i]
                    .chars()
                    .all(|c| c.is_ascii_digit() || "dhms".contains(c)),
            "column {i} must be a compact duration, got {:?}",
            cols[i]
        );
        cols[i] = "<clock>".to_string();
    }
    assert_eq!(
        cols.join("\t"),
        "wf\t\t<clock>\tunknown\t\t\t<clock>\tpending\t\t",
        "the feed row for the fixture, golden apart from the clock"
    );

    let help = koto_cmd(dir.path()).arg("--help").output().unwrap();
    let help_text = String::from_utf8(help.stdout).unwrap();
    let verbs: Vec<&str> = help_text
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        verbs,
        vec![
            "version",
            "init",
            "next",
            "cancel",
            "rewind",
            "workflows",
            "template",
            "session",
            "context",
            "status",
            "decisions",
            "overrides",
            // Added by the check lifecycle work, not by the dashboard.
            "disputes",
            // Added by the document review telemetry work, not by the
            // dashboard.
            "review",
            "config",
            "decider",
            "workspace",
            "request",
            "dashboard",
            "help",
        ],
        "the top-level verb list is unchanged"
    );
}

/// The design's grep-style guard: no code in `src/cli/dashboard*.rs`
/// composes a filesystem path from a manifest string. Content reads go
/// through `session_view::read_key_preview`, whose accessor joins only a
/// validated key through the store's own `content_path`.
#[test]
fn dashboard_sources_compose_no_path_from_manifest_strings() {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli");
    // Every `dashboard*.rs`, found by listing rather than named, so a new
    // dashboard module is scanned the day it lands.
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&cli)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("dashboard") && name.ends_with(".rs")
        })
        .collect();
    files.sort();
    assert!(files.len() >= 4, "the dashboard modules were found");
    for file in &files {
        let file = file.display();
        let source = std::fs::read_to_string(file.to_string()).unwrap();
        for forbidden in [
            "content_path",
            "ctx_dir",
            ".join(key",
            ".join(&key",
            ".join(raw",
            ".join(&raw",
            "PathBuf::from(key",
            "Path::new(&key",
            "join(\"ctx\")",
        ] {
            assert!(
                !source.contains(forbidden),
                "{file} must not compose a path from a manifest string \
                 (found `{forbidden}`)"
            );
        }
    }
}

/// The error object's text is sanitized, not raw `anyhow` output: an I/O
/// error that quotes a store path carrying ESC and U+202E reaches stdout
/// with both as visible escapes. serde_json alone escapes ESC but passes
/// U+202E through raw, so the absent bidi override is what pins the fix.
#[test]
fn detail_error_text_is_sanitized() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path().join("evil\u{1B}[2J\u{202E}base");
    // A directory where the state log should be: the open fails and the
    // error message quotes the hostile path.
    std::fs::create_dir_all(base.join("wf").join("koto-wf.state.jsonl")).unwrap();

    let output = koto_cmd(dir.path())
        .env("KOTO_SESSIONS_BASE", &base)
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\u{1B}'), "no raw ESC: {stdout}");
    assert!(
        !stdout.contains('\u{202E}'),
        "no raw bidi override: {stdout}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let error = v["error"].as_str().unwrap();
    assert!(
        error.contains("\\u{1B}") && error.contains("\\u{202E}"),
        "visible escape forms: {error}"
    );
}

/// The flag's clap contract: it needs the positional name, and it refuses
/// every feed and TUI flag rather than silently picking one mode.
#[test]
fn detail_requires_a_name_and_refuses_the_feed_and_tui_flags() {
    let dir = tempfile::TempDir::new().unwrap();
    local_fixture(dir.path());

    let no_name = koto_cmd(dir.path())
        .args(["dashboard", "--detail"])
        .output()
        .unwrap();
    assert_eq!(no_name.status.code(), Some(2));
    assert!(no_name.stdout.is_empty());

    for flag in [
        &["--once"][..],
        &["--status", "active"],
        &["--needs-you"],
        &["--all"],
        &["--interval", "100"],
    ] {
        let mut args = vec!["dashboard", "wf", "--detail"];
        args.extend_from_slice(flag);
        let out = koto_cmd(dir.path()).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag:?} must be refused");
        assert!(out.stdout.is_empty(), "{flag:?}: nothing on stdout");
    }
}

/// A key whose content file was swapped for a symlink or a FIFO renders
/// the unreadable marker -- never followed, never blocked on -- while the
/// other keys still render.
#[test]
fn detail_symlink_and_fifo_keys_are_unreadable_not_followed() {
    let dir = tempfile::TempDir::new().unwrap();
    let local = local_fixture(dir.path());
    local.add("wf", "linked.md", b"placeholder").unwrap();
    local.add("wf", "piped.md", b"placeholder").unwrap();
    let ctx = dir.path().join("sessions/wf/ctx");

    let secret = dir.path().join("outside-secret.txt");
    std::fs::write(&secret, b"SECRET-OUTSIDE-THE-STORE").unwrap();
    std::fs::remove_file(ctx.join("linked.md")).unwrap();
    std::os::unix::fs::symlink(&secret, ctx.join("linked.md")).unwrap();

    std::fs::remove_file(ctx.join("piped.md")).unwrap();
    let fifo = std::ffi::CString::new(ctx.join("piped.md").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);

    let output = koto_cmd(dir.path())
        .args(["dashboard", "wf", "--detail"])
        .output()
        .unwrap();
    assert!(output.status.success(), "a FIFO must not block the read");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("SECRET-OUTSIDE-THE-STORE"),
        "a symlinked key is never followed"
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys = v["keys"].as_array().unwrap();
    for name in ["linked.md", "piped.md"] {
        let k = keys.iter().find(|k| k["key"] == name).unwrap();
        assert_eq!(k["content"]["type"], "unreadable", "{name}");
    }
    let notes = keys.iter().find(|k| k["key"] == "notes.md").unwrap();
    assert_eq!(notes["content"]["type"], "text");
}

/// Intent set the way people set it -- `koto init --intent`, then
/// `koto session update --intent` -- reaches the `--detail` facts, and the
/// latest update wins. koto records both as `intent_updated` events and
/// leaves the header field empty, so a header-only read shows nothing.
#[test]
fn detail_shows_the_intent_from_init_and_the_latest_update() {
    let dir = tempfile::TempDir::new().unwrap();
    let template = dir.path().join("t.md");
    std::fs::write(
        &template,
        "---\nname: demo\nversion: \"1.0\"\ninitial_state: gather\nstates:\n  gather:\n    \
         accepts:\n      route:\n        type: enum\n        required: true\n        values: [go]\n    \
         transitions:\n      - target: done\n        when:\n          route: go\n  done:\n    \
         terminal: true\n---\n\n## gather\n\nCollect inputs.\n\n## done\n\nDone.\n",
    )
    .unwrap();

    koto_cmd(dir.path())
        .args(["init", "demo", "--template", template.to_str().unwrap()])
        .args(["--intent", "first intent"])
        .assert()
        .success();
    assert_eq!(detail_json(dir.path(), "demo")["intent"], "first intent");

    koto_cmd(dir.path())
        .args(["session", "update", "--intent", "second intent", "demo"])
        .assert()
        .success();
    assert_eq!(detail_json(dir.path(), "demo")["intent"], "second intent");
}
