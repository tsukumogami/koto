#![cfg(unix)]
//! Compatibility baselines for the failure-reporting feature.
//!
//! Two records, both taken from the code as it stood before the feature
//! changed anything, so every later step runs against them:
//!
//! 1. **Compiled-template identity.** Every template under the repository's
//!    test fixture template directories compiles to exactly the JSON it
//!    compiled to before. The compiled JSON is what a session's
//!    `template_hash` is taken over, so a byte of drift here means an
//!    existing session's template stops matching its cache entry. Snapshots
//!    live in `tests/fixtures/compiled-snapshots/` (the directory
//!    `gate_overridable_test.rs` already reads; the templates it pins keep
//!    their files and the rest are added beside them).
//!
//! 2. **Failing `koto next` responses.** The response bodies for a failing
//!    command gate, a failing context-exists gate and a failing
//!    `default_action` are recorded in
//!    `tests/fixtures/next-response-baseline/failing-responses.json`. The
//!    rule later work must keep is that a failing response may only gain
//!    optional fields: after [`json_baseline::strip_added_keys`] deletes
//!    every key the fixture lacks, the response must equal the fixture.
//!
//! The third piece, that koto v0.14.1 reads a session log the new koto
//! writes, needs the released binary and lives in
//! `test/compat/failure-reporting-v0_14_1.sh`.
//!
//! A failure in this file is a finding, not a prompt to regenerate: the
//! fixtures' value is that they predate the change. The regeneration helpers
//! at the bottom spell out when they may be used.

#[path = "support/json_baseline.rs"]
mod json_baseline;

use assert_cmd::Command;
use assert_fs::TempDir;
use json_baseline::{strip_added_keys, tokenize_strings};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ===========================================================================
//  1. Compiled-template identity
// ===========================================================================

const SNAPSHOT_DIR: &str = "tests/fixtures/compiled-snapshots";

/// Template directories whose every `.md` file must keep its compiled JSON,
/// and where each one's snapshot lives relative to [`SNAPSHOT_DIR`]. The
/// functional templates map to the snapshot directory's root so the four
/// snapshots `gate_overridable_test.rs` already pins stay where they are.
/// A new fixture directory gets a line here and a subdirectory of its own.
const TEMPLATE_DIRS: &[(&str, &str)] = &[
    ("test/functional/fixtures/templates", ""),
    ("tests/fixtures/template-hash", "template-hash"),
    ("tests/fixtures/decider", "decider"),
    ("test/compat/fixtures", "compat"),
];

/// Every fixture template paired with its snapshot path, sorted by source.
fn fixture_templates() -> Vec<(PathBuf, PathBuf)> {
    let root = manifest_dir();
    let mut out = Vec::new();
    for (dir, sub) in TEMPLATE_DIRS {
        let entries = std::fs::read_dir(root.join(dir))
            .unwrap_or_else(|e| panic!("read template directory {dir}: {e}"));
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
            let snapshot = root
                .join(SNAPSHOT_DIR)
                .join(sub)
                .join(format!("{stem}.json"));
            out.push((path, snapshot));
        }
    }
    out.sort();
    out
}

/// The compiled JSON exactly as `koto` hashes it: `compile(.., false)`, as
/// `koto init` and `koto next` do, serialized with `to_string_pretty`.
fn compiled_json(source: &Path) -> String {
    let compiled = koto::template::compile::compile(source, false)
        .unwrap_or_else(|e| panic!("compile {}: {e:#}", source.display()));
    serde_json::to_string_pretty(&compiled).unwrap()
}

