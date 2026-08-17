//! Behavioral coverage for the delivery rule (Issue 2 of
//! docs/plans/PLAN-inline-phase-details.md).
//!
//! `tests/next_response_baseline.rs` pins byte-identity for templates that
//! declare no instructions -- it cannot exercise the delivery rule itself,
//! because a phase with no `<!-- details -->` marker never carries `details`
//! regardless of delivery history. This file uses templates that do declare
//! instructions, and asserts presence/absence of `details` across every
//! arrival path the design's predicate must handle uniformly on both the
//! natural-advancement and directed-transition construction sites.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// Run a `koto` invocation and parse its last non-blank stdout line as JSON.
/// Panics if the process did not exit successfully.
fn run_koto(dir: &Path, args: &[&str]) -> serde_json::Value {
    let output = koto_cmd(dir).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "`koto {}` failed: stdout={} stderr={}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    serde_json::from_str(last).unwrap_or(serde_json::Value::Null)
}

fn details_of(resp: &serde_json::Value) -> Option<&str> {
    resp.get("details").and_then(|v| v.as_str())
}

fn assert_carries(resp: &serde_json::Value, expected: &str, context: &str) {
    assert_eq!(
        details_of(resp),
        Some(expected),
        "{context}: expected details {:?}, got response {}",
        expected,
        resp
    );
}

fn assert_omits(resp: &serde_json::Value, context: &str) {
    assert_eq!(
        details_of(resp),
        None,
        "{context}: expected no details, got response {}",
        resp
    );
}

// ---------------------------------------------------------------------------
//  Templates
// ---------------------------------------------------------------------------

/// `gather` and `implement` both declare instructions. `implement` declares a
/// self-transition (`loop_again: yes`), a loop-back into `gather`
/// (`loop_again: redo`), and a terminal exit (`loop_again: no`), and is also a
/// valid `--to` target from `gather`, so the same template covers conditional,
/// unconditional (via `relay`), self, loop-back, directed, rewind, and
/// override arrivals.
const DELIVERY_TEMPLATE: &str = r#"---
name: delivery
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [direct, indirect]
    transitions:
      - target: implement
        when:
          route: direct
      - target: relay
        when:
          route: indirect
  relay:
    transitions:
      - target: implement
  implement:
    accepts:
      loop_again:
        type: enum
        required: true
        values: [yes, no, redo]
    transitions:
      - target: implement
        when:
          loop_again: yes
      - target: gather
        when:
          loop_again: redo
      - target: done
        when:
          loop_again: no
  done:
    terminal: true
---

## gather

Collect the inputs.

<!-- details -->

Gather instructions.

## relay

Hand off to the implementer.

## implement

Make the change.

<!-- details -->

Implement instructions.

## done

All done.
"#;

/// A gate that can never pass without a `koto context add`, so the tick
/// blocks deterministically across repeated ticks.
const GATE_BLOCKED_TEMPLATE: &str = r#"---
name: gate-delivery
version: "1.0"
initial_state: guarded
states:
  guarded:
    gates:
      approval:
        type: context-exists
        key: approval_note
    transitions:
      - target: done
  done:
    terminal: true
---

## guarded

Wait for the approval note.

<!-- details -->

Guarded instructions.

## done

All done.
"#;

const PARENT_TEMPLATE: &str = r#"---
name: delivery-parent
version: "1.0"
initial_state: fan_out
states:
  fan_out:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      children:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: done
        when:
          gates.children.all_complete: true
  done:
    terminal: true
---

## fan_out

Fan the work out.

## done

All done.
"#;

const CHILD_TEMPLATE: &str = r#"---
name: delivery-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done, skip]
    transitions:
      - target: finished
        when:
          marker: done
      - target: skipped
        when:
          marker: skip
  finished:
    terminal: true
  skipped:
    terminal: true
    skipped_marker: true
---

## work

Do the work.

<!-- details -->

Child work instructions.

## finished

All done.

## skipped

Skipped.
"#;

/// Write `DELIVERY_TEMPLATE` into `dir` and return its path.
fn write_delivery_template(dir: &Path) -> PathBuf {
    let path = dir.join("delivery.md");
    std::fs::write(&path, DELIVERY_TEMPLATE).unwrap();
    path
}

// ---------------------------------------------------------------------------
//  Initial arrival
// ---------------------------------------------------------------------------

#[test]
fn init_then_first_tick_carries_details_for_initial_phase() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    let resp = run_koto(root, &["next", "wf"]);
    assert_carries(&resp, "Gather instructions.", "init + first tick");
}

#[test]
fn batch_spawned_child_first_tick_carries_details_for_its_own_initial_phase() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("parent.md"), PARENT_TEMPLATE).unwrap();
    std::fs::write(root.join("child.md"), CHILD_TEMPLATE).unwrap();

    koto_cmd(root)
        .args(["init", "par", "--template", "parent.md"])
        .assert()
        .success();
    koto_cmd(root)
        .args([
            "next",
            "par",
            "--with-data",
            r#"{"tasks":[{"name":"a","waits_on":[],"vars":{}}]}"#,
        ])
        .assert()
        .success();
    let resp = run_koto(root, &["next", "par.a"]);
    assert_carries(
        &resp,
        "Child work instructions.",
        "batch child's first tick",
    );
}

// ---------------------------------------------------------------------------
//  Transition arrivals: conditional, unconditional, self, loop-back
// ---------------------------------------------------------------------------

#[test]
fn conditional_transition_arrival_carries_details() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    let resp = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"route":"direct"}"#],
    );
    assert_carries(
        &resp,
        "Implement instructions.",
        "conditional-transition arrival",
    );
}

