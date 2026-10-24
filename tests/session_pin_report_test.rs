//! What koto reports about pins (DESIGN-session-pin.md): the hierarchy
//! query's pin, release record and pinned members, `koto session list
//! --pinned`, unreadable and unindexed pins through every removal, the
//! clocks a pin leaves alone, a pinned child its lineage no longer reaches,
//! and a pinned session also held by a live anchor. Driven through the real
//! binary in a scratch HOME on the local backend.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// A session that runs until `marker: done` reaches its terminal.
const WORK: &str = r#"---
name: pin-report
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

const REASON: &str = "record of the merged change";

struct Env {
    _tmp: TempDir,
    root: PathBuf,
    home: PathBuf,
    cache: PathBuf,
    template: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("work.md");
        std::fs::write(&template, WORK).unwrap();
        Env {
            _tmp: tmp,
            root,
            home,
            cache,
            template,
            work,
        }
    }

    /// A command in the workspace with a one-second retention and no
    /// anchor, agent session or backend inherited, then `env` on top.
    fn cmd(&self, env: &[(&str, &str)]) -> assert_cmd::Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        self.cmd(env).args(args).output().unwrap()
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.run_env(args, &[])
    }

    fn ok_env(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run_env(args, env);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_env(args, &[])
    }

    fn text(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "koto {args:?} failed: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// The JSON on stdout of a command expected to exit `code`.
    fn fails(&self, args: &[&str], code: i32) -> Value {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// A running session, optionally a koto child of `parent`.
    fn running(&self, name: &str, parent: Option<&str>) {
        self.running_env(name, parent, &[]);
    }

    fn running_env(&self, name: &str, parent: Option<&str>, env: &[(&str, &str)]) {
        let template = self.template.to_str().unwrap();
        let mut args = vec!["init", name, "--template", template];
        if let Some(p) = parent {
            args.extend(["--parent", p]);
        }
        self.ok_env(&args, env);
    }

    /// Tick `name` to its terminal.
    fn tick(&self, name: &str, no_cleanup: bool) -> Value {
        let mut args = vec!["next", name, "--with-data", r#"{"marker":"done"}"#];
        if no_cleanup {
            args.push("--no-cleanup");
        }
        self.ok(&args)
    }

    /// A finished session, kept on disk.
    fn done(&self, name: &str, parent: Option<&str>) {
        self.running(name, parent);
        self.tick(name, true);
    }

    fn pin(&self, name: &str) -> Value {
        self.ok(&["session", "pin", name, "--reason", REASON])["pin"].clone()
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).is_dir()
    }

    fn pin_file(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/links/pin")
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    /// SHA-256 of every file in session `name`'s directory, by relative
    /// path; a symlink hashes as its target path, never followed.
    fn hashes(&self, name: &str) -> BTreeMap<PathBuf, String> {
        let root = self.sessions().join(name);
        let mut out = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                let bytes = if meta.is_dir() {
                    stack.push(p);
                    continue;
                } else if meta.file_type().is_symlink() {
                    std::fs::read_link(&p)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                        .into_bytes()
                } else {
                    std::fs::read(&p).unwrap()
                };
                out.insert(
                    p.strip_prefix(&root).unwrap().to_path_buf(),
                    hex::encode(Sha256::digest(bytes)),
                );
            }
        }
        out
    }

    fn hierarchy(&self, name: &str) -> Value {
        self.ok(&["session", "hierarchy", name, "--json"])
    }

    fn pinned(&self) -> Value {
        self.ok(&["session", "list", "--pinned", "--json"])
    }

    /// Drop `parent_workflow` from `name`'s header, so its lineage to its
    /// parent is gone and only its name and pin remain.
    fn drop_lineage(&self, name: &str) {
        let path = self.state_file(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let (head, rest) = text.split_once('\n').unwrap();
        let mut header: Value = serde_json::from_str(head).unwrap();
        assert!(header.get("parent_workflow").is_some(), "{header}");
        header.as_object_mut().unwrap().remove("parent_workflow");
        std::fs::write(&path, format!("{header}\n{rest}")).unwrap();
    }
}

fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(1300));
}

fn names(list: &Value) -> Vec<String> {
    let mut names: Vec<String> = list
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|r| match r {
            Value::String(s) => s.clone(),
            r => r["name"].as_str().unwrap().to_string(),
        })
        .collect();
    names.sort();
    names
}

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no entry for {name} in {list}"))
}

