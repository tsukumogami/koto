//! `koto session import` keeps its source as a read-only predecessor: the
//! copy holds the continues relation naming the source, with its
//! back-reference beside the source, and `koto session prune
//! --migrated-from` waits until the copy's lineage lets the source go.
//!
//! Driven through the real binary on simulated hosts
//! (`support/migration_carrier.rs`) that share the in-process stand-in
//! bucket (`support/fake_s3.rs`). Every command here runs with every
//! inherited `KOTO_*` name removed, a `KOTO_HOME` of its own under the
//! test's temporary root, and only the stand-in's endpoint configured;
//! each test checks that before anything else runs.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/migration_carrier.rs"]
mod migration_carrier;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_s3::FakeS3;
use migration_carrier::{Cloud, Host, Run};
use serde_json::{json, Value};

const BUCKET: &str = "koto-import-keeps-source";

/// `work` waits for a marker; `done` is the terminal.
const WORK: &str = r#"---
name: import-keeps-source
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

const TEMPLATE: &str = "work.md";

/// Wait out the one-second retention every command runs with.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

/// The koto root a host's commands run with: `KOTO_HOME`.
fn koto_home(host: &Host) -> PathBuf {
    host.home.join(".koto")
}

/// A koto command on `host` with no inherited `KOTO_*` name (nor any
/// inherited `AWS_*` name but the stand-in's credentials the host sets),
/// `KOTO_HOME` under the host's own home, and a one-second retention.
fn cmd(host: &Host) -> assert_cmd::Command {
    let mut cmd = host.cmd();
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        let aws_set_by_host = name == "AWS_ACCESS_KEY_ID" || name == "AWS_SECRET_ACCESS_KEY";
        if name.starts_with("KOTO_") || (name.starts_with("AWS_") && !aws_set_by_host) {
            cmd.env_remove(&name);
        }
    }
    cmd.env("KOTO_HOME", koto_home(host))
        .env("KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS", "0")
        .env("KOTO_RETENTION_KEEP", "1s")
        .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
        .env("KOTO_RETENTION_STALE_AFTER", "2h");
    cmd
}

fn run(host: &Host, args: &[&str]) -> Run {
    let output = cmd(host).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(Value::Null)
    });
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        json,
    }
}

fn ok(host: &Host, args: &[&str]) -> Run {
    let r = run(host, args);
    assert!(r.ok(), "{} koto {args:?}: {}", host.label, r.describe());
    r
}

fn under(path: &Path, root: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(root)
}

/// Before anything runs: the host's home, `KOTO_HOME`, template cache,
/// workspace and session store all lie under the test's root, and the only
/// bucket its commands can reach is the stand-in on this machine.
fn assert_scratch(root: &Path, host: &Host, s3: &FakeS3) {
    for (what, path) in [
        ("HOME", host.home.clone()),
        ("KOTO_HOME", koto_home(host)),
        ("XDG_CACHE_HOME", host.cache.clone()),
        ("workspace", host.ws.clone()),
        (
            "session store",
            host.session_dir("x").parent().unwrap().into(),
        ),
    ] {
        assert!(
            under(&path, root),
            "{}'s {what} {} is outside the test root {}",
            host.label,
            path.display(),
            root.display()
        );
    }
    let endpoint = s3.endpoint();
    assert!(
        endpoint.starts_with("http://127.0.0.1:"),
        "the bucket endpoint {endpoint} isn't the local stand-in"
    );
    let config = std::fs::read_to_string(host.ws.join(".koto/config.toml")).unwrap();
    assert!(
        config.contains(&format!("endpoint = {endpoint:?}")) && config.contains(BUCKET),
        "{}'s workspace config names another bucket: {config}",
        host.label
    );
    let built = cmd(host);
    let envs: BTreeMap<String, Option<String>> = built
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        envs.get("KOTO_HOME").cloned().flatten().map(PathBuf::from),
        Some(koto_home(host))
    );
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if name.starts_with("KOTO_") {
            assert!(envs.contains_key(&name), "{name} would be inherited");
        }
    }
}

