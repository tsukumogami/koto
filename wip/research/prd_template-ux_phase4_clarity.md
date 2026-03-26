# Clarity Review

## Verdict: PASS

This PRD is well-structured with specific, testable requirements. The ambiguities found are minor and mostly in non-functional requirements rather than core behavior.

## Ambiguities Found

1. **R3 (HTML format), "color-coded by type"**: The PRD says state nodes are "color-coded by type" but never defines what the state types are or what colors map to them. Two developers could pick entirely different color schemes and type taxonomies.
   -> Define the state types (e.g., normal, gated, terminal) and either specify colors or say "colors are implementation-defined and tested only for distinctness."

2. **R3 (HTML format), "click-to-highlight for tracing paths"**: Does clicking a state highlight only its immediate neighbors (one hop) or the full transitive path to/from that state? The acceptance criteria say "incoming and outgoing transitions" which suggests one hop, but "tracing paths" in the requirement text implies multi-hop.
   -> Clarify: "Click a state to highlight its direct incoming and outgoing edges and their connected states (one hop)."

3. **R7 (Committed diagram artifacts), "configurable output path"**: For HTML files, the PRD says they're written to "a configurable output path" but doesn't specify how this is configured. Is it a CLI flag (already covered by `--output`), a config file, or a convention? The acceptance criteria don't test this configurability beyond what `--output` already provides.
   -> If `--output` is the configuration mechanism, say so explicitly. If there's a separate config mechanism, specify it.

4. **R9 (CI freshness workflow), "configurable path"**: The GHA workflow accepts "a configurable path" for templates. Does this mean a single directory, a glob pattern, multiple directories, or a list of individual files? The acceptance criteria say "configurable template path input" (singular) which still leaves this ambiguous.
   -> Specify the input type: "A glob pattern (e.g., `templates/**/*.md`) passed as a workflow input."

5. **R12 (HTML file size), "should be under 30 KB"**: The word "should" makes this advisory rather than enforceable. Is this a hard limit that CI checks, or a soft target? There's no acceptance criterion for it.
   -> Either add an acceptance criterion ("HTML output for templates up to 30 states is under 30 KB") or explicitly mark it as a guideline.

6. **R13 (Compilation latency), "should complete in under 500ms"**: Same ambiguity as R12. No acceptance criterion tests this. "Should" leaves room for interpretation.
   -> Add a benchmark test or mark as a non-binding target.

7. **R10 (Reusable workflow distribution), "downloads a release binary"**: The PRD says "downloads a release binary" but the open question about which release version to pin (latest? locked?) is not addressed. The GHA workflow needs to resolve a version somehow.
   -> Add a requirement or open question: specify whether the workflow pins a specific version, uses `latest`, or accepts a version input.

8. **Acceptance criteria, HTML output, "dark mode via prefers-color-scheme"**: This criterion exists in acceptance criteria but has no corresponding functional requirement in the R1-R14 list. It's testable but untraceable.
   -> Add dark mode support to R3 or note it as a detail of R3.

9. **R2 (Mermaid format), "gate annotations as notes"**: What content goes in the note? The gate name? The gate command? Both? The HTML acceptance criteria specify "gate name and command" for tooltips, but the Mermaid equivalent is unspecified.
   -> Specify: "Gates appear as `note` annotations containing the gate name."

10. **R3 (HTML format), "Cytoscape.js and dagre from CDN"**: Which CDN? jsdelivr, unpkg, cdnjs? The SRI hash requirement in acceptance criteria pins exact versions, but the CDN choice affects availability and security posture.
    -> Specify the CDN or say "any CDN that supports SRI hashes" and let the implementer choose.

## Suggested Improvements

1. **Add a visual mockup or example output**: Even a small hand-drawn Mermaid snippet showing the expected output for a 3-state template would eliminate interpretation gaps in R2 and R3. Two developers reading "stateDiagram-v2 with gate annotations as notes" could produce meaningfully different Mermaid syntax.

2. **Trace acceptance criteria back to requirements**: The dark mode criterion (HTML) and the `[*]` start marker node (HTML) appear in acceptance criteria but aren't explicit in R3. A traceability pass would catch these gaps.

3. **Specify error message format for R11**: "Actionable error messages" with "the exact command to run" is good intent but doesn't specify format. A concrete example (e.g., `Error: stale diagram. Run: koto template export workflow.md --output workflow.mermaid.md`) would remove ambiguity.

4. **Address the line ending open question before implementation**: The open question about LF normalization directly affects R5 (deterministic output) and CI freshness checks. This should be resolved before implementation begins, since it changes whether `.gitattributes` guidance is a requirement or a recommendation.

## Summary

The PRD is above average in specificity. Core functional requirements (R1-R8) are concrete and most acceptance criteria are binary pass/fail. The main gaps are in non-functional requirements that use "should" without enforcement criteria, a few under-specified details in the HTML format (color scheme, click behavior depth, CDN choice), and one open question (line endings) that directly impacts a hard requirement (deterministic output). None of these would cause a fundamentally wrong implementation, but they could cause rework during review.
