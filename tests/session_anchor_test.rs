//! Integration tests for agent-session anchors through the CLI:
//! `koto session anchor open|observe|close`, the held-by links and
//! activity events `koto init` and `koto next` record from `KOTO_ANCHOR`,
//! the refusal of `koto next` on anchors, and their absence from the
//! workflow views.
//!
//! Every held-by scenario checks the single-writer rule with SHA-256
//! hashes: each session other than the one the command opened or ticked,
//! and the running agent session's own anchor, is byte-identical before
//! and after.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_fs::TempDir;
use sha2::{Digest, Sha256};

// ===== Harness =====

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = koto_command::koto();
    for var in [
        "CLAUDE_CODE_SESSION_ID",
        "KOTO_SESSION_BACKEND",
        "KOTO_ANCHOR",
        "KOTO_DISPATCHER_ANCHOR",
        "KOTO_RETENTION_IDLE_PERIOD",
        "KOTO_WORKFLOWS_DIR",
    ] {
        cmd.env_remove(var);
    }
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let mut cmd = koto_cmd(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.args(args).output().unwrap();
    Out {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn run(dir: &Path, args: &[&str]) -> Out {
    run_env(dir, args, &[])
}

fn ok(out: Out, what: &str) -> Out {
    assert_eq!(out.code, 0, "{what}\n{}\n{}", out.stdout, out.stderr);
    out
}

/// The `key=value` lines a verb printed.
fn lines(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn open(dir: &Path, harness: &str, extra: &[&str]) -> BTreeMap<String, String> {
    let mut args = vec![
        "session",
        "anchor",
        "open",
        "--harness-session",
        harness,
        "--source",
        "startup",
    ];
    args.extend_from_slice(extra);
    lines(&ok(run(dir, &args), "anchor open").stdout)
}

const TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
        when:
          status: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("scope.md"), TEMPLATE).unwrap();
    tmp
}

fn init_env(dir: &Path, name: &str, env: &[(&str, &str)]) -> Out {
    let template = dir.join("scope.md");
    run_env(
        dir,
        &["init", name, "--template", template.to_str().unwrap()],
        env,
    )
}

fn sessions(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

fn link_exists(dir: &Path, session: &str, kind: &str, peer: &str) -> bool {
    sessions(dir)
        .join(session)
        .join("ctx/koto/links")
        .join(kind)
        .join(peer)
        .is_file()
}

fn held_by(dir: &Path, session: &str) -> Vec<String> {
    let mut out: Vec<String> =
        std::fs::read_dir(sessions(dir).join(session).join("ctx/koto/links/held-by"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.ends_with(".lock"))
                    .collect()
            })
            .unwrap_or_default();
    out.sort();
    out
}

/// The events of `name`'s log with type `ty`.
fn events_of_type(dir: &Path, name: &str, ty: &str) -> Vec<serde_json::Value> {
    let path = sessions(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == ty)
        .collect()
}

/// SHA-256 of every file of `session`, by relative path.
fn hashes(dir: &Path, session: &str) -> BTreeMap<PathBuf, String> {
    let root = sessions(dir).join(session);
    let mut out = BTreeMap::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let digest = Sha256::digest(std::fs::read(&p).unwrap());
                out.insert(
                    p.strip_prefix(&root).unwrap().to_path_buf(),
                    format!("{digest:x}"),
                );
            }
        }
    }
    out
}

/// Hashes of every session in the store except `except`.
fn store_hashes(dir: &Path, except: &[&str]) -> BTreeMap<String, BTreeMap<PathBuf, String>> {
    std::fs::read_dir(sessions(dir))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !except.contains(&n.as_str()))
        .map(|n| {
            let h = hashes(dir, &n);
            (n, h)
        })
        .collect()
}

// ===== The verbs =====

