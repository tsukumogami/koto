//! End-to-end coverage for the session execution anchor (R11-R16,
//! Decisions 6 and 7 of DESIGN-koto-runs-commands.md).
//!
//! What anchoring promises is that every tick of a session happens from
//! the directory the session is bound to, and that the session's
//! commands run at that directory rather than wherever `koto next` was
//! typed. What it does not promise -- and what nothing here asserts --
//! is any bound on where a command can go once it is running.
//!
//! Every case drives the real binary, because the property under test
//! is about the process working directory and only the binary has one.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Sessions live outside every anchor directory these tests create, so
/// deleting an anchor never takes the session log with it.
fn sessions_base(home: &Path) -> PathBuf {
    let base = home.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// A `koto` invocation whose process working directory is `cwd` and
/// whose session storage is rooted at `home`.
fn koto_cmd(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(cwd);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(home));
    cmd.env("HOME", home);
    cmd
}

struct Run {
    success: bool,
    code: Option<i32>,
    stdout: String,
    json: serde_json::Value,
}

fn run_koto(home: &Path, cwd: &Path, args: &[&str]) -> Run {
    let output = koto_cmd(home, cwd).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
    });
    Run {
        success: output.status.success(),
        code: output.status.code(),
        stdout,
        json,
    }
}

fn state_path(home: &Path, name: &str) -> PathBuf {
    sessions_base(home)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name))
}

fn state_lines(home: &Path, name: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(state_path(home, name))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("state file line should be JSON"))
        .collect()
}

fn header(home: &Path, name: &str) -> serde_json::Value {
    state_lines(home, name).remove(0)
}

fn events(home: &Path, name: &str) -> Vec<serde_json::Value> {
    let mut lines = state_lines(home, name);
    lines.remove(0);
    lines
}

fn event_types(home: &Path, name: &str) -> Vec<String> {
    events(home, name)
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A workflow whose one action writes a marker file at a RELATIVE path,
/// so where the file lands is the answer to "which directory did the
/// command run in".
const MARKER_TEMPLATE: &str = r#"---
name: marker
version: "1.0"
initial_state: mark
states:
  mark:
    default_action:
      command: "printf ok > marker.txt"
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## mark

Write the marker.

## done

Done.
"#;

const PLAIN_TEMPLATE: &str = r#"---
name: plain
version: "1.0"
initial_state: start
states:
  start:
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

Decide.

## done

Done.
"#;

/// Init a session anchored at `anchor`, with the template written
/// somewhere else entirely so template location and anchor never get
/// conflated.
fn init_at(home: &Path, anchor: &Path, name: &str, template: &str) -> Run {
    let src = home.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    let run = run_koto(
        home,
        anchor,
        &["init", name, "--template", src.to_str().unwrap()],
    );
    assert!(run.success, "init failed: {}", run.stdout);
    run
}

/// Strip `execution_dir` from a session's header, reproducing a state
/// file written by a koto build from before anchoring existed.
fn strip_anchor(home: &Path, name: &str) {
    let path = state_path(home, name);
    let contents = std::fs::read_to_string(&path).unwrap();
    let mut lines = contents.lines();
    let mut header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    header.as_object_mut().unwrap().remove("execution_dir");
    let rest: Vec<&str> = lines.collect();
    let mut out = serde_json::to_string(&header).unwrap();
    for line in rest {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    std::fs::write(&path, out).unwrap();
}

fn dir(base: &Path, name: &str) -> PathBuf {
    let path = base.join(name);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(&path).unwrap()
}

// ---------------------------------------------------------------------------
// R11: the anchor is recorded at init
// ---------------------------------------------------------------------------

#[test]
fn init_records_the_directory_it_ran_in() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");

    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);

    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(anchor.to_str().unwrap())
    );
}

