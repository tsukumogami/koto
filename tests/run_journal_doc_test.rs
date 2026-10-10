//! `docs/reference/run-journal.md` against the code that writes the journal.
//!
//! The reference is what external tooling reads the journal by, so every
//! record kind and field it names has to be one koto writes, and every one
//! koto writes has to be named there. The source of truth is the string
//! literals in `src/run_journal/`, the only module that builds records.
//!
//! The reference also has to stay a description of a local file: no link
//! outside the koto repository, no attribute name outside the `koto.`
//! namespace, and no cost figure.

use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const REFERENCE: &str = "docs/reference/run-journal.md";
const SOURCE_DIR: &str = "src/run_journal";

/// Fields every record carries that aren't in the `koto.` namespace.
const ENVELOPE: &[&str] = &["kind", "v", "at", "session"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn reference() -> String {
    std::fs::read_to_string(root().join(REFERENCE)).unwrap()
}

/// Every string literal in the run journal module's non-test source.
fn source_literals() -> BTreeSet<String> {
    let literal = Regex::new(r#""([^"\\]*)""#).unwrap();
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(root().join(SOURCE_DIR)).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Unit tests live in a trailing `#[cfg(test)]` module.
        let text = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        for cap in literal.captures_iter(text) {
            out.insert(cap[1].to_string());
        }
    }
    out
}

/// Backticked spans in `text`.
fn spans(text: &str) -> Vec<String> {
    Regex::new(r"`([^`\n]+)`")
        .unwrap()
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect()
}

/// The record kinds the reference lists: the first cell of each row of the
/// table under `### Record kinds`.
fn documented_kinds(text: &str) -> BTreeSet<String> {
    let section = text
        .split("### Record kinds")
        .nth(1)
        .expect("the reference has a Record kinds section");
    let row = Regex::new(r"^\| `([a-z_]+)` \|").unwrap();
    section
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| row.captures(l).map(|c| c[1].to_string()))
        .collect()
}

/// The `koto.` fields the reference names anywhere.
fn documented_fields(text: &str) -> BTreeSet<String> {
    let field = Regex::new(r"^koto\.[a-z_]+(\.[a-z_]+)*$").unwrap();
    spans(text)
        .into_iter()
        .filter(|s| field.is_match(s))
        .collect()
}

#[test]
fn every_kind_the_reference_names_is_one_koto_writes_and_the_reverse() {
    let text = reference();
    let literals = source_literals();
    let kinds = documented_kinds(&text);
    assert_eq!(
        kinds,
        [
            "cancelled",
            "driver_seen",
            "session_started",
            "state_entered",
            "terminal"
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<BTreeSet<_>>(),
        "the reference's kinds table"
    );
    for kind in &kinds {
        assert!(
            literals.contains(kind),
            "{REFERENCE} names the kind `{kind}`, which {SOURCE_DIR} never writes"
        );
    }
    // Every kind the module builds a record with is in the table.
    let built = Regex::new(r#"Record::new\(\s*"([a-z_]+)""#).unwrap();
    for entry in std::fs::read_dir(root().join(SOURCE_DIR)).unwrap() {
        let path = entry.unwrap().path();
        if path.ends_with("tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for cap in built.captures_iter(&text) {
            assert!(
                kinds.contains(&cap[1]),
                "{} writes `{}`, which {REFERENCE} doesn't list",
                path.display(),
                &cap[1]
            );
        }
    }
}

#[test]
fn every_field_the_reference_names_is_one_koto_writes_and_the_reverse() {
    let text = reference();
    let literals = source_literals();
    let fields = documented_fields(&text);
    assert!(fields.contains("koto.run.id"), "{fields:?}");
    for field in &fields {
        assert!(
            literals.contains(field),
            "{REFERENCE} names `{field}`, which {SOURCE_DIR} never writes"
        );
    }
    for literal in literals.iter().filter(|l| l.starts_with("koto.")) {
        assert!(
            fields.contains(literal),
            "{SOURCE_DIR} writes `{literal}`, which {REFERENCE} doesn't name"
        );
    }
    for field in ENVELOPE {
        assert!(
            literals.contains(*field),
            "the envelope field `{field}` isn't in {SOURCE_DIR}"
        );
        assert!(
            text.contains(&format!("| `{field}` |")),
            "{REFERENCE} doesn't describe the envelope field `{field}`"
        );
    }
}

/// The reference's JSON examples parse, and every dotted key in them is in
/// the `koto.` namespace and documented.
#[test]
fn the_examples_are_records_the_reference_describes() {
    let text = reference();
    let fields = documented_fields(&text);
    let kinds = documented_kinds(&text);
    let mut seen = 0;
    for block in text.split("```json\n").skip(1) {
        let body = block.split("```").next().unwrap();
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let record: Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("example line is not JSON ({e}): {line}"));
            let obj = record.as_object().unwrap();
            assert!(kinds.contains(obj["kind"].as_str().unwrap()), "{line}");
            assert_eq!(obj["v"], 1, "{line}");
            for key in obj.keys() {
                assert!(
                    ENVELOPE.contains(&key.as_str()) || fields.contains(key),
                    "example key `{key}` isn't a documented field: {line}"
                );
            }
            seen += 1;
        }
    }
    assert!(seen > 0, "the reference has no example records");
}

