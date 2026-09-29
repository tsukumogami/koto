//! Keeps value routing's documentation complete: every `E-VAR-ROUTE-*` code
//! the compiler can emit is listed in the error-code reference and the
//! template-format reference, the two event fields are in the session-feed
//! contract, the koto-user skill tells an agent what a value route does to a
//! response, and the template-format reference's example compiles.
//!
//! docs/designs/DESIGN-koto-value-routing.md, Implementation Approach step 4.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

const TEMPLATE_FORMAT: &str =
    "plugins/koto-skills/skills/koto-author/references/template-format.md";

/// Every `E-VAR-ROUTE-<NAME>` the compiler source names.
fn emitted_codes() -> BTreeSet<String> {
    let src = read("src/template/types.rs");
    let mut codes = BTreeSet::new();
    let mut rest = src.as_str();
    while let Some(at) = rest.find("E-VAR-ROUTE-") {
        let tail = &rest[at..];
        let end = tail["E-VAR-ROUTE-".len()..]
            .find(|c: char| !(c.is_ascii_uppercase()))
            .map(|n| n + "E-VAR-ROUTE-".len())
            .unwrap_or(tail.len());
        codes.insert(tail[..end].to_string());
        rest = &tail[end..];
    }
    codes.remove("E-VAR-ROUTE-");
    codes
}

#[test]
fn every_emitted_code_is_documented() {
    let codes = emitted_codes();
    assert_eq!(
        codes.len(),
        4,
        "expected the four E-VAR-ROUTE codes, found {codes:?}"
    );
    for doc in ["docs/reference/error-codes.md", TEMPLATE_FORMAT] {
        let text = read(doc);
        for code in &codes {
            assert!(text.contains(code.as_str()), "{doc} doesn't list {code}");
        }
    }
}

#[test]
fn the_event_fields_are_in_the_session_feed_contract() {
    let feed = read("docs/reference/session-feed.md");
    for field in ["vars_matched:", "previous:", "`vars_matched`", "`previous`"] {
        assert!(
            feed.contains(field),
            "session-feed.md doesn't mention {field}"
        );
    }
}

#[test]
fn the_user_skill_explains_value_routes() {
    let skill = read("plugins/koto-skills/skills/koto-user/SKILL.md");
    assert!(skill.contains("vars.MODE: auto"), "koto-user SKILL.md");
    let shapes = read("plugins/koto-skills/skills/koto-user/references/response-shapes.md");
    assert!(shapes.contains("vars_matched"), "response-shapes.md");
}

#[test]
fn the_template_format_example_compiles() {
    let doc = read(TEMPLATE_FORMAT);
    let section = doc
        .split("### Routing on a variable's value")
        .nth(1)
        .expect("the value routing section");
    let yaml = section
        .split("```yaml\n")
        .nth(1)
        .and_then(|s| s.split("```").next())
        .expect("a yaml example");
    let src = format!(
        "---\nname: example\nversion: \"1.0\"\ninitial_state: confirm\n{yaml}  proceed:\n    terminal: true\n  ask_author:\n    terminal: true\n---\n\n## confirm\n\nConfirm.\n\n## proceed\n\nGo.\n\n## ask_author\n\nAsk.\n"
    );
    let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    f.write_all(src.as_bytes()).unwrap();
    koto::template::compile::compile(f.path(), true)
        .unwrap_or_else(|e| panic!("the documented example doesn't compile: {e:#}\n{src}"));
}
