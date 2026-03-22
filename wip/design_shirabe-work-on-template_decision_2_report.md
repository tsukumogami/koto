<!-- decision:start id="context-injection-state-design" status="assumed" -->
### Decision: context_injection State Design for Issue-Backed Workflows

**Context**

The shirabe /work-on koto template includes a `context_injection` state intended to mirror the real skill's Phase 0, which runs extract-context.sh and creates `wip/IMPLEMENTATION_CONTEXT.md`. This file carries design rationale forward into implementation — Phase 4 explicitly references it. The original design gated this state on `gh issue view {{ISSUE_NUMBER}} --json number --jq .number`, which checks issue accessibility but does not require or verify that context extraction happened. A panel review identified this as a core gap: "The entire context injection purpose is lost."

The design must choose between four approaches: (a) keep the accessibility-only gate with extraction in the directive; (b) gate on the context artifact file's existence; (c) split into two separate states; or (d) fold extraction into the analysis directive and drop the dedicated state.

The critical technical fact shaping this decision: the `--var` flag for template variable interpolation is not implemented in koto today. Gate commands containing `{{ISSUE_NUMBER}}` are inert at runtime — they don't interpolate. This eliminates option (a)'s gate and makes option (c)'s first state non-functional until --var ships. However, `test -f wip/IMPLEMENTATION_CONTEXT.md` uses a fixed path and works today, making option (b) viable without waiting for the CLI feature.

**Assumptions**

- The real extract-context.sh script creates `wip/IMPLEMENTATION_CONTEXT.md` (fixed path), consistent with what the skill implementer panel documented about Phase 4 referencing this specific file. If the real script uses a parameterized path, the gate path and directive references would need revision.
- The `--var` flag will eventually ship, at which point an accessibility pre-check state could be added cheaply. This decision does not block that future addition.
- extract-context.sh will exit non-zero if the GitHub issue is inaccessible, providing a natural early failure that makes an explicit accessibility gate less critical.

**Chosen: (b) Gate on context artifact file existence; extraction is the state's work**

The `context_injection` state's directive instructs the agent to run extract-context.sh and create `wip/IMPLEMENTATION_CONTEXT.md`. The gate is `test -f wip/IMPLEMENTATION_CONTEXT.md`. On first `koto next` call, the gate fails (file doesn't exist), returning `GateBlocked`. The agent reads the directive, runs the script, creates the file, then calls `koto next` again — the gate passes and the state auto-advances to `setup`. No evidence submission is needed.

The context file path `wip/IMPLEMENTATION_CONTEXT.md` is fixed (no --var dependency), matches the real skill's convention, and can be referenced statically in the implementation directive: "Before starting implementation, review `wip/IMPLEMENTATION_CONTEXT.md` if it exists."

**Rationale**

Option (b) is the only choice that simultaneously fixes the panel-identified gap, works today without --var, and uses koto enforcement correctly. The file-existence gate is verifiable — koto can confirm the artifact was created, unlike evidence-only states where the agent self-reports completion. The fixed path is already used in the real skill, so no new convention is needed.

Option (a) was the current design. Even with --var implemented, the accessibility gate auto-advances without verifying extraction happened. An agent following option (a) can call `koto next`, have the gate pass because the issue exists, and arrive at `setup` without ever running extract-context.sh. The panel critique was correct: option (a) loses the state's purpose.

Option (c) would be the strongest enforcement model — issue accessibility confirmed, then extraction verified — but requires --var for the accessibility gate to work. The marginal enforcement value of an explicit accessibility check doesn't justify an extra state when extract-context.sh provides the same early failure naturally. Option (c) is a future upgrade path once --var ships, not a current recommendation.

Option (d) abandons koto enforcement entirely. The whole value of a dedicated state is that it cannot be skipped. Folding context work into the analysis directive degrades this to a "please remember to do this" instruction with no structural guarantee.

**Alternatives Considered**

- **(a) Gate on accessibility; extraction in directive**: Gate is currently broken (no --var). Even when --var ships, the accessibility check auto-advances without verifying extraction. Agent can skip extraction entirely. Rejected because the enforcement purpose of the state is lost.
- **(c) Two separate states (accessibility + extraction)**: Stronger enforcement with clear separation of concerns, but the accessibility gate requires --var which isn't implemented. Adds a state for a check that extract-context.sh provides implicitly. Rejected as premature — a good upgrade path once --var ships.
- **(d) Fold into analysis directive**: No koto enforcement. Agent can skip context loading with no consequence in the state machine. Rejected because it gives up the core value of using koto for this workflow.

**Consequences**

What changes: `context_injection` becomes a retriable command-gated state. Gate is `test -f wip/IMPLEMENTATION_CONTEXT.md`. Directive instructs the agent to run extract-context.sh before koto can advance. The state does real work and koto verifies the output, not just the agent's word.

What becomes easier: Implementation directive can statically reference `wip/IMPLEMENTATION_CONTEXT.md` without variable interpolation. The file is guaranteed to exist by the time the agent reaches `setup`. Design context reliably reaches implementation.

What becomes harder: The state doesn't directly verify issue accessibility before extraction. This is acceptable because extract-context.sh will fail on inaccessible issues. The gate-with-evidence-fallback pattern (engine change) is not required for this state since there's no evidence schema — the gate is a hard block until the file exists.

One future improvement: when --var ships, add a lightweight `context_injection` pre-state that gates on `gh issue view {{ISSUE_NUMBER}}` and rename the current state to `context_extraction`. This gives two clean checkpoints at minimal cost.
<!-- decision:end -->
