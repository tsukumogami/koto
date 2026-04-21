# koto next Response Shapes

All `koto next` calls return a JSON object on stdout. The `action` field is always first
and determines which other fields are present. This file annotates every scenario an
agent will encounter, including which fields are absent and why.

---

## Field presence overview

| Field | evidence_required | gate_blocked | integration | integration_unavailable | done | confirm |
|---|---|---|---|---|---|---|
| `action` | always | always | always | always | always | always |
| `state` | always | always | always | always | always | always |
| `directive` | always | always | always | always | **absent** | always |
| `details` | conditional | conditional | conditional | conditional | **absent** | conditional |
| `advanced` | always | always | always | always | always | always |
| `expects` | always (object) | always (`null`) | object or `null` | object or `null` | always (`null`) | object or `null` |
| `blocking_conditions` | always (array) | always (array) | **absent** | **absent** | **absent** | **absent** |
| `integration` | **absent** | **absent** | always | always | **absent** | **absent** |
| `action_output` | **absent** | **absent** | **absent** | **absent** | **absent** | always |
| `error` | `null` | `null` | `null` | `null` | `null` | `null` |

`directive` is never present when `action` is `"done"`. The `done` variant has no
`directive` field in its struct — the key is not written at all, not written as `null`.

`details` follows visit-count logic: present on the first visit to a state, absent on
subsequent visits unless `--full` is passed. It is always absent on `done` regardless.

`blocking_conditions` is present only on `evidence_required` and `gate_blocked`. On all
other action types the key does not appear.

---

## Scenario (a): evidence_required — pure evidence gate, no failed gates

The state has an `accepts` block and the agent must submit evidence to advance. No gates
are blocking.

```json
{
  "action": "evidence_required",
  "state": "review",
  "directive": "Check the output and submit your assessment.",
  "details": "Extended guidance shown on first visit only.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "outcome": {
        "type": "enum",
        "required": true,
        "values": ["approve", "reject", "defer"]
      },
      "notes": {
        "type": "string",
        "required": false
      }
    },
    "options": [
      {"target": "approved", "when": {"outcome": "approve"}},
      {"target": "rejected", "when": {"outcome": "reject"}}
    ]
  },
  "blocking_conditions": [],
  "error": null
}
```

**Decision points:**
- `blocking_conditions` is an empty array `[]` — no gates failed.
- `expects.options` lists which evidence values route to named states. The fallback
  transition (no `when` condition) is not listed here; the engine selects it when no
  other condition matches.
- `options` is omitted entirely when the template has no conditional transitions. If
  absent, there is only a fallback transition and all evidence values lead to the same
  next state.
- `details` is omitted on subsequent visits unless `--full` is passed.

Submit evidence with:
```
koto next my-workflow --with-data '{"outcome": "approve"}'
```

---

## Scenario (b): evidence_required — gates failed, accepts block also present

The state has both gates and an `accepts` block. One or more gates failed, but the
template lets the agent submit override evidence instead of being fully blocked.

```json
{
  "action": "evidence_required",
  "state": "validate",
  "directive": "CI checks failed. Provide override evidence or fix the issue.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": {
        "type": "enum",
        "required": true,
        "values": ["completed", "override"]
      },
      "detail": {
        "type": "string",
        "required": false
      }
    }
  },
  "blocking_conditions": [
    {
      "name": "ci_check",
      "type": "command",
      "status": "failed",
      "category": "corrective",
      "agent_actionable": true,
      "output": {
        "exit_code": 1,
        "error": ""
      }
    }
  ],
  "error": null
}
```

**Decision points:**
- `blocking_conditions` is non-empty — one or more gates failed.
- `category` is `"corrective"` — the agent or user must fix the underlying issue. For
  `"temporal"` blocks (e.g., `children-complete` gates), retry later instead.
- The presence of an `accepts` block means the agent can choose to submit override
  evidence rather than being fully blocked. This is why the action is `evidence_required`
  rather than `gate_blocked`.
- `agent_actionable: true` means the agent can also call `koto overrides record` to
  mark the gate as passed, as an alternative to submitting evidence.
- `details` is absent here because this is a repeat visit. It would appear on the first
  visit to this state.
- `options` is absent from `expects` here because there are no conditional transitions
  (all transitions use the fallback path or a single `when` condition).

---

## Scenario (c): evidence_required — auto-advance candidate (empty expects)

The state has no `accepts` block, no integration, no failed gates, and is not terminal.
The engine returned this shape as a signal that it could not fully auto-advance. In
practice this is rarely seen by an agent because the advance loop auto-advances through
such states before returning. If you do see it, call `koto next` again without any
`--with-data`.

```json
{
  "action": "evidence_required",
  "state": "intermediate",
  "directive": "Intermediate processing state.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {}
  },
  "blocking_conditions": [],
  "error": null
}
```