struct World {
    s3: FakeS3,
    root: PathBuf,
    _tmp: tempfile::TempDir,
    /// Host 1, workspace W1: the source's host.
    a: Host,
    /// Host 2, workspace W2: the importing host.
    b: Host,
    /// Host 1 again, in workspace W2: a same-host import's side.
    a2: Host,
}

impl World {
    fn new() -> Self {
        let s3 = FakeS3::start(BUCKET);
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let cloud = Cloud {
            endpoint: s3.endpoint(),
            bucket: s3.bucket().to_string(),
            region: "us-east-1".to_string(),
            path_style: true,
            credentials: Some(("test-access-key".to_string(), "test-secret-key".to_string())),
        };
        let a = Host::new(&root, "a", &cloud);
        let b = Host::new(&root, "b", &cloud);
        let mut a2 = Host::new(&root, "a-w2", &cloud);
        a2.home = a.home.clone();
        a2.cache = a.cache.clone();
        for host in [&a, &b, &a2] {
            std::fs::write(host.ws.join(TEMPLATE), WORK).unwrap();
            // Under KOTO_HOME only the root's own config selects the cloud
            // backend; the workspace's names the stand-in.
            std::fs::create_dir_all(koto_home(host)).unwrap();
            std::fs::write(
                koto_home(host).join("config.toml"),
                "[session]\nbackend = \"cloud\"\n",
            )
            .unwrap();
            assert_scratch(&root, host, &s3);
        }
        World {
            s3,
            root,
            _tmp: tmp,
            a,
            b,
            a2,
        }
    }

    fn template(host: &Host) -> String {
        host.ws.join(TEMPLATE).to_string_lossy().into_owned()
    }

    fn init(&self, host: &Host, name: &str, extra: &[&str]) {
        let t = Self::template(host);
        let mut args = vec!["init", name, "--template", t.as_str()];
        args.extend_from_slice(extra);
        ok(host, &args);
    }

    fn compile(&self, host: &Host) {
        let t = Self::template(host);
        ok(host, &["template", "compile", &t]);
    }

    /// Tick `name` to `done`, keeping it.
    fn done(&self, host: &Host, name: &str) {
        let r = ok(
            host,
            &[
                "next",
                name,
                "--with-data",
                r#"{"marker":"done"}"#,
                "--no-cleanup",
            ],
        );
        assert_eq!(r.json["action"], "done", "{}", r.describe());
    }

    fn import(&self, into: &Host, name: &str, from: &Host, extra: &[&str]) -> Run {
        let ws = from.ws_str();
        let mut args = vec!["session", "import", name, "--from", ws.as_str()];
        args.extend_from_slice(extra);
        run(into, &args)
    }

    /// v1 then v2 continuing it in W1 on host 1, both finished, the
    /// retention waited out; host 2 has the template compiled.
    fn chain(&self) {
        self.init(&self.a, "v1", &[]);
        self.init(&self.a, "v2", &["--continues", "v1"]);
        self.done(&self.a, "v1");
        self.done(&self.a, "v2");
        self.compile(&self.b);
        settle();
    }

    /// `chain`, then v2 imported into W2 on host 2 as `as_name` (its own
    /// name when `None`) and moved back to `work` there, so the copy runs.
    fn imported_running(&self, as_name: Option<&str>) -> String {
        self.chain();
        let extra: Vec<&str> = as_name.map(|n| vec!["--as", n]).unwrap_or_default();
        let r = self.import(&self.b, "v2", &self.a, &extra);
        assert!(r.ok(), "import: {}", r.describe());
        let copy = as_name.unwrap_or("v2").to_string();
        let r = ok(&self.b, &["rewind", &copy]);
        assert_eq!(r.json["state"], "work", "{}", r.describe());
        copy
    }

    fn marker_key(&self, name: &str) -> String {
        format!("{}/{name}/migrated.json", self.a.prefix())
    }

    fn pointer_key(
        &self,
        pred_host: &Host,
        pred: &str,
        holder_host: &Host,
        holder: &str,
    ) -> String {
        format!(
            "{}/{pred}/ctx/koto/continued-by/{}/{holder}",
            pred_host.prefix(),
            holder_host.prefix()
        )
    }

