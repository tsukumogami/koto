---
status: In Progress
problem: |
  When agents bypass gates in koto, the reasoning disappears. Overrides are
  implicit (inferred from evidence on a gate-failed state), and states without
  an accepts block can't be overridden at all. The event log captures evidence
  but not why the agent chose to bypass the gate. Session visualization and
  human review of agent behavior depend on answering "why was this gate
  overridden?"
goals: |
  Make gate overrides first-class auditable events with mandatory rationale,
  queryable across the full session. Persist enough context for future
  visualization and redo capabilities without building those consumers now.
source_issue: 108
---

# PRD: Override gate rationale

## Status

In Progress

## Problem statement

Gate overrides in koto are invisible. When an agent submits evidence on a
gate-failed state, the engine advances the workflow, but no event records that
a gate was bypassed or why the agent chose to proceed despite the failure.

Four things are broken:

1. **Overrides require template workarounds.** On deterministic gate states
   (where the gate checks a condition and the workflow should advance when it
   passes), template authors must add an `accepts` block with an `override`
   enum value and a matching conditional transition -- all just so agents
   have a way to bypass a failed gate. This is boilerplate that exists only
   because the engine lacks a built-in override mechanism. Without this
   workaround, the agent's only option on a gate-blocked state is `--to`,
   which bypasses everything with no audit trail.

2. **Overrides are implicit.** Even with the workaround, the engine infers
   override from evidence presence on a gate-failed state. There's no
   explicit "I'm overriding this gate" signal.

3. **Rationale is disconnected.** An agent can call `koto decisions record` to
   log reasoning, but that's a separate operation with no structural link to the
   override. Nothing forces it. The override and the rationale live in different
   events with no connection.

4. **No cross-session query surface.** `koto decisions list` is epoch-scoped
   (current state only). There's no way to ask "show me all overrides in this
   session" without parsing raw JSONL.

This matters because the north star is session visualization: a human reviewer
should be able to see every gate override an agent made, understand the
reasoning, and eventually force a redo when they disagree. Without structured
override data, none of that is possible.

## Goals

- Every gate override produces an auditable event with mandatory rationale
- Override events capture enough context to answer: what gate failed, why it
  failed, and why the agent proceeded anyway
- Override history is queryable across the full session, not just the current
  state epoch
- The data shape supports future visualization and redo consumers without
  requiring schema changes when those features arrive

## User stories

**As a workflow skill author**, I want gate overrides to automatically capture
rationale so I don't have to remember to call `koto decisions record` as a
separate step after overriding.

**As a workflow skill author**, I want every gate-blocked state to be
overridable without requiring the template to declare an `accepts` block with
an `override` value. Override is an engine capability, not a template design
choice.

**As a human reviewer**, I want to query all gate overrides in a session so I
can audit agent behavior and identify questionable bypasses.

**As a template author**, I want override events to include which gate failed
and why so I can diagnose whether my gate conditions are too strict or agents
are bypassing legitimate checks.

**As a future visualization consumer**, I want override events to be
self-contained (gate failure context + rationale) so I can render an override
timeline without correlating multiple event types.

## Interaction examples

These show the concrete CLI interactions between an agent and koto, before and
after this feature.

### Example 1: CI gate override -- today's workaround vs. engine-level override

A template has a `verify` state with a CI gate. Today, template authors must
add an `accepts` block with an `override` enum value just so agents have a
way to bypass a failed gate. This is boilerplate that exists only because the
engine lacks a built-in override mechanism.

**Today (workaround pattern):**

The template must include accepts + override enum + matching transition:
```yaml
verify:
  gates:
    ci_check:
      type: command
      command: "test -f ci-passed.txt"
  accepts:                    # workaround: only exists for override
    status:
      type: enum
      values: [completed, override]
      required: true
  transitions:
    - target: deploy
      when:
        status: completed     # workaround: manual coupling
    - target: deploy
      when:
        status: override      # workaround: manual coupling
    - target: deploy          # unconditional fallback (gates pass)
```

