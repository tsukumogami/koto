# Catalog: PRD and Design Requirements for koto Skills

Extracted from all seven source documents. Organized by mapping to skill artifact.

Sources consulted:
- `PRD-gate-transition-contract.md` (status: Implemented)
- `PRD-koto-next-output-contract.md` (status: Done)
- `PRD-koto-user-skill.md` (status: In Progress)
- `PRD-session-persistence-storage.md` (status: In Progress)
- `PRD-unified-koto-next.md` (status: Accepted)
- `DESIGN-unified-koto-next.md` (status: Planned)
- `DESIGN-koto-user-skill.md` (status: Accepted)

---

## (a) Requirements mapping to koto-user SKILL.md

These requirements define what the SKILL.md file itself must cover at top-level — the
content an agent reads first on every session.

### Session lifecycle

**Source: PRD-koto-user-skill R14; DESIGN-koto-user-skill "Content contracts"**

SKILL.md must describe the full session lifecycle in order:
1. `koto init <name> --template <path>` — creates workflow and session
2. `koto next <name>` loop — get directive, do work, advance
3. Evidence submission sub-loop: `koto next <name> --with-data '<json>'`
4. Override sub-loop when gates block: `koto overrides record`, then `koto next`
5. `action: "done"` — workflow complete, session cleanup triggered automatically

### Action dispatch table (all 6 values)

**Source: PRD-koto-next-output-contract R1, R4; PRD-unified-koto-next R9; PRD-koto-user-skill R14**

SKILL.md must contain a dispatch table covering all six `action` values with the
agent behavior for each:

| `action` | Agent behavior |
|----------|----------------|
| `evidence_required` | Read directive; determine sub-case; submit evidence via `--with-data` if needed |
| `gate_blocked` | Inspect `blocking_conditions`; fix or override; call `koto next` again — do not submit evidence |
| `integration` | Process `integration.output`; if `expects` present, submit evidence; if `expects` null, call `koto next` again |
| `integration_unavailable` | Submit evidence if `expects` present; use `--to` to skip; or report runner needs configuration |
| `done` | Stop. Workflow is complete. |
| `confirm` | Read `action_output`; if `expects` present, submit evidence; if null, call `koto next` again |

The `action` field is the authoritative dispatch signal. `advanced` is informational
only and must not be used to determine next action (PRD-koto-next-output-contract R2,
D1).

### Three evidence_required sub-cases

**Source: PRD-koto-user-skill R14; DESIGN-koto-user-skill "Content contracts"; PRD-koto-next-output-contract R6**

SKILL.md must spell out the three `evidence_required` sub-cases with their
distinguishing signals — this is correctness-critical (agents submit evidence in
case (b) that should first handle gates):

- **(a) Agent data needed**: `expects.fields` is non-empty AND `blocking_conditions` is empty.
  Agent does the work and submits evidence.
- **(b) Gate failed with accepts block present**: `blocking_conditions` is non-empty AND
  `expects.fields` is non-empty. Submitting valid evidence that matches a conditional
  transition advances the workflow past the failed gates (gates are not re-evaluated;
  evidence resolves routing directly). Agent can also choose to override the gate
  via `koto overrides record`.
- **(c) Auto-advance candidate**: `expects.fields` is empty AND `blocking_conditions` is empty.
  Engine can advance without agent input. Call `koto next` again to trigger advancement.

### Two-step override flow

**Source: PRD-gate-transition-contract R5, R5a; PRD-koto-user-skill R14; DESIGN-koto-user-skill "Content contracts"**

SKILL.md must document the override flow as exactly two steps, not a single command:

1. `koto overrides record <name> --gate <gate_name> --rationale "reason"`
   (optionally with `--with-data '{"field": "value"}'` to specify non-default values)
2. `koto next <name>` — engine reads override events and substitutes gate output

Key behavioral facts agents need:
- Each `koto overrides record` call overrides exactly one gate (R5a)
- Multiple gates require multiple calls, each with its own rationale
- Override events are sticky within an epoch (persist until state transitions)
- Multiple agents can push overrides independently; orchestrator calls `koto next` when ready
- Without `--with-data`, the gate's built-in `override_default` is applied
- With `--with-data`, agent-supplied data is substituted instead

### `advanced` field definition

**Source: PRD-koto-next-output-contract R2, R3; DESIGN-unified-koto-next post-implementation note**

