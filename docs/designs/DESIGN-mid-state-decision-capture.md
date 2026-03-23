---
status: Planned
spawned_from:
  issue: 68
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-shirabe-work-on-template.md
problem: |
  During long-running judgment states like implementation and analysis, agents make
  non-obvious choices — assumptions about API behavior, tradeoff decisions, approach
  pivots — that are currently buried in reasoning traces. koto has no way to accept
  structured records of these decisions mid-state without triggering the advancement
  loop. Evidence submission always resolves transitions, so agents can only record
  decisions at the moment they're ready to leave the state. By then, the decisions
  are reconstructed from memory rather than captured as they happen.
decision: |
  A new koto decisions subcommand with record and list sub-operations appends a
  DecisionRecorded event to the state file without running the advancement loop. A
  koto decisions list sub-operation retrieves accumulated decisions as a standalone
  response. Decision capture is advisory — no template enforcement, no minimum counts.
rationale: |
  A dedicated event type keeps decisions out of the evidence merge path, preventing
  accidental transition triggers. The sub-operation pattern (koto decisions record,
  koto decisions list) follows the precedent set by koto template compile/validate —
  related operations grouped under one namespace. Advisory enforcement avoids stateful
  validation complexity for a mechanism that can't distinguish meaningful decisions from
  trivial ones.
---

# DESIGN: mid-state decision capture

## Status

Planned

## Context and Problem Statement

koto's evidence submission model is tightly coupled to state advancement. When an agent
calls `koto transition --with-data`, the engine validates the evidence, appends an
`evidence_submitted` event, and immediately runs the advancement loop (gate checks,
transition resolution). There's no way to record structured data mid-state without
risking an unintended transition.

This matters for judgment states where agents work for extended periods — writing code,
researching approaches, creating plans. During this work, agents make decisions that
shape the outcome: "the API doesn't support batch operations, so I'll iterate instead,"
"this test framework doesn't support mocking at this level, I'll use integration tests,"
"the design says X but the code suggests Y, going with Y." These decisions are currently
invisible to anyone reviewing the work — they're in the agent's reasoning, not in koto's
event log.

The parent design (DESIGN-shirabe-work-on-template) identifies this as a cross-cutting
engine concern: the `implementation` and `analysis` states in the work-on template both
need decision capture, and any future template with long-running judgment states will
benefit from the same mechanism.

The specific requirements from the parent design:
- Agents submit decision records mid-state without triggering transitions
- Records include at minimum: `choice`, `rationale`, `alternatives_considered`
- koto persists decisions in the event log and surfaces them to the user
- Templates can optionally require decision capture before allowing completion
- The mechanism is compatible with the existing evidence submission flow

## Decision Drivers

- **Decoupled from advancement**: submitting a decision must not trigger the advancement
  loop — the agent stays in the current state
- **Structured and queryable**: decisions are typed records in the event log, not freeform
  text — they can be filtered, counted, and displayed
- **Minimal engine surface**: the change should be small and targeted — one new event type
  or CLI flag, not a new subsystem
- **Backwards compatible**: existing templates and workflows must work unchanged
- **Surfaceable**: accumulated decisions must be retrievable — via a dedicated
  subcommand, `koto query`, or similar mechanism
- **Rewind-safe**: rewinding past a state discards its decisions, consistent with how
  evidence events work today

## Considered Options

### Decision 1: How decision records enter the system and are stored

Agents working through long-running states need to persist structured decision records
in koto's event log without triggering the advancement loop. The mechanism must be
general-purpose — any template with judgment states can use it, not just the work-on
template. The question is where in the CLI surface this belongs and what event format
to use.

#### Chosen: New `koto decisions record <name> --with-data` sub-operation with a `DecisionRecorded` event type

A new `koto decisions record <name> --with-data '{...}'` sub-operation records a decision
without advancing state. The handler (`handle_decisions_record`):

1. Loads the state file and verifies the template hash (shared setup, factored into a
   common function with `handle_next`).
2. Validates the payload against a fixed decision schema: `choice` (string, required),
   `rationale` (string, required), `alternatives_considered` (array of strings, optional).
3. Appends a `DecisionRecorded` event to the state file with fields `state` (current
   state name) and `decision` (the validated payload).
4. Returns a confirmation with the current state name and decision count for the epoch.
5. Does NOT run the advancement loop.

The `Command` enum gets a `Decisions` variant with a `DecisionsCommand` sub-enum
containing `Record` and `List`, following the `Template`/`TemplateCommand` pattern. The
`Record` variant takes a positional `name` argument and a required `--with-data` flag.
No mutual exclusivity concerns — the argument space is self-contained.

