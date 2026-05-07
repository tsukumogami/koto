# Lead: Consumer Event Classification

## Findings

### WorkflowInitialized

**Purpose:** Written once at `koto init` time. Carries `template_path`, `variables`, and an optional `spawn_entry` snapshot (for batch-spawned children only).

**Dashboard relevance:** Required. It establishes the session's starting state and template identity. A dashboard uses this to display the workflow name, template used, and any initial variable bindings. The `spawn_entry` field is also dashboard-relevant when showing parent/child relationships in a batch view — it records which task entry the child was created from.

**Classification:** Required display.

---

### Transitioned

**Purpose:** Records every automatic or evidence-driven state change. Fields: `from`, `to`, `condition_type`, and optional `skip_if_matched`.

**Dashboard relevance:** Required. This is the primary progress event. A dashboard uses `to` to show current state, uses the sequence of `Transitioned` events to reconstruct the session timeline, and can show `skip_if_matched` to explain auto-skipped states.

**Classification:** Required display.

---

### EvidenceSubmitted

**Purpose:** Records what an agent submitted for a state. Fields: `state`, `fields` (free-form JSON), and optional `submitter_cwd` (used internally by the batch path resolver).

**Dashboard relevance:** Required. The `fields` content is the agent's work product for a given state — decisions, outputs, structured results. A dashboard that shows "what the agent did in state X" reads this event. The `submitter_cwd` field is internal to the batch scheduler's path resolution and has no display value.

**Classification:** Required display. `submitter_cwd` is an internal-only field within the payload.

---

### IntegrationInvoked

**Purpose:** Records when a named integration (external system call) ran during a state. Fields: `state`, `integration`, `output`.

**Dashboard relevance:** Optional. An agent-execution dashboard would want to show "integration X ran with output Y" for debugging. Not needed for basic state-progress views, but valuable for detailed audit trails.

**Classification:** Optional display.

---

### DirectedTransition

**Purpose:** Records a manual state override (agent or human explicitly called `koto next --to <state>`). Fields: `from`, `to`, optional `rationale`.

**Dashboard relevance:** Required. A directed transition is a significant user-visible event — it signals that a human or agent deliberately overrode the normal progression. The `rationale` field carries the decision justification. A dashboard should distinguish these from automatic `Transitioned` events.

**Classification:** Required display.

---

### Rewound

**Purpose:** Records a rollback to a prior state. Fields: `from`, `to`, optional `rationale`.

**Dashboard relevance:** Required. Rewinding is a meaningful workflow correction. A dashboard showing session history must display rewinds to give users an accurate picture of what happened. The F3 local dashboard spec explicitly lists session hierarchy; rewinding changes what states are active.

**Classification:** Required display.

---

### ContextAdded

**Purpose:** Appended by `koto context add`. Fields: `key`, `hash` (SHA-256), `size`. Tracks what context artifacts the agent wrote and when, relative to state transitions.

**Dashboard relevance:** Optional but useful. The PRD-session-schema-hygiene.md explicitly defines this event to support auditing "which context was available at transition T" by comparing `seq` values. A full audit dashboard needs it. A simple progress view does not.

**Classification:** Optional display. Primarily audit/forensic value.

---

### WorkflowCancelled

**Purpose:** Records explicit workflow cancellation. Fields: `state`, `reason`.

**Dashboard relevance:** Required. Cancellation is a terminal session event. Any dashboard must display it to convey the session's final outcome.

**Classification:** Required display.

---

### DefaultActionExecuted

**Purpose:** Records when a state's `default_action` command ran. Fields: `state`, `command`, `exit_code`, `stdout`, `stderr`.

**Dashboard relevance:** Optional. Useful for debugging or detailed execution logs, but the full `stdout`/`stderr` content is verbose. A summary display might show command + exit_code; raw output is low-value for most views. The integration tests (scenario in `integration_test.rs`) confirm this fires on auto-advancing states with `default_action` configured.

**Classification:** Optional display. Contains both summary-useful fields (`command`, `exit_code`) and detail-only fields (`stdout`, `stderr`).

---

### DecisionRecorded

**Purpose:** Records a structured agent decision captured mid-state via `koto decisions record`. Fields: `state`, `decision` (free-form JSON).

**Dashboard relevance:** Optional but high value. The design (`DESIGN-mid-state-decision-capture.md`) describes these as audit-trail records — the agent's reasoning captured inside a state before evidence is submitted. A dashboard showing agent decision history surfaces these.