    fn relation_object(&self, host: &Host, holder: &str, pred: &str) -> String {
        format!("{}/{holder}/ctx/koto/links/continues/{pred}", host.prefix())
    }

    /// Every object under `prefix`, with its bytes.
    fn objects(&self, prefix: &str) -> BTreeMap<String, Vec<u8>> {
        self.s3
            .keys_under(prefix)
            .into_iter()
            .map(|k| {
                let v = self.s3.object(&k).unwrap();
                (k, v)
            })
            .collect()
    }

    /// `koto session prune --migrated-from <W1> <extra>` on `host`.
    fn migrated_from(&self, host: &Host, extra: &[&str]) -> Run {
        let ws = self.a.ws_str();
        let mut args = vec!["session", "prune", "--migrated-from", ws.as_str()];
        args.extend_from_slice(extra);
        run(host, &args)
    }

    /// Take the copy's relation out, locally and in the bucket, with its
    /// back-reference: the copy as a release without the relation built it.
    fn strip_relation(&self, host: &Host, copy: &str, source: &str) {
        let dir = host.session_dir(copy);
        let key = format!("koto/links/continues/{source}");
        let _ = std::fs::remove_file(dir.join("ctx").join(&key));
        let _ = std::fs::remove_file(dir.join(".continues-pointer"));
        let manifest = dir.join("ctx/manifest.json");
        let mut m: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        m["keys"].as_object_mut().unwrap().remove(&key);
        std::fs::write(&manifest, serde_json::to_vec_pretty(&m).unwrap()).unwrap();

        self.s3.remove(&self.relation_object(host, copy, source));
        let remote = format!("{}/{copy}/ctx/manifest.json", host.prefix());
        let mut m: Value = serde_json::from_slice(&self.s3.object(&remote).unwrap()).unwrap();
        m["keys"].as_object_mut().unwrap().remove(&key);
        self.s3
            .put(&remote, &serde_json::to_vec_pretty(&m).unwrap());
        self.s3
            .remove(&self.pointer_key(&self.a, source, host, copy));
        assert!(self
            .s3
            .keys_under(&format!(
                "{}/{source}/ctx/koto/continued-by/",
                self.a.prefix()
            ))
            .is_empty());
    }
}

/// Every file under a session directory with its bytes.
fn dir_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn header_id(host: &Host, name: &str) -> String {
    host.header(name)["session_id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The entry for `name` in `list` of a prune's output.
fn entry<'a>(out: &'a Value, list: &str, name: &str) -> &'a Value {
    out[list]
        .as_array()
        .unwrap_or_else(|| panic!("no {list}: {out:#}"))
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't in {list}: {out:#}"))
}

