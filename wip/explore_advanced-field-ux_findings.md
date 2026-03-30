# Exploration Findings: advanced-field-ux

## Core Question

The auto-advancement engine exists and works, but there's no authoritative spec for its behavioral contract. Callers misinterpret the `advanced` field and lack clear guidance on what each `koto next` response shape means and what they should do with it. We need a PRD that pins down the contract so behavior can't drift by accident.

## Round 1

### Key Insights

1. **No caller contract exists anywhere.** Every design doc specifies what the engine produces but none say what callers should do with it. No decision tree, no "when you see X, do Y." (leads: existing-design-docs, agents-md-contract)

2. **`advanced` has three different meanings depending on the code path.** For `--to`: hard-coded `true` (directed transition happened). For the loop path: "at least one auto-advance transition fired." Original design intent: "an event was appended before dispatching." Issue #89 flagged this overload; the fix was a post-implementation note saying "ignore `advanced`, use the response variant." (leads: existing-design-docs, transition-submit-interaction)

3. **9 of 14 possible outcomes are undocumented in user-facing guides.** cli-usage.md covers 5 response shapes; the code produces 14 distinct outcomes including `action: "confirm"` (undocumented in main guides), 6 errors collapsed into `precondition_failed`, and signal interruption that silently degrades. (lead: undocumented-edge-cases)

4. **Gate-with-evidence-fallback is a significant behavioral cliff with zero documentation.** When gates fail on a state with an `accepts` block, the engine returns EvidenceRequired instead of GateBlocked. Callers have no way to know gates failed. (leads: agents-md-contract, test-invariants)

5. **12+ files still reference the removed `koto transition` command.** Active guides including custom-skill-authoring.md tell agents to use a command that doesn't exist. (lead: transition-submit-interaction)

6. **13+ behavioral invariants are only encoded in tests, not in any prose doc.** Starting-state exemption for cycle detection, evidence clearing on auto-advance, action execution order relative to gates, decisions cleared after rewind. (lead: test-invariants)

7. **`--to` bypasses auto-advancement entirely and skips gate evaluation on the target state.** If a directed transition lands on a passthrough state, the caller must call `koto next` again. This is undocumented. (lead: transition-submit-interaction)

8. **AGENTS.md vs. cli-usage.md drift.** The plugin-shipped agent doc is more current (documents `action: "confirm"` and auto-advancement) while the main repo guide doesn't. Fragmented contract. (lead: agents-md-contract)

### Tensions

- "Ignore `advanced`" guidance vs. reality: the field ships in every response and callers rely on it. Deprecation without replacement leaves callers with less info.
- `precondition_failed` catch-all: 6 structurally different failures produce the same error code. PersistenceError is infrastructure, not caller error.
- `--to` and loop path give `advanced` different meanings with no documentation of the difference.

### Gaps

- Complete response shape catalog with all 14 outcomes
- Formal `advanced` field definition (or replacement)
- Caller decision tree for each response shape
- StopReason -> NextResponse mapping spec
- Edge case documentation (cycle, chain limit, signal, persistence)
- Stale `koto transition` references cleanup
- Gate-with-evidence-fallback documentation

### User Focus

User confirmed findings are sufficient and ready to decide on artifact type.

## Accumulated Understanding

The koto auto-advancement engine is well-implemented and well-tested, but its contract with callers was never specified as a standalone document. Design docs cover engine internals (loop mechanics, stopping conditions, edge cases). Test suites pin down 13+ behavioral invariants. But the caller-facing layer -- what JSON shapes callers see, what each field means, what callers should do in response -- exists only as scattered examples in guides that are partially outdated.

The `advanced` field is the visible symptom of a deeper problem: the output contract was built incrementally across three design efforts (CLI output contract, unified koto next, auto-advancement engine) without anyone writing down the unified caller perspective. Each design specified its piece; nobody specified the whole.

The path forward is a PRD that defines the complete caller-facing behavioral contract: every response shape, every field's semantics, every error code, and a decision tree for caller behavior. This PRD would also resolve the `advanced` field ambiguity (rename, redefine, or deprecate) and provide the authoritative spec that prevents accidental drift.

Secondary concerns surfaced during research:
- Whether `precondition_failed` should be split into distinct error codes
- Whether `--to` should trigger auto-advancement after landing
- Whether SignalReceived should produce a distinct response shape
- Cleanup of stale `koto transition` references across 12+ files

## Decision: Crystallize
