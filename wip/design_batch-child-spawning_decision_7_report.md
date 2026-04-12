<!-- decision:start id="batch-blocked-agent-guidance" status="confirmed" -->
### Decision 7: Batch-blocked agent guidance mechanism

**Context**

When a parent workflow submits a batch task list and the children-complete gate blocks, the `koto next` response carries structured data (children counts, per-child status in `blocking_conditions`, spawned/blocked lists in the new `scheduler` field) but nothing explicitly tells the agent in plain English what to do next. The concern is that agents reading structured JSON without prose context may misinterpret expectations -- should they drive children sequentially, delegate to sub-agents, wait for an external system, or poll?

The question is whether this gap requires a new feature (engine-generated guidance, a new template field, or a template interpolation engine) or whether the existing directive mechanism already handles it.

**Assumptions**

- Agents consuming batch responses will be guided by a skill (koto-user) that documents the batch-specific response pattern. Agents operating without a skill may struggle with batch responses, but that's true of all koto responses.
- Template authors writing batch workflows are sophisticated users. The batch feature is advanced functionality; expecting good directive prose from these authors is reasonable.

**Chosen: Existing directive + details mechanism (no new features)**

The `GateBlocked` response variant already includes `directive: String` -- the template-authored prose from the state's markdown body section. This directive appears on every children-complete gate block. Template authors already write this text for every state; the batch state is no exception. The existing mechanism provides:

1. **Directive prose (always present).** The template author writes batch-aware text in the state's markdown body. The Decision 1 template example in the design doc demonstrates the pattern:
   ```yaml
   plan_and_await:
     # In the markdown body:
     # "If you haven't submitted a task list yet, read the plan
     #  and submit one. Otherwise wait for children to complete."
   ```

2. **Details text (first visit).** The `<!-- details -->` marker lets authors include extended batch instructions on first visit -- explaining the sub-agent spawn pattern, how to interpret scheduler output, when to re-tick the parent -- that disappear on repeat visits to reduce noise.

3. **Structured scheduler data.** The `scheduler` field (new in this design) already carries spawned/blocked/skipped lists as machine-readable JSON, complementing the prose directive.

4. **Structured gate output.** `blocking_conditions[].output` carries children-complete counts and per-child status with outcome enums.

5. **Skill-level documentation.** The koto-user skill's response-shapes reference documents the batch response pattern, teaching agents how to interpret the scheduler outcome alongside the directive.

No code changes are needed. The guidance mechanism is a template authoring concern, not an engine concern, and the authoring tools already exist.