#[test]
fn execution_dir_flag_overrides_the_init_directory() {
    let home = TempDir::new().unwrap();
    let where_i_stand = dir(home.path(), "elsewhere");
    let chosen = dir(home.path(), "checkout");

    let src = home.path().join("tpl.md");
    std::fs::write(&src, PLAIN_TEMPLATE).unwrap();
    let run = run_koto(
        home.path(),
        &where_i_stand,
        &[
            "init",
            "wf",
            "--template",
            src.to_str().unwrap(),
            "--execution-dir",
            chosen.to_str().unwrap(),
        ],
    );
    assert!(run.success, "init failed: {}", run.stdout);

    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(chosen.to_str().unwrap())
    );

    // And the flag is what the tick is judged against: the directory
    // init actually ran in is now the wrong tree.
    let refused = run_koto(home.path(), &where_i_stand, &["next", "wf"]);
    assert!(!refused.success);
    assert_eq!(refused.json["error"]["code"], "execution_anchor_mismatch");
}

#[test]
fn a_named_execution_dir_that_does_not_resolve_is_a_caller_error() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let src = home.path().join("tpl.md");
    std::fs::write(&src, PLAIN_TEMPLATE).unwrap();

    let run = run_koto(
        home.path(),
        &anchor,
        &[
            "init",
            "wf",
            "--template",
            src.to_str().unwrap(),
            "--execution-dir",
            "/definitely/not/a/directory/koto",
        ],
    );
    assert!(!run.success);
    assert_eq!(run.code, Some(2));
    assert!(
        run.json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("--execution-dir"),
        "error must name the flag, got {}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// R12: every tick is checked, and the tick runs at the anchor
// ---------------------------------------------------------------------------

#[test]
fn a_tick_from_a_different_tree_is_refused_and_names_the_anchor() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let other = dir(home.path(), "other-checkout");
    init_at(home.path(), &anchor, "wf", MARKER_TEMPLATE);

    let before = event_types(home.path(), "wf");
    let run = run_koto(home.path(), &other, &["next", "wf"]);

    assert!(!run.success);
    assert_eq!(run.code, Some(2));
    assert_eq!(run.json["error"]["code"], "execution_anchor_mismatch");
    let message = run.json["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(anchor.to_str().unwrap()),
        "refusal must name the bound directory, got {}",
        message
    );

    // Nothing ran, nothing moved: no action, no gate, no transition.
    assert!(
        !anchor.join("marker.txt").exists() && !other.join("marker.txt").exists(),
        "a refused tick must not execute the state's action"
    );
    assert_eq!(
        event_types(home.path(), "wf"),
        before,
        "a refused tick must append no event"
    );
}

#[test]
fn a_tick_from_a_subdirectory_is_accepted_and_runs_at_the_anchor() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let nested = dir(&anchor, "src");
    init_at(home.path(), &anchor, "wf", MARKER_TEMPLATE);

    let run = run_koto(home.path(), &nested, &["next", "wf"]);
    assert!(run.success, "tick from a subdirectory must be accepted");

    assert!(
        anchor.join("marker.txt").exists(),
        "the command must run at the anchor"
    );
    assert!(
        !nested.join("marker.txt").exists(),
        "the command must not run at the process working directory"
    );
}

#[test]
fn a_tick_through_a_symlink_to_the_anchor_is_accepted() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let link = home.path().join("link-to-checkout");
    std::os::unix::fs::symlink(&anchor, &link).unwrap();
    init_at(home.path(), &anchor, "wf", MARKER_TEMPLATE);

    let run = run_koto(home.path(), &link, &["next", "wf"]);
    assert!(
        run.success,
        "canonicalization resolves the link, so this is the anchor: {}",
        run.stdout
    );
    assert!(anchor.join("marker.txt").exists());
}

