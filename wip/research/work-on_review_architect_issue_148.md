# Architect Review: Issue #148 -- `koto template export --format mermaid` fence wrapping

## Change Summary

`koto template export --format mermaid` now wraps the raw mermaid diagram in a markdown fence
(` ```mermaid\n...\n``` `) before writing or printing. The wrapping is applied at line 994 of
`src/cli/mod.rs`, after calling `crate::export::to_mermaid()`.

## Layer Boundary Assessment

**Library function (`src/export/mermaid.rs` -- `to_mermaid`)**: returns raw mermaid text
(`stateDiagram-v2\n...`). It has no knowledge of output destination, rendering context, or
container format. This is correct: it is a pure transformation from `CompiledTemplate` to mermaid
syntax.

**CLI layer (`src/cli/mod.rs` line 991-997)**: computes `output_bytes` from the raw result, adding
the fence. The CLI is the right place for this decision -- it knows the output target (file or
stdout) and the intended consumer (a markdown document). The fence is a presentation concern, not a
diagram-content concern.

The boundary is intact. The change does not push presentation logic into the library, and it does
not pull template compilation into the CLI.

## Consistency Check: `--check` path

The `--check` branch at line 999-1031 calls `check_freshness(&output_bytes, path)` using the same
`output_bytes` that already contains the fence. This is correct: freshness is determined by
comparing what the CLI would write against what is on disk. If an existing file was generated
without the fence, `--check` will correctly report it as stale after this change. That is the
intended behavior for a format change.

## Dependency Direction

`src/export/mermaid.rs` imports only `crate::template::types::CompiledTemplate`. It does not import
`src/cli`. The wrapping code in `src/cli/mod.rs` imports `crate::export::to_mermaid`. Direction:
cli -> export -> template. This is correct -- no inversion.

## Parallel Pattern Check

`generate_html` (the other export format) returns `Vec<u8>` directly from the library and is used
as-is at line 996. Mermaid now returns `String` (raw diagram) and the CLI wraps it. These are not
parallel patterns at risk of diverging: HTML is a self-contained format that needs no CLI-level
wrapper; mermaid is a diagram language that requires a container when embedded in markdown. The
asymmetry is intentional and justified.

If a future format (e.g., SVG) needed similar wrapping, the correct place would also be the CLI
match arm, consistent with this change.

## Findings

**Blocking:** 0

**Advisory:** 0

The implementation fits the existing architecture. The fence wrapping belongs in the CLI layer, the
library function remains a pure transformation, dependency direction is correct, and the `--check`
freshness comparison is consistent with what the CLI writes.
