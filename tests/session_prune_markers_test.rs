//! `koto session prune --migrated-from`, driven through the real binary on
//! simulated hosts that share the in-process fake S3 endpoint
//! (`tests/support/fake_s3.rs`). Markers come from real `koto session
//! import` runs (host B importing from host A); only the unreadable marker
//! and the aged `migrated_at` are written into the bucket directly.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use std::collections::BTreeMap;
use std::path::Path;

use fake_s3::FakeS3;
use migration_carrier::{Cloud, Host, Run};
use serde_json::Value;

const BUCKET: &str = "koto-prune-markers";

struct Hosts {
    s3: FakeS3,
    _root: tempfile::TempDir,
    /// The source: sessions start here.
    a: Host,
    /// The target: sessions are imported here.
    b: Host,
    /// A third host, for a target that moves on again.
    c: Host,
}

impl Hosts {
    fn new() -> Self {
        let s3 = FakeS3::start(BUCKET);
        let root = tempfile::TempDir::new().unwrap();
        let cloud = Cloud {
            endpoint: s3.endpoint(),
            bucket: s3.bucket().to_string(),
            region: "us-east-1".to_string(),
            path_style: true,
            credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
        };
        let a = Host::new(root.path(), "a", &cloud);
        let b = Host::new(root.path(), "b", &cloud);
        let c = Host::new(root.path(), "c", &cloud);
        for host in [&b, &c] {
            let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
            let run = host.koto(&["template", "compile", template.to_str().unwrap()]);
            assert!(run.ok(), "compile: {}", run.describe());
        }
        Hosts {
            s3,
            _root: root,
            a,
            b,
            c,
        }
    }

    /// Start `name` on A with one context key, and import it into B, which
    /// leaves a marker under A's prefix.
    fn migrate(&self, name: &str) {
        self.start(&self.a, name);
        self.import(&self.b, name, &self.a);
    }

    fn start(&self, host: &Host, name: &str) {
        let template = host.ws.join(migration_carrier::TEMPLATE_FILE);
        let run = host.koto(&["init", name, "--template", template.to_str().unwrap()]);
        assert!(run.ok(), "init {name}: {}", run.describe());
        let src = host.home.join(format!("{name}-notes.md"));
        std::fs::write(&src, b"notes").unwrap();
        let run = host.koto(&[
            "context",
            "add",
            name,
            "notes.md",
            "--from-file",
            src.to_str().unwrap(),
        ]);
        assert!(run.ok(), "context add: {}", run.describe());
    }

    fn import(&self, into: &Host, name: &str, from: &Host) {
        let ws = from.ws_str();
        let run = into.koto(&["session", "import", name, "--from", &ws]);
        assert!(run.ok(), "import {name}: {}", run.describe());
    }

    /// `koto session prune --migrated-from <A> <extra>` run on `host`.
    fn prune_on(&self, host: &Host, extra: &[&str]) -> Run {
        let ws = self.a.ws_str();
        let mut args = vec!["session", "prune", "--migrated-from", ws.as_str()];
        args.extend_from_slice(extra);
        host.koto(&args)
    }

    fn prune(&self, extra: &[&str]) -> Run {
        self.prune_on(&self.b, extra)
    }

    fn prune_ok(&self, extra: &[&str]) -> Value {
        let run = self.prune(extra);
        assert!(run.ok(), "prune {extra:?}: {}", run.describe());
        run.json
    }

    /// `<A's prefix>/<name>/`.
    fn source(&self, name: &str) -> String {
        format!("{}/{}/", self.a.prefix(), name)
    }

    fn marker_key(&self, name: &str) -> String {
        format!("{}migrated.json", self.source(name))
    }

    /// Every object in the bucket with its bytes.
    fn bucket(&self) -> BTreeMap<String, Vec<u8>> {
        self.s3
            .keys()
            .into_iter()
            .map(|k| {
                let v = self.s3.object(&k).unwrap();
                (k, v)
            })
            .collect()
    }
}

/// Every marker the output lists, by name, from `remove` and `kept`.
fn entries(out: &Value) -> BTreeMap<String, Value> {
    let mut all = BTreeMap::new();
    for list in ["remove", "kept"] {
        for e in out[list].as_array().unwrap() {
            all.insert(e["name"].as_str().unwrap().to_string(), e.clone());
        }
    }
    all
}

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