#[test]
fn a_sibling_directory_sharing_a_name_prefix_is_refused() {
    // Containment is compared component-wise, so `checkout-2` is not
    // beneath `checkout` even though its path string starts with it.
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let sibling = dir(home.path(), "checkout-2");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);

    let run = run_koto(home.path(), &sibling, &["next", "wf"]);
    assert!(!run.success);
    assert_eq!(run.json["error"]["code"], "execution_anchor_mismatch");
}

#[test]
fn the_check_runs_on_every_tick_not_only_the_first() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let other = dir(home.path(), "other-checkout");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);

    let first = run_koto(home.path(), &anchor, &["next", "wf"]);
    assert!(first.success, "first tick at the anchor: {}", first.stdout);

    let second = run_koto(home.path(), &other, &["next", "wf"]);
    assert!(
        !second.success,
        "moving between ticks must be caught on the second tick"
    );
    assert_eq!(second.json["error"]["code"], "execution_anchor_mismatch");
}

// ---------------------------------------------------------------------------
// R15: a recorded anchor that no longer resolves
// ---------------------------------------------------------------------------

#[test]
fn an_anchor_that_no_longer_resolves_is_a_distinct_refusal() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let elsewhere = dir(home.path(), "elsewhere");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
    let before = event_types(home.path(), "wf");

    std::fs::remove_dir_all(&anchor).unwrap();

    let run = run_koto(home.path(), &elsewhere, &["next", "wf"]);
    assert!(!run.success);
    assert_eq!(run.code, Some(3));
    assert_eq!(
        run.json["error"]["code"], "execution_anchor_unresolvable",
        "a deleted checkout must be distinguishable from the wrong tree by code, not wording"
    );
    let message = run.json["error"]["message"].as_str().unwrap();
    assert!(message.contains(anchor.to_str().unwrap()));
    assert!(
        message.contains("rebind"),
        "the refusal must point at the rebind verb, got {}",
        message
    );
    assert_eq!(
        event_types(home.path(), "wf"),
        before,
        "a refused tick must append no event"
    );
}

// ---------------------------------------------------------------------------
// R13: rebinding a session whose checkout moved
//
// Both refusals above name `koto session rebind` as the repair, so
// these cases are the other half of those: a refusal a developer can
// actually act on, and a move that lands in the log rather than
// happening quietly.
// ---------------------------------------------------------------------------

/// The `execution_anchor_rebound` payloads in a session's log, oldest
/// first.
fn rebound_events(home: &Path, name: &str) -> Vec<serde_json::Value> {
    events(home, name)
        .into_iter()
        .filter(|e| e["type"] == "execution_anchor_rebound")
        .map(|e| e["payload"].clone())
        .collect()
}

#[test]
fn a_moved_checkout_is_repaired_by_rebinding_and_ticks_from_the_new_tree() {
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    init_at(home.path(), &old, "wf", MARKER_TEMPLATE);

    // Where this starts: the session is bound to a tree the developer
    // is no longer standing in, and the tick is refused.
    let refused = run_koto(home.path(), &moved, &["next", "wf"]);
    assert!(!refused.success);
    assert_eq!(refused.json["error"]["code"], "execution_anchor_mismatch");

    // The one command the refusal names.
    let rebind = run_koto(
        home.path(),
        &moved,
        &["session", "rebind", "wf", "--to", moved.to_str().unwrap()],
    );
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);
    assert_eq!(rebind.json["rebound"], serde_json::json!(true));
    assert_eq!(
        rebind.json["from"],
        serde_json::json!(old.to_str().unwrap())
    );
    assert_eq!(
        rebind.json["to"],
        serde_json::json!(moved.to_str().unwrap())
    );

    // The header now names the new tree...
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(moved.to_str().unwrap())
    );

    // ...and the move is in the log, with both ends of it.
    assert_eq!(
        rebound_events(home.path(), "wf"),
        vec![serde_json::json!({
            "from": old.to_str().unwrap(),
            "to": moved.to_str().unwrap(),
        })]
    );

    // The tick that was refused now runs, and it runs in the new tree.
    let tick = run_koto(home.path(), &moved, &["next", "wf"]);
    assert!(
        tick.success,
        "the tick that was refused must now succeed: {}",
        tick.stdout
    );
    assert!(
        moved.join("marker.txt").exists(),
        "the action must run at the new anchor"
    );
    assert!(!old.join("marker.txt").exists());
}

