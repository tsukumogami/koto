# Lead: koto Event Log Data Available for Dashboard Enrichment

## Findings

### EventPayload Variants — Field-by-Field Audit

**WorkflowInitialized**
- `template_path` (String): Path to the compiled template JSON. Currently used only indirectly via `derive_machine_state` to check `is_terminal`. The template can be parsed to enumerate all states and identify remaining/pending ones — this is the path to a "progress through states" view. **Not shown in dashboard.**
- `variables` (HashMap<String, String>): Variable bindings set at `koto init` time. Could show what parameterized the workflow. **Not shown.**
- `spawn_entry` (Option<SpawnEntrySnapshot>): For batch-spawned children, records the source template, var bindings, and `waits_on` list. Would let the detail pane show dependency chains. **Not shown.**

**Transitioned**
- `from` (Option<String>): Previous state. **Not shown.** Could power a state-transition history list in the detail pane.
- `to` (String): Current state. Shown in the list column.
- `condition_type` (String): What drove the transition (`"auto"`, `"gate"`, `"evidence"`). **Not shown.** Useful for explaining why the state changed.
- `skip_if_matched` (Option<BTreeMap<String, Value>>): Which `skip_if` conditions matched. **Not shown.**

**EvidenceSubmitted**
- `state` (String): Which state received evidence.
- `fields` (HashMap<String, Value>): The actual submitted key-value payload. Currently extracted in `read_detail()` as `EvidenceEntry.fields` and rendered as a JSON blob. Shown in the detail pane (up to 3 entries). However, the rendering is a raw JSON dump — field names and values are not pretty-printed. **Partially shown; could be much richer.**
- `submitter_cwd` (Option<PathBuf>): Process working directory at submission. **Not shown.** Could be shown for debugging path-resolution issues.

**IntegrationInvoked**
- `state` (String), `integration` (String), `output` (Value): Records when an integration (e.g., GitHub) was called and its result. **Not shown at all** — `read_detail()` has no branch for this variant, and `DetailData` has no field for it.

**DirectedTransition**
- `from` (String), `to` (String): Manual transition endpoints.
- `rationale` (Option<String>): Human reason for the skip. **Not shown.** The rationale field is the most human-readable data that could appear in the detail pane.

**Rewound**
- `from` (String), `to` (String): Rewind endpoints.
- `rationale` (Option<String>): Why the rewind happened. **Not shown.** Same as DirectedTransition — a natural fit for the detail pane's history view.

**ContextAdded**
- `key` (String): Context artifact name (e.g., `"plan.md"`).
- `hash` (String): SHA-256 digest.
- `size` (u64): Size in bytes. **Not shown at all.** Could show what context artifacts are attached to the session.

**WorkflowCancelled**
- `state` (String): State when cancelled.
- `reason` (String): Cancellation reason. **Not shown.** A cancelled workflow with a reason is important diagnostic information.

**DefaultActionExecuted**
- `state` (String), `command` (String), `exit_code` (i32), `stdout` (String), `stderr` (String): Full output of a default action command. **Not shown.** This is the richest diagnostic data for auto-advancing states — exit code and stderr especially.

**DecisionRecorded**
- `state` (String), `decision` (Value): Agent decisions captured during a state. **Not shown** in the dashboard. `derive_decisions()` exists in persistence.rs but is never called from dashboard_data.rs or dashboard_render.rs.

**GateEvaluated**
- `state`, `gate`, `output` (Value), `outcome` (String), `timestamp` (String). Partially shown: gate name, outcome (as PASS/FAIL), elapsed since timestamp, command (extracted from output). The `output` JSON blob could expose additional gate-specific fields (e.g., structured error messages).

**GateOverrideRecorded**
- `state`, `gate`, `rationale` (String), `override_applied` (Value), `actual_output` (Value), `timestamp`. **Not shown.** `derive_overrides()` exists but is not called from the dashboard. Overrides are a significant audit event — the rationale field is directly human-readable.

**SchedulerRan**
- `state` (String), `tick_summary` (SchedulerTickSummary), `timestamp`. `SchedulerTickSummary` has `spawned_count`, `errored_count`, `skipped_count`, `reclassified`. **Not shown.** Useful for batch parent sessions to show how many children were spawned per tick.

