---
status: Accepted
upstream: docs/prds/PRD-koto-next-output-contract.md
problem: |
  koto next produces six NextResponse variants and 14+ error paths, but the action
  field collapses four variants into "execute", error codes conflate fixable and
  unfixable failures under precondition_failed, and the directive field carries all
  instructions regardless of whether the caller has seen them before. The Rust type
  system (NextResponse enum, NextErrorCode enum, custom Serialize impl) and the
  CLI handler (advance_until_stop -> StopReason -> NextResponse mapping) need
  coordinated changes across src/cli/, src/engine/, and the koto-skills plugin.
decision: |
  Rename action values in the Serialize impl (not the Rust enum names), add
  blocking_conditions to EvidenceRequired via StopReason threading, add details
  field with visit-count-based conditional inclusion, split error codes by
  actionability with new NextErrorCode variants, and migrate unstructured errors
  to NextError format. Update AGENTS.md, koto.mdc, and koto-author materials
  in the same release.
rationale: |
  The changes touch four code layers (template types, engine advance, CLI next_types,
  CLI handler) plus documentation. The action rename is purely a Serialize change --
  Rust enum names stay the same, minimizing refactor scope. The details field requires
  threading visit counts from the JSONL log through to the response serializer, which
  is the most structurally invasive change. Error code splitting is additive to the
  NextErrorCode enum. All changes are coordinated to ship as one contract version.
---

# DESIGN: koto next output contract

## Status

Accepted

## Context and problem statement

The `koto next` command's output layer was built incrementally across three design efforts. The CLI output contract (#37) defined the `NextResponse` enum and custom serialization. The unified koto next design (#43) added auto-advancement. The auto-advancement engine (#49) added the advancement loop with `StopReason` -> `NextResponse` mapping. Each effort extended the output without reconciling the caller-facing contract.

The technical problems are concrete:

1. **Action field collapse.** The `Serialize` impl for `NextResponse` writes `"execute"` for four of six variants (EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable). Callers must reconstruct the variant from field presence -- a fragile pattern that breaks when new fields are added.

2. **Error code conflation.** `NextErrorCode::PreconditionFailed` covers caller errors (bad flags), template bugs (cycle detected), and infrastructure failures (persistence error). The `Err(advance_err)` catch-all in the CLI handler maps all `AdvanceError` variants to `precondition_failed` with exit code 2, telling agents "change your approach" for failures they can't fix.

3. **Gate-with-evidence-fallback is invisible.** When `advance_until_stop` falls through from a gate failure to `EvidenceRequired` (because the state has an accepts block), the `StopReason::EvidenceRequired` variant carries no gate data. The gate results are computed but discarded.

4. **Directive is all-or-nothing.** `TemplateState.directive` is a single string. States with long instructions repeat the full text on every `koto next` call, wasting context window for callers that already have the instructions.

5. **Unstructured errors.** Several error paths (template read failures, hash mismatches, state-not-found) produce ad-hoc JSON (`{"error": "<string>", "command": "next"}`) instead of the `NextError` format.

The engine logic is correct. The changes are to the serialization layer, error classification, and response field population -- not to state machine semantics.

## Decision drivers

- **Minimize engine changes.** The advancement loop and transition resolution work correctly. Changes should be in the serialization and classification layers, not the core state machine.
- **Ship as one contract version.** Callers shouldn't face partial upgrades where some action values changed but error codes didn't. All contract changes land together.
- **Backward compatibility where possible.** `"done"` and `"confirm"` action values stay. New fields (`blocking_conditions` on EvidenceRequired, `details`) are additive. The `advanced` field stays.
- **Breaking change is acceptable for `action`.** koto is pre-1.0. Current callers are internal skill plugins that ship in the same repo and can be updated atomically.
- **Template format for details is a design decision.** The PRD specifies the output contract (details field behavior). This design must choose the template source format.
- **Documentation is part of the deliverable.** AGENTS.md, koto.mdc, koto-author skill materials, and template-format.md must be updated in the same release.

## Considered options

### Decision 1: Template source format for the directive/details split