**Concrete response example.** Here is what an agent sees on the second call to `koto next parent-42` after submitting a batch task list, with one child newly spawned by the scheduler:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Children have been spawned from your task list. For each child in the scheduler.spawned list below, start a sub-agent that drives it via `koto next <child-name>`. Re-check the parent with `koto next parent-42` after each child completes to spawn newly-unblocked tasks.",
  "details": "The batch scheduler runs on every `koto next parent-42` call. It reads child state files from disk and spawns tasks whose dependencies are all terminal. You don't need to track which children are ready -- the scheduler outcome tells you. Drive children in parallel when possible. Each child is an independent workflow with its own state file.\n\nTo check batch progress without driving the parent: `koto status parent-42`.\nTo inspect a specific child: `koto status parent-42.issue-1`.",
  "advanced": true,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "done",
      "type": "children-complete",
      "status": "failed",
      "category": "temporal",
      "agent_actionable": true,
      "output": {
        "total": 10,
        "completed": 1,
        "pending": 9,
        "success": 1,
        "failed": 0,
        "skipped": 0,
        "blocked": 6,
        "all_complete": false,
        "children": [
          {"name": "parent-42.issue-1", "state": "done", "complete": true, "outcome": "success"},
          {"name": "parent-42.issue-2", "state": "implementing", "complete": false, "outcome": "pending"},
          {"name": "parent-42.issue-3", "state": "implementing", "complete": false, "outcome": "pending"},
          {"name": "parent-42.issue-4", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["parent-42.issue-2"]}
        ]
      }
    }
  ],
  "scheduler": {
    "spawned": ["parent-42.issue-4"],
    "skipped": [],
    "already": ["parent-42.issue-1", "parent-42.issue-2", "parent-42.issue-3"],
    "blocked": ["parent-42.issue-5", "parent-42.issue-6", "parent-42.issue-7", "parent-42.issue-8", "parent-42.issue-9", "parent-42.issue-10"]
  },
  "error": null
}
```

The directive and details text above are entirely template-authored. A different template might say "Wait for the CI system to complete each child -- do not drive them manually" or "Delegate each child to a separate team member via the task assignment system." koto doesn't prescribe; the template author does.

**Rationale**

The directive field already solves this problem for non-batch states, and it solves it for batch states too. Adding a new field (`batch_directive`, `suggested_action`, `action_hint`) or a template interpolation engine would:

- **Create redundancy.** Two places to write agent guidance (directive AND the new field), with unclear precedence when both are present.
- **Add surface area.** A new field means new documentation, new skill content, new test coverage, and a new concept for template authors to learn.
- **Violate "contract layer, not execution engine"** (for hardcoded options). Engine-generated prose tells agents how to work, which is the template author's prerogative.
- **Add a dependency** (for interpolation). Conditional logic in directives (`{{#if batch.spawned}}`) requires a template engine library, new parsing, and new error modes -- all for a feature that the details marker already approximates.

The one valid concern -- that template authors might forget to write batch-aware directives -- is a documentation problem. The koto-author skill's template-format reference and the batch feature's own docs should include directive examples for the single-state fan-out pattern. That's where the investment belongs.

**Alternatives Considered**

- **Hardcoded engine-generated `suggested_action`.** Rejected because generic prose can't fit every use case (drive vs delegate vs wait), and engine-generated behavioral suggestions contradict koto's "contract layer" philosophy. The engine doesn't know whether the agent should run children in parallel, sequentially, or delegate them.

- **Template-authored `batch_directive` on materialize_children hook.** Rejected as redundant with the existing directive. The state already has a directive; adding a second one creates an unclear relationship. Which text does the agent read? What if they contradict? The existing directive already covers the batch case.

- **Extend directive with batch-aware interpolation.** Rejected because it introduces a template engine dependency (Handlebars/Tera) and conditional syntax (`{{#if batch.spawned}}`) into a system where directives are currently plain text with simple variable substitution. The complexity is disproportionate to the value. Runtime state (child names, counts) is already available in `blocking_conditions[].output` and `scheduler` -- putting it in prose too adds no decision-relevant information.

- **Structured `action_hint` on gate output.** Rejected as a new concept in koto's gate output vocabulary that overlaps with the existing directive. The `action_hint.message` field would be a third place to write agent guidance alongside directive and details.

**Consequences**

- Template authors must write good batch-aware directives. This is documented in the template-format reference and batch feature docs with examples.
- No new code needed for this decision. The batch feature's implementation scope shrinks by one field/serialization path.
- The koto-user skill's response-shapes reference needs a new scenario (batch gate_blocked) showing the `scheduler` field and explaining the polling pattern. This is a skill update, not an engine change.
- If a future use case genuinely needs runtime data in directive prose (not just batch), the interpolation approach (Alternative 3) can be revisited as a separate feature. The current decision doesn't foreclose it.

<!-- decision:end -->

---

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Existing directive + details mechanism (no new features)"
  confidence: "high"
  rationale: >
    The GateBlocked response already includes template-authored directive
    prose, the details marker provides first-visit extended guidance, and
    the scheduler field carries machine-readable batch data. All four
    new-feature alternatives add redundancy without solving a problem
    the existing mechanism doesn't already handle.
  assumptions:
    - "Agents consuming batch responses will be guided by a skill (koto-user) that documents the batch-specific pattern"
    - "Template authors writing batch workflows are sophisticated enough to write good directive text"
  rejected:
    - name: "Hardcoded engine-generated guidance"
      reason: "Generic prose can't fit every use case; contradicts contract-layer philosophy"
    - name: "Template-authored batch_directive on hook"
      reason: "Redundant with existing directive field; creates unclear two-directive relationship"
    - name: "Batch-aware directive interpolation"
      reason: "Requires template engine dependency for a problem plain text already solves"
    - name: "Structured action_hint on gate output"
      reason: "New concept that overlaps with existing directive; third place to write guidance"
  report_file: "wip/design_batch-child-spawning_decision_7_report.md"
```
