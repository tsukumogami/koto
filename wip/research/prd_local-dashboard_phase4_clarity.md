# Clarity Review

## Verdict: PASS
The PRD is specific enough that two developers would converge on the same design, but six ambiguities would produce divergent implementation details worth resolving before work starts.

## Ambiguities Found

1. **R3 — "Level 2: Batch coordinator sessions (have children but no `parent_workflow`)"**: The definition contradicts itself. A batch coordinator that was spawned by a root orchestrator would have a `parent_workflow` field set, yet it is still a coordinator (Level 2) with its own children. The parenthetical condition silently collapses these cases: a session can be both a child (has `parent_workflow`) and a coordinator (has children). The hierarchy description implies exactly 3 levels, but real pipelines like explore → prd → design → plan → work-on produce 3+ levels of nesting, all of which collapse to flat rendering. Clarify the definition: "Level 2 is any session that has child sessions, regardless of whether it has a `parent_workflow`; Level 3 is any session without children that has a `parent_workflow`."

2. **R5 — "pending" catch-all in the aggregate**: The aggregate row uses the status values `success`, `failed`, `skipped`, `pending`, `blocked`, `spawn_failed`, but the display formula shows only `done · failed · pending`. There is no mention of where `blocked`, `skipped`, and `spawn_failed` appear in the rendered aggregate. Do `blocked` sessions count toward `pending`? Does `skipped` count toward `done`? Do `spawn_failed` sessions count toward `failed` or get their own bucket? A developer reading this would make different mapping choices. Specify the exact status-to-bucket mapping (e.g., "done = success + skipped; failed = failed + spawn_failed; pending = pending + blocked").

3. **R6 — "more than 5 children" threshold source**: The rule is that >5 children collapses by default. It is not stated whether this threshold applies at the time the dashboard opens (based on current child count) or is re-evaluated on each poll cycle. If a batch job starts with 3 children (expanded) and grows to 200 during a session, should the view auto-collapse mid-session? Clarify: "The threshold is evaluated once at initial render. Subsequent additions to the child list do not trigger automatic collapse."

4. **R13 — `--once` output format is unspecified**: R13 says `--once` outputs "the current dashboard state to the terminal as plain text." It does not define what "plain text" means: is it the same rows the TUI renders without color codes? Is it JSON? Is it the same format `koto workflows` uses? The acceptance criterion only checks exit code 0 and says nothing about the format. A developer building this would make an arbitrary choice, and a reviewer could not objectively verify it. Specify the format: "The `--once` output renders each session row as a single line with tab-separated fields, no ANSI color codes, in the same order as the TUI list."

5. **R9 — "truncated preview of the evidence content (first 80 characters)"**: "First 80 characters" is ambiguous when evidence content contains newlines or multibyte Unicode characters. Does "80 characters" mean 80 bytes, 80 Unicode codepoints, or 80 display columns? Does the preview include the raw text (including embedded newlines that would break the layout) or is it collapsed to a single line? Clarify: "First 80 Unicode codepoints of the evidence string, with newlines replaced by spaces, and an ellipsis appended when truncated."

6. **R7 — "Running / pending" as a single sort group**: R7 lists "Running / pending" as one bucket and R5 treats them as separate statuses. It is unclear whether a `running` session and a `pending` session sort identically within that group (both deferred to name sort) or whether `running` outranks `pending` within the group. Also, `blocked` appears in the sort order at position 2, but R5 lists `blocked` as a distinct status value that R5's aggregate formula doesn't account for (see ambiguity #2 above). Clarify whether `running` and `pending` are a single equivalent bucket or whether `running` ranks above `pending`.

## Suggested Improvements

1. **Add a concrete status mapping table to R5**: Replace the prose description of the aggregate formula with a two-column table: status value → display bucket. This removes all guesswork about `skipped`, `blocked`, and `spawn_failed` and is directly testable.

2. **Add an example `--once` output block to R13**: Show a 3-line example of what `koto dashboard --once` writes to stdout. This makes the acceptance criterion binary: does the output match this format? Without it, the criterion "outputs current state as plain text" cannot be objectively verified.

3. **Clarify the hierarchy definition in R3 with a worked example**: Add a small ASCII example showing the exact session graph (which fields are set) that produces each level assignment. The current parenthetical conditions are not sufficient to handle the nested-coordinator case that the problem statement itself describes (explore → prd → design → plan → work-on).

4. **Strengthen the acceptance criterion for R16/R17**: "≤1 second" and "≤100ms" are measurable, but the criteria do not say how to measure them (wall clock from command invocation to first render, or from TUI open to first frame?). Specify the measurement point: "from the moment the user presses Enter to the moment the first frame is painted."

5. **Resolve the auto-collapse behavior under R6**: The Known Limitations section notes that 1000-sibling polling "may skip the current poll cycle," but does not address UI state changes (collapse/expand) triggered by count crossing the threshold during a session. Make the collapse rule static or dynamic, and document it.

## Summary

The PRD is well-structured and specific in most areas. The problem statement is sharp, the user stories are concrete enough to derive test cases from, and most acceptance criteria are binary. The main gaps cluster around the aggregate row formula (R5), the `--once` output format (R13), and the hierarchy level definition edge case in R3. Fixing these six points would make the PRD unambiguous enough to hand to any developer and get a consistent implementation.
