# Decision 3: HTML Generation Architecture

## Chosen: Option A -- include_str! with string replacement

## Rationale

The HTML preview is a single self-contained file where the only dynamic content is a JSON blob representing the compiled template graph. `include_str!("preview.html")` embeds a complete, valid HTML file at compile time, and a single `.replace("/*KOTO_GRAPH_DATA*/", &json_data)` call injects the template-specific data at runtime.

This approach wins because:

1. **The template is a real HTML file.** Developers can open `preview.html` directly in a browser during design iteration. The placeholder sits inside a `<script>` block as a valid JS comment, so the file renders (with empty data) without modification. This makes CSS/JS changes fast -- edit, refresh, done.

2. **Zero new dependencies.** koto's dependency list is already lean. Adding askama or tera for a single variable injection adds compile time and surface area that isn't justified. If future features need conditionals or loops in the template, that's the right time to reconsider.

3. **The "fragility" concern is overstated.** The replacement target is a JS comment inside a known `const` declaration (`const GRAPH_DATA = /*KOTO_GRAPH_DATA*/{};`). A compile-time `debug_assert!` or unit test that the placeholder exists in the embedded string catches drift immediately. There's exactly one replacement, not a pattern of them.

4. **Matches the project's existing style.** koto already uses `include_str!` for embedded content (template compilation). String replacement for a single injection point is idiomatic for this scale of problem.

## Alternatives Considered

**Option B (split template into head/middle/tail):** Solves a problem that doesn't meaningfully exist. The fragility of one well-tested string replacement doesn't justify splitting a cohesive HTML file into three fragments that can't be previewed individually. Developers lose the ability to open the template in a browser during iteration, which is the single biggest productivity advantage of Option A.

**Option C (askama/tera):** Proper template engines shine when you have multiple variables, conditionals, loops, or partial includes. This feature has one injection point. Adding a proc-macro dependency (askama) or a runtime engine (tera) for `{{ graph_data }}` is over-engineering. If the preview grows to need conditional sections (e.g., different layouts per template type), revisit this.

**Option D (programmatic construction):** HTML in Rust string literals is painful to read, edit, and iterate on. The preview involves non-trivial CSS and JS (tooltips, click-to-highlight, legend rendering). Burying that in `format!()` calls makes design changes require recompilation and eliminates the browser-preview workflow entirely.

## Sub-decisions

### Output file path

The default output path is `<template-stem>.preview.html` in the current working directory. For example, `koto template preview workflow.md` produces `./workflow.preview.html`.

Rationale:
- Placing it next to where the user invoked the command is predictable and discoverable.
- The `.preview.html` suffix distinguishes it from source files and is obvious in file listings.
- A `--output` / `-o` flag allows override for CI or custom workflows (e.g., writing to a `docs/` directory for GitHub Pages).
- Avoid writing into the template's source directory by default -- the user may not have write access, or the template may come from a registry path.

### CDN version pinning

Hardcode specific versions in the HTML template. Current targets:

- `cytoscape@3.30.4`
- `dagre@0.8.5`
- `cytoscape-dagre@2.5.0`

Rationale:
- Reproducibility matters more than auto-updating for a developer tool. A preview that renders differently across runs because a CDN version changed is a debugging hazard.
- Version bumps happen via PRs that update the template file, which lets CI catch rendering regressions.
- No configuration flag needed. Users who want different versions can edit the generated HTML directly -- it's a standalone file, not a locked artifact.

## Assumptions

- The compiled template data serializes cleanly to JSON via serde_json (already a dependency). No special escaping beyond what `serde_json::to_string` provides is needed, since the data goes into a JS variable assignment, not an HTML attribute.
- The HTML template will stabilize relatively quickly. If it turns out the preview needs multiple injection points (e.g., separate metadata block, per-state custom CSS), the migration path to askama is straightforward and the template file structure transfers directly.
- CDN availability is acceptable for the target audience. Offline use isn't a launch requirement (the explore phase already decided on CDN over inlining).

## Confidence: High

This is a well-understood pattern. The decision is easy to reverse (swap `include_str!` + replace for askama) if requirements grow, and the migration cost is low since the HTML template file carries over as-is.
