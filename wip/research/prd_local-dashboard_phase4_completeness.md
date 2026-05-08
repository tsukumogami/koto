# Completeness Review

## Verdict: FAIL
The PRD covers the core happy path well but has six meaningful gaps — two of which would force an implementer to guess at behavior, and one that omits an AC for a stated requirement.

## Issues Found

1. **R3 hierarchy definition is ambiguous for real-world session graphs**: R3 defines Level 1 as "no `parent_workflow`" and Level 2 as "have children but no `parent_workflow`". These two rules overlap — a root session can also have children without being a batch coordinator. The PRD doesn't define what distinguishes a "root session" from a "batch coordinator session" in the session data. An implementer cannot determine from R3 alone whether a plain root session with a single child session is a Level 1 or Level 2 entry. Fix: define the discriminator explicitly (e.g., a session is a batch coordinator if it has more than one direct child, or if a specific header field is set).

2. **R5 aggregate counts reference status fields not defined in the PRD**: R5 lists six child status values (`success`, `failed`, `skipped`, `pending`, `blocked`, `spawn_failed`) but the PRD never specifies how the dashboard derives those statuses. R4 defines status only for the currently displayed session row (`running`, `done`, `failed`, `blocked`) — a different and smaller set. An implementer reading only this document cannot know where `skipped` or `spawn_failed` come from, or how child sessions map to these six values. Fix: add a mapping table (child session event state → aggregate status bucket) or point to the F2 data contract that defines these fields.

3. **R6 collapse threshold has no AC**: R6 specifies that sessions with more than 5 children are collapsed by default and those with 5 or fewer are expanded. The acceptance criteria list does not include a test for the ≤5 auto-expand case — only the >5 collapsed case is covered ("A batch coordinator with >5 children shows an aggregate row by default"). This leaves the auto-expand path untested. Fix: add an AC like "A batch coordinator with ≤5 children shows its children expanded by default without pressing Enter."

4. **`--once` output format is not specified**: R13 and the corresponding AC say `--once` outputs "current dashboard state as plain text," but the format is entirely unspecified. An implementer must guess whether this is a line-per-session tabular format, JSON, or something else. The scope document called this out for scripting use — consumers will write code against this output. A format change after V1 would be a breaking change. Fix: specify the exact output format for `--once` (columns, delimiter, ordering) or explicitly state it is a plain-text approximation of the TUI layout with no stability guarantee.

5. **No requirement for minimum terminal dimensions or resize handling**: The scope document mentioned an 80×24 standard terminal as a design constraint. The PRD does not specify what happens when the terminal is narrower or shorter than needed to render the full layout. ratatui can crash or produce garbled output if widgets are given zero-sized areas. Fix: add a requirement specifying minimum supported terminal dimensions and behavior below that threshold (e.g., show a "terminal too small" message and exit cleanly, or truncate gracefully).

6. **No AC for R12 poll interval header**: R12 specifies "A header row shows the time since the last successful poll refresh," but no acceptance criterion verifies this header is present or updates correctly. Fix: add an AC that validates the last-refresh timestamp is displayed and updates each poll cycle.

7. **Exit code for `--once` with a missing session name is undefined**: The AC states "`koto dashboard nonexistent-name` exits with code 1 and an error message" — but only in normal TUI mode. The behavior of `koto dashboard nonexistent-name --once` is not specified. Fix: explicitly state the exit code and error output for `--once` with an invalid session name, or confirm it follows the same rule as TUI mode.

## Suggested Improvements

1. **Add a session status state machine or decision table**: R4 defines four status values but the logic for determining `failed` vs `blocked` vs `running` is scattered (terminal detection in R10, epoch-branch filtering in R11, child aggregation in R5). A single table that maps session state + template terminal flag + child states → display status would remove ambiguity and make implementation deterministic.

2. **Specify scrolling behavior for long child lists**: R14 defines cursor navigation keys but does not describe viewport scrolling. With 1000 children, a user pressing `j` will reach the bottom of the visible area. The PRD should clarify whether the list scrolls (and if so, with a scrollbar indicator) or pages. This is particularly important for the primary use case (1000-task batch) described in UC-1.

3. **Define the column layout or truncation rules**: R4 specifies what each row displays but not the column widths, truncation strategy, or what happens when session names are very long (koto session names can include path segments). Without this, two implementers will produce different UIs. Even a rough note ("session name truncated with `…` at 40 characters in the repo-wide view") removes the guesswork.

4. **Clarify whether `koto dashboard` must be run from within a git repository**: R1 says repo-id is derived from the current working directory "same derivation as `koto init`." It does not say what happens if the user runs `koto dashboard` from a directory that is not a koto-managed repository or has no sessions. An error message and exit code should be specified for this case.

## Summary

The PRD is thorough on the high-level feature shape, the decisions section is unusually strong, and the known limitations section is honest. The gaps are mostly in the detail layer: status field provenance, output format stability, and missing ACs for specified behaviors. None of the issues would require rethinking the architecture, but issues 1, 2, and 4 in particular would force implementers to make undocumented choices that could produce inconsistent behavior or break downstream consumers.