#### Alternatives Considered

- **Top-level `koto record` subcommand.** Shorter command, but splits related operations
  (recording and listing) across two separate namespaces. Doesn't signal the relationship
  between recording and retrieval to users scanning `koto --help`. Rejected for splitting
  related operations.

- **`--record` flag on `koto next`.** Keeps the CLI at six commands, but overloads
  "next" (which means "advance" or "tell me what's next") with an operation that
  explicitly doesn't advance. The mutual exclusivity matrix grows (`--record` + `--to`
  is an error, `--record` without `--with-data` is an error). `koto next --to` already
  stretched the semantics; adding `--record` stretches further. koto's other commands
  are named for their action (init, cancel, rewind) — recording should follow the same
  pattern. Rejected for semantic mismatch.

- **Reuse `EvidenceSubmitted` with a `--no-advance` flag.** Zero engine changes, but
  decision fields would enter `merge_epoch_evidence()` and could accidentally match
  transition `when` conditions, causing silent wrong-state advancement. No naming
  convention can make this safe — one missed prefix causes a hard-to-debug routing
  failure.

- **Generic `koto annotate` with an `AnnotationRecorded` event.** Maximum flexibility,
  but over-generalizes. A generic annotation type can't enforce decision structure
  (choice + rationale), and the flexibility invites schema drift between agents. If a
  second annotation use case emerges, an `AnnotationRecorded` event can be added
  alongside `DecisionRecorded` at that point.

### Decision 2: How accumulated decisions are retrieved and displayed

Once decisions are recorded in the event log, consumers need to read them back —
primarily the skill wrapper (shirabe), which already calls `koto next` in a loop
and parses the JSON response. The parent design constrains the CLI surface: no new
subcommands.

#### Chosen: `koto decisions list <name>` sub-operation

A new `koto decisions list <name>` sub-operation retrieves accumulated decisions for the
current state's epoch as a standalone JSON response. This is a separate command invocation,
not a flag on `koto next`.

The response contains:
- `state`: the current state name
- `decisions.count`: number of decision events in the current state's epoch
- `decisions.items`: array of decision records, each with `choice` and `rationale`

Example response:
```json
{
  "state": "implementation",
  "decisions": {
    "count": 2,
    "items": [
      {
        "choice": "Use feature flags for rollout",
        "rationale": "Lower risk, can revert without deploy"
      },
      {
        "choice": "Skip database migration",
        "rationale": "Data volume too small to justify schema change"
      }
    ]
  }
}
```

#### Alternatives Considered

- **`--decisions` flag on `koto next`.** Keeps retrieval on an existing command, but
  overloads "next" with a read operation that has nothing to do with advancement. The
  design's own Decision 1 rejected `--record` on `koto next` because recording "explicitly
  doesn't advance" — the same argument applies to retrieval. Rejected for the same
  semantic mismatch identified in Decision 1.

- **Always include decisions in `koto next` response.** Simplest implementation, but
  permanently enlarges every response and breaks backwards compatibility. Most `koto next`
  calls during gate evaluation and auto-advance don't need decision data.

- **New `koto query` command.** Clean read/advance separation, but requires skill wrappers
  to make a second CLI call with a different response format. The `derive_decisions()`
  function built here is reusable if a `koto query` command is added later.

### Decision 3: Whether the template enforces decision capture

The engine's evidence validation is currently stateless: `validate_evidence` checks a
single JSON payload against the state's `accepts` schema without accessing the event
log. Adding enforcement would make validation stateful, requiring access to prior epoch
events to count decision submissions.

Note: early analysis of this question assumed decisions would use `EvidenceSubmitted`
events, but Decision 1 chose a separate `DecisionRecorded` event type. This correction
is reflected below — enforcement would need to count `DecisionRecorded` events
specifically, not evidence events.

#### Chosen: Purely advisory (no enforcement mechanism)

Decision capture is advisory. No `min_decisions` field, no `decisions_required` boolean,
no engine validation of decision counts. The template directive instructs agents to
capture decisions; the `koto decisions record` command provides the tool. The engine records whatever
the agent submits without gatekeeping on count or content.

Three factors converge on advisory over enforcement:

1. **Enforcement doesn't prevent gaming.** A `min_decisions` check counts submissions but
   can't evaluate whether the decisions are meaningful. An agent that submits three trivial
   records passes the check. Enforcement creates the appearance of rigor without the
   substance.

2. **YAGNI and complexity cost.** Enforcement requires making evidence validation stateful
   (event log access), adding a new `TemplateState` field, extending the template compiler,
   and updating the CLI error path. That's real complexity for speculative value.