#[test]
fn the_verbs_print_their_lines_and_a_repeat_close_changes_nothing() {
    let tmp = setup();
    let dir = tmp.path();
    let first = open(dir, "h1", &[]);
    assert_eq!(first["anchor"], "anchor-h1");
    assert_eq!(first["opened"], "new");
    assert_eq!(first["observed"], "no");
    assert!(!first.contains_key("previous"));

    let again = open(dir, "h1", &[]);
    assert_eq!(again["opened"], "continued");

    let obs = lines(
        &ok(
            run(
                dir,
                &[
                    "session",
                    "anchor",
                    "observe",
                    "--anchor",
                    "anchor-h1",
                    "--key=k1",
                ],
            ),
            "observe",
        )
        .stdout,
    );
    assert_eq!(obs["anchor"], "anchor-h1");
    assert_eq!(obs["observed"], "yes");

    let anchor_before = hashes(dir, "anchor-h1");
    let close = |dir: &Path| {
        ok(
            run(
                dir,
                &[
                    "session",
                    "anchor",
                    "close",
                    "--key=k1",
                    "--harness-session=h1",
                ],
            ),
            "close",
        )
        .stdout
    };
    assert_eq!(close(dir).trim(), "closed=1");
    assert_eq!(hashes(dir, "anchor-h1"), anchor_before);
    let closed = hashes(dir, "anchor-h1-closed");
    assert_eq!(close(dir).trim(), "closed=0");
    assert_eq!(hashes(dir, "anchor-h1-closed"), closed);
    assert_eq!(hashes(dir, "anchor-h1"), anchor_before);

    // The closed anchor gets a successor on the next start.
    let succ = open(dir, "h1", &[]);
    assert_eq!(succ["anchor"], "anchor-h1-s2");
    assert_eq!(succ["opened"], "successor");
    assert!(link_exists(
        dir,
        "anchor-h1-s2",
        "dispatched-by",
        "anchor-h1"
    ));
}

#[test]
fn a_failed_open_prints_no_anchor_line() {
    let tmp = setup();
    let dir = tmp.path();
    for args in [
        vec!["--harness-session", "h1-closed", "--source", "startup"],
        vec!["--harness-session", "bad id", "--source", "startup"],
        vec![
            "--harness-session",
            "h1",
            "--source",
            "startup",
            "--observed-key",
            "a b",
        ],
    ] {
        let mut full = vec!["session", "anchor", "open"];
        full.extend(args.iter().copied());
        let out = run(dir, &full);
        assert_eq!(out.code, 2, "{full:?}\n{}\n{}", out.stdout, out.stderr);
        assert!(
            !out.stdout.lines().any(|l| l.starts_with("anchor=")),
            "{}",
            out.stdout
        );
    }
    // A name held by an ordinary session is refused too.
    ok(init_env(dir, "anchor-taken", &[]), "init");
    let out = run(
        dir,
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "taken",
            "--source",
            "startup",
        ],
    );
    assert_eq!(out.code, 2);
    assert!(!out.stdout.contains("anchor="));
}

#[test]
fn a_clear_prints_the_predecessor_and_the_inherit_deadline() {
    let tmp = setup();
    let dir = tmp.path();
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 30;
    let token = format!("host:77:{start}");
    open(dir, "h1", &["--process", &token]);
    let out = ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "open",
                "--harness-session",
                "h2",
                "--source",
                "clear",
                "--process",
                &token,
            ],
        ),
        "clear open",
    );
    let l = lines(&out.stdout);
    assert_eq!(l["anchor"], "anchor-h2");
    assert_eq!(l["opened"], "successor");
    assert_eq!(l["previous"], "anchor-h1");
    assert!(l["inherit_until"].ends_with('Z'));
    // Inherit: the predecessor gets a key, the successor copies it.
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "observe",
                "--anchor",
                "anchor-h1",
                "--key=k9",
            ],
        ),
        "observe",
    );
    let inh = lines(
        &ok(
            run(
                dir,
                &[
                    "session",
                    "anchor",
                    "observe",
                    "--anchor",
                    "anchor-h2",
                    "--inherit",
                ],
            ),
            "inherit",
        )
        .stdout,
    );
    assert_eq!(inh["observed"], "yes");
}

#[test]
fn open_ignores_koto_anchor_so_an_anchor_holds_no_held_by_link() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "a1", &[]);
    let a1 = hashes(dir, "anchor-a1");
    let out = run_env(
        dir,
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "a2",
            "--source",
            "startup",
        ],
        &[("KOTO_ANCHOR", "anchor-a1")],
    );
    ok(out, "open");
    assert!(held_by(dir, "anchor-a2").is_empty());
    assert_eq!(hashes(dir, "anchor-a1"), a1);
}

// ===== held-by from KOTO_ANCHOR =====

#[test]
fn init_and_next_write_one_held_by_link_per_anchor_and_touch_nothing_else() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "a1", &[]);
    open(dir, "a2", &[]);
    ok(init_env(dir, "bystander", &[]), "init bystander");

    let before = store_hashes(dir, &["wf", "anchor-a1"]);
    let out = ok(
        init_env(dir, "wf", &[("KOTO_ANCHOR", "anchor-a1")]),
        "init wf",
    );
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    assert_eq!(held_by(dir, "wf"), vec!["anchor-a1"]);
    assert_eq!(store_hashes(dir, &["wf", "anchor-a1"]), before);

    // A tick under the same anchor adds nothing.
    let wf_links = held_by(dir, "wf");
    ok(
        run_env(dir, &["next", "wf"], &[("KOTO_ANCHOR", "anchor-a1")]),
        "next",
    );
    assert_eq!(held_by(dir, "wf"), wf_links);
    assert_eq!(store_hashes(dir, &["wf", "anchor-a1"]), before);

    // A tick under a second anchor adds a second link.
    let before = store_hashes(dir, &["wf", "anchor-a2"]);
    ok(
        run_env(dir, &["next", "wf"], &[("KOTO_ANCHOR", "anchor-a2")]),
        "next a2",
    );
    assert_eq!(held_by(dir, "wf"), vec!["anchor-a1", "anchor-a2"]);
    assert_eq!(store_hashes(dir, &["wf", "anchor-a2"]), before);

    // The anchors themselves hold no held-by links.
    assert!(held_by(dir, "anchor-a1").is_empty());
    assert!(held_by(dir, "anchor-a2").is_empty());
}

