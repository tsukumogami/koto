# Reviewer: clarity

## Verdict
PASS

All six required changes from the prior round are resolved cleanly, and the new Definitions subsection and scoped R14 hold up under the same altitude/ambiguity/citation checks applied to the rest of the document.

## Findings

### Re-check of the six required changes

1. **R6/R14 contradiction — resolved.** R6 now reads "The discoverability pointer of R14 does not appear on such a response, so this requirement and R14 do not compete," R14 is scoped to "every non-terminal response for a phase that declares instructions," and Goal 5 matches ("a channel that reaches it on every response for a phase that has instructions to recover"). A new Decisions entry ("The discoverability pointer is scoped to phases that declare instructions") names the trade-off honestly: an instruction-free workflow's agent never learns the retrieval exists, argued as acceptable because such an agent has nothing to recover. No remaining tension between the two requirements.

2. **R6's criterion — resolved.** It now requires "the full response body is byte-identical to the response the pre-change binary produces for the same template and the same sequence of calls," not just absence of the instructions field. A Decisions entry ("The byte-identity baseline does not exist yet and must be captured first") correctly flags that this criterion needs new test infrastructure before it's satisfiable — good, since an AC that can't be run against the current suite is worth calling out rather than leaving implicit.

3. **"Occupancy" — resolved.** The new Definitions subsection gives a mechanical definition ("begins when a state-entry event names that phase as its target, and ends when the next state-entry event names any phase, including the same one") and explicitly settles the self-transition case, with a stated rationale for why self-transition gets the same treatment as any other re-entry. A matching Acceptance Criterion was added. "State-entry event" leans on vocabulary the Problem Statement already establishes (koto's existing log already records entries into a state), so this reads as citing an existing mechanism rather than introducing new architecture.

4. **"Advance" — resolved.** The Definitions subsection names the two code paths explicitly (natural-advancement, directed-transition), states both advance the workflow, and pins down "does not advance the workflow" to mean no transition of either kind occurred. R4 and R2 now use consistent vocabulary.

5. **R18 — resolved.** Now states the bound in caller-observable terms ("performs no file read it does not perform today," "any added per-call work stays proportional to the session data koto already reads") and explicitly defers the mechanism: "How that is achieved is the DESIGN's to decide." The companion Acceptance Criterion adds a concrete verification method (comparing file-open syscalls via `strace`) — that's a legitimate test technique in an AC, not an architecture decision in a requirement.

6. **Out of Scope — resolved.** Now opens with an explicit citation to the BRIEF's Scope Boundary and carries only the bare exclusion list, with the one PRD-specific exclusion (directed-transition gate evaluation) kept separate and justified in this document. No duplicated reasoning.

### New text checked against the full rubric

- **Altitude.** The Definitions subsection, R25, and the new Acceptance Criteria (advisory-lock race test, strace-based file-read check, enumerated response-shape list for discoverability) all name *existing* system concepts (the session log's state-entry events, the existing advisory lock, the existing response-shape variants, the existing plugin-validation CI workflow) rather than prescribing new mechanisms — consistent with how R17/R20-R24 were already judged legitimate WHAT in the prior round.
- **Ambiguity.** No new contradictions found. One residual minor point, not blocking: R3 lists "a self-transition" as its own item alongside "a conditional transition, an unconditional transition, ... a rewind," while the Definitions subsection says koto has only two code paths (natural-advancement, directed-transition). It's inferable that self-transition and rewind are each specific cases running through one of those two paths rather than separate paths, and the Occupancy definition makes the actual behavior unambiguous regardless of which path carries them — so this doesn't leave two engineers building different things, just a slightly loose taxonomy. Not required, but worth a sentence if the authors want to close it entirely.
- **Undefined terms.** "Occupancy" and "advance"/the two transition-path names are now defined and used consistently everywhere they appear afterward, including in the new Acceptance Criteria.
- **Requirements vs Acceptance Criteria.** No new confusion. The new ACs (self-transition arrival, unconditional-transition arrival, terminal-phase retrieval as success not error, the two lock/race scenarios, the enumerated response-shape list, the syscall-diff check) are all scenario-level verifications of existing requirements, not restated requirements.
- **Writing style.** Clean — no hits for the banned words, no emojis, no preamble phrasing in the new text.

## Required changes

No required changes.
