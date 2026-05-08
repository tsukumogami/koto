<!-- decision:start id="gate-history-rendering" status="confirmed" -->
### Decision: Gate context in the History tab for GateEvaluated events

**Context**

The koto dashboard's History tab (PRD requirement R5) renders a chronological event
timeline for a selected session. The current R5 specification renders `GateEvaluated`
events with: gate name, result (PASS/FAIL), and command. This is insufficient for the
primary monitoring use case: an operator sees "gate `evidence-complete` FAIL" but has no
way to understand why without leaving the TUI to run `koto query`.

Gate definitions live in the compiled template (a JSON file in `~/.cache/koto/<hash>.json`),
which the dashboard already loads for the Remaining tab (R6). The `Gate` struct carries
all the information that distinguishes gate types: `gate_type`, `command`, `key`,
`pattern`, `completion`, and `name_filter`. The `GateEvaluated` event payload carries
`output` (a JSON blob with gate-type-specific fields like `exit_code`, `exists`, `matches`,
or children counts), `outcome`, and `timestamp`. Gate types in the current engine are:
`command`, `context-exists`, `context-matches`, and `children-complete`.

The detail pane will be 60% of terminal width (≥48 columns on an 80-column terminal)
with a scrollable History tab per R5. Multiline rendering per gate event is acceptable
because the tab is already scrollable.

**Assumptions**

- The compiled template is available in `~/.cache/koto/<hash>.json` for the vast majority
  of local sessions. The "template unavailable" fallback already established by R6 is
  the accepted degradation path.
- Operators do not need to evaluate gate conditions against current evidence at render time;
  the event log already records the outcome. Re-evaluation at render time would be
  expensive and inconsistent with what actually happened.
- The 48-column minimum width is wide enough for a two-line gate rendering (name/result on
  one line, condition on the next) without truncation that destroys readability.

**Chosen: Add gate type and condition text inline (one additional line from the compiled template)**

For each `GateEvaluated` event in the History tab, render two lines:

```
[YYYY-MM-DD HH:MM:SS] Gate: <name>  PASS|FAIL
  <gate-type>: <condition-summary>
```

Where `<condition-summary>` is derived from the gate definition in the compiled template
and the gate-type-specific output from the event log:

| Gate type | Condition line | Source |
|---|---|---|
| `command` | `cmd: <command>` | template `gate.command` |
| `context-exists` | `key: <key>` | template `gate.key` |
| `context-matches` | `key: <key>  pattern: <pattern>` | template `gate.key`, `gate.pattern` |
| `children-complete` | `children: <completed>/<total> complete` | event `output.completed`, `output.total` |

When the compiled template is unavailable, the condition line is omitted entirely and
only the first line (name + result) is shown — identical to the current baseline. No
error text is inserted; the fallback is silent omission.

**Rationale**

The core operator question when seeing a gate failure is "what did it check?" — not
"what was the full current state of the evidence store?" The gate type and condition
fields answer this question directly and are static (they come from the template
definition, not from runtime evaluation). This avoids re-evaluation at render time.

The two-line format keeps the History tab dense enough to remain useful as a timeline.
A single additional indented line per gate event adds meaningful context without
consuming disproportionate vertical space: a typical History with 10 events remains
viewable without excessive scrolling, whereas a collapsible section or a full definition
block would make the timeline harder to scan.

Rendering condition context only on failure (option 5) was considered but rejected:
pass context is equally useful when tracing why a workflow progressed through a
particular sequence. An operator reconstructing a session's history needs to see what
each gate verified, not just where it broke.

A collapsible section (option 4) was rejected because ratatui collapsible sections add
implementation complexity (key binding, focus tracking per-event) disproportionate to
the benefit. The detail pane already scrolls; adding a second navigation mode within it
creates a confusing interaction model.

Re-evaluating the condition against evidence at render time (option 3) was rejected:
it produces the current result, not the result at the time the event was recorded, which
can mislead operators investigating historical failures. It also requires running shell
commands or context lookups at render time, violating the principle that the History tab
is a read-only replay of what already happened.

**Alternatives Considered**

- **No change — name, result, command only (option 1)**: Rejected. Fails the stated
  monitoring use case: an operator sees "FAIL" but has no information about what the gate
  was checking. For context-exists and context-matches gates there is no command at all,
  so the current rendering produces a gate name and "FAIL" with no other content.

- **Re-evaluate condition against current evidence (option 3)**: Rejected. Produces
  present-tense results for past events, misleading operators investigating historical
  gate failures. Requires runtime execution (shell commands, context store reads) during
  rendering, which violates the read-only replay contract of the History tab and adds
  latency.

- **Collapsible section per event (option 4)**: Rejected. Adds a second navigation model
  within the History tab (expand/collapse per event row), complicating key binding design
  and the ratatui implementation. The benefit over a fixed two-line rendering is small
  because the tab already scrolls.

- **Show condition only on failure (option 5)**: Rejected. Pass context is equally
  useful when tracing workflow progression. Filtering by outcome forces operators to
  scroll selectively rather than reading the full narrative.

**Consequences**

- The History tab render function gains a new branch: when a `GateEvaluated` event is
  encountered, look up the gate definition in the compiled template (already loaded for R6)
  and emit a second indented line with the condition summary.
- `DetailData` or the History tab render path needs access to the compiled template's
  `states` map, not just the event log. The simplest approach is to pass the
  `CompiledTemplate` (already available in the dashboard state for R6) into the History
  tab renderer.
- The `children-complete` gate's condition line is sourced from the event output blob
  (not the template definition), so it reflects the count at evaluation time rather than
  the static filter configuration. This is more useful to operators than the filter spec.
- When the template is unavailable, the History tab still renders all events; gate events
  simply show one line instead of two. This is consistent with R6's existing fallback.
- No new I/O is introduced at render time. The template is read once on session focus
  change (same lifecycle as R6) and the History tab renderer consumes the already-loaded
  data.
<!-- decision:end -->