#[test]
fn a_closed_anchor_with_a_successor_holds_through_the_successor_too() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "h1", &["--observed-key", "k1"]);
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k1",
                "--harness-session=h1",
            ],
        ),
        "close",
    );
    assert_eq!(open(dir, "h1", &[])["anchor"], "anchor-h1-s2");
    let before = store_hashes(dir, &["wf", "anchor-h1-s2"]);
    ok(init_env(dir, "wf", &[("KOTO_ANCHOR", "anchor-h1")]), "init");
    assert_eq!(held_by(dir, "wf"), vec!["anchor-h1", "anchor-h1-s2"]);
    assert_eq!(store_hashes(dir, &["wf", "anchor-h1-s2"]), before);
}

#[test]
fn activity_events_are_throttled_to_a_tenth_of_the_idle_period() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "a1", &[]);
    let env = [
        ("KOTO_ANCHOR", "anchor-a1"),
        ("KOTO_RETENTION_IDLE_PERIOD", "10s"),
    ];
    ok(init_env(dir, "wf", &env), "init");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        ok(run_env(dir, &["next", "wf"], &env), "next");
        std::thread::sleep(Duration::from_millis(200));
    }
    let activity = events_of_type(dir, "anchor-a1", "anchor_activity");
    assert!(
        !activity.is_empty(),
        "the 10s idle period lets some through"
    );
    let times: Vec<std::time::SystemTime> = activity
        .iter()
        .map(|e| parse_ts(e["timestamp"].as_str().unwrap()))
        .collect();
    for pair in times.windows(2) {
        let gap = pair[1].duration_since(pair[0]).unwrap();
        assert!(gap >= Duration::from_secs(1), "gap {gap:?} in {activity:?}");
    }
    // At most one per second over the run, plus the init's.
    let span = start.elapsed().as_secs() + 2;
    assert!(activity.len() as u64 <= span, "{} events", activity.len());
    assert_eq!(activity[0]["payload"]["session"], "wf");
}

fn parse_ts(ts: &str) -> std::time::SystemTime {
    // YYYY-MM-DDTHH:MM:SS.mmmZ
    let (date, time) = ts.trim_end_matches('Z').split_once('T').unwrap();
    let d: Vec<i64> = date.split('-').map(|p| p.parse().unwrap()).collect();
    let (hms, ms) = time.split_once('.').unwrap_or((time, "0"));
    let t: Vec<i64> = hms.split(':').map(|p| p.parse().unwrap()).collect();
    let (y, m, day) = (d[0], d[1], d[2]);
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + t[0] * 3600 + t[1] * 60 + t[2];
    std::time::UNIX_EPOCH
        + Duration::from_secs(secs as u64)
        + Duration::from_millis(ms.parse().unwrap())
}

#[test]
fn only_koto_anchor_counts_never_the_dispatcher_variable() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "a1", &[]);
    let anchor = hashes(dir, "anchor-a1");
    let env = [
        ("KOTO_DISPATCHER_ANCHOR", "anchor-a1"),
        ("KOTO_RETENTION_IDLE_PERIOD", "1s"),
    ];
    std::thread::sleep(Duration::from_millis(300));
    ok(init_env(dir, "wf", &env), "init");
    ok(run_env(dir, &["next", "wf"], &env), "next");
    assert!(held_by(dir, "wf").is_empty());
    assert_eq!(hashes(dir, "anchor-a1"), anchor);
}

#[test]
fn init_with_no_anchor_named_succeeds_silently() {
    let tmp = setup();
    let dir = tmp.path();
    let out = ok(init_env(dir, "wf1", &[]), "init");
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    assert!(held_by(dir, "wf1").is_empty());

    ok(init_env(dir, "plain", &[]), "init plain");
    for value in ["anchor-nobody", "plain", "not a name", ""] {
        let name = format!("wf-{}", value.len());
        let out = ok(init_env(dir, &name, &[("KOTO_ANCHOR", value)]), "init");
        assert!(out.stderr.is_empty(), "{value:?}: {}", out.stderr);
        assert!(held_by(dir, &name).is_empty(), "{value:?}");
    }
}

