# Reviewer: architecture

## Verdict
PASS

Both required changes from the prior review are correctly and precisely resolved, and no new issues surfaced on the re-read.

## Requirement coverage

| Req | Status | Evidence |
|---|---|---|
| R1 | Addressed | Predicate replaces visit-count check; keyed on delivery within occupancy. |
| R2 | Addressed | Combinator suppresses `details` when `already_delivered && !full`. |
| R3 | Addressed | Every arrival path (conditional/unconditional `Transitioned`, `DirectedTransition`, `Rewound`, init's `Transitioned{from:None}`) appends an entry event; predicate slices after the most recent one. Verified in `src/engine/advance.rs:503-509,545-551`, `src/cli/mod.rs:3336-3340`, rewind's append, `src/cli/init_child.rs:497-502`. |
| R4 | Addressed | One combinator, two call sites (`mod.rs:3357`, `mod.rs:4198`), confirmed identical in source. |
| R5 | Addressed | `full` baked into combinator signature. |
| R6 | Addressed | `details.is_empty()` guard gates both the new read and the new write; the read is already gated this way today (`mod.rs:3999-4004`). |
| R7 | Addressed | `koto status <name>`, existing signature, no new argument. |
| R8/R9 | Addressed | `directive`/`details`/`expects` sourced from the same `TemplateState` and substitution pipeline `next` uses; `derive_expects` confirmed pure, no I/O. |
| R10 | Addressed | Retrieval doesn't call the combinator or append anything. |
| R11 | Addressed | Verified `handle_status` never calls `lock_state_file`, `append_event`, gate evaluation, or terminal cleanup. |
| R12 | Addressed | `lock_state_file` has exactly one call site in `src/cli/mod.rs`, gated to batch-scoped states inside `handle_next`. |
| R13 | Addressed | Terminal/no-instructions cases return normal success with fields omitted. |
| R14 | Addressed | Pointer covers exactly the five instructions-bearing variants by construction. The interaction with the abandonment notice — the one open item from the prior review — is now resolved by the new "Splice ordering when both notices apply" subsection. |
| R15 | Addressed | The new subsection makes the non-displacement guarantee concrete under the one case where it was previously ambiguous: when both koto-authored splices apply, the abandonment notice (the more urgent one) ends up closest to the front, and neither splice touches the phase's own directive text, which both splices already preserved as a suffix. |
| R16/R17 | Addressed | `CURRENT_SCHEMA_VERSION` doc comment and the `adding_the_request_family_does_not_move_the_schema_version` precedent test confirm additive variants don't bump it. |
| R18 | Addressed | Natural path reuses the already-gated read; directed path builds the event list in memory from the just-appended payload. |
| R19 | Deferred to implementation | Design names the unit/integration test surface at the altitude expected of a design doc. |
| R20-R25 | Deferred to implementation (Phase 4) | Named but not detailed; standard for this altitude. |

## Strawman check

Unchanged from the prior pass — the new subsection doesn't touch the Considered Options for any of the four decisions, and I re-read all four sections to confirm. Findings stand:

- **D1 Option D** ("disproved, not merely disfavoured"): independently verified against `src/engine/advance.rs` in the prior pass — `TransitionResolution::NeedsEvidence`'s `accepts.is_some()` arm appends no event, so two sessions differing only in delivery status are genuinely byte-identical in the log. This is a proof, not an assertion.
- **D1 Option C** (additive `StateFileHeader` field): independently verified `StateFileHeader` derives no `Default`, isn't `#[non_exhaustive]`, and `koto-stability-tests` constructs it with exhaustive struct literals — a genuine compile-time fact, not a strawman.
- **D1 Option B**, **D2 Options B/C**, **D3 Options A/B/D**, **D4 Options B/C/D**: each rejection rests on a checkable code fact (verified: the batch lock at `mod.rs:3746-3776` is acquired unconditionally before any dispatch logic runs, ruling out D2's Option C on a hard constraint; `dispatch_next`'s ~20 positional-call unit tests make D3's Options A/B a real signature-change cost; `expects` is documented always-null on `GateBlocked`, ruling out D4's Option C structurally) or a stated PRD requirement (D3's Option D collides with the Out-of-Scope exclusion on directed-path gate evaluation). D4's Option D (skill-only, no pointer) continues to get the most genuine hearing of any rejected option in the set before being folded in as a required complement rather than dismissed.

No strawmen in either pass.

## Findings

**1. Splice-ordering gap — resolved.** The new "Splice ordering when both notices apply" subsection (after "The one thing the pointer must not key on") states: the recovery pointer is spliced first and the abandonment notice second, so the abandonment notice ends up closest to the front of `directive`. This is mechanically correct given `with_directive_prefix`'s `format!("{prefix}{directive}")` prepend semantics — the second call's prefix ends up outermost — and it adopts the same resolution the upstream research had already converged on (never bury the stop instruction under routine navigational text). The stated reason (the abandonment notice demands action before anything else; the pointer is routine lookup information) is sound and consistent with the abandonment notice's own existing doc comment ("retained for context only"). This closes the gap cleanly; nothing further to invent at implementation time on this axis.

**2. Phase 1/2 justification — resolved.** The Implementation Approach's closing paragraph now states the coupling correctly: phase 1 is inert by construction (adds a variant and a predicate nothing calls yet) and can land alone; the real coupling is between phase 2's natural-path and directed-path halves, which must land together or the two paths disagree. This matches the actual behavior of the phased plan and is no longer misattributed to the phase 1/phase 2 boundary.

**3. No regressions from the edit.** The rest of the document is unchanged from the version I reviewed previously, which I had already verified line-by-line against source for the mechanism (entry-event uniformity, `derive_visit_counts`'s untouched second consumer, the two existing combinators' exact five-variant coverage, `handle_status`'s structural non-effects, the print-then-append precedent via `finish_terminal_tick`, and the template-hash-verification finding and ruling). Re-reading the full document start to finish, I found no other change beyond the new subsection, the rewritten closing paragraph, and a small wip-hygiene edit to the Considered Options preamble (removing a reference to reports that "do not survive the branch," consistent with this workspace's wip-hygiene rule against committed references to `wip/`-staged artifacts).

**4. Layering and phasing.** Both still check out as in the prior pass: the predicate sits in `src/engine/persistence.rs` beside `latest_epoch_gate_failed` (same epoch-slicing idiom, same layer), the combinator sits in `src/cli/next_types.rs` beside its two siblings, and the call sites sit in `src/cli/mod.rs` where both existing combinator calls already live. No inversion.

**5. Writing style.** No instances of "tier/tiered," "robust," "leverage," "comprehensive/holistic," "facilitate," or preamble phrasing found on a fresh grep of the full file. No emojis.

## Required changes

None.
