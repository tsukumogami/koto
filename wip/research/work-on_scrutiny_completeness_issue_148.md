# Completeness Review: Issue #148 — Mermaid fence wrapping

## Acceptance Criteria vs. Implementation

### AC1: `koto template export template.md 2>/dev/null` outputs wrapped in ` ```mermaid ` fence on stdout

**Status: COVERED.**

`src/cli/mod.rs:992-995` — the `ExportFormat::Mermaid` arm now unconditionally wraps:
```rust
let raw = crate::export::to_mermaid(&compiled);
format!("```mermaid\n{}```\n", raw).into_bytes()
```

The stdout path (`else` branch at line 1044-1046) writes those bytes directly to stdout with no further gating.

Test coverage: `export_cli_outputs_mermaid_to_stdout` (line 4137) asserts `starts_with("```mermaid\n")`, `ends_with("```\n")`, and content markers.

### AC2: `koto template export template.md --output file.md` still wraps

**Status: COVERED.**

The `--output` path at line 1034-1043 calls `std::fs::write(output_path, &output_bytes)` where `output_bytes` is already the fence-wrapped bytes produced unconditionally at line 994. No separate branch exists for the `--output` case.

Test coverage: `export_cli_writes_to_output_file` (line 4168) asserts `content.starts_with("```mermaid\n")` and `content.ends_with("```\n")` on the written file.

### AC3: No new `--wrap` flag needed

**Status: COVERED.**

`ExportArgs` struct (lines 328-348) has no `wrap` field. No flag was added.

## Other Tests Named in the PR Description

The issue description mentions two test updates by name:

- **`export_cli_outputs_mermaid_to_stdout`** — found at line 4137. Assertion matches the claimed diff: `starts_with("```mermaid\n")`, closing fence check, and content `contains` assertions. Matches.
- **`export_30_state_template_latency_under_500ms`** — found at line 5040. Asserts `starts_with("```mermaid\n")`, `ends_with("```\n")`, `contains("[*] --> s0")`, `contains("s29 --> [*]")`. Matches the claimed update pattern.

## Gaps

None. All three acceptance criteria have matching implementation and test coverage. Evidence claims in the PR description are verifiable from the current source.