/// Every file under a host's session store with its bytes, leaving out the
/// stored plans.
fn local_store(host: &Host) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if rel == ".prune-plans" {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let root = host.home.join(".koto").join("sessions");
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out);
    out
}

fn plan_files(host: &Host) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(
        host.home
            .join(".koto")
            .join("sessions")
            .join(".prune-plans"),
    )
    .map(|d| {
        d.flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    })
    .unwrap_or_default();
    files.sort();
    files
}

fn refused(run: &Run, code: &str, exit: i32) {
    assert_eq!(run.code, Some(exit), "{code}: {}", run.describe());
    assert_eq!(run.json["error"]["code"], code, "{}", run.describe());
}

#[test]
fn each_target_status_is_named_and_siblings_never_cross_match() {
    let h = Hosts::new();
    for name in ["foo", "foo-2", "again", "moved-on", "unreachable"] {
        h.migrate(name);
    }
    // absent: B removed its copy of `foo`, while `foo-2` stays present.
    let run = h.b.koto(&["session", "cleanup", "foo"]);
    assert!(run.ok(), "cleanup: {}", run.describe());
    // replaced: B removed `again` and started a new session under the name.
    let run = h.b.koto(&["session", "cleanup", "again"]);
    assert!(run.ok(), "cleanup: {}", run.describe());
    h.start(&h.b, "again");
    // migrated: C imported `moved-on` from B, so B's copy carries a marker.
    h.import(&h.c, "moved-on", &h.b);
    // unknown: the target's listing fails.
    h.s3.fail("LIST", &format!("{}/unreachable/", h.b.prefix()), 500);
    // unreadable: a marker whose body doesn't parse (seeded directly).
    h.s3.put(&h.marker_key("broken"), b"not json");
    // A context key named migrated.json is not a marker.
    h.s3.put(
        &format!("{}ctx/migrated.json", h.source("foo-2")),
        b"{\"migrated_at\":\"2020-01-01T00:00:00Z\"}",
    );

    for extra in [&[][..], &["--source-retired"][..]] {
        let out = h.prune_ok(extra);
        assert_eq!(out["mode"], "preview");
        assert_eq!(out["prefix"], h.a.prefix());
        let all = entries(&out);
        assert_eq!(
            all.keys().cloned().collect::<Vec<_>>(),
            vec!["again", "broken", "foo", "foo-2", "moved-on", "unreachable"],
            "{out:#}"
        );
        for (name, status) in [
            ("foo", "absent"),
            ("foo-2", "present"),
            ("again", "replaced"),
            ("moved-on", "migrated"),
            ("unreachable", "unknown"),
        ] {
            let e = &all[name];
            assert_eq!(e["target_status"], status, "{name}: {e:#}");
            assert_eq!(e["target"]["session"], name, "{name}: {e:#}");
            assert_eq!(e["target"]["prefix"], h.b.prefix(), "{name}: {e:#}");
            assert_eq!(e["target"]["workspace"], h.b.ws_str(), "{name}: {e:#}");
            assert_eq!(e["age_days"], 0, "{name}: {e:#}");
            assert!(e["migrated_at"].as_str().is_some(), "{name}: {e:#}");
            if status != "present" {
                // Kept at any age and with or without the flag, the status
                // being the reason.
                assert_eq!(e["removable"], false, "{name}: {e:#}");
                assert_eq!(e["reason"], status, "{name}: {e:#}");
            }
        }
        let broken = &all["broken"];
        assert_eq!(broken["removable"], false);
        assert_eq!(broken["reason"], "unreadable");
        assert!(broken["target_status"].is_null());

        let foo2 = &all["foo-2"];
        if extra.is_empty() {
            assert_eq!(foo2["removable"], false, "{foo2:#}");
            assert_eq!(foo2["reason"], "age", "{foo2:#}");
            assert!(names(&out["remove"]).is_empty());
        } else {
            assert_eq!(foo2["removable"], true, "{foo2:#}");
            assert_eq!(foo2["reason"], "source_retired", "{foo2:#}");
            assert_eq!(names(&out["remove"]), vec!["foo-2"]);
        }
    }
    let replaced = &entries(&h.prune_ok(&[]))["again"];
    assert!(
        replaced["detail"]
            .as_str()
            .unwrap()
            .contains("another session"),
        "{replaced:#}"
    );
}