3. **Consistency with koto's validation model.** koto validates evidence structure (types,
   required fields, enum values) but not evidence meaning. A `rationale: string` field
   passes validation whether it contains a thoughtful explanation or "n/a." Advisory is
   consistent with this boundary.

The upgrade path is straightforward: if usage patterns later show that agents consistently
skip decision capture despite directive instructions, a `min_decisions` field can be added
to `TemplateState` without breaking existing templates (it would default to 0).

#### Alternatives Considered

- **`min_decisions` field (engine enforcement).** Adds a `min_decisions: u32` to
  `TemplateState`; engine counts epoch `DecisionRecorded` events before accepting
  completion evidence. Rejected because it makes evidence validation stateful and can't
  distinguish meaningful from trivial decisions.

- **`decisions_required` boolean (soft warning).** Adds a flag to `TemplateState`;
  `koto next` includes a notice when decisions are expected. Rejected because a schema
  field with no runtime enforcement effect is unusual in koto's model, and the directive
  text already serves this purpose.

## Decision Outcome

The three decisions compose into a simple interaction pattern: agents record decisions
with `koto decisions record` during a state, and consumers retrieve them with
`koto decisions list` when they need the trail.

### CLI interaction pattern

**Recording a decision:**
```
koto decisions record my-workflow --with-data '{"choice": "Use retry with backoff", "rationale": "The API has no batch endpoint, rate limits at 100 req/s", "alternatives_considered": ["Parallel requests", "Queue-based processing"]}'
```

The command appends a `DecisionRecorded` event, returns a confirmation with the current
state and decision count, and does not advance the state.

**Retrieving decisions:**
```
koto decisions list my-workflow
```

Returns a standalone JSON response with the current state, count, and items for the
current state's epoch.

### Worked example

An agent is in the `implementation` state, working on a code change. It discovers that
the target API doesn't support batch operations:

```bash
# Agent records the decision as it happens
koto decisions record my-workflow --with-data '{
  "choice": "Iterate with single-item API calls",
  "rationale": "Batch endpoint not available; rate limit is 100/s which is sufficient",
  "alternatives_considered": ["Wait for batch API", "Aggregate client-side"]
}'
# Response: {"state": "implementation", "decisions_recorded": 1}

# Agent continues working, makes another decision
koto decisions record my-workflow --with-data '{
  "choice": "Use integration tests instead of mocks",
  "rationale": "Mock library does not support this HTTP client version"
}'
# Response: {"state": "implementation", "decisions_recorded": 2}

# Later, skill wrapper retrieves decisions for PR summary
koto decisions list my-workflow
# Response: {"state": "implementation", "decisions": {"count": 2, "items": [...]}}
```

On rewind, decisions from the rewound epoch are discarded — the same epoch-boundary
mechanism that scopes evidence applies to `DecisionRecorded` events.

## Solution Architecture

### Components changed

| File | Change |
|------|--------|
| `src/engine/types.rs` | New `EventPayload::DecisionRecorded` variant |
| `src/engine/persistence.rs` | New `derive_decisions()` function |
| `src/cli/mod.rs` | New `Decisions` command variant with `DecisionsCommand` sub-enum (Record, List), handler functions |
| `src/cli/next_types.rs` | `DecisionSummary` struct for `koto decisions list` response |

### New event type: `EventPayload::DecisionRecorded`

Added to the `EventPayload` enum in `src/engine/types.rs`:

```rust
DecisionRecorded {
    state: String,
    decision: serde_json::Value,
}
```

The `type_name()` method returns `"decision_recorded"`. A new deserialization arm in
`Event`'s `Deserialize` impl handles the `"decision_recorded"` type string. The
`decision` field holds the validated JSON payload (choice, rationale,
alternatives_considered).

### New function: `derive_decisions()`

Added to `src/engine/persistence.rs`, parallel to `derive_evidence()`. It finds the
epoch boundary (the most recent state-changing event whose `to` matches the current
state), then collects `DecisionRecorded` events after that boundary.

```rust
pub fn derive_decisions(events: &[Event]) -> Vec<&Event> {
    // Same epoch-boundary logic as derive_evidence(),
    // filtering for DecisionRecorded instead of EvidenceSubmitted
}
```

Rewind naturally discards decisions from prior epochs because the epoch boundary moves
forward on each `Rewound` event.

### CLI: `koto decisions` subcommand with Record and List sub-operations

A new `Decisions` variant in the `Command` enum, following the `Template`/`TemplateCommand`
pattern:

```rust
Decisions {
    #[command(subcommand)]
    subcommand: DecisionsCommand,
}

enum DecisionsCommand {
    Record {
        name: String,
        #[arg(long = "with-data")]
        with_data: String,  // required, not optional
    },
    List {
        name: String,
    },
}
```

