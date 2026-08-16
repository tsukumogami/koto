# Reviewer: completeness

## Verdict
PASS

All five required changes from the prior FAIL are resolved with grounded, testable additions, and the cross-reviewer edits (R14 scoping, the split concurrency criterion, the BRIEF-citing Out of Scope) introduce no new gaps.

## Requirement-to-criterion mapping

| Req | Criteria covering it | Status |
|---|---|---|
| R1 | gate-fail re-tick AC (first carries, second omits) | Covered |
| R2 | same AC | Covered |
| R3 | loop-back AC (conditional), self-transition AC, unconditional-transition AC, rewind AC, two directed-transition ACs, `koto init` AC, batch-child-init AC — all six arrival kinds R3 now names (conditional, unconditional, directed, self-transition, rewind, initialization) have a dedicated criterion | Covered |
| R4 | the two directed-transition ACs plus the "both directed-transition cases" line in the test-coverage AC | Covered |
| R5 | override-flag AC | Covered |
| R6 | new byte-identity AC against a pre-change baseline, backed by the Decisions entry on capturing that baseline | Covered |
| R7 | "invoked with only the workflow name" AC | Covered |
| R8 | same AC + variable-substitution AC | Covered |
| R9 | evidence-schema AC | Covered |
| R10 | "returns instructions where suppressed" AC + "does not change what the next `koto next` returns" AC | Covered |
| R11 | state-file-byte-identical AC, gate-not-executed AC, default-action-not-executed AC, terminal-no-cleanup AC | Covered |
| R12 | new split ACs: batch-scoped-lock-held case and non-batch respawn-race case | Covered |
| R13 | unknown-workflow AC; no-instructions-phase AC; terminal-phase AC now asserts a normal success envelope, not just no-cleanup | Covered |
| R14 | discoverability AC (now enumerates all six non-terminal response shapes) | Covered |
| R15 | directive-unaltered AC | Covered |
| R16 | no-new-file / schema-version AC | Covered |
| R17 | `koto-stability-tests` AC | Covered |
| R18 | new file-open-syscall-diff AC | Covered |
| R19 | test-coverage AC | Covered |
| R20 | koto-user docs AC | Covered |
| R21 | koto-author docs AC | Covered |
| R22 | cli-usage.md / cursor-rules AC | Covered |
| R23 | eval-retention AC | Covered |
| R24 | CHANGELOG AC | Covered |
| R25 | new `koto template compile` AC | Covered |

Orphans (criteria with no requirement behind them): `cargo fmt`/`clippy`/full-suite pass, and `wip/` hygiene. Both are standing workspace-wide boilerplate that every PRD in this chain carries — not a sign of missing scope, same as last pass.

## Findings

**The five required changes from the prior verdict are each closed on their own terms, not just nominally addressed.**

1. R18 is now written in caller-observable terms ("no file read it does not perform today... proportional to the session data koto already reads") and paired with a concrete syscall-diff criterion. The proportionality clause isn't independently tested by the syscall AC, but that clause is explicitly left to the DESIGN ("How that is achieved is the DESIGN's to decide"), and the syscall check is a legitimate, testable proxy for the substantive concern the original research raised (no new file reads on the `koto next` path). Not a gap.

2. R25 names the template-compilation merge gate directly, with a criterion that matches exactly what the plugin-validation workflow runs (`koto template compile` against every shipped template, triggered because R20–R23 require touching `plugins/koto-skills/`).

3. R13 now enumerates the terminal-phase case as an explicit non-error alongside the no-instructions case, and the matching criterion asserts a normal success envelope, not merely the absence of cleanup. This closes the exact gap the recovery-contract research flagged as needing an explicit PRD decision.

4. The new Definitions subsection resolves the R3 gap cleanly: it names the natural-advancement and directed-transition paths, defines occupancy (including the self-transition edge case, reasoned by analogy to loop-back rather than asserted), and R3's arrival list grew from five items to six (adding self-transition) — every one of the six now has its own acceptance criterion, including a criterion that separately exercises the unconditional-transition case even though it shares a code path with the conditional one.

5. R6's criterion is now a real byte-identity check against a pre-change baseline, and the Decisions section is honest that the baseline doesn't exist yet and is prerequisite work the implementation owes before the criterion is satisfiable — that's a legitimate way to handle a criterion whose test infrastructure doesn't pre-exist (the syscall-diff criterion for R18 has the same shape: it presupposes building two binaries to compare).

**The other reviewers' edits tighten rather than loosen completeness.** R14's scope-to-instruction-bearing-phases fixes a real latent contradiction with R6 that existed in the prior draft (an unconditional pointer on every response would have broken R6's "byte-identical to today" promise for instruction-free phases) — this is a correctness improvement, not new content that needs its own gap-check, and it's backed by both a positive AC (pointer present when instructions exist) and a negative one (no pointer when they don't). The split concurrency criterion (batch-lock case vs. non-batch respawn-race case) makes R12's coverage more precise than the single criterion it replaced, matching the recovery-contract research's own distinction between conditional batch-parent locking and the lock-free non-batch path. The discoverability criterion's enumeration of six non-terminal response shapes (adding `signal-received` to the five variants the recovery-contract research named) is grounded in the path-matrix research's own citation of `SignalReceived` as a shape that carries the same `directive`/`details` computation — if anything this is more complete than what either research doc enumerated alone. Out of Scope citing the BRIEF instead of restating its reasoning loses no content: all six BRIEF exclusions are still listed verbatim, plus the PRD's own addition, matching what I verified last pass.

**Re-ran the full rubric, not just the five items.** Every established defect and constraint from the three phase-2 research docs and the explore findings still has a requirement (rubric 1) — nothing regressed here since the edits were additive. The BRIEF's four user journeys are still each represented in the User Stories, unchanged from last pass (rubric 3). Out of Scope still matches the BRIEF's six exclusions plus one PRD-specific addition with a stated reason, in both directions (rubric 5). No Open Questions section exists, and the two new Decisions entries (pointer-scoping, baseline-capture) both read as resolved decisions with stated reasoning, not hidden unresolved questions (rubric 6).

No remaining redundant requirements, and no claim I could trace back to the PRD text lacks grounding in the BRIEF or the research.

## Required changes

None.
