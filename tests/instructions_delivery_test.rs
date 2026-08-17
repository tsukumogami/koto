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

/// The recovery pointer is gated on the *phase* declaring instructions, not on
/// this response carrying them, so it must survive suppression. It is the only
/// route back to a procedure lost inside a loop, which is what makes suppressing
/// there safe.
fn assert_pointer(resp: &serde_json::Value, context: &str) {
    let directive = resp
        .get("directive")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        directive.starts_with("[koto] Lost context?"),
        "{context}: a suppressed response must still name the retrieval, got directive {:?}",
        directive
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

/// `implement` declares instructions and routes through `bounce`, whose sole
/// transition is unconditional, so a single tick can leave `implement`, pass
/// through another phase, and arrive back at `implement`. That round trip is the
/// case a paraphrase of the delivery rule most often gets wrong: the tick begins
/// and ends in the same phase and is nonetheless a genuine arrival, because the
/// entry event that lands it records a different source.
///
/// `DELIVERY_TEMPLATE` cannot express it -- `implement`'s only non-self targets
/// are `gather`, which has required `accepts` and stops the chain, and the
/// terminal `done`.
const ROUND_TRIP_TEMPLATE: &str = r#"---
name: round-trip
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [direct]
    transitions:
      - target: implement
        when:
          route: direct
  implement:
    accepts:
      loop_again:
        type: enum
        required: true
        values: [hop, no]
    transitions:
      - target: bounce
        when:
          loop_again: hop
      - target: done
        when:
          loop_again: no
  bounce:
    transitions:
      - target: implement
  done:
    terminal: true
---

## gather

Collect the inputs.

<!-- details -->

Gather instructions.

## implement

Make the change.

<!-- details -->

Implement instructions.

## bounce

Pass straight through.

## done

All done.
"#;

/// Write `DELIVERY_TEMPLATE` into `dir` and return its path.
fn write_delivery_template(dir: &Path) -> PathBuf {
    let path = dir.join("delivery.md");
    std::fs::write(&path, DELIVERY_TEMPLATE).unwrap();
    path
}

/// Write `ROUND_TRIP_TEMPLATE` into `dir` and return its path.
fn write_round_trip_template(dir: &Path) -> PathBuf {
    let path = dir.join("round-trip.md");
    std::fs::write(&path, ROUND_TRIP_TEMPLATE).unwrap();
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
fn self_transition_omits_details_and_keeps_the_pointer() {
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
    // The arrival at `implement` already delivered. A self-transition is a lap
    // around a loop the agent is already in rather than an arrival, so it opens
    // no delivery window and the instructions are not re-sent.
    let first = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"yes"}"#],
    );
    assert_eq!(first["state"], "implement");
    assert_eq!(first["advanced"], true);
    assert_omits(&first, "self-transition");
    assert_pointer(&first, "self-transition");

    // ...and it stays omitted however many laps follow.
    let second = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"yes"}"#],
    );
    assert_eq!(second["state"], "implement");
    assert_eq!(second["advanced"], true);
    assert_omits(&second, "second consecutive self-transition");
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
    // sequence. Looping back into it from `implement` is an arrival from a
    // different phase, so it opens a fresh delivery window and delivery happens
    // again -- this is the case a visit-count predicate gets backwards, and the
    // one a rule keyed on "did the state change" would get backwards too.
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

    // Same failing gate, no transition: no entry event was appended, so the
    // delivery window has not moved and it already delivered.
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

    // No evidence submitted: does not advance, so it stays inside the delivery
    // window the directed arrival opened and already delivered for.
    let repeat = run_koto(root, &["next", "wf"]);
    assert_omits(&repeat, "non-advancing tick after directed transition");
}

#[test]
fn a_second_directed_transition_into_the_occupied_phase_omits() {
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
    // into it is valid -- and it is a hand-driven lap of that loop rather
    // than an arrival, so it opens no delivery window and repeats nothing.
    let second = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert_omits(&second, "directed transition into the occupied phase");
    assert_pointer(&second, "directed transition into the occupied phase");
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
/// details already recorded by an earlier response in the same delivery window.
/// `--full` is applied to the *first* tick of the window -- the only
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

    // First tick of `implement`'s delivery window, requested with --full.
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
        "override on first tick of the delivery window",
    );

    let repeat = run_koto(root, &["next", "wf"]);
    assert_omits(
        &repeat,
        "plain non-advancing tick after an override-only delivery",
    );
}