**Decision points:**
- `expects.fields` is an empty object `{}`.
- `options` is absent (not written as `[]`).
- `blocking_conditions` is `[]`.
- `advanced: true` confirms the engine did make progress before stopping here.
- Call `koto next` again; the engine will continue advancing.

---

## Scenario (c2): skip_if fired — engine advanced one or more states automatically

One or more states in the chain had a `skip_if` block whose conditions matched. The
engine fired the matching transition and continued advancing without waiting for agent
input. The response reflects the final landing state after all chained `skip_if`
transitions completed.

```json
{
  "action": "evidence_required",
  "state": "do_work",
  "directive": "The preparatory state was skipped automatically. Proceed with the main task.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "result": {
        "type": "enum",
        "required": true,
        "values": ["pass", "fail"]
      }
    }
  },
  "blocking_conditions": [],
  "error": null
}
```

**Decision points:**
- `advanced: true` signals that the engine made at least one `skip_if` transition during
  this call. The agent may have landed several states ahead of where it started.
- The response reflects the final state only — intermediate states that were skipped are
  not listed. To see the full transition history, inspect the JSONL event log for the
  session.
- In the event log, each `skip_if` transition appears as a `Transitioned` event with
  `"condition_type": "skip_if"` and a non-null `skip_if_matched` map showing the
  conditions that fired. Events for states advanced via evidence or gates use a different
  `condition_type` value and have `skip_if_matched` absent.
- Consecutive `skip_if` states chain within a single call. If the landing state also has
  a `skip_if` whose conditions match, the engine advances again before returning.
- The shape of the landing state determines the action: `evidence_required`,
  `gate_blocked`, `done`, etc. `advanced: true` is the only `skip_if`-specific signal in
  the response itself.

---

## Scenario (d): gate_blocked — agent can resolve

The state has one or more failed gates, no `accepts` block, and at least one gate is
actionable (the agent can record an override).

```json
{
  "action": "gate_blocked",
  "state": "check",
  "directive": "Waiting for CI to pass before proceeding.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "ci_check",
      "type": "command",
      "status": "failed",
      "category": "corrective",
      "agent_actionable": true,
      "output": {
        "exit_code": 1,
        "error": ""
      }
    }
  ],
  "error": null
}
```

**Decision points:**
- `expects` is `null` — there is no `accepts` block, so no evidence submission is possible.
- `category` is `"corrective"` — the agent or user must fix something. See Scenario (j)
  for the `"temporal"` variant used by `children-complete` gates.
- `agent_actionable: true` — the gate has an `override_default` value or a built-in
  default. The agent can call `koto overrides record my-workflow --gate ci_check
  --rationale "manual verification passed"` to unblock.
- `output` carries the structured result from the gate runner:
  - `command` gates: `{"exit_code": <int>, "error": "<string>"}`
  - `context-exists` gates: `{"exists": false, "error": "<string>"}`
  - `children-complete` gates: `{"total": <int>, "completed": <int>, "pending": <int>, "success": <int>, "failed": <int>, "skipped": <int>, "blocked": <int>, "spawn_failed": <int>, "all_complete": <bool>, "all_success": <bool>, "any_failed": <bool>, "any_skipped": <bool>, "any_spawn_failed": <bool>, "needs_attention": <bool>, "children": [...], "error": "<string>"}`. Each `children[]` entry has `{"name", "state", "complete", "outcome"}`; failed children add `failure_mode` and `reason_source`; skipped children add `skipped_because`, `skipped_because_chain`, `reason_source`; blocked children add `blocked_by`.
- `blocking_conditions` only includes gates that failed; passing gates are excluded.

---

## Scenario (e): gate_blocked — agent cannot resolve

The state has a failed gate that is not actionable. The agent cannot override it.

```json
{
  "action": "gate_blocked",
  "state": "approval",
  "directive": "Waiting for external approval before this workflow can continue.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "manager_sign_off",
      "type": "context-exists",
      "status": "failed",
      "category": "corrective",
      "agent_actionable": false,
      "output": {
        "exists": false,
        "error": ""
      }
    }
  ],
  "error": null
}
```

**Decision points:**
- `agent_actionable: false` — the gate has no `override_default` and no built-in default
  for its type. The agent cannot call `koto overrides record` to resolve this.
- The right action is to report the blocking condition to the user. The directive text
  typically explains what external action is required.
- Do not retry `koto next` in a loop — the condition is externally controlled and will
  not change until someone else acts.

---

## Scenario (f): integration — integration ran successfully

An integration runner executed and returned output. The workflow is paused for the agent
to review the result.

```json
{
  "action": "integration",
  "state": "run_tests",
  "directive": "Review the test results and proceed.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "confirmed": {
        "type": "boolean",
        "required": true
      }
    }
  },
  "integration": {
    "name": "test-runner",
    "output": {"passed": 42, "failed": 0}
  },
  "error": null
}
```

**Decision points:**
- `blocking_conditions` is **absent** — this key does not appear on `integration`.
- `expects` may be `null` when the state has no `accepts` block.
- `integration.output` is the raw JSON value returned by the integration runner.

