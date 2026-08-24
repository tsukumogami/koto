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