#[test]
fn rebind_defaults_to_the_directory_it_runs_in() {
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    init_at(home.path(), &old, "wf", PLAIN_TEMPLATE);

    // No `--to`: stand in the checkout you moved to and rebind.
    let rebind = run_koto(home.path(), &moved, &["session", "rebind", "wf"]);
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);
    assert_eq!(
        rebind.json["to"],
        serde_json::json!(moved.to_str().unwrap())
    );
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(moved.to_str().unwrap())
    );

    assert!(run_koto(home.path(), &moved, &["next", "wf"]).success);
}

#[test]
fn rebind_repairs_an_anchor_that_no_longer_resolves() {
    // The other refusal: the recorded directory is gone, so there is
    // nowhere to change to. Rebinding is the only way out.
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    init_at(home.path(), &old, "wf", PLAIN_TEMPLATE);
    std::fs::remove_dir_all(&old).unwrap();

    let refused = run_koto(home.path(), &moved, &["next", "wf"]);
    assert_eq!(
        refused.json["error"]["code"],
        "execution_anchor_unresolvable"
    );

    let rebind = run_koto(
        home.path(),
        &moved,
        &["session", "rebind", "wf", "--to", moved.to_str().unwrap()],
    );
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);

    let tick = run_koto(home.path(), &moved, &["next", "wf"]);
    assert!(
        tick.success,
        "a deleted checkout must be repairable without restoring it: {}",
        tick.stdout
    );
}

#[test]
fn rebind_is_symlink_and_trailing_slash_insensitive() {
    // What lands on the header is canonical, because that is the form
    // the per-tick check compares against.
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    let link = home.path().join("link-to-moved");
    std::os::unix::fs::symlink(&moved, &link).unwrap();
    init_at(home.path(), &old, "wf", PLAIN_TEMPLATE);

    let rebind = run_koto(
        home.path(),
        &old,
        &[
            "session",
            "rebind",
            "wf",
            "--to",
            &format!("{}/", link.to_str().unwrap()),
        ],
    );
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(moved.to_str().unwrap()),
        "the anchor is stored canonical, not as the caller spelled it"
    );
}

#[test]
fn rebinding_to_the_directory_already_bound_appends_nothing() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
    let before = event_types(home.path(), "wf");

    let rebind = run_koto(
        home.path(),
        &anchor,
        &["session", "rebind", "wf", "--to", anchor.to_str().unwrap()],
    );
    assert!(rebind.success);
    assert_eq!(
        rebind.json["rebound"],
        serde_json::json!(false),
        "an anchor that did not move is reported as not rebound"
    );
    assert_eq!(
        event_types(home.path(), "wf"),
        before,
        "the event means the anchor moved; a no-op did not move it"
    );
}

#[test]
fn rebind_works_on_a_session_created_by_another_session() {
    // R16: a child is anchored to its parent's tree, and nothing about
    // rebinding cares how the session was created.
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    init_at(home.path(), &old, "parent", PLAIN_TEMPLATE);

    let src = home.path().join("child-template.md");
    std::fs::write(&src, MARKER_TEMPLATE).unwrap();
    let created = run_koto(
        home.path(),
        &old,
        &[
            "init",
            "parent.child",
            "--template",
            src.to_str().unwrap(),
            "--parent",
            "parent",
        ],
    );
    assert!(created.success, "child init failed: {}", created.stdout);

    let rebind = run_koto(
        home.path(),
        &moved,
        &[
            "session",
            "rebind",
            "parent.child",
            "--to",
            moved.to_str().unwrap(),
        ],
    );
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);
    assert_eq!(
        header(home.path(), "parent.child")["execution_dir"],
        serde_json::json!(moved.to_str().unwrap())
    );

    // The child moved; the parent did not. Rebinding is per session.
    assert_eq!(
        header(home.path(), "parent")["execution_dir"],
        serde_json::json!(old.to_str().unwrap())
    );

    let tick = run_koto(home.path(), &moved, &["next", "parent.child"]);
    assert!(tick.success, "child tick failed: {}", tick.stdout);
    assert!(moved.join("marker.txt").exists());
}