#[test]
fn fixture_templates_compile_byte_identical_to_their_snapshots() {
    let templates = fixture_templates();
    assert!(
        templates.len() >= 20,
        "expected at least 20 fixture templates, found {}; did a directory move?",
        templates.len()
    );

    let mut failures = Vec::new();
    for (source, snapshot) in &templates {
        let actual = compiled_json(source);
        match std::fs::read_to_string(snapshot) {
            Ok(expected) if expected == actual => {}
            Ok(_) => failures.push(format!(
                "{} no longer compiles to {}",
                source.display(),
                snapshot.display()
            )),
            Err(e) => failures.push(format!(
                "{} has no snapshot at {} ({e})",
                source.display(),
                snapshot.display()
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "compiled-template identity broke:\n  {}\n\n\
         A template that doesn't use a new feature must compile to the same \
         bytes as before, or every existing session's template hash stops \
         matching. Fix the compiler. The one legitimate reason to regenerate \
         is a new fixture template, whose snapshot doesn't exist yet; see \
         `regenerate_compiled_snapshots`.",
        failures.join("\n  ")
    );
}

/// A snapshot whose source was deleted or renamed would otherwise sit in the
/// directory testing nothing.
#[test]
fn every_compiled_snapshot_has_a_source_template() {
    let expected: Vec<PathBuf> = fixture_templates().into_iter().map(|(_, s)| s).collect();
    let mut stray = Vec::new();
    let mut dirs = vec![manifest_dir().join(SNAPSHOT_DIR)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if !expected.contains(&path) {
                stray.push(path.display().to_string());
            }
        }
    }
    assert!(
        stray.is_empty(),
        "snapshots with no fixture template behind them: {stray:?}"
    );
}

/// Regeneration helper for compiled snapshots.
///
/// Use it only to record a snapshot for a fixture template that has none
/// yet, or after a deliberate compiled-format change that is announced as
/// breaking template hashes. It rewrites every snapshot, so run it and then
/// check with `git status` that no existing snapshot changed:
///
/// ```text
/// cargo test --test compat_baseline_test regenerate_compiled_snapshots -- --ignored
/// ```
#[test]
#[ignore = "regeneration helper; rewrites compiled snapshots"]
fn regenerate_compiled_snapshots() {
    for (source, snapshot) in fixture_templates() {
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, compiled_json(&source)).unwrap();
        println!("wrote {}", snapshot.display());
    }
}

// ===========================================================================
//  2. Failing `koto next` responses
// ===========================================================================

const RESPONSE_FIXTURE: &str = "tests/fixtures/next-response-baseline/failing-responses.json";

/// Stands in for the per-case temporary directory in any recorded string.
const ROOT_TOKEN: &str = "<ROOT>";

/// A command gate whose script writes one line to each stream and exits 1.
/// The command holds no path, so it lands in the fixture verbatim.
const COMMAND_GATE_TEMPLATE: &str = r#"---
name: failing-command-gate
version: "1.0"
initial_state: check
states:
  check:
    gates:
      tests:
        type: command
        command: "echo tests-stdout-line; echo tests-stderr-line >&2; exit 1"
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Run the tests.

## done

Done.
"#;

/// A context-exists gate on a key nothing writes.
const CONTEXT_GATE_TEMPLATE: &str = r#"---
name: failing-context-gate
version: "1.0"
initial_state: check
states:
  check:
    gates:
      review:
        type: context-exists
        key: review_note
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Wait for the review note.

## done

Done.
"#;

/// A default action that writes one line to each stream and exits 3.
const DEFAULT_ACTION_TEMPLATE: &str = r#"---
name: failing-default-action
version: "1.0"
initial_state: setup
states:
  setup:
    default_action:
      command: "echo action-stdout-line; echo action-stderr-line >&2; exit 3"
    transitions:
      - target: done
  done:
    terminal: true
---

## setup

Prepare the workspace.

## done

Done.
"#;

struct Case {
    label: &'static str,
    description: &'static str,
    template: &'static str,
}

const CASES: &[Case] = &[
    Case {
        label: "failing-command-gate",
        description: "A command gate whose script prints a line to stdout and one to stderr, then exits 1. The first `koto next` after `koto init`.",
        template: COMMAND_GATE_TEMPLATE,
    },
    Case {
        label: "failing-context-gate",
        description: "A context-exists gate on a key no one has added. The first `koto next` after `koto init`.",
        template: CONTEXT_GATE_TEMPLATE,
    },
    Case {
        label: "failing-default-action",
        description: "A default_action that prints a line to stdout and one to stderr, then exits 3. The first `koto next` after `koto init`.",
        template: DEFAULT_ACTION_TEMPLATE,
    },
];

const NOTES: &[&str] = &[
    "Parsed `koto next` response bodies for three failing checks, captured from koto before the failure-reporting feature changed any response.",
    "Each case runs in its own temporary HOME and KOTO_SESSIONS_BASE. Any occurrence of that directory in a string is replaced with <ROOT>.",
    "The rule these pin: a later koto may add optional keys to these responses but may not change or remove any key recorded here. `strip_added_keys` in tests/support/json_baseline.rs deletes keys the fixture lacks before comparing.",
];

fn koto_cmd(dir: &Path) -> Command {
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions);
    cmd.env("HOME", dir);
    cmd
}

fn run_ok(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = koto_cmd(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "`koto {}` failed: stdout={} stderr={}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Run one case in a fresh directory and return its tokenized response.
fn capture_case(case: &Case) -> Value {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let template = root.join("template.md");
    std::fs::write(&template, case.template).unwrap();
    run_ok(
        root,
        &["init", "wf", "--template", template.to_str().unwrap()],
    );
    let stdout = run_ok(root, &["next", "wf"]);
    let mut body: Value = serde_json::from_slice(&stdout)
        .unwrap_or_else(|e| panic!("{}: response is not JSON: {e}", case.label));

    // The canonical form first: on macOS the tempdir sits behind a symlink,
    // and the canonical path contains the raw one as a suffix.
    let mut pairs = Vec::new();
    if let Ok(canon) = root.canonicalize() {
        pairs.push((canon.to_str().unwrap().to_string(), ROOT_TOKEN));
    }
    pairs.push((root.to_str().unwrap().to_string(), ROOT_TOKEN));
    tokenize_strings(&mut body, &pairs);
    body
}

fn capture_responses() -> Value {
    let cases: Vec<Value> = CASES
        .iter()
        .map(|c| {
            serde_json::json!({
                "label": c.label,
                "description": c.description,
                "response": capture_case(c),
            })
        })
        .collect();
    serde_json::json!({ "notes": NOTES, "cases": cases })
}

fn load_response_fixture() -> Value {
    let raw = std::fs::read_to_string(manifest_dir().join(RESPONSE_FIXTURE))
        .unwrap_or_else(|e| panic!("read {RESPONSE_FIXTURE}: {e}"));
    serde_json::from_str(&raw).unwrap()
}

fn fixture_case<'a>(fixture: &'a Value, label: &str) -> &'a Value {
    fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .map(|c| &c["response"])
        .unwrap_or_else(|| panic!("{RESPONSE_FIXTURE} has no `{label}` case"))
}