```bash
# Gate fails -- agent gets evidence_required because accepts block exists
$ koto next my-workflow
{"action": "evidence_required", "state": "verify",
 "blocking_conditions": [{"gate": "ci_check", "result": "failed", "exit_code": 1}],
 "expects": {"fields": {"status": {"type": "enum", "values": ["completed", "override"]}}}}

# Agent submits override evidence -- rationale is lost
$ koto next my-workflow --with-data '{"status": "override"}'
{"action": "done", "state": "deploy", "advanced": true}

# Later, reviewer asks: "why did the agent skip CI?" -- no answer in the log
```

**After this feature:**

The template is simpler -- just gates and a transition. No accepts workaround:
```yaml
verify:
  gates:
    ci_check:
      type: command
      command: "test -f ci-passed.txt"
  transitions:
    - target: deploy          # gates pass -> auto-advance here
```

```bash
# Gate fails -- returns gate_blocked (no accepts block needed)
$ koto next my-workflow
{"action": "gate_blocked", "state": "verify",
 "blocking_conditions": [{"gate": "ci_check", "result": "failed", "exit_code": 1}]}

# Agent overrides at the engine level -- no --with-data, no accepts block
$ koto next my-workflow --override-rationale "CI failure is flaky test_network_timeout, unrelated to docs change"
{"action": "done", "state": "deploy", "advanced": true}

# Empty rationale is rejected
$ koto next my-workflow --override-rationale ""
{"error": {"code": "invalid_submission", "message": "--override-rationale requires a non-empty string"}}

# Reviewer queries overrides
$ koto overrides list my-workflow
{"overrides": [
  {"state": "verify", "gates_failed": {"ci_check": {"result": "failed", "exit_code": 1}},
   "rationale": "CI failure is flaky test_network_timeout, unrelated to docs change",
   "seq": 8, "timestamp": "2026-03-30T14:22:00Z"}
]}
```

### Example 3: Context injection override in a skill workflow

A skill template has a `context_injection` state where the gate checks if a
baseline artifact file exists. The agent already has the context from reading
the issue via `gh issue view` and wants to skip generating the artifact.

```bash
# Gate fails because baseline artifact doesn't exist
$ koto next work-session
{"action": "gate_blocked", "state": "context_injection",
 "blocking_conditions": [{"gate": "baseline_exists", "result": "failed", "exit_code": 1}]}

# Agent overrides with rationale
$ koto next work-session \
    --override-rationale "Issue context already loaded via gh issue view #42, baseline artifact not needed"
{"action": "evidence_required", "state": "planning", "advanced": true, ...}
```

### Example 4: Normal evidence submission (not an override)

When gates pass, or evidence is submitted for transition routing (not gate
bypass), everything works exactly as today. `--override-rationale` is not involved.

```bash
# Gate passes (file exists), auto-advances
$ koto next my-workflow
{"action": "done", "state": "complete", "advanced": true}

# Gates pass, state needs evidence for transition routing
$ koto next my-workflow --with-data '{"mode": "issue_backed", "issue_number": "42"}'
{"action": "evidence_required", "state": "planning", "advanced": true, ...}
# No override event emitted -- this is normal evidence, not a gate bypass
```

### Example 5: Override and evidence together

Some states have both gates and an `accepts` block. The agent might want to
override the gate AND provide evidence for transition routing.

```bash
# Gate fails on a state that also accepts evidence
$ koto next my-workflow
{"action": "evidence_required", "state": "setup",
 "blocking_conditions": [{"gate": "branch_exists", "result": "failed", "exit_code": 1}],
 "expects": {"fields": {"status": {"type": "enum", "values": ["completed", "override"]}}}}

# Agent overrides the gate AND provides evidence for transition resolution
$ koto next my-workflow --override-rationale "Reusing existing branch per user request" \
    --with-data '{"status": "override"}'
{"action": "evidence_required", "state": "implementation", "advanced": true, ...}
```

## Requirements

### Functional

