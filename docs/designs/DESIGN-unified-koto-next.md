---
status: Proposed
upstream: docs/prds/PRD-unified-koto-next.md
problem: |
  koto's three core systems — CLI contract, state model, and workflow definition format —
  must change together to support a unified `koto next` command. The CLI contract must
  become self-describing so agents never need out-of-band knowledge. The state model must
  support per-state evidence scoping so evidence doesn't contaminate branching or looping
  workflows. The template format must support per-transition conditions so workflows can
  branch on what agents submit. These systems are interdependent; the design must make the
  unifying architectural choice that governs how they fit together.
decision: |
  Redesign koto around an event-sourced state machine. The state file becomes an
  append-only JSONL event log; current state and per-state evidence are derived by replay.
  The template format adds `accepts`/`when` blocks for evidence schema and routing
  conditions. `koto next` output gains an `expects` field — a self-describing schema
  of what event the agent should submit next. Four tactical sub-designs implement each
  system boundary once the shared event taxonomy is accepted.
rationale: |
  The event log model produces the best architecture for koto's correctness requirements.
  It buys structural evidence scoping (no policy-based clearing), simpler atomicity
  (append, not rewrite), and a first-class audit trail. Protocol-first and
  declarative-language-first approaches treat the output schema or template format as the
  design constraint, producing a weaker architecture. Minimal extension leaves two
  coexisting transition syntaxes and policy-based evidence clearing as ongoing maintenance
  burdens.
---

# DESIGN: Unified koto next Command

## Status

Proposed

## Context and Problem Statement

koto's current architecture treats state reading and state advancement as separate
operations. Agents call `koto next` to read the current directive, then call
`koto transition <target>` to advance — and must know which to call and when. As new
capabilities are added (evidence submission, delegation, per-transition conditions),
this model breaks down: the set of valid operations at any point grows, agents must track
it themselves, and each new capability adds a new command.

Three systems need to change together to fix this. The **CLI contract** — what `koto next`
accepts and returns — must become self-describing so agents never need out-of-band
knowledge. The **state model** — what koto persists between calls — must support per-state
evidence scoping so evidence doesn't contaminate branching or looping workflows. The
**workflow definition format** — how developers author koto templates — must support
per-transition conditions so workflows can branch based on what agents submit without
agents naming target states.

These systems are interdependent: the CLI contract depends on what state is stored, and
state storage depends on what templates can declare. But each is large enough to warrant
its own tactical design. This document makes the three unifying high-level decisions and
defines the constraints each tactical design must satisfy.

## Decision Drivers

- **Agent contract stability**: the CLI surface (`koto next` input/output schema) must
  stay constant as capabilities are added; tactical sub-designs must not require flag or
  schema changes visible to agents
- **Self-describing output**: an agent that has never seen the workflow template must be
  able to determine its next action from the `koto next` response alone
- **Sub-design independence**: tactical designs for CLI contract, state model, and
  template format must be implementable in sequence without circular dependencies
- **Evidence correctness**: per-state evidence scoping is required for correctness with
  directed transitions and looping workflows — the design must specify the scoping model,
  not defer it
- **Template authoring ergonomics**: workflow developers need a template format that's
  readable and writable without understanding koto internals
- **Testability**: every system boundary (CLI output, state persistence, template
  compilation) must be independently testable

## Considered Options

### Decision: Design philosophy for unifying CLI contract, state model, and workflow definition

koto needs three interdependent systems to change together: the CLI contract (what `koto
next` outputs and accepts), the state model (what koto persists between calls), and the
workflow definition format (what templates declare). The question isn't just what to change
in each — it's what philosophy should govern how they fit together. Choose the wrong anchor
and the three systems pull in different directions. Choose right and each system's design
follows naturally from the others.

This design introduces breaking changes to both the state file format and the template
format. koto has not shipped a stable release, so no existing users are affected and the
changes are made without migration concerns.

#### Chosen: Event-Sourced State Machine