/// The entry names `pin`: `kept_by_pin`, and the pin's reason, setter, time
/// and source.
fn assert_names_pin(e: &Value, pin: &Value) {
    assert_eq!(e["kept_by_pin"], true, "{e}");
    assert_eq!(e["pin"]["reason"], REASON, "{e}");
    assert_eq!(e["pin"]["set_by"], Value::Null, "{e}");
    assert_eq!(e["pin"]["set_at"], pin["set_at"], "{e}");
    assert_eq!(e["pin"]["source"], "local", "{e}");
}

/// The entry names an unreadable pin: `kept_by_pin` and `pin_unreadable`.
fn assert_unreadable(e: &Value, problem: &str) {
    assert_eq!(e["kept_by_pin"], true, "{e}");
    assert_eq!(e["pin_unreadable"], problem, "{e}");
    assert!(e.get("pin").is_none(), "{e}");
}

/// The JSON object a command printed last, after any preview text.
fn last_json(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text.lines().last().unwrap_or_default();
    let start = line
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in {text}"));
    serde_json::from_str(&line[start..]).unwrap()
}

/// `v` with every `now` field removed, at any depth: the one field that
/// differs between two runs over an unchanged store.
fn without_now(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| k.as_str() != "now")
                .map(|(k, v)| (k.clone(), without_now(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_now).collect()),
        v => v.clone(),
    }
}

#[test]
fn the_hierarchy_query_shows_the_pin_its_release_and_the_pinned_members() {
    let env = Env::new();
    env.done("A", None);
    env.done("B", None);
    env.ok(&["session", "link", "B", "--dispatched-by", "A"]);
    let pin = env.pin("A");
    let (a, b) = (env.hashes("A"), env.hashes("B"));

    // A's own query: the pin, and that it keeps A only.
    let h = env.hierarchy("A");
    assert_names_pin(&h, &pin);
    assert_eq!(h["pin_keeps"], serde_json::json!(["A"]), "{h}");
    assert!(h.get("pinned_members").is_none(), "{h}");
    assert!(h.get("pin_release").is_none(), "{h}");
    let text = env.text(&["session", "hierarchy", "A"]);
    assert!(text.contains("A: kept by its pin"), "{text}");
    assert!(
        text.contains(&format!(
            "pinned: {REASON} (set by hand at {})",
            pin["set_at"].as_str().unwrap()
        )),
        "{text}"
    );
    assert!(text.contains("the pin keeps A only"), "{text}");

    // B's query lists A as a pinned member and carries no pin of its own.
    let h = env.hierarchy("B");
    assert_eq!(h["pinned_members"], serde_json::json!(["A"]), "{h}");
    for field in ["kept_by_pin", "pin", "pin_unreadable", "pin_keeps"] {
        assert!(h.get(field).is_none(), "{field} in {h}");
    }
    let text = env.text(&["session", "hierarchy", "B"]);
    assert!(
        text.contains("pinned members, each kept by its own pin: A"),
        "{text}"
    );

    // The queries wrote nothing.
    assert_eq!(env.hashes("A"), a);
    assert_eq!(env.hashes("B"), b);

    // After an unpin, A's query shows its latest release record.
    env.ok(&["session", "unpin", "A", "--reason", "the change shipped"]);
    let h = env.hierarchy("A");
    assert!(h.get("kept_by_pin").is_none(), "{h}");
    let r = &h["pin_release"];
    assert_eq!(r["released_by"], Value::Null, "{h}");
    assert_eq!(r["authority"], Value::Null, "{h}");
    assert_eq!(r["reason"], "the change shipped", "{h}");
    assert!(r["released_at"].as_str().unwrap().ends_with('Z'), "{h}");
    assert_eq!(r["pin"]["reason"], REASON, "{h}");
    assert_eq!(r["pin"]["set_by"], Value::Null, "{h}");
    assert_eq!(r["pin"]["set_at"], pin["set_at"], "{h}");
    let text = env.text(&["session", "hierarchy", "A"]);
    assert!(
        text.contains("pin released by hand at")
            && text.contains("(reason: the change shipped)")
            && text.contains(&format!("the pin it released: {REASON} (set by hand at")),
        "{text}"
    );
    assert!(env.hierarchy("B").get("pinned_members").is_none());

    // A session never pinned prints none of these blocks.
    env.done("N", None);
    let h = env.hierarchy("N");
    for field in [
        "kept_by_pin",
        "pin",
        "pin_unreadable",
        "pin_keeps",
        "pinned_members",
        "pin_release",
    ] {
        assert!(h.get(field).is_none(), "{field} in {h}");
    }
}