#[test]
fn a_session_with_no_recorded_anchor_can_be_rebound_before_it_adopts_one() {
    // A log written before anchoring existed carries no anchor to move
    // away from, so the event records only where it landed.
    let home = TempDir::new().unwrap();
    let old = dir(home.path(), "checkout");
    let moved = dir(home.path(), "moved-checkout");
    init_at(home.path(), &old, "wf", PLAIN_TEMPLATE);
    strip_anchor(home.path(), "wf");

    let rebind = run_koto(
        home.path(),
        &old,
        &["session", "rebind", "wf", "--to", moved.to_str().unwrap()],
    );
    assert!(rebind.success, "rebind failed: {}", rebind.stdout);
    assert_eq!(rebind.json["from"], serde_json::Value::Null);

    assert_eq!(
        rebound_events(home.path(), "wf"),
        vec![serde_json::json!({"to": moved.to_str().unwrap()})],
        "with nothing to move away from, the payload carries only `to`"
    );

    // And the session is anchored now: the first tick does not adopt.
    let tick = run_koto(home.path(), &moved, &["next", "wf"]);
    assert!(tick.success, "tick failed: {}", tick.stdout);
    assert!(
        !event_types(home.path(), "wf")
            .iter()
            .any(|t| t == "execution_anchor_adopted"),
        "a rebind ahead of the first tick is what bound it, so nothing adopts"
    );
}

#[test]
fn rebinding_to_a_directory_that_does_not_exist_leaves_the_anchor_alone() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
    let before = event_types(home.path(), "wf");

    let run = run_koto(
        home.path(),
        &anchor,
        &[
            "session",
            "rebind",
            "wf",
            "--to",
            "/definitely/not/a/directory/koto",
        ],
    );
    assert!(!run.success, "a target that does not resolve must refuse");

    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(anchor.to_str().unwrap()),
        "a refused rebind must not move the anchor"
    );
    assert_eq!(event_types(home.path(), "wf"), before);
}

#[test]
fn rebinding_to_a_file_rather_than_a_directory_is_refused() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
    let file = home.path().join("not-a-directory");
    std::fs::write(&file, "x").unwrap();

    let run = run_koto(
        home.path(),
        &anchor,
        &["session", "rebind", "wf", "--to", file.to_str().unwrap()],
    );
    assert!(!run.success);
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(anchor.to_str().unwrap())
    );
}

