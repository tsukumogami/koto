use crate::template::types::CompiledTemplate;

/// The HTML template is embedded at compile time.
const TEMPLATE_HTML: &str = include_str!("preview.html");

/// The placeholder marker in the HTML template that gets replaced with
/// serialized graph data.
const PLACEHOLDER: &str = "/*KOTO_GRAPH_DATA*/{}";

/// Generate a self-contained HTML file with an interactive Cytoscape.js
/// diagram of the given compiled template.
///
/// Returns the HTML as bytes with LF line endings. The caller is responsible
/// for writing the bytes to the output path.
pub fn generate_html(template: &CompiledTemplate) -> Vec<u8> {
    let json = serde_json::to_string(template).expect("CompiledTemplate is always serializable");

    // Escape </ as <\/ to prevent script context injection when the JSON
    // is embedded inside a <script> tag.
    let safe_json = json.replace("</", "<\\/");

    let html = TEMPLATE_HTML.replace(PLACEHOLDER, &safe_json);

    // Normalize to LF line endings.
    let normalized = html.replace("\r\n", "\n");

    normalized.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::types::{
        CompiledTemplate, FieldSchema, Gate, TemplateState, Transition, GATE_TYPE_COMMAND,
    };
    use std::collections::BTreeMap;

    fn test_template() -> CompiledTemplate {
        let mut states = BTreeMap::new();

        let mut accepts = BTreeMap::new();
        accepts.insert(
            "route".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["build".to_string(), "investigate".to_string()],
                description: String::new(),
            },
        );

        states.insert(
            "start".to_string(),
            TemplateState {
                directive: "Begin work.".to_string(),
                details: String::new(),
                transitions: vec![
                    Transition {
                        target: "build".to_string(),
                        when: Some({
                            let mut m = BTreeMap::new();
                            m.insert("route".to_string(), serde_json::json!("build"));
                            m
                        }),
                    },
                    Transition {
                        target: "investigate".to_string(),
                        when: Some({
                            let mut m = BTreeMap::new();
                            m.insert("route".to_string(), serde_json::json!("investigate"));
                            m
                        }),
                    },
                ],
                terminal: false,
                gates: {
                    let mut g = BTreeMap::new();
                    g.insert(
                        "check-repo".to_string(),
                        Gate {
                            gate_type: GATE_TYPE_COMMAND.to_string(),
                            command: "test -d .git".to_string(),
                            timeout: 0,
                            key: String::new(),
                            pattern: String::new(),
                            override_default: None,
                            completion: None,
                            name_filter: None,
                        },
                    );
                    g
                },
                accepts: Some(accepts),
                integration: None,
                default_action: None,
                materialize_children: None,
                failure: false,
                skipped_marker: false,
            },
        );
        states.insert(
            "build".to_string(),
            TemplateState {
                directive: "Build the feature.".to_string(),
                details: String::new(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
                materialize_children: None,
                failure: false,
                skipped_marker: false,
            },
        );
        states.insert(
            "investigate".to_string(),
            TemplateState {
                directive: "Investigate the issue.".to_string(),
                details: String::new(),
                transitions: vec![Transition {
                    target: "done".to_string(),
                    when: None,
                }],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
                materialize_children: None,
                failure: false,
                skipped_marker: false,
            },
        );
        states.insert(
            "done".to_string(),
            TemplateState {
                directive: "Work complete.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
                materialize_children: None,
                failure: false,
                skipped_marker: false,
            },
        );

        CompiledTemplate {
            format_version: 1,
            name: "test-workflow".to_string(),
            version: "1.0".to_string(),
            description: "A test workflow".to_string(),
            initial_state: "start".to_string(),
            variables: BTreeMap::new(),
            states,
        }
    }

    #[test]
    fn contains_template_data() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(
            text.contains("test-workflow"),
            "should contain template name"
        );
        assert!(
            text.contains("Begin work."),
            "should contain state directive"
        );
        assert!(text.contains("check-repo"), "should contain gate name");
        assert!(
            text.contains("investigate"),
            "should contain transition target"
        );
    }

    #[test]
    fn contains_sri_hashes() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        // Every CDN script tag must have an integrity attribute.
        for line in text.lines() {
            if line.contains("<script src=") && line.contains("unpkg.com") {
                assert!(
                    text.contains("integrity=\"sha384-"),
                    "CDN script tag must have SRI integrity hash: {}",
                    line
                );
                assert!(
                    text.contains("crossorigin=\"anonymous\""),
                    "CDN script tag must have crossorigin attribute: {}",
                    line
                );
            }
        }
    }

    #[test]
    fn no_server_side_directives() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(
            !text.contains("<?"),
            "should not contain PHP-style directives"
        );
        assert!(
            !text.contains("<%"),
            "should not contain ASP-style directives"
        );
        // Note: {{ can appear in the serialized JSON data legitimately (template
        // variable references), so we only check for server-side template markers.
    }

    #[test]
    fn lf_line_endings() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(!text.contains("\r\n"), "output must use LF, not CRLF");
        assert!(!text.contains('\r'), "output must not contain CR");
    }

    #[test]
    fn script_injection_prevention() {
        // Create a template with a </script> in the directive to test escaping.
        let mut states = BTreeMap::new();
        states.insert(
            "evil".to_string(),
            TemplateState {
                directive: "Contains </script> tag".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
                materialize_children: None,
                failure: false,
                skipped_marker: false,
            },
        );
        let t = CompiledTemplate {
            format_version: 1,
            name: "injection-test".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "evil".to_string(),
            variables: BTreeMap::new(),
            states,
        };
        let html = generate_html(&t);
        let text = String::from_utf8(html).unwrap();
        // The raw </script> from the directive must be escaped in the JSON.
        // Count occurrences of </script> -- should only be the legitimate closing tags.
        let closing_script_count = text.matches("</script>").count();
        // There should be exactly 4 closing script tags: 3 CDN + 1 inline script.
        assert_eq!(
            closing_script_count, 4,
            "injected </script> must be escaped; found {} closing tags",
            closing_script_count
        );
        // The escaped form must appear in the output.
        assert!(
            text.contains("<\\/script>"),
            "escaped </script> must appear as <\\/script>"
        );
    }

    #[test]
    fn deterministic_output() {
        let t = test_template();
        let first = generate_html(&t);
        let second = generate_html(&t);
        assert_eq!(first, second, "output must be byte-identical across runs");
    }

    #[test]
    fn placeholder_is_replaced() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(
            !text.contains("/*KOTO_GRAPH_DATA*/"),
            "placeholder must be replaced with actual data"
        );
    }

    #[test]
    fn valid_html_structure() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(text.contains("<!DOCTYPE html>"), "must have DOCTYPE");
        assert!(text.contains("<html"), "must have html tag");
        assert!(text.contains("</html>"), "must close html tag");
        assert!(text.contains("<head>"), "must have head");
        assert!(text.contains("</head>"), "must close head");
        assert!(text.contains("<body>"), "must have body");
        assert!(text.contains("</body>"), "must close body");
    }

    #[test]
    fn contains_start_marker() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(
            text.contains("__start__"),
            "should contain [*] start marker node"
        );
    }

    #[test]
    fn contains_dark_mode() {
        let html = generate_html(&test_template());
        let text = String::from_utf8(html).unwrap();
        assert!(
            text.contains("prefers-color-scheme: dark"),
            "should contain dark mode media query"
        );
    }
}
