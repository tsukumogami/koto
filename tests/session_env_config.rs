//! Session settings from `KOTO_SESSION_*` variables, driven through the
//! binary: the origin report in `koto config list`, the refusals
//! `build_backend` makes, the koto credential pair at the endpoint, and the
//! settings reaching the commands a tick runs.
//!
//! Every command starts from a disposable `HOME` and workspace, with every
//! `KOTO_SESSION_*`, `AWS_*` and decider variable removed from its
//! environment, so the ambient environment never leaks in; a test sets only
//! what it exercises. The cloud cases use the in-process endpoint in
//! `tests/support/fake_s3.rs`, which records the access key each request was
//! signed with.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/project_trust.rs"]
mod project_trust;

use fake_s3::FakeS3;
use std::path::PathBuf;

const BUCKET: &str = "koto-env";

/// Every variable a test might set; each command removes them all first.
const CLEARED: &[&str] = &[
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_CLOUD_ENDPOINT",
    "KOTO_SESSION_CLOUD_BUCKET",
    "KOTO_SESSION_CLOUD_REGION",
    "KOTO_SESSION_CLOUD_PATH_STYLE",
    "KOTO_SESSION_CLOUD_ACCESS_KEY",
    "KOTO_SESSION_CLOUD_SECRET_KEY",
    "KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SECURITY_TOKEN",
    "AWS_PROFILE",
    "KOTO_SESSIONS_BASE",
    "KOTO_DECIDER",
    "KOTO_DECIDER_API_KEY",
    "KOTO_DECIDER_ENDPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_WORKFLOWS_DIR",
];

/// A template whose first state waits on a context key, then evidence.
const TEMPLATE: &str = r#"---
name: env-demo
version: "1.0"
initial_state: start
states:
  start:
    gates:
      notes:
        type: context-exists
        key: notes.md
    transitions:
      - target: wait
        when:
          gates.notes.exists: true
  wait:
    accepts:
      choice:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          choice: go
  done:
    terminal: true
---

## start

Gather notes.

## wait

Wait.

## done

Done.
"#;

/// One disposable host: `HOME`, cache and a workspace.
struct Box_ {
    _root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    ws: PathBuf,
}

