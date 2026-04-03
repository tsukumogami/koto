# Lead: koto-user persona needs

## Findings

### 1. The koto init command

Source: `src/cli/mod.rs` lines 964–1083

`koto init` takes three inputs: a workflow name, a `--template` path, and optional `--var KEY=VALUE` flags. It:

1. Validates the name against `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`
2. Rejects duplicate init (state file already exists)
3. Compiles the template (or uses cache)
4. Resolves variables: `--var` value > template default > error if required
5. Writes a header + `WorkflowInitialized` event + initial `Transitioned` event
6. Returns JSON: `{"name": "<name>", "state": "<initial_state>"}`

Key agent knowledge needed:
- Name format rule (alphanumeric, dots, dashes, underscores)
- `--template` points to a `.md` source file (or `.json` compiled file)
- Required vs optional template variables; how to list them before init
- The returned `state` field tells you the first state without calling `koto next`

### 2. The koto next command — response schema

Source: `src/cli/next_types.rs`, `src/cli/next.rs`

`koto next <name>` returns JSON with an `action` field as the primary dispatch signal. Six action values exist:

| action | meaning | agent behavior |
|--------|---------|----------------|
| `evidence_required` | state needs data or has conditional transitions | Read `directive` and `expects`; submit via `--with-data` |
| `gate_blocked` | one or more gates failed; no `accepts` block | Read `blocking_conditions`; fix or override; call `koto next` again |
| `integration` | an integration ran successfully | Read `integration.output` |
| `integration_unavailable` | integration declared but no runner configured | Treat as manual step; use `expects` if present |
| `done` | terminal state reached | Workflow complete |
| `confirm` | default action ran, needs confirmation | Read `action_output`; submit evidence to proceed |

Fields present in most non-terminal responses:
- `state`: current state name
- `directive`: what to do (may contain `{{VARIABLE}}` substitutions)
- `details`: extended guidance, only on first visit (use `--full` to force)
- `advanced`: `true` if the engine auto-advanced through states before stopping
- `expects`: schema describing what evidence to submit (null for `gate_blocked`, `done`)
- `blocking_conditions`: array of gate failures (empty array when none)
- `error`: always null on success

The `expects` schema has three parts:
- `event_type`: always `"evidence_submitted"`
- `fields`: map of field name → `{type, required, values?}`
- `options`: array of `{target, when}` — the transition routing conditions

The `blocking_conditions` items have:
- `name`: gate name
- `type`: gate type (`command`, `context-exists`, `context-matches`)
- `status`: `failed`, `timed_out`, or `error`
- `agent_actionable`: `true` when the gate has an override default (agent can call `koto overrides record`)
- `output`: structured gate output — for `command` gates: `{"exit_code": N, "error": "..."}`, for `context-exists`: `{"exists": false, "error": "..."}`

### 3. The evidence submission flow

Source: `src/cli/mod.rs` lines 1485–1566, functional test `gate-with-evidence-fallback.feature`

`koto next <name> --with-data '{"field": "value"}'` submits evidence. The engine:
1. Validates the JSON payload against the `accepts` schema
2. Rejects the reserved `"gates"` key
3. Appends an `EvidenceSubmitted` event
4. Runs the advance loop with the merged evidence

Key rules for agents:
- `--with-data` is only valid when `action` is `evidence_required` (state has an `accepts` block)
- Use `--with-data` and `--to` are mutually exclusive
- Evidence is validated against `expects.fields`; required fields must be present; enum fields must match allowed values
- The `"gates"` top-level key is reserved — never include it in submissions

Functional test example (from `gate-with-evidence-fallback.feature`):
```
koto next test-wf --with-data '{"status": "completed", "detail": "manual"}'
```

### 4. Gate blocking behavior

Source: `src/engine/advance.rs` lines 298–400, `src/cli/next.rs`, `src/cli/next_types.rs`

Gate outcomes:
- **Passed**: gate succeeded; engine continues
- **Failed**: gate condition not met; `exit_code != 0` for command gates
- **TimedOut**: command didn't finish in configured timeout
- **Error**: command couldn't be spawned

When gates fail, behavior depends on whether the state has an `accepts` block:
- **No accepts block**: returns `gate_blocked` — agent must fix the condition or override
- **Has accepts block**: returns `evidence_required` with `blocking_conditions` populated — agent can submit override evidence

Gate types (from `src/gate.rs`):
- `command`: runs a shell command; output is `{"exit_code": N, "error": ""}`
- `context-exists`: checks if a session context key exists; output is `{"exists": bool, "error": ""}`
- `context-matches`: checks if context content matches a pattern; similar output

Structured gate routing: when transitions have `when` clauses referencing `gates.<name>.<field>` (e.g., `gates.ci_check.exit_code: 0`), gate output is injected into the evidence map and routes automatically. The agent doesn't need to submit evidence — the engine resolves the path based on gate results.

