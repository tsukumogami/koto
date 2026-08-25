//! A context gate's `key` and `pattern` resolve `{{KEY}}` references (Issue #222).
//!
//! Before this, `substitute_gate_commands` rewrote only `Gate::command`, so a
//! state whose action stored `{{SESSION_NAME}}-note` and whose gate read
//! `key: "{{SESSION_NAME}}-note"` disagreed about what that reference meant.
//! The action was right and the gate asked the store for a key literally
//! spelled with the token, so the symptom was a gate that would not pass.
//!
//! `pattern` is a regex, so a substituted value there is escaped: the tests
//! below pin both halves of that, because "resolves" and "resolves literally"
//! are different claims and only one of them is the one that was decided.
//!
//! Each test's doc says which wrong implementation it rules out, because most of
//! them would pass against several. Compile-time refusals are mostly pinned as
//! unit tests in `src/template/types.rs`, which call the same `validate`; what
//! lives here is the behaviour a template author actually meets.

use assert_cmd::prelude::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("koto")
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    // Override HOME so the tests don't read the user's own config.
    cmd.env("HOME", dir);
    // A default_action inherits this environment, so putting the built binary
    // first on PATH is what lets an action shell out to `koto` itself -- which
    // is how the motivating case stores the key its own gate then reads.
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            koto_binary().parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd
}

/// Run `koto init` and return the process output without asserting on it, for
/// the cases where the failure is the thing under test.
fn try_init(dir: &Path, name: &str, template: &str) -> std::process::Output {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap()
}