**R1: First-class override event.** When an agent uses `--override-rationale` on a
gate-blocked state and the transition resolves, the engine emits a
`GateOverrideRecorded` event in the JSONL log. The event is distinct from
`EvidenceSubmitted` and `DecisionRecorded`.

**R2: Mandatory rationale.** The `--override-rationale` flag takes a non-empty
string argument. Providing the flag is both the override signal and the
rationale in one. No separate `--override` or `--rationale` flags exist.

**R3: Gate failure context in the override event.** The `GateOverrideRecorded`
event includes: the state name, which gates failed (names and result details),
and the rationale string.

**R4: Override event is self-contained.** A consumer reading a single
`GateOverrideRecorded` event can answer: what state, which gate(s) failed, why
they failed (exit code, timeout, error), and why the agent overrode. No
correlation with other events is needed.

**R5: Engine-level override flag.** `koto next` accepts
`--override-rationale <string>`. The flag tells the engine to bypass failed
gates and advance, and its argument is the rationale. It can be combined with
`--with-data` when the state also needs evidence for transition routing.
`--override-rationale` on a non-gate-blocked state is a no-op (no error, no
override event).

**R6: Every gate-blocked state is overridable.** `--override-rationale` works on any
state where gates have failed, regardless of whether the template declares an
`accepts` block. Override is an engine capability, not a template schema
concern. States with gates but no `accepts` block (which today can only be
unblocked via `--to`) become overridable.

**R7: Cross-epoch override query.** A `derive_overrides` function returns all
`GateOverrideRecorded` events across the full session, not scoped to the
current epoch. This follows the existing `derive_*` pattern in persistence.rs.

**R8: CLI query surface.** `koto overrides list` returns all override events
for a workflow, formatted as JSON. Supports the "all overrides in session"
query pattern.

**R9: Normal evidence is unaffected.** Evidence submitted via `--with-data`
on states where gates pass (or states without gates) doesn't require
`--override-rationale` and doesn't produce override events. The
override mechanism only triggers when `--override-rationale` is used on a gate-blocked
state.

**R10: Partial gate failure handling.** When a state has multiple gates and
some fail while others pass, the override event lists all failed gates.
`--override-rationale` bypasses all failed gates simultaneously (no per-gate
granularity).

### Non-functional

**R11: Backward compatibility.** Existing workflows without override events
continue to function. Old state files without `GateOverrideRecorded` events
are valid. Templates that currently use `override` as an evidence enum value
continue to work -- `--with-data '{"status": "override"}'` without
`--override-rationale` is still plain evidence submission.

**R12: Event ordering.** When `--override-rationale` is combined with
`--with-data`, `EvidenceSubmitted` and `GateOverrideRecorded` are emitted in
strict sequence (evidence first, override second) within the same invocation.
When `--override-rationale` is used alone, only `GateOverrideRecorded` is
emitted.

## Acceptance criteria

- [ ] `--override-rationale ""` (empty string) returns a validation error
  (exit code 2)
- [ ] `--override-rationale "reason"` on a gate-blocked state emits a
  `GateOverrideRecorded` event and advances past the failed gates
- [ ] `GateOverrideRecorded` event contains: state, failed gate names, gate
  result details (exit code/timeout/error), and rationale string
- [ ] `--override-rationale` on a gate-blocked state with no `accepts` block works --
  the engine bypasses gates and advances to the next state
- [ ] `--override-rationale "reason" --with-data '{"status": "override"}'`
  on a state with both gates and accepts emits both `EvidenceSubmitted` and
  `GateOverrideRecorded` events, with evidence first (lower sequence number)
- [ ] `--with-data` on a non-gate-blocked state works as today -- no
  `--override-rationale` needed, no `GateOverrideRecorded` emitted
- [ ] `--override-rationale` on a non-blocked state is a no-op (no error, no event)
- [ ] `koto overrides list` returns all override events across the full session
  as JSON
- [ ] Override events survive rewind: if state A is overridden, agent advances
  to B, then rewinds to A, the original override event is still in the log and
  visible via `koto overrides list`