The state file becomes an append-only event log. Every state transition — whether triggered
by auto-advancement, evidence submission, or a human-directed override — is recorded as an
immutable typed event with a sequence number, timestamp, event type, and payload. Current
state is derived by reading the final transition event in the log; there is no mutable
`CurrentState` field to synchronize. Evidence is no longer a global accumulated map — it
lives inside the events that submitted it, scoped by definition to the state in which it
was submitted.

This changes the three systems in a coherent way. The **state model** becomes an event log
schema: what event types exist, what each carries, how sequence numbers work, how snapshots
accelerate replay for long logs. The **template format** declares event schemas: per-transition
conditions become per-event-type schemas that describe what payload triggers which transition.
The **CLI contract** describes what event the current state expects next: the `expects` field
in `koto next` output specifies an event type and its input schema, derived from the template's
event declarations.

Per-state evidence scoping — one of the most critical correctness requirements in the PRD —
is structural rather than enforced. There is no global evidence map to clear. Evidence
submitted while in state A is in the event log as "submitted while in state A." A transition
event that moves to state B doesn't reference state A's evidence; it simply doesn't contain
it. Template authors don't need to reason about clearing semantics because the log model
makes contamination impossible by construction.

The audit trail becomes first-class. The event log is the state file; replaying it yields
not just current state but the complete, timestamped record of every action taken in the
workflow. Recovery is replay from the last valid event — no partial-mutation edge cases.
Atomic writes simplify: appending an event is simpler than the current temp-file-rename
pattern because an append either succeeds fully or fails detectably (sequence number gap).

Both the state file format and the template format are breaking changes. This is intentional
— koto has no released users and no existing workflows to preserve.

#### Alternatives Considered

**Protocol-First**: Design the `koto next` JSON output schema first; derive state model and
template format from what the protocol needs to produce. Strong at creating a stable agent
contract, but doesn't solve the fundamental model problem — it specifies what the output
looks like without specifying how state is structured. The event-sourced model produces a
better protocol as a consequence of its structure, rather than treating the protocol as a
constraint to design around.

**Declarative Language First**: Design the template format as the primary artifact; derive
state model and CLI output from template declarations. Creates one source of truth for
workflow semantics, but evidence schema expressiveness is limited by what YAML can declare
cleanly. Complex schemas require an embedded DSL. The event-sourced model handles schema
declaration through event types, which are a more natural fit for the "what does this
transition accept?" question than YAML blocks.

**Minimal Extension**: Extend the existing model with the minimum changes required —
backward-compatible template additions, policy-based evidence clearing, optional `expects`
declarations. Fastest to ship, but leaves two coexisting transition syntaxes and
policy-based evidence clearing as ongoing maintenance burdens. Rejected because the weaker
model produces no architectural benefit; the event-sourced approach costs the same (both
are breaking changes at this stage) and buys structural correctness guarantees.

## Decision Outcome

**Chosen: Event-Sourced State Machine**

### Summary

koto's state file changes from a mutable document (`CurrentState`, `Evidence`, `History`)
to an append-only event log. Every state change is a typed, immutable event: `workflow_initialized`,
`transitioned`, `evidence_submitted`, `directed_transition`. Each event carries a sequence
number, timestamp, event type, and type-specific payload. Current state is the `to` field
of the last `transitioned` event. Evidence for a state is the union of all
`evidence_submitted` events whose `state` field matches the current state. There is no
global evidence map; per-state scoping is a structural property of the log.

The template format adds event schema declarations alongside the existing state and
transition structure. A state that expects evidence submission declares the event schema —
what fields the `evidence_submitted` payload must contain and what per-transition `when`
conditions determine which outgoing transition fires. These declarations drive two things:
(1) the `koto next` output `expects` field, which is computed from the current state's
event schema and presented to the agent as a self-describing contract, and (2) payload
validation when the agent submits via `--with-data`.