/// The durable check: a failing response may only add keys.
#[test]
fn failing_responses_differ_from_baseline_only_by_added_keys() {
    let fixture = load_response_fixture();
    for case in CASES {
        let expected = fixture_case(&fixture, case.label);
        let mut actual = capture_case(case);
        strip_added_keys(&mut actual, expected);
        assert_eq!(
            &actual, expected,
            "{}: after deleting keys the baseline lacks, the response still \
             differs from {RESPONSE_FIXTURE}. A key the baseline records was \
             changed or removed, which breaks readers of today's response. \
             Fix the code; don't regenerate.",
            case.label
        );
    }
}

/// Today there is nothing to strip, which is what makes the check above
/// meaningful: the fixture is today's response, not a subset of it. The
/// first change that adds an optional field to one of these responses is
/// expected to retire this test; the added-keys test above stays.
#[test]
fn failing_responses_equal_baseline_unfiltered() {
    let fixture = load_response_fixture();
    for case in CASES {
        assert_eq!(
            &capture_case(case),
            fixture_case(&fixture, case.label),
            "{}: the unfiltered response differs from {RESPONSE_FIXTURE}",
            case.label
        );
    }
}

/// Guards against a regeneration that stops recording what the file claims.
#[test]
fn failing_response_fixture_covers_each_failure_kind() {
    let fixture = load_response_fixture();
    let blocking = |label: &str| -> Vec<String> {
        let resp = fixture_case(&fixture, label);
        assert_eq!(resp["action"], "gate_blocked", "{label}: {resp}");
        assert_eq!(resp["advanced"], false, "{label}: {resp}");
        resp["blocking_conditions"]
            .as_array()
            .unwrap_or_else(|| panic!("{label}: no blocking_conditions: {resp}"))
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(blocking("failing-command-gate"), vec!["tests"]);
    assert_eq!(blocking("failing-context-gate"), vec!["review"]);
    assert_eq!(blocking("failing-default-action"), vec!["__action__"]);

    let text = serde_json::to_string(&fixture["cases"]).unwrap();
    assert!(
        !text.contains("/tmp/") && !text.contains("/var/folders/"),
        "the fixture holds a machine-specific path"
    );
}

#[test]
fn strip_added_keys_removes_only_keys_the_baseline_lacks() {
    let baseline = serde_json::json!({
        "a": 1,
        "nested": { "b": "x" },
        "list": [{ "c": true }],
    });

    let mut added = serde_json::json!({
        "a": 1,
        "extra": [1, 2],
        "nested": { "b": "x", "more": null },
        "list": [{ "c": true, "d": 0 }],
    });
    strip_added_keys(&mut added, &baseline);
    assert_eq!(added, baseline, "added keys at every depth are stripped");

    let mut changed = serde_json::json!({
        "a": 2,
        "nested": { "b": "x" },
        "list": [{ "c": true }],
    });
    strip_added_keys(&mut changed, &baseline);
    assert_ne!(changed, baseline, "a changed value is not hidden");

    let mut removed = serde_json::json!({ "a": 1, "list": [{ "c": true }] });
    strip_added_keys(&mut removed, &baseline);
    assert_ne!(removed, baseline, "a removed key is not hidden");

    let mut longer = serde_json::json!({
        "a": 1,
        "nested": { "b": "x" },
        "list": [{ "c": true }, { "c": false }],
    });
    strip_added_keys(&mut longer, &baseline);
    assert_ne!(longer, baseline, "an added array element is not hidden");
}

/// Regeneration helper for the failing-response fixture.
///
/// While the failure-reporting feature is being built, a failure above is
/// the finding and this must not be run: it would record the changed
/// responses over the only copy of the old ones. It is for a later,
/// deliberate response-format change that the compatibility promise allows
/// to break, after this feature has shipped:
///
/// ```text
/// cargo test --test compat_baseline_test regenerate_failing_response_fixture -- --ignored
/// ```
#[test]
#[ignore = "regeneration helper; rewrites the failing-response fixture"]
fn regenerate_failing_response_fixture() {
    let path = manifest_dir().join(RESPONSE_FIXTURE);
    let mut doc = serde_json::to_string_pretty(&capture_responses()).unwrap();
    doc.push('\n');
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &doc).unwrap();
    println!("wrote {} ({} bytes)", path.display(), doc.len());
}