### 5. The overrides mechanism

Source: `src/cli/overrides.rs`

`koto overrides record <name> --gate <gate_name> --rationale "<text>"` records a gate override. Optional `--with-data` provides a custom override value.

Three-tier value resolution:
1. `--with-data` value (explicit)
2. Gate's `override_default` field (from template)
3. Built-in default for gate type (`{"exit_code": 0, "error": ""}` for command gates)

The `agent_actionable` flag in `blocking_conditions` tells the agent whether an override is available — `true` when either `override_default` is set or a built-in default exists.

`koto overrides list <name>` returns full override history with fields: `state`, `gate`, `rationale`, `override_applied`, `actual_output`, `timestamp`.

After recording an override, call `koto next` again — the override is injected into gate evaluation, replacing the real gate result.

When to override: when a gate represents a manual check, when the automated check is known-flaky, or when business logic warrants bypassing a blocking condition with documented rationale.

### 6. The rewind mechanism

Source: `src/cli/mod.rs` lines 1085–1157, `test/functional/features/rewind.feature`

`koto rewind <name>` appends a `Rewound` event, returning to the previous state.

Behavior:
- Returns `{"name": "<name>", "state": "<prev_state>"}`
- Cannot rewind past initial state (error: "already at initial state, cannot rewind")
- Clears decisions accumulated since the rewound-to state
- Does not clear gate override history (overrides persist across rewinds)

Use cases for agents: when you realize the current state's action was wrong; when a review state returns you and you want to redo; when implementing a review-implement-review loop.

### 7. The --to flag (directed transition)

Source: `src/cli/mod.rs` lines 1379–1468

