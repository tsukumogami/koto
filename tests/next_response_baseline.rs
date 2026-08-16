#![cfg(unix)]
//! Byte-identity baseline for `koto next` response bodies on an
//! instruction-free template.
//!
//! PRD-inline-phase-details R6 says a phase declaring no instructions must keep
//! producing exactly the response koto produces today. That is only checkable
//! against a record of what "today" was, and the record has to be taken before
//! the first behavior change lands -- a baseline captured alongside the change
//! would already carry it.
//!
//! So this file exists ahead of its purpose. It is committed with the inert
//! event and predicate of the first issue, when nothing in the response path has
//! moved yet, and the fixture it pins is what the pre-change binary emits. Every
//! later issue in the feature runs it unchanged; the first response body that
//! shifts by a byte fails here.
//!
//! The comparison is on raw stdout strings rather than parsed
//! `serde_json::Value`s, which is a deliberate departure from the repo's other
//! golden test (`tests/native_workflows_shape.rs`). The criterion is byte
//! identity, and a `Value` comparison would pass through key reordering and
//! whitespace drift -- both of which are real for these responses, since the
//! natural-advancement path serializes through a key-sorted `serde_json::Map`
//! and the directed path through a struct's declared field order.
//!
//! Regenerating is not the fix for a failure here while this feature is being
//! built: the baseline's whole value is that it predates the change, and
//! recapturing would overwrite it with whatever the code now does. The
//! regeneration helper at the bottom of this file spells out the one case that
//! does call for it.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::Path;

const FIXTURE: &str = "tests/fixtures/next-response-baseline/instruction-free.json";

/// Placeholder standing in for the tempdir-rooted template path in recorded
/// argv, so the fixture holds no machine-specific path.
const TEMPLATE_TOKEN: &str = "<TEMPLATE>";
const PARENT_TEMPLATE_TOKEN: &str = "<PARENT_TEMPLATE>";
const GATE_BLOCKED_TEMPLATE_TOKEN: &str = "<GATE_BLOCKED_TEMPLATE>";
const CONFIRM_TEMPLATE_TOKEN: &str = "<CONFIRM_TEMPLATE>";
const INTEGRATION_TEMPLATE_TOKEN: &str = "<INTEGRATION_TEMPLATE>";

// ---------------------------------------------------------------------------
//  Templates -- instruction-free by construction
//
//  None of these carry a `<!-- details -->` marker, so `TemplateState::details`
//  is empty for every phase and the `details` key is absent from every response
//  regardless of visit count. That is exactly the condition R6 covers. They also
//  declare no variables, so no session path can reach a response body and the
//  captured bytes are stable across machines.
// ---------------------------------------------------------------------------

const BASELINE_TEMPLATE: &str = r#"---
name: baseline
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
        values: [yes, no]
    transitions:
      - target: implement
        when:
          loop_again: yes
      - target: done
        when:
          loop_again: no
  done:
    terminal: true
---

## gather

Collect the inputs.

## relay

Hand off to the implementer.

## implement

Make the change.

## done

All done.
"#;

const PARENT_TEMPLATE: &str = r#"---
name: baseline-parent
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

/// A gate that can never pass without a `koto context add`, so the tick blocks
/// deterministically: no shell, no children, no clock. The state declares no
/// `accepts` and no `gates.*` when-clause, which is what keeps the engine from
/// falling through to an evidence-required response.
const GATE_BLOCKED_TEMPLATE: &str = r#"---
name: baseline-gate-blocked
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

## done

All done.
"#;

/// `echo ready` rather than anything path-bearing: the command string is
/// substituted into `action_output.command` verbatim, so an absolute path here
/// would put the machine into the fixture.
const CONFIRM_TEMPLATE: &str = r#"---
name: baseline-confirm
version: "1.0"
initial_state: apply
states:
  apply:
    default_action:
      command: "echo ready"
      requires_confirmation: true
    transitions:
      - target: done
  done:
    terminal: true
