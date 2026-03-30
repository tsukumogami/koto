# Lead: What edge cases exist in the advancement loop that callers can encounter but aren't documented?

## Findings

### Complete inventory of StopReason variants and AdvanceError variants

From `src/engine/advance.rs`:

**StopReason** (returned via `AdvanceResult.stop_reason`):
1. `Terminal` -- reached a terminal state
2. `GateBlocked(BTreeMap<String, GateResult>)` -- one or more gates failed
3. `EvidenceRequired` -- conditional transitions exist, evidence doesn't match, but state has accepts block
4. `Integration { name, output }` -- integration invoked successfully
5. `IntegrationUnavailable { name }` -- integration declared but no runner
6. `CycleDetected { state }` -- loop visited the same state twice
7. `ChainLimitReached` -- exceeded 100 transitions in one invocation
8. `ActionRequiresConfirmation { state, exit_code, stdout, stderr }` -- action ran but needs confirmation
9. `SignalReceived` -- SIGTERM/SIGINT between iterations
10. `UnresolvableTransition` -- conditional transitions but no accepts block

**AdvanceError** (returned as `Err(...)` from `advance_until_stop`):
1. `AmbiguousTransition { state, targets }` -- multiple conditional transitions matched
2. `DeadEndState { state }` -- no transitions and not terminal
3. `UnknownState { state }` -- state not in template
4. `PersistenceError(String)` -- failed to persist event

### How each maps to caller-visible JSON

#### Normal responses (exit code 0):

| StopReason | action | Response variant | JSON shape |
|---|---|---|---|
| Terminal | `"done"` | Terminal | `{action, state, advanced, expects: null, error: null}` |
| GateBlocked | `"execute"` | GateBlocked | `{action, state, directive, advanced, expects: null, blocking_conditions: [...], error: null}` |
| EvidenceRequired | `"execute"` | EvidenceRequired | `{action, state, directive, advanced, expects: {...}, error: null}` |
| Integration | `"execute"` | Integration | `{action, state, directive, advanced, expects, integration: {name, output}, error: null}` |
| IntegrationUnavailable | `"execute"` | IntegrationUnavailable | `{action, state, directive, advanced, expects, integration: {name, available: false}, error: null}` |
| ActionRequiresConfirmation | `"confirm"` | ActionRequiresConfirmation | `{action, state, directive, advanced, action_output: {command, exit_code, stdout, stderr}, expects, error: null}` |
| SignalReceived | `"execute"` or `"done"` | EvidenceRequired or Terminal | Degrades to EvidenceRequired or Terminal depending on the stopped-at state |

#### Error responses (exit code 2):

| StopReason / AdvanceError | Exit code | JSON shape |
|---|---|---|
| CycleDetected | 2 | `{error: {code: "precondition_failed", message: "cycle detected: advancement loop would revisit state '<name>'", details: []}}` |
| ChainLimitReached | 2 | `{error: {code: "precondition_failed", message: "advancement chain limit reached (100 transitions)", details: []}}` |
| UnresolvableTransition | 2 | `{error: {code: "precondition_failed", message: "state '<name>' has conditional transitions but no accepts block; the agent cannot submit evidence to resolve this", details: []}}` |
| AmbiguousTransition | 2 | `{error: {code: "precondition_failed", message: "ambiguous transition from state '<name>': multiple matches [...]", details: []}}` |
| DeadEndState | 2 | `{error: {code: "precondition_failed", message: "state '<name>' has no transitions and is not terminal", details: []}}` |
| UnknownState | 2 | `{error: {code: "precondition_failed", message: "state '<name>' not found in template", details: []}}` |
| PersistenceError | 2 | `{error: {code: "precondition_failed", message: "failed to persist event: <msg>", details: []}}` |

Note: All AdvanceError variants are caught by the `Err(advance_err)` branch at line 1841 of `cli/mod.rs` and uniformly mapped to `precondition_failed` with exit code 2. This is a lossy mapping -- a PersistenceError is arguably infrastructure (exit code 3), not a caller error.

### Documentation audit by edge case