fn names(out: &Value, list: &str) -> Vec<String> {
    let mut v: Vec<String> = out[list]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| e["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The keepers an entry names: keeper, standing and path.
fn keepers(e: &Value) -> Vec<Value> {
    e["relation_keepers"]
        .as_array()
        .unwrap_or_else(|| panic!("no relation_keepers: {e:#}"))
        .iter()
        .map(|k| json!({"keeper": k["keeper"], "standing": k["standing"], "path": k["path"]}))
        .collect()
}

// ---------------------------------------------------------------------
// The import writes the relation
// ---------------------------------------------------------------------

#[test]
fn import_records_the_relation_with_its_back_reference_before_the_push_and_the_marker_after() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.chain();
    let source_id = header_id(&w.a, "v2");
    w.s3.clear_requests();
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "import: {}", r.describe());
    // The source's relation naming v1 wasn't carried, so nothing came
    // across but what the import wrote itself.
    assert_eq!(r.json["keys"], 0, "{}", r.describe());

    // The copy's only relation names the source with every field.
    let copy_id = header_id(&w.b, "v2");
    let ctx = w.b.session_dir("v2").join("ctx");
    let relations: Vec<String> = std::fs::read_dir(ctx.join("koto/links/continues"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(relations, ["v2"]);
    let local = std::fs::read(ctx.join("koto/links/continues/v2")).unwrap();
    let body: Value = serde_json::from_slice(&local).unwrap();
    assert_eq!(body["session"], "v2", "{body:#}");
    assert_eq!(body["session_id"], source_id.as_str(), "{body:#}");
    assert_eq!(body["workspace"], w.a.ws_str().as_str(), "{body:#}");
    assert_eq!(body["prefix"], w.a.prefix().as_str(), "{body:#}");
    assert_eq!(body["holder_session_id"], copy_id.as_str(), "{body:#}");
    assert_eq!(
        w.b.manifest("v2")["keys"]["koto/links/continues/v2"]["writer"],
        "koto"
    );
    assert!(w.b.manifest("v2")["keys"]
        .get("koto/links/continues/v1")
        .is_none());

    // The bucket holds the same relation, and none naming v1.
    let relation = w.relation_object(&w.b, "v2", "v2");
    assert_eq!(w.s3.object(&relation), Some(local));
    assert!(w.s3.object(&w.relation_object(&w.b, "v2", "v1")).is_none());

    // The back-reference sits beside the source, naming the copy's id.
    let pointer = w.pointer_key(&w.a, "v2", &w.b, "v2");
    let pbody: Value = serde_json::from_slice(&w.s3.object(&pointer).unwrap()).unwrap();
    assert_eq!(pbody["holder_session_id"], copy_id.as_str(), "{pbody:#}");
    let mark = std::fs::read_to_string(w.b.session_dir("v2").join(".continues-pointer")).unwrap();
    assert_eq!(mark.trim(), pointer);

    // Order: the back-reference before anything of the copy, the marker
    // after all of it.
    let puts: Vec<String> =
        w.s3.requests()
            .into_iter()
            .filter(|r| r.method == "PUT")
            .map(|r| r.key)
            .collect();
    let copy_prefix = format!("{}/v2/", w.b.prefix());
    let at = |key: &str| puts.iter().position(|k| k == key).unwrap();
    let first_copy = puts
        .iter()
        .position(|k| k.starts_with(&copy_prefix))
        .unwrap();
    let last_copy = puts
        .iter()
        .rposition(|k| k.starts_with(&copy_prefix))
        .unwrap();
    assert!(at(&pointer) < first_copy, "{puts:#?}");
    assert!(at(&w.marker_key("v2")) > last_copy, "{puts:#?}");
    assert!(puts.contains(&relation), "{puts:#?}");
}

#[test]
fn a_failed_push_leaves_no_marker_and_takes_the_back_reference_back() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.chain();
    let copy_prefix = format!("{}/v2/", w.b.prefix());
    w.s3.fail("PUT", &format!("{copy_prefix}koto-v2.state.jsonl"), 500);
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert_eq!(r.code, Some(1), "{}", r.describe());
    assert_eq!(r.json["error"]["code"], "import_push_failed");
    assert!(w.s3.object(&w.marker_key("v2")).is_none());
    assert!(w
        .s3
        .object(&w.pointer_key(&w.a, "v2", &w.b, "v2"))
        .is_none());
    assert!(w.s3.keys_under(&copy_prefix).is_empty());
    assert!(!w.b.session_dir("v2").exists());

    // Once the bucket takes writes again the same import goes through.
    w.s3.clear_faults();
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "{}", r.describe());
    assert!(w.s3.object(&w.marker_key("v2")).is_some());
    assert!(w
        .s3
        .object(&w.pointer_key(&w.a, "v2", &w.b, "v2"))
        .is_some());
}