#[test]
fn unconditional_transition_arrival_carries_details_separately_from_conditional() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    // `route: indirect` routes through `relay`, whose sole transition is
    // unconditional, so the tick chains on to `implement` without a second
    // evidence submission.
    let resp = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"route":"indirect"}"#],
    );
    assert_eq!(resp["state"], "implement");
    assert_carries(
        &resp,
        "Implement instructions.",
        "unconditional-transition arrival",
    );
}

#[test]
fn self_transition_arrival_carries_details_again() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    koto_cmd(root)
        .args(["next", "wf", "--with-data", r#"{"route":"direct"}"#])
        .assert()
        .success();
    // First occupancy of `implement` has already delivered. A self-transition
    // starts a fresh occupancy, so delivery happens again.
    let resp = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"yes"}"#],
    );
    assert_eq!(resp["state"], "implement");
    assert_carries(&resp, "Implement instructions.", "self-transition arrival");
}

#[test]
fn loop_back_arrival_at_previously_occupied_phase_carries_details_again() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    koto_cmd(root)
        .args(["next", "wf", "--with-data", r#"{"route":"direct"}"#])
        .assert()
        .success();
    // `gather` was already occupied (and delivered) at the start of this
    // sequence. Looping back into it via `implement` starts a fresh
    // occupancy, so delivery happens again -- this is the case a
    // visit-count predicate gets backwards.
    let resp = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"redo"}"#],
    );
    assert_eq!(resp["state"], "gather");
    assert_carries(&resp, "Gather instructions.", "loop-back arrival");
}

#[test]
fn rewind_arrival_carries_details() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    koto_cmd(root)
        .args(["next", "wf", "--with-data", r#"{"route":"direct"}"#])
        .assert()
        .success();
    koto_cmd(root).args(["rewind", "wf"]).assert().success();
    let resp = run_koto(root, &["next", "wf"]);
    assert_eq!(resp["state"], "gather");
    assert_carries(&resp, "Gather instructions.", "rewind arrival");
}

// ---------------------------------------------------------------------------
//  Gate-blocked repeat
// ---------------------------------------------------------------------------

#[test]
fn gate_blocked_first_tick_carries_and_repeat_omits() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("gate.md"), GATE_BLOCKED_TEMPLATE).unwrap();

    koto_cmd(root)
        .args(["init", "wf", "--template", "gate.md"])
        .assert()
        .success();

    let first = run_koto(root, &["next", "wf"]);
    assert_eq!(first["action"], "gate_blocked");
    assert_carries(&first, "Guarded instructions.", "gate-blocked first tick");

    // Same failing gate, no transition: the occupancy has not moved, and
    // it already delivered.
    let second = run_koto(root, &["next", "wf"]);
    assert_eq!(second["action"], "gate_blocked");
    assert_omits(&second, "gate-blocked repeat tick");
}

// ---------------------------------------------------------------------------
//  Directed transitions
// ---------------------------------------------------------------------------

#[test]
fn directed_transition_carries_then_nonadvancing_tick_omits() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();

    let directed = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert_carries(
        &directed,
        "Implement instructions.",
        "directed transition arrival",
    );

    // No evidence submitted: does not advance, same occupancy as the
    // directed arrival just delivered.
    let repeat = run_koto(root, &["next", "wf"]);
    assert_omits(&repeat, "non-advancing tick after directed transition");
}

#[test]
fn two_consecutive_directed_transitions_into_same_phase_both_carry() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();

    let first = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert_carries(
        &first,
        "Implement instructions.",
        "first directed transition",
    );

    // `implement` declares itself as a transition target (the self-
    // transition `when: loop_again: yes`), so a second directed transition
    // into it is valid and is a fresh occupancy.
    let second = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert_carries(
        &second,
        "Implement instructions.",
        "second directed transition (self-transition)",
    );
}

// ---------------------------------------------------------------------------
//  Override flag
// ---------------------------------------------------------------------------

#[test]
fn full_override_returns_details_on_a_response_that_would_otherwise_be_suppressed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();
    koto_cmd(root)
        .args(["next", "wf", "--with-data", r#"{"route":"direct"}"#])
        .assert()
        .success();

    // Confirm the plain repeat is suppressed first, so the override
    // assertion below is meaningfully overriding something.
    let suppressed = run_koto(root, &["next", "wf"]);
    assert_omits(&suppressed, "plain non-advancing repeat");

    let overridden = run_koto(root, &["next", "wf", "--full"]);
    assert_carries(
        &overridden,
        "Implement instructions.",
        "--full override on a suppressed response",
    );
}

/// The override call must record its own delivery, not merely surface
/// details already recorded by an earlier response in the same occupancy.
/// `--full` is applied to the *first* tick of the occupancy -- the only
/// possible source of a delivery record for it -- so an implementation
/// that gates the append on `!full` would fail the assertion below: the
/// following plain tick would wrongly carry the instructions again.
#[test]
fn override_call_records_a_delivery_so_the_next_plain_call_omits_instructions() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_delivery_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();

    // First tick of `implement`'s occupancy, requested with --full.
    // already_delivered is false here regardless of the flag, so this
    // assertion alone would pass even if --full's own append were
    // skipped -- the point is the *next* assertion.
    let arrival = run_koto(
        root,
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"route":"direct"}"#,
            "--full",
        ],
    );
    assert_carries(
        &arrival,
        "Implement instructions.",
        "override on first tick of occupancy",
    );

    let repeat = run_koto(root, &["next", "wf"]);
    assert_omits(
        &repeat,
        "plain non-advancing tick after an override-only delivery",
    );
}
