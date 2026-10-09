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
//! - `path_under_tempdir`: the directory, with symlinks resolved (or as
//!   recorded when it no longer resolves), lies under the process's
//!   temporary directory, or its path matches `pattern` (`/tmp`,
//!   `/var/folders` and their `/private` forms).
//! - `path_segment_regex`: some segment of the recorded path matches
//!   `pattern`. With `ancestor_pattern`, an earlier segment must also match
//!   that.

use std::path::{Component, Path, PathBuf};
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
}

#[derive(Debug)]
enum Rule {
    UnderTempdir {
        pattern: Regex,
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
                "path_under_tempdir" => Ok(Rule::UnderTempdir {
                    pattern: compile(&row.pattern)?,
                }),
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
    let Some(dir) = template_source_dir else {
        return false;
    };
    rules().iter().any(|rule| matches(rule, dir))
}

fn matches(rule: &Rule, dir: &Path) -> bool {
    match rule {
        Rule::UnderTempdir { pattern } => {
            let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            if pattern.is_match(&resolved.to_string_lossy()) {
                return true;
            }
            temp_roots().iter().any(|root| resolved.starts_with(root))
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

/// The process's temporary directory, as given and with symlinks resolved.
fn temp_roots() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    let mut roots = vec![tmp.clone()];
    if let Ok(resolved) = std::fs::canonicalize(&tmp) {
        if resolved != tmp {
            roots.push(resolved);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(p: &str) -> bool {
        is_fixture(Some(Path::new(p)))
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
    fn temporary_directories_are_fixtures() {
        assert!(fixture("/tmp/x/templates"));
        assert!(fixture("/tmp"));
        assert!(fixture("/private/tmp/x"));
        assert!(fixture("/var/folders/ab/cd/T/x"));
        assert!(fixture("/private/var/folders/ab/cd/T/x"));
        let under_temp = std::env::temp_dir().join("no-such-dir-for-run-journal");
        assert!(is_fixture(Some(&under_temp)));
    }

    #[test]
    fn near_misses_of_the_temporary_directory_rule_are_not_fixtures() {
        assert!(!fixture("/tmpfoo/x"));
        assert!(!fixture("/var/folder/x"));
        assert!(!fixture("/home/me/tmp/x"));
        assert!(!fixture("/private/tmpx/y"));
        assert!(!fixture("/opt/var/folders/x"));
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