#[test]
fn a_same_host_import_records_the_relation_and_keeps_its_source() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.a2, &w.s3);
    w.init(&w.a, "v2", &[]);
    let source_id = header_id(&w.a, "v2");
    // No --as: the copy takes the name v2 in the shared local store.
    let r = w.import(&w.a2, "v2", &w.a, &[]);
    assert!(r.ok(), "import: {}", r.describe());
    let copy_id = header_id(&w.a2, "v2");
    assert_ne!(copy_id, source_id);

    let local = std::fs::read(w.a2.session_dir("v2").join("ctx/koto/links/continues/v2")).unwrap();
    let body: Value = serde_json::from_slice(&local).unwrap();
    assert_eq!(body["session_id"], source_id.as_str(), "{body:#}");
    assert_eq!(body["prefix"], w.a.prefix().as_str(), "{body:#}");
    assert_eq!(body["workspace"], w.a.ws_str().as_str(), "{body:#}");
    assert_eq!(body["holder_session_id"], copy_id.as_str(), "{body:#}");
    assert_eq!(
        w.s3.object(&w.relation_object(&w.a2, "v2", "v2")),
        Some(local)
    );
    assert!(w
        .s3
        .object(&w.pointer_key(&w.a, "v2", &w.a2, "v2"))
        .is_some());
    assert!(w.s3.object(&w.marker_key("v2")).is_some());

    // The copy runs, so the source and its marker are kept, named by it.
    let r = w.migrated_from(&w.a2, &[]);
    assert!(r.ok(), "{}", r.describe());
    let e = entry(&r.json, "kept", "v2");
    assert_eq!(e["reason"], "relation", "{e:#}");
    assert_eq!(
        keepers(e),
        [json!({"keeper": "v2", "standing": "alive", "path": []})]
    );
}

// ---------------------------------------------------------------------
// Re-runs
// ---------------------------------------------------------------------

#[test]
fn a_rerun_adds_a_missing_relation_and_refuses_one_naming_another_session() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.chain();
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "import: {}", r.describe());
    let keys = r.json["keys"].clone();

    // A copy built without the relation: a re-run adds it, pointer first.
    w.strip_relation(&w.b, "v2", "v2");
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "re-run: {}", r.describe());
    assert_eq!(r.json["keys"], keys, "{}", r.describe());
    let relation = w.relation_object(&w.b, "v2", "v2");
    let body: Value = serde_json::from_slice(&w.s3.object(&relation).unwrap()).unwrap();
    assert_eq!(body["session_id"], header_id(&w.a, "v2").as_str());
    assert_eq!(body["prefix"], w.a.prefix().as_str());
    assert_eq!(body["holder_session_id"], header_id(&w.b, "v2").as_str());
    assert!(w
        .b
        .session_dir("v2")
        .join("ctx/koto/links/continues/v2")
        .is_file());
    assert!(w
        .s3
        .object(&w.pointer_key(&w.a, "v2", &w.b, "v2"))
        .is_some());
    // Run again with the relation there: nothing to add.
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "re-run: {}", r.describe());

    // A copy whose relation names another session: exit 2, nothing changed.
    w.strip_relation(&w.b, "v2", "v2");
    w.init(&w.b, "other", &[]);
    ok(&w.b, &["session", "link", "v2", "--continues", "other"]);
    let copy_prefix = format!("{}/v2/", w.b.prefix());
    let local_before = dir_files(&w.b.session_dir("v2"));
    let bucket_before = w.objects(&copy_prefix);
    let source_before = w.objects(&format!("{}/v2/", w.a.prefix()));
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert_eq!(r.code, Some(2), "{}", r.describe());
    assert_eq!(
        r.json["error"]["code"],
        "relation_conflict",
        "{}",
        r.describe()
    );
    let message = r.json["error"]["message"].as_str().unwrap();
    assert!(message.contains("'other'"), "{message}");
    assert_eq!(dir_files(&w.b.session_dir("v2")), local_before);
    assert_eq!(w.objects(&copy_prefix), bucket_before);
    assert_eq!(w.objects(&format!("{}/v2/", w.a.prefix())), source_before);
}

// ---------------------------------------------------------------------
// `prune --migrated-from` waits for the lineage
// ---------------------------------------------------------------------

