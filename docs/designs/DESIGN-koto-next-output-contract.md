---
status: Proposed
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

Proposed

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
