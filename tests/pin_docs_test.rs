//! What the guides owe a reader about pins (PRD-session-pin.md, R26).
//!
//! Every guide that tells a reader to run `koto session cleanup` also says to
//! unpin first, since a pinned session refuses it. The changelog's entry and
//! the session retention guide name the first release that honours pins in
//! full, say which commands older releases don't honour pins in, and list pin
//! and unpin among koto's cross-session writers (koto#445).

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

/// The guides a reader follows: the README, `docs/guides/`, and the
/// koto-user skill with its references.
fn guides() -> Vec<PathBuf> {
    let root = root();
    let skill = root.join("plugins/koto-skills/skills/koto-user");
    let mut files = vec![root.join("README.md"), skill.join("SKILL.md")];
    files.extend(markdown_in(&root.join("docs/guides")));
    files.extend(markdown_in(&skill.join("references")));
    files
}

#[test]
fn every_guide_that_runs_cleanup_says_to_unpin_first() {
    let mut missing = Vec::new();
    for file in guides() {
        let text = std::fs::read_to_string(&file).unwrap();
        if text.contains("koto session cleanup") && !text.contains("koto session unpin") {
            missing.push(file.strip_prefix(root()).unwrap().display().to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "these guides run `koto session cleanup` and never say to unpin a pinned session \
         first (`koto session unpin <name>`), which it refuses:\n  {}",
        missing.join("\n  ")
    );
}

/// The changelog's unreleased section, up to the first released version.
fn unreleased_changelog() -> String {
    let text = std::fs::read_to_string(root().join("CHANGELOG.md")).unwrap();
    let start = text
        .find("## [Unreleased]")
        .expect("an [Unreleased] section");
    let rest = &text[start..];
    let end = rest[1..].find("\n## [").map_or(rest.len(), |i| i + 1);
    rest[..end].to_string()
}

#[test]
fn the_changelog_and_the_retention_guide_say_which_releases_honour_pins() {
    let guide = std::fs::read_to_string(root().join("docs/guides/session-retention.md")).unwrap();
    for (what, text) in [
        ("CHANGELOG.md", unreleased_changelog()),
        ("session-retention.md", guide),
    ] {
        for needed in [
            "first release that honours pins in full",
            "v0.23.0",
            "`koto session cleanup`",
            "`koto cancel --cleanup`",
            "`koto session import`",
            "honour pins",
            "koto#445",
        ] {
            assert!(text.contains(needed), "{what} doesn't say {needed:?}");
        }
    }
}
