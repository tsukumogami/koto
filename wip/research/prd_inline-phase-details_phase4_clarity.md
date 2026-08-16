# Reviewer: clarity

## Verdict
FAIL

The document contains a direct, unreconciled contradiction between R6/Goal-6 and R14/Goal-5 that makes the delivery-rule contract inconsistent for the one case (a phase with no instructions) it explicitly claims to leave untouched, plus two genuine term-definition gaps ("occupancy," "advance") that two competent engineers would resolve differently.

## Findings

### 1. Altitude

Mostly clean. Requirements read as outcomes, not mechanisms, with two exceptions:

- **R18.** "Per-call cost stays within the same order as today's derivation: one additional pass over an already-read event list, with no new file reads on the `koto next` path." The clause "one additional pass over an already-read event list" names an algorithmic technique (iterate the list once more), not an externally observable outcome. A non-functional requirement should bound cost in caller-visible terms (no new I/O, same asymptotic order) without prescribing that the computation be a single linear pass — that forecloses, e.g., a cached/incremental approach the DESIGN might legitimately prefer.
- **R11.** "The retrieval performs none of the following: ... writing to the request store, or advancing a discovery cursor." "The request store" and "a discovery cursor" are internal component names. This is defensible as *preserving an existing invariant* rather than *introducing a mechanism* (same logic the task gave for R20-R24), so I did not fail the document over it, but it sits closer to the line than R7/R12, which state the same kind of constraint in caller-observable terms ("does not block," "requires no argument"). Worth the DESIGN author's attention, not a required change.
- **R20-R24 judgment (asked for explicitly):** legitimate WHAT. Each names a deliverable *surface* that must be accurate after the change ("these docs describe X as shipped"), not a mechanism for producing the change. This is the same pattern as citing an existing test suite in R17. No issue.
- **Acceptance criterion** "`cargo fmt --check`, `cargo clippy -D warnings`, and the full test suite pass" names specific tooling commands rather than an outcome ("code passes the repository's lint and format gates"). Minor; if this is a standing convention repeated across this workspace's PRDs, it's not worth blocking on alone, but it is HOW by the letter of the rule.

### 2. Ambiguity — the one that matters

**R6** says: "A phase that declares no instructions produces responses byte-identical to those koto produces today."

**R14** says: "A pointer to the retrieval reaches the agent on a channel that is present in every non-terminal response, so an agent that has lost its context learns the retrieval exists without having retained anything."

These cannot both hold. R14 puts a new pointer field on *every* non-terminal response, full stop — not conditioned on the phase carrying instructions. R6 promises byte-identical output for phases that declare no instructions. If the pointer is genuinely on every non-terminal response, a no-instructions phase's response is no longer byte-identical to today's (which carries no such field); if R6 is to hold, R14's "every non-terminal response" must actually mean "every response for a phase that has (or the rule would suppress) instructions," which is not what it says.

This isn't a one-off wording slip — it's structural. It's restated in the Goals section too:

- Goal 6: "Templates that attach no instructions behave exactly as they do today."
- Goal 5: "An agent that has lost its context still learns that the retrieval exists, through a channel that reaches it on every response."