impl Box_ {
    fn new() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let canon = |p: PathBuf| {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::canonicalize(p).unwrap()
        };
        let home = canon(root.path().join("home"));
        let cache = canon(root.path().join("cache"));
        let ws = canon(root.path().join("ws"));
        std::fs::write(ws.join("demo.md"), TEMPLATE).unwrap();
        Box_ {
            _root: root,
            home,
            cache,
            ws,
        }
    }

    fn user_file(&self, body: &str) {
        let dir = self.home.join(".koto");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
    }

    fn project_file(&self, body: &str) {
        let dir = self.ws.join(".koto");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
        project_trust::trust(&self.home.join(".koto"), &self.ws);
    }

    fn koto(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = koto_command::koto();
        for k in CLEARED {
            cmd.env_remove(k);
        }
        cmd.current_dir(&self.ws)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.args(args).output().unwrap()
    }

    fn sessions(&self) -> Vec<String> {
        let dir = self.home.join(".koto").join("sessions");
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn describe(o: &std::process::Output) -> String {
    format!(
        "exit {:?}; stdout: {}; stderr: {}",
        o.status.code(),
        stdout(o).trim(),
        stderr(o).trim()
    )
}

/// The environment that puts a box on the fake endpoint with the koto pair.
fn cloud_env(s3: &FakeS3) -> Vec<(&'static str, String)> {
    vec![
        ("KOTO_SESSION_BACKEND", "cloud".to_string()),
        ("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint()),
        ("KOTO_SESSION_CLOUD_BUCKET", s3.bucket().to_string()),
        ("KOTO_SESSION_CLOUD_REGION", "us-east-1".to_string()),
        ("KOTO_SESSION_CLOUD_PATH_STYLE", "true".to_string()),
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY",
            "koto-access-key-id".to_string(),
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY",
            "koto-secret-key-value".to_string(),
        ),
    ]
}

fn as_refs<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    env.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

fn list_json(b: &Box_, env: &[(&str, &str)]) -> serde_json::Value {
    let out = b.koto(&["config", "list", "--json"], env);
    assert!(out.status.success(), "config list: {}", describe(&out));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn origin(list: &serde_json::Value, setting: &str) -> (String, Option<String>) {
    let o = &list["origins"][setting];
    (
        o["source"].as_str().unwrap_or("").to_string(),
        o["variable"].as_str().map(str::to_string),
    )
}

// ---------------------------------------------------------------------------
// Origins
// ---------------------------------------------------------------------------

#[test]
fn config_list_reports_every_source() {
    let b = Box_::new();
    b.user_file("[session.cloud]\nregion = \"user-region\"\n");
    b.project_file("[session.cloud]\nbucket = \"project-bucket\"\n");
    let list = list_json(
        &b,
        &[
            ("KOTO_SESSION_BACKEND", "cloud"),
            ("AWS_ACCESS_KEY_ID", "aws-access-key-id"),
            ("AWS_SECRET_ACCESS_KEY", "aws-secret-key-value"),
        ],
    );
    let o = list["origins"].as_object().expect("an origins object");
    // Nine session settings and the three retention settings.
    assert_eq!(o.len(), 12, "{list}");
    assert_eq!(
        origin(&list, "session.backend"),
        ("env".into(), Some("KOTO_SESSION_BACKEND".into()))
    );
    assert_eq!(
        origin(&list, "session.cloud.bucket"),
        ("project".into(), None)
    );
    assert_eq!(origin(&list, "session.cloud.region"), ("user".into(), None));
    assert_eq!(
        origin(&list, "session.cloud.endpoint"),
        ("unset".into(), None)
    );
    assert_eq!(
        origin(&list, "session.cloud.path_style"),
        ("default".into(), None)
    );
    assert_eq!(
        origin(&list, "session.cloud.marker_retain_days"),
        ("default".into(), None)
    );
    assert_eq!(
        origin(&list, "session.cloud.access_key"),
        ("env".into(), Some("AWS_ACCESS_KEY_ID".into()))
    );
    // The tables keep their shape, and the credentials stay redacted.
    assert_eq!(list["session"]["backend"], "cloud");
    assert_eq!(list["session"]["cloud"]["bucket"], "project-bucket");
    assert_eq!(list["session"]["cloud"]["access_key"], "<set>");
    assert!(!list.to_string().contains("aws-secret-key-value"));
}

#[test]
fn the_koto_pair_is_reported_under_its_own_names() {
    let b = Box_::new();
    let list = list_json(
        &b,
        &[
            ("KOTO_SESSION_CLOUD_ACCESS_KEY", "koto-access-key-id"),
            ("KOTO_SESSION_CLOUD_SECRET_KEY", "koto-secret-key-value"),
        ],
    );
    assert_eq!(
        origin(&list, "session.cloud.secret_key"),
        ("env".into(), Some("KOTO_SESSION_CLOUD_SECRET_KEY".into()))
    );
    assert_eq!(list["session"]["cloud"]["secret_key"], "<set>");
    assert!(!list.to_string().contains("koto-secret-key-value"));
}

#[test]
fn toml_output_parses_to_the_same_tables_with_origin_comments() {
    let b = Box_::new();
    b.user_file("[session]\nbackend = \"local\"\n");
    let toml_out = b.koto(&["config", "list"], &[("KOTO_SESSION_CLOUD_BUCKET", "b")]);
    assert!(toml_out.status.success(), "{}", describe(&toml_out));
    let text = stdout(&toml_out);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("# origin: "))
        .collect();
    assert_eq!(lines.len(), 12, "{text}");
    assert!(
        lines.contains(&"# origin: session.backend = user"),
        "{text}"
    );
    assert!(
        lines.contains(&"# origin: session.cloud.bucket = env KOTO_SESSION_CLOUD_BUCKET"),
        "{text}"
    );
    // The comments change nothing the TOML parses to: it equals the JSON
    // form's tables.
    let parsed: toml::Value = text.parse().expect("valid TOML");
    let mut json = list_json(&b, &[("KOTO_SESSION_CLOUD_BUCKET", "b")]);
    json.as_object_mut().unwrap().remove("origins");
    json.as_object_mut().unwrap().remove("project_config");
    let as_json: serde_json::Value = serde_json::to_value(&parsed).unwrap();
    let strip_nulls = |v: &serde_json::Value| -> serde_json::Value {
        fn walk(v: &serde_json::Value) -> serde_json::Value {
            match v {
                serde_json::Value::Object(m) => serde_json::Value::Object(
                    m.iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), walk(v)))
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        walk(v)
    };
    assert_eq!(strip_nulls(&as_json), strip_nulls(&json));
}

#[test]
fn a_session_table_without_backend_reports_default() {
    let b = Box_::new();
    b.user_file("[session]\n[session.cloud]\nregion = \"r\"\n");
    b.project_file("[session]\n");
    let list = list_json(&b, &[]);
    assert_eq!(origin(&list, "session.backend"), ("default".into(), None));
    let b = Box_::new();
    b.project_file("session.backend = \"local\"\n");
    let list = list_json(&b, &[]);
    assert_eq!(origin(&list, "session.backend"), ("project".into(), None));
}

#[test]
fn marker_retain_days_takes_no_variable() {
    let b = Box_::new();
    b.user_file("[session.cloud]\nmarker_retain_days = 7\n");
    let env = [("KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS", "1")];
    let list = list_json(&b, &env);
    assert_eq!(
        origin(&list, "session.cloud.marker_retain_days"),
        ("user".into(), None)
    );
    let out = b.koto(&["config", "get", "session.cloud.marker_retain_days"], &env);
    assert_eq!(stdout(&out).trim(), "7");
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn an_unusable_backend_or_path_style_refuses_and_config_still_shows_it() {
    for (var, value) in [
        ("KOTO_SESSION_BACKEND", "s3"),
        ("KOTO_SESSION_BACKEND", "Cloud"),
        ("KOTO_SESSION_CLOUD_PATH_STYLE", "yes"),
    ] {
        let b = Box_::new();
        let env = [(var, value)];
        for args in [
            vec!["init", "demo", "--template", "demo.md"],
            vec!["next", "demo"],
        ] {
            let out = b.koto(&args, &env);
            assert!(
                !out.status.success(),
                "{var}={value} {args:?}: {}",
                describe(&out)
            );
            let err = stderr(&out) + &stdout(&out);
            assert!(
                err.contains(var) && err.contains(value),
                "{var}={value}: {err}"
            );
        }
        assert!(
            b.sessions().is_empty(),
            "{var}={value} created {:?}",
            b.sessions()
        );
        let list = list_json(&b, &env);
        assert_eq!(
            origin(
                &list,
                if var.ends_with("BACKEND") {
                    "session.backend"
                } else {
                    "session.cloud.path_style"
                }
            )
            .0,
            "env"
        );
        if var.ends_with("BACKEND") {
            assert_eq!(list["session"]["backend"], value);
            let got = b.koto(&["config", "get", "session.backend"], &env);
            assert!(got.status.success());
            assert_eq!(stdout(&got).trim(), value);
        } else {
            assert_eq!(
                list["origins"]["session.cloud.path_style"]["invalid"],
                "\"yes\""
            );
        }
    }
}

#[test]
fn a_padded_backend_value_is_trimmed() {
    let b = Box_::new();
    let list = list_json(&b, &[("KOTO_SESSION_BACKEND", " local ")]);
    assert_eq!(list["session"]["backend"], "local");
    let out = b.koto(
        &["init", "demo", "--template", "demo.md"],
        &[("KOTO_SESSION_BACKEND", " local ")],
    );
    assert!(out.status.success(), "{}", describe(&out));
}

#[test]
fn the_variable_wins_over_a_project_file_and_empty_falls_through() {
    let s3 = FakeS3::start(BUCKET);
    let b = Box_::new();
    b.project_file("[session]\nbackend = \"local\"\n");
    let mut env = cloud_env(&s3);
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!s3.keys().is_empty(), "the cloud backend pushed nothing");

    for empty in ["", "  "] {
        env.retain(|(k, _)| *k != "KOTO_SESSION_BACKEND");
        env.push(("KOTO_SESSION_BACKEND", empty.to_string()));
        let list = list_json(&b, &as_refs(&env));
        assert_eq!(list["session"]["backend"], "local", "{empty:?}");
        assert_eq!(origin(&list, "session.backend"), ("project".into(), None));
    }
}

#[test]
fn a_half_pair_refuses_before_any_request() {
    for (set, missing) in [
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY",
            "KOTO_SESSION_CLOUD_SECRET_KEY",
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY",
            "KOTO_SESSION_CLOUD_ACCESS_KEY",
        ),
    ] {
        let s3 = FakeS3::start(BUCKET);
        let b = Box_::new();
        let mut env: Vec<(&'static str, String)> = cloud_env(&s3)
            .into_iter()
            .filter(|(k, _)| *k != missing)
            .collect();
        env.push(("AWS_ACCESS_KEY_ID", "aws-access-key-id".to_string()));
        env.push(("AWS_SECRET_ACCESS_KEY", "aws-secret-key-value".to_string()));
        let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
        assert!(!out.status.success(), "{set}: {}", describe(&out));
        assert!(stderr(&out).contains(missing), "{set}: {}", describe(&out));
        assert!(s3.requests().is_empty(), "{set}: {:?}", s3.requests());

        // The local backend doesn't need the pair.
        env.retain(|(k, _)| *k != "KOTO_SESSION_BACKEND");
        let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
        assert!(out.status.success(), "{set} on local: {}", describe(&out));
    }
}

#[test]
fn the_pair_is_never_sent_to_a_project_file_destination() {
    let s3 = FakeS3::start(BUCKET);
    let b = Box_::new();
    b.project_file(&format!(
        "[session.cloud]\nendpoint = {:?}\n",
        "http://127.0.0.1:9"
    ));
    let env: Vec<(&'static str, String)> = cloud_env(&s3)
        .into_iter()
        .filter(|(k, _)| *k != "KOTO_SESSION_CLOUD_ENDPOINT")
        .collect();
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(
        stderr(&out).contains("session.cloud.endpoint"),
        "{}",
        describe(&out)
    );
    assert!(s3.requests().is_empty());

    let env = cloud_env(&s3);
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
    assert!(out.status.success(), "{}", describe(&out));

    // The bucket the same way.
    let b = Box_::new();
    b.project_file("[session.cloud]\nbucket = \"elsewhere\"\n");
    s3.clear_requests();
    let env: Vec<(&'static str, String)> = cloud_env(&s3)
        .into_iter()
        .filter(|(k, _)| *k != "KOTO_SESSION_CLOUD_BUCKET")
        .collect();
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(
        stderr(&out).contains("session.cloud.bucket"),
        "{}",
        describe(&out)
    );
    assert!(s3.requests().is_empty());
}

// ---------------------------------------------------------------------------
// The cloud backend from the environment alone
// ---------------------------------------------------------------------------

#[test]
fn env_only_cloud_syncs_and_signs_with_the_koto_pair() {
    let s3 = FakeS3::start(BUCKET);
    let b = Box_::new();
    b.user_file(
        "[session.cloud]\naccess_key = \"file-access-key\"\nsecret_key = \"file-secret-key\"\n",
    );
    let mut env = cloud_env(&s3);
    env.push(("AWS_ACCESS_KEY_ID", "aws-access-key-id".to_string()));
    env.push(("AWS_SECRET_ACCESS_KEY", "aws-secret-key-value".to_string()));
    let env = as_refs(&env);
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &env);
    assert!(out.status.success(), "{}", describe(&out));
    let mut add = koto_command::koto();
    for k in CLEARED {
        add.env_remove(k);
    }
    let added = add
        .current_dir(&b.ws)
        .env("HOME", &b.home)
        .env("XDG_CACHE_HOME", &b.cache)
        .envs(env.iter().copied())
        .args(["context", "add", "demo", "notes.md"])
        .write_stdin("hello")
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", describe(&added));
    let out = b.koto(&["next", "demo"], &env);
    assert!(out.status.success(), "{}", describe(&out));

    let requests = s3.requests();
    assert!(!requests.is_empty());
    for r in &requests {
        assert_eq!(
            r.access_key.as_deref(),
            Some("koto-access-key-id"),
            "{} {} signed with {:?}",
            r.method,
            r.key,
            r.access_key
        );
    }
    assert!(
        s3.keys()
            .iter()
            .any(|k| k.ends_with("/ctx/notes.md") || k.contains("notes.md")),
        "the context key reached the bucket: {:?}",
        s3.keys()
    );
}

#[test]
fn empty_koto_credentials_fall_through_to_the_aws_pair() {
    let s3 = FakeS3::start(BUCKET);
    let b = Box_::new();
    let mut env: Vec<(&'static str, String)> = cloud_env(&s3)
        .into_iter()
        .filter(|(k, _)| !k.ends_with("_KEY"))
        .collect();
    env.push(("KOTO_SESSION_CLOUD_ACCESS_KEY", String::new()));
    env.push(("KOTO_SESSION_CLOUD_SECRET_KEY", " ".to_string()));
    env.push(("AWS_ACCESS_KEY_ID", "aws-access-key-id".to_string()));
    env.push(("AWS_SECRET_ACCESS_KEY", "aws-secret-key-value".to_string()));
    let out = b.koto(&["init", "demo", "--template", "demo.md"], &as_refs(&env));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(s3
        .requests()
        .iter()
        .all(|r| r.access_key.as_deref() == Some("aws-access-key-id")));
}

/// The case the coordinate workflow relies on: a gate that calls koto
/// reaches the same cloud backend as the tick that runs it, in a default
/// session (whose commands get only a minimal environment) and in a legacy
/// one. The default session is the discriminating half: its commands see
/// none of the caller's variables unless koto passes them. A legacy
/// session's commands inherit the caller's environment, so that half pins
/// that passing the settings again doesn't break it. The gate checks that the nested koto resolved the cloud backend and
/// read the key through it; without the settings in its environment it
/// would see the local backend, and without the pair it couldn't
/// authenticate.
#[test]
fn a_gate_that_calls_koto_reaches_the_cloud_backend() {
    let koto_bin = koto_command::koto_bin();
    let bin_dir = koto_bin.parent().unwrap().to_path_buf();
    for legacy in [false, true] {
        let s3 = FakeS3::start(BUCKET);
        let b = Box_::new();
        let template = TEMPLATE
            .replace(
                "          gates.notes.exists: true\n",
                "          gates.notes.exists: true\n          gates.nested.exit_code: 0\n",
            )
            .replace(
            "      notes:\n        type: context-exists\n        key: notes.md\n",
            "      notes:\n        type: context-exists\n        key: notes.md\n      nested:\n        type: command\n        command: \"test \\\"$(koto config get session.backend)\\\" = cloud && koto context get gated notes.md | grep -q hello && echo \\\"$KOTO_SESSION_CLOUD_SECRET_KEY\\\"\"\n",
        );
        std::fs::write(b.ws.join("gated.md"), template).unwrap();
        let path = format!("{}:/usr/bin:/bin", bin_dir.display());
        let mut env = cloud_env(&s3);
        env.push(("PATH", path));
        let env = as_refs(&env);
        let mut init: Vec<&str> = vec!["init", "gated", "--template", "gated.md"];
        if legacy {
            init.push("--legacy-environment");
        }
        let out = b.koto(&init, &env);
        assert!(out.status.success(), "legacy={legacy}: {}", describe(&out));
        let mut add = koto_command::koto();
        for k in CLEARED {
            add.env_remove(k);
        }
        let added = add
            .current_dir(&b.ws)
            .env("HOME", &b.home)
            .env("XDG_CACHE_HOME", &b.cache)
            .envs(env.iter().copied())
            .args(["context", "add", "gated", "notes.md"])
            .write_stdin("hello")
            .output()
            .unwrap();
        assert!(added.status.success(), "{}", describe(&added));
        let out = b.koto(&["next", "gated"], &env);
        let text = stdout(&out);
        assert!(out.status.success(), "legacy={legacy}: {}", describe(&out));
        let resp: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or_default();
        assert_eq!(
            resp["state"],
            "wait",
            "legacy={legacy}: the gate calling koto must pass: {}",
            describe(&out)
        );
        assert!(
            !text.contains("koto-secret-key-value"),
            "legacy={legacy}: the secret reached the response unredacted"
        );
    }
}

#[test]
fn credentials_from_the_pair_are_redacted_in_command_output() {
    let b = Box_::new();
    let template = r#"---
name: leaky
version: "1.0"
initial_state: run
states:
  run:
    gates:
      leak:
        type: command
        command: "cat secrets.txt; echo \"env=$KOTO_SESSION_CLOUD_SECRET_KEY\"; exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run.

## done

Done.
"#;
    std::fs::write(b.ws.join("leaky.md"), template).unwrap();
    // The file holds the values as set (padded) and as koto uses them
    // (trimmed); the gate also echoes the variable koto passes it.
    std::fs::write(
        b.ws.join("secrets.txt"),
        "padded=[ koto-secret-key-value ] trimmed=koto-secret-key-value id=koto-access-key-id\n",
    )
    .unwrap();
    let env = [
        ("KOTO_SESSION_CLOUD_ACCESS_KEY", "koto-access-key-id"),
        ("KOTO_SESSION_CLOUD_SECRET_KEY", " koto-secret-key-value "),
    ];
    let out = b.koto(&["init", "leaky", "--template", "leaky.md"], &env);
    assert!(out.status.success(), "{}", describe(&out));
    let out = b.koto(&["next", "leaky"], &env);
    let all = stdout(&out) + &stderr(&out);
    assert!(!all.contains("koto-secret-key-value"), "{all}");
    assert!(!all.contains("koto-access-key-id"), "{all}");
    assert!(
        all.contains("[REDACTED:KOTO_SESSION_CLOUD_SECRET_KEY]"),
        "the secret is labeled with its variable: {all}"
    );
    assert!(
        all.contains("[REDACTED:KOTO_SESSION_CLOUD_ACCESS_KEY]"),
        "the access key is labeled with its variable: {all}"
    );
    assert!(
        all.contains("env=[REDACTED:KOTO_SESSION_CLOUD_SECRET_KEY]"),
        "the gate received the secret from koto and its echo is redacted: {all}"
    );
}

#[test]
fn credentials_stay_out_of_the_project_file() {
    let b = Box_::new();
    let out = b.koto(&["config", "set", "session.cloud.access_key", "x"], &[]);
    assert!(!out.status.success(), "{}", describe(&out));
}