#[test]
fn the_age_bound_and_source_retired_decide_a_present_target() {
    let h = Hosts::new();
    h.migrate("foo");
    h.migrate("foo-2");

    // Inside the default 30-day bound: kept, and the age is the reason.
    let out = h.prune_ok(&[]);
    assert_eq!(out["marker_retain_days"], 30);
    for e in entries(&out).values() {
        assert_eq!(
            (&e["removable"], &e["reason"]),
            (&Value::Bool(false), &Value::from("age"))
        );
        assert!(e["detail"]
            .as_str()
            .unwrap()
            .contains("inside the 30-day bound"));
    }
    // --source-retired widens the rule to them, and says so.
    let out = h.prune_ok(&["--source-retired"]);
    assert_eq!(names(&out["remove"]), vec!["foo", "foo-2"]);
    for e in entries(&out).values() {
        assert_eq!(e["reason"], "source_retired");
    }

    // The bound is a user config key. A project can't shorten it: setting it
    // there is refused, and a project file that holds it is ignored.
    let run =
        h.b.koto(&["config", "set", "session.cloud.marker_retain_days", "0"]);
    assert!(
        !run.ok(),
        "a project value was accepted: {}",
        run.describe()
    );
    let project = h.b.ws.join(".koto/config.toml");
    let text = std::fs::read_to_string(&project).unwrap();
    assert!(text.contains("[session.cloud]\n"), "{text}");
    std::fs::write(
        &project,
        text.replacen(
            "[session.cloud]\n",
            "[session.cloud]\nmarker_retain_days = 0\n",
            1,
        ),
    )
    .unwrap();
    let out = h.prune_ok(&[]);
    assert_eq!(out["marker_retain_days"], 30);
    assert!(names(&out["remove"]).is_empty());
    std::fs::write(&project, &text).unwrap();

    // Set for the user, at 0 days every present target's marker is past it.
    let run = h.b.koto(&[
        "config",
        "set",
        "--user",
        "session.cloud.marker_retain_days",
        "0",
    ]);
    assert!(run.ok(), "config set: {}", run.describe());
    let run =
        h.b.koto(&["config", "get", "session.cloud.marker_retain_days"]);
    assert_eq!(run.stdout.trim(), "0", "{}", run.describe());
    let out = h.prune_ok(&[]);
    assert_eq!(out["marker_retain_days"], 0);
    assert_eq!(names(&out["remove"]), vec!["foo", "foo-2"]);
    for e in entries(&out).values() {
        assert_eq!(e["reason"], "age");
    }

    // Back to the default, with `foo`'s marker dated long ago: past the
    // bound, while `foo-2` stays inside it.
    let run = h.b.koto(&[
        "config",
        "unset",
        "--user",
        "session.cloud.marker_retain_days",
    ]);
    assert!(run.ok(), "config unset: {}", run.describe());
    let key = h.marker_key("foo");
    let mut marker: Value = serde_json::from_slice(&h.s3.object(&key).unwrap()).unwrap();
    marker["migrated_at"] = Value::from("2020-01-01T00:00:00.000Z");
    h.s3.put(&key, &serde_json::to_vec(&marker).unwrap());
    let out = h.prune_ok(&[]);
    assert_eq!(names(&out["remove"]), vec!["foo"]);
    let all = entries(&out);
    assert_eq!(all["foo"]["reason"], "age");
    assert!(all["foo"]["age_days"].as_u64().unwrap() > 30);
    assert_eq!(all["foo-2"]["removable"], false);

    // A bound larger than the marker's age keeps it again.
    let run = h.b.koto(&[
        "config",
        "set",
        "--user",
        "session.cloud.marker_retain_days",
        "100000",
    ]);
    assert!(run.ok(), "config set: {}", run.describe());
    let out = h.prune_ok(&[]);
    assert!(names(&out["remove"]).is_empty(), "{out:#}");
}