#[test]
fn rebinding_a_session_that_does_not_exist_is_refused() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");

    let run = run_koto(
        home.path(),
        &anchor,
        &[
            "session",
            "rebind",
            "nope",
            "--to",
            anchor.to_str().unwrap(),
        ],
    );
    assert!(!run.success);
    assert!(
        run.stdout.is_empty(),
        "the refusal belongs on stderr, not in the JSON body: {}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// R14: a session written before anchoring existed
// ---------------------------------------------------------------------------

#[test]
fn a_session_with_no_anchor_adopts_one_with_exactly_one_notice() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let other = dir(home.path(), "other-checkout");
    init_at(home.path(), &anchor, "wf", PLAIN_TEMPLATE);
    strip_anchor(home.path(), "wf");
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::Value::Null
    );

    // First tick: accepted, not refused, and it says what it bound.
    let first = run_koto(home.path(), &anchor, &["next", "wf"]);
    assert!(
        first.success,
        "an anchor-less session must keep working: {}",
        first.stdout
    );
    let directive = first.json["directive"].as_str().unwrap();
    assert!(
        directive.contains("now bound to") && directive.contains(anchor.to_str().unwrap()),
        "the adoption must be visible, not silent: {}",
        directive
    );

    // The adoption is recorded on both the log and the header.
    assert_eq!(
        event_types(home.path(), "wf")
            .iter()
            .filter(|t| *t == "execution_anchor_adopted")
            .count(),
        1
    );
    assert_eq!(
        header(home.path(), "wf")["execution_dir"],
        serde_json::json!(anchor.to_str().unwrap())
    );

    // Second tick: ordinary path, no second notice.
    let second = run_koto(home.path(), &anchor, &["next", "wf"]);
    assert!(second.success);
    assert!(
        !second.json["directive"]
            .as_str()
            .unwrap_or_default()
            .contains("now bound to"),
        "the notice is one-time: {}",
        second.stdout
    );
    assert_eq!(
        event_types(home.path(), "wf")
            .iter()
            .filter(|t| *t == "execution_anchor_adopted")
            .count(),
        1,
        "exactly one adoption per session"
    );

    // And from then on it is anchored: a different tree is refused.
    let refused = run_koto(home.path(), &other, &["next", "wf"]);
    assert!(!refused.success);
    assert_eq!(refused.json["error"]["code"], "execution_anchor_mismatch");
}

// ---------------------------------------------------------------------------
// R16: a session created by another session
// ---------------------------------------------------------------------------

#[test]
fn a_child_session_records_the_parents_anchor() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    let elsewhere = dir(home.path(), "elsewhere");
    init_at(home.path(), &anchor, "parent", PLAIN_TEMPLATE);

    // Create the child from a completely different directory: what
    // lands on its header must be the parent's anchor, not this one.
    let src = home.path().join("child-template.md");
    std::fs::write(&src, PLAIN_TEMPLATE).unwrap();
    let run = run_koto(
        home.path(),
        &elsewhere,
        &[
            "init",
            "parent.child",
            "--template",
            src.to_str().unwrap(),
            "--parent",
            "parent",
        ],
    );
    assert!(run.success, "child init failed: {}", run.stdout);

    assert_eq!(
        header(home.path(), "parent.child")["execution_dir"],
        serde_json::json!(anchor.to_str().unwrap()),
        "a child copies the parent's anchor rather than adopting the spawning directory"
    );
}

// ---------------------------------------------------------------------------
// Decision 8: `working_dir` resolves against the anchor, and cannot leave it
// ---------------------------------------------------------------------------

/// A workflow whose action writes a marker at a relative path from a
/// `working_dir` supplied by the caller.
fn working_dir_template(working_dir: &str, variables: &str) -> String {
    format!(
        r#"---
name: wd
version: "1.0"
initial_state: mark
{variables}states:
  mark:
    default_action:
      command: "printf ok > marker.txt"
      working_dir: "{working_dir}"
    transitions:
      - target: done
  done:
    terminal: true
---

## mark

Write the marker.

## done

Done.
"#
    )
}

/// The single `__action__` condition of a blocked response.
fn action_condition(run: &Run) -> &serde_json::Value {
    assert_eq!(
        run.json["action"], "gate_blocked",
        "a refused working_dir stops the tick as an action failure; got {}",
        run.stdout
    );
    run.json["blocking_conditions"]
        .as_array()
        .expect("blocking_conditions should be an array")
        .iter()
        .find(|c| c["name"] == "__action__")
        .expect("the stop should carry an __action__ condition")
}

