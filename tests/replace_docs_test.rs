//! What the guides and references owe a reader about restarting a run:
//! `koto init NEW --replaces OLD` and `koto session replace OLD --by NEW`,
//! the close they write in OLD's log (a `workflow_cancelled` naming NEW and
//! a `session_superseded` fence marked `replaced`), how `koto status` reads
//! a replaced run, and the codes both commands refuse with.
//!
//! Text is compared after joining Rust string continuations
//! (backslash-newline) and collapsing runs of whitespace, case-insensitive,
//! so a phrase wrapped across lines still matches.

#[path = "support/test_home.rs"]
mod test_home;

use std::path::PathBuf;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

/// `text` with backslash-newline continuations joined, whitespace runs
/// collapsed to one space, and lowercased.
fn normalize(text: &str) -> String {
    let joined = text.replace("\\\n", " ");
    joined
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The markdown heading level of `line` (the count of leading `#`), or
/// `None` when it isn't a heading.
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (hashes > 0 && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// The section of `text` that starts at the line equal to `heading`, up to
/// the next heading of the same level or above. `None` when no line is
/// `heading`. Lines inside fenced code blocks are never headings.
fn section(text: &str, heading: &str) -> Option<String> {
    let level = heading_level(heading).expect("a heading");
    let mut out = Vec::new();
    let mut inside = false;
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced {
            if inside {
                if heading_level(line).is_some_and(|l| l <= level) {
                    break;
                }
            } else if line.trim_end() == heading {
                inside = true;
            }
        }
        if inside {
            out.push(line);
        }
    }
    inside.then(|| out.join("\n"))
}

/// The section `inner` inside the section `outer` of `text`.
fn nested(text: &str, outer: &str, inner: &str) -> Option<String> {
    section(&section(text, outer)?, inner)
}

/// Each phrase of `needed` that `text` lacks, as `file: phrase`.
fn missing_from(file: &str, text: &str, needed: &[&str]) -> Vec<String> {
    let text = normalize(text);
    needed
        .iter()
        .filter(|p| !text.contains(&normalize(p)))
        .map(|p| format!("{file}: {p}"))
        .collect()
}

/// Check `needed` against the section `found`, or report the section as
/// missing under `what`.
fn check(missing: &mut Vec<String>, what: &str, found: Option<String>, needed: &[&str]) {
    match found {
        Some(sec) => missing.extend(missing_from(what, &sec, needed)),
        None => missing.push(format!("{what}: the section itself")),
    }
}

const GUIDE: &str = "docs/guides/cli-usage.md";
const REFERENCE: &str = "plugins/koto-skills/skills/koto-user/references/command-reference.md";
const CODES: &str = "docs/reference/error-codes.md";

/// Ways of saying the restart verbs refuse the cloud backend, which stopped
/// being true once the cloud close landed.
const CLOUD_REFUSED: &[&str] = &[
    "under the cloud backend it refuses with `invalid_usage`",
    "local backend only: under the cloud backend",
    "the command needs the local backend",
];

#[test]
fn the_guide_and_the_command_reference_describe_init_replaces_and_session_replace() {
    let mut missing = Vec::new();

    let guide = read(GUIDE);
    check(
        &mut missing,
        "docs/guides/cli-usage.md (### init > #### Replacing a run)",
        nested(&guide, "### init", "#### Replacing a run"),
        &[
            "--replaces <old>",
            "--abandon-tick",
            "--wait",
            "koto/replaces",
            "workflow_cancelled",
            "replaced_by",
            "replaced_by_session_id",
            "\"replaced\": true",
            "tick_in_flight",
            "\"tick_in_flight\": false",
            "session_busy",
            "waited_for_ms",
            "session_replaced",
            "session_exists",
            "session_rebound",
            "invalid_fence",
            "fence_unavailable",
            "koto cancel <new> --cleanup",
            "koto session replace",
            "--replace-terminal",
        ],
    );
    check(
        &mut missing,
        "docs/guides/cli-usage.md (### init > #### Superseding a session)",
        nested(&guide, "### init", "#### Superseding a session"),
        &["session_replaced"],
    );
    check(
        &mut missing,
        "docs/guides/cli-usage.md (### session > #### session replace)",
        nested(&guide, "### session", "#### session replace"),
        &[
            "--by <new>",
            "--abandon-tick",
            "koto/replaces",
            "tick_in_flight",
            "waited_for_ms",
            "invalid_usage",
            "invalid_fence",
            "session_rebound",
            "newest",
        ],
    );
    check(
        &mut missing,
        "docs/guides/cli-usage.md (### status)",
        section(&guide, "### status"),
        &["replaced_by", "\"status\": \"cancelled\""],
    );

    let reference = read(REFERENCE);
    check(
        &mut missing,
        "koto-user command-reference.md (## koto init)",
        section(&reference, "## koto init"),
        &[
            "--replaces <old>",
            "--abandon-tick",
            "koto/replaces",
            "tick_in_flight",
            "session_busy",
            "session_replaced",
            "koto session replace",
        ],
    );
    check(
        &mut missing,
        "koto-user command-reference.md (## koto session replace)",
        section(&reference, "## koto session replace"),
        &[
            "--by <new>",
            "waited_for_ms",
            "invalid_usage",
            "invalid_fence",
            "session_rebound",
            "newest",
        ],
    );
    check(
        &mut missing,
        "koto-user command-reference.md (## koto status)",
        section(&reference, "## koto status"),
        &["replaced_by"],
    );

    let codes = read(CODES);
    check(
        &mut missing,
        "docs/reference/error-codes.md (### init)",
        section(&codes, "### init"),
        &[
            "--replaces",
            "session_busy",
            "waited_for_ms",
            "session_replaced",
            "session_exists",
            "session_rebound",
            "invalid_fence",
            "fence_unavailable",
            "koto cancel <new> --cleanup",
            "koto session replace",
            "--replace-terminal",
            "replaced_by",
            "newest",
        ],
    );
    check(
        &mut missing,
        "docs/reference/error-codes.md (### session replace)",
        section(&codes, "### session replace"),
        &[
            "\"command\":\"session replace\"",
            "waited_for_ms",
            "session_not_found",
            "invalid_usage",
            "invalid_fence",
            "session_rebound",
            "session_busy",
            "session_replaced",
            "superseded_by",
            "newest",
            "tick_in_flight",
        ],
    );

    missing.extend(missing_from(
        "koto-user error-handling.md",
        &read("plugins/koto-skills/skills/koto-user/references/error-handling.md"),
        &["session_replaced", "koto session replace"],
    ));
    missing.extend(missing_from(
        "koto-user SKILL.md",
        &read("plugins/koto-skills/skills/koto-user/SKILL.md"),
        &["session_replaced"],
    ));

    for rel in [GUIDE, REFERENCE, CODES] {
        let text = normalize(&read(rel));
        for phrase in CLOUD_REFUSED {
            if text.contains(&normalize(phrase)) {
                missing.push(format!(
                    "{rel}: still says the cloud backend refuses the restart verbs (\"{phrase}\")"
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "these docs don't yet describe `koto init --replaces` and `koto session replace`, \
         what they write, how a replaced run reads, or what they refuse with:\n  {}",
        missing.join("\n  ")
    );
}

/// The frontmatter of session-feed.md, parsed.
fn feed_spec(text: &str) -> Value {
    let rest = text
        .strip_prefix("---\n")
        .expect("session-feed.md starts with frontmatter");
    let end = rest.find("\n---\n").expect("the frontmatter is closed");
    serde_yaml_ng::from_str(&rest[..end]).expect("the frontmatter parses")
}

/// Each way the declaration of `event.field` differs from `ty` and
/// `required`.
fn field_problems(spec: &Value, event: &str, field: &str, ty: &str, required: bool) -> Vec<String> {
    let decl = &spec["events"][event]["fields"][field];
    if decl.is_null() {
        return vec![format!("{event}.{field} isn't declared in the frontmatter")];
    }
    let mut problems = Vec::new();
    if decl["type"].as_str() != Some(ty) {
        problems.push(format!(
            "{event}.{field} is declared {:?}, not {ty}",
            decl["type"]
        ));
    }
    if decl["required"].as_bool().unwrap_or(false) != required {
        problems.push(format!(
            "{event}.{field} should be declared required: {required}"
        ));
    }
    problems
}

#[test]
fn session_feed_declares_session_superseded_and_the_replace_fields() {
    let text = read("docs/reference/session-feed.md");
    let spec = feed_spec(&text);
    let mut problems = Vec::new();

    for (field, ty, required) in [
        ("by", "string", true),
        ("by_session_id", "string", true),
        ("actor", "object", true),
        ("at", "string", true),
        ("replaced", "boolean", false),
    ] {
        problems.extend(field_problems(
            &spec,
            "session_superseded",
            field,
            ty,
            required,
        ));
    }
    for field in ["replaced_by", "replaced_by_session_id"] {
        problems.extend(field_problems(
            &spec,
            "workflow_cancelled",
            field,
            "string",
            false,
        ));
    }

    match section(&text, "#### `session_superseded`") {
        Some(sec) => problems.extend(missing_from(
            "session-feed.md (#### `session_superseded`)",
            &sec,
            &[
                "by_session_id",
                "actor",
                "replaced",
                "koto init",
                "--supersede",
                "--replaces",
                "koto session replace",
            ],
        )),
        None => problems.push("session-feed.md: a `#### `session_superseded`` entry".to_string()),
    }
    match section(&text, "#### `workflow_cancelled`") {
        Some(sec) => problems.extend(missing_from(
            "session-feed.md (#### `workflow_cancelled`)",
            &sec,
            &["replaced_by", "replaced_by_session_id"],
        )),
        None => problems.push("session-feed.md: a `#### `workflow_cancelled`` entry".to_string()),
    }

    assert!(
        problems.is_empty(),
        "session-feed.md doesn't yet declare the events a replace writes:\n  {}",
        problems.join("\n  ")
    );
}
