# Category B: Design Fidelity

## Verdict
findings

## Critical findings

- **Issue 4, AC2** ("The pointer's presence keys on whether the phase declares instructions, not on whether this response carries them, so it appears on exactly the responses where they were suppressed.") misstates the design's own ruling and contradicts Issue 4's AC1 in the same issue. The design's "one thing the pointer must not key on" section says the two conditions ("declares instructions" vs. "carries instructions") *differ* only on suppressed responses — meaning the pointer appears on suppressed responses *in addition to* carrying ones, not *instead of* them. AC1 already states the pointer appears unconditionally "when the current phase declares instructions" (no delivery-status qualifier); AC2 then narrows that to "exactly the responses where they were suppressed," which excludes first-delivery/carrying responses the design and AC1 both require the pointer on. Left as written, this AC could be read as license to build a pointer that appears only when suppressed — the opposite of the design's "declares, not carries" ruling. Correction hint: reword AC2 to state the pointer appears on every response for a phase that declares instructions, including — not exclusively — the suppressed ones, e.g. "...so it appears both on responses that carry the instructions and on the suppressed ones a recovering agent needs it most."

## Reasoning

Walked all four design decisions against their corresponding issues: Decision 1 (InstructionsDelivered variant, option A) → Issue 1, matches, including CURRENT_SCHEMA_VERSION and unknown-variant compatibility. Decision 2 (extend `koto status`, no flag) → Issue 3, matches; no spurious flag introduced. Decision 3 (shared combinator `with_details_suppressed_unless_full`, option C) → Issue 2, matches signature and call-site placement. Decision 4 (reuse `with_directive_prefix`, option A, with D as required doc complement) → Issue 4 (splice) + Issue 5 (docs), matches the five named instructions-bearing variants exactly.

Security ruling (`handle_status` verifies template hash, reports mismatch via conditionally-present key rather than failing) is carried faithfully into Issue 3's last AC, word-for-word consistent with the design's "The ruling" paragraph.

Splice-ordering rule (recovery pointer spliced first, abandonment notice second, so the notice ends up closest to the front) is carried correctly and non-inverted in Issue 4.

Record-appended-after-print ordering and its stated reason (crash window fails toward re-delivery, not suppression) is carried correctly in Issue 2.

Self-transition/directed-transition contradiction: the PRD already shows the corrected acceptance criteria (both "directed transition + non-advancing tick omits them" and "two consecutive directed transitions into the same phase both carry them, because each begins a new occupancy" appear side by side, non-contradictory, consistent with the Definitions section's self-transition-begins-new-occupancy rule). Plan Issue 1 and Issue 2's ACs carry both scenarios faithfully and coherently — the prior correction holds up under re-verification, no re-introduced contradiction found.

Issue 5 covers all of R20–R25: koto-user docs (R20), koto-author docs (R21), cli-usage.md/Cursor rules (R22), evals (R23, plan's phrasing is a superset — "every skill" vs. "both skills" — not a gap), CHANGELOG (R24), and template-compile merge gate (R25).

No interface/method naming drift, no cross-issue mutually-exclusive behavior, no config/schema field-name conflicts found.