#[test]
fn migrated_from_keeps_the_source_while_the_copy_runs_and_removes_it_once_released() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.imported_running(None);
    let source = format!("{}/v2/", w.a.prefix());
    let before = w.objects(&source);

    // The copy runs: kept, the relation the reason, the copy named alive.
    for host in [&w.b, &w.a] {
        let r = w.migrated_from(host, &[]);
        assert!(r.ok(), "{}", r.describe());
        assert!(r.json.get("ignored").is_none(), "{:#}", r.json);
        assert!(!r.stderr.contains("ignored:"), "{}", r.stderr);
        let e = entry(&r.json, "kept", "v2");
        assert_eq!(e["target_status"], "present", "{e:#}");
        assert_eq!(e["reason"], "relation", "{e:#}");
        assert_eq!(e["removable"], false, "{e:#}");
        assert_eq!(e["kept_by_relation"], true, "{e:#}");
        assert_eq!(
            keepers(e),
            [json!({"keeper": "v2", "standing": "alive", "path": []})]
        );
        assert!(names(&r.json, "remove").is_empty());
    }

    // --source-retired is accepted, reported ignored, and changes nothing.
    let r = w.migrated_from(&w.b, &["--source-retired"]);
    assert!(r.ok(), "{}", r.describe());
    assert_eq!(r.json["ignored"], json!(["source_retired"]));
    assert!(
        r.stderr
            .contains("ignored: source_retired no longer decides removal"),
        "{}",
        r.stderr
    );
    assert_eq!(entry(&r.json, "kept", "v2")["reason"], "relation");
    // So is the retain bound, set for the user, alone or with the flag.
    ok(
        &w.b,
        &[
            "config",
            "set",
            "--user",
            "session.cloud.marker_retain_days",
            "0",
        ],
    );
    let r = w.migrated_from(&w.b, &[]);
    assert!(r.ok(), "{}", r.describe());
    assert_eq!(r.json["ignored"], json!(["marker_retain_days"]));
    assert!(
        r.stderr
            .contains("ignored: marker_retain_days no longer decides removal"),
        "{}",
        r.stderr
    );
    let r = w.migrated_from(&w.b, &["--apply", "--source-retired"]);
    assert!(r.ok(), "{}", r.describe());
    assert_eq!(
        r.json["ignored"],
        json!(["marker_retain_days", "source_retired"])
    );
    assert!(names(&r.json, "removed").is_empty(), "{:#}", r.json);
    assert_eq!(entry(&r.json, "kept", "v2")["reason"], "relation");
    assert_eq!(w.objects(&source), before);

    // A prune on the source's host, which never pulled the copy, keeps
    // the source and names the copy.
    let r = ok(
        &w.a,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    let e = entry(&r.json, "kept", "v2");
    assert_eq!(
        keepers(e),
        [json!({"keeper": "v2", "standing": "alive", "path": []})]
    );
    assert!(!names(&r.json, "removed").contains(&"v2".to_string()));
    assert_eq!(w.objects(&source), before);

    // The copy finishes and its retention passes: the source goes, marker
    // last, and the copy stays.
    w.done(&w.b, "v2");
    settle();
    let r = w.migrated_from(&w.b, &["--apply"]);
    assert!(r.ok(), "{}", r.describe());
    assert_eq!(names(&r.json, "removed"), ["v2"], "{:#}", r.json);
    assert_eq!(entry(&r.json, "removed", "v2")["reason"], "released");
    // Only the copy's back-reference is left: it is the copy's, not the
    // source's.
    assert_eq!(
        w.s3.keys_under(&source),
        [w.pointer_key(&w.a, "v2", &w.b, "v2")]
    );
    assert!(w
        .s3
        .object(&format!("{}/v2/koto-v2.state.jsonl", w.b.prefix()))
        .is_some());
}

#[test]
fn a_chain_member_imported_elsewhere_keeps_its_predecessor_through_the_copy() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    let copy = w.imported_running(Some("v2-next"));

    // The copy holds no relation naming v1.
    assert!(w.s3.object(&w.relation_object(&w.b, &copy, "v1")).is_none());
    for args in [
        &["session", "prune", "--all"][..],
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ][..],
    ] {
        let r = ok(&w.a, args);
        let e = entry(&r.json, "kept", "v1");
        assert_eq!(
            keepers(e),
            [json!({"keeper": copy, "standing": "alive", "path": ["v2"]})],
            "{args:?}"
        );
    }
    assert!(w
        .s3
        .object(&format!("{}/v1/koto-v1.state.jsonl", w.a.prefix()))
        .is_some());
}