Templates need a way for authors to separate short summary text (returned every time) from extended instructions (returned only on first visit). The compiled format is settled: `TemplateState` gets `directive: String` + `details: String`. But the source format -- how authors express the split in their markdown template -- has three viable options.

The choice matters for the koto-author skill (which teaches the format), template-format.md (which documents it), and the compiler (which parses it). It doesn't affect the engine, CLI handler, or response serialization.

#### Chosen: Markdown separator (`<!-- details -->`)

Authors add an HTML comment `<!-- details -->` within a state's `## heading` section. Content before the marker becomes `directive`; content after becomes `details`. States without the marker behave identically to today.

```markdown
## analyze

Read the issue body and identify acceptance criteria.

<!-- details -->

### Steps

1. Run `gh issue view {{ISSUE}} --json body` to fetch the full issue.
2. Extract each acceptance criterion into a checklist.
3. If the issue references a design doc, read it and cross-reference.
```

The compiler change is localized to `extract_directives` in `src/template/compile.rs`: after collecting lines for a state section, split on the first `<!-- details -->` line. Lines before become `directive`, lines after become `details`. No YAML schema changes. The marker is an HTML comment, invisible in GitHub rendered previews and unambiguous (unlike `---` which is overloaded in markdown).

#### Alternatives considered

**YAML summary field**: Add an optional `summary` field to the YAML state declaration. When present, `summary` becomes `directive` and the markdown body becomes `details`. Rejected because it breaks the existing pattern where directives come from the markdown body (not YAML), forces content into two locations, and YAML multiline strings are awkward for markdown-formatted content.

**External file reference (`details_file`)**: Add an optional YAML field pointing to a separate markdown file inlined at compile time. Rejected because it breaks the single-file template model, adds file resolution complexity to the compiler, and creates the highest maintenance burden for template authors.

### Decision 2: Threading gate results through StopReason

When gates fail on a state with an `accepts` block, the engine falls through to `EvidenceRequired` instead of `GateBlocked`. But `StopReason::EvidenceRequired` is currently a unit variant -- it carries no gate data. The CLI handler can't populate `blocking_conditions` on the response because the information was discarded in the engine.

#### Chosen: Add gate data to StopReason::EvidenceRequired

Change `StopReason::EvidenceRequired` from a unit variant to a struct variant carrying `Option<BTreeMap<String, GateResult>>`. The engine passes gate results when gates failed, `None` otherwise. The CLI handler converts to `Vec<BlockingCondition>` using the existing conversion logic (extracted into a shared helper to eliminate the current duplication).

```rust
pub enum StopReason {
    // ...
    EvidenceRequired {
        failed_gates: Option<BTreeMap<String, GateResult>>,
    },
    // ...
}
```

In `advance.rs`, `gate_results` is hoisted out of the gate evaluation block so it's available when constructing the EvidenceRequired return. The construction passes `Some(gate_results)` when `gates_failed` is true, `None` otherwise.

The `GateResult -> BlockingCondition` conversion logic (currently duplicated in `src/cli/next.rs` lines 42-55 and `src/cli/mod.rs` lines 1684-1697) is extracted into a shared `blocking_conditions_from_gates` function.

#### Alternatives considered

**Re-evaluate gates in the CLI handler**: After the loop returns EvidenceRequired, call `evaluate_gates` again on the final state. Rejected because gate commands may produce different results on a second run (non-deterministic), it doubles execution time, and the CLI layer shouldn't be making engine-level evaluation decisions.

**Thread gate results through a separate AdvanceResult field**: Add `gate_results: Option<...>` to `AdvanceResult` alongside `stop_reason`. Rejected because it scatters related data across two struct fields -- the compiler can't enforce that handlers check the separate field, and it pollutes the common result type with data meaningful for only 2 of 9 stop reasons.

### Decision 3: Visit count computation

The `details` field should be included on first visit to a state and omitted on subsequent visits. The JSONL event log contains all state-entry events (`Transitioned`, `DirectedTransition`, `Rewound`), and the PRD prohibits new state files or schema changes. The question is how to compute and propagate visit information.