`koto next` output gains `advanced: bool`, structured `error` (with code and message),
and `expects` (with event type, field schema, and per-transition options). The `--with-data`
flag submits an `evidence_submitted` event; `--to` submits a `directed_transition` event.
Both are appended to the log and trigger re-evaluation. Auto-advancement chains through
states by appending `transitioned` events until a stopping condition; each event in the
chain is independently durable. A crash mid-chain leaves the log at the last valid event;
resuming replays from there.

The three tactical sub-designs are: (1) event log format and state file schema, (2) template
format event schema declarations and compilation pipeline, (3) CLI contract — the `koto next`
output schema including `expects`, error codes, and integration output. A fourth sub-design
covers the auto-advancement engine: replay, current-state derivation, loop, stopping
conditions, and integration invocation. Each sub-design can proceed once the event log
format is accepted, since all three depend on the event type taxonomy.

### Rationale

The event-sourced model produces the right architecture for koto's correctness requirements.
The mutable-state model requires policy-based evidence clearing (clear the map on each
transition) and careful atomicity to avoid partial mutations. The event log model makes
both unnecessary: evidence scoping is structural (events are immutable and state-tagged),
and writes are appends (simpler atomicity guarantees). These aren't minor improvements —
they eliminate entire categories of bugs and simplify the recovery story for long-running
workflows. The state file and template format changes are breaking, which is acceptable
at koto's current pre-release stage.

The approach also aligns the three systems around a single concept. Every API boundary in
koto — what templates declare, what the state file stores, what `koto next` outputs — is
expressed in terms of events and their schemas. This makes the tactical sub-designs coherent:
each one is specifying a different view of the same event taxonomy.

## Solution Architecture

### Overview

koto's three core systems — state persistence, workflow definition, and CLI contract — are
redesigned around a shared event taxonomy. Every operation on a workflow produces a typed,
immutable event appended to the state file. Templates declare what events each state expects
and what conditions route them to outgoing transitions. `koto next` output describes what
event the agent should submit next.

### Event Taxonomy

Six event types cover all workflow operations:

| Event type | Triggered by | Key payload fields |
|-----------|-------------|-------------------|
| `workflow_initialized` | `koto init` | `workflow`, `template_hash`, `variables` |
| `transitioned` | auto-advancement | `from`, `to`, `condition_type` |
| `evidence_submitted` | `koto next --with-data` | `state`, `fields` (key-value map) |
| `directed_transition` | `koto next --to` | `from`, `to`, `directed: true` |
| `integration_invoked` | processing integration stop | `state`, `integration`, `output` |
| `rewound` | `koto rewind` | `from`, `to` |

All events share: `seq` (monotonic integer), `timestamp` (RFC 3339), `type` (string).

### State File Format

The state file changes from a mutable JSON object to a JSONL event log:

```
{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123","created_at":"..."}
{"seq":1,"timestamp":"...","type":"workflow_initialized","payload":{"variables":{}}}
{"seq":2,"timestamp":"...","type":"transitioned","payload":{"from":null,"to":"gather_info","condition_type":"auto"}}
{"seq":3,"timestamp":"...","type":"evidence_submitted","payload":{"state":"gather_info","fields":{"input_file":"results.json"}}}
{"seq":4,"timestamp":"...","type":"transitioned","payload":{"from":"gather_info","to":"analyze","condition_type":"gate"}}
```

- First line: header object (`schema_version`, `workflow`, `template_hash`, `created_at`)
- Subsequent lines: events, one per line, in append order
- **Current state**: `to` field of the last `transitioned` or `directed_transition` event
- **Current evidence**: `evidence_submitted` events occurring after the most recent
  `transitioned` event whose `payload.to` matches the current state — this is the epoch
  boundary rule. Evidence from prior visits to the same state (in looping workflows) is
  archived in the log but excluded from the current evidence set. Only the evidence
  accumulated since the last arrival at this state is active.
- **Atomicity**: each event is appended with fsync; a sequence number gap detects partial writes
- **Breaking change**: the existing mutable JSON state file format is replaced entirely; this
  is intentional and acceptable at koto's pre-release stage

### Template Format