#[test]
fn an_earlier_releases_import_keeps_its_source_by_the_marker_alone() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.imported_running(None);
    // As a release without the relation left it: a marker naming the
    // copy, and no relation or back-reference.
    w.strip_relation(&w.b, "v2", "v2");

    for host in [&w.b, &w.a] {
        let r = w.migrated_from(host, &[]);
        assert!(r.ok(), "{}", r.describe());
        let e = entry(&r.json, "kept", "v2");
        assert_eq!(e["reason"], "relation", "{e:#}");
        assert_eq!(
            keepers(e),
            [json!({"keeper": "v2", "standing": "alive", "path": []})]
        );
    }
    // A prune on the source's host, which never pulled the copy.
    let r = ok(
        &w.a,
        &[
            "session",
            "prune",
            "--apply",
            "--all",
            "--confirm-short-retention",
        ],
    );
    let e = entry(&r.json, "kept", "v2");
    assert_eq!(
        keepers(e),
        [json!({"keeper": "v2", "standing": "alive", "path": []})]
    );
    assert!(w
        .s3
        .object(&format!("{}/v2/koto-v2.state.jsonl", w.a.prefix()))
        .is_some());
}

#[test]
fn a_marker_whose_target_is_gone_keeps_the_source_as_absent() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.imported_running(None);
    ok(&w.b, &["session", "cleanup", "v2"]);
    assert!(w.s3.keys_under(&format!("{}/v2/", w.b.prefix())).is_empty());
    let source = format!("{}/v2/", w.a.prefix());
    let before = w.objects(&source);
    let r = w.migrated_from(&w.b, &["--apply"]);
    assert!(r.ok(), "{}", r.describe());
    let e = entry(&r.json, "kept", "v2");
    assert_eq!(e["target_status"], "absent", "{e:#}");
    assert_eq!(e["reason"], "absent", "{e:#}");
    assert!(e.get("relation_keepers").is_none(), "{e:#}");
    assert_eq!(w.objects(&source), before);
}

#[test]
fn the_sources_pin_travels_and_keeps_nothing_once_the_copy_is_released() {
    let w = World::new();
    assert_scratch(&w.root, &w.a, &w.s3);
    assert_scratch(&w.root, &w.b, &w.s3);
    w.init(&w.a, "v2", &[]);
    ok(
        &w.a,
        &["session", "pin", "v2", "--reason", "keep for review"],
    );
    w.compile(&w.b);
    let r = w.import(&w.b, "v2", &w.a, &[]);
    assert!(r.ok(), "import: {}", r.describe());
    let pin = format!("{}/v2/ctx/koto/links/pin", w.b.prefix());
    let source_pin = format!("{}/v2/ctx/koto/links/pin", w.a.prefix());
    assert_eq!(w.s3.object(&pin), w.s3.object(&source_pin));
    assert!(w.s3.object(&pin).is_some());

    // The copy finishes; its pin keeps the source.
    w.done(&w.b, "v2");
    settle();
    let r = w.migrated_from(&w.b, &["--apply"]);
    assert!(r.ok(), "{}", r.describe());
    let e = entry(&r.json, "kept", "v2");
    assert_eq!(e["relation_keepers"][0]["standing"], "pin", "{e:#}");

    // Unpinned and past its retention, the copy keeps nothing, and the
    // source's own pin keeps nothing either.
    ok(&w.b, &["session", "unpin", "v2"]);
    settle();
    assert!(w.s3.object(&source_pin).is_some());
    let r = w.migrated_from(&w.b, &["--apply"]);
    assert!(r.ok(), "{}", r.describe());
    assert_eq!(names(&r.json, "removed"), ["v2"], "{:#}", r.json);
    assert_eq!(
        w.s3.keys_under(&format!("{}/v2/", w.a.prefix())),
        [w.pointer_key(&w.a, "v2", &w.b, "v2")]
    );
}
