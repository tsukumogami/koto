//! What the guides and references owe a reader about
//! `koto session rebind --whole`: what one call moves (the execution
//! anchor, the held-by links, the `/workflows` publish location and the
//! recorded command environment), the events it logs, and the codes it
//! refuses with. Since the move re-records the command environment, no doc
//! may still say that record can't be changed.
//!
//! Text is compared after joining Rust string continuations
//! (backslash-newline) and collapsing runs of whitespace, case-insensitive,
//! so a phrase wrapped across lines still matches.

use std::path::{Path, PathBuf};

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

/// The `#### session rebind` section of cli-usage.md, which must sit under
/// its `### session` section.
fn cli_usage_rebind(text: &str) -> Option<String> {
    let session = section(text, "### session")?;
    section(&session, "#### session rebind")
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

#[test]
fn the_guide_and_the_command_reference_describe_what_rebind_whole_moves() {
    let mut missing = Vec::new();

    let cli = read("docs/guides/cli-usage.md");
    match cli_usage_rebind(&cli) {
        Some(sec) => missing.extend(missing_from(
            "docs/guides/cli-usage.md (#### session rebind)",
            &sec,
            &[
                "--whole",
                "--wait",
                "held-by",
                "publish location",
                "command environment",
                "environment_rerecorded",
                "session_busy",
                "no_anchor",
                "rebind_cross_host",
            ],
        )),
        None => missing.push(
            "docs/guides/cli-usage.md: a `#### session rebind` section under `### session`"
                .to_string(),
        ),
    }

    let reference = read("plugins/koto-skills/skills/koto-user/references/command-reference.md");
    match section(&reference, "## koto session rebind") {
        Some(sec) => missing.extend(missing_from(
            "koto-user command-reference.md (## koto session rebind)",
            &sec,
            &[
                "--whole [--wait <secs>]",
                "held_by",
                "publish_location",
                "waited",
                "rerecorded",
                "session_busy",
                "no_anchor",
                "rebind_cross_host",
                "nested_invocation",
            ],
        )),
        None => missing
            .push("koto-user command-reference.md: a `## koto session rebind` section".to_string()),
    }

    let codes = read("docs/reference/error-codes.md");
    match section(&codes, "### session rebind") {
        Some(sec) => missing.extend(missing_from(
            "docs/reference/error-codes.md (### session rebind)",
            &sec,
            &[
                "session_busy",
                "no_anchor",
                "rebind_cross_host",
                "nested_invocation",
                "session_rebound",
            ],
        )),
        None => missing
            .push("docs/reference/error-codes.md: a `### session rebind` section".to_string()),
    }

    missing.extend(missing_from(
        "docs/reference/session-feed.md",
        &read("docs/reference/session-feed.md"),
        &[
            "held_by_rebound",
            "publish_location_dropped",
            "environment_rerecorded",
            "whole_epoch",
        ],
    ));
    missing.extend(missing_from(
        "koto-user error-handling.md",
        &read("plugins/koto-skills/skills/koto-user/references/error-handling.md"),
        &["session_busy", "no_anchor", "rebind_cross_host"],
    ));
    missing.extend(missing_from(
        "docs/guides/default-action-authoring.md",
        &read("docs/guides/default-action-authoring.md"),
        &["koto session rebind --whole", "environment_rerecorded"],
    ));

    assert!(
        missing.is_empty(),
        "these docs don't yet describe what `koto session rebind --whole` moves, logs or \
         refuses with:\n  {}",
        missing.join("\n  ")
    );
}

/// The files that describe the recorded command environment to a reader,
/// and the notices koto prints about it.
const ENVIRONMENT_DOCS: &[&str] = &[
    "docs/guides/cli-usage.md",
    "plugins/koto-skills/skills/koto-user/references/command-reference.md",
    "docs/guides/default-action-authoring.md",
    "plugins/koto-skills/skills/koto-user/SKILL.md",
    "plugins/koto-skills/skills/koto-user/references/error-handling.md",
    "plugins/koto-skills/skills/koto-adhoc/SKILL.md",
    "plugins/koto-skills/skills/koto-author/references/template-format.md",
    "src/cli/next_types.rs",
    "docs/guides/cloud-sync-setup.md",
    "docs/reference/session-feed.md",
    "docs/reference/error-codes.md",
];

/// Ways of saying the record is fixed for good, which stopped being true
/// once `--whole` re-records it.
const FIXED_FOR_GOOD: &[&str] = &[
    "record can't be changed",
    "record cannot be changed",
    "record can't change",
    "record cannot change",
    "can't change after creation",
    "no command rewrites it",
    "never rewritten afterwards",
];

#[test]
fn no_doc_says_the_command_environment_record_cant_be_changed() {
    let mut found = Vec::new();
    for rel in ENVIRONMENT_DOCS {
        let path: &Path = Path::new(rel);
        let text = normalize(&read(rel));
        for phrase in FIXED_FOR_GOOD {
            if text.contains(phrase) {
                found.push(format!("{}: \"{phrase}\"", path.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these files still say the recorded command environment can't be changed, which \
         `koto session rebind --whole` now does (it re-records it and logs \
         `environment_rerecorded`):\n  {}",
        found.join("\n  ")
    );
}