#[test]
fn the_reference_links_nowhere_outside_the_koto_repository() {
    let text = reference();
    let url = Regex::new(r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s)>\]`]+").unwrap();
    for m in url.find_iter(&text) {
        assert!(
            m.as_str()
                .starts_with("https://github.com/tsukumogami/koto"),
            "{REFERENCE} links outside the koto repository: {}",
            m.as_str()
        );
    }
    // Relative links resolve inside the repository.
    let link = Regex::new(r"\]\(([^)#]+)(#[^)]*)?\)").unwrap();
    let dir = root().join(REFERENCE);
    let dir = dir.parent().unwrap();
    for cap in link.captures_iter(&text) {
        let target = &cap[1];
        if target.contains("://") {
            continue;
        }
        let resolved = dir.join(target);
        assert!(resolved.exists(), "broken link {target} in {REFERENCE}");
        let canonical = std::fs::canonicalize(&resolved).unwrap();
        assert!(
            canonical.starts_with(std::fs::canonicalize(root()).unwrap()),
            "{target} leaves the repository"
        );
    }
}

#[test]
fn every_attribute_name_in_the_reference_is_in_the_koto_namespace() {
    let text = reference();
    // A dotted lower-case name, such as `service.name`; file names (a final
    // segment that is a file extension) aren't attribute names.
    let dotted = Regex::new(r"^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$").unwrap();
    let file = Regex::new(r"\.(json|jsonl|md|rs|toml|lock)$").unwrap();
    for span in spans(&text) {
        if dotted.is_match(&span) && !file.is_match(&span) {
            assert!(
                span.starts_with("koto."),
                "{REFERENCE} names `{span}`, an attribute outside the koto. namespace"
            );
        }
    }
}

#[test]
fn the_reference_carries_no_cost_figure() {
    let text = reference();
    let cost = Regex::new(r"(?i)\bcosts?\b|\$\s?\d|\b(usd|eur|cents?)\b").unwrap();
    let found: Vec<&str> = text.lines().filter(|l| cost.is_match(l)).collect();
    assert!(found.is_empty(), "{REFERENCE} mentions cost: {found:?}");
}

/// The checks above fire on what they are meant to catch.
#[test]
fn the_checks_catch_what_they_look_for() {
    let bad = "see `service.name` and https://example.com for $5 of cost";
    let dotted = Regex::new(r"^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$").unwrap();
    assert!(spans(bad).iter().any(|s| dotted.is_match(s)));
    assert!(Regex::new(r"[a-zA-Z][a-zA-Z0-9+.-]*://")
        .unwrap()
        .is_match(bad));
    assert!(Regex::new(r"(?i)\bcosts?\b|\$\s?\d").unwrap().is_match(bad));
    let table = "### Record kinds\n\n| Kind | x |\n|---|---|\n| `ghost_kind` | y |\n";
    assert!(documented_kinds(table).contains("ghost_kind"));
    assert!(!source_literals().contains("ghost_kind"));
    assert!(Path::new(&root().join(SOURCE_DIR)).is_dir());
}

/// The session-feed contract declares the two lineage fields a child's
/// header carries, with the type koto writes, and its header table
/// describes them.
#[test]
fn the_session_feed_contract_declares_the_lineage_header_fields() {
    let spec = std::fs::read_to_string(root().join("docs/reference/session-feed.md")).unwrap();
    let front = spec
        .strip_prefix("---\n")
        .and_then(|rest| rest.split("\n---\n").next())
        .expect("session-feed.md has frontmatter");
    let front: serde_yaml_ng::Value = serde_yaml_ng::from_str(front).unwrap();
    let declared = &front["header"]["fields"];

    // A root and a child, through the koto binary in a scratch home.
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let template = tmp.path().join("t.md");
    std::fs::write(
        &template,
        "---\nname: lineage\nversion: \"1.0\"\ninitial_state: a\nstates:\n  a:\n    terminal: true\n---\n\n## a\n\nA.\n",
    )
    .unwrap();
    for args in [
        vec!["init", "root", "--template", template.to_str().unwrap()],
        vec![
            "init",
            "root.kid",
            "--template",
            template.to_str().unwrap(),
            "--parent",
            "root",
        ],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_koto"))
            .args(&args)
            .current_dir(tmp.path())
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", tmp.path().join("cache"))
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let header = |name: &str| -> Value {
        let path = home
            .join(".koto/sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"));
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    };
    let root_header = header("root");
    let kid = header("root.kid");
    assert_eq!(kid["root_session_id"], root_header["session_id"]);
    assert_eq!(kid["parent_session_id"], root_header["session_id"]);
    for field in ["root_session_id", "parent_session_id"] {
        assert!(kid[field].is_string(), "{field}: {kid}");
        assert!(root_header.get(field).is_none(), "a root has no {field}");
        assert_eq!(
            declared[field]["type"].as_str(),
            Some("string"),
            "session-feed.md's frontmatter doesn't declare {field}"
        );
        assert!(
            spec.contains(&format!("| `{field}` | string | No |")),
            "session-feed.md's header table doesn't describe {field}"
        );
    }
}