**BatchFinalized**
- `state`, `view` (Value): Frozen snapshot of `children-complete` gate output. The `view` JSON encodes `total`, `completed`, `pending`, `success`, `failed`, `skipped`, `blocked`, `spawn_failed`, `all_complete`, `all_success`, `any_failed`, `any_skipped`, `children` array. **Not shown** — `task_counts_for_root()` in dashboard_state.rs recomputes these counts live from CachedSession fields rather than reading the BatchFinalized event. The `BatchFinalized` event is more authoritative for cleaned-up children.

**ChildCompleted**
- `child_name` (String), `task_name` (String), `outcome` (TerminalOutcome: success/failure/skipped), `final_state` (String). **Not shown.** Persisted specifically so the parent can account for cleaned-up children; the dashboard does not use it.

---

### What `CachedSession` Currently Holds

`CachedSession` stores:
- `header` (StateFileHeader): Full header including `created_at`, `parent_workflow`, `session_id`, `template_source_dir`
- `current_state` (Option<String>): Derived current state
- `is_terminal` (bool)
- `is_blocked` (bool)
- `mtime` (SystemTime)
- `state_path` (PathBuf)

Fields in `header` that are available but **not rendered**: `created_at` (session age independent of mtime), `session_id` (UUID), `template_source_dir`.

The `elapsed` field in `RowDescriptor` is always `Duration::from_secs(0)` — the comment on line 48 of dashboard_state.rs says "Issue 5 wires up actual timing via mtime" but the code hardcodes zero. So even the **elapsed column is broken** for current sessions.

### What `DetailData` Currently Holds

`DetailData` stores:
- `session_id` (String)
- `gate_name` (String): Most recent gate name
- `command` (Option<String>): Extracted from gate output
- `result` (String): "PASS" or "FAIL"
- `elapsed` (Duration): Since gate evaluation
- `evidence` (Vec<EvidenceEntry>): Current-epoch evidence, newest-first

Missing from `DetailData` that exists in the log:
- State transition history (Transitioned/Rewound/DirectedTransition chain with timestamps)
- Decisions (DecisionRecorded events via `derive_decisions()`)
- Overrides (GateOverrideRecorded events via `derive_overrides()`)
- Context artifacts (ContextAdded events)
- Cancellation reason (WorkflowCancelled)
- Default action output (DefaultActionExecuted)
- Integration outputs (IntegrationInvoked)

### Derivation Functions in persistence.rs Not Used by Dashboard

| Function | What it computes | Used by dashboard? |
|---|---|---|
| `derive_state_from_log` | Current state | Yes (via read_session) |
| `derive_evidence` | Current-epoch EvidenceSubmitted events | Yes (in read_detail) |
| `derive_decisions` | Current-epoch DecisionRecorded events | No |
| `derive_overrides` | Current-epoch GateOverrideRecorded events | No |
| `derive_overrides_all` | All GateOverrideRecorded events | No |
| `derive_last_gate_evaluated` | Most recent gate output | Yes (in read_detail) |
| `derive_machine_state` | Current state + template path + hash | Partially (for is_terminal) |
| `derive_visit_counts` | Per-state entry counts | No |

Three derivation functions — `derive_decisions`, `derive_overrides`, `derive_visit_counts` — are completely unused in the dashboard path.

### The `read_detail()` Gate-Only Assumption

`read_detail()` returns `None` unless a `GateEvaluated` event exists in the current epoch. This means:

- Evidence-only sessions (no gates) get "No gate evaluations recorded" in the detail pane, even though they have `EvidenceSubmitted` data.
- Sessions with `DecisionRecorded`, `WorkflowCancelled`, `GateOverrideRecorded`, or `DefaultActionExecuted` events show nothing in the detail pane unless a `GateEvaluated` also exists.
- The function's guard on line 398 (`find_map` for GateEvaluated, returning `None` if absent) is the single blocking point.

---

## Implications

**Zero new event types required for the following enrichments:**

1. **Fix elapsed column** — compute from `header.created_at` (session age) or `mtime` (time since last activity). `compute_elapsed_since` already exists in dashboard_data.rs.

2. **Transition history in detail pane** — scan events for `Transitioned`, `DirectedTransition`, `Rewound` variants and show a timeline. The `from`/`to` fields, `condition_type`, and optional `rationale` are all persisted.

