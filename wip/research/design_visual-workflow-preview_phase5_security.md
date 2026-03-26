# Security Review: visual-workflow-preview

## Dimension Analysis

### External Artifact Handling

**Applies:** Yes

The generated HTML file loads three JavaScript libraries at runtime from unpkg.com CDN:
- `cytoscape@3.30.4`
- `dagre@0.8.5`
- `cytoscape-dagre@2.5.0`

These are fetched by the user's browser when they open the preview file. koto itself doesn't download or execute anything -- the binary only writes an HTML file and calls `opener` to launch it. However, the browser will execute whatever JavaScript those CDN URLs serve.

**Risks:**

1. **CDN compromise or hijack (medium severity).** If unpkg.com is compromised, serves a malicious package version, or an attacker gains control of one of those npm packages, the browser executes arbitrary JS with full access to the page's DOM and any data embedded in it. Pinning exact versions (e.g., `@3.30.4`) reduces the window but doesn't eliminate it -- a compromised CDN could serve different content for the same version URL.

2. **Network-dependent availability (low severity).** The preview won't render in offline/airgapped environments. This isn't a security issue per se, but it's a failure mode worth noting since workflow templates may contain sensitive operational data that users might not want fetched over a network connection.

**Mitigations:**

- Add [Subresource Integrity (SRI)](https://developer.mozilla.org/en-US/docs/Web/Security/Subresource_Integrity) hashes to all `<script>` tags. This is the single highest-value mitigation: the browser will refuse to execute any script whose content doesn't match the expected hash, neutralizing CDN compromise. The `integrity` attribute with a `sha384` or `sha512` hash and `crossorigin="anonymous"` is all that's needed.
- Document that the preview file requires network access. Consider a `--offline` flag in future iterations that inlines the JS bundles.

### Permission Scope

**Applies:** Yes

The feature performs two side effects beyond stdout:

1. **File write to current directory.** `koto template preview` writes `<template-stem>.preview.html` to the working directory. This is a predictable filename in a user-controlled location.

2. **Process launch via `opener` crate.** This delegates to the OS default browser handler (`xdg-open`, `open`, or `start`). The design specifies graceful fallback if the launch fails.

**Risks:**

1. **File overwrite without confirmation (low severity).** If a file named `<stem>.preview.html` already exists, it gets silently overwritten. In normal use this is expected behavior (regenerating a preview), but it's worth noting.

2. **Path traversal via template filename (low severity).** If the template path contains `../` components, the output filename derivation needs to use only the final stem, not reconstruct a path that could write outside the current directory. Standard `Path::file_stem()` in Rust handles this correctly.

3. **`opener` crate trust (low severity).** The crate simply shells out to OS utilities with the file path as argument. No escalation risk beyond what the user's default browser handler already permits.

**Mitigations:**

- Use `Path::file_stem()` (not manual string manipulation) for output filename derivation.
- Consider printing the output path to stderr so users know exactly what was written and opened.

### Supply Chain or Dependency Trust

**Applies:** Yes

Two dependency additions:

1. **`opener` crate (Rust, from crates.io).** Small, well-known crate for launching URLs/files with the OS default handler. It has minimal transitive dependencies and a straightforward implementation (calls `xdg-open`/`open`/`start`).

2. **CDN-loaded JS libraries (npm packages via unpkg.com).** These aren't Rust dependencies but runtime dependencies of the generated artifact. Trust is placed in: (a) unpkg.com's infrastructure, (b) the npm package maintainers for cytoscape, dagre, and cytoscape-dagre.

**Risks:**

1. **`opener` crate compromise (low severity).** Standard crates.io supply chain risk, mitigated by Cargo.lock pinning and the crate's narrow scope.

2. **npm/CDN supply chain for JS libraries (medium severity).** The generated HTML creates an implicit supply chain dependency on three npm packages. Users who share preview files (e.g., committing them, hosting on GH Pages) extend this trust chain to anyone who opens the file.

**Mitigations:**

- SRI hashes (covered above) are the primary mitigation for CDN trust.
- Pin `opener` to a specific version in Cargo.toml and review its dependency tree.
- The design doc's mention of GH Pages hosting makes SRI even more important -- shared preview files should not become a vector for serving compromised JS to third parties.

### Data Exposure

**Applies:** Yes

The compiled template JSON is embedded directly in the generated HTML file. This data includes:

- State names and directive text (markdown content with instructions for agents)
- Transition targets and conditions
- Gate commands (shell commands that agents execute)
- Evidence schema field types
- Variable declarations
- Default action commands (shell commands)

**Risks:**

1. **Sensitive command exposure in preview files (medium severity).** Gate commands and default action commands are shell commands that may reference internal paths, tool names, API endpoints, or environment variable names. If preview files are shared (GH Pages, committed to repos), this information becomes public.

2. **Directive content exposure (low-medium severity).** Directive text contains operational instructions for agents. Depending on the workflow, these could include references to internal systems, credentials handling patterns, or proprietary processes.

3. **CDN request metadata (low severity).** When a user opens the preview, their browser makes requests to unpkg.com, revealing their IP address and the fact that they're using koto. This is standard for any CDN-loaded page but worth noting for users with strict privacy requirements.

**Mitigations:**

- Document clearly that preview HTML files contain the full compiled template data in plaintext. Users should treat these files with the same sensitivity as the source templates.
- Consider adding a visible banner in the generated HTML noting that it contains embedded workflow data.
- The `export --format mermaid` command (stdout-only by default) is inherently safer for sharing since Mermaid diagrams contain only state names and transitions, not full directive text or commands. Documentation should recommend Mermaid export for sharing and preview for local debugging.

## Recommended Outcome

**OPTION 2: Document considerations.**

No design changes are strictly required, but one strong recommendation rises above documentation:

**SRI hashes on CDN script tags should be treated as a required implementation detail, not optional.** Without SRI, the design trusts unpkg.com's infrastructure unconditionally for every user who opens a preview file. With SRI, trust is pinned to specific artifact content at build time. This is a one-line-per-script-tag addition with outsized security value.

Beyond SRI, the remaining items are documentation concerns: make users aware that preview files embed full template data, recommend Mermaid export for sharing, and note the network dependency.

## Summary

The design's primary security surface is the CDN-loaded JavaScript in generated HTML files. Adding Subresource Integrity hashes to the three script tags eliminates the most significant risk (CDN compromise) with minimal implementation effort. The embedded template data -- which includes shell commands and agent directives -- should be documented as sensitive, with guidance to prefer Mermaid export for sharing. No architectural changes are needed; this is an OPTION 2 outcome with SRI hashes as a strong implementation recommendation.
