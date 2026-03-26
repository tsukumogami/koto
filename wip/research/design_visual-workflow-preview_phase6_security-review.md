# Security Review: visual-workflow-preview (Phase 6)

Review of the phase 5 security analysis and the design doc's Security Considerations section.

## 1. Attack Vectors Not Considered

### 1.1 JSON injection via template data (MISSED - medium severity)

The placeholder replacement mechanism is:

```rust
template_str.replace("/*KOTO_GRAPH_DATA*/", &json_data)
```

Neither the design doc nor the security analysis examines what happens if the serialized JSON contains the substring `</script>`. A `CompiledTemplate` embeds arbitrary user content: directive text (free-form markdown), gate commands (arbitrary shell strings), and variable defaults. If any of these contain `</script>`, the replacement produces HTML where the browser interprets the embedded string as a script tag boundary.

Example: a directive containing `</script><script>alert(1)</script>` would break out of the data assignment and execute arbitrary JavaScript.

**Mitigation required:** After `serde_json::to_string()`, replace all occurrences of `</` with `<\/` in the JSON output before inserting into the HTML template. This is standard practice for embedding JSON in `<script>` tags. The design doc's claim that "serde_json handles all escaping" is incorrect -- serde_json produces valid JSON, but JSON validity does not guarantee safe embedding in HTML script contexts.

### 1.2 Template filename with special characters (MISSED - low severity)

The design derives the output filename as `<template-stem>.preview.html`. The phase 5 report covers `../` path traversal (correctly noting `Path::file_stem()` handles it), but doesn't consider filenames containing shell metacharacters or whitespace when passed to `opener::open()`. The `opener` crate on Linux calls `xdg-open` with the path as an argument. If `xdg-open` delegates to a handler that doesn't properly quote the path, filenames with spaces or special characters could cause unexpected behavior.

**Risk is low** since `opener` passes the path as a single argument to the OS API (not through shell expansion), but worth a note in implementation.

### 1.3 CDN availability as a denial-of-rendering attack (NOT COVERED)

If unpkg.com is temporarily unavailable or blocked by corporate firewalls/proxies, the preview file renders as a blank page with no error indication. This isn't a traditional security issue, but it creates a user experience where someone commits a preview file to GitHub Pages documentation, and readers see nothing.

**Mitigation (minor):** The HTML template should include a `<noscript>` fallback and/or an `onerror` handler on the script tags that displays a message explaining the CDN dependency.

### 1.4 `crossorigin="anonymous"` and CORS behavior (NOT COVERED)

The design specifies `crossorigin="anonymous"` on script tags (required for SRI). If unpkg.com ever changes its CORS headers to not include `Access-Control-Allow-Origin: *`, SRI checks would fail and scripts wouldn't load. This is extremely unlikely for a public CDN but represents a single-point dependency.

No mitigation needed -- just an observation about the trust relationship.

## 2. Sufficiency of Mitigations

### 2.1 SRI hashes -- SUFFICIENT but implementation detail missing

Both documents correctly identify SRI as the primary mitigation for CDN compromise. The design doc includes SRI in the Security Considerations section and specifies `sha384` hashes. This is the right call.

**Gap:** Neither document specifies how SRI hashes are computed or verified during development. The process should be documented: download the pinned version, compute `sha384` hash, embed in the template. A CI check that verifies the embedded hashes match freshly-computed ones against the pinned URLs would prevent accidental hash staleness.

### 2.2 Path traversal via `Path::file_stem()` -- SUFFICIENT

Both documents identify this and the Rust standard library handles it correctly. No gap.

### 2.3 Data exposure documentation -- MOSTLY SUFFICIENT

The design doc includes a clear statement about treating preview files with the same sensitivity as source templates, and recommends Mermaid for sharing. The phase 5 report suggests adding a visible banner in the HTML.

**Gap:** The banner suggestion from phase 5 didn't make it into the design doc. Given that preview files may be committed and opened by people who didn't generate them, a small visible notice ("This file contains embedded workflow data including shell commands") would be a worthwhile addition.

### 2.4 File overwrite without confirmation -- ACCEPTABLE

The phase 5 report flags this as low severity. For a developer tool that regenerates output files, silent overwrite is standard behavior (matches `cargo doc`, `mdBook build`, etc.). No mitigation needed.

## 3. "Not Applicable" Justification Review

The phase 5 analysis doesn't explicitly mark any dimensions as "not applicable" -- it evaluates all four dimensions (External Artifact Handling, Permission Scope, Supply Chain, Data Exposure) as applicable and provides analysis for each. This is correct; all four dimensions are relevant.

However, there are implicit gaps where a dimension applies but wasn't fully explored:

### 3.1 Input validation (implicit N/A)

Neither document discusses validation of the `CompiledTemplate` data before rendering. The Mermaid export function writes state names directly into the Mermaid syntax. If a state name contains Mermaid syntax characters (`-->`, `[*]`, newlines), the output could produce malformed diagrams or, in theory, inject additional Mermaid directives. This is low severity (Mermaid injection can't execute code), but it could produce confusing output.

**Recommendation:** Sanitize or quote state names in Mermaid output. For the HTML preview, state names appear in Cytoscape.js data objects (JSON), so serde_json handles escaping there correctly.

### 3.2 Denial of service via large templates (implicit N/A)

A template with hundreds of states would produce a very large HTML file and potentially cause the browser to struggle with layout. The design mentions "30+ states" as a target. Neither document considers the upper bound.

**Risk is negligible** for a developer tool -- users who create absurdly large templates will see slow rendering, which is self-correcting feedback.

## 4. Residual Risk Assessment

### Residual risks that should be escalated:

**None.** The design's security posture is appropriate for a local developer tool that generates static HTML files. The highest-severity issue (CDN compromise) is fully addressed by SRI hashes, which the design already specifies.

### Residual risks that should be tracked but don't block:

1. **JSON-in-HTML injection (section 1.1 above).** This is the one finding that requires a code-level fix. It's not in the design doc's security section and wasn't caught in phase 5. Should be added as a required implementation detail alongside SRI.

2. **SRI hash verification in CI.** Without automated verification, hash staleness during CDN version bumps is likely. Should be added to the implementation plan as a CI task.

3. **Embedded data sensitivity banner.** A one-line HTML notice reduces the chance of accidental data exposure when preview files are shared.

## Summary Table

| Finding | Severity | Status in Design | Status in Phase 5 | Action |
|---------|----------|------------------|--------------------|--------|
| `</script>` injection in embedded JSON | Medium | Not covered | Not covered | Add `</` escaping as required implementation detail |
| SRI hashes on CDN scripts | Medium (if absent) | Covered | Covered | Already specified; add CI verification |
| Data exposure in preview files | Low-Medium | Covered | Covered | Add HTML banner per phase 5 suggestion |
| Mermaid state name sanitization | Low | Not covered | Not covered | Add to implementation notes |
| Script load failure UX | Low | Not covered | Mentioned (availability) | Add onerror fallback message |
| File overwrite behavior | Low | Not covered | Covered | Acceptable; no action |
| Path traversal | Low | Covered | Covered | Already mitigated |
| Large template rendering | Negligible | Not covered | Not covered | No action needed |