Note: as of the current release, no integration runners are implemented. This shape is
defined and reserved for future use.

---

## Scenario (g): integration_unavailable

The template declares an integration, but no runner is configured. This is the shape
agents encounter today for any state that uses `integration:`.

```json
{
  "action": "integration_unavailable",
  "state": "run_tests",
  "directive": "Run the test suite and report results.",
  "advanced": false,
  "expects": null,
  "integration": {
    "name": "test-runner",
    "available": false
  },
  "error": null
}
```

**Decision points:**
- `integration.available` is always `false`.
- `blocking_conditions` is **absent**.
- `expects` is `null` here because this state has no `accepts` block. It would be an
  `ExpectsSchema` object when the state also has an `accepts` block.
- The agent should report this to the user — the template requires an integration that
  has not been set up.

---

## Scenario (h): done — workflow reached terminal state

```json
{
  "action": "done",
  "state": "complete",
  "advanced": true,
  "expects": null,
  "error": null
}
```

**Decision points:**
- `directive` is **absent** — the key is not written at all, not written as `null`.
  Do not attempt to read `response.directive` when `action == "done"`.
- `details` is **absent** — the terminal variant has no `details` field.
- `blocking_conditions` is **absent**.
- `advanced: true` confirms the engine transitioned into this terminal state during
  the current call. `advanced: false` would mean the workflow was already terminal
  before this call (e.g., you called `koto next` on an already-completed workflow).
- After `done`, the session directory is cleaned up automatically unless `--no-cleanup`
  was passed. Any subsequent `koto next` call returns exit 2 with
  `error.code = "terminal_state"`.

---

## Scenario (i): confirm — default action ran and needs confirmation

A default action executed and requires the agent to confirm before the workflow advances.
The `action_output` field carries what the command printed.

```json
{
  "action": "confirm",
  "state": "deploy",
  "directive": "Review the deployment output and confirm.",
  "advanced": false,
  "action_output": {
    "command": "deploy.sh",
    "exit_code": 0,
    "stdout": "Deployed to staging.\n",
    "stderr": ""
  },
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "confirmed": {
        "type": "boolean",
        "required": true
      }
    }
  },
  "error": null
}
```

**Decision points:**
- `blocking_conditions` is **absent**.
- `action_output.stdout` and `action_output.stderr` are truncated to 64 KB each.
- `expects` may be `null` when the state has no `accepts` block.

---

## Scenario (j): gate_blocked — temporal block (children-complete)

A `children-complete` gate is blocking because one or more child workflows haven't
finished yet. This is a temporal condition — it resolves on its own as children complete.

```json
{
  "action": "gate_blocked",
  "state": "converge",
  "directive": "Waiting for all research agents to finish.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "children-done",
      "type": "children-complete",
      "status": "failed",
      "category": "temporal",
      "agent_actionable": true,
      "output": {
        "total": 3,
        "completed": 2,
        "pending": 1,
        "success": 2,
        "failed": 0,
        "skipped": 0,
        "blocked": 0,
        "spawn_failed": 0,
        "all_complete": false,
        "all_success": false,
        "any_failed": false,
        "any_skipped": false,
        "any_spawn_failed": false,
        "needs_attention": false,
        "children": [
          {"name": "explore.r1", "state": "done", "complete": true, "outcome": "success"},
          {"name": "explore.r2", "state": "done", "complete": true, "outcome": "success"},
          {"name": "explore.r3", "state": "research", "complete": false, "outcome": "pending"}
        ],
        "error": ""
      }
    }
  ],
  "error": null
}
```

**Decision points:**
- `category: "temporal"` — don't try to fix anything. The children will finish on their
  own. Poll `koto next` again later.
- `output.all_complete` is `false` — at least one child is still running.
- `output.children` gives per-child detail. Use `koto status <child>` or
  `koto context get <child> <key>` for deeper inspection.
- `agent_actionable: true` — you can override with `koto overrides record` if you need
  to proceed before all children finish. The override pretends all children are done.
- Don't retry in a tight loop. Children are running their own workflows and need time.

---

## Checking for absent fields

Several fields are conditionally absent rather than `null`. When writing code to parse
`koto next` output:

- Check `action == "done"` before reading `directive`.
- Check `action` before assuming `blocking_conditions` is present — it only appears on
  `evidence_required` and `gate_blocked`.
- Check whether `details` is present before reading it; it may be omitted on any action
  type depending on visit count.
- `expects` is always written but may be `null` — this is not the same as absent.
- `options` inside an `expects` object is omitted (not written) when empty, not written
  as `[]`.
- In the JSONL event log, `skip_if_matched` is absent on `Transitioned` events whose
  `condition_type` is not `"skip_if"`. Don't assume this field is present — check
  `condition_type` first.

---

For full field definitions and the `advance_until_stop` stopping condition table, see
`docs/guides/cli-usage.md`.