Same collision, same two bullets, unresolved. Two competent engineers would build materially different things here: one would gate the pointer on instructions being attached to the phase (satisfying R6, narrowing R14/Goal-5's discoverability guarantee to only the phases where it's needed); the other would put the pointer on every response unconditionally (satisfying R14/Goal-5, breaking R6/Goal-6's byte-identical promise). The PRD doesn't say which is right, and the Acceptance Criteria quietly duck the question — see item 6 below.

**Other requirement-level ambiguities:**

- **R7**: "requires no argument the caller would have had to memorize from an earlier response." "Memorize" is doing real work here (it's what makes the retrieval usable by a context-compacted agent) but is fuzzy at the edge: if a future design obtains a value via a separate discovery call rather than "an earlier response," does that satisfy or violate R7? Minor; not blocking, but worth a sentence of tightening.

### 3. Undefined / shifting terms

- **"occupancy"** appears exactly once, in R1: "during the current occupancy of that phase." It is never defined. R2/R3 use "arriving" and "not advance the workflow" to gesture at the same concept, and R3 enumerates the ways of *arriving* (conditional, unconditional, directed, rewind, init), but the document never says when an occupancy *ends*, and in particular never addresses a self-transition — a phase that transitions back into itself (e.g., a conditional retry loop where source and destination phase are the same). Does that end the current occupancy (instructions re-delivered) or continue it (instructions withheld, per R2's "does not advance" reading — except a self-transition *does* transition, so R2 doesn't obviously apply either)? The document gives no way to resolve this, and it's a real template pattern, not an edge case invented for this review.

- **"advance"** shifts meaning between two requirements:
  - R2: "A response that does not **advance** the workflow" — here "advance" means "no transition occurred at all" (the gate-blocked re-tick case).
  - R4: "R1 through R3 hold identically on the directed-transition path and on the **advance path**." Here "advance path" is a noun naming one specific transition mechanism (conditional/unconditional), held up against "the directed-transition path" as if the latter doesn't also advance the workflow — but a directed transition unambiguously does transition the workflow forward.
  
  A reader has to reverse-engineer that "advance path" in R4 means "the non-directed transition mechanism," which is a different sense of the word than R2's verb use. This is exactly the kind of shift the rubric asks about, and it's adjacent to the "occupancy" gap above — both center on what exactly demarcates one phase-visit from the next.

- **"delivery"**, **"arrival"**, and **"the rule"** are used consistently throughout and are well-anchored by concrete examples (R2/R3's enumerations, the Problem Statement's three-path breakdown). No issue with these three.

### 4. Problem Statement standalone-ness

Passes. The Problem Statement is fully self-contained — it re-derives the phase/instructions/directive mechanism, the three break paths, the retrieval gap, and the measured cost without requiring the BRIEF. This matches the format contract's requirement that the Problem Statement (unlike every other section) restates rather than cites.

**Out of Scope, however, is a restatement problem.** Every one of its seven bullets closely paraphrases the corresponding BRIEF bullet, carrying the same reasoning nearly clause-for-clause, with no citation back to the BRIEF anywhere in the section (the document cites the BRIEF only in the frontmatter `upstream:` field and once in the Status section). Example:

> BRIEF: "Auto-advance discarding the phases it crosses. ... It predates this mechanism and is broader than it — the honest framing is that auto-advance has never surfaced intermediate instructions at all, and folding it in here would disguise that as a details regression. Filed separately."
>
> PRD: "Auto-advance discarding the phases it crosses. A `koto next` that advances through an intermediate phase surfaces neither that phase's instructions nor its directive. It predates this mechanism, is broader than it, and would mis-frame as a delivery regression. Filed separately."

This is the "second copy that drifts" the format contract warns against under Citation vs Restatement — the reasoning is duplicated, not cited, and nothing points a reader back to the BRIEF's Scope Boundary as the source. (Out of Scope is a required PRD section, so it legitimately needs its own list of exclusions — the issue isn't that it exists, it's that the *justification prose* is re-derived rather than pointed at.)

### 5. User Stories

No issues. Each of the six stories covers a distinct scenario (blocked gate, rewind, compaction, respawn, authoring, test coverage) and connects to a concrete, non-generic payoff. The five agent-role stories are the same underlying actor (an AI agent) in different situations rather than five different roles, but the format contract's actual bar is "distinct scenario," which each one clears.

### 6. Requirements vs Acceptance Criteria confusion

One real instance, tied directly to finding #2: the Acceptance Criterion meant to verify R6 —

> "A template whose phases declare no instructions produces responses with no instructions field, on every path above."

— silently narrows R6's claim. R6 says "byte-identical," which would include the new discoverability field from R14; the AC only checks for the absence of the *instructions* field. It doesn't verify byte-identity at all, so it can't actually catch the contradiction in finding #2 — a shipped implementation that adds the R14 pointer to every response would pass this AC while violating R6 as written. The AC should either be widened to actually test byte-identity (making the R6/R14 conflict fail loudly in CI), or R6 itself needs to be the thing that changes.

No other criterion/requirement pairs showed this problem — the rest of the Acceptance Criteria read as genuine scenario-level verifications of their requirements, not restatements.

### Writing style

Clean. No hits for "tier/tiered," "robust," "leverage," "comprehensive/holistic," "facilitate," or preamble phrases. No emojis. Sentence length varies naturally with the density of each claim.

## Required changes

1. **Resolve the R6/R14 (and Goal-6/Goal-5) contradiction.** Either (a) scope R14's pointer to phases where instructions exist or the rule would suppress them, and change Goal 5 / R14's "every non-terminal response" accordingly, or (b) amend R6 and Goal 6 to "byte-identical ... aside from the discoverability pointer introduced by R14." As written, satisfying one falsifies the other. This is a decision for the PRD authors (or an explicit note deferring it to DESIGN with the tension named), not something a reviewer can pick for them.

2. **Widen or replace the R6-verifying Acceptance Criterion** ("produces responses with no instructions field") so it actually tests "byte-identical," once #1 is resolved — otherwise the contradiction ships silently.

3. **Define "occupancy" explicitly** in R1 or a shared definitions note: when does a phase's current occupancy begin and end, and specifically — does a self-transition (a phase transitioning back into itself) start a new occupancy or continue the existing one? R2/R3 don't settle this by implication.

4. **Reconcile "advance."** R2 uses "advance" to mean "any transition occurred"; R4's "the advance path" names one specific transition mechanism as distinct from "the directed-transition path," even though a directed transition also advances the workflow. Rename R4's "advance path" to something that doesn't reuse R2's verb in a different sense (e.g., "the conditional/unconditional-transition path"), or add a clarifying sentence.

5. **Rewrite R18** to state the performance bound in caller-observable terms and drop the algorithmic prescription: something like "no new file reads are added on the `koto next` path, and the additional per-call cost stays proportional to the size of the event data koto already reads" — without specifying "one additional pass."

6. **Cite rather than restate the BRIEF's Out of Scope reasoning.** Point each bullet (or the section as a whole) back to the BRIEF's Scope Boundary rather than re-deriving the same justification prose, so the two documents don't carry two copies of the same reasoning that can drift independently.