3. **Show decisions** — `derive_decisions()` is written and tested; just wire it into `read_detail()` and `DetailData`.

4. **Show gate overrides** — `derive_overrides()` is written and tested; same wire-up required.

5. **Context artifact list** — scan for `ContextAdded` events; `key` and `size` are the user-facing fields.

6. **Show cancellation reason** — scan for `WorkflowCancelled`; `reason` is a string.

7. **Show default action results** — scan for `DefaultActionExecuted`; `command`, `exit_code`, `stderr` are directly displayable.

8. **Show spawn variables for batch children** — `WorkflowInitialized.spawn_entry.vars` is available on every batch-spawned child.

9. **Remove the GateEvaluated guard** — make `read_detail()` return data for any session that has any displayable event (evidence, decisions, etc.), not just those with gate evaluations.

10. **Show `created_at` as session age** — `header.created_at` is already in `CachedSession.header`; no additional I/O.

11. **Use `BatchFinalized.view` for authoritative batch counts** — more reliable than live recomputation, especially for cleaned-up children.

---

## Surprises

1. **The elapsed column is always 0.** The `RowDescriptor.elapsed` field is hard-coded to `Duration::from_secs(0)` in `visible_rows()`. The column shows "0s" for every session.

2. **`derive_decisions`, `derive_overrides`, `derive_visit_counts` exist but are dead code in the dashboard path.** They were added to persistence.rs but never connected to the dashboard layer.

3. **Evidence-only sessions get the "No gate evaluations recorded" message.** The detail pane is designed around gate evaluation as the primary artifact, but most evidence-based workflows never hit a gate. The detail pane is empty for a large class of sessions.

4. **`GateOverrideRecorded` carries the human rationale** — the most directly actionable text field in the entire log. It records why a human bypassed a gate check. Not shown anywhere in the dashboard.

5. **`DirectedTransition.rationale` and `Rewound.rationale` are similarly invisible.** When an operator manually skips or rewinds a state, the reason is persisted but never surfaced.

6. **`BatchFinalized.view` duplicates data that `task_counts_for_root()` computes.** The live computation may disagree with the finalized view for cleaned-up children, since `ChildCompleted` events (added specifically to handle this) are also not read by the dashboard.

7. **`session_id` (UUID v4) is available on `CachedSession.header`** but not surfaced. Could be used as a stable copy-able identifier in the detail pane.

---

## Open Questions

1. **How should the detail pane be structured when it must show gate data, evidence, decisions, and overrides simultaneously?** The current 8-row fixed height is insufficient for even moderate event density. Does the design call for a scrollable detail pane, or separate tabbed views?

2. **What is the intended behavior when `derived_decisions()` returns decisions tagged with a state that no longer matches the current epoch?** The existing filter in `derive_decisions` scopes to the current epoch correctly, but the UX question is whether historical decisions (pre-rewind) should be optionally viewable.

3. **Should `derive_visit_counts` feed a "retried N times" indicator?** A visit count > 1 for a state means it was rewound back to and re-entered. This is useful diagnostic data for identifying problem states.

4. **Is `DefaultActionExecuted.stdout`/`stderr` safe to display verbatim?** The strings are uncapped and could be large. A truncation/preview strategy is needed before including them in the TUI.

5. **How should the `BatchFinalized.view.children` array interact with the live `SessionTree`?** Cleaned-up children are not in the tree but appear in the finalized view. Merging these requires a reconciliation strategy.

6. **Does the template file at `template_path` need to be re-read on every refresh, or can it be cached?** Reading the compiled template to enumerate remaining states adds I/O on every mtime-change cycle.

---

## Summary

The event log contains rich data across 15+ event types, but the dashboard's `DetailData` struct and `read_detail()` function were built around gate evaluations as the only display artifact, leaving decisions, overrides, transition history, context artifacts, cancellation reasons, and default action outputs completely unused despite their derivation functions already being implemented in persistence.rs. The most immediately impactful fix is removing the `GateEvaluated` guard in `read_detail()` and wiring in `derive_decisions()` and `derive_overrides()`, which would enrich the detail pane for evidence-based sessions without any new event types or CLI commands. The biggest open question is how to fit this richer data into the current 8-row fixed detail pane without requiring a scrollable or tabbed redesign.
