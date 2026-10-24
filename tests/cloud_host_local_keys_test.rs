//! Context keys koto writes for itself, on the cloud backend (koto#386).
//!
//! Every key koto writes for a session either reaches the bucket with its
//! object or is host-local and stays off the bucket's manifest. The one
//! host-local key is `workflows/publish-location`, the Claude Code
//! `/workflows` folder koto renders a session into, which koto publishes on
//! the commit funnel through the local store. koto 0.19.0 listed it in the
//! bucket's manifest without uploading its object, so `koto session import`
//! refused the session with `import_source_unreadable`.
//!
//! Every test drives the koto binary against the in-process endpoint in
//! `tests/support/fake_s3.rs`, with the backend chosen through
//! `KOTO_SESSION_*` variables alone. A simulated host is a disposable
//! `HOME`; a workspace is a directory under it.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use fake_s3::FakeS3;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-host-local";
const NAME: &str = "coord";
const TEMPLATE_FILE: &str = "coord.md";
const LOCATION: &str = "workflows/publish-location";

/// A system `PATH` for the action's shell, after the directory holding the
/// koto under test.
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// A first state whose action writes a context key from inside the tick,
/// a state waiting on evidence, and a terminal.
const TEMPLATE: &str = r#"---
name: coord
version: "1.0"
initial_state: work
states:
  work:
    default_action:
      command: |
        printf one | koto context add coord one.md
    transitions:
      - target: wait
  wait:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## wait

Wait.

## done

Done.
"#;