SKILL.md must state that `advanced` means "at least one state transition occurred during
this invocation." Agents must dispatch on `action`, not on `advanced`. The post-implementation
note in DESIGN-unified-koto-next confirms this was a historical source of confusion (#89).

### Directed transition (`--to`) behavior

**Source: PRD-koto-next-output-contract R7, R8; PRD-unified-koto-next R10a**

SKILL.md should cover `koto next --to <state>` for agents who may receive human
instruction to advance to a specific state:
- Single-shot: does not chain auto-advancement past the target
- Does not evaluate gates on target state
- Must call `koto next` again to trigger advancement loop from there
- `--to` and `--with-data` are mutually exclusive (returns `precondition_failed` exit 2)
- Use case: human supervisor instructs agent to skip past a blocked state

### No phantom commands

**Source: PRD-koto-user-skill R15**

SKILL.md must not reference `koto status` or `koto query` — these commands do not exist
in the CLI. The correct alternatives are:
- Get current state: `koto next <name>` (returns current directive without advancing if nothing changed)
- List active workflows: `koto workflows`

---

## (b) Requirements mapping to koto-user references/

### command-reference.md

**Source: PRD-koto-user-skill R17; DESIGN-koto-user-skill "Content contracts"; PRD-session-persistence-storage R2, R3, R4, R12**

Must document every subcommand relevant to workflow runners. Exhaustive list per R17:

**Workflow lifecycle:**
- `koto init <name> --template <path>` — creates workflow and session (session is 1:1 with workflow)
- `koto next <name>` — get directive, auto-advance if conditions met
- `koto next <name> --with-data '<json>'` — submit evidence
- `koto next <name> --to <state>` — directed transition (bypasses gates)
- `koto next <name> --full` — forces `details` field even on subsequent visits (context recovery)
- `koto cancel <name>` — cancel workflow
- `koto rewind <name>` — roll back to previous state

**Override and decision commands:**
- `koto overrides record <name> --gate <gate> --rationale <text>` — record a gate override
- `koto overrides record <name> --gate <gate> --rationale <text> --with-data '<json>'` — override with explicit data
- `koto overrides list <name>` — list all override events across the session
- `koto decisions record <name> --with-data '<json>'` — record a decision
- `koto decisions list <name>` — list recorded decisions

**Session and context commands:**
- `koto workflows` — list active workflows in the current directory
- `koto session dir <name>` — print session directory path
- `koto session list` — list all local sessions with workflow name and last-modified time
- `koto session cleanup <name>` — remove all artifacts for a session (local and cloud)
- `koto context add <name> --key <key>` — submit workflow context (stdin or `--from-file <path>`)
- `koto context get <name> --key <key>` — retrieve context (stdout or `--to-file <path>`)
- `koto context exists <name> --key <key>` — exit 0 if key exists, non-zero otherwise
- `koto context list <name>` — list all context keys in session

**Note**: `koto context` commands are required because agents hitting a blocking
`context-exists` gate need to know how to add the expected content. Omitting these
leaves a gap in the gate recovery path.

Each file ends with: *"For topics not covered here, see `docs/guides/cli-usage.md`."*

---

### response-shapes.md

**Source: PRD-koto-next-output-contract R1, R6, R9; DESIGN-unified-koto-next CLI Output Schema; DESIGN-koto-user-skill "Content contracts"; PRD-gate-transition-contract R4a**

Must include annotated JSON examples for each scenario below. The sub-case (b) and
`gate_blocked` with `agent_actionable: false` are the correctness-critical cases — these
must not be omitted.

Required scenarios (from DESIGN-koto-user-skill):
1. `evidence_required` — sub-case (a): clean state, agent submits evidence
   - `expects.fields` non-empty, `blocking_conditions` empty
2. `evidence_required` — sub-case (b): gate failed, accepts block present, agent can submit or override
   - `blocking_conditions` non-empty, `expects.fields` non-empty
3. `evidence_required` — sub-case (c): auto-advance candidate
   - `expects.fields` empty, `blocking_conditions` empty
4. `gate_blocked` — `agent_actionable: true`, override is possible
5. `gate_blocked` — `agent_actionable: false`, agent cannot unblock
6. `integration` — external integration running, agent waits
7. `integration_unavailable` — integration not reachable, agent decides
8. `done` — terminal state
9. `confirm` — agent must confirm before transition

**`blocking_conditions` item schema** (from PRD-koto-user-skill R6, DESIGN-unified-koto-next):
```json
{
  "gate": "ci_check",
  "output": {"exit_code": 1, "error": ""}
}
```
Also document: `name`, `type`, `status`, `agent_actionable` (boolean — whether agent can
resolve this gate by submitting evidence or an override).

**`expects` schema structure** (from DESIGN-unified-koto-next CLI Output Schema):
```json
{
  "fields": {
    "decision": { "type": "enum", "values": ["proceed", "escalate"], "required": true },
    "rationale": { "type": "string", "required": true }
  },
  "options": [
    { "target": "deploy", "when": { "decision": "proceed" } },
    { "target": "escalate_review", "when": { "decision": "escalate" } }
  ]
}
```

**`details` field** (from PRD-koto-next-output-contract R9):
- Present on first visit to a state; absent on subsequent visits
- `koto next --full` forces inclusion regardless of visit count (context recovery)
- States without `details` defined: field absent in all responses

**`advanced` field** (from PRD-koto-next-output-contract R2, R3):
- `true` if at least one state transition occurred during this invocation
- `true` always after `--to` (directed transition is always a transition)
- `true` if `--with-data` triggered at least one transition

**Error response shape** (from PRD-koto-next-output-contract R12; DESIGN-unified-koto-next):
```json
{
  "error": {
    "code": "invalid_submission",
    "message": "Missing required field: rationale",
    "details": [{ "field": "rationale", "reason": "required but not provided" }]
  }
}
```
All error responses use this structured `NextError` format (no unstructured shapes).

---

### error-handling.md

**Source: PRD-koto-next-output-contract R5, R12; PRD-unified-koto-next R9, R20; PRD-koto-user-skill R17**

Must document exit codes 0–3 with semantics and correct agent response:

| Exit code | Meaning | Agent response |
|-----------|---------|----------------|
| 0 | Success — directive returned | Continue loop per `action` value |
| 1 | Transient — gates not satisfied, integration unavailable, lock contention | Wait and retry; no operator involvement needed |
| 2 | Caller error — bad input, invalid submission, precondition failed | Fix input and retry |
| 3 | Config/template error — corrupt state, bad template, not initialized | Report to user; agent cannot fix |

**Error codes and their exit codes:**

| Code | Exit | Meaning |
|------|------|---------|
| `precondition_failed` | 2 | Bad flags (--with-data + --to together); invalid --to target; no accepts block for --with-data |
| `invalid_submission` | 2 | Invalid evidence JSON; evidence validation failure; missing required fields |
| `terminal_state` | 2 | `--to` on already-terminal workflow |
| `workflow_not_initialized` | 2 | `koto next` before `koto init` |
| `template_error` | 3 | Cycle detected; chain limit reached; ambiguous transition; dead-end state; unresolvable transition; unknown state |
| `persistence_error` | 3 | Disk I/O failure |
| `concurrent_access` | 1 | Lock contention (two concurrent `koto next` calls on same workflow) |
| `gate_blocked` | 1 | Gate conditions not satisfied (from dispatch path) |
| `integration_unavailable` | 1 | Integration runner missing or timed out |

**`agent_actionable: false` scenario** (from PRD-koto-user-skill R17; DESIGN-koto-user-skill):

When `blocking_conditions` contains entries where `agent_actionable` is `false`, the agent
cannot resolve the block by submitting evidence or overriding. Correct agent responses:
- Wait and call `koto next` again (condition integrations are polling-style)
- Report to user that external action is required (e.g., CI must be fixed)
- Use `--to` if a human supervisor has explicitly authorized bypassing the condition

---

## (c) Requirements mapping to koto-author updates

### Remove phantom commands (SKILL.md)

**Source: PRD-koto-user-skill R1**

- Remove all mentions of `koto status` and `koto query` from koto-author SKILL.md
- Correct alternatives: `koto next <name>` to get current state; `koto workflows` to list

### Extend template-format.md Layer 3: structured gate output schemas

**Source: PRD-koto-user-skill R2, R3; PRD-gate-transition-contract R1, R2; DESIGN-koto-user-skill Decision 2**

For each gate type, add a subsection with:
1. Field table (name, type, description)
2. Annotated YAML block showing gate declaration + `when`-block using emitted fields

Gate type output schemas (per PRD-gate-transition-contract R1):

| Gate type | Output fields | Pass condition |
|-----------|--------------|----------------|
| `command` | `exit_code: number`, `error: string` | `exit_code == 0` |
| `context-exists` | `exists: boolean`, `error: string` | `exists == true` |
| `context-matches` | `matches: boolean`, `error: string` | `matches == true` |

- `error` is empty on success; contains failure reason on failure; `"timed_out"` on timeout
- Timeout behavior: `command` produces `{exit_code: -1, error: "timed_out"}`; context gates produce matching-schema variants
- Output shape is always consistent regardless of success/failure/timeout/error

**`gates.<name>.<field>` path syntax** (PRD-gate-transition-contract R3):
```yaml
transitions:
  - target: deploy
    when:
      gates.ci_check.exit_code: 0
```
Gate output is namespaced under `gates.<gate_name>.<field>`. Agent-submitted evidence
lives at the top level (no `gates.` prefix). The `gates` key is reserved — `--with-data`
payloads containing a `gates` field are rejected (R7).

### Document `override_default` field

**Source: PRD-koto-user-skill R4; PRD-gate-transition-contract R4**

In template-format.md, document `override_default` as an optional per-gate field:
- Type: must match the gate type's output schema
- Three-tier resolution order: `--with-data` > `override_default` > built-in default
- Built-in defaults: command → `{exit_code: 0, error: ""}`, context-exists → `{exists: true, error: ""}`, context-matches → `{matches: true, error: ""}`
- Compiler validates that custom `override_default` matches gate type's schema
- Template authors set custom `override_default` when they want overrides to route to a non-default transition

### Document override CLI commands in template-format.md

**Source: PRD-koto-user-skill R5; PRD-gate-transition-contract R5**

Document in template-format.md (from a template author's perspective — what to set up
to support the override flow):
- `koto overrides record <name> --gate <gate> --rationale "reason"` — mandatory rationale, no empty string
- `koto overrides record <name> --gate <gate> --rationale "reason" --with-data '<json>'` — explicit data
- `koto overrides list <name>` — query override audit trail
- When to declare `override_default` in the template to support override routing

### Document `blocking_conditions` item schema in SKILL.md

**Source: PRD-koto-user-skill R6**

koto-author SKILL.md must document the full `blocking_conditions` item schema:
- `name` (gate name)
- `type` (gate type: command, context-exists, context-matches)
- `status`
- `agent_actionable` (boolean — whether agent can resolve)
- `output` (gate-type-specific structured data matching the gate's output schema)

### Document `--allow-legacy-gates` and D5 diagnostic

**Source: PRD-koto-user-skill R7**

In template-format.md:
- Document `--allow-legacy-gates` flag on `koto template compile`
- Document D5 diagnostic: gate with no `gates.*` routing in any `when` clause produces a
  warning (not error) about legacy boolean pass/fail behavior
- When legacy behavior is acceptable: backward compatibility during migration
- When structured routing is required: all new templates, strict mode CI

**Known limitation on `--allow-legacy-gates`** (PRD-koto-user-skill Known Limitations):
This flag is transitory — it will be removed once the shirabe `work-on` template migrates
to structured gate routing. Document this flag as temporary.

### Update complex-workflow.md example

**Source: PRD-koto-user-skill R8**

Both gate-bearing states in `references/examples/complex-workflow.md` must use `gates.*`
when routing. The updated file must pass `koto template compile` without `--allow-legacy-gates`.

### Update koto-author.md template

**Source: PRD-koto-user-skill R9**

The `compile_validation` gate in `koto-templates/koto-author.md`:
- Verify whether the gate needs updating to `gates.template_exists.exists: true` routing,
  or whether it already routes correctly via a different mechanism (read template before changing)
- Add D5 entry to the compile error list in the `compile_validation` directive

### Document `details` split as a template authoring concept

**Source: PRD-koto-next-output-contract R14**

koto-author must teach the `details` split as a first-class concept:
- `template-format.md` must document the `details` split syntax alongside `directive`
- Explain when to use `details` (long multi-paragraph instructions, checklists) vs. keeping
  everything in `directive` (short single-line instructions)
- Document the mapping from template features to caller-visible action values:
  - gates (no accepts) → `gate_blocked`
  - accepts block → `evidence_required`
  - terminal state → `done`
  - integration → `integration`
- The koto-author workflow must guide authors through the `details` split during
  state_design and template_drafting phases
- The koto-author template itself should dogfood the `details` split on longer states
  (state_design, template_drafting, compile_validation are candidates)
- At least one bundled example template must demonstrate the `details` convention

---

## (d) Open questions and provisional items

### `context-matches` gate is provisional

**Source: PRD-koto-user-skill R3; PRD-koto-user-skill Open Questions**

The `context-matches` gate output schema is documented in source (`matches: bool, error: string`)
but no functional test fixture was verified. Document as available with the stated schema, and
note that functional-test verification is pending.

**Decision per PRD-koto-user-skill R3**: document as provisional in both koto-author and koto-user.

### Variable substitution in directives — undocumented, potentially confusing

**Source: PRD-koto-user-skill Open Questions**

`{{VARIABLE_NAME}}` tokens in `directive` and `details` fields are substituted by the engine
before output. Agents receiving raw `{{...}}` tokens (if substitution fails) vs. substituted
values may be confused. Neither koto-author nor koto-user currently covers this behavior.

**Current status**: neither skill documents this; PRD flags it as open. Should koto-user's
`response-shapes.md` include a note about variable substitution in directive text?

### `--allow-legacy-gates` is transitory

**Source: PRD-koto-user-skill Known Limitations**

The `--allow-legacy-gates` flag will be removed once the shirabe `work-on` template migrates.
Document it with an explicit note that it is temporary and will be removed in a future release.

### `koto next --to` skips gates

**Source: PRD-koto-next-output-contract Known Limitations; PRD-unified-koto-next R10a**

A directed transition bypasses safety gates on the target state. This is intentional (honor
the caller's destination) but means `--to` can land on states whose gates would otherwise block.
Both skills should note this behavior where `--to` is documented.

### SignalReceived is invisible to callers

**Source: PRD-koto-next-output-contract R10, Known Limitations**

When SIGTERM/SIGINT interrupts the advancement loop, the response is a valid response shape
for the state the engine stopped at. Callers cannot tell if a chain was interrupted. The
response is complete and valid; calling `koto next` again continues correctly. Template authors
should be aware that long chains can be cut short.

### `integration_unavailable` name-field ambiguity

**Source: PRD-koto-next-output-contract Known Limitations**

When an integration runner fails, the error message may be concatenated into the `name` field
of `IntegrationUnavailable`. The `name` field can contain either a clean integration name or
an error message. Skills should document this as a known edge case in `response-shapes.md`.

### Session persistence commands are partially implemented

**Source: PRD-session-persistence-storage Acceptance Criteria**

Several session/context commands are implemented (marked `[x]`), but cloud backend and
`koto session resolve` are not yet complete (marked `[ ]`). When documenting context commands,
note which capabilities require cloud backend configuration vs. working with the local backend.

Implemented and available:
- `koto context add`, `get`, `exists`, `list`
- `koto session list`, `koto session cleanup`
- `{{SESSION_DIR}}` template variable substitution

Not yet available (requires cloud backend):
- `koto session resolve --keep local|remote`
- Cloud sync across machines

### `koto next` output contract action values changed from `"execute"`

**Source: PRD-koto-next-output-contract R11, D7**

The `action` field changed from the generic `"execute"` to descriptive names
(`evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`).
This is a breaking change. Skills must not reference `action: "execute"` in any example.
The `"done"` and `"confirm"` values are unchanged.

### Backward compatibility: legacy gate templates still work

**Source: PRD-gate-transition-contract R10**

Existing templates without `gates.*` routing in `when` clauses continue to work (legacy
boolean pass/block behavior). The compiler warns but does not error. Templates using
`accepts` blocks with `override` enum values as the workaround pattern continue to work
via `--with-data '{"status": "override"}'` (plain evidence submission, unrelated to the
new override mechanism). Skills should note that the legacy pattern is preserved.

---

## Cross-cutting notes for skill authors

1. **Verify field names against Rust source before writing**: The DESIGN-koto-user-skill
   explicitly requires verifying gate output field names against `src/gate/`
   (`StructuredGateResult` and per-gate result types) before writing documentation.
   This is a required pre-step, not something to draft from memory.

2. **Dispatch on `action`, not field presence**: PRD-koto-next-output-contract D7 explains
   that the historical `"execute"` value forced field-presence inspection to determine
   response shape. The new descriptive action values eliminate this — skills must reinforce
   dispatching on `action` directly.

3. **The `gates` top-level key is reserved**: `--with-data` payloads with a `gates` field
   at the top level are rejected. Skills should note this constraint to prevent agent errors.

4. **Override events survive rewind**: Per PRD-gate-transition-contract acceptance criteria,
   override events from `koto overrides list` are visible even after `koto rewind`. Skills
   documenting `koto rewind` should note this.

5. **Evidence is scoped to the current state epoch**: Evidence submitted in state A is not
   available when the workflow enters state B. Each state starts with an empty evidence map
   (DESIGN-unified-koto-next event model). Skills must clarify this to prevent agents from
   assuming accumulated evidence persists.
