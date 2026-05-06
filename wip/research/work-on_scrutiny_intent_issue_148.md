# Intent Review: Issue #148 -- `koto template export --format mermaid` markdown fence wrapping

## Issue intent

Issue #148 asks that `koto template export > file.md` produce a file that renders correctly in markdown viewers. The described mechanism is wrapping stdout in a ```` ```mermaid\n ... ``` ```` fence.

## What the change does

`src/cli/mod.rs` lines 991-995: the mermaid branch unconditionally wraps in a markdown fence before writing to `output_bytes`. That byte slice is then either written to disk (when `--output` is given) or sent to stdout. The previous code produced the fence only when `--output` was present; stdout received raw mermaid text.

## Does the change match the issue intent?

Yes. The issue's stated goal is that piping to a file produces markdown-renderable output. The change achieves exactly that -- `koto template export template.md > docs/diagram.md` now produces a fenced block that any markdown renderer will display.

The `--check` path is unaffected: it still compares `output_bytes` (which now includes the fence) against the on-disk file, so a file written before this change will be flagged as stale on the next `--check` run. That is correct behavior -- the content contract changed.

## Pipe-to-tool composability concern

The original code comment cited "Raw mermaid text for stdout composability." That was a real trade-off: tools like `mmdc` (Mermaid CLI) accept raw mermaid syntax, not a markdown fence. After this change, a user who pipes stdout to `mmdc` will get an error because `mmdc` does not expect the backtick wrapper.

There is no escape hatch in the current `ExportArgs` struct. The flags are: `--format`, `--output`, `--open`, `--check`. No `--raw` flag exists. Users who want raw mermaid for tool consumption have no supported path after this change.

**This is an advisory finding, not blocking.** The issue's stated intent is markdown output, and the change delivers it. However, the breakage of the piping use case is silent -- no flag was added to recover raw output, and the help text gives no indication that stdout is now fenced. A `--raw` flag (or `--no-fence`) would preserve composability without contradicting the issue goal.

## Test coverage

Both updated tests (`export_cli_outputs_mermaid_to_stdout` at line 4137 and the 30-state performance test at line 5065) assert `starts_with("```mermaid\n")` and `ends_with("```\n")`. They correctly encode the new contract.

Neither test covers the `--output` path for mermaid to confirm the fence also appears in the file -- but this is an omission in coverage, not a structural problem with the implementation.

## Summary

| Finding | Level |
|---------|-------|
| The change matches the issue's intent (markdown-renderable output on stdout) | No issue |
| No escape hatch for raw mermaid piping to tools like `mmdc` | Advisory |
| Test coverage correctly reflects the new contract | No issue |

**Blocking: 0. Advisory: 1.**

The advisory: the always-wrap behavior silently removes the raw-mermaid-on-stdout composability path with no flag to recover it. Adding `--raw` / `--no-fence` before shipping would close this gap without undermining the issue goal.