// ===== Refusals and views =====

#[test]
fn next_refuses_anchors_and_close_sessions() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "h1", &["--observed-key", "k1"]);
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k1",
                "--harness-session=h1",
            ],
        ),
        "close",
    );
    for name in ["anchor-h1", "anchor-h1-closed"] {
        let before = hashes(dir, name);
        let out = run(dir, &["next", name]);
        assert_eq!(out.code, 2, "{name}: {}\n{}", out.stdout, out.stderr);
        assert!(out.stdout.contains("bookkeeping"), "{}", out.stdout);
        assert_eq!(hashes(dir, name), before);
    }
}

#[test]
fn anchors_are_left_out_of_workflows_and_listed_with_their_kind() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "h1", &["--observed-key", "k1"]);
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k1",
                "--harness-session=h1",
            ],
        ),
        "close",
    );
    ok(init_env(dir, "wf", &[]), "init");

    let wfs: serde_json::Value =
        serde_json::from_str(&ok(run(dir, &["workflows"]), "workflows").stdout).unwrap();
    let names: Vec<&str> = wfs
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["wf"]);

    let list: serde_json::Value =
        serde_json::from_str(&ok(run(dir, &["session", "list"]), "list").stdout).unwrap();
    let kinds: BTreeMap<String, Option<String>> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_string(),
                s.get("kind").and_then(|k| k.as_str()).map(str::to_string),
            )
        })
        .collect();
    assert_eq!(kinds["anchor-h1"].as_deref(), Some("anchor"));
    assert_eq!(kinds["anchor-h1-closed"].as_deref(), Some("anchor-close"));
    assert_eq!(kinds["wf"], None);
}

#[test]
fn the_workflows_materializer_skips_anchors() {
    let tmp = setup();
    let dir = tmp.path();
    let wf_dir = dir.join("wfdir");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let env = [("KOTO_WORKFLOWS_DIR", wf_dir.to_str().unwrap())];
    let open_out = run_env(
        dir,
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "h1",
            "--source",
            "startup",
            "--observed-key",
            "k1",
        ],
        &env,
    );
    ok(open_out, "open");
    ok(
        run_env(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k1",
                "--harness-session=h1",
            ],
            &env,
        ),
        "close",
    );
    assert_eq!(std::fs::read_dir(&wf_dir).unwrap().count(), 0);
    assert!(!dir
        .join("sessions/anchor-h1/ctx/workflows/publish-location")
        .exists());

    // An ordinary session in the same setup does render.
    ok(init_env(dir, "wf", &env), "init");
    ok(
        run_env(
            dir,
            &["context", "add", "wf", "note.md", "--from-file", "scope.md"],
            &env,
        ),
        "context add",
    );
    assert_eq!(std::fs::read_dir(&wf_dir).unwrap().count(), 1);
}

/// Copy the directory `from` to `to`, files and subdirectories.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

#[test]
fn a_leftover_staging_entry_is_invisible_to_list_and_prune_and_is_swept() {
    let tmp = setup();
    let dir = tmp.path();
    open(dir, "h1", &["--observed-key", "k1"]);
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k1",
                "--harness-session=h1",
            ],
        ),
        "close",
    );
    // A complete close session left under the staging prefix, as a close
    // interrupted before its rename leaves it.
    let staging = sessions(dir).join(".koto-staging-anchor-h9-closed-0000");
    copy_dir(
        &sessions(dir).join("anchor-h1-closed"),
        &staging.join("anchor-h1-closed"),
    );

    let list = ok(run(dir, &["session", "list"]), "list").stdout;
    assert!(!list.contains("koto-staging"), "{list}");
    let prune = ok(run(dir, &["session", "prune", "--all"]), "prune").stdout;
    assert!(!prune.contains("koto-staging"), "{prune}");
    let wfs = ok(run(dir, &["workflows"]), "workflows").stdout;
    assert!(!wfs.contains("koto-staging"), "{wfs}");

    // A close while the entry is fresh leaves it; once it is older than ten
    // minutes, the next close sweeps it.
    open(dir, "h2", &["--observed-key", "k2"]);
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k2",
                "--harness-session=h2",
            ],
        ),
        "close h2",
    );
    assert!(staging.exists());
    let old = std::time::SystemTime::now() - Duration::from_secs(11 * 60);
    std::fs::File::open(&staging)
        .unwrap()
        .set_modified(old)
        .unwrap();
    ok(
        run(
            dir,
            &[
                "session",
                "anchor",
                "close",
                "--key=k2",
                "--harness-session=h2",
            ],
        ),
        "close again",
    );
    assert!(!staging.exists());
}