The `handle_decisions_record` function:

1. Loads the state file and verifies the template hash (shared with `handle_next` via
   a common `load_workflow` helper).
2. Validates the `--with-data` payload against the fixed decision schema (choice required,
   rationale required, alternatives_considered optional array of strings).
3. Appends `DecisionRecorded { state, decision }` to the state file.
4. Returns `{"state": "<current>", "decisions_recorded": <count>}` — the count is the
   total for this epoch after appending.

No advancement loop. No interaction with `handle_next`.

The `handle_decisions_list` function:

1. Loads the state file and verifies the template hash (shared helper).
2. Calls `derive_decisions()` to collect `DecisionRecorded` events from the current epoch.
3. Returns `{"state": "<current>", "decisions": {"count": N, "items": [...]}}` as a
   standalone response.

### Response shape

The `koto decisions list` response is a standalone JSON object:

```json
{
  "state": "implementation",
  "decisions": {
    "count": 2,
    "items": [
      {
        "choice": "string",
        "rationale": "string",
        "alternatives_considered": ["string", "..."]
      }
    ]
  }
}
```

The `koto next` response shape is unchanged from today — no decision-related fields.

## Implementation Approach

### Phase 1: Core mechanism

Add the event type and recording path.

- Add `DecisionRecorded { state: String, decision: serde_json::Value }` to `EventPayload`
  in `src/engine/types.rs`.
- Add `type_name()` return for `"decision_recorded"`.
- Add deserialization arm for `"decision_recorded"` in `Event`'s `Deserialize` impl.
- Add `DecisionRecordedPayload` helper struct for typed deserialization.
- Add `derive_decisions()` to `src/engine/persistence.rs`.
- Add `Decisions` command variant with `DecisionsCommand` sub-enum (Record, List) to
  `src/cli/mod.rs`, following the `Template`/`TemplateCommand` pattern.
- Factor state-file loading into a shared `load_workflow` helper (used by both
  `handle_next` and `handle_decisions_record`).
- Add `handle_decisions_record`: validate decision schema, append event, return
  confirmation with epoch decision count.
- Unit tests: `DecisionRecorded` round-trip serialization, `derive_decisions()` epoch
  scoping, `derive_decisions()` after rewind, schema validation (missing choice, missing
  rationale, valid with and without alternatives_considered).

### Phase 2: Surfacing

Add the retrieval path.

- Add `DecisionSummary` struct to `src/cli/next_types.rs`.
- Add `handle_decisions_list`: load workflow, call `derive_decisions()`, return standalone
  JSON response with state, count, and items.
- Integration test: init workflow, record two decisions, call `koto decisions list`,
  verify response includes both decisions with correct count.

## Security Considerations

Decision records are user-controlled strings stored in a local JSONL file. The threat
model is identical to evidence submission via `--with-data`:

- No network calls. All data stays on the local filesystem.
- No untrusted input beyond what agents already submit through `--with-data`.
- The existing 1 MB payload size limit (`MAX_WITH_DATA_BYTES`) applies to
  `koto decisions record` payloads, preventing unbounded file growth from a single
  submission.
- State files are created with mode 0600 (owner read/write only), same as today.

No additional security measures are needed.

## Consequences

### Positive

- Agents can record decisions incrementally during long-running states. The event log
  captures the full decision trail, scoped to the current epoch.
- The `koto decisions` namespace groups related operations (record and list) under one
  command, following the `koto template` precedent.
- Rewind naturally discards decisions from rewound epochs — no special cleanup logic.
- The `derive_decisions()` function is reusable by a future `koto query` command.
- `koto next` stays focused on advancement — no decision-related flags or response fields.

### Negative

- koto's CLI grows from six to seven top-level commands (counting `Decisions` as one
  entry with two subcommands). `koto next` stays at three modes (bare, `--with-data`,
  `--to`).
- The fixed decision schema (choice + rationale + alternatives_considered) may be too
  rigid for some templates. Per-state decision schemas would require adding a `decisions`
  block to the template format.
- Decision capture quality depends entirely on directive text quality. If the directive
  doesn't explain what constitutes a decision worth recording, agents won't record
  useful decisions.

### Mitigations

- The `koto decisions` subcommands have self-contained argument spaces, no mutual
  exclusivity concerns with other commands.
- The schema can be extended later (new optional fields) without breaking existing
  decision records, since the `decision` field stores `serde_json::Value`.
- Template authors control directive text. The work-on template's `implementation`
  directive will include specific guidance on what decisions to capture (API assumptions,
  tradeoff choices, approach changes).
