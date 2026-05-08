# Lead: User Information Needs from a Workflow Dashboard

## Findings

### The three mental models form a clear hierarchy

User interactions with a workflow dashboard fall into three distinct modes that differ not just in depth but in urgency and cognitive load. Mapping each mode to what koto's data model actually provides reveals where the current MVP design succeeds and where it falls short.

**Quick-check mode (seconds of attention)**

Primary question: *Is my workflow doing something, stuck, or done?*

The user wants a traffic-light answer without context-switching. The data that satisfies this is narrow: session name, status bucket (running / blocked / failed / done), and elapsed time. The current `--once` mode delivers exactly this for scripting. The TUI list view delivers it for humans. Both are correct for quick-check.

What quick-check does not need: gate details, evidence content, transition history, or child breakdowns. These actively harm quick-check by adding visual noise. The current design correctly collapses these behind Enter / detail pane.

A significant gap: the current list view surfaces state *name* (e.g., `implementing`) but not what that state *means* in the workflow's progress. A state named `implementing` tells the user where koto is, but not where that is relative to completion. The "N tasks · X done" aggregate is good for batch coordinators, but for linear workflows there's nothing equivalent — no sense of "step 3 of 7."

**Status review mode (minutes of attention)**

Primary question: *What is the workflow doing right now, and is it on track?*

The user has noticed something worth examining — slower than expected, a notification that drew their eye, or they're checking in before a meeting. They want to understand what state the session is in, what the agent submitted most recently, and whether any gate is blocked.

The data that satisfies this: current state name + directive text (what the state asks the agent to do), the most recently submitted evidence fields (what the agent claims to have done), and the current gate status (passing / failing / never evaluated). The current design exposes gate status in the detail panel (7 rows), but it does not surface the state's directive text at all. The directive text is present in the compiled template (`TemplateState.directive`) and would answer "what should be happening here?" without requiring the user to read the template source.

Evidence submitted in the current epoch is the core "proof of work" artifact. The current design shows 2–3 evidence entries in the detail panel, newest-first. This is correct for status review. The truncated preview (first 80 codepoints) is the right default — full evidence values can be large and would not fit in a 7-row panel.