**Classification:** Optional display. Richer than most events — could be a "details" tier rather than a headline progress item.

---

### GateEvaluated

**Purpose:** Records each gate check result. Fields: `state`, `gate`, `output` (structured JSON), `outcome`, `timestamp`.

**Dashboard relevance:** Optional but significant. The F3 dashboard spec explicitly lists "gate evaluations" as a display item. A dashboard can show which gates were checked, whether they passed or blocked, and the structured output (which may include CI pass/fail counts, context-match scores, etc.). Multiple `GateEvaluated` events may appear per state (polling).

**Classification:** Optional display. The F3 spec makes this conditionally required — "show gate evaluations" is part of the described dashboard surface.

---

### GateOverrideRecorded

**Purpose:** Records when an agent bypassed a gate. Fields: `state`, `gate`, `rationale`, `override_applied`, `actual_output`, `timestamp`.

**Dashboard relevance:** Required. Gate overrides are human-in-the-loop decisions — someone chose to proceed despite a failing gate. A dashboard must surface these prominently. The PRD-gate-transition-contract.md describes overrides as an audit concern. Missing a gate override in the UI would hide a significant decision.

**Classification:** Required display.

---

### SchedulerRan

**Purpose:** Per-tick audit record from the batch scheduler. Emitted only on non-trivial ticks (at least one spawn, reclassification, skip, or error). Fields: `state`, `tick_summary` (spawned/errored/skipped counts, `reclassified` flag), `timestamp`.

**Dashboard relevance:** Internal. The `tick_summary` data (counts of spawned, errored, skipped children per tick) is batch scheduler bookkeeping. Dashboards do not need per-tick spawn counts — they need aggregate batch status, which is derived from `BatchFinalized` and live child state reads. The code comment in `batch.rs` explicitly notes this event is for "downstream consumers reading just the payload" who want per-tick audit without re-running the scheduler — this is a developer/debug audience, not a user dashboard audience.

**Classification:** Internal. Can be surfaced by developer tooling or `koto query --events`, but not a primary dashboard display item.

---

### BatchFinalized

**Purpose:** Emitted when the `children-complete` gate first reports `all_complete: true`. Freezes the final batch view at that moment. Fields: `state`, `view` (frozen gate output snapshot), `timestamp`, optional `superseded_by`. The most recent `BatchFinalized` drives `koto status` batch display after children are cleaned up.

**Dashboard relevance:** Required for batch workflows. A dashboard showing a parent workflow that ran children must use `BatchFinalized` to reconstruct the batch's outcome after child state files may have been auto-cleaned. The `view` payload includes counts (total, completed, success, failed, skipped, pending, blocked) and child-level details.

**Classification:** Required display for batch workflows. For non-batch workflows, this event never appears and is irrelevant.

---

### ChildCompleted

**Purpose:** Written to the PARENT's log when a child reaches a terminal state and is about to be auto-cleaned. Fields: `child_name`, `task_name`, `outcome` (success/failure/skipped), `final_state`. Serves as a fallback for the `children-complete` gate when the child's on-disk state file has been removed.

**Dashboard relevance:** Optional — internal recovery mechanism. This event exists specifically to handle the race between auto-cleanup and the parent's next gate evaluation. A dashboard that is replaying events (rather than reading live state) can use `ChildCompleted` events to reconstruct which children finished and with what outcome. However, for live dashboards, `BatchFinalized.view` covers the same ground. For historical reconstruction, `ChildCompleted` is the event-log record that a child completed.

**Classification:** Optional for display; required for accurate event-log-only replay of batch outcomes.

---

## Implications

### Should the contract define audience tiers or stay flat?

The evidence supports three tiers rather than a flat specification. The events divide cleanly:

**Tier 1 — Required display:** WorkflowInitialized, Transitioned, DirectedTransition, Rewound, EvidenceSubmitted, WorkflowCancelled, GateOverrideRecorded, BatchFinalized (batch-only).

These are the events a dashboard MUST render to give an accurate picture of session progress. Omitting any of them leaves users without critical context: they won't know the session started, advanced, was manually overridden, was cancelled, or (for batch workflows) how children completed.

**Tier 2 — Optional/enrichment display:** IntegrationInvoked, ContextAdded, DefaultActionExecuted, DecisionRecorded, GateEvaluated, ChildCompleted.

These add depth — audit trails, decision histories, gate-check details — but a minimal viable dashboard can omit them without becoming misleading. The F3 spec lists gate evaluations as a display item, which nudges `GateEvaluated` toward required, but for an MVP a dashboard without gate evaluation detail still conveys accurate session progress.