Template YAML frontmatter adds two new blocks per state: `accepts` (evidence field schema)
and per-transition `when` conditions. Existing `gates` (command gates, field checks) remain
unchanged — they're for koto-verifiable conditions; `accepts`/`when` are for agent-submitted
evidence.

```yaml
states:
  analyze_results:
    # Evidence field declarations (generates `expects` in koto next output)
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
      rationale:
        type: string
        required: true

    # Per-transition routing conditions
    transitions:
      - target: deploy
        when:
          decision: proceed
      - target: escalate_review
        when:
          decision: escalate

    # Koto-verifiable condition (not agent-submitted)
    gates:
      tests_passed:
        type: command
        command: ./check-ci.sh

    # Processing integration (string tag; routing lives in user config)
    integration: delegate_review
```

States with no `accepts` block and no `when` conditions are auto-advanced through when
their `gates` are satisfied. The template compiler validates that per-transition `when`
conditions on the same state are mutually exclusive (same field, disjoint values) and
rejects templates that are non-deterministic. The template format is a breaking change
from the current YAML structure; acceptable at koto's pre-release stage.

### CLI Output Schema

`koto next` returns a JSON object. The schema varies by stopping condition:

**Normal execution / stopped at a state requiring evidence:**
```json
{
  "action": "execute",
  "state": "analyze_results",
  "directive": "Review the test output...",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": { "type": "enum", "values": ["proceed", "escalate"], "required": true },
      "rationale": { "type": "string", "required": true }
    },
    "options": [
      { "target": "deploy", "when": { "decision": "proceed" } },
      { "target": "escalate_review", "when": { "decision": "escalate" } }
    ]
  },
  "error": null
}
```

**Stopped at a state with only koto-verifiable gates (no agent submission needed):**
```json
{
  "action": "execute",
  "state": "wait_for_ci",
  "directive": "Waiting for CI to pass...",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    { "name": "tests_passed", "type": "command", "agent_actionable": false }
  ],
  "error": null
}
```

**Stopped at a processing integration:**
```json
{
  "action": "execute",
  "state": "delegate_analysis",
  "directive": "Deep analysis required.",
  "advanced": true,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": { "interpretation": { "type": "string", "required": true } }
  },
  "integration": {
    "name": "delegate_review",
    "output": "..."
  },
  "error": null
}
```

**Terminal state:**
```json
{ "action": "done", "state": "complete", "advanced": true, "expects": null, "error": null }
```

**Error response:**
```json
{
  "error": {
    "code": "invalid_submission",
    "message": "Missing required field: rationale",
    "details": [{ "field": "rationale", "reason": "required but not provided" }]
  }
}
```

Error codes: `gate_blocked`, `invalid_submission`, `precondition_failed`,
`integration_unavailable`, `terminal_state`, `workflow_not_initialized`.

Exit codes: `0` success, `1` transient (gates blocked, integration unavailable, version
conflict), `2` caller error (bad input, invalid submission), `3` config error (corrupt
state, invalid template, not initialized).

**`koto transition` removal**: the existing `koto transition <target>` command is removed
by this design. Directed transitions are submitted via `koto next --to <target>` as part
of the unified command. This is a breaking change; acceptable at koto's pre-release stage.

### Data Flow

```
koto next [--with-data data.json] [--to target]
  │
  ├─ Load state file (JSONL): derive current state + current evidence by log replay
  ├─ Load compiled template (SHA-256 cache)
  │
  ├─ If --with-data:
  │   ├─ Validate payload against current state's `accepts` schema
  │   ├─ Append evidence_submitted event → fsync
  │   └─ Continue to advancement evaluation
  │
  ├─ If --to:
  │   ├─ Validate target is a valid outgoing transition
  │   ├─ Append directed_transition event → fsync
  │   └─ Return immediately (always stops after directed)
  │
  ├─ Advancement loop:
  │   ├─ visited := {}
  │   └─ loop:
  │       ├─ current := last transitioned.to
  │       ├─ if visited[current]: stop (cycle detected)
  │       ├─ visited[current] = true
  │       ├─ if terminal: stop
  │       ├─ if integration configured: invoke runner, append integration_invoked → stop
  │       ├─ evaluate gates: if any fail → stop (gate_blocked)
  │       ├─ if accepts block: evaluate which transition's `when` conditions match current evidence
  │       │   ├─ if none match: stop (expects evidence submission)
  │       │   └─ if match: append transitioned event → fsync → continue loop
  │       └─ if no accepts and gates pass: append transitioned event → fsync → continue
  │
  └─ Return koto next output JSON
```