#### Chosen: `derive_visit_counts` with `HashMap<String, usize>`

Add a `derive_visit_counts(events: &[Event]) -> HashMap<String, usize>` function to `src/engine/persistence.rs`. It scans all events once, incrementing a counter for each state name that appears as a `to` field in entry events. The CLI handler calls it alongside `derive_state_from_log` and passes the count for the final state to the response construction layer.

```rust
pub fn derive_visit_counts(events: &[Event]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for event in events {
        let target = match &event.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if let Some(state_name) = target {
            *counts.entry(state_name.clone()).or_insert(0) += 1;
        }
    }
    counts
}
```

First visit = count of 1 (the transition event for the current state is already in the log when `koto next` reads it). The `--full` flag bypasses the check at the serialization layer, not inside `derive_visit_counts`.

#### Alternatives considered

**Boolean scan with `HashSet<String>`**: Same approach but returns presence, not count. Rejected because it's strictly less capable for identical implementation complexity -- counts support future features (loop detection, retry budgets) at zero extra cost.

**Persist visit count alongside events**: Add a running counter to derived state or a separate tracking file. Rejected because it violates PRD R9 ("no new state files or schema changes").

## Decision outcome

### Summary

The output contract changes span four code layers, all shipping together. At the serialization layer, the custom `Serialize` impl on `NextResponse` changes the `action` strings from `"execute"` to descriptive names (`"evidence_required"`, `"gate_blocked"`, `"integration"`, `"integration_unavailable"`) while keeping `"done"` and `"confirm"` unchanged. Rust enum variant names stay the same -- only the wire format changes.

At the engine layer, `StopReason::EvidenceRequired` gains an `Option<BTreeMap<String, GateResult>>` field that carries gate failure data through to the CLI handler. The handler converts this to a `blocking_conditions` array on the `EvidenceRequired` response: empty when gates passed or weren't evaluated, populated when gates failed and the state accepts evidence. The duplicated `GateResult -> BlockingCondition` conversion is extracted into a shared helper.

At the template layer, a `<!-- details -->` marker within state markdown sections splits content into `directive` (before marker, always returned) and `details` (after marker, returned only on first visit). The compiler's `extract_directives` function splits on the marker. A new `derive_visit_counts` function in `persistence.rs` scans the JSONL event log to count state entries, and the CLI handler uses the count to conditionally include `details` in the response. The `--full` flag bypasses the visit check.

At the error layer, `NextErrorCode` gains `TemplateError`, `PersistenceError`, and `ConcurrentAccess` variants. The blanket `Err(advance_err)` catch-all in the CLI handler splits into per-variant mapping: template structural errors -> `template_error` (exit 3), disk I/O -> `persistence_error` (exit 3), lock contention -> `concurrent_access` (exit 1). Unstructured error paths (`{"error": "...", "command": "next"}`) are migrated to the `NextError` format.

Documentation (AGENTS.md, koto.mdc, koto-author SKILL.md and template-format.md, example templates) updates in the same release. The koto-author skill's template dogfoods the `<!-- details -->` marker on its longer states.

### Rationale

The decisions reinforce each other through a clean layering: the template format decision (D1) feeds the compiler, the gate threading decision (D2) feeds the engine, and the visit counting decision (D3) feeds the persistence layer. All three converge at the CLI handler, which constructs `NextResponse` from compiled template data + engine results + visit counts. No decision constrains another.

The breaking change to `action` values is the right trade-off: koto is pre-1.0, callers are internal skill plugins in the same repo, and the gain (callers dispatch on `action` alone, no field-presence reconstruction) is permanent. The `advanced` field stays unchanged because the breaking change budget is better spent on `action`, and the PRD formally defines `advanced` as informational-only.

## Solution architecture

### Overview

The changes touch four code layers that converge at the CLI handler. Each layer is modified independently, and the handler orchestrates the data flow from template -> engine -> persistence -> serialization.

### Components

**1. Template compiler** (`src/template/compile.rs`, `src/template/types.rs`)
- `extract_directives` gains `<!-- details -->` splitting logic
- `TemplateState` gains `details: String` field in `src/template/types.rs` (`#[serde(default, skip_serializing_if = "String::is_empty")]`)
- No changes to YAML frontmatter parsing