#[test]
fn the_pin_listing_lists_exactly_the_pinned_sessions_sorted_by_name() {
    let env = Env::new();
    // None pinned: an empty list, exit 0.
    assert_eq!(env.pinned(), serde_json::json!([]));
    assert_eq!(
        env.text(&["session", "list", "--pinned"]),
        "no pinned sessions\n"
    );

    for i in 0..10 {
        env.done(&format!("u{i}"), None);
    }
    env.running("c-run", None);
    env.done("a-done", None);
    env.done("b-bad", None);
    let run_pin = env.pin("c-run");
    let done_pin = env.pin("a-done");
    env.pin("b-bad");
    std::fs::write(env.pin_file("b-bad"), b"not a pin").unwrap();

    let list = env.pinned();
    let rows = list.as_array().unwrap();
    let order: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(order, vec!["a-done", "b-bad", "c-run"], "{list}");

    let a = &rows[0];
    assert_eq!(a["state"], "finished", "{a}");
    assert_eq!(a["pin"]["reason"], REASON, "{a}");
    assert_eq!(a["pin"]["set_at"], done_pin["set_at"], "{a}");
    assert_eq!(a["source"], "local", "{a}");
    let b = &rows[1];
    assert_eq!(b["state"], "finished", "{b}");
    assert_eq!(b["pin_unreadable"], "unparseable", "{b}");
    assert!(b.get("pin").is_none(), "{b}");
    let c = &rows[2];
    assert_eq!(c["state"], "running", "{c}");
    assert_eq!(c["pin"]["set_at"], run_pin["set_at"], "{c}");
    for row in rows {
        assert!(row.get("kept_by_pin").is_none(), "{row}");
    }

    let text = env.text(&["session", "list", "--pinned"]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(
        lines[0].starts_with("a-done (finished): pinned: "),
        "{text}"
    );
    assert_eq!(lines[1], "b-bad (finished): pin unreadable: unparseable");
    assert!(lines[2].starts_with("c-run (running): pinned: "), "{text}");

    // The flag stands alone.
    assert_eq!(
        env.run(&["session", "list", "--pinned", "--state"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        env.run(&["session", "list", "--json"]).status.code(),
        Some(2)
    );
}

/// Each way a pin can be there and unreadable, and the name koto reports.
const DAMAGED: [&str; 5] = [
    "unparseable",
    "too_large",
    "not_a_file:symlink",
    "not_a_file:directory",
    "missing_content",
];

/// Replace `name`'s pin by the damaged form `form`, keeping its index entry.
fn damage(env: &Env, name: &str, form: &str) {
    let path = env.pin_file(name);
    std::fs::remove_file(&path).unwrap();
    match form {
        "unparseable" => std::fs::write(&path, b"{\"v\": 1, \"reason\": ").unwrap(),
        "too_large" => {
            let pin = serde_json::json!({
                "v": 1,
                "id": "0123456789abcdef0123456789abcdef",
                "reason": "x".repeat(5 * 1024),
                "set_by": null,
                "set_at": "2026-01-01T00:00:00Z",
            });
            std::fs::write(&path, pin.to_string()).unwrap();
        }
        "not_a_file:symlink" => {
            let target = env.root.join(format!("{name}-pin.json"));
            std::fs::write(
                &target,
                serde_json::json!({
                    "v": 1,
                    "id": "0123456789abcdef0123456789abcdef",
                    "reason": "elsewhere",
                    "set_by": null,
                    "set_at": "2026-01-01T00:00:00Z",
                })
                .to_string(),
            )
            .unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
        }
        "not_a_file:directory" => {
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("inner"), b"{}").unwrap();
        }
        "missing_content" => {}
        other => panic!("no form {other}"),
    }
}

#[test]
fn each_unreadable_pin_keeps_its_session_through_every_removal() {
    for form in DAMAGED {
        let problem = form.split(':').next().unwrap();
        let env = Env::new();
        // X is P's koto parent; D, finished, was dispatched by P.
        env.running("X", None);
        env.running("P", Some("X"));
        env.done("D", None);
        env.ok(&["session", "link", "D", "--dispatched-by", "P"]);
        env.pin("P");
        damage(&env, "P", form);

        // S1: P's terminal tick without --no-cleanup keeps it.
        let s1 = env.tick("P", false);
        assert_eq!(s1["retention"]["retained"], true, "{form}: {s1}");
        assert_unreadable(&s1["retention"], problem);
        let held = env.hashes("P");

        // S2: X's terminal tick lists P among the children it keeps.
        let s2 = env.tick("X", false);
        assert_unreadable(entry(&s2["kept_children"], "P"), problem);

        // The explicit removals refuse it.
        for args in [
            vec!["session", "cleanup", "P"],
            vec!["cancel", "P", "--cleanup"],
        ] {
            let refused = env.fails(&args, 2);
            assert_eq!(refused["error"]["code"], "pin_unreadable", "{form}");
            assert_unreadable(&refused["error"], problem);
        }
        assert_eq!(env.hashes("P"), held, "{form}");

        settle();
        // S4: workspace prune of X keeps P and removes X.
        let out = env.run(&["workspace", "prune", "--root", "X", "--yes"]);
        assert!(out.status.success(), "{form}: {out:?}");
        let out = last_json(&out.stdout);
        assert_eq!(names(&out["removed"]), vec!["X"], "{form}: {out}");
        assert_unreadable(entry(&out["kept"], "P"), problem);

        // S3: the prune keeps P by its pin and removes what P dispatched.
        let preview = env.ok(&["session", "prune", "--all"]);
        assert_unreadable(entry(&preview["kept"], "P"), problem);
        let s3 = env.ok(&["session", "prune", "--apply", "--all"]);
        let kept = entry(&s3["kept"], "P");
        assert_eq!(kept["rule"], "p", "{form}");
        assert_unreadable(kept, problem);
        assert_eq!(names(&s3["removed"]), vec!["D"], "{form}: {s3}");
        assert_eq!(env.hashes("P"), held, "{form}");

        // The listing reports it.
        let list = env.pinned();
        assert_eq!(list[0]["name"], "P", "{form}: {list}");
        assert_eq!(list[0]["pin_unreadable"], problem, "{form}: {list}");

        // S6: the replace moves it aside, naming the unreadable pin.
        let template = env.template.to_str().unwrap();
        let s6 = env.ok(&["init", "P", "--template", template, "--replace-terminal"]);
        let moved = s6["moved_to"].as_str().unwrap().to_string();
        assert_unreadable(&s6, problem);
        let h = env.hierarchy(&moved);
        assert_unreadable(&h, problem);

        // Pinning it is refused, naming the unreadable pin; an unpin by hand
        // clears it, with the released pin's fields null.
        let refused = env.fails(&["session", "pin", &moved, "--reason", REASON], 2);
        assert_eq!(refused["error"]["code"], "pin_unreadable", "{form}");
        assert_eq!(refused["error"]["pin_unreadable"], problem, "{form}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains(problem),
            "{form}: {refused}"
        );
        let released = env.ok(&["session", "unpin", &moved]);
        let pin = &released["released"]["pin"];
        for field in ["reason", "set_by", "set_at"] {
            assert_eq!(pin[field], Value::Null, "{form}: {released}");
        }
        assert!(
            std::fs::symlink_metadata(env.pin_file(&moved)).is_err(),
            "{form}"
        );
        assert_eq!(env.pinned(), serde_json::json!([]), "{form}");
    }
}

#[test]
fn a_pin_file_the_index_does_not_list_is_a_readable_pin() {
    let env = Env::new();
    env.done("P", None);
    let pin = env.pin("P");
    let manifest = env.sessions().join("P/ctx/manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert!(m["keys"]
        .as_object_mut()
        .unwrap()
        .remove("koto/links/pin")
        .is_some());
    std::fs::write(&manifest, m.to_string()).unwrap();
    let held = env.hashes("P");
    settle();

    let h = env.hierarchy("P");
    assert_names_pin(&h, &pin);
    let listed = env.pinned();
    assert_eq!(listed[0]["pin"]["set_at"], pin["set_at"], "{listed}");
    let refused = env.fails(&["session", "cleanup", "P"], 2);
    assert_eq!(refused["error"]["code"], "pinned", "{refused}");
    let pruned = env.ok(&["session", "prune", "--apply", "--all"]);
    assert_names_pin(entry(&pruned["kept"], "P"), &pin);
    assert_eq!(env.hashes("P"), held);

    // The replace moves it aside with its pin.
    let template = env.template.to_str().unwrap();
    let out = env.ok(&["init", "P", "--template", template, "--replace-terminal"]);
    assert_names_pin(&out, &pin);
}

#[test]
fn pin_records_move_no_clock() {
    let env = Env::new();
    // An unobserved anchor that idles closed after a second holds P.
    let idle = [("KOTO_RETENTION_IDLE_PERIOD", "1s")];
    env.ok_env(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "S1",
            "--source",
            "startup",
        ],
        &idle,
    );
    env.running_env("P", None, &[("KOTO_ANCHOR", "anchor-S1")]);
    env.tick("P", true);
    settle();

    let query = || env.ok_env(&["session", "hierarchy", "P", "--json"], &idle);
    let clocks = |h: &Value| {
        let p = entry(&h["members"], "P");
        let a = entry(&h["members"], "anchor-S1");
        assert!(a["last_activity"].as_str().is_some(), "{h}");
        (
            p["stopped_at"].clone(),
            h["retention"]["release"].clone(),
            a["last_activity"].clone(),
            a["stopped_at"].clone(),
        )
    };
    let before = clocks(&query());
    assert!(before.0.is_string() && before.1.is_object(), "{before:?}");
    let log = std::fs::read(env.state_file("P")).unwrap();
    let anchor = env.hashes("anchor-S1");

    env.pin("P");
    let pinned = query();
    assert_eq!(clocks(&pinned), before);
    env.ok(&["session", "unpin", "P"]);
    let released = query();
    assert_eq!(clocks(&released), before);
    assert!(released["pin_release"].is_object(), "{released}");

    assert_eq!(std::fs::read(env.state_file("P")).unwrap(), log);
    assert_eq!(env.hashes("anchor-S1"), anchor);
}

#[test]
fn a_pinned_child_whose_lineage_was_removed_is_listed_by_its_parent() {
    // X's terminal tick: X is kept with C1 and C3, and lists C2 by its pin.
    let env = Env::new();
    env.running("X", None);
    for c in ["C1", "C2", "C3"] {
        env.done(c, Some("X"));
    }
    let pin = env.pin("C2");
    env.drop_lineage("C2");
    let c2 = env.hashes("C2");
    let out = env.tick("X", false);
    assert_eq!(out["retention"]["retained"], true, "{out}");
    let kept = entry(&out["kept_children"], "C2");
    assert_eq!(kept["reason"], "pinned", "{out}");
    assert_names_pin(kept, &pin);
    for c in ["C1", "C3"] {
        assert!(entry(&out["kept_children"], c).get("kept_by_pin").is_none());
    }

    // koto workspace prune lists it as kept and removes the rest.
    settle();
    let out = env.run(&["workspace", "prune", "--root", "X", "--yes"]);
    assert!(out.status.success(), "{out:?}");
    let out = last_json(&out.stdout);
    assert_eq!(names(&out["removed"]), vec!["C1", "C3", "X"], "{out}");
    let kept = entry(&out["kept"], "C2");
    assert_names_pin(kept, &pin);
    assert!(env.exists("C2"));
    assert_eq!(env.hashes("C2"), c2);

    // A released same-name replace lists it too.
    let env = Env::new();
    env.done("X", None);
    for c in ["C1", "C2", "C3"] {
        env.done(c, Some("X"));
    }
    let pin = env.pin("C2");
    env.drop_lineage("C2");
    let c2 = env.hashes("C2");
    settle();
    let template = env.template.to_str().unwrap();
    let out = env.ok(&["init", "X", "--template", template, "--replace-terminal"]);
    assert_eq!(out["outcome"], "replaced", "{out}");
    assert_names_pin(entry(&out["kept_children"], "C2"), &pin);
    assert!(!env.exists("C1") && !env.exists("C3"));
    assert_eq!(env.hashes("C2"), c2);
}

#[test]
fn a_pinned_session_held_by_a_live_anchor_reports_the_pin_and_the_anchor() {
    let env = Env::new();
    env.ok(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        "S1",
        "--source",
        "startup",
    ]);
    env.running_env("P", None, &[("KOTO_ANCHOR", "anchor-S1")]);
    env.tick("P", true);
    settle();

    let keepers_before = env.hierarchy("P")["keepers"].clone();
    let preview = env.ok(&["session", "prune", "--all"]);
    let anchor_before = without_now(entry(&preview["kept"], "anchor-S1"));

    let pin = env.pin("P");
    let h = env.hierarchy("P");
    assert_names_pin(&h, &pin);
    assert_eq!(h["kept"], true, "{h}");
    assert_eq!(h["keepers"], keepers_before, "{h}");
    assert_eq!(names(&h["keepers"]), vec!["anchor-S1"], "{h}");
    let text = env.text(&["session", "hierarchy", "P"]);
    assert!(text.contains("pinned: "), "{text}");
    assert!(
        text.contains("alive in its hierarchy: anchor-S1 (anchor)"),
        "{text}"
    );

    let preview = env.ok(&["session", "prune", "--all"]);
    let p = entry(&preview["kept"], "P");
    assert_eq!(p["rule"], "p", "{p}");
    assert_names_pin(p, &pin);
    assert_eq!(p["hierarchy"]["keepers"], keepers_before, "{p}");
    assert_eq!(
        without_now(entry(&preview["kept"], "anchor-S1")),
        anchor_before
    );
}