// ---------------------------------------------------------------------------
//  Same-tick round trip, same-phase rewind, and the non-entry event
// ---------------------------------------------------------------------------

/// A tick that leaves `implement`, passes through `bounce`, and arrives back at
/// `implement` is an arrival, not a lap. The tick begins and ends in the same
/// phase, so a rule paraphrased as "did the state change" gets it backwards; the
/// entry event that lands the tick records `bounce` as its source, which is what
/// decides it.
#[test]
fn same_tick_round_trip_back_to_a_phase_carries_details_again() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = write_round_trip_template(root);

    koto_cmd(root)
        .args(["init", "wf", "--template", template.to_str().unwrap()])
        .assert()
        .success();
    koto_cmd(root).args(["next", "wf"]).assert().success();

    let arrival = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"route":"direct"}"#],
    );
    assert_carries(&arrival, "Implement instructions.", "arrival at implement");

    let round_trip = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"hop"}"#],
    );
    assert_eq!(round_trip["state"], "implement");
    assert_carries(
        &round_trip,
        "Implement instructions.",
        "implement -> bounce -> implement within one tick",
    );
}

/// `koto rewind` right after a self-transition records the same phase as both
/// source and target -- its destination is the second-to-last state-changing
/// event's target, which the self-transition made `implement` twice over. A
/// rewind means redo this rather than continue, so it delivers whatever phases
/// it records; the rule reads the event's variant rather than its `from` field,
/// so this answer cannot drift with a change to how rewind picks a destination.
#[test]
fn a_rewind_recording_the_same_phase_twice_carries_details_again() {
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
    let lap = run_koto(
        root,
        &["next", "wf", "--with-data", r#"{"loop_again":"yes"}"#],
    );
    assert_omits(&lap, "self-transition before the rewind");

    koto_cmd(root).args(["rewind", "wf"]).assert().success();

    let after = run_koto(root, &["next", "wf"]);
    assert_eq!(after["state"], "implement");
    assert_carries(
        &after,
        "Implement instructions.",
        "tick after a rewind recording implement as both source and target",
    );
}

/// The override forces the instructions through on a response the rule would
/// otherwise suppress. This is a regression check on `--full`, not a test of its
/// recording clause: inside a self-entry window the arrival already recorded a
/// delivery, so the override's own record can never be the load-bearing one
/// there. The recording is covered by
/// `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`,
/// which applies `--full` to a window's first response.
#[test]
fn override_forces_details_through_on_a_suppressed_self_transition() {
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

    let forced = run_koto(
        root,
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"loop_again":"yes"}"#,
            "--full",
        ],
    );
    assert_carries(
        &forced,
        "Implement instructions.",
        "--full on a self-transition the rule would suppress",
    );

    let plain = run_koto(root, &["next", "wf"]);
    assert_omits(&plain, "non-advancing tick after the forced delivery");
}

/// An event that is not a state entry neither opens a delivery window nor closes
/// one. Recording a decision is the instrument here rather than a gate override:
/// an override unblocks the phase, so the next response would belong to a
/// different phase -- and on this file's gated template, to a terminal one with
/// no `details` field to assert against at all.
#[test]
fn a_recorded_decision_does_not_reopen_the_delivery_window() {
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

    let before = run_koto(root, &["next", "wf"]);
    assert_omits(&before, "non-advancing tick before the decision");

    koto_cmd(root)
        .args([
            "decisions",
            "record",
            "wf",
            "--with-data",
            r#"{"choice":"keep the current approach","rationale":"nothing about the phase changed"}"#,
        ])
        .assert()
        .success();

    let after = run_koto(root, &["next", "wf"]);
    assert_omits(&after, "non-advancing tick after a recorded decision");
    assert_pointer(&after, "non-advancing tick after a recorded decision");
}