### Sub-Design Boundaries

This strategic design spawns four tactical sub-designs, each independently implementable
once the event taxonomy (above) is accepted:

| Sub-design | Scope | Depends on |
|-----------|-------|-----------|
| Event log format | State file schema, JSONL structure, event type definitions | Nothing (foundational) |
| Template format v2 | `accepts` block, `when` conditions, `integration` field, compiler changes | Event taxonomy |
| CLI output contract | `koto next` JSON schema, `expects` derivation, error codes, exit codes | Event taxonomy |
| Auto-advancement engine | Replay logic, loop, stopping conditions, integration invocation, `koto rewind` | All three above |

## Implementation Approach

### Phase 1: Event taxonomy and log format (foundational)

Accept the event type definitions and JSONL format. Everything else builds on this.

Deliverables:
- Event taxonomy document (6 event types, all fields, sequence semantics) — including
  the `integration_invoked` output field type, which Phase 3 needs for the CLI schema
- State file schema specification (JSONL header + event lines)
- Epoch boundary rule for evidence replay (evidence scoped to most recent arrival at
  each state, not all historical visits)
- JSONL vs. JSON-array evaluation (the tactical sub-design should document this
  trade-off explicitly before committing to the line-by-line reader approach)
- **Tactical sub-design**: Event Log Format

### Phase 2: Template format v2 (parallel with Phase 3)

Add event schema declarations to the template format. Can proceed once Phase 1 is accepted.

Deliverables:
- `accepts` block syntax and field type definitions
- `when` conditions on transitions (replacing `transitions: []string`)
- `integration` field (string tag, config-bound routing)
- Mutual exclusivity validation in compiler
- **Tactical sub-design**: Template Format v2

### Phase 3: CLI output contract (parallel with Phase 2)

Define the `koto next` JSON output schema. Can proceed once Phase 1 is accepted.

Deliverables:
- Complete output schema (`action`, `state`, `directive`, `advanced`, `expects`, `blocking_conditions`, `integration`, `error`)
- `expects` field derivation rules (from template `accepts` + `when`)
- Error code taxonomy and structured error format
- Exit code mapping
- `--with-data` and `--to` flag behavior spec
- **Tactical sub-design**: CLI Output Contract

### Phase 4: Auto-advancement engine (after Phases 1-3)

Implement the advancement loop, replay, and all stopping conditions. Depends on the
accepted outputs of Phases 1-3.

Deliverables:
- Event log reader / replay (current state derivation, current evidence derivation)
- Advancement loop with visited-state cycle detection
- `--with-data` payload validation against `accepts` schema
- `--to` directed transition
- Integration runner interface and invocation
- `koto rewind` as rewound event
- **Tactical sub-design**: Auto-Advancement Engine

## Implementation Language

koto will be implemented in Rust. The event-sourced refactor represents a near-complete
rewrite of core logic — switching languages at the same time does not add significant cost.
The four tactical sub-designs below should target Rust (Cargo workspace, clap v4, serde,
tokio). The external CLI contract (command names, flag names, JSON output schema) is
unchanged; agents must not notice the language switch. A separate migration design covers
the Go→Rust transition, workspace structure, CI changes, and sequencing.

## Required Tactical Designs

