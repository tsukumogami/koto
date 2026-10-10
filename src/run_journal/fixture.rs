//! Whether a session is a test fixture, from the directory its template
//! came from.
//!
//! The rules are data, not code: `fixture_rules.json` holds one row per
//! rule, and [`is_fixture`] evaluates the rows in order. A session is a
//! fixture when any row matches. A session with no recorded template source
//! directory (`koto init --from-stdin`, `koto session start`) is never one.
//!
//! Two row kinds exist:
//!
//! - `path_under_tempdir`: the directory as the session header records it,
//!   normalized lexically the way POSIX `normpath` does (`.`, `..` and
//!   repeated slashes collapsed, no symlink resolved), lies under `TMPDIR`
//!   (as given, normalized the same way, and only when absolute), or matches
//!   `pattern` (`/tmp`, `/var/folders`) either directly or below the row's
//!   `alias_root`. A relative directory is under no temporary directory.
//!
//!   macOS keeps the real `/tmp` and `/var/folders` under one top-level
//!   directory and makes the familiar names symlinks into it, so a path
//!   recorded after resolution starts with that directory. `alias_root`
//!   names it as a single path segment, and the pattern is tried again on
//!   the path with that one leading segment removed. The roots are matched by how they
//!   are spelled, not by resolving `/tmp` at runtime, so Linux and macOS
//!   classify the same set of paths: resolving would drop the alias forms
//!   on Linux, where `/tmp` is no symlink.
//! - `path_segment_regex`: some segment of the recorded path matches
//!   `pattern`. With `ancestor_pattern`, an earlier segment must also match
//!   that.

use std::path::{Component, Path};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

const RULES_JSON: &str = include_str!("fixture_rules.json");

/// The one header field the rules read.
const FIELD_TEMPLATE_SOURCE_DIR: &str = "template_source_dir";

#[derive(Debug, Deserialize)]
struct RuleRow {
    rule: String,
    kind: String,
    field: String,
    pattern: String,
    #[serde(default)]
    ancestor_pattern: Option<String>,
    #[serde(default)]
    alias_root: Option<String>,
}

#[derive(Debug)]
enum Rule {
    UnderTempdir {
        pattern: Regex,
        /// `/` plus the row's `alias_root` segment: the prefix the pattern
        /// is also tried below.
        alias_prefix: Option<String>,
    },
    Segment {
        pattern: Regex,
        ancestor: Option<Regex>,
    },
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| parse_rules(RULES_JSON).expect("embedded fixture rules are valid"))
}

fn parse_rules(json: &str) -> Result<Vec<Rule>, String> {
    let rows: Vec<RuleRow> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    rows.into_iter()
        .map(|row| {
            if row.field != FIELD_TEMPLATE_SOURCE_DIR {
                return Err(format!("rule {}: unknown field {}", row.rule, row.field));
            }
            let compile = |p: &str| Regex::new(p).map_err(|e| format!("rule {}: {}", row.rule, e));
            match row.kind.as_str() {
                "path_under_tempdir" => {
                    let alias_prefix = match row.alias_root.as_deref() {
                        None => None,
                        Some(seg) if !seg.is_empty() && !seg.contains('/') => {
                            Some(format!("/{seg}"))
                        }
                        Some(seg) => {
                            return Err(format!(
                                "rule {}: alias_root {seg:?} is not one path segment",
                                row.rule
                            ))
                        }
                    };
                    Ok(Rule::UnderTempdir {
                        pattern: compile(&row.pattern)?,
                        alias_prefix,
                    })
                }
                "path_segment_regex" if row.alias_root.is_some() => Err(format!(
                    "rule {}: alias_root applies only to path_under_tempdir",
                    row.rule
                )),
                "path_segment_regex" => Ok(Rule::Segment {
                    pattern: compile(&row.pattern)?,
                    ancestor: row.ancestor_pattern.as_deref().map(compile).transpose()?,
                }),
                other => Err(format!("rule {}: unknown kind {}", row.rule, other)),
            }
        })
        .collect()
}