fn init_workflow(dir: &Path, name: &str, template: &str) {
    let output = try_init(dir, name, template);
    assert!(
        output.status.success(),
        "init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn next(dir: &Path, name: &str) -> Value {
    let output = koto_cmd(dir).args(["next", name]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "next should print one JSON object: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Add a context key through the CLI, for the cases that do not need an action
/// to have written it.
fn context_add(dir: &Path, session: &str, key: &str, content: &str) {
    let file = dir.join("ctx-payload.txt");
    std::fs::write(&file, content).unwrap();
    let output = koto_cmd(dir)
        .args([
            "context",
            "add",
            session,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "context add failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The named gate's entry in a `gate_blocked` response, or `None` when the
/// response is not a block.
fn blocking_condition<'a>(resp: &'a Value, gate: &str) -> Option<&'a Value> {
    resp.get("blocking_conditions")?
        .as_array()?
        .iter()
        .find(|c| c["name"] == gate)
}

// ---------------------------------------------------------------------------
// context-exists: the motivating case
// ---------------------------------------------------------------------------

/// The issue's own reproduction, end to end: one state stores a context key
/// scoped by session name and gates on that same key. Both halves must read the
/// reference the same way.
#[test]
fn context_exists_gate_key_resolves_session_name() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.txt"), "the note").unwrap();

    let template = r#"---
name: gate-key-session-scope
version: "1.0"
initial_state: s
states:
  s:
    default_action:
      command: 'koto context add {{SESSION_NAME}} {{SESSION_NAME}}-note --from-file note.txt'
      fallback: "the action did not run"
    gates:
      has_note:
        type: context-exists
        key: "{{SESSION_NAME}}-note"
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Store the note.

## done

Done.
"#;

    init_workflow(dir.path(), "kprobe1", template);
    let resp = next(dir.path(), "kprobe1");

    // The action wrote the real key, so the gate must find it.
    assert!(
        blocking_condition(&resp, "has_note").is_none(),
        "the gate should not block on a key its own state just stored; got {resp}"
    );
    assert_eq!(
        resp["action"], "done",
        "the state should advance past the gate; got {resp}"
    );
}

/// The key the gate asks for is the substituted one, checked against the store
/// rather than inferred from the transition. This is the assertion that names
/// the defect: on the old behaviour the store held `kprobe2-note` while the gate
/// asked for `{{SESSION_NAME}}-note`.
#[test]
fn context_exists_gate_reads_the_key_the_action_wrote() {
    let dir = TempDir::new().unwrap();

    let template = r#"---
name: gate-key-store-agreement
version: "1.0"
initial_state: s
states:
  s:
    gates:
      has_note:
        type: context-exists
        key: "{{SESSION_NAME}}-note"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.has_note.exists: true
  done:
    terminal: true
---

## s

Wait for the note.

## done

Done.
"#;

    init_workflow(dir.path(), "kprobe2", template);

    // Nothing stored yet: the gate reports the substituted key absent, which is
    // the correct answer to the right question.
    let before = next(dir.path(), "kprobe2");
    let cond = blocking_condition(&before, "has_note")
        .unwrap_or_else(|| panic!("gate should block while the key is absent; got {before}"));
    assert_eq!(cond["output"]["exists"], false);

    // Store it under the resolved name and the same gate passes. Under the old
    // behaviour it kept blocking, because it was asking for a key nobody writes.
    context_add(dir.path(), "kprobe2", "kprobe2-note", "the note");
    let after = next(dir.path(), "kprobe2");
    assert!(
        blocking_condition(&after, "has_note").is_none(),
        "gate should pass once the substituted key is present; got {after}"
    );
}

// ---------------------------------------------------------------------------
// context-matches: key and pattern
// ---------------------------------------------------------------------------

/// Both fields of a `context-matches` gate resolve, and the value written into
/// the pattern matches itself.
#[test]
fn context_matches_gate_key_and_pattern_resolve() {
    let dir = TempDir::new().unwrap();

    let template = r#"---
name: gate-matches-both-fields
version: "1.0"
initial_state: s
states:
  s:
    gates:
      is_ready:
        type: context-matches
        key: "{{SESSION_NAME}}-status"
        pattern: "^ready for {{SESSION_NAME}}$"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.is_ready.matches: true
  done:
    terminal: true
---

## s

Wait for the status.

## done

Done.
"#;

    init_workflow(dir.path(), "mprobe1", template);
    context_add(dir.path(), "mprobe1", "mprobe1-status", "ready for mprobe1");

    let resp = next(dir.path(), "mprobe1");
    assert!(
        blocking_condition(&resp, "is_ready").is_none(),
        "both key and pattern should resolve; got {resp}"
    );
}

/// The decision on `pattern`: a substituted value is escaped, so it matches
/// itself and nothing else.
///
/// A session name may contain a dot (`validate_workflow_name` permits one, and
/// the batch-coordinator example names children `research.topic-1`). Unescaped,
/// that dot is a wildcard and a gate written to scope by session name would
/// admit sessions it was written to exclude. Both halves are asserted here
/// because each rules out a different wrong implementation: the first fails if
/// the value is not substituted at all -- the raw token is not even a valid
/// regex -- and the second fails if it is substituted without escaping.
#[test]
fn context_matches_pattern_matches_the_value_literally() {
    let template = r#"---
name: gate-matches-literal-value
version: "1.0"
initial_state: s
states:
  s:
    gates:
      is_ready:
        type: context-matches
        key: status
        pattern: "^saw {{SESSION_NAME}}$"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.is_ready.matches: true
  done:
    terminal: true
---

## s

Wait for the status.

## done

Done.
"#;

    // The dot means itself: the session's own name matches.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "probe.one", template);
    context_add(dir.path(), "probe.one", "status", "saw probe.one");
    let resp = next(dir.path(), "probe.one");
    assert!(
        blocking_condition(&resp, "is_ready").is_none(),
        "the substituted session name should match itself; got {resp}"
    );

    // The dot is not a wildcard: a different string of the same shape does not.
    let dir = TempDir::new().unwrap();
    init_workflow(dir.path(), "probe.one", template);
    context_add(dir.path(), "probe.one", "status", "saw probeXone");
    let resp = next(dir.path(), "probe.one");
    let cond = blocking_condition(&resp, "is_ready").unwrap_or_else(|| {
        panic!("a dot in the substituted value must not act as a wildcard; got {resp}")
    });
    assert_eq!(cond["output"]["matches"], false);
    assert_eq!(
        cond["output"]["error"], "",
        "the pattern should be a valid regex, not a passed-through token; got {resp}"
    );
}

// ---------------------------------------------------------------------------
// compile time
// ---------------------------------------------------------------------------

/// Substituting these fields without validating them would relocate the silent
/// failure rather than end it: an undeclared reference passes through as its raw
/// token and the store is asked for a key nobody will ever write. So the
/// compiler refuses it, naming the gate and the field.
#[test]
fn undeclared_reference_in_gate_key_is_rejected_at_compile_time() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-key-undeclared
version: "1.0"
initial_state: s
states:
  s:
    gates:
      has_note:
        type: context-exists
        key: "{{NOT_DECLARED}}-note"
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

    let output = try_init(dir.path(), "badkey", template);
    assert!(!output.status.success(), "init should refuse the template");
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        message.contains("not declared")
            && message.contains("NOT_DECLARED")
            && message.contains("has_note")
            && message.contains("key"),
        "the error should say the reference is undeclared and name the gate and the field; \
         got {message}"
    );
}

/// A value supplied with `--var` reaches both fields, not just the two runtime
/// names.
///
/// The two layers substitute in separate passes -- runtime names first, then the
/// overlay and the log's bindings -- so a test using only `{{SESSION_NAME}}`
/// would pass with the declared-variable layer wired to neither field. The
/// value carries a dot so the escaping is exercised on this layer too.
/// Compile-time acceptance of the same shape is pinned by
/// `accepts_declared_and_runtime_names_in_gate_key_and_pattern` in
/// `src/template/types.rs`; what this adds is the value arriving at the store
/// and at the regex.
#[test]
fn a_declared_variable_resolves_in_gate_key_and_pattern() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-declared-var
version: "1.0"
initial_state: s
variables:
  SLUG:
    description: "scopes the context key"
    required: true
states:
  s:
    gates:
      is_ready:
        type: context-matches
        key: "{{SLUG}}-status"
        pattern: "^done: {{SLUG}}$"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.is_ready.matches: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

    let src = dir.path().join("declared-template.md");
    std::fs::write(&src, template).unwrap();
    let output = koto_cmd(dir.path())
        .args([
            "init",
            "vprobe1",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "SLUG=my.slug",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init should accept a declared reference: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    context_add(dir.path(), "vprobe1", "my.slug-status", "done: my.slug");
    let resp = next(dir.path(), "vprobe1");
    assert!(
        blocking_condition(&resp, "is_ready").is_none(),
        "a declared variable should resolve in both fields; got {resp}"
    );
}

// ---------------------------------------------------------------------------
// the polling loop
// ---------------------------------------------------------------------------

/// A polling action re-evaluates its state's gates from inside its own loop.
/// That evaluation has to resolve the same names the advance loop resolves, or a
/// gate passes outside the loop and never inside it -- which is the drift PR #223
/// found for a gate's `command`, from a second copy of the substitution that has
/// since been consolidated into one helper.
///
/// The key is stored before the tick, so on a build that substitutes it the
/// first in-loop evaluation passes and the action returns at once. On one that
/// does not, the loop asks for the literal token until it times out.
#[test]
fn a_polling_loop_resolves_the_gate_key_it_re_evaluates() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: polling-gate-key
version: "1.0"
initial_state: wait
states:
  wait:
    default_action:
      command: 'true'
      polling:
        interval_secs: 1
        timeout_secs: 5
    gates:
      has_note:
        type: context-exists
        key: "{{SESSION_NAME}}-note"
    transitions:
      - target: done
  done:
    terminal: true
---

## wait

Wait for the note.

## done

Done.
"#;

    init_workflow(dir.path(), "pprobe1", template);
    context_add(dir.path(), "pprobe1", "pprobe1-note", "the note");

    let resp = next(dir.path(), "pprobe1");
    assert!(
        blocking_condition(&resp, "has_note").is_none(),
        "the in-loop evaluation should see the substituted key; got {resp}"
    );
    assert_eq!(
        resp["action"], "done",
        "the polling loop should return on the first evaluation; got {resp}"
    );
}

// ---------------------------------------------------------------------------
// a reference that resolves to nothing
// ---------------------------------------------------------------------------

/// [`init_workflow`] under another name, so a call site reads as "no `--var` is
/// passed here, deliberately".
///
/// Every caller declares an optional variable and leaves it unsupplied, which
/// materializes it empty -- the setup the tests below are about. The alias
/// enforces nothing; it labels intent that would otherwise be invisible in a
/// call that differs from its neighbours only by what it omits.
fn init_with_optional(dir: &Path, name: &str, template: &str) {
    let src = dir.join(format!("{}-template.md", name));
    std::fs::write(&src, template).unwrap();
    let output = koto_cmd(dir)
        .args(["init", name, "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init should accept the template: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A pattern that collapses to empty fails closed, loudly.
///
/// An empty regex matches every input, so with no guard a gate whose pattern is
/// nothing but a `{{KEY}}` with an empty value would pass on content it was
/// written to reject. That is the failing-open direction, and it is worse than
/// the failing-closed symptom this issue was filed about -- so it is worth a
/// guard even though the compiler cannot see it, since the compiler reads the
/// pattern before substitution.
#[test]
fn a_pattern_that_resolves_to_empty_is_an_error_not_a_pass() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-empty-pattern
version: "1.0"
initial_state: s
variables:
  STATUS:
    description: "the status the gate looks for"
    required: false
states:
  s:
    gates:
      is_ready:
        type: context-matches
        key: status
        pattern: "{{STATUS}}"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.is_ready.matches: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

    init_with_optional(dir.path(), "eprobe1", template);
    context_add(dir.path(), "eprobe1", "status", "definitely not ready");

    let resp = next(dir.path(), "eprobe1");
    let cond = blocking_condition(&resp, "is_ready").unwrap_or_else(|| {
        panic!("an empty pattern must not pass on arbitrary content; got {resp}")
    });
    assert_eq!(cond["output"]["matches"], false);
    assert!(
        cond["output"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("empty"),
        "the gate should say why rather than reporting a bare mismatch; got {resp}"
    );
}

/// A key that substitutes into something the store will not accept says so,
/// rather than reporting the key absent.
///
/// The store answers an unusable key exactly as it answers a missing one, so
/// left alone this is a gate that will not pass with nothing pointing at why --
/// the exact symptom of the issue, reached through the fix for it.
///
/// Three cases. A key that resolves to nothing at all, whose message must name
/// emptiness because that is the wording an operator greps for. The sharper
/// shape a hyphen makes, since a key component must start with an alphanumeric,
/// so `{{PREFIX}}-note` with an empty prefix leaves `-note` -- one character
/// from `{{PREFIX}}note`, which works and is the documented reason `key` takes
/// the plain form. And the same thing on a `context-matches` gate, which is a
/// separate call site reporting under a different evidence key: get that key
/// wrong and the reason is populated in a field no `when` clause routes on, so
/// the gate blocks forever with the answer sitting right there.
#[test]
fn a_key_that_resolves_to_something_unusable_says_so() {
    // (case, gate type, extra gate field, evidence key, expected in the message)
    let cases = [
        ("{{SCOPE}}", "context-exists", "", "exists", "empty"),
        ("{{SCOPE}}-note", "context-exists", "", "exists", "letter"),
        (
            "{{SCOPE}}-note",
            "context-matches",
            "\n        pattern: \"^ok$\"",
            "matches",
            "letter",
        ),
    ];

    for (n, (key_expr, gate_type, extra, field, expected)) in cases.iter().enumerate() {
        let dir = TempDir::new().unwrap();
        let template = format!(
            r#"---
name: gate-unusable-key
version: "1.0"
initial_state: s
variables:
  SCOPE:
    description: "scopes the context key"
    required: false
states:
  s:
    gates:
      has_note:
        type: {gate_type}
        key: "{key_expr}"{extra}
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.has_note.{field}: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#
        );

        let name = format!("uprobe{n}");
        init_with_optional(dir.path(), &name, &template);

        let resp = next(dir.path(), &name);
        let cond = blocking_condition(&resp, "has_note")
            .unwrap_or_else(|| panic!("the gate should block for {key_expr}; got {resp}"));
        // The evidence key the gate's shape carries is still present, so a
        // `when` clause routing on it still has something to read.
        assert_eq!(
            cond["output"][field], false,
            "the {field} evidence key should be present and false; got {resp}"
        );
        let message = cond["output"]["error"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected),
            "the {gate_type} gate's message for {key_expr} should mention {expected:?}; \
             got {resp}"
        );
        assert!(
            message.contains("remedy:"),
            "the message should say what to change; got {resp}"
        );
    }
}

/// An empty value that only partly fills a key still leaves the rest, which is
/// the behaviour the plain form was chosen for: `{{PREFIX}}note` with no prefix
/// asks the store for `note`, not for `''note`.
#[test]
fn an_empty_value_inside_a_larger_key_renders_the_rest() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-partial-key
version: "1.0"
initial_state: s
variables:
  PREFIX:
    description: "optional scope prefix"
    required: false
states:
  s:
    gates:
      has_note:
        type: context-exists
        key: "{{PREFIX}}note"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.has_note.exists: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

    init_with_optional(dir.path(), "eprobe3", template);
    context_add(dir.path(), "eprobe3", "note", "the note");

    let resp = next(dir.path(), "eprobe3");
    assert!(
        blocking_condition(&resp, "has_note").is_none(),
        "an empty prefix should leave the rest of the key intact; got {resp}"
    );
}

// ---------------------------------------------------------------------------
// Issue #227: the value grammar is wider than the key grammar
// ---------------------------------------------------------------------------

/// A value koto documents as legal, substituted into a gate's `key`, says which
/// character makes the result unusable.
///
/// This is the case Issue #227 was filed about and the one the tests above miss.
/// They cover a key that resolved to nothing and a key left with a leading
/// hyphen -- both reachable only from an unset optional variable. This one needs
/// no mistake at all: `VALUE_PATTERN` admits a space, a `:` and an `@` on
/// purpose, so a calendar title or a filter expression is a legal value, and
/// `validate_context_key` admits none of the three. A template that scopes a key
/// on such a value is doing the obvious thing.
///
/// Left alone the store answers an unusable key exactly as it answers a missing
/// one, so the gate would report `{"exists": false, "error": ""}` -- a gate that
/// will not pass with nothing pointing at why.
///
/// Each of the three characters gets its own case rather than one standing for
/// the family: they enter `validate_context_key` by two different routes (the
/// first-character rule and the trailing-character rule), and a change that
/// stopped reporting one of them would leave the other two passing.
#[test]
fn a_legal_value_that_cannot_be_a_key_says_which_character() {
    // (value, the character the message must name)
    let cases = [
        ("Weekly Planning", "' '"),
        ("newer_than:90d", "':'"),
        ("user@example.com", "'@'"),
    ];

    for (n, (title, character)) in cases.iter().enumerate() {
        let dir = TempDir::new().unwrap();
        let template = r#"---
name: gate-legal-value-illegal-key
version: "1.0"
initial_state: s
variables:
  TITLE:
    description: "names the thing the note is about"
    required: true
states:
  s:
    gates:
      has_note:
        type: context-exists
        key: "{{TITLE}}-note"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.has_note.exists: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

        let name = format!("tprobe{n}");
        let src = dir.path().join("title-template.md");
        std::fs::write(&src, template).unwrap();
        let output = koto_cmd(dir.path())
            .args([
                "init",
                &name,
                "--template",
                src.to_str().unwrap(),
                "--var",
                &format!("TITLE={title}"),
            ])
            .output()
            .unwrap();
        // The value's legality is half the point: if `--var` ever stopped
        // accepting these, the asymmetry would have closed from the other side
        // and this test would be asserting something that cannot happen.
        assert!(
            output.status.success(),
            "koto documents {title:?} as a legal value: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let resp = next(dir.path(), &name);
        let cond = blocking_condition(&resp, "has_note")
            .unwrap_or_else(|| panic!("the gate should block for {title:?}; got {resp}"));
        assert_eq!(
            cond["output"]["exists"], false,
            "the exists evidence key should still be present and false; got {resp}"
        );

        let message = cond["output"]["error"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "an unusable key must not report as a bare absence; got {resp}"
        );
        assert!(
            message.contains(character),
            "the message for {title:?} should name {character}; got {message}"
        );
        assert!(
            message.contains(&format!("{title}-note")),
            "the message should quote the substituted key, not the reference; got {message}"
        );
    }
}

/// The gate and the CLI word a refusal identically.
///
/// Two surfaces answer the same question about the same key, and before this
/// they answered it in different words -- the gate in gate-shaped language and
/// the CLI not at all. Equality is the assertion rather than "both mention the
/// character", because near-identical wordings are how the two drift apart: a
/// caller comparing them would pass either way, and only a byte-for-byte check
/// notices the day one of them gains a clause.
#[test]
fn the_gate_and_the_cli_word_a_refusal_identically() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-wording-parity
version: "1.0"
initial_state: s
variables:
  TITLE:
    description: "names the thing the note is about"
    required: true
states:
  s:
    gates:
      has_note:
        type: context-exists
        key: "{{TITLE}}-note"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.has_note.exists: true
  done:
    terminal: true
---

## s

Wait.

## done

Done.
"#;

    let src = dir.path().join("parity-template.md");
    std::fs::write(&src, template).unwrap();
    let output = koto_cmd(dir.path())
        .args([
            "init",
            "wprobe1",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "TITLE=Weekly Planning",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "init should accept the value");

    let resp = next(dir.path(), "wprobe1");
    let cond = blocking_condition(&resp, "has_note")
        .unwrap_or_else(|| panic!("the gate should block; got {resp}"));
    let from_gate = cond["output"]["error"].as_str().unwrap_or_default();

    let cli = koto_cmd(dir.path())
        .args(["context", "exists", "wprobe1", "Weekly Planning-note"])
        .output()
        .unwrap();
    assert_eq!(
        cli.status.code(),
        Some(2),
        "the CLI should report the key unusable rather than absent"
    );
    let body: Value = serde_json::from_slice(&cli.stdout).unwrap_or_else(|e| {
        panic!(
            "the CLI should print a JSON error: {e}; got {}",
            String::from_utf8_lossy(&cli.stdout)
        )
    });
    let from_cli = body["error"].as_str().unwrap_or_default();

    assert_eq!(
        from_gate, from_cli,
        "the two surfaces must word one refusal one way"
    );
    assert!(
        !from_gate.is_empty(),
        "the shared wording should not be empty"
    );
}

// ---------------------------------------------------------------------------
// name_filter and fallback (Issues #224 and #228)
// ---------------------------------------------------------------------------

/// A child template that walks straight to a terminal state, so a parent's
/// `children-complete` gate has something to count.
fn child_template() -> &'static str {
    r#"---
name: filter-child
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Do the work.

## done

Done.
"#
}

/// Write a template to a file and return the path, without initializing anything.
fn write_template(dir: &Path, filename: &str, content: &str) -> PathBuf {
    let src = dir.join(filename);
    std::fs::write(&src, content).unwrap();
    src
}

/// Initialize a child under `parent`, advancing it to its terminal state.
fn spawn_terminal_child(dir: &Path, name: &str, parent: &str, child_src: &Path) {
    let output = koto_cmd(dir)
        .args([
            "init",
            name,
            "--template",
            child_src.to_str().unwrap(),
            "--parent",
            parent,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // `--no-cleanup` keeps the terminal child's session on disk, which is what
    // the parent's gate reads.
    let output = koto_cmd(dir)
        .args(["next", name, "--no-cleanup"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child advance failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `children-complete` gate's `name_filter` resolves `{{SESSION_NAME}}`, so a
/// gate scoped to the parent's own fan-out counts the children the parent
/// spawned (Issue #224).
///
/// This is the motivating case rather than a reduced one. Children are spawned
/// as `<parent>.research.<n>`, which is what the skill guidance points at, and
/// the gate scopes to them with `{{SESSION_NAME}}.research.`. Against a build
/// that does not substitute the field, no child name starts with a literal
/// `{{SESSION_NAME}}`, the gate matches nothing, and it reports "no matching
/// children found" -- the quiet failure the issue describes, since a filter that
/// matches nothing looks exactly like a fan-out that has not finished.
///
/// The unrelated child is what makes this a test of the *filter* rather than of
/// counting: a build that substituted the reference but dropped the prefix match
/// would also see the research child, and would additionally see this one.
#[test]
fn name_filter_resolves_the_session_name_it_scopes_to() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: parent-session-scoped
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      research_done:
        type: children-complete
        completion: "terminal"
        name_filter: "{{SESSION_NAME}}.research."
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.research_done.all_complete: true
  done:
    terminal: true
---

## wait

Wait for the research children.

## done

Done.
"#;

    let parent_src = write_template(dir.path(), "parent-scoped.md", template);
    let child_src = write_template(dir.path(), "filter-child.md", child_template());

    let output = koto_cmd(dir.path())
        .args(["init", "kprobe", "--template", parent_src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init should accept a runtime name in name_filter: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    spawn_terminal_child(dir.path(), "kprobe.research.r1", "kprobe", &child_src);
    spawn_terminal_child(dir.path(), "kprobe.other", "kprobe", &child_src);

    let resp = next(dir.path(), "kprobe");
    let cond = blocking_condition(&resp, "research_done")
        .unwrap_or_else(|| panic!("the gate should report its evidence; got {resp}"));

    // The gate's own verdict, not whether the response advanced. These children
    // carry no dereferenceable result, so the converge predicate holds the gate
    // regardless of the filter -- asserting on advancement would pass or fail
    // for a reason this test is not about.
    assert_eq!(
        cond["output"]["total"], 1,
        "the filter should resolve to `kprobe.research.` and match exactly the \
         research child; got {resp}"
    );
    assert_eq!(
        cond["output"]["children"][0]["name"], "kprobe.research.r1",
        "the matched child should be the one under the resolved prefix; got {resp}"
    );
    assert_eq!(
        cond["output"]["all_complete"], true,
        "the matched child is terminal, so the gate's completion verdict is \
         true; got {resp}"
    );
}

/// The same gate, counting. Pinned separately because "the gate passed" and
/// "the gate counted exactly the fan-out it names" are different claims, and a
/// build that resolved the reference but ignored the prefix would satisfy only
/// the first.
///
/// One research child is left short of terminal, so the gate does not pass and
/// its evidence is on the response to be read.
#[test]
fn name_filter_counts_only_the_fan_out_it_names() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: parent-counting
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      research_done:
        type: children-complete
        completion: "terminal"
        name_filter: "{{SESSION_NAME}}.research."
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.research_done.all_complete: true
  done:
    terminal: true
---

## wait

Wait for the research children.

## done

Done.
"#;

    let parent_src = write_template(dir.path(), "parent-counting.md", template);
    let child_src = write_template(dir.path(), "filter-child.md", child_template());

    koto_cmd(dir.path())
        .args(["init", "cprobe", "--template", parent_src.to_str().unwrap()])
        .output()
        .unwrap();

    spawn_terminal_child(dir.path(), "cprobe.research.r1", "cprobe", &child_src);

    // A second research child, initialized but never advanced, so the gate has
    // something to still be waiting on.
    koto_cmd(dir.path())
        .args([
            "init",
            "cprobe.research.r2",
            "--template",
            child_src.to_str().unwrap(),
            "--parent",
            "cprobe",
        ])
        .output()
        .unwrap();
    // And one outside the fan-out, also unfinished. A gate that ignored the
    // prefix would count three here rather than two.
    koto_cmd(dir.path())
        .args([
            "init",
            "cprobe.audit",
            "--template",
            child_src.to_str().unwrap(),
            "--parent",
            "cprobe",
        ])
        .output()
        .unwrap();

    let resp = next(dir.path(), "cprobe");
    let cond = blocking_condition(&resp, "research_done")
        .unwrap_or_else(|| panic!("the gate should not pass with r2 unfinished; got {resp}"));
    assert_eq!(
        cond["output"]["total"], 2,
        "the gate should count the two research children and not the audit child; got {resp}"
    );
}

/// An undeclared reference in `name_filter` is refused at compile time, naming
/// the gate and the field (Issue #224).
///
/// Without this the raw token passes through, the filter matches nothing, and
/// the gate blocks forever with nothing anywhere naming the reference -- and
/// the compile-time warning that catches a missing trailing dot stays silent,
/// because the trailing dot is there.
#[test]
fn undeclared_reference_in_name_filter_is_rejected_at_compile_time() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: parent-undeclared-filter
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      research_done:
        type: children-complete
        completion: "terminal"
        name_filter: "{{NOT_DECLARED}}.research."
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.research_done.all_complete: true
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

    let output = try_init(dir.path(), "badfilter", template);
    assert!(!output.status.success(), "init should refuse the template");
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        message.contains("not declared")
            && message.contains("NOT_DECLARED")
            && message.contains("research_done")
            && message.contains("name_filter"),
        "the error should say the reference is undeclared and name the gate and \
         the field; got {message}"
    );
}

/// A `name_filter` that resolves to empty is refused at the gate, not applied.
///
/// An empty prefix does not narrow the gate -- it removes the filter, so a gate
/// written to wait on one fan-out would silently wait on every child of the
/// parent. That is the fail-open direction, and worse than the symptom the
/// issue was filed about. The compiler cannot catch it: it reads the authored
/// string, and `"{{PREFIX}}"` is only empty once `PREFIX` resolves to nothing.
///
/// The unrelated child is load-bearing. Without it, a build that applied the
/// empty filter would find the same single child and pass, and the test would
/// report success for the behaviour it exists to reject.
#[test]
fn a_name_filter_that_resolves_to_empty_is_an_error_not_a_wider_match() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: parent-empty-filter
version: "1.0"
initial_state: wait
variables:
  PREFIX:
    description: "scopes the gate to one fan-out"
    required: false
states:
  wait:
    gates:
      research_done:
        type: children-complete
        completion: "terminal"
        name_filter: "{{PREFIX}}"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.research_done.all_complete: true
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

    let parent_src = write_template(dir.path(), "parent-empty.md", template);
    let child_src = write_template(dir.path(), "filter-child.md", child_template());

    let output = koto_cmd(dir.path())
        .args(["init", "eprobe", "--template", parent_src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init should accept an optional variable in name_filter: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Every child of this parent is terminal, so an empty filter -- which
    // matches all of them -- would pass the gate.
    spawn_terminal_child(dir.path(), "eprobe.research.r1", "eprobe", &child_src);
    spawn_terminal_child(dir.path(), "eprobe.audit", "eprobe", &child_src);

    let resp = next(dir.path(), "eprobe");
    let cond = blocking_condition(&resp, "research_done").unwrap_or_else(|| {
        panic!("an empty name_filter must not pass by matching every child; got {resp}")
    });
    let error = cond["output"]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("empty"),
        "the gate should say why rather than reporting a bare not-complete; got {resp}"
    );
}

/// A gate that declares no `name_filter` is untouched by the empty-value
/// refusal.
///
/// "Absent" and "resolved to empty" are different states, and the fix has to
/// tell them apart -- collapsing them is the specific wrong implementation the
/// design rules out. Without this test, a fix that treated a missing filter as
/// an empty one would break every unfiltered `children-complete` gate in
/// existence and no test here would say so.
#[test]
fn a_gate_with_no_name_filter_still_watches_every_child() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: parent-unfiltered
version: "1.0"
initial_state: wait
states:
  wait:
    gates:
      all_done:
        type: children-complete
        completion: "terminal"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
          gates.all_done.all_complete: true
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

    let parent_src = write_template(dir.path(), "parent-unfiltered.md", template);
    let child_src = write_template(dir.path(), "filter-child.md", child_template());

    koto_cmd(dir.path())
        .args(["init", "uprobe", "--template", parent_src.to_str().unwrap()])
        .output()
        .unwrap();

    spawn_terminal_child(dir.path(), "uprobe.research.r1", "uprobe", &child_src);
    spawn_terminal_child(dir.path(), "uprobe.audit", "uprobe", &child_src);

    let resp = next(dir.path(), "uprobe");
    let cond = blocking_condition(&resp, "all_done")
        .unwrap_or_else(|| panic!("the gate should report its evidence; got {resp}"));
    assert_eq!(
        cond["output"]["total"], 2,
        "an absent name_filter means no filter, so both children count; got {resp}"
    );
    assert_eq!(
        cond["output"]["all_complete"], true,
        "both children are terminal, so the gate's completion verdict is true; \
         got {resp}"
    );
    assert_eq!(
        cond["output"]["error"], "",
        "an absent name_filter is not an empty one, and must not reach the \
         empty-value refusal; got {resp}"
    );
}

/// A `{{KEY}}` reference in a `default_action`'s `fallback` is refused at
/// compile time (Issue #228).
///
/// The field is spliced onto a failure response's directive after substitution
/// has run, deliberately, so the prose reaches the agent as written. That is
/// documented behaviour and it does not change. What changes is that an author
/// who writes a reference there is now told, instead of finding out from an
/// agent that could not follow a pointer to `{{SESSION_DIR}}`.
///
/// The reference names a DECLARED variable, which is the case a check written
/// against undeclared names would miss and the one an author is most likely to
/// write: the same `{{V}}` resolves in the directive two lines away.
#[test]
fn a_reference_in_a_default_action_fallback_is_rejected_at_compile_time() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: action-fallback-ref
version: "1.0"
initial_state: s
variables:
  V:
    description: "a declared variable"
    required: true
states:
  s:
    default_action:
      command: 'false'
      fallback: "the run left something in {{V}}, go and read it"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
  done:
    terminal: true
---

## s

Directive prose sees {{V}}.

## done

Done.
"#;

    let src = write_template(dir.path(), "fallback-ref.md", template);
    let output = koto_cmd(dir.path())
        .args([
            "init",
            "fbprobe",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "V=alpha",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "init should refuse a reference in fallback: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        message.contains("fallback") && message.contains("never expanded"),
        "the error should name the field and say it is never expanded; got {message}"
    );
    assert!(
        message.contains("directive"),
        "the error should point at the directive, which does resolve a \
         reference -- telling an author their reference will not work without \
         saying where one would leaves them nowhere; got {message}"
    );
}

/// `fallback` prose with no reference in it still reaches the agent on failure,
/// unchanged.
///
/// The refusal above must not have been written broadly enough to refuse the
/// field itself, and the splice it guards is the whole reason the field exists.
#[test]
fn a_fallback_without_a_reference_still_reaches_the_agent() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: action-fallback-plain
version: "1.0"
initial_state: s
variables:
  V:
    description: "a declared variable"
    required: true
states:
  s:
    default_action:
      command: 'false'
      fallback: "the command failed; read the error above and carry on yourself"
    accepts:
      status:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          status: ok
  done:
    terminal: true
---

## s

Directive prose sees {{V}}.

## done

Done.
"#;

    let src = write_template(dir.path(), "fallback-plain.md", template);
    let output = koto_cmd(dir.path())
        .args([
            "init",
            "fbplain",
            "--template",
            src.to_str().unwrap(),
            "--var",
            "V=alpha",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init should accept literal fallback prose: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let resp = next(dir.path(), "fbplain");
    let directive = resp["directive"].as_str().unwrap_or_default();
    assert!(
        directive.contains("read the error above and carry on yourself"),
        "the fallback prose should be spliced onto the directive when the action \
         fails; got {resp}"
    );
    assert!(
        directive.contains("alpha"),
        "the directive itself still resolves its own reference; got {resp}"
    );
}

// ---------------------------------------------------------------------------
// an undelivered capture in a gate field (Issue #225)
// ---------------------------------------------------------------------------

/// Run `koto next` and return (exit status code, parsed JSON), for the cases
/// where the exit status is part of what is being pinned.
///
/// `next` above unwraps the JSON and discards the status, which is right for a
/// blocked tick and wrong for a refusal: the whole claim here is that the run
/// stops with a particular code, and a helper that throws the code away cannot
/// tell that from a gate that merely failed.
fn next_with_status(dir: &Path, name: &str) -> (Option<i32>, Value) {
    let output = koto_cmd(dir).args(["next", name]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "next should print one JSON object: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code(), json)
}

/// The refusal's message, or a panic naming what came back instead.
fn capture_unset_message(resp: &Value) -> String {
    let err = resp
        .get("error")
        .unwrap_or_else(|| panic!("expected a capture_unset error; got {resp}"));
    assert_eq!(
        err["code"], "capture_unset",
        "the gate refusal reuses the code a directive and an action already get for \
         the same defect; got {resp}"
    );
    err["message"].as_str().unwrap_or_default().to_string()
}

/// A template whose first state gates on a capture a later state delivers.
///
/// `gate_body` is spliced under the gate's own name so one shape serves every
/// field: the command case, and the three that reach the store or the regex
/// engine rather than a shell.
fn undelivered_capture_template(gate_body: &str) -> String {
    format!(
        r#"---
name: gate-unset-capture
version: "1.0"
initial_state: s
states:
  s:
    gates:
      probe:
{gate_body}
    transitions:
      - target: producer
  producer:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Probe.

## producer

Produce.

## done

Done.
"#
    )
}

/// The issue's own reproduction: a gate command reading a capture the run has
/// not reached yet is refused, and the command does not run.
///
/// The side-effect file is the load-bearing half. A test that only checked the
/// exit status would pass against an implementation that refused *after*
/// handing the string to `sh -c`, which is the outcome the issue is about --
/// the token reached the shell and the gate answered on text nobody wrote.
#[test]
fn a_gate_command_reading_an_undelivered_capture_is_refused_before_it_runs() {
    let dir = TempDir::new().unwrap();
    let template = undelivered_capture_template(
        "        type: command\n        command: 'printf \"%s\" \"gate-{{TOKEN}}\" > gate-out.txt'",
    );

    init_workflow(dir.path(), "gprobe1", &template);
    let (code, resp) = next_with_status(dir.path(), "gprobe1");

    assert_eq!(code, Some(3), "the refusal exits 3; got {resp}");
    let message = capture_unset_message(&resp);
    for expected in ["'s'", "'probe'", "command", "{{TOKEN}}", "'producer'"] {
        assert!(
            message.contains(expected),
            "the message must name the state, the gate, the field, the capture and \
             the producing state; {expected} missing from {message:?}"
        );
    }
    assert!(
        !dir.path().join("gate-out.txt").exists(),
        "nothing may run before the refusal, and the gate command had a side effect \
         that proves whether it did"
    );
}

/// The other three gate fields get the same refusal, rather than the unrelated
/// errors they produce today.
///
/// Each of these already fails in some way, which is exactly why they are worth
/// pinning: `name_filter` blocks on a zero-child count with no diagnostic at
/// all, `pattern` reports an invalid regex, and `key` reports an unusable
/// character. All three send the operator to fix a value when the template's
/// state ordering is what is wrong.
#[test]
fn every_substitutable_gate_field_gets_the_same_refusal() {
    // (field name as the message reports it, the gate body that carries it)
    let cases = [
        (
            "key",
            "        type: context-exists\n        key: \"{{TOKEN}}\"",
        ),
        (
            "pattern",
            "        type: context-matches\n        key: status\n        pattern: \"{{TOKEN}}\"",
        ),
        (
            "name_filter",
            "        type: children-complete\n        name_filter: \"{{TOKEN}}\"",
        ),
    ];

    for (n, (field, gate_body)) in cases.iter().enumerate() {
        let dir = TempDir::new().unwrap();
        let template = undelivered_capture_template(gate_body);
        let name = format!("fprobe{n}");

        init_workflow(dir.path(), &name, &template);
        let (code, resp) = next_with_status(dir.path(), &name);

        assert_eq!(
            code,
            Some(3),
            "case {field}: the refusal exits 3; got {resp}"
        );
        let message = capture_unset_message(&resp);
        assert!(
            message.contains(field),
            "case {field}: the message must name the field that carried the \
             reference; got {message:?}"
        );
        assert!(
            message.contains("{{TOKEN}}") && message.contains("'producer'"),
            "case {field}: the message must name the capture and the producing \
             state; got {message:?}"
        );
    }
}

/// The polling loop refuses identically to the advance loop.
///
/// This is the position Issue #220 found the two paths drifted at, so it is
/// pinned by running the same reference through a state whose action polls --
/// where the gates are resolved on the way *into* the action rather than after
/// it -- and asserting the same code and the same five values come back.
#[test]
fn the_polling_loop_refuses_an_undelivered_capture_as_the_advance_loop_does() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-unset-capture-polling
version: "1.0"
initial_state: s
states:
  s:
    default_action:
      command: 'sleep 30'
      polling:
        interval_secs: 1
        timeout_secs: 20
    gates:
      probe:
        type: command
        command: 'printf "%s" "gate-{{TOKEN}}" > gate-out.txt'
    transitions:
      - target: producer
  producer:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Poll.

## producer

Produce.

## done

Done.
"#;

    init_workflow(dir.path(), "pprobe1", template);
    let (code, resp) = next_with_status(dir.path(), "pprobe1");

    assert_eq!(
        code,
        Some(3),
        "the polling position refuses with the same status as the advance \
         position; got {resp}"
    );
    let message = capture_unset_message(&resp);
    for expected in ["'s'", "'probe'", "command", "{{TOKEN}}", "'producer'"] {
        assert!(
            message.contains(expected),
            "the polling refusal carries the same five values as the advance one; \
             {expected} missing from {message:?}"
        );
    }
    assert!(
        !dir.path().join("gate-out.txt").exists(),
        "the polling refusal happens before the action is spawned, so the gate \
         command cannot have run either"
    );
}

/// A gate reading the capture its own state's non-polling action delivers still
/// resolves.
///
/// This is the case a check hoisted ahead of the action would break. The action
/// runs first and the state's gates are evaluated afterwards, so by the time the
/// gate is resolved the value exists -- and refusing it would be a false
/// refusal on a template that works today.
#[test]
fn a_gate_reading_its_own_states_capture_still_resolves() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-own-capture
version: "1.0"
initial_state: s
states:
  s:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    gates:
      probe:
        type: command
        command: 'test "{{TOKEN}}" = "abc"'
    transitions:
      - target: done
        when:
          gates.probe.exit_code: 0
  done:
    terminal: true
---

## s

Produce and check.

## done

Done.
"#;

    init_workflow(dir.path(), "oprobe1", template);
    let (code, resp) = next_with_status(dir.path(), "oprobe1");

    assert_ne!(
        code,
        Some(3),
        "the action delivers the value before the gate is resolved, so this must \
         not refuse; got {resp}"
    );
    assert_eq!(
        resp["state"], "done",
        "the gate should resolve the value and pass; got {resp}"
    );
}

/// The same gate under a polling action is refused, and says so in its own
/// terms.
///
/// A polling action's gates are substituted before its command finishes, so the
/// overlay cannot hold the capture that action delivers. The general sentence
/// would tell the operator the run has not entered a state it is standing in,
/// which is why the message is different here.
#[test]
fn a_polling_gate_reading_its_own_states_capture_says_so() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-own-capture-polling
version: "1.0"
initial_state: s
states:
  s:
    default_action:
      command: 'sleep 30'
      capture_stdout_as: TOKEN
      polling:
        interval_secs: 1
        timeout_secs: 20
    gates:
      probe:
        type: command
        command: 'test "{{TOKEN}}" = "abc"'
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Poll.

## done

Done.
"#;

    init_workflow(dir.path(), "sprobe1", template);
    let (code, resp) = next_with_status(dir.path(), "sprobe1");

    assert_eq!(
        code,
        Some(3),
        "this reference can never resolve; got {resp}"
    );
    let message = capture_unset_message(&resp);
    assert!(
        message.contains("that same state's default_action"),
        "the message must say the gate reads the capture its own state delivers, \
         rather than naming a state the run has not entered; got {message:?}"
    );
    assert!(
        !message.contains("has not entered that state"),
        "the general wording would be wrong here -- the run is standing in that \
         state; got {message:?}"
    );
}

/// A capture an earlier state delivered on this same tick resolves in a later
/// state's gate.
///
/// The no-regression half. An implementation that refused every gate carrying a
/// capture reference would pass every test above and fail this one.
#[test]
fn a_capture_delivered_earlier_in_the_same_tick_resolves_in_a_gate() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-delivered-capture
version: "1.0"
initial_state: producer
states:
  producer:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    transitions:
      - target: s
  s:
    gates:
      probe:
        type: command
        command: 'test "{{TOKEN}}" = "abc"'
    transitions:
      - target: done
        when:
          gates.probe.exit_code: 0
  done:
    terminal: true
---

## producer

Produce.

## s

Check.

## done

Done.
"#;

    init_workflow(dir.path(), "dprobe1", template);
    let (code, resp) = next_with_status(dir.path(), "dprobe1");

    assert_ne!(
        code,
        Some(3),
        "a delivered capture must still resolve; got {resp}"
    );
    assert_eq!(
        resp["state"], "done",
        "the gate should have resolved the value the earlier state produced on this \
         same tick; got {resp}"
    );
}

/// A capture delivered on an earlier tick resolves on a later one.
///
/// The same claim across a tick boundary, where the value arrives from the log's
/// bindings rather than from the per-tick overlay. Both layers are consulted,
/// and a fix that checked only one of them would pass the test above and fail
/// this one.
#[test]
fn a_capture_delivered_on_an_earlier_tick_resolves_in_a_gate() {
    let dir = TempDir::new().unwrap();
    let template = r#"---
name: gate-earlier-tick-capture
version: "1.0"
initial_state: producer
states:
  producer:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    accepts:
      ready:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: s
        when:
          ready: ok
  s:
    gates:
      probe:
        type: command
        command: 'test "{{TOKEN}}" = "abc"'
    transitions:
      - target: done
        when:
          gates.probe.exit_code: 0
  done:
    terminal: true
---

## producer

Produce.

## s

Check.

## done

Done.
"#;

    init_workflow(dir.path(), "tprobe1", template);
    // First tick runs the action and stops for evidence, so the capture reaches
    // the log rather than only the tick's own overlay.
    let (first, first_resp) = next_with_status(dir.path(), "tprobe1");
    assert_ne!(
        first,
        Some(3),
        "the first tick must not refuse; got {first_resp}"
    );

    let output = koto_cmd(dir.path())
        .args(["next", "tprobe1", "--with-data", r#"{"ready":"ok"}"#])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resp: Value = serde_json::from_str(stdout.trim()).unwrap();

    assert_ne!(
        output.status.code(),
        Some(3),
        "a capture from an earlier tick must still resolve; got {resp}"
    );
    assert_eq!(
        resp["state"], "done",
        "the gate should have resolved the value the earlier tick produced; got {resp}"
    );
}