**Tier 3 — Internal/debug only:** SchedulerRan.

`SchedulerRan` is a batch scheduler audit log. Dashboards and relays should skip it unless presenting a developer debug view. The event's own doc comment says it's for consumers who want per-tick audit "without re-running the scheduler" — a developer scenario, not a user scenario.

A flat specification would force every consumer to make the same classification call independently, with no guidance. Given that the contract is targeting a local dashboard, an S3-backed dashboard, and a hosted relay — three different consumers — specifying audience tiers in the contract prevents each consumer from accidentally dropping a required event or surfacing a debug-only event to users.

The tier label for each event should be: `required`, `optional`, or `internal`. The contract can also note when a tier applies conditionally (e.g., `BatchFinalized` is `required` for batch workflows and `not applicable` for non-batch workflows).

---

## Surprises

**EvidenceSubmitted contains an internal field.** The `submitter_cwd` field inside `EvidenceSubmitted` is not user-facing — it exists solely for the batch scheduler's path resolver. This is unusual: the event itself is Tier 1 (required display), but one of its fields is purely internal. The contract should note that `submitter_cwd` is a scheduler-internal field that dashboards can ignore, without changing the event's overall tier.

**GateEvaluated is borderline.** The F3 spec lists gate evaluations as a dashboard item, but a single state transition may produce many `GateEvaluated` events during a polling sequence. A dashboard that renders every gate evaluation as a headline event would be noisy. The contract likely needs to distinguish between showing the most recent gate result (useful) vs. all gate evaluations (verbose). This is not just a tier question — it's a rendering guidance question.

**ChildCompleted is dual-purpose.** It serves a pure internal purpose (recovering from auto-cleanup race conditions) but also carries semantic information (child outcome, final state) that has display value for event-log-only replay. A consumer reading only the event log (no live state access) needs `ChildCompleted` to reconstruct batch outcomes. A consumer with live state access can ignore it. The tier depends on the consumer's access model, which is unusual.

**BatchFinalized carries display content in its internal format.** The `view` payload is the frozen output of the `children-complete` gate — a rich JSON blob that includes per-child statuses. This is the same data surface `koto status` uses after cleanup. A dashboard reading `BatchFinalized.view` gets a complete batch summary without any live state reads. This is a stronger display contract than most events provide.

---

## Open Questions

1. **Should GateEvaluated be Tier 1 or Tier 2?** The F3 spec includes "gate evaluations" in the dashboard's display list, but the volume of gate events during polling makes all-events display impractical. Does the contract need to specify "show only the final gate result before transition" vs. "show all gate evaluations"? Or is display guidance out of scope for the event classification contract?

2. **How should the contract handle EvidenceSubmitted.submitter_cwd?** Should the contract annotate individual fields within an event's payload as internal, or only classify events as a whole? Field-level annotation is more precise but significantly complicates the spec.

3. **What does "internal" mean for a relay consumer?** A hosted relay that forwards events to an S3-backed dashboard presumably forwards all events (it can't know downstream display intent). Does `internal` mean "dashboards should not surface this to users" or "relays should filter this out"? These are different contracts.

4. **Is ChildCompleted's tier access-model-dependent?** If the contract needs to say "Tier 2 for consumers with live state access, Tier 1 for event-log-only consumers," that's a structurally different kind of conditional than batch-only events. Human input is needed on whether the contract should address access models.

5. **Does DefaultActionExecuted warrant field-level tier annotation?** `command` and `exit_code` are display-relevant (show "ran command X, exited N"). `stdout` and `stderr` are debug-level. Should the contract call this out?

---

## Summary

The 15 event types split cleanly into three tiers: 8 are required for any honest session dashboard (WorkflowInitialized, Transitioned, DirectedTransition, Rewound, EvidenceSubmitted, WorkflowCancelled, GateOverrideRecorded, BatchFinalized), 6 are optional enrichment (IntegrationInvoked, ContextAdded, DefaultActionExecuted, DecisionRecorded, GateEvaluated, ChildCompleted), and 1 is purely internal to the batch scheduler (SchedulerRan). A tiered contract rather than a flat spec is the right choice here: three distinct consumers (local dashboard, S3 dashboard, relay) would otherwise each classify events independently, with no shared guidance on what is user-visible vs. internal. The biggest open question is whether GateEvaluated should move to Tier 1 given its explicit presence in the F3 dashboard spec, and whether the contract should address field-level internal annotations within otherwise-required events like EvidenceSubmitted.