/// Whether a session whose template came from `template_source_dir` is a
/// test fixture.
pub(crate) fn is_fixture(template_source_dir: Option<&Path>) -> bool {
    let tmpdir = std::env::var_os("TMPDIR");
    is_fixture_with_tmpdir(
        template_source_dir,
        tmpdir.as_deref().map(|t| t.to_string_lossy()).as_deref(),
    )
}

/// [`is_fixture`] with the `TMPDIR` value given, so tests can name one
/// without changing the environment.
fn is_fixture_with_tmpdir(template_source_dir: Option<&Path>, tmpdir: Option<&str>) -> bool {
    let Some(dir) = template_source_dir else {
        return false;
    };
    rules().iter().any(|rule| matches(rule, dir, tmpdir))
}

fn matches(rule: &Rule, dir: &Path, tmpdir: Option<&str>) -> bool {
    match rule {
        Rule::UnderTempdir {
            pattern,
            alias_prefix,
        } => {
            // The path as the session header records it, normalized only
            // lexically: resolving symlinks would classify paths the header's
            // own value doesn't name. A relative path is under no temporary
            // directory.
            let Some(text) = normpath(&dir.to_string_lossy()) else {
                return false;
            };
            if pattern.is_match(&text) {
                return true;
            }
            if let Some(rest) = alias_prefix.as_deref().and_then(|p| text.strip_prefix(p)) {
                if pattern.is_match(rest) {
                    return true;
                }
            }
            // TMPDIR counts as given, and only when it is absolute. A TMPDIR
            // of `/` names no temporary directory: it would make every
            // absolute path a fixture. Any other value counts as given, so a
            // TMPDIR of the home directory makes everything under home one.
            tmpdir.and_then(normpath).is_some_and(|root| {
                let trimmed = root.trim_end_matches('/');
                !trimmed.is_empty() && (text == root || text.starts_with(&format!("{}/", trimmed)))
            })
        }
        Rule::Segment { pattern, ancestor } => {
            let segments: Vec<String> = dir
                .components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            segments.iter().enumerate().any(|(i, seg)| {
                pattern.is_match(seg)
                    && match ancestor {
                        None => true,
                        Some(a) => segments[..i].iter().any(|s| a.is_match(s)),
                    }
            })
        }
    }
}