fn koto_dir() -> PathBuf {
    koto_command::koto_bin().parent().unwrap().to_path_buf()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The remote prefix koto derives for a workspace.
fn prefix_of(dir: &Path) -> String {
    let canonical = std::fs::canonicalize(dir).unwrap();
    sha256_hex(canonical.to_string_lossy().as_bytes())[..16].to_string()
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn json(&self) -> serde_json::Value {
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line).unwrap_or(serde_json::Value::Null)
    }

    fn describe(&self) -> String {
        format!(
            "exit {:?}; stdout: {}; stderr: {}",
            self.code,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

/// One simulated host: a disposable `HOME` and template cache, and the
/// shared bucket's endpoint.
struct Host {
    root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    endpoint: String,
}

impl Host {
    fn new(s3: &FakeS3) -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let home = mk(root.path().join("home"));
        let cache = mk(root.path().join("cache"));
        Host {
            root,
            home,
            cache,
            endpoint: s3.endpoint(),
        }
    }

    /// A workspace on this host holding the template.
    fn workspace(&self, name: &str) -> PathBuf {
        let ws = mk(self.root.path().join(name));
        std::fs::write(ws.join(TEMPLATE_FILE), TEMPLATE).unwrap();
        ws
    }

    /// A koto run in `ws` with nothing inherited from the test process but
    /// `extra`.
    fn koto(&self, ws: &Path, args: &[&str], extra: &[(&str, &str)]) -> Out {
        Self::out(self.cmd(ws, args, extra).output().unwrap())
    }

    /// `koto context add` from outside any tick.
    fn add(&self, ws: &Path, name: &str, content: &[u8]) -> Out {
        Self::out(
            self.cmd(ws, &["context", "add", NAME, name], &[])
                .write_stdin(content.to_vec())
                .output()
                .unwrap(),
        )
    }

    fn out(o: std::process::Output) -> Out {
        Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn cmd(&self, ws: &Path, args: &[&str], extra: &[(&str, &str)]) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_clear()
            .current_dir(ws)
            .env("PATH", format!("{}:{}", koto_dir().display(), SYSTEM_PATH))
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", &self.endpoint)
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.args(args);
        cmd
    }

    /// The session's local store directory on this host.
    fn session_dir(&self) -> PathBuf {
        self.home.join(".koto").join("sessions").join(NAME)
    }
}

fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

/// The bucket key of `rest` under `ws`'s copy of the session.
fn key(ws: &Path, rest: &str) -> String {
    session_key(ws, NAME, rest)
}

/// The bucket key of `rest` under `ws`'s copy of `session`.
fn session_key(ws: &Path, session: &str, rest: &str) -> String {
    format!("{}/{}/{}", prefix_of(ws), session, rest)
}

/// The bucket's manifest for `ws`'s copy, checked whole: every key it names
/// has an object whose hash matches, and no host-local key is listed.
/// Returns the manifest's keys.
fn assert_bucket_manifest_whole(s3: &FakeS3, ws: &Path) -> Vec<String> {
    assert_session_manifest_whole(s3, ws, NAME)
}

/// [`assert_bucket_manifest_whole`] for `session`.
fn assert_session_manifest_whole(s3: &FakeS3, ws: &Path, session: &str) -> Vec<String> {
    let key = |rest: &str| session_key(ws, session, rest);
    let manifest: serde_json::Value = s3
        .object(&key("ctx/manifest.json"))
        .map(|b| serde_json::from_slice(&b).expect("remote manifest parses"))
        .expect("the bucket holds a manifest");
    let keys = manifest["keys"].as_object().cloned().unwrap_or_default();
    for (name, meta) in &keys {
        let object = s3
            .object(&key(&format!("ctx/{}", name)))
            .unwrap_or_else(|| panic!("the manifest names {name}, which the bucket lacks"));
        assert_eq!(
            meta["hash"].as_str().unwrap(),
            sha256_hex(&object),
            "{name}'s object doesn't match the manifest"
        );
    }
    assert!(
        !keys.contains_key(LOCATION),
        "the bucket's manifest lists the host-local {LOCATION}: {manifest}"
    );
    assert!(
        s3.object(&key(&format!("ctx/{}", LOCATION))).is_none(),
        "the bucket holds the host-local {LOCATION}"
    );
    keys.keys().cloned().collect()
}

/// The `/workflows` files koto rendered into `dir`.
fn rendered(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("koto-") && n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Init the session in `ws` with `env`, tick it once into the state that
/// waits for evidence, and check it published `wf` as its location.
fn source(h: &Host, ws: &Path, env: &[(&str, &str)], wf: &Path) {
    let out = h.koto(ws, &["init", NAME, "--template", TEMPLATE_FILE], env);
    assert!(out.ok(), "init: {}", out.describe());
    let out = h.koto(ws, &["next", NAME], env);
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "wait", "{}", out.describe());
    let out = h.koto(ws, &["context", "get", NAME, LOCATION], env);
    assert!(out.ok(), "get {LOCATION}: {}", out.describe());
    assert_eq!(out.stdout.trim(), wf.to_str().unwrap());
    assert_eq!(
        rendered(wf).len(),
        1,
        "nothing rendered into {}",
        wf.display()
    );
}

/// Advance the imported session in `ws` with `env` and check it published
/// `wf`, a folder on its own host, and rendered there.
fn assert_resolves_its_own_location(h: &Host, ws: &Path, env: &[(&str, &str)], wf: &Path) {
    let out = h.koto(ws, &["context", "exists", NAME, LOCATION], env);
    assert_eq!(
        out.code,
        Some(1),
        "the import carried {LOCATION}: {}",
        out.describe()
    );
    let out = h.koto(
        ws,
        &[
            "next",
            NAME,
            "--with-data",
            r#"{"go":"go"}"#,
            "--no-cleanup",
        ],
        env,
    );
    assert!(out.ok(), "next: {}", out.describe());
    assert_eq!(out.json()["state"], "done", "{}", out.describe());
    let out = h.koto(ws, &["context", "get", NAME, LOCATION], env);
    assert!(out.ok(), "get {LOCATION}: {}", out.describe());
    assert_eq!(out.stdout.trim(), wf.to_str().unwrap());
    assert_eq!(
        rendered(wf).len(),
        1,
        "nothing rendered into {}",
        wf.display()
    );
}

/// After init and a tick, every key in the bucket's manifest has its object
/// and the published location isn't listed, through each way koto pushes a
/// manifest: the tick's batch flush, a write outside a tick, a removal,
/// `koto workflows publish`, and `koto session resolve --keep local`. The
/// location stays readable on its own host, and `--keep remote` keeps it.
#[test]
fn every_manifest_entry_has_its_object_and_the_location_stays_on_its_host() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let ws = h.workspace("ws");
    let wf = mk(h.home.join("wf"));
    let env = [("KOTO_WORKFLOWS_DIR", wf.to_str().unwrap())];
    source(&h, &ws, &env, &wf);

    // The tick's action wrote one.md inside the batch; the location was
    // published on the commit funnel, through the local store.
    let local_manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(h.session_dir().join("ctx").join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert!(local_manifest["keys"].get(LOCATION).is_some());
    assert_eq!(assert_bucket_manifest_whole(&s3, &ws), ["one.md"]);

    // A write outside a tick pushes the manifest on its own.
    let out = h.add(&ws, "two.md", b"two");
    assert!(out.ok(), "context add: {}", out.describe());
    assert_eq!(assert_bucket_manifest_whole(&s3, &ws), ["one.md", "two.md"]);
    let out = h.koto(&ws, &["context", "remove", NAME, "two.md"], &env);
    assert!(out.ok(), "remove: {}", out.describe());
    assert_eq!(assert_bucket_manifest_whole(&s3, &ws), ["one.md"]);

    // Repointing the session by hand keeps the location on this host too.
    let other = mk(h.home.join("wf-other"));
    let out = h.koto(
        &ws,
        &[
            "workflows",
            "publish",
            "--dir",
            other.to_str().unwrap(),
            "--session",
            NAME,
        ],
        &[],
    );
    assert!(out.ok(), "workflows publish: {}", out.describe());
    let out = h.koto(&ws, &["context", "get", NAME, LOCATION], &[]);
    assert_eq!(out.stdout.trim(), other.to_str().unwrap());
    assert_eq!(assert_bucket_manifest_whole(&s3, &ws), ["one.md"]);

    // The push the import guide asks for before a move uploads the whole
    // local copy, a nested key included, and still leaves the location out.
    // The nested key's object is deleted first, so only the push can put it
    // back; the flat listing it replaced failed on any nested key.
    let out = h.add(&ws, "notes/plan.md", b"plan");
    assert!(out.ok(), "context add: {}", out.describe());
    s3.remove(&key(&ws, "ctx/notes/plan.md"));
    let out = h.koto(&ws, &["session", "resolve", NAME, "--keep", "local"], &[]);
    assert!(out.ok(), "resolve --keep local: {}", out.describe());
    assert_eq!(
        assert_bucket_manifest_whole(&s3, &ws),
        ["notes/plan.md", "one.md"]
    );

    // Taking the bucket's copy keeps this host's location.
    let out = h.koto(&ws, &["session", "resolve", NAME, "--keep", "remote"], &[]);
    assert!(out.ok(), "resolve --keep remote: {}", out.describe());
    let out = h.koto(&ws, &["context", "get", NAME, LOCATION], &[]);
    assert!(out.ok(), "get {LOCATION}: {}", out.describe());
    assert_eq!(out.stdout.trim(), other.to_str().unwrap());
    let out = h.koto(&ws, &["context", "get", NAME, "one.md"], &[]);
    assert_eq!(out.stdout, "one", "{}", out.describe());
}

/// The owner's case: a session that published a `/workflows` location,
/// imported into a second workspace on the same host. The import leaves the
/// location behind, and the session resolves its own on its first commit.
#[test]
fn a_published_session_imports_into_another_workspace_on_the_same_host() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let (a, b) = (h.workspace("ws-a"), h.workspace("ws-b"));
    let (wf_a, wf_b) = (mk(h.home.join("wf-a")), mk(h.home.join("wf-b")));
    source(
        &h,
        &a,
        &[("KOTO_WORKFLOWS_DIR", wf_a.to_str().unwrap())],
        &wf_a,
    );

    let env_b = [("KOTO_WORKFLOWS_DIR", wf_b.to_str().unwrap())];
    let out = h.koto(
        &b,
        &["session", "import", NAME, "--from", a.to_str().unwrap()],
        &env_b,
    );
    assert!(out.ok(), "import: {}", out.describe());
    assert_eq!(out.json()["keys"], 1, "{}", out.describe());
    assert_resolves_its_own_location(&h, &b, &env_b, &wf_b);
    assert_bucket_manifest_whole(&s3, &b);
}

/// The same session imported from a second `HOME`, whose Claude Code
/// session koto finds from the environment: the location it publishes is
/// under that `HOME`, never the source's.
#[test]
fn a_published_session_imports_from_another_home() {
    let s3 = FakeS3::start(BUCKET);
    let ha = Host::new(&s3);
    let a = ha.workspace("ws");
    let wf_a = mk(ha.home.join("wf-a"));
    source(
        &ha,
        &a,
        &[("KOTO_WORKFLOWS_DIR", wf_a.to_str().unwrap())],
        &wf_a,
    );

    let hb = Host::new(&s3);
    let b = hb.workspace("ws");
    let project = mk(hb.home.join(".claude").join("projects").join("proj"));
    std::fs::write(project.join("sess-b.jsonl"), b"").unwrap();
    let env_b = [("CLAUDE_CODE_SESSION_ID", "sess-b")];
    let out = hb.koto(
        &b,
        &[
            "session",
            "import",
            NAME,
            "--from",
            a.to_str().unwrap(),
            "--trust-template",
        ],
        &env_b,
    );
    assert!(out.ok(), "import: {}", out.describe());
    assert_eq!(out.json()["keys"], 1, "{}", out.describe());
    assert_resolves_its_own_location(&hb, &b, &env_b, &project.join("sess-b").join("workflows"));
    assert_eq!(
        rendered(&wf_a).len(),
        1,
        "the destination wrote on the source host"
    );
    assert_bucket_manifest_whole(&s3, &b);
}

/// A bucket koto 0.19.0 wrote lists the location in the manifest without
/// its object, which made every import refuse `import_source_unreadable`.
/// The import now leaves the entry behind, on the same host and from
/// another `HOME`.
#[test]
fn an_import_skips_a_location_an_older_koto_listed_without_its_object() {
    for same_host in [true, false] {
        let s3 = FakeS3::start(BUCKET);
        let ha = Host::new(&s3);
        let a = ha.workspace("ws-a");
        let wf_a = mk(ha.home.join("wf-a"));
        source(
            &ha,
            &a,
            &[("KOTO_WORKFLOWS_DIR", wf_a.to_str().unwrap())],
            &wf_a,
        );

        // List the location in the bucket's manifest as 0.19.0 did, with
        // the local entry and no object.
        let local: serde_json::Value = serde_json::from_slice(
            &std::fs::read(ha.session_dir().join("ctx").join("manifest.json")).unwrap(),
        )
        .unwrap();
        let mut remote: serde_json::Value =
            serde_json::from_slice(&s3.object(&key(&a, "ctx/manifest.json")).unwrap()).unwrap();
        remote["keys"][LOCATION] = local["keys"][LOCATION].clone();
        assert!(remote["keys"][LOCATION].is_object());
        s3.put(
            &key(&a, "ctx/manifest.json"),
            &serde_json::to_vec_pretty(&remote).unwrap(),
        );
        assert!(s3.object(&key(&a, &format!("ctx/{LOCATION}"))).is_none());

        let hb;
        let (host, b) = if same_host {
            (&ha, ha.workspace("ws-b"))
        } else {
            hb = Host::new(&s3);
            let b = hb.workspace("ws-b");
            (&hb, b)
        };
        let wf_b = mk(host.home.join("wf-b"));
        let env_b = [("KOTO_WORKFLOWS_DIR", wf_b.to_str().unwrap())];
        let out = host.koto(
            &b,
            &[
                "session",
                "import",
                NAME,
                "--from",
                a.to_str().unwrap(),
                "--trust-template",
            ],
            &env_b,
        );
        assert!(
            out.ok(),
            "import (same host: {same_host}): {}",
            out.describe()
        );
        assert_eq!(out.json()["keys"], 1, "{}", out.describe());
        assert_resolves_its_own_location(host, &b, &env_b, &wf_b);
        assert_bucket_manifest_whole(&s3, &b);
    }
}

const CHILD_TEMPLATE: &str = r#"---
name: batch-child
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

Do the work.

## done

Done.
"#;

const PARENT_TEMPLATE: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          gates.done.all_complete: true
  summarize:
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: closed
        when:
          status: ok
  closed:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize.

## closed

Closed.
"#;

/// The other key koto writes for itself, `batch_final_view`, goes through
/// the cloud backend: it reaches the bucket with its object, and the
/// location the batch's sessions publish stays off every manifest.
#[test]
fn batch_final_view_reaches_the_bucket_with_its_object() {
    let s3 = FakeS3::start(BUCKET);
    let h = Host::new(&s3);
    let ws = h.workspace("ws");
    std::fs::write(ws.join("child.md"), CHILD_TEMPLATE).unwrap();
    std::fs::write(ws.join("parent.md"), PARENT_TEMPLATE).unwrap();
    let wf = mk(h.home.join("wf"));
    let env = [("KOTO_WORKFLOWS_DIR", wf.to_str().unwrap())];
    let run = |args: &[&str]| {
        let out = h.koto(&ws, args, &env);
        assert!(out.ok(), "{args:?}: {}", out.describe());
        out
    };

    run(&["init", "parent", "--template", "parent.md"]);
    let tasks = serde_json::json!({
        "tasks": [
            {"name": "A", "waits_on": [], "vars": {}},
            {"name": "B", "waits_on": [], "vars": {}},
        ]
    });
    run(&["next", "parent", "--with-data", &tasks.to_string()]);
    for child in ["parent.A", "parent.B"] {
        run(&[
            "next",
            child,
            "--no-cleanup",
            "--with-data",
            r#"{"marker":"done"}"#,
        ]);
    }
    let out = run(&["next", "parent"]);
    assert_eq!(out.json()["state"], "summarize", "{}", out.describe());

    let keys = assert_session_manifest_whole(&s3, &ws, "parent");
    assert!(
        keys.iter().any(|k| k == "batch_final_view"),
        "the bucket's manifest lacks batch_final_view: {keys:?}"
    );
    for child in ["parent.A", "parent.B"] {
        if s3
            .object(&session_key(&ws, child, "ctx/manifest.json"))
            .is_some()
        {
            assert_session_manifest_whole(&s3, &ws, child);
        }
    }
    let out = run(&["context", "get", "parent", LOCATION]);
    assert_eq!(out.stdout.trim(), wf.to_str().unwrap());
}