| Edge case | cli-usage.md | error-codes.md | AGENTS.md | Design docs | Caller-triggerable? |
|---|---|---|---|---|---|
| **Terminal** | Yes (full example) | No | Yes | Yes | Yes -- normal |
| **GateBlocked** | Yes (full example) | Yes | Yes | Yes | Yes -- normal |
| **EvidenceRequired** | Yes (full example) | Implicit | Yes | Yes | Yes -- normal |
| **Integration** | Yes (brief example) | Yes (code listed) | No | Yes | Yes -- if template uses integrations |
| **IntegrationUnavailable** | Yes (brief example) | Yes (code listed) | No | Yes | Yes -- always, since runner deferred |
| **ActionRequiresConfirmation** | **No** | **No** | Yes (AGENTS.md only) | Yes (DESIGN-default-action-execution.md) | Yes -- if template has `requires_confirmation` action |
| **CycleDetected** | **No** | **No** | **No** | Yes (DESIGN-auto-advancement-engine.md) | Unlikely -- requires template with cycle that auto-advances back |
| **ChainLimitReached** | **No** | **No** | **No** | Yes (DESIGN-auto-advancement-engine.md) | Very unlikely -- 100+ auto-advance states |
| **SignalReceived** | **No** | **No** | **No** | Yes (DESIGN-auto-advancement-engine.md) | Yes -- SIGTERM during long chains |
| **UnresolvableTransition** | **No** | **No** | **No** | Yes (DESIGN-unified-koto-next.md, issue #89) | Unlikely -- validator rejects, but possible with bypassed validation |
| **AmbiguousTransition** | **No** | **No** | **No** | Mentioned in PLAN | Defense-in-depth -- validator should catch |
| **DeadEndState** | **No** | **No** | **No** | Mentioned in PLAN | Defense-in-depth -- validator should catch |
| **UnknownState** | **No** | **No** | **No** | No | Defense-in-depth -- should never happen |
| **PersistenceError** | **No** | **No** | **No** | No | Yes -- disk full, permissions, etc. |

### Key gap: ActionRequiresConfirmation is undocumented in user-facing guides

The `action: "confirm"` response variant exists in AGENTS.md (the agent instructions file shipped with the koto-skills plugin) but is completely absent from:
- `docs/guides/cli-usage.md` -- the response variants table lists 5 variants but omits ActionRequiresConfirmation
- `docs/reference/error-codes.md` -- doesn't mention the confirm action at all
- The "typical agent workflow" loop in cli-usage.md only checks for `"done"` and doesn't handle `"confirm"`

### Key gap: SignalReceived silently degrades

When SIGTERM/SIGINT fires mid-chain, the CLI maps SignalReceived to either Terminal or EvidenceRequired depending on the state it stopped at. The caller gets a normal-looking response with `advanced: true` and no indication that advancement was interrupted. A caller running `koto next` again would continue from the interrupted state, which is correct behavior, but the caller has no way to know the response represents a partial chain rather than a natural stopping point.

### Key gap: error-codes.md `precondition_failed` is overloaded

Six distinct edge cases all map to `precondition_failed` with exit code 2:
- CycleDetected
- ChainLimitReached
- UnresolvableTransition
- AmbiguousTransition
- DeadEndState
- PersistenceError (arguably should be exit code 3)

The error-codes.md documentation says `precondition_failed` means: `--with-data and --to used together, --to targets an invalid state, or the state has no accepts block`. It doesn't mention cycles, chain limits, ambiguous transitions, dead ends, or persistence failures. A caller receiving `precondition_failed` has to parse the human-readable message to understand which of 6+ different failure modes occurred.

### Triggerability assessment

Edge cases fall into three categories:

**Normally triggerable by callers:**
- Terminal, GateBlocked, EvidenceRequired, Integration, IntegrationUnavailable -- standard workflow operation
- ActionRequiresConfirmation -- any template with `requires_confirmation: true` on a default_action
- SignalReceived -- SIGTERM during any `koto next` with auto-advancement
- PersistenceError -- disk full or permission issues

**Triggerable with unusual templates:**
- CycleDetected -- a template where auto-advance states form a cycle (e.g., A -> B -> A with unconditional transitions)
- ChainLimitReached -- a template with 100+ linearly chaining auto-advance states

**Defense-in-depth only (validator should prevent):**
- AmbiguousTransition -- multiple `when` conditions match the same evidence
- DeadEndState -- non-terminal state with no transitions
- UnresolvableTransition -- conditional transitions on a state with no accepts block
- UnknownState -- state reference that doesn't exist in the template

## Implications

1. **The cli-usage.md guide and error-codes.md reference are out of date.** They were written before the auto-advancement engine and default-action-execution features. The response variants table needs a 6th row for ActionRequiresConfirmation, and the error-codes doc needs entries for cycle/chain-limit/ambiguous/dead-end errors.

2. **The `precondition_failed` error code is a catch-all bucket.** A PRD should decide whether to add new error codes (e.g., `cycle_detected`, `chain_limit`, `template_error`) or document the message-parsing approach. This directly affects whether callers can programmatically distinguish between "I passed bad flags" and "the template has a cycle."

3. **SignalReceived's invisible degradation is by design but undocumented.** The PRD should state whether this is the intended contract or whether a `"interrupted": true` field should be added.

4. **PersistenceError is misclassified.** A disk-full error is infrastructure (exit code 3), not a caller error (exit code 2). This should be a PRD decision.

5. **AGENTS.md (the plugin-shipped agent guide) is more current than cli-usage.md.** It documents `action: "confirm"` while the main docs don't. This creates a documentation fragmentation problem.

## Surprises

1. **ActionRequiresConfirmation introduces a third `action` value (`"confirm"`) that the main CLI documentation doesn't acknowledge.** The cli-usage.md says action is `"execute"` or `"done"`, but callers will see `"confirm"` too. Any caller that dispatches on the action field will hit an unknown case.

2. **All AdvanceError variants are uniformly mapped to `precondition_failed`.** I expected at least PersistenceError to get its own exit code or error code, since it's not a caller error. The lossy mapping means a disk-full error tells the agent "change your approach" rather than "report to user."

3. **The `advanced` field semantics differ between SignalReceived and other stop reasons.** For most stop reasons, `advanced: true` means "at least one transition happened." For SignalReceived, `advanced: true` means "at least one transition happened before the signal arrived, but the chain was cut short." Same field, subtly different meaning, no way for the caller to distinguish.

4. **IntegrationError::Failed is mapped to IntegrationUnavailable** (line 248-255 of advance.rs). When an integration runner fails, the error message is concatenated into the `name` field of IntegrationUnavailable (`format!("{}: {}", integration_name, msg)`). This means the `name` field can contain either a clean integration name or an error message, which is a type-level ambiguity callers can't predict.

## Open Questions

1. Should the PRD introduce new error codes for template-structural errors (cycle, ambiguous, dead-end), or is `precondition_failed` with distinct messages sufficient?

2. Should SignalReceived produce a distinct response shape (e.g., with an `interrupted` flag), or is the current transparent degradation the intended contract?

3. Should PersistenceError map to exit code 3 (infrastructure) instead of exit code 2 (caller error)?

4. Is the IntegrationError::Failed -> IntegrationUnavailable mapping intentional, or should Failed get its own stop reason with a separate error message field?

5. Who is the canonical audience for AGENTS.md vs. cli-usage.md? Should they be kept in sync, or does AGENTS.md serve a different purpose (agent-consumed vs. human-consumed)?

## Summary

The advancement loop has 10 StopReason variants and 4 AdvanceError variants, but only 5 of the 14 total outcomes are documented in user-facing guides (cli-usage.md and error-codes.md); the remaining 9 -- including the caller-triggerable ActionRequiresConfirmation, SignalReceived, and PersistenceError -- exist only in design docs or code. The main implication is that callers can encounter three `action` values (`execute`, `done`, `confirm`) but documentation only describes two, and six structurally different failures all collapse into a single `precondition_failed` error code with no programmatic way to distinguish them. The biggest open question is whether the PRD should break `precondition_failed` into distinct error codes or document the message-parsing approach as the intended contract.
