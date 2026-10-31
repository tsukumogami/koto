//! Keeps koto's `/workflows` shape fixture and the Claude Code convention
//! guard's file-shape probes in step, offline.
//!
//! `scripts/check-claude-code-conventions.sh` checks a Claude Code release for
//! the names koto's file relies on, one probe per field, listed in
//! `tests/fixtures/native-workflows/claude-code-conventions.json`. The guard
//! only checks what that file lists, so a field koto starts writing without a
//! probe would go unchecked. This test walks the shape fixture koto's own shape
//! test pins (`tests/fixtures/native-workflows/enriched-shape.json`), collects
//! every key outside the top-level `koto` block and every `type` value, and
//! fails naming any that is neither a file-shape probe's `item` nor listed in
//! that convention's `unread` list, the fields koto writes that Claude Code
//! doesn't read, each with its reason. Keys inside `koto` are skipped: Claude
//! Code ignores that block.
//!
//! It also checks the recorded release the guard reads by default parses as a
//! `MAJOR.MINOR.PATCH` version. Nothing here touches the network.

use std::collections::BTreeSet;

use serde_json::Value;

const SHAPE: &str = "tests/fixtures/native-workflows/enriched-shape.json";
const CONVENTIONS: &str = "tests/fixtures/native-workflows/claude-code-conventions.json";
const RELEASE: &str = "tests/fixtures/native-workflows/claude-code-release";
const FILE_SHAPE_SLUG: &str = "file-shape";

fn read_json(path: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

/// The names Claude Code would have to know to read `shape`: every object key
/// at any depth, except the top-level `koto` block and what is under it, and
/// every string value of a `type` key (the progress node types).
fn screen_names(shape: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    match shape {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "koto" {
                    continue;
                }
                collect(key, value, &mut names);
            }
        }
        other => walk(other, &mut names),
    }
    names
}

fn collect(key: &str, value: &Value, names: &mut BTreeSet<String>) {
    names.insert(key.to_string());
    if key == "type" {
        if let Value::String(node_type) = value {
            names.insert(node_type.clone());
        }
    }
    walk(value, names);
}

fn walk(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                collect(key, inner, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, names);
            }
        }
        _ => {}
    }
}

fn file_shape(conventions: &Value) -> &Value {
    conventions["conventions"]
        .as_array()
        .unwrap_or_else(|| panic!("{CONVENTIONS}: conventions is not a list"))
        .iter()
        .find(|c| c["slug"] == FILE_SHAPE_SLUG)
        .unwrap_or_else(|| panic!("{CONVENTIONS}: no convention with slug {FILE_SHAPE_SLUG}"))
}

fn strings_at(list: &Value, key: &str) -> BTreeSet<String> {
    list.as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e[key].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every name the screen reads from koto's file has a file-shape probe, or is
/// listed as a field Claude Code doesn't read.
#[test]
fn every_shape_fixture_field_has_a_file_shape_probe() {
    let names = screen_names(&read_json(SHAPE));
    let conventions = read_json(CONVENTIONS);
    let shape = file_shape(&conventions);
    let probed = strings_at(&shape["probes"], "item");
    let unread = strings_at(&shape["unread"], "field");

    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !probed.contains(*n) && !unread.contains(*n))
        .collect();
    assert!(
        missing.is_empty(),
        "fields in {SHAPE} with no file-shape probe in {CONVENTIONS}: {}. \
         Find where Claude Code's /workflows reader uses each one and add a \
         probe whose item is the field name, then run \
         scripts/check-claude-code-conventions.sh against the recorded release \
         and --alter file-shape (docs/guides/native-workflows-verification.md). \
         A field Claude Code doesn't read belongs in the koto block, or in the \
         file-shape convention's unread list with the reason.",
        missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// The unread list only exempts fields koto actually writes, so an entry
/// can't outlive its field and quietly cover a later one of the same name.
#[test]
fn unread_fields_are_fields_koto_writes() {
    let names = screen_names(&read_json(SHAPE));
    let conventions = read_json(CONVENTIONS);
    let unread = strings_at(&file_shape(&conventions)["unread"], "field");
    let stale: Vec<&String> = unread.iter().filter(|f| !names.contains(*f)).collect();
    assert!(
        stale.is_empty(),
        "{CONVENTIONS} lists unread fields that {SHAPE} doesn't have: {stale:?}; \
         remove them"
    );
}

/// The release the guard checks by default is one `MAJOR.MINOR.PATCH` line.
#[test]
fn recorded_release_is_a_version() {
    let text = std::fs::read_to_string(RELEASE).unwrap_or_else(|e| panic!("read {RELEASE}: {e}"));
    let version = text.strip_suffix('\n').unwrap_or(&text);
    let re = regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$").unwrap();
    assert!(
        re.is_match(version),
        "{RELEASE} must hold one MAJOR.MINOR.PATCH version, found {version:?}"
    );
}

/// The walk skips the `koto` block and nothing else: a key added beside it is
/// collected, at the top level or inside a node, while one added inside it
/// isn't, and node types are collected as names.
#[test]
fn screen_names_skip_only_the_koto_block() {
    let shape = serde_json::json!({
        "workflowName": "w",
        "probeCheck": 1,
        "workflowProgress": [{"type": "workflow_agent", "nodeField": true}],
        "koto": {"sessionId": "s", "insideKoto": {"deeper": 1}}
    });
    let names = screen_names(&shape);
    for expected in [
        "workflowName",
        "probeCheck",
        "workflowProgress",
        "type",
        "workflow_agent",
        "nodeField",
    ] {
        assert!(names.contains(expected), "{expected} not collected");
    }
    for skipped in ["koto", "sessionId", "insideKoto", "deeper", "s", "w"] {
        assert!(!names.contains(skipped), "{skipped} collected");
    }
}