---

## apply

Apply the change.

## done

All done.
"#;

const INTEGRATION_TEMPLATE: &str = r#"---
name: baseline-integration
version: "1.0"
initial_state: delegate
states:
  delegate:
    integration: code_review
    transitions:
      - target: done
  done:
    terminal: true
---

## delegate

Delegate the review.

## done

All done.
"#;

const CHILD_TEMPLATE: &str = r#"---
name: baseline-child
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

## finished

All done.

## skipped

Skipped.
"#;

/// Prose recorded into the fixture alongside the bodies, so a reader who opens
/// it later knows what it is and what it deliberately leaves out.
const NOTES: &[&str] = &[
    "Baseline of full `koto next` response bodies for a template whose phases declare no instructions (no `<!-- details -->` marker anywhere). Captured from the binary as it stood before any behavior change in the inline-phase-details feature.",
    "Every sequence runs in its own temporary HOME and KOTO_SESSIONS_BASE, so no session state carries between them and the real session store is never touched.",
    "`stdout` holds the response bytes verbatim, including the trailing newline the CLI writes. Steps that only set a sequence up are run but not recorded -- `koto init`, `koto rewind`, and the ticks that get a workflow to the phase whose response is the point.",
    "One step is unrecorded for a harder reason than that: the parent tick that spawns a batch child. Its body embeds an `unassigned_children` entry carrying a wall-clock `created_at`, so it cannot be compared byte for byte at all. The child's own first tick, which is what the sequence is for, is recorded.",
    "Every call sequence the plan enumerates -- conditional-transition arrival, unconditional-transition arrival, directed transition, self-transition, rewind, the `--full` override, `koto init` plus the first tick, and a batch child's first tick -- is expressible in the template grammar and is recorded here. Nothing was omitted.",
    "Beyond those, the fixture records every response shape `koto next` can produce for a phase that declares no instructions: gate-blocked (with its non-advancing repeat, the scenario the feature exists for), terminal, action-requires-confirmation, and integration-unavailable. A later issue splices a discoverability pointer through all of them, and each needs something to be compared against.",
    "The bodies were also confirmed identical between the debug binary this harness runs and a `cargo build --release` binary, so the baseline is a property of the source rather than of the profile.",
    "Several recorded bodies are identical to each other -- init and rewind arrivals, the conditional and unconditional and self-transition arrivals, the non-advancing repeat and its `--full` counterpart. That is not redundancy to be tidied away. The equality across paths is exactly what a delivery rule applied to one construction site and not the other would break.",
];

// ---------------------------------------------------------------------------
//  Harness
// ---------------------------------------------------------------------------

/// Return a `koto` command with `KOTO_SESSIONS_BASE` set to the sessions
/// subdirectory of `dir`, and `HOME` overridden so nothing reads or writes the
/// real `~/.koto/`.
fn koto_cmd(dir: &Path) -> Command {
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions);
    cmd.env("HOME", dir);
    cmd
}

/// One CLI call in a sequence. `record` decides whether its stdout lands in the
/// fixture; setup calls run either way.
struct Step {
    argv: &'static [&'static str],
    record: bool,
}

const fn setup(argv: &'static [&'static str]) -> Step {
    Step {
        argv,
        record: false,
    }
}

const fn record(argv: &'static [&'static str]) -> Step {
    Step { argv, record: true }
}

struct Sequence {
    label: &'static str,
    description: &'static str,
    steps: &'static [Step],
}