#[test]
fn apply_removes_only_the_removable_source_and_leaves_its_sibling_byte_identical() {
    let h = Hosts::new();
    h.migrate("foo");
    h.migrate("foo-2");
    // `foo-2`'s target is gone, so its marker is kept; `foo` is removable
    // with --source-retired. `plain` was never migrated.
    let run = h.b.koto(&["session", "cleanup", "foo-2"]);
    assert!(run.ok(), "cleanup: {}", run.describe());
    h.start(&h.a, "plain");
    assert!(h.s3.keys_under(&h.source("foo")).len() > 1);

    let before = h.bucket();
    let b_local = local_store(&h.b);
    let a_local = local_store(&h.a);

    let preview = h.prune_ok(&["--source-retired"]);
    assert_eq!(names(&preview["remove"]), vec!["foo"]);
    assert_eq!(names(&preview["kept"]), vec!["foo-2"]);
    // The preview changes nothing in the bucket.
    assert_eq!(h.bucket(), before);
    let token = preview["plan"].as_str().unwrap().to_string();
    assert_eq!(
        plan_files(&h.b),
        vec![format!("markers-{}-{token}.json", h.a.prefix())]
    );

    h.s3.clear_requests();
    let out = h.prune_ok(&["--apply", "--plan", &token]);
    assert_eq!(out["mode"], "apply");
    assert_eq!(names(&out["removed"]), vec!["foo"]);
    assert_eq!(names(&out["kept"]), vec!["foo-2"]);
    assert!(out["failed"].as_array().unwrap().is_empty());
    assert!(out["skipped"].as_array().unwrap().is_empty());

    // Every object under `<A>/foo/` is gone, and nothing else changed.
    let after = h.bucket();
    assert!(h.s3.keys_under(&h.source("foo")).is_empty());
    let mut expected = before.clone();
    expected.retain(|k, _| !k.starts_with(&h.source("foo")));
    assert_eq!(after, expected);
    assert!(!h.s3.keys_under(&h.source("foo-2")).is_empty());

    // The marker is deleted last.
    let deletes: Vec<String> =
        h.s3.requests()
            .into_iter()
            .filter(|r| r.method == "DELETE")
            .map(|r| r.key)
            .collect();
    assert_eq!(deletes.last(), Some(&h.marker_key("foo")), "{deletes:?}");
    assert!(deletes.iter().all(|k| k.starts_with(&h.source("foo"))));

    // Nothing local was touched on either host; the applied plan is gone.
    assert_eq!(local_store(&h.b), b_local);
    assert_eq!(local_store(&h.a), a_local);
    assert!(plan_files(&h.b).is_empty(), "{:?}", plan_files(&h.b));
}

#[test]
fn a_delete_failure_leaves_the_marker_refusing_and_a_retry_finishes() {
    let h = Hosts::new();
    h.migrate("foo");
    let state = format!("{}koto-foo.state.jsonl", h.source("foo"));
    h.s3.fail("DELETE", &state, 500);

    let run = h.prune(&["--apply", "--source-retired"]);
    assert_eq!(run.code, Some(3), "{}", run.describe());
    assert_eq!(names(&run.json["failed"]), vec!["foo"]);
    assert!(run.json["removed"].as_array().unwrap().is_empty());
    let error = run.json["failed"][0]["error"].as_str().unwrap();
    assert!(error.contains("koto-foo.state.jsonl"), "{error}");
    // Part of the copy went, the marker didn't, and A still refuses.
    assert!(h.s3.object(&h.marker_key("foo")).is_some());
    assert!(h.s3.object(&state).is_some());
    let run = h.a.koto(&["next", "foo"]);
    refused(&run, "session_migrated", 2);

    // Run again, this time on A itself: it finishes, and A's own local copy
    // of the session stays where it is.
    h.s3.clear_faults();
    let a_local = local_store(&h.a);
    let run = h.prune_on(&h.a, &["--apply", "--source-retired"]);
    assert!(run.ok(), "retry: {}", run.describe());
    assert_eq!(names(&run.json["removed"]), vec!["foo"]);
    assert!(h.s3.keys_under(&h.source("foo")).is_empty());
    assert_eq!(local_store(&h.a), a_local);
    assert!(h.a.session_dir("foo").is_dir());
}