/// POSIX `normpath` on an absolute path: collapse `.`, `..` and repeated
/// slashes without touching the filesystem. As POSIX allows, exactly two
/// leading slashes are kept as two; one, or three or more, become one.
/// `None` for a relative path.
fn normpath(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let lead = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(format!("{lead}{}", parts.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(p: &str) -> bool {
        is_fixture(Some(Path::new(p)))
    }

    /// The top-level directory macOS keeps the real `/tmp` and
    /// `/var/folders` under, as one segment.
    const MACOS_ALIAS_ROOT: &str = "private";

    /// `path` below the macOS alias root.
    fn alias(path: &str) -> String {
        format!("/{MACOS_ALIAS_ROOT}{path}")
    }

    /// Whether `p` is a fixture with `TMPDIR` set to `tmpdir`.
    fn fixture_with_temp_dir(p: &str, tmpdir: &str) -> bool {
        is_fixture_with_tmpdir(Some(Path::new(p)), Some(tmpdir))
    }

    /// Whether `p` is a fixture with `TMPDIR` unset.
    fn fixture_without_tmpdir(p: &str) -> bool {
        is_fixture_with_tmpdir(Some(Path::new(p)), None)
    }

    #[test]
    fn the_embedded_table_has_four_rows_that_compile() {
        assert_eq!(parse_rules(RULES_JSON).unwrap().len(), 4);
    }

    #[test]
    fn no_template_source_dir_is_not_a_fixture() {
        assert!(!is_fixture(None));
    }

    #[test]
    fn the_embedded_temporary_directory_row_names_the_macos_alias_root() {
        let rows: Vec<RuleRow> = serde_json::from_str(RULES_JSON).unwrap();
        let row = rows
            .iter()
            .find(|r| r.kind == "path_under_tempdir")
            .unwrap();
        assert_eq!(row.alias_root.as_deref(), Some(MACOS_ALIAS_ROOT));
    }

    #[test]
    fn an_alias_root_must_be_one_segment_on_a_temporary_directory_row() {
        let row = |kind: &str, alias: &str| {
            format!(
                r#"[{{"rule":"r","kind":"{kind}","field":"template_source_dir","pattern":"x","alias_root":"{alias}"}}]"#
            )
        };
        assert!(parse_rules(&row("path_under_tempdir", "a")).is_ok());
        assert!(parse_rules(&row("path_under_tempdir", "a/b")).is_err());
        assert!(parse_rules(&row("path_under_tempdir", "")).is_err());
        assert!(parse_rules(&row("path_segment_regex", "a")).is_err());
    }

    #[test]
    fn the_tmp_root_is_a_fixture() {
        assert!(fixture("/tmp"));
        assert!(fixture("/tmp/x/templates"));
    }

    #[test]
    fn the_var_folders_root_is_a_fixture() {
        assert!(fixture("/var/folders"));
        assert!(fixture("/var/folders/ab/cd/T/x"));
    }

    #[test]
    fn the_macos_alias_of_tmp_is_a_fixture() {
        assert!(fixture(&alias("/tmp")));
        assert!(fixture(&alias("/tmp/x")));
    }

    #[test]
    fn the_macos_alias_of_var_folders_is_a_fixture() {
        assert!(fixture(&alias("/var/folders")));
        assert!(fixture(&alias("/var/folders/ab/cd/T/x")));
    }

    #[test]
    fn the_process_temporary_directory_is_a_fixture() {
        // A temporary directory outside /tmp and /var/folders, named
        // explicitly: paths under it count, paths beside it don't.
        let tmpdir = "/home/me/scratch-temp";
        assert!(fixture_with_temp_dir("/home/me/scratch-temp", tmpdir));
        assert!(fixture_with_temp_dir("/home/me/scratch-temp/a/b", tmpdir));
        assert!(!fixture_with_temp_dir("/home/me/scratch-tempx/a", tmpdir));
        assert!(!fixture_with_temp_dir("/home/me/other/a", tmpdir));
        assert!(!fixture_with_temp_dir(
            "/home/me/scratch-temp/a",
            "/elsewhere"
        ));
        // A TMPDIR of the root names no temporary directory.
        for root in ["/", "//", "///"] {
            assert!(!fixture_with_temp_dir("/home/me/project", root), "{root}");
        }
        assert!(
            fixture_with_temp_dir("/tmp/x", "/"),
            "the fixed roots still count"
        );

        // A relative TMPDIR names nothing.
        assert!(!fixture_with_temp_dir(
            "/home/me/scratch-temp/a",
            "scratch-temp"
        ));
        // TMPDIR is matched as given, not resolved, and normalized lexically.
        assert!(fixture_with_temp_dir(
            "/home/me/scratch-temp/a",
            "/home/me/./scratch-temp/"
        ));
    }

    #[test]
    fn paths_are_normalized_lexically_before_matching() {
        assert!(fixture_without_tmpdir("/home/me/../../tmp/x"));
        assert!(fixture_without_tmpdir("/tmp/./a//b"));
        assert!(fixture_without_tmpdir("///tmp/x"), "three slashes are one");
        assert!(!fixture_without_tmpdir("/tmp/../home/x"));
        assert!(!fixture_without_tmpdir("//tmp/x"), "two slashes stay two");
        assert!(!fixture_without_tmpdir("/../tmpx"));
        assert!(fixture_without_tmpdir("/../tmp"));
        assert!(fixture_without_tmpdir(&format!(
            "{}/../../tmp/x",
            alias("/var")
        )));
    }

    #[test]
    fn a_relative_path_is_under_no_temporary_directory() {
        assert!(!fixture_without_tmpdir("tmp/x"));
        assert!(!fixture_without_tmpdir("./tmp/x"));
        assert!(!fixture_with_temp_dir("scratch/a", "scratch"));
        // The segment rules still read a relative path's segments.
        assert!(fixture_without_tmpdir("work/tmp.Ab12Cd/x"));
    }

    #[test]
    fn normpath_matches_posix() {
        for (input, want) in [
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("/a/b/..", "/a"),
            ("/a/./b/", "/a/b"),
            ("/..", "/"),
            ("/../a", "/a"),
            ("//a//b", "//a/b"),
            ("/a/../../b", "/b"),
        ] {
            assert_eq!(normpath(input).as_deref(), Some(want), "{input}");
        }
        assert_eq!(normpath("a/b"), None);
        assert_eq!(normpath(""), None);
    }

    #[test]
    fn near_misses_of_the_temporary_directory_rule_are_not_fixtures() {
        assert!(!fixture("/tmpx/y"));
        assert!(!fixture("/tmpfoo/x"));
        assert!(!fixture("/var/foldersx/y"));
        assert!(!fixture("/var/folder/x"));
        assert!(!fixture("/home/me/tmp/x"), "contains a tmp segment");
        assert!(!fixture("/home/me/atmpdir/x"), "contains tmp");
        assert!(!fixture("/opt/var/folders/x"));
        assert!(!fixture(&alias("/tmpx/y")));
        assert!(!fixture(&alias("/var/foldersx/y")));
        assert!(!fixture(&alias("")), "the alias root alone");
        assert!(
            !fixture(&format!("{}/tmp/y", alias("x"))),
            "a longer segment"
        );
        assert!(
            !fixture(&format!("/opt{}", alias("/tmp/y"))),
            "not at the top"
        );
    }

    #[test]
    fn mktemp_segments_are_fixtures_and_near_misses_are_not() {
        assert!(fixture("/home/me/tmp.AbC123/templates"));
        assert!(fixture("/home/me/tmp.abcdefghij"));
        assert!(!fixture("/home/me/tmp.AbC12/templates"), "five characters");
        assert!(!fixture("/home/me/tmp.AbC-123/templates"), "a dash");
        assert!(!fixture("/home/me/xtmp.AbC123/templates"), "a prefix");
        assert!(!fixture("/home/me/tmp_AbC123/templates"), "no dot");
    }

    #[test]
    fn ablation_segments_are_fixtures_and_near_misses_are_not() {
        assert!(fixture("/home/me/shirabe-ablation.42/skills"));
        assert!(fixture("/home/me/shirabe-ablation.x"));
        assert!(!fixture("/home/me/shirabe-ablation/skills"), "no dot");
        assert!(!fixture("/home/me/my-shirabe-ablation.42"), "a prefix");
    }

    #[test]
    fn tool_test_directories_are_fixtures_and_near_misses_are_not() {
        assert!(fixture("/home/me/src/koto/tests/fixtures"));
        assert!(fixture("/home/me/niwa/internal/test/x"));
        assert!(fixture("/home/me/shirabe/skills/tests"));
        assert!(
            !fixture("/home/me/tests/koto/x"),
            "the tool segment comes after"
        );
        assert!(!fixture("/home/me/myproject/tests/x"), "no tool segment");
        assert!(!fixture("/home/me/koto-fork/tests/x"), "not exactly koto");
        assert!(!fixture("/home/me/koto/testing/x"), "not exactly test");
        assert!(
            !fixture("/home/me/src/koto/plugins/skills"),
            "no test segment"
        );
    }
}