**2. Engine advance** (`src/engine/advance.rs`)
- `StopReason::EvidenceRequired` changes from unit variant to `EvidenceRequired { failed_gates: Option<BTreeMap<String, GateResult>> }`
- `gate_results` hoisted from the gate evaluation block to be available at the EvidenceRequired construction site
- All match arms on StopReason updated for destructuring

**3. Persistence** (`src/engine/persistence.rs`)
- New `derive_visit_counts(events: &[Event]) -> HashMap<String, usize>` function
- Follows the existing `derive_*` pattern (pure function, `&[Event]` input)

**4. CLI next_types** (`src/cli/next_types.rs`)
- `NextResponse::EvidenceRequired` gains `blocking_conditions: Vec<BlockingCondition>` field (always present, empty when no gate issues)
- All non-terminal variants gain `details: Option<String>` field
- Custom `Serialize` impl changes action strings: `"execute"` -> variant-specific names
- `Serialize` conditionally includes `details` (present when `Some`, absent when `None`)
- `with_substituted_directive` extended to also apply `{{VARIABLE}}` substitution to the `details` field (same two-pass substitution as `directive`)
- New shared function: `blocking_conditions_from_gates(gate_results: &BTreeMap<String, GateResult>) -> Vec<BlockingCondition>`

**5. CLI dispatch** (`src/cli/next.rs`)
- `dispatch_next` is retained -- it's the only code path for `--to` directed transitions
- Its gate-with-evidence-fallback logic (lines 60-73) is updated to use `blocking_conditions_from_gates` shared helper instead of inline conversion
- The gate-to-EvidenceRequired fallback in `dispatch_next` stays because `--to` skips the advancement loop and needs its own classification
- The doc comment ("five possible responses") corrected to six (includes ActionRequiresConfirmation)

**6. CLI handler** (`src/cli/mod.rs`)
- `NextErrorCode` gains `TemplateError`, `PersistenceError`, `ConcurrentAccess` variants with exit codes
- The `Err(advance_err)` catch-all splits into per-variant mapping
- Unstructured error paths migrated to `NextError` format
- After `advance_until_stop` returns, calls `derive_visit_counts` and passes count to response construction
- `--full` flag added to CLI arg parsing, bypasses visit check
- Response construction populates `details` from `TemplateState.details` when visit count == 1 or `--full` is set

**7. Documentation** (`plugins/koto-skills/`)
- AGENTS.md: action values, error codes, blocking_conditions, details, advanced definition
- `.cursor/rules/koto.mdc`: full rewrite to current API
- koto-author SKILL.md: execution loop section updated
- template-format.md: `<!-- details -->` documented as Layer 1 concept, feature-to-action mapping added
- Example templates: at least one demonstrates `<!-- details -->`
- koto-author's own template: dogfoods `<!-- details -->` on state_design and template_drafting

### Key interfaces

**Response JSON shapes (exit 0):**

```
action: "evidence_required"  -- state, directive, details?, advanced, expects, blocking_conditions, error: null
action: "gate_blocked"       -- state, directive, details?, advanced, expects: null, blocking_conditions, error: null
action: "integration"        -- state, directive, details?, advanced, expects?, integration, error: null
action: "integration_unavailable" -- state, directive, details?, advanced, expects?, integration, error: null
action: "confirm"            -- state, directive, details?, advanced, action_output, expects?, error: null
action: "done"               -- state, advanced, expects: null, error: null
```

`details?` = present on first visit (or with `--full`), absent on subsequent visits and when state has no details.

**Error JSON shape (exit 1, 2, 3):**

```json
{"error": {"code": "<string>", "message": "<string>", "details": [...]}}
```

All error paths use this format. No unstructured errors.

**New error codes:**

| Code | Exit | Replaces |
|------|------|----------|
| `template_error` | 3 | `precondition_failed` for CycleDetected, ChainLimitReached, AmbiguousTransition, DeadEndState, UnresolvableTransition, UnknownState |
| `persistence_error` | 3 | `precondition_failed` for PersistenceError |
| `concurrent_access` | 1 | `precondition_failed` for lock contention |