#[test]
fn the_marker_plan_applies_unchanged_and_refuses_when_stale_or_unknown() {
    let h = Hosts::new();
    h.migrate("foo");
    h.migrate("foo-2");

    let preview = h.prune_ok(&["--source-retired"]);
    assert_eq!(names(&preview["remove"]), vec!["foo", "foo-2"]);
    let stale = preview["plan"].as_str().unwrap().to_string();

    // `foo-2`'s target goes away after the preview: the token is stale.
    let run = h.b.koto(&["session", "cleanup", "foo-2"]);
    assert!(run.ok(), "cleanup: {}", run.describe());
    let before = h.bucket();
    let run = h.prune(&["--apply", "--plan", &stale]);
    refused(&run, "prune_plan_stale", 4);
    let dropped = names(&run.json["error"]["dropped"]);
    assert_eq!(dropped, vec!["foo-2"]);
    assert_eq!(h.bucket(), before, "a stale plan removed something");

    // A fresh preview's token applies, with the --source-retired its preview
    // was given (the stored plan records it).
    let preview = h.prune_ok(&["--source-retired"]);
    let token = preview["plan"].as_str().unwrap().to_string();
    assert_ne!(token, stale);
    // A session-mode apply never takes a marker plan.
    let run = h.b.koto(&["session", "prune", "--apply", "--plan", &token]);
    refused(&run, "prune_plan_unknown", 4);
    // Nor does a marker apply for another source workspace.
    let other = h.c.ws_str();
    let run = h.b.koto(&[
        "session",
        "prune",
        "--migrated-from",
        &other,
        "--apply",
        "--plan",
        &token,
    ]);
    refused(&run, "prune_plan_unknown", 4);
    assert_eq!(h.bucket(), before);

    let out = h.prune_ok(&["--apply", "--plan", &token]);
    assert_eq!(names(&out["removed"]), vec!["foo"]);
    assert!(h.s3.keys_under(&h.source("foo")).is_empty());
    assert!(h.s3.object(&h.marker_key("foo-2")).is_some());

    // Applied once, the plan is gone.
    let run = h.prune(&["--apply", "--plan", &token]);
    refused(&run, "prune_plan_unknown", 4);
}

#[test]
fn arguments_and_the_local_backend_are_checked_before_anything_runs() {
    let h = Hosts::new();
    h.migrate("foo");
    let before = h.bucket();

    for extra in [
        &["--all"][..],
        &["--older-than", "1d"][..],
        &["--execution-dir", "/work/project"][..],
        &["--plan", "0123456789abcdef0123456789abcdef"][..],
        &["--apply", "--plan", "not-a-token"][..],
        &[
            "--apply",
            "--plan",
            "0123456789abcdef0123456789abcdef",
            "--source-retired",
        ][..],
    ] {
        let run = h.prune(extra);
        refused(&run, "invalid_arguments", 2);
    }
    let run =
        h.b.koto(&["session", "prune", "--migrated-from", "relative/path"]);
    refused(&run, "invalid_arguments", 2);
    let run = h.b.koto(&["session", "prune", "--source-retired"]);
    refused(&run, "invalid_arguments", 2);

    // Under the local backend the flag is refused.
    std::fs::write(
        h.b.ws.join(".koto").join("config.toml"),
        "[session]\nbackend = \"local\"\n",
    )
    .unwrap();
    let run = h.prune(&[]);
    refused(&run, "invalid_arguments", 2);
    let run = h.prune(&["--apply", "--source-retired"]);
    refused(&run, "invalid_arguments", 2);

    assert_eq!(h.bucket(), before);
}

/// The state log key of session `name` under host `host`'s prefix.
fn log_key(host: &Host, name: &str) -> String {
    format!("{}/{name}/koto-{name}.state.jsonl", host.prefix())
}

/// Rewrite the log object at `key` line by line.
fn rewrite_log(h: &Hosts, key: &str, f: impl FnOnce(Vec<String>) -> Vec<String>) {
    let bytes =
        h.s3.object(key)
            .unwrap_or_else(|| panic!("no log at {key}"));
    let lines: Vec<String> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let mut out = f(lines).join("\n");
    out.push('\n');
    h.s3.put(key, out.as_bytes());
}

/// `lines` with `n` more events after the last, each a copy of it with the
/// next seq.
fn with_more_events(mut lines: Vec<String>, n: u64) -> Vec<String> {
    let mut last: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    for _ in 0..n {
        last["seq"] = Value::from(last["seq"].as_u64().unwrap() + 1);
        lines.push(last.to_string());
    }
    lines
}

/// Every object under `<A>/<name>/` with its bytes.
fn source_objects(h: &Hosts, name: &str) -> BTreeMap<String, Vec<u8>> {
    let mut all = h.bucket();
    all.retain(|k, _| k.starts_with(&h.source(name)));
    all
}