| Sub-design | Repo | Scope |
|-----------|------|-------|
| Go→Rust Migration | koto | Cargo workspace layout, crate structure, CI, migration sequencing |
| Event Log Format | koto | State file JSONL schema, event type taxonomy |
| Template Format v2 | koto | `accepts`/`when` syntax, compiler changes, format version |
| CLI Output Contract | koto | `koto next` JSON schema, `expects` derivation, errors |
| Auto-Advancement Engine | koto | Replay, loop, stopping conditions, integration invocation |

## Security Considerations

### Command Gates and Integration Invocation

koto executes arbitrary shell commands via two mechanisms: command gates (evaluate exit
codes to allow transitions) and integration invocation (run user-configured subprocesses
and record output). Both are correct by design when template sources are trusted.

Templates come from two sources in koto's workflow:

- **Plugin-installed templates:** Reviewed as part of the plugin; installation requires
  explicit user action.
- **Project-scoped templates:** Committed to the project repository and reviewed via PR.

**Implementation constraint:** Integration names must resolve from a closed set (project
configuration or plugin manifest), not from arbitrary strings in template files. A template
declaring `integration: some-name` tells koto to route to the configured handler for
`some-name`; the actual command or process is defined in user or project configuration,
not in the template itself.

Command gates already enforce this implicitly: the command string is authored by the
developer who writes the template. If koto is extended to load templates from untrusted
sources, command gates require additional validation.

### Evidence Persistence

The event log persists evidence (agent-submitted data) and integration output as plaintext
JSON. Event logs may contain sensitive data submitted by agents — API keys, credentials,
or sensitive analysis output. Event log files should be protected like any file containing
secrets. They are not suitable for committing to public repositories.

Integration output stored in `integration_invoked` events should be:

- Validated against size limits before storage to prevent log bloat
- Treated as untrusted if used in downstream interpolation contexts
- Subject to schema validation if the integration is expected to return structured data

Event log files must be created with restricted permissions (mode 0600) to limit
access to the owning user. Evidence validation against the `accepts` schema must occur
before appending the event to the log — not deferred post-storage.

### Template Hash Verification

The design retains SHA-256 hash verification of templates: the `template_hash` field in
the JSONL header ties the event log to the exact template version it was created with.
Replaying events against a modified template is detected and rejected. This is sufficient
to prevent tampered templates from being silently applied to existing workflows.

## Consequences

### Positive

- **Evidence scoping is structural**: per-state evidence is a property of the event log
  model — there is no global evidence map to accidentally contaminate; this eliminates
  an entire class of correctness bugs
- **Audit trail is first-class**: the event log is the state file; every action, its
  inputs, and its timestamp are preserved in replay order; debugging and recovery are
  trivial
- **Simpler atomicity**: appending an event is simpler than the current full-document
  rewrite; a sequence number gap reliably detects partial writes without a separate
  checksum
- **Recovery is well-defined**: replay from the last valid event is standard and
  predictable; no edge cases around partial state mutations
- **Agent contract is explicit**: the `expects` field gives agents a typed schema for
  their next action; agents never need to consult templates or secondary commands
- **Sub-designs are coherent**: all four tactical designs are specifying different views
  of the same event taxonomy; changes to one propagate naturally to the others

### Negative

- **Event replay latency grows with log length**: replaying hundreds of events on every
  `koto next` call adds measurable latency for long-running workflows; snapshot mechanism
  (optional initially) is needed for production workflows that run long
- **Template authoring is more complex**: workflow authors must understand the `accepts`/
  `when` model to write branching workflows; flat `transitions: []string` was simpler
- **Five tactical sub-designs before implementation**: the strategic design requires
  accepting five sub-designs (including the Go→Rust migration) before a single line of
  implementation code is written; the upfront design investment is higher than other approaches

### Mitigations

- **Replay latency**: the initial implementation skips snapshots; a snapshot event type
  is reserved in the event taxonomy so it can be added without a schema change when needed
- **Authoring complexity**: the template format guide includes worked examples for common
  patterns (linear, branching, looping, delegation); the compiler error messages for
  invalid `when` conditions name the conflicting transitions explicitly