The `created_at` field from `StateFileHeader` is the session creation timestamp. Elapsed is computed from it. But the elapsed display in the current design is the total elapsed since session creation, not elapsed since the last state transition. For long-running workflows, total elapsed is nearly meaningless for assessing "is this stuck?" — what matters is elapsed *in the current state*. The timestamp of the most recent state-changing event (the epoch boundary event's `timestamp` field) is already in the JSONL log and could drive a "time in current state" display.

**Deep investigation mode (tens of minutes)**

Primary question: *How did the workflow get to this state, and what exactly happened at each step?*

The user is diagnosing a failure, reviewing a completed run, or trying to understand an unexpected outcome. They need the full event history in chronological order: every transition, the evidence submitted at each state, every gate evaluation result, any directed transitions and their rationales, any rewound events and their rationales.

The data that satisfies this is the complete JSONL event log read in seq order. `koto query` provides this today as raw JSON. The dashboard's focused view provides a shallow subset — last gate result, last 3 evidence entries. For true deep investigation, the dashboard has no mechanism to show the full timeline.

A critical gap: the current design has no way to answer "what did the agent do at state X two steps ago?" The evidence is epoch-scoped, meaning evidence from prior states is not surfaced at all in the current focused view. A user investigating a completed workflow — or a workflow that was rewound — has no way to see the prior-epoch evidence through the dashboard.

The compiled template exposes the full state graph (`states`, `transitions`, `gates`, `terminal`). This means "what states are remaining" is answerable from the template plus the current state: walk the DAG forward from the current state to find all reachable non-terminal states. This is not currently surfaced in any form.

### Persona-specific observations

**Developer who just kicked off a workflow**

Wants instant confirmation the workflow started and is in the right initial state. The `--once` output satisfies this. The TUI would also satisfy it. Current gap: no "just started" visual distinction — a brand-new session looks identical to a running one that's been active for hours.

**Developer whose workflow is stuck**

Urgently needs to know: which gate is blocking, what the gate last returned, and how long the session has been in the current state. The current design surfaces gate type, result, and elapsed since gate evaluation. What it does not surface: the full gate command that ran (only if it's a command gate and only if the gate output has a `command` field), the full stderr/stdout from the gate command, and how long the session has been *in the current state* (vs. total elapsed). For a `command` gate, the exit code is in `gate_evaluated.output`; the full stdout/stderr is in `default_action_executed` events, not in the gate payload. These are distinct events.

**Developer reviewing a completed workflow**

Wants a retrospective: what states were visited, what evidence was produced, was anything rewound or overridden. The current design shows nothing useful for completed workflows — the detail panel shows the *last* gate evaluation, which for a terminal session may be in a long-past state. The evidence timeline stops at the current epoch. A completed workflow retrospective requires the full event log, which the dashboard does not expose.

**Developer managing 10+ concurrent workflows**

Needs density and sorting. The list view provides density. The failure-first child sorting is correct. What's missing: filtering (e.g., "show only failed or blocked") and a cross-session aggregate ("N workflows running, M blocked, K done"). The PRD's R5 aggregate counts work at the coordinator level but there's no repo-wide aggregate in the header row.

**Developer navigating a composable workflow tree**

The parent-child hierarchy via `parent_workflow` is the foundation. The tree view up to 3 levels is correct for the immediate near-term. The future shirabe pipeline (explore → prd → design → plan → work-on) would be a 2–3 level tree at minimum. The key information gap at this level: there's no way to see *which phase* a root orchestrator is currently coordinating, because the root's own state name ("phase-2" or "spawning") is abstracted away from the child work it's waiting on. The meaningful question is "what is this pipeline actually doing" — which requires correlating the root's current state with the active child sessions' states.

### What data is available vs. what's surfaced

| Information | Available in JSONL | Surfaced in current dashboard |
|---|---|---|
| Current state name | Yes (derived from last transition event) | Yes |
| Status bucket (running/blocked/failed/done) | Derivable | Yes |
| Elapsed since session start | Yes (header `created_at`) | Yes |
| Elapsed in current state | Yes (epoch boundary event `timestamp`) | No |
| Last gate outcome (pass/fail) | Yes (`gate_evaluated` event) | Yes |
| Gate command string | Depends on gate output structure | Partially (if `command` field exists) |
| Gate exit code / error message | Yes (in `gate_evaluated.output`) | No |
| Gate stdout/stderr | Yes (in `default_action_executed`) | No |
| Current state directive text | Yes (compiled template) | No |
| Evidence submitted (current epoch) | Yes | Partially (3 entries, truncated) |
| Evidence submitted (prior epochs) | Yes (in JSONL, before epoch boundary) | No |
| Full transition history | Yes | No |
| Rewind history and rationales | Yes (`rewound` events) | No |
| Directed transition rationales | Yes (`directed_transition.rationale`) | No |
| Gate override records | Yes (`gate_override_recorded`) | No |
| States remaining (future path) | Derivable from compiled template | No |
| Progress fraction (step N of M) | Derivable from template graph | No |
| Batch child breakdown by status | Derivable from child sessions | Yes (aggregate row) |
| Variables bound at init | Yes (`workflow_initialized.variables`) | No |

### The "what's remaining" problem

The compiled template exposes every state, its transitions, and which states are terminal. Given the current state, it's possible to compute the set of reachable terminal states and the minimum number of transitions needed to reach each. This is a directed graph reachability problem. For linear workflows (no branching), this reduces to "steps remaining." For branching workflows, the reachable terminal states and their associated paths are the meaningful unit.

The current dashboard does not load the compiled template except to check `is_terminal`. The template is at `derive_machine_state` → `template_path` → the compiled JSON in koto's cache. It's already being read for terminal detection on every poll cycle. Exposing state count or progress fraction would add no I/O cost.

A linear "3 of 7 states complete" fraction would satisfy most quick-check needs better than the current state-name display for users who don't know the template structure by heart.

### Information density mismatch in the current design

The current design conflates two different detail levels into a single 7-row panel: the gate detail view (investigation) and the evidence preview (status review). These serve different purposes and have different audiences. A developer doing status review wants to see the evidence summary; a developer investigating a stuck workflow wants gate details. Mixing them in a fixed 7-row panel forces a trade-off that serves neither well.

The current `render_detail` function shows: gate type, command (if command gate), result (PASS/FAIL), elapsed, and 2–3 evidence entries. This is acceptable as a minimum but misses the exit code, error message, and the actual content of complex evidence values (since they're truncated at 80 chars).

### The "no workflow_completed event" gap

As documented in the session-feed contract, there is no `workflow_completed` event. Terminal detection requires loading the compiled template. This means:

1. If the compiled template is deleted or moved after session creation, the session shows `unknown` forever — even if it reached a terminal state. The dashboard user has no way to know whether this means the session is genuinely unknown or just has a broken template reference.

2. A dashboard user looking at a "completed" run cannot see *when* the workflow completed from the event log alone — they'd have to look at the timestamp of the final transition event, which is not surfaced in the current design.

3. Cross-machine or archived sessions (where the compiled template may not be present) cannot be correctly classified. This is a data-layer limitation, not a UI one, but it affects what information the dashboard can reliably provide.

---

## Implications

**Add "time in current state" alongside total elapsed.** The epoch boundary event's timestamp is available with no additional I/O. "Time in current state" is the most actionable staleness signal — it directly answers "is this stuck?" Displaying both (e.g., `45m / 2h 10m total`) takes no more space than the current single elapsed column.

**Surface the current state's directive text in the detail panel.** This is the single highest-value addition for status review. The compiled template is already loaded for terminal detection. Adding a directive line to the detail panel answers "what is the workflow supposed to be doing here?" without the user needing to find and read the template file.

**The information hierarchy should have three layers, not two.** Current design: list view (quick-check) and detail panel (everything else). Better: list view (quick-check), focused session summary (status review — current state, directive, last evidence, gate status, time in state), full event log viewer (deep investigation). The 7-row detail panel is too small for status review and too shallow for deep investigation.

**Progress fraction is achievable and high-value.** Reading the compiled template state count and the current state position in the graph (trivially: states visited count / total state count for linear workflows, or "N states remain" for branching ones) would make the list view significantly more informative. No new I/O is required — the template is already loaded.

**"What's remaining" requires template coupling.** For linear workflows, "N states remain" is a simple subtraction from the template's state list. For branching workflows (workflows with `when` conditions on transitions), the remaining states depend on which branch is taken. Surfacing this accurately requires either showing the full remaining graph or simplifying to "reachable terminal states: X." A "minimum N steps to completion" would be the simplest useful display.

**Evidence from prior epochs is invisible.** A completed workflow retrospective is impossible with the current panel design. A "full history" mode that scrolls through all `evidence_submitted` events in seq order — grouped by epoch — would serve the deep investigation use case without breaking the current quick-check and status review designs.

**The aggregate row for batch coordinators (R5) is the right design.** But there's no equivalent for the repo-wide view. A header row showing "N sessions · X running · Y blocked · Z failed" would provide quick-check value at the top level without changing the list layout.

**Gate override history is invisible.** `gate_override_recorded` events are Tier 1 (they document when a gate was bypassed). A user reviewing a completed workflow that had overrides needs to know about them — an override on a CI gate changes the meaning of the outcome. This should appear in any retrospective or deep investigation view.

**The `directed_transition.rationale` field is high-value and invisible.** When an agent or user manually overrides the normal transition path (`koto next --to <state> --rationale "..."`) the rationale is preserved in the event log. This is exactly the kind of context a user investigating "why did this workflow skip step X?" needs. It's not surfaced anywhere in the current design.

---

## Surprises

**The compiled template must be loaded for basic terminal detection.** This wasn't obvious from the surface-level design. The absence of a `workflow_completed` event means every consumer — dashboard, status command, any future relay — must load the compiled template to determine whether a session is done. This creates a tight coupling between session state and the local template cache that breaks in cross-machine scenarios. It also means the compiled template is being loaded on every poll cycle for every session, which could add up at scale.

**Evidence is epoch-scoped in a way that hides prior work.** Evidence from states the workflow has already transitioned through is not "archived" in any accessible way in the current design — it's just earlier in the JSONL file, before the epoch boundary. For a user reviewing a completed multi-phase workflow, all the evidence from prior states is invisible unless they read the raw JSONL. The epoch-scoping design was correct for the engine (clean slate on each state arrival) but creates a UI gap for retrospective review.

**The `default_action_executed` event contains gate-adjacent information but is Tier 2.** When a state has a `default_action` (automatic shell command on entry), the stdout/stderr and exit code are recorded in `default_action_executed`. But this is classified Tier 2 (optional display). For a user investigating a failed workflow, this is the most actionable information — it's the actual command output that explains the failure. Classifying it as optional means the MVP dashboard may miss the most useful failure diagnostic.

**`decision_recorded` events carry structured agent decisions.** The `koto decisions record` command lets agents record structured choices mid-state. This appears in `decision_recorded` events (Tier 2). These decisions are a form of evidence-light: they explain *why* the agent did something, not just *what* it did. For retrospective review, they're highly valuable. The current dashboard design doesn't consider them at all.

**Rewind creates invisible branching history.** After a rewind, the archived epoch branch (session named with `~`) is hidden from the main list. The focused parent view shows an "Archived epochs" summary row (count only, no expansion). A user who wants to understand "what happened before the rewind" has no dashboard path to this information. The archived epoch branches exist as separate JSONL files and could in principle be shown, but the current design explicitly defers this.

---

## Open Questions

**Should "time in current state" replace or augment total elapsed?** The total elapsed time is useful context (tells you how long the workflow has been running overall), but time in current state is more actionable (tells you if it's stuck). Both are derivable. The right default display is unclear without user testing.

**How should the detail panel handle completed (terminal) sessions?** The current design shows gate details for the last gate evaluation — but for a terminal session, that gate was in a state the workflow has already left. What a retrospective user wants to see is the final state's evidence, any overrides, the full transition history. The current 7-row panel cannot hold this, and the current design gives no indication that a different view would be appropriate for terminal sessions.

**What's the right affordance for accessing the full event log?** Deep investigation requires the full history. A `koto query --events <name>` command provides this today as raw JSON. Should the dashboard provide a viewer for this, or is the CLI fallback acceptable? If the dashboard is meant to be a complete observability surface, the answer is the former. If it's a monitoring tool that defers to CLI for investigation, the latter.

**How should "what's remaining" display for branching templates?** For workflows with conditional transitions (multiple `when` clauses on transitions from a state), the remaining path depends on evidence not yet submitted. Showing "steps remaining" is impossible without knowing which branch will be taken. Options: show minimum steps to any terminal state, show the graph of remaining states, or skip this for branching workflows and only show it for linear ones.

**How does the dashboard scale to the full shirabe pipeline?** The shirabe pipeline (explore → prd → design → plan → work-on) as a single navigable workflow tree would have a root orchestrator session, phase sessions at level 1, and potentially dozens of work-on sessions at level 2. At 3 visible indent levels, this fits. But the root orchestrator's own current state (e.g., "running-plan-phase") is opaque without knowing the template. The meaningful display for a multi-phase pipeline is phase progress, not the orchestrator's internal state name.

**Should `gate_override_recorded` events be surfaced in the list view?** A session with an active gate override is neither blocked nor simply running — it was blocked, and the override was recorded to unblock it. The current status bucket derivation (running / blocked / failed / done) doesn't account for this. A session with an unacknowledged override might need a distinct visual treatment (e.g., an override indicator in the status column).

**What is the right display for the `directed_transition.rationale` field?** When a manual transition override is recorded with a rationale, that rationale is high-value context for retrospective review. Should it appear in the detail panel, the full history view, or both? The current design surfaces none of it.

---

## Summary

Users need three distinct information layers from a workflow dashboard — quick-check (status bucket + elapsed), status review (current state directive + recent evidence + gate outcome + time in current state), and deep investigation (full event history + prior epoch evidence + rationales) — but the current dashboard collapses status review and investigation into a single 7-row panel that serves neither well. The most actionable gaps are: "time in current state" (derivable from the epoch boundary timestamp at no extra I/O cost), the current state's directive text (already loaded for terminal detection), and prior-epoch evidence visibility (requires scrolling the full event log). The biggest open question is whether the dashboard aims to be a complete observability surface (requiring a full event log viewer) or a monitoring tool that defers deep investigation to the CLI.