#[test]
fn a_truncated_or_corrupt_target_log_keeps_the_marker_and_the_source_through_preview_and_apply() {
    let h = Hosts::new();
    for name in ["cut", "garbled", "whole"] {
        h.migrate(name);
    }
    let preview = h.prune_ok(&["--source-retired"]);
    assert_eq!(names(&preview["remove"]), vec!["cut", "garbled", "whole"]);
    let stale = preview["plan"].as_str().unwrap().to_string();
    let whole = &entries(&preview)["whole"];
    assert_eq!(
        whole["objects"],
        h.s3.keys_under(&h.source("whole")).len(),
        "{whole:#}"
    );

    // `cut`'s target log loses half its final line, as a torn write leaves
    // it; `garbled`'s first event no longer parses. Both headers still name
    // the marker's session id.
    rewrite_log(&h, &log_key(&h.b, "cut"), |mut lines| {
        let last = lines.pop().unwrap();
        lines.push(last[..last.len() / 2].to_string());
        lines
    });
    rewrite_log(&h, &log_key(&h.b, "garbled"), |mut lines| {
        assert!(lines.len() > 2, "{lines:?}");
        lines[1] = "{not an event".to_string();
        lines
    });
    let cut_before = source_objects(&h, "cut");
    let garbled_before = source_objects(&h, "garbled");

    // The plan previewed while they were whole is stale now.
    let before = h.bucket();
    let run = h.prune(&["--apply", "--plan", &stale]);
    refused(&run, "prune_plan_stale", 4);
    assert_eq!(names(&run.json["error"]["dropped"]), vec!["cut", "garbled"]);
    assert_eq!(h.bucket(), before, "a stale plan removed something");

    // A fresh preview keeps both, whatever the age or the flag.
    for extra in [&[][..], &["--source-retired"][..]] {
        let all = entries(&h.prune_ok(extra));
        for name in ["cut", "garbled"] {
            let e = &all[name];
            assert_eq!(e["target_status"], "unverified", "{name}: {e:#}");
            assert_eq!(e["reason"], "unverified", "{name}: {e:#}");
            assert_eq!(e["removable"], false, "{name}: {e:#}");
            assert!(
                e["detail"]
                    .as_str()
                    .unwrap()
                    .contains("doesn't read to the end"),
                "{name}: {e:#}"
            );
        }
    }

    // An apply with no plan judges again and removes only `whole`.
    let out = h.prune_ok(&["--apply", "--source-retired"]);
    assert_eq!(names(&out["removed"]), vec!["whole"]);
    assert_eq!(names(&out["kept"]), vec!["cut", "garbled"]);
    assert!(h.s3.keys_under(&h.source("whole")).is_empty());
    assert_eq!(source_objects(&h, "cut"), cut_before);
    assert_eq!(source_objects(&h, "garbled"), garbled_before);
    // The old copies still refuse on the source host.
    for name in ["cut", "garbled"] {
        refused(&h.a.koto(&["next", name]), "session_migrated", 2);
    }
}

#[test]
fn a_source_log_past_the_targets_or_unreadable_keeps_the_marker() {
    let h = Hosts::new();
    for name in ["ahead", "torn-source", "level"] {
        h.migrate(name);
    }
    // `ahead`'s source log gains two events past the target's last, which
    // carries the source's events plus the import's one.
    rewrite_log(&h, &log_key(&h.a, "ahead"), |lines| {
        with_more_events(lines, 2)
    });
    // `torn-source`'s source log ends in a line that doesn't parse.
    rewrite_log(&h, &log_key(&h.a, "torn-source"), |mut lines| {
        lines.push("{\"seq\":".to_string());
        lines
    });
    // `level`'s source log gains one: it now ends where the target's does.
    rewrite_log(&h, &log_key(&h.a, "level"), |lines| {
        with_more_events(lines, 1)
    });
    let ahead_before = source_objects(&h, "ahead");
    let torn_before = source_objects(&h, "torn-source");

    let all = entries(&h.prune_ok(&["--source-retired"]));
    for (name, says) in [
        ("ahead", "the source's runs to seq"),
        ("torn-source", "the source copy can't be read"),
    ] {
        let e = &all[name];
        assert_eq!(e["target_status"], "unverified", "{name}: {e:#}");
        assert_eq!(e["removable"], false, "{name}: {e:#}");
        assert!(
            e["detail"].as_str().unwrap().contains(says),
            "{name}: {e:#}"
        );
    }
    assert_eq!(all["level"]["target_status"], "present");
    assert_eq!(all["level"]["removable"], true);

    let out = h.prune_ok(&["--apply", "--source-retired"]);
    assert_eq!(names(&out["removed"]), vec!["level"]);
    assert_eq!(source_objects(&h, "ahead"), ahead_before);
    assert_eq!(source_objects(&h, "torn-source"), torn_before);
}