`koto next <name> --to <state>` forces a transition to a named state. Constraints:
- Target must be a valid transition from the current state (in template's `transitions` array)
- Mutually exclusive with `--with-data`
- Single-shot: does not run the advancement loop; dispatches on the target state directly

This is a "skip" mechanism. The agent uses it when: conditions are already met and auto-advance logic would loop unnecessarily; or when a state should be manually bypassed.

### 8. The `details` field and --full flag

Source: `src/cli/mod.rs` lines 1784–1798

`details` is extended guidance on a state. It only appears on first visit (visit count ≤ 1). On subsequent visits, it's suppressed to reduce noise. Use `koto next <name> --full` to force `details` to always appear.

### 9. Decisions

Source: `test/functional/features/decisions.feature`

`koto decisions record <name> --with-data '{"choice": "...", "rationale": "..."}'` records a structured decision without advancing state. Required fields: `choice` and `rationale`. Optional: `alternatives_considered`.

`koto decisions list <name>` returns accumulated decisions for the current state.

Decisions are scoped to the current epoch (state) — they're cleared on rewind.

### 10. Session and context

The `koto session dir <name>` command returns the session directory path — useful when a skill needs to store artifacts alongside the state file.

`koto context add <session> <key> [--from-file]` and `koto context get <session> <key>` store/retrieve keyed content. Context keys support hierarchy (e.g., `scope.md`, `research/r1/lead.md`). Context is used by `context-exists` and `context-matches` gate types.

`koto workflows` lists all active workflows in the current directory.

### 11. Exit codes (agent-critical)

Source: `src/cli/mod.rs` lines 35–43, `src/cli/next_types.rs` lines 306–325

Exit codes 0/1/2/3 carry semantics agents should respect:
- `0`: success
- `1`: transient/retryable (`gate_blocked`, `integration_unavailable`, `concurrent_access`) — retry is appropriate
- `2`: caller error (`invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized`) — agent must fix behavior
- `3`: infrastructure/config error (corrupted state, template hash mismatch) — escalate

### 12. The agent journey end-to-end (composite)

Functional test `workflow-lifecycle.feature` demonstrates the full loop:
1. `koto init <name> --template <path> [--var K=V]` → creates session, returns `{"name", "state"}`
2. `koto next <name>` → returns action directive
3. Do work based on `directive` and `details`
4. If `action == "evidence_required"`: submit `koto next <name> --with-data '{...}'`
5. If `action == "gate_blocked"`: fix the blocking condition (or override), then `koto next <name>` again
6. If `action == "done"`: workflow complete; session is auto-cleaned up
7. At any time: `koto rewind <name>` to go back one state

## Implications

**Skill scope**: The koto-user skill needs to cover the complete agent runtime loop, not just the happy path. The `gate_blocked` vs `evidence_required` distinction, the `blocking_conditions` interpretation, and the override mechanism are all non-obvious and frequently encountered.

**JSON schema literacy is core**: The agent must interpret `koto next` JSON output correctly to take the right action. The skill should make the full output contract explicit, with annotated examples for each `action` value.

**Gate types matter for evidence routing**: Agents using structured gate routing (with `gates.*` when clauses) will never see `gate_blocked` for those states — the engine resolves automatically. This is invisible to agents unless they know it. The skill should explain when manual intervention is vs isn't needed.

**Override decision is judgment, not mechanical**: `agent_actionable: true` is a signal, not a directive. The agent must decide whether an override is appropriate, provide a meaningful rationale, and understand that the override value matters for routing. The skill needs to set this framing clearly.

**Two separate read patterns**: Some state transitions require `--with-data` (agent submits evidence), others are automatic (gate routing). Confusing these causes the most common failure mode (submitting evidence to a state that will auto-advance, or calling `koto next` repeatedly without submitting evidence when `expects.fields` is non-empty).

**koto-user differs from koto-author structurally**: koto-author is a practice guide (how to design templates). koto-user is a runtime reference (how to run them). The skill should be organized around the agent's decision loop, not the template author's design concerns.

**Decisions vs evidence**: These are related but distinct. Evidence advances state; decisions record context. The skill should clearly distinguish them.

## Surprises

**`evidence_required` covers multiple cases**: The same `action` value appears for: (a) states waiting for agent data, (b) states where gates failed but accepts is present, and (c) auto-advance candidates (empty `expects.fields`). An agent reading just `action == "evidence_required"` must then look at `expects.fields` to determine which case it's in. This is a subtlety the skill must address.

**`advanced: true` does not mean done**: The engine can auto-advance through N states and still stop at an `evidence_required` or `gate_blocked`. Agents that treat `advanced: true` as success will misread responses.

**Gate override writes are decoupled from `koto next`**: You call `koto overrides record` separately, then call `koto next` again. Many agents would expect to submit the override as part of `koto next --with-data`. The skill must make this two-step flow explicit.

**`koto next --to` does not run gates**: Forced transitions skip gate evaluation entirely. This is a footgun if used incorrectly — the agent can advance past gates without them passing or being overridden. The skill should flag this clearly.

**Session auto-cleanup on terminal**: When `action == "done"`, the session directory is automatically removed (unless `--no_cleanup` was passed). This means `koto status` or `koto next` after `done` will fail with workflow-not-found. Agents that retry after `done` will be confused.

**`details` is suppressed on repeat visits**: The field is absent (not null) after first visit. Agents that check `resp.details` on retry will get undefined. The skill should explain the visit-count logic and the `--full` flag.

## Open Questions

1. **Does `koto status` exist?** The koto-author SKILL.md references `koto status` in several places, but the CLI in `src/cli/mod.rs` has no `Status` subcommand. Is this command missing from the implementation, or is `koto status` an alias for something else? If it's unimplemented, the koto-user skill should not reference it.

2. **What is the `koto query` command?** The CLAUDE.local.md mentions `koto query` to "inspect full workflow state as JSON," but this subcommand isn't visible in `src/cli/mod.rs` at the lines I read. It may exist in later sections, or it may be planned but unimplemented. The skill should only reference commands that exist.

3. **Variable substitution in directives**: The skill should explain that `{{VARIABLE_NAME}}` tokens in `directive` and `details` are substituted by the engine before output. Agents may be confused by templates that show raw `{{...}}` vs the substituted values. Is this documented anywhere?

4. **Template discovery for `koto init`**: What's the convention for how a koto-backed skill documents which `--template` path to use? The koto-author SKILL.md uses `${CLAUDE_SKILL_DIR}/koto-templates/<skill>.md`. Should the koto-user skill document this pattern?

5. **`koto workflows` output format**: The full JSON schema for `koto workflows` output isn't confirmed. What fields does it return beyond workflow name and state?

6. **Context gate semantics for `context-matches`**: The `context-matches` gate type is listed in `gate.rs` but not in any functional test fixture I read. What's its exact match behavior (regex, substring, exact)? The skill should document this gate type correctly.

7. **Multi-workflow sessions**: Can multiple workflows coexist in the same directory? The `koto workflows` command suggests yes. Are there ordering or dependency mechanics? This affects whether skills that spin up multiple workflows need coordination guidance.

## Summary

The koto-user persona needs a runtime reference organized around the `koto next` output contract: the six `action` values, how to interpret each response, when to submit evidence vs wait vs override, and how to handle the full session lifecycle from `koto init` through `done`. The most critical skill content is the `action`-field dispatch table with annotated JSON examples, the distinction between automatic gate routing and manual evidence submission, and the two-step override flow (`koto overrides record` → `koto next`). The biggest open question is whether `koto status` is a real command — the koto-author SKILL.md references it, but it's absent from the CLI source, which would mean the reference skill already contains inaccurate guidance.
