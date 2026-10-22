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
            "check_disputed",
            "check_evaluated",
            "check_finding",
            "check_overridden",
            "check_pending",
            "check_rule",
            "context_reads",
            "decider_answered",
            "decider_check_answered",
            "decider_checked",
            "decider_consulted",
            "decider_fallback",
            "decider_field",
            "driver_seen",
            "response",
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
    // Every kind the module builds a record with is in the table: identity
    // records through `Record::new`, relayed ones through `Relay::record`.
    let built = Regex::new(r#"(?:Record::new|\.record)\(\s*"([a-z_]+)""#).unwrap();
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

/// Each kind's own fields, from the second column of the kinds table.
fn documented_kind_fields(text: &str) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    let section = text.split("### Record kinds").nth(1).unwrap();
    let row = Regex::new(r"^\| `([a-z_]+)` \| ([^|]*) \|").unwrap();
    section
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| row.captures(l))
        .map(|c| {
            let fields = spans(&c[2]).into_iter().collect();
            (c[1].to_string(), fields)
        })
        .collect()
}

/// The fields of the kinds that carry sequence numbers (check, decider,
/// override, response and context reads) are listed in the kinds table,
/// each has a row in its kind's field table, and every such field in the
/// module is one the table lists.
#[test]
fn every_check_kind_lists_its_fields_and_describes_each() {
    let text = reference();
    let by_kind = documented_kind_fields(&text);
    for kind in [
        "check_evaluated",
        "check_rule",
        "check_finding",
        "check_pending",
        "check_disputed",
        "context_reads",
        "decider_consulted",
        "decider_checked",
        "decider_fallback",
        "check_overridden",
        "response",
    ] {
        let fields = &by_kind[kind];
        for common in ["koto.seq", "koto.visit.seq"] {
            assert!(fields.contains(common), "{kind} doesn't list {common}");
        }
        // `response` has a section of its own rather than a subsection.
        let heading = match kind {
            "response" => "## Response records".to_string(),
            _ => format!("### `{kind}`"),
        };
        let section = text
            .split(&heading)
            .nth(1)
            .unwrap_or_else(|| panic!("{REFERENCE} has no `{kind}` section"));
        let section = section.split("\n### ").next().unwrap();
        for field in fields {
            if field == "koto.seq" || field == "koto.visit.seq" {
                assert!(
                    text.contains(&format!("| `{field}` | integer |")),
                    "{field}"
                );
                continue;
            }
            assert!(
                section.contains(&format!("| `{field}` |")),
                "the `{kind}` section doesn't describe {field}"
            );
        }
    }
    let listed: BTreeSet<&String> = by_kind.values().flatten().collect();
    for literal in source_literals()
        .iter()
        .filter(|l| l.starts_with("koto.") && !l.ends_with(".id"))
    {
        assert!(
            listed.contains(literal),
            "{SOURCE_DIR} writes `{literal}`, which no kind in the kinds table lists"
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

/// The patterns the checks above use match the kind of text they are meant
/// to catch, and a kind koto doesn't write is not among the source's
/// literals. (The checks themselves run against the real reference.)
#[test]
fn the_check_patterns_match_what_they_look_for() {
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

/// The attributes the decider fallback added (DESIGN-decider-fallback.md,
/// "The exact names") collide with nothing the journal writes: none equals
/// another attribute, none starts with another followed by `.`, and none is
/// such a prefix of another, so a backend that nests dotted names never
/// sees one as both a value and an object.
#[test]
fn the_check_lifecycle_attributes_collide_with_no_other_attribute() {
    let field = Regex::new(r"^koto\.[a-z_]+(\.[a-z_]+)*$").unwrap();
    let written: BTreeSet<String> = source_literals()
        .into_iter()
        .filter(|s| field.is_match(s))
        .collect();
    for new in [
        "koto.finding.level",
        "koto.finding.count",
        "koto.dispute.source",
    ] {
        assert!(written.contains(new), "{SOURCE_DIR} never writes {new}");
        for other in written.iter().filter(|o| o.as_str() != new) {
            assert!(
                !other.starts_with(&format!("{new}.")),
                "{other} nests under {new}"
            );
            assert!(
                !new.starts_with(&format!("{other}.")),
                "{new} nests under {other}"
            );
        }
    }
    // The two `check_evaluated` additions sit under `koto.check` beside
    // that kind's other `koto.check.*` fields, so they add no new shape.
    // `koto.check.name` is the path an exporter that nests dotted names
    // gives `check_evaluated`'s scalar `koto.check`; `check_finding` and
    // `check_disputed` write it so joins read one path, and carry no scalar
    // `koto.check` of their own.
    for new in [
        "koto.check.finding_count",
        "koto.check.findings_truncated",
        "koto.check.name",
    ] {
        assert!(written.contains(new), "{SOURCE_DIR} never writes {new}");
    }
}

#[test]
fn the_decider_fallback_attributes_collide_with_no_other_attribute() {
    let field = Regex::new(r"^koto\.[a-z_]+(\.[a-z_]+)*$").unwrap();
    let written: BTreeSet<String> = source_literals()
        .into_iter()
        .filter(|s| field.is_match(s))
        .collect();
    for new in [
        "koto.decider.input_bytes",
        "koto.decider.awaiting_count",
        "koto.decider.agent_answered_count",
    ] {
        assert!(written.contains(new), "{SOURCE_DIR} never writes {new}");
        for other in written.iter().filter(|o| o.as_str() != new) {
            assert!(
                !other.starts_with(&format!("{new}.")),
                "{other} nests under {new}"
            );
            assert!(
                !new.starts_with(&format!("{other}.")),
                "{new} nests under {other}"
            );
        }
    }
}
