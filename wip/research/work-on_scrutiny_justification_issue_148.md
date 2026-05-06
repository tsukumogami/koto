# Justification Review: Issue #148 — mermaid fence default

## Finding 1: No blocking deviations — "always wrap" is the correct call

The issue offered two options: add a `--wrap` flag or make fenced output the default. The implementation chose "always wrap" unconditionally. The user confirmed this intent. No flag was left dead; no speculative parameter was added. This is the simpler choice.

The trade-off is sound: the only caller who could object is a script parsing raw mermaid syntax from stdout. Adding a `--no-wrap` flag for that hypothetical caller would be speculative generality (heuristic #2). The current implementation avoids it.

## Finding 2: Right layer for the change

`to_mermaid()` returns diagram content without wrapping. The fence is presentation — it belongs in the CLI output layer, not in the library function. Keeping the fence in the CLI arm at `src/cli/mod.rs:994` is correct: it means `to_mermaid()` stays composable (callers who embed the output in their own fence or parse it programmatically are not forced to strip the wrapper).

The implementation at line 993–994:
```rust
let raw = crate::export::to_mermaid(&compiled);
format!("```mermaid\n{}```\n", raw).into_bytes()
```

This is the minimal, correct change. The fence is applied once, uniformly, at the point where output is materialized — whether writing to a file (`--output`) or stdout.

## Finding 3: `--check` path is consistent

The `--check` path compares `output_bytes` against the file on disk (line 1003). Since `output_bytes` now always includes the fence for mermaid, existing checked files that were written without the fence will be reported as stale. This is the correct behavior — the change is a format change and stale detection should catch it.

No inconsistency between the check path and the write path.

## Finding 4: Tests in mermaid.rs do not assert fence — no problem

Unit tests in `src/export/mermaid.rs` test `to_mermaid()` directly and correctly assert no fence (e.g., `output.starts_with("stateDiagram-v2\n")`). These tests remain valid. The fence is a CLI concern; it should not be in library tests.

If there are no CLI-level integration tests for the export command, that's a test coverage gap — but not a justification problem, and out of scope for this review.

## Summary

No blocking findings. The "always wrap" decision avoids a speculative `--no-wrap` flag, is consistent across stdout and file paths, and is applied at the correct layer (CLI, not library). The deviation from "add a flag" is explicitly justified by the user's preference and produces simpler code.
