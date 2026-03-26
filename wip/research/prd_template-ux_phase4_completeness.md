# Completeness Review

## Verdict: PASS

The PRD is thorough and implementable. The issues found are minor gaps rather than structural deficiencies.

## Issues Found

1. **No AC for R6 (source vs compiled input differentiation)**: R6 says the command accepts both `.md` source templates and `.json` compiled files. The AC section has one bullet that mentions this, but there's no criterion verifying the behavior when given a `.json` file specifically -- e.g., that Mermaid output from `workflow.json` matches output from `workflow.md` for the same template. Add an AC that exports from both input types and compares output.

2. **No AC for R5 determinism across platforms**: R5 says "no platform-dependent output." The AC bullets verify byte-identical output across runs, but only implicitly on a single platform. There's no criterion that addresses cross-platform determinism (e.g., line endings, path separators). The open question about line endings acknowledges this gap but doesn't resolve it. Suggest promoting the `.gitattributes` guidance from open question to a requirement or adding an AC that output uses LF unconditionally.

3. **No AC for R12 (HTML file size)**: R12 specifies "under 30 KB excluding CDN-loaded dependencies" but there's no acceptance criterion that verifies this. Add an AC: generated HTML for a 30-state template is under 30 KB.

4. **No AC for R13 (compilation latency)**: R13 specifies "under 500ms for templates up to 30 states" but has no corresponding AC. Add a performance AC or explicitly note this is verified by benchmarks outside the AC checklist.

5. **R14 (offline degradation) has no AC**: R14 states Mermaid works offline and HTML requires internet. No AC verifies either claim. While the Mermaid case is trivially true (it's text), an AC confirming HTML gracefully degrades (or at least doesn't crash) without internet would be useful.

6. **Missing error case: invalid input file**: No AC covers what happens when the input file is not a valid template or compiled JSON. The command should produce a clear error. This matters because users will inevitably pass the wrong file.

7. **Scope document mentioned "docs reader" persona but PRD website story lacks specificity**: The scope document identified three personas. The PRD expanded to four (adding repo maintainer), which is good. However, the "documentation reader on a project website" story doesn't specify how the HTML file gets deployed. The PRD correctly marks deployment pipelines as the consumer's responsibility, but a brief note in the user story or requirements about how `--output` supports deployment workflows (e.g., outputting to a `docs/` directory) would help implementers understand the intended integration point.

8. **GHA workflow: no version pinning strategy for koto binary**: R9/R10 say the workflow downloads a release binary. There's no requirement or AC specifying how the consuming repo pins which koto version to use. Should the workflow accept a `version` input? Without this, consuming repos can't control which koto version runs in their CI.

## Suggested Improvements

1. **Add a "flag compatibility matrix"**: The interaction rules between `--format`, `--output`, `--check`, and `--open` are scattered across multiple requirements. A small table showing valid/invalid combinations would prevent implementer guesswork and reduce ambiguity. For example: is `--open --check` valid? (Probably not, but it's not stated.)

2. **Specify Mermaid output structure more precisely**: R2 says "stateDiagram-v2" with states, transitions, conditions, gate annotations, and `[*]` markers. An implementer would benefit from a brief example showing the expected output for a simple 3-state template. This doesn't need to be normative -- just illustrative.

3. **Clarify `--check` behavior with `--format mermaid` and no `--output`**: R8 says `--check` requires `--output`. For mermaid, R7 defines the conventional sibling file path (`<stem>.mermaid.md`). Should `--check` infer the output path from the input path when the format is mermaid? This would simplify CI scripts. If not, the CI workflow (R9) needs to compute the path itself.

4. **Resolve the line-ending open question before leaving Draft**: This directly impacts R5 (deterministic output) and the CI freshness check (R8). Leaving it open means an implementer must make a judgment call that could break cross-platform CI. Recommend deciding on LF-only output and documenting the `.gitattributes` recommendation.

## Summary

The PRD covers its problem space well, with clear requirements, justified trade-offs, and a well-scoped out-of-scope section. The main gaps are missing acceptance criteria for non-functional requirements (R12-R14) and a few edge cases (invalid input, flag combinations, cross-platform determinism). The open question about line endings should be resolved before implementation begins since it directly affects the determinism guarantee that CI enforcement depends on.
