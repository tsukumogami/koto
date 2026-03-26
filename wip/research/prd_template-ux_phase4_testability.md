# Testability Review

## Verdict: PASS
Acceptance criteria are specific, verifiable, and cover both happy paths and error conditions, with a small number of items that need tightening.

## Untestable Criteria

1. **"GitHub renders as a state diagram" (Export AC #3)**: Whether GitHub renders the file correctly depends on GitHub's Mermaid renderer, which is an external service outside test control. -> Reframe as: "output is valid `stateDiagram-v2` Mermaid syntax" (verifiable with a Mermaid parser or syntax check). GitHub rendering can be a manual smoke test.

2. **"Displays an interactive directed graph in the browser" (HTML AC #1)**: "Interactive" is subjective without specifying what interactions constitute pass/fail. The subsequent criteria (hover, click-to-highlight, pan/zoom) partially cover this, making the top-level criterion redundant. -> Either remove it (it's a summary, not a criterion) or restate as: "HTML file opens in a browser and renders a visible graph with nodes and edges matching the template states and transitions."

3. **"Pan/zoom works for navigating large graphs" (HTML AC #5)**: "Large" is undefined. What state count? How do you verify "works"? -> Reframe as: "Pan/zoom is enabled via Cytoscape.js configuration; a template with 20+ states renders without clipping or overlap that hides node labels."

4. **"Includes dark mode via `prefers-color-scheme`" (HTML AC #6)**: Testable only if you define what "dark mode" means visually. A CSS media query existing in the HTML is verifiable; correct visual appearance is subjective. -> Reframe as: "HTML contains a `prefers-color-scheme: dark` media query that sets background to a dark color and node/edge colors to light values" (inspectable in DOM/CSS, no visual judgment needed).

5. **"Works when served as a static page (GitHub Pages) without server-side processing" (HTML AC #9)**: Depends on an external hosting environment. -> Reframe as: "HTML file contains no server-side directives (PHP, SSI, etc.) and all resource references are absolute CDN URLs or inline" (inspectable from file content alone).

## Missing Test Coverage

1. **R5 cross-platform determinism**: The acceptance criteria test "same output twice" but don't test cross-platform byte-identity (Linux vs macOS vs Windows). R5 explicitly calls out "no platform-dependent output." -> Add AC: "Output generated on Linux and macOS for the same input is byte-identical."

2. **R6 compiled JSON input**: AC says "accepts both .md and .json" but doesn't specify behavior when given an invalid or malformed JSON file, or an .md file that fails compilation. -> Add error-case ACs for: invalid JSON input, .md that fails to compile, non-existent file path.

3. **R11 actionable error messages**: The freshness check ACs mention "prints the fix command" but don't specify what the fix command looks like or that it's runnable. -> Add AC: "The printed fix command, when copy-pasted and executed, resolves the drift (exit 0 on re-check)."

4. **R12 HTML file size**: No acceptance criterion for the 30 KB limit. -> Add AC: "Generated HTML for a 30-state template is under 30 KB."

5. **R13 compilation latency**: No acceptance criterion for the 500ms budget. -> Add AC: "Export of a 30-state template completes in under 500ms."

6. **R14 offline degradation**: No acceptance criterion for Mermaid working offline or HTML failing gracefully. -> Add AC: "Mermaid export succeeds without network access" and "HTML export succeeds without network access (rendered file requires CDN at view time, not generation time)."

7. **Edge case: empty template or single-state template**: No criteria for minimal inputs. A template with one state and no transitions should still produce valid output.

8. **Edge case: `--check` with wrong format**: What happens if you run `--check` against a mermaid file but pass `--format html`? The file exists but content won't match. This is covered implicitly by "stale file exits non-zero" but an explicit AC would prevent confusion.

9. **GHA workflow version/tag configuration**: R10 says the workflow is callable via `uses:` with a tag reference, but no AC verifies that the workflow accepts a version input or defaults to latest.

10. **Mermaid AC: gate annotations**: "Gates appear as `note` annotations" -- testable, but doesn't specify what content the note contains (gate name? gate command? both?). R2 says "gate annotations as notes" which is equally vague. -> Specify: "Gate notes contain the gate name and gate command."

## Summary

The acceptance criteria are well-structured and largely testable. Most items can be verified through command-line invocation and output inspection. The main gaps are: non-functional requirements (file size, latency, offline behavior) lack corresponding ACs; a few HTML criteria depend on subjective browser behavior that should be reframed as DOM/CSS inspections; and error/edge cases for invalid inputs are missing. Addressing 4 untestable criteria and 10 coverage gaps would make this fully test-plannable.