const SEQUENCES: &[Sequence] = &[
    Sequence {
        label: "init-then-first-tick",
        description: "`koto init` followed by the first `koto next`: the arrival response for the initial phase.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            record(&["next", "wf"]),
        ],
    },
    Sequence {
        label: "conditional-transition-arrival",
        description: "Evidence routes `gather` to `implement` through a `when` clause.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            record(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
        ],
    },
    Sequence {
        label: "unconditional-transition-arrival",
        description: "Evidence routes `gather` to `relay`, whose sole transition is unconditional, so the tick chains on and the arrival at `implement` is reached unconditionally.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            record(&["next", "wf", "--with-data", r#"{"route":"indirect"}"#]),
        ],
    },
    Sequence {
        label: "non-advancing-repeat",
        description: "A second `koto next` on a phase awaiting evidence, which does not advance. This is the response the delivery rule will later change.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            setup(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
            record(&["next", "wf"]),
        ],
    },
    Sequence {
        label: "full-override",
        description: "The same non-advancing tick under `--full`.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            setup(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
            record(&["next", "wf", "--full"]),
        ],
    },
    Sequence {
        label: "self-transition-arrival",
        description: "`implement` transitions to itself, ending one occupancy and beginning another.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            setup(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
            record(&["next", "wf", "--with-data", r#"{"loop_again":"yes"}"#]),
        ],
    },
    Sequence {
        label: "directed-transition",
        description: "Two consecutive `--to` transitions into `implement`. The second is reachable only because `implement` declares itself as a target. Note the key order differs from the natural-advancement path.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            record(&["next", "wf", "--to", "implement"]),
            record(&["next", "wf", "--to", "implement"]),
        ],
    },
    Sequence {
        label: "rewind-arrival",
        description: "The workflow is advanced past `gather` and then rewound into it; the next response is the arrival.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            setup(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
            setup(&["rewind", "wf"]),
            record(&["next", "wf"]),
        ],
    },
    Sequence {
        label: "gate-blocked-then-repeat",
        description: "A phase whose gate fails, then a second `koto next` that evaluates the same failing gate and does not transition. This is the scenario the feature exists for, and the only recorded pair on the `gate_blocked` response shape.",
        steps: &[
            setup(&["init", "wf", "--template", GATE_BLOCKED_TEMPLATE_TOKEN]),
            record(&["next", "wf"]),
            record(&["next", "wf"]),
            record(&["next", "wf", "--full"]),
        ],
    },
    Sequence {
        label: "terminal",
        description: "The tick that reaches a terminal phase. `done` responses omit instructions regardless of the rule, so this pins the shape that must not acquire them.",
        steps: &[
            setup(&["init", "wf", "--template", TEMPLATE_TOKEN]),
            setup(&["next", "wf"]),
            setup(&["next", "wf", "--with-data", r#"{"route":"direct"}"#]),
            record(&["next", "wf", "--with-data", r#"{"loop_again":"no"}"#]),
        ],
    },
    Sequence {
        label: "action-requires-confirmation",
        description: "A phase whose default action requires confirmation, which is a fourth response shape the pointer of a later issue also passes through.",
        steps: &[
            setup(&["init", "wf", "--template", CONFIRM_TEMPLATE_TOKEN]),
            record(&["next", "wf"]),
        ],
    },
    Sequence {
        label: "integration-unavailable",
        description: "A phase declaring an integration, which is unconditionally unavailable in this build.",
        steps: &[
            setup(&["init", "wf", "--template", INTEGRATION_TEMPLATE_TOKEN]),
            record(&["next", "wf"]),
        ],
    },
    Sequence {
        label: "batch-child-first-tick",
        description: "A batch-spawned child's first `koto next`. The parent tick that spawns it is setup; the child's arrival response is the record.",
        steps: &[
            setup(&["init", "par", "--template", PARENT_TEMPLATE_TOKEN]),
            setup(&[
                "next",
                "par",
                "--with-data",
                r#"{"tasks":[{"name":"a","waits_on":[],"vars":{}}]}"#,
            ]),
            record(&["next", "par.a"]),
        ],
    },
];

/// Run every sequence and return the fixture document as pretty JSON with a
/// trailing newline.
fn capture() -> String {
    let mut sequences = Vec::new();

    for seq in SEQUENCES {
        // A fresh tempdir per sequence: sessions never share a store, so no
        // sequence can perturb another's `unassigned_children` or discovery
        // cursor.
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let templates: [(&str, &str, &str); 5] = [
            (TEMPLATE_TOKEN, "baseline.md", BASELINE_TEMPLATE),
            (PARENT_TEMPLATE_TOKEN, "parent.md", PARENT_TEMPLATE),
            (
                GATE_BLOCKED_TEMPLATE_TOKEN,
                "gate-blocked.md",
                GATE_BLOCKED_TEMPLATE,
            ),
            (CONFIRM_TEMPLATE_TOKEN, "confirm.md", CONFIRM_TEMPLATE),
            (
                INTEGRATION_TEMPLATE_TOKEN,
                "integration.md",
                INTEGRATION_TEMPLATE,
            ),
        ];
        for (_, filename, body) in templates {
            std::fs::write(root.join(filename), body).unwrap();
        }
        // Not tokenised: the parent template names it by relative path, and it
        // is resolved against the parent template's own directory.
        std::fs::write(root.join("child.md"), CHILD_TEMPLATE).unwrap();

        let mut responses = Vec::new();
        for step in seq.steps {
            let argv: Vec<String> = step
                .argv
                .iter()
                .map(
                    |a| match templates.iter().find(|(token, _, _)| token == a) {
                        Some((_, filename, _)) => root.join(filename).to_str().unwrap().to_string(),
                        None => a.to_string(),
                    },
                )
                .collect();

            let output = koto_cmd(root).args(&argv).output().unwrap();
            assert!(
                output.status.success(),
                "sequence {}: `koto {}` failed: stdout={} stderr={}",
                seq.label,
                step.argv.join(" "),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );

            if step.record {
                let stdout = String::from_utf8(output.stdout).unwrap_or_else(|e| {
                    panic!("sequence {}: stdout was not valid UTF-8: {e}", seq.label)
                });
                responses.push(serde_json::json!({
                    // The token, not the resolved path: the fixture must not
                    // carry a machine-specific directory.
                    "argv": step.argv,
                    "stdout": stdout,
                }));
            }
        }

        sequences.push(serde_json::json!({
            "label": seq.label,
            "description": seq.description,
            "responses": responses,
        }));
    }

    let doc = serde_json::json!({
        "notes": NOTES,
        "sequences": sequences,
    });
    let mut out = serde_json::to_string_pretty(&doc).unwrap();
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[test]
fn instruction_free_responses_are_byte_identical_to_the_baseline() {
    let actual = capture();
    let expected = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("read baseline fixture {FIXTURE}: {e}"));

    if actual != expected {
        // Point at the first differing sequence before dumping anything: the
        // whole document is large and the interesting part is usually one body.
        let first_diff = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, b)| a != b)
            .map(|n| n + 1);
        panic!(
            "the captured document drifted from the baseline in {FIXTURE}{}.\n\n\
             READ THIS BEFORE REGENERATING. If the drift is in a response body, \
             it is almost certainly the bug this fixture exists to catch: \
             PRD-inline-phase-details R6 requires a phase that declares no \
             instructions to produce exactly the bytes koto produced before the \
             feature, and the baseline was captured from a binary that predates \
             it. Regenerating would recapture from the changed binary and destroy \
             the only record of what the responses used to be. Fix the code \
             instead.\n\n\
             Regeneration is legitimate in one case only: a deliberate change to \
             the `koto next` response format that has nothing to do with this \
             feature, made after it has shipped. Then run:\n  \
             cargo test --test next_response_baseline -- --ignored --nocapture\n\n\
             Note that the document also embeds this file's `NOTES` and the \
             per-sequence `description` strings, so editing that prose trips this \
             test too. A diff confined to those lines is the harmless case.\n\n\
             --- actual ---\n{actual}\n--- expected ---\n{expected}",
            match first_diff {
                Some(line) => format!(" (first difference at line {line})"),
                None => " (one document is a prefix of the other)".to_string(),
            }
        );
    }
}

/// Guards the fixture against being regenerated into something that no longer
/// tests what it claims to. Without this, a future regeneration that dropped a
/// sequence -- or that started recording a template which does declare
/// instructions -- would still pass the comparison above.
#[test]
fn baseline_fixture_covers_every_required_sequence_and_stays_instruction_free() {
    let raw = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("read baseline fixture {FIXTURE}: {e}"));
    let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let sequences = doc["sequences"].as_array().expect("sequences array");

    let labels: Vec<&str> = sequences
        .iter()
        .map(|s| s["label"].as_str().unwrap())
        .collect();
    for required in [
        "init-then-first-tick",
        "conditional-transition-arrival",
        "unconditional-transition-arrival",
        "non-advancing-repeat",
        "full-override",
        "self-transition-arrival",
        "directed-transition",
        "rewind-arrival",
        "gate-blocked-then-repeat",
        "terminal",
        "action-requires-confirmation",
        "integration-unavailable",
        "batch-child-first-tick",
    ] {
        assert!(
            labels.contains(&required),
            "baseline fixture is missing the `{required}` sequence"
        );
    }

    // Every response shape `koto next` can produce for an instruction-free
    // phase must have a baseline, or a later issue's pointer splice lands on a
    // shape with nothing to be compared against.
    let mut actions: Vec<&str> = sequences
        .iter()
        .flat_map(|s| s["responses"].as_array().unwrap())
        .map(|r| {
            let body: serde_json::Value =
                serde_json::from_str(r["stdout"].as_str().unwrap().trim()).unwrap();
            match body["action"].as_str().unwrap() {
                "evidence_required" => "evidence_required",
                "gate_blocked" => "gate_blocked",
                "done" => "done",
                "confirm" => "confirm",
                "integration_unavailable" => "integration_unavailable",
                other => panic!("unrecognised action `{other}` in the fixture"),
            }
        })
        .collect();
    actions.sort_unstable();
    actions.dedup();
    assert_eq!(
        actions,
        vec![
            "confirm",
            "done",
            "evidence_required",
            "gate_blocked",
            "integration_unavailable",
        ],
        "the baseline no longer covers every response shape"
    );

    for seq in sequences {
        let responses = seq["responses"].as_array().expect("responses array");
        assert!(
            !responses.is_empty(),
            "sequence {} records no response at all",
            seq["label"]
        );
        for response in responses {
            let stdout = response["stdout"].as_str().expect("stdout string");
            let body: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
                panic!(
                    "sequence {}: recorded stdout is not JSON: {e}",
                    seq["label"]
                )
            });
            // The whole point of the fixture is that the template declares no
            // instructions, so no recorded body may carry a `details` key.
            assert!(
                body.get("details").is_none(),
                "sequence {}: a recorded response carries `details`, so the \
                 template is not instruction-free and the baseline does not \
                 test what it claims",
                seq["label"]
            );
        }
    }
}

/// Regeneration helper.
///
/// Reach for this only for a deliberate change to the `koto next` response
/// format that has nothing to do with inline phase details, made after that
/// feature has shipped. While the feature is being built, a failing baseline is
/// the finding, and rewriting the fixture destroys the pre-change record the
/// comparison depends on.
///
/// Writes the fixture rather than printing it: the document embeds response
/// bodies as escaped strings, and round-tripping that through a terminal is how
/// a stray byte gets in. `#[ignore]` keeps it out of every ordinary run, so it
/// only fires when someone asks for it by name.
#[test]
#[ignore = "regeneration helper; rewrites the baseline fixture"]
fn regenerate_baseline_fixture() {
    let doc = capture();
    std::fs::create_dir_all(Path::new(FIXTURE).parent().unwrap()).unwrap();
    std::fs::write(FIXTURE, &doc)
        .unwrap_or_else(|e| panic!("write baseline fixture {FIXTURE}: {e}"));
    println!("wrote {} ({} bytes)", FIXTURE, doc.len());
}