### Data flow

```
Template .md --[compile]--> TemplateState { directive, details } --[load]-->
                                                                            |
Events .jsonl --[read]--> [Event] --[derive_visit_counts]--> HashMap -------+
                                  --[derive_state_from_log]--> current_state |
                                  --[merge_epoch_evidence]--> evidence ------+
                                                                            |
                           advance_until_stop(state, template, evidence) ----+
                                       |                                    |
                              AdvanceResult { stop_reason, advanced } ------+
                                       |                                    |
                              CLI handler: visit_count + stop_reason --------+
                                       |                                    |
                              NextResponse { action, directive, details?, blocking_conditions, ... }
                                       |
                              JSON serialization --> stdout
```

## Implementation approach

### Phase 1: Engine and type changes

Core Rust changes that don't affect the wire format yet.

Deliverables:
- `TemplateState.details` field in `src/template/types.rs`
- `extract_directives` splitting in `src/template/compile.rs`
- `StopReason::EvidenceRequired { failed_gates }` in `src/engine/advance.rs`
- `derive_visit_counts` in `src/engine/persistence.rs`
- `blocking_conditions_from_gates` shared helper in `src/cli/next_types.rs`
- Updated tests for all of the above

### Phase 2: Wire format changes

The breaking changes to JSON output.

Deliverables:
- Action value rename in `NextResponse` custom Serialize impl
- `blocking_conditions` field on `EvidenceRequired` serialization
- `details` field on all non-terminal variants (conditional on visit count)
- `--full` flag in CLI arg parsing
- `NextErrorCode` new variants (`TemplateError`, `PersistenceError`, `ConcurrentAccess`)
- Per-variant error mapping replacing the catch-all
- Unstructured error migration to `NextError` format
- Updated integration tests and functional feature tests (~12 serialization tests assert `action == "execute"` and need updating)

### Phase 3: Documentation

All doc updates, shipping with Phase 2.

Deliverables:
- AGENTS.md rewrite (action values, error codes, blocking_conditions, details, advanced)
- `.cursor/rules/koto.mdc` rewrite
- koto-author SKILL.md execution loop update
- template-format.md `<!-- details -->` section + feature-to-action mapping
- Example template with `<!-- details -->`
- koto-author template dogfooding

## Consequences

### Positive

- Callers dispatch on `action` alone -- no field-presence reconstruction needed
- Error codes tell callers what to do (fix input / report template bug / retry) instead of lumping everything into "precondition failed"
- Gate-with-evidence-fallback is visible: callers can see which gates failed and decide whether to override via evidence
- Long directives don't waste context window on repeat visits
- One authoritative contract spec (AGENTS.md) replaces scattered, inconsistent documentation

### Negative

- Breaking change to `action` values requires updating all callers in the same release
- `EvidenceRequired` responses gain `blocking_conditions` (always-present array), adding ~20 bytes to every evidence-required response even when no gates are involved
- `derive_visit_counts` scans the full event log on every `koto next` call, adding O(n) work proportional to workflow length
- The `<!-- details -->` marker is a convention that authors must learn -- it's not self-documenting

### Mitigations

- All callers (AGENTS.md consumers, koto.mdc, koto-author skill) are in the same repo and updated atomically
- The empty `blocking_conditions` array is ~25 bytes -- negligible compared to directive text
- Typical workflows have tens to low hundreds of events; the full scan is sub-millisecond
- The koto-author skill teaches the marker during the template_drafting phase, and template-format.md documents it as a Layer 1 concept

## Security considerations

No security dimensions apply to this design. It restructures how `koto next` serializes responses and classifies errors, operating entirely on data already loaded in memory from local files with owner-only permissions. The new `blocking_conditions` and `details` fields expose template-authored content and gate evaluation results that were previously computed but discarded -- this is the design's intended behavior, not an unintended leak. No new external inputs, dependencies, network access, or privilege changes are introduced.
