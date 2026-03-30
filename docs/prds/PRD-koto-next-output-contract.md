---
status: Accepted
problem: |
  The koto auto-advancement engine's caller-facing behavioral contract was never specified.
  Design docs cover engine internals but no document defines what callers see or how they
  should respond to each koto next output shape. The advanced field has inconsistent semantics
  across code paths, 9 of 14 possible engine outcomes lack user-facing documentation, and
  callers misinterpret response shapes because no decision tree exists. This has already caused
  agent confusion during workflows (issue #102).
goals: |
  Pin down the complete caller-facing contract for koto next so behavior can't drift by
  accident: every response shape, every field's semantics, every error code, and a caller
  decision tree. The engine works correctly; this PRD specifies what callers should see and do.
source_issue: 102
---

# PRD: koto next output contract

## Status

Accepted

## Problem statement

koto's auto-advancement engine was built across three design efforts: the CLI output contract (#37), the unified koto next command (#43), and the auto-advancement engine (#49). Each design specified its piece of the system. Nobody specified the whole from the caller's perspective.

The result: AI agents calling `koto next` encounter response shapes, field values, and error codes that no single document explains. The `advanced` field -- present in every response -- has three different meanings depending on the code path. The `precondition_failed` error code covers six structurally different failures that require different caller responses. The `action: "confirm"` response variant isn't mentioned in the main CLI guide. Gate-with-evidence-fallback (where `EvidenceRequired` is returned instead of `GateBlocked`) is completely undocumented.

Issue #102 surfaced this concretely: during a workflow session, every phase returned `advanced: true`, and the caller interpreted it as "these phases are pre-cleared" rather than "you just entered a new phase." The caller skipped substantive work because the contract never said what `advanced: true` means.

This isn't a naming problem. It's an absent contract. The engine's behavior is correct and well-tested; what's missing is the authoritative specification of what callers should see and do.

## Goals

- Every response shape a caller can receive from `koto next` is cataloged with exact JSON structure
- Every field's semantics are formally defined, including edge cases and per-code-path behavior
- Callers can determine their next action from the response alone, without consulting engine internals
- Error codes distinguish failure categories that require different caller responses
- The contract is testable: acceptance criteria define binary pass/fail conditions

## User stories

**As an AI agent running a koto workflow**, I want a decision tree that tells me what to do for every possible `koto next` response, so that I don't need to understand engine internals to use the CLI correctly.

**As an AI agent encountering an error**, I want error codes that distinguish "fix my input" from "report a template bug" from "retry later," so that I can take the right corrective action.

**As a template author**, I want to know what response shapes my template's states will produce for callers, so that I can write directives and accepts blocks that make sense in context.

**As a koto maintainer**, I want a single authoritative contract for `koto next` output, so that engine changes are evaluated against a spec and don't accidentally change caller-visible behavior.

## Requirements

### Functional requirements

**R1. Response shape catalog.** `koto next` must produce exactly one of these success response shapes (exit 0). The `action` field carries the shape name directly -- callers dispatch on `action`, not on field presence:

| Shape | `action` |
|-------|----------|
| EvidenceRequired | `"evidence_required"` |
| GateBlocked | `"gate_blocked"` |
| Integration | `"integration"` |
| IntegrationUnavailable | `"integration_unavailable"` |
| Terminal | `"done"` |
| ActionRequiresConfirmation | `"confirm"` |

The only loop control a caller needs is `action == "done"`. All other action values mean "the workflow isn't done, here's what's needed." The action value tells the caller exactly what kind of stop it hit, without inspecting other fields.

All non-terminal shapes include `directive` (always present) and optionally `details` (present on first visit, omitted on retries; see R9).

After `koto next --to` lands on an auto-advanceable state (no accepts, unconditional transition), the response reflects that state as `evidence_required` with empty `expects`. The caller should call `koto next` again to trigger the advancement loop. Under normal `koto next` (no `--to`), the engine auto-advances through these states internally.

**R2. `advanced` field definition.** The `advanced` field is present in every success response as a boolean. Its meaning: "at least one state transition occurred during this invocation of `koto next`." Callers must not use `advanced` to determine their next action. The `action` field is the authoritative signal for caller behavior. `advanced` is informational context only.

**R3. `advanced` consistency across code paths.** The `advanced` field must have the same semantic across all invocation modes:
- Bare `koto next`: true if the advancement loop made at least one transition.
- `koto next --with-data`: true if evidence submission triggered at least one transition.
- `koto next --to`: true (a directed transition always constitutes a transition).

**R4. Caller decision tree.** The documented caller contract must specify what callers should do for each response shape. The decision tree dispatches on exit code, then `action`:

- `action: "done"` -> Stop. Workflow is complete.
- `action: "evidence_required"` -> Read directive, do the work, submit evidence via `--with-data` matching `expects.fields`. If `blocking_conditions` is non-empty, gates failed but evidence can override (see R6).
- `action: "gate_blocked"` -> Fix blocking conditions, call `koto next` again. Don't submit evidence.
- `action: "integration"` -> Process `integration.output`. If `expects` is present, submit evidence. If `expects` is null, call `koto next` again.
- `action: "integration_unavailable"` -> Integration runner is missing. The caller can: submit evidence if `expects` is present, use `--to` to skip to another state, or report to the user that the runner needs configuration (an out-of-band action).
- `action: "confirm"` -> Read `action_output` (command, exit code, stdout, stderr). If `expects` is present, evaluate the output and submit evidence via `--with-data`. If `expects` is null, call `koto next` again to re-evaluate.

**R5. Error code categories.** Error responses must use distinct codes that tell callers which category of failure occurred:

| Category | Exit code | Caller action |
|----------|-----------|---------------|
| Caller error (fixable input) | 2 | Fix the input and retry |
| Template or infrastructure error | 3 | Report to user; agent can't fix. Distinguish by `error.code`: `template_error` means the template is malformed, `persistence_error` means I/O failure. |
| Transient error | 1 | Wait and retry |

Specific mapping:

| Failure | Code | Exit |
|---------|------|------|
| Bad flags (--with-data + --to together) | `precondition_failed` | 2 |
| Invalid --to target | `precondition_failed` | 2 |
| No accepts block for --with-data | `precondition_failed` | 2 |
| Invalid evidence JSON | `invalid_submission` | 2 |
| Evidence validation failure | `invalid_submission` | 2 |
| Workflow already terminal or cancelled | `terminal_state` | 2 |
| No workflow found | `workflow_not_initialized` | 2 |
| Cycle detected | `template_error` | 3 |
| Chain limit reached | `template_error` | 3 |
| Ambiguous transition | `template_error` | 3 |
| Dead-end state | `template_error` | 3 |
| Unresolvable transition | `template_error` | 3 |
| Unknown state | `template_error` | 3 |
| Persistence failure (disk I/O) | `persistence_error` | 3 |
| Lock contention (concurrent access) | `concurrent_access` | 1 |
| Gate blocked (from dispatch path) | `gate_blocked` | 1 |
| Integration unavailable | `integration_unavailable` | 1 |

**R6. Gate-with-evidence-fallback visibility.** When gates fail on a state that has an `accepts` block, the response must be `EvidenceRequired` (not `GateBlocked`) with `blocking_conditions` included as an array. An empty `blocking_conditions` array means no gate issues. A populated array means gates failed but the state accepts evidence -- submitting valid evidence that matches a conditional transition advances the workflow past the failed gates. Gates are not re-evaluated; the evidence submission resolves the transition directly.

**R7. `--to` behavior contract.** `koto next --to <target>` is a single-shot directed transition. It must:
- Validate the target is a legal transition from the current state
- Append a directed_transition event
- Return the target state's response shape
- Not trigger auto-advancement past the target state
- Not evaluate gates on the target state (directed transitions honor the caller's chosen destination)

**R8. `--to` does not chain auto-advancement.** After a directed transition, the caller receives the target state's classification. If the target is an auto-advanceable state (no accepts, unconditional transition), the response reflects that state -- the caller must call `koto next` again to trigger the advancement loop. This is intentional: the caller chose a destination and should see it.

**R9. Directive split: summary and details.** Responses include `directive` (short summary, always present on non-terminal shapes) and an optional `details` field (extended instructions). Response behavior:
- **First visit** to a state: both `directive` and `details` are present. `details` contains the full instructions the caller needs to execute the phase.
- **Subsequent visits** (retries, self-loops, polling): `directive` is present, `details` is absent (omitted from JSON, not null). The caller already received the full instructions on the first visit.
- **Force-full retrieval**: `koto next --full` forces `details` to be included regardless of visit count. This covers context recovery (new session, context compression dropped the instructions).
- **No details defined**: States that don't define extended instructions produce no `details` key. Callers see the same response shape as today.
- **Visit tracking**: Uses existing JSONL state events. A state's visit count is derived from the event log (counting `Transitioned`, `DirectedTransition`, and `Rewound` events targeting that state). No new state files or schema changes needed.

How template authors specify the summary/details split (markdown separator, YAML field, external file) is a template format concern, not an output contract concern. The downstream design doc determines the template source format.

**R10. SignalReceived transparency.** When SIGTERM/SIGINT interrupts the advancement loop, the response resolves to a fully valid response shape for the state the engine stopped at. The caller sees a normal response with no interruption indicator. This is the intended contract: the response is complete and valid for the stopped-at state, and calling `koto next` again continues correctly from that state.

### Non-functional requirements

**R11. Backward compatibility.** The `action` field values change from `"execute"` to descriptive names (`"evidence_required"`, `"gate_blocked"`, `"integration"`, `"integration_unavailable"`). This is a breaking change for callers that match on `action == "execute"`. The `"done"` and `"confirm"` values are unchanged. Adding `blocking_conditions` to `EvidenceRequired` responses, adding the optional `details` field, and adding new error codes (`template_error`, `persistence_error`, `concurrent_access`) are additive changes. The `advanced` field is not removed or renamed.

**R12. Error response consistency.** All error responses (exit 1, 2, 3) must use the structured `NextError` format: `{"error": {"code": "<string>", "message": "<string>", "details": [...]}}`. Unstructured error shapes (`{"error": "<string>", "command": "next"}`) must be migrated to the structured format.

**R13. Caller and authoring documentation updates.** Documentation that references `koto next` response shapes must be updated atomically with the contract changes. Affected files:

*Caller-facing (what agents read at runtime):*
- **AGENTS.md**: All `action: "execute"` references updated to descriptive action values. JSON examples updated. Error code table expanded with `template_error`, `persistence_error`, `concurrent_access`. `blocking_conditions` documented on `EvidenceRequired` responses. `details` field documented with first-visit/subsequent-visit behavior and `--full` flag. `advanced` field definition added.
- **`.cursor/rules/koto.mdc`**: Updated to current `koto next` API (still references removed `koto transition` command and outdated response shapes).

*Template authoring (what skill authors read when writing templates):*
- **koto-author SKILL.md** (`plugins/koto-skills/skills/koto-author/SKILL.md`): The koto execution loop section (lines 46-50) describes the generic "run next, do work, submit evidence" pattern but doesn't mention distinct action values for gates, confirmations, or integrations. Must be updated so template authors understand what response shapes their template features produce for callers.
- **template-format.md** (`plugins/koto-skills/skills/koto-author/references/template-format.md`): The template format guide teaches how to write templates. Must explain the mapping from template features (gates, accepts, terminals, integrations) to caller-visible action values (`gate_blocked`, `evidence_required`, `done`, etc.) and how the `details` split works in directive body sections.
- **Example templates**: The koto-author's own template and bundled examples should demonstrate the `details` marker convention (once the design doc determines the template source format).

## Acceptance criteria

- [ ] Every `NextResponse` variant's JSON shape is documented with field names, types, and presence rules
- [ ] The `advanced` field definition ("at least one state transition during this invocation") appears in user-facing documentation
- [ ] Documentation explicitly states that callers dispatch on `action`, not on `advanced` or field presence
- [ ] The `action` field carries descriptive shape names: `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `done`, `confirm`
- [ ] A complete caller decision tree covers all six `action` values
- [ ] Error code `template_error` (exit 3) exists for cycle detection, chain limit, ambiguous transition, dead-end state, unresolvable transition, and unknown state
- [ ] Error code `persistence_error` (exit 3) exists for disk I/O failures
- [ ] Error code `concurrent_access` (exit 1) exists for lock contention
- [ ] `EvidenceRequired` responses include a `blocking_conditions` field (empty array when no gate issues, populated when gates failed with evidence fallback)
- [ ] `--to` does not trigger auto-advancement or gate evaluation on the target state
- [ ] All error responses use the structured `NextError` format (no unstructured error shapes)
- [ ] Calling `koto next` twice on a gate-blocked state (without fixing gates) returns `advanced: false` on both calls (no transitions occur in either invocation)
- [ ] `koto next --to <current_state>` returns `advanced: true` (a directed transition is always a transition)
- [ ] `koto next --with-data` returns `advanced: true` when evidence submission triggers at least one transition
- [ ] Bare `koto next` on a state requiring evidence (no prior transitions) returns `advanced: false`
- [ ] `koto next --to <invalid_target>` returns `precondition_failed` with exit code 2
- [ ] When SIGTERM interrupts a multi-state advancement chain, the response is a valid shape for the state the engine stopped at
- [ ] `EvidenceRequired` with empty `blocking_conditions` array (no gate issues) is distinguishable from `EvidenceRequired` with populated `blocking_conditions` (gate failure with evidence fallback)
- [ ] Callers that don't inspect `blocking_conditions` or new error codes receive no regressions in existing response shapes
- [ ] AGENTS.md JSON examples use descriptive action values (no `action: "execute"` references remain)
- [ ] AGENTS.md error code table includes `template_error`, `persistence_error`, and `concurrent_access`
- [ ] AGENTS.md documents `blocking_conditions` on `EvidenceRequired` responses
- [ ] AGENTS.md documents the `details` field and `--full` flag
- [ ] `.cursor/rules/koto.mdc` uses current `koto next` API (no `koto transition` references)
- [ ] koto-author SKILL.md execution loop section references distinct action values (not just generic "run next, submit evidence")
- [ ] template-format.md documents the mapping from template features (gates, accepts, terminals) to caller-visible action values
- [ ] At least one example template demonstrates the `details` directive split
- [ ] `template_error` (exit 3) and `persistence_error` (exit 3) are distinguishable by `error.code`
- [ ] First visit to a state with `details` defined returns both `directive` and `details` in the response
- [ ] Subsequent visits to the same state omit `details` from the response
- [ ] A force-full mechanism (flag or command) returns `details` even on subsequent visits
- [ ] States without `details` defined behave identically to current behavior (`details` field absent)

## Out of scope

- **Engine refactoring.** The auto-advancement loop, gate evaluation, and transition resolution logic are correct and not changing. This PRD specifies the output contract, not the engine internals.
- **Template authoring guide.** How to write templates that produce good caller experiences is a separate concern.
- **Stale documentation cleanup.** The 12+ files referencing `koto transition` (a removed command) need cleanup but aren't part of this contract specification.
- **AGENTS.md vs. cli-usage.md consolidation.** Whether these docs should be merged or kept separate is deferred. Both must be updated to reflect the new contract, but structural consolidation is a separate concern.
- **`advanced` field rename or removal.** The field stays as-is with a formal definition. The breaking change budget is spent on descriptive `action` values, which provides more value than renaming `advanced`.
- **Contract versioning.** Whether the output contract version is independent of the CLI version is deferred.

## Known limitations

- **SignalReceived is invisible.** Callers can't tell if an advancement chain was interrupted. This is by design (the response is valid for the stopped-at state), but template authors should know that long chains can be cut short.
- **`--to` skips gates.** A directed transition bypasses safety gates on the target state. This is intentional (honor the caller's destination) but means `--to` can land on states whose gates would otherwise block.
- **`precondition_failed` remains for genuine caller errors.** Callers still need to parse error messages to distinguish between "bad flags," "invalid target," and "no accepts block" within this code. Splitting further would fragment the error space for cases where the caller action is the same (fix the input).
- **IntegrationError::Failed maps to IntegrationUnavailable.** When an integration runner fails, the error message is concatenated into the `name` field of `IntegrationUnavailable`. The `name` field can contain either a clean integration name or an error message. This type-level ambiguity persists.

## Decisions and trade-offs

**D1. Keep `advanced` rather than rename or deprecate.**
Renaming to `state_changed` or `transitioned` would be a breaking change to the JSON wire format. Adding a companion field (like `transitions_made: u32`) was considered but adds complexity for a signal callers shouldn't dispatch on. The chosen path: formal definition + documentation that callers use `action`, not `advanced`. Alternatives: rename (breaking), deprecate (removes useful context), add companion field (marginal value).

**D6. PRD specifies details output contract, defers template source format to design.**
The `details` field behavior (present on first visit, absent on retries, `--full` forces it) is an output contract concern and belongs in the PRD. How template authors write the summary/details split (markdown separator, YAML field, external file) is a template format concern. Research confirmed all source format options converge to the same compiled representation, so the output contract is format-agnostic. Prescribing the template format in the PRD would overconstrain the design without benefiting callers. Alternative: prescribe markdown separator in the PRD (rejected: crosses the what/how boundary).

**D7. Descriptive `action` values instead of generic `"execute"`.**
The original design used `"execute"` for all non-terminal, non-confirm shapes, requiring callers to reconstruct the shape from field presence. This forced a field-presence dance (`if expects... elif blocking_conditions... elif integration...`) that the action field should have handled. The fix: `action` carries the shape name directly (`"evidence_required"`, `"gate_blocked"`, `"integration"`, `"integration_unavailable"`). The terminal check (`action == "done"`) stays simple. This is a breaking change for callers matching `action == "execute"`, but koto is pre-1.0 and the current callers are internal skill plugins that will be updated alongside the contract. Alternative: keep `"execute"` and add a `shape` field (rejected: two fields for the same information, `action` is the natural discriminator).

**D2. Add `blocking_conditions` to `EvidenceRequired` rather than a new response variant.**
A new variant (`EvidenceRequiredWithGateContext`) would fragment the response space for what's the same caller action (submit evidence). An always-present `blocking_conditions` array on `EvidenceRequired` (empty when no gate issues) is additive, backward-compatible, and keeps the type system simple. Alternative: optional `gate_context` nested object (rejected: adds unnecessary nesting).

**D3. Split error codes by actionability rather than by failure mode.**
The question was whether to create error codes for every failure (cycle_detected, chain_limit_reached, ambiguous_transition, etc.) or group by what the caller should do. Grouping by actionability won: `template_error` covers all "agent can't fix this" failures, `concurrent_access` covers "retry," and `precondition_failed` covers "fix your input." This gives callers exactly the signal they need without fragmenting the error space. Alternative: per-failure codes (rejected: same caller action for all template bugs).

**D4. `--to` stays single-shot (no auto-advancement chaining).**
When `--to` lands on a passthrough state, the caller must call `koto next` again. This is intentional: the caller chose a destination and should see it. Chaining past the destination would undermine the caller's intent and change behavior for callers who use `--to` as an override. Alternative: chain after landing (rejected: contradicts "honor the caller's destination").

**D5. `--to` does not evaluate gates on the target state.**
Directed transitions are an explicit choice to move to a state. Running gates would mean `--to` could fail for reasons unrelated to the caller's input. If `--to` is an escape mechanism (skip past a blocked state), gate evaluation would defeat its purpose. Alternative: evaluate gates (rejected: undermines escape-hatch use case).