- [ ] Re-overriding a state after rewind produces a new, separate override event
- [ ] Existing workflows that use `--with-data '{"status": "override"}'`
  without `--override-rationale` continue to work (backward compatible plain evidence)
- [ ] State with 3 gates where 2 fail: override event lists both failed gates
  with their individual results
- [ ] `derive_overrides` returns all `GateOverrideRecorded` events across
  epochs, including events from states that were later rewound past
- [ ] `--override-rationale` on a gate-blocked state where the transition can't resolve
  does NOT emit `GateOverrideRecorded` (override event only on successful
  transition per D5)

## Out of scope

- **Visualization UI.** This PRD covers the data persistence and query layer.
  Visualization is a future consumer.
- **Redo/rewind triggered by override disagreement.** The override data enables
  this, but the redo mechanism is future work.
- **Evidence verification by koto.** Koto doesn't yet independently verify
  evidence (polling CI, parsing files). When it does, the override concept
  gains sharper meaning. For now, "override" means "agent explicitly chose to
  bypass failed gates."
- **`--to` directed transition tracking.** Directed transitions bypass all
  gates but are a separate mechanism (explicit state jump, no evidence
  submission). Tracking `--to` as an override-like event is deferred.
- **Action skip tracking.** Evidence presence causes default actions to be
  skipped (independent of gate state). Auditing action skips is related but
  distinct work.
- **`required_when` conditional validation.** General conditional field
  requirements in the template schema. Override and rationale are engine-level
  flags, not schema concerns.
- **Per-gate override granularity.** When multiple gates fail, evidence
  overrides all of them. Selective per-gate override is deferred.

## Known limitations

- **Override detection depends on gate evaluation timing.** Gates are evaluated
  once per `koto next` invocation. If gate state changes between invocations
  (e.g., CI goes green), the second invocation won't see a gate failure and
  won't require rationale. The override event is a point-in-time snapshot.
- **Rationale quality is unvalidated.** The engine requires a non-empty string
  but can't assess whether the rationale actually justifies the override. This
  is a human review concern, not a machine validation concern.
- **No link between override and decision events.** Override rationale and
  `koto decisions record` entries are independent. An agent might record both
  for the same action. Deduplication is a consumer concern.

## Decisions and trade-offs

**D1: Dedicated event type, not reuse of decisions subsystem.** Override
rationale could flow through `DecisionRecorded` (agent-initiated, epoch-scoped)
for consistency. We chose a dedicated `GateOverrideRecorded` event because
overrides are engine-detected (not agent-initiated), need cross-epoch
queryability, and are conceptually distinct from agent deliberation. Mixing them
would complicate queries and blur the semantic boundary.

**D2: Engine-level flag eliminates template workarounds.** Today, template
authors working with deterministic gates must add `accepts` blocks with
`override` enum values and matching conditional transitions -- boilerplate
that exists only because the engine has no override mechanism. The
`--override-rationale` flag makes all of that unnecessary. Override becomes
an engine capability: the flag neutralizes gate failure, and normal transition
resolution proceeds as if gates had passed. Templates with deterministic gates
can be simplified to just gates + an unconditional transition. A single flag
(not separate `--override` + `--rationale`) eliminates invalid combinations.

**D3: Scope to gate overrides only.** The codebase has three implicit override
mechanisms: gate bypass, action skipping via evidence presence, and `--to`
directed transitions. We scoped to gate bypass because it's the primary user
need (issue #108), the most common pattern in workflow skills, and the cleanest
to define. Action skipping and `--to` tracking are noted as future work.

**D4: `--override-rationale` and `--with-data` can be combined.** When a state
has both gates and an accepts block, the agent may need to override the gate
AND provide evidence for transition routing. Making them mutually exclusive
would force agents to use `--to` in these cases, losing the rationale.
Allowing combination keeps the override audit trail intact.

**D5: Override event emitted only on successful transition.** An override event
could be emitted whenever `--override-rationale` is used (even if the transition can't
resolve). We chose to emit only when the override succeeds because a failed
attempt doesn't actually override anything. The agent will retry, producing a
new override event on success.