#[test]
fn a_relative_working_dir_resolves_against_the_anchor_not_the_tick_directory() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    // Both candidates exist, so the command can run in either and the
    // marker's location is the whole answer.
    let at_anchor = dir(&anchor, "sub");
    let elsewhere = dir(&anchor, "elsewhere");
    let at_tick_dir = dir(&elsewhere, "sub");

    init_at(home.path(), &anchor, "wd", &working_dir_template("sub", ""));

    // Tick from a subdirectory of the anchor, which containment accepts.
    let run = run_koto(home.path(), &elsewhere, &["next", "wd"]);
    assert!(run.success, "tick failed: {}", run.stdout);

    assert!(
        at_anchor.join("marker.txt").exists(),
        "working_dir is relative to the anchor; got {}",
        run.stdout
    );
    assert!(
        !at_tick_dir.join("marker.txt").exists(),
        "working_dir must not be relative to the directory `koto next` was typed in"
    );
}

#[test]
fn a_variable_derived_absolute_working_dir_is_an_action_failure() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    // A real directory outside the anchor: the template names it only
    // through a variable, so the compiler cannot see it is absolute and
    // the rejection has to happen after substitution.
    let outside = dir(home.path(), "outside");

    let variables = "variables:\n  DIR:\n    description: \"Where to run\"\n    required: true\n";
    let src = home.path().join("wd-template.md");
    std::fs::write(&src, working_dir_template("{{DIR}}", variables)).unwrap();
    let init = run_koto(
        home.path(),
        &anchor,
        &[
            "init",
            "wd",
            "--template",
            src.to_str().unwrap(),
            "--var",
            &format!("DIR={}", outside.to_str().unwrap()),
        ],
    );
    assert!(init.success, "init failed: {}", init.stdout);

    let run = run_koto(home.path(), &anchor, &["next", "wd"]);
    assert!(run.success, "tick failed: {}", run.stdout);

    let condition = action_condition(&run);
    let stderr = condition["output"]["stderr"].as_str().unwrap_or_default();
    assert!(
        stderr.contains("working_dir")
            && stderr.contains("absolute")
            && stderr.contains(outside.to_str().unwrap()),
        "the failure must name the field and the offending path, got {}",
        run.stdout
    );

    // The command never ran, so nothing was written and the tick stayed put.
    assert!(
        !outside.join("marker.txt").exists(),
        "an absolute working_dir must not be joined away and used"
    );
    assert_eq!(run.json["state"], "mark");
    assert_eq!(run.json["advanced"], false);
    assert!(
        !event_types(home.path(), "wd").contains(&"default_action_executed".to_string()),
        "no command ran, so no execution is recorded"
    );
}

#[test]
fn a_relative_working_dir_that_escapes_upward_is_an_action_failure() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");
    // `..` from the anchor lands here.
    let above = home.path().to_path_buf();

    init_at(home.path(), &anchor, "wd", &working_dir_template("..", ""));

    let run = run_koto(home.path(), &anchor, &["next", "wd"]);
    assert!(run.success, "tick failed: {}", run.stdout);

    let condition = action_condition(&run);
    let stderr = condition["output"]["stderr"].as_str().unwrap_or_default();
    assert!(
        stderr.contains("working_dir") && stderr.contains("outside"),
        "the failure must say the value left the anchor, got {}",
        run.stdout
    );
    assert!(
        !above.join("marker.txt").exists(),
        "a working_dir that escapes upward must not run"
    );
}

#[test]
fn a_literal_absolute_working_dir_never_compiles() {
    let home = TempDir::new().unwrap();
    let anchor = dir(home.path(), "checkout");

    let src = home.path().join("abs-template.md");
    std::fs::write(&src, working_dir_template("/tmp", "")).unwrap();
    let run = run_koto(
        home.path(),
        &anchor,
        &["init", "wd", "--template", src.to_str().unwrap()],
    );

    assert!(
        !run.success,
        "init should refuse the template: {}",
        run.stdout
    );
    assert!(
        run.stdout.contains("/tmp") && run.stdout.contains("absolute"),
        "the compile error must name the path, got {}",
        run.stdout
    );
}
