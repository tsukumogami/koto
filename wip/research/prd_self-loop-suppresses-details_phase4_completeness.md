# Verdict: PASS

Third read of `docs/prds/PRD-self-loop-suppresses-details.md`, from disk, in
full. Every changed claim was re-verified against the tree at HEAD `e79531f`
(read-only; no build, no koto session, no writing git command).
`shirabe validate` exits 0; sections present and in canonical order.

Both remaining round-two findings are resolved, and resolved correctly rather
than nominally — I checked the instruments the new criteria depend on and each
one exists and does what the criterion says. Every requirement R1 through R21
now has at least one criterion, every arrival class the Phase 2 research
enumerates has both, and no criterion in the document is unsatisfiable, vacuous,
or in conflict with another. Three MINOR items remain; none blocks acceptance
and none needs another round.

## Re-review

Rounds one and two, in order. "1" / "2" in the round column are review rounds,
not requirement numbers.

| Round | Finding | Status | Note |
|---|---|---|---|
| 1 | BLOCKING — R11 (retrieval pointer) has no criterion | **Resolved** (r2) | R11 qualified to non-terminal instruction-declaring phases; two criteria added. Behavior re-verified at `src/cli/next_types.rs:155-166` and the two splice sites `src/cli/mod.rs:3429`, `:4313`. |
| 1 | BLOCKING — R16's compatibility sentence contradicts R4 | **Resolved** (r2) | Antecedent named; the decisive sentence added ("not the answers the old binary gave"); in-flight case carried in Known Limitations. |
| 1 | BLOCKING — the no-diff criterion is unsatisfiable | **Resolved** (r2, strengthened r3) | "Added tests and added template constants are not modifications", now with a further clause governing edits to shared template constants. |
| 1 | BLOCKING — self-rewind has only a synthetic-log criterion | **Resolved** (r2) | End-to-end criterion added with the right justification; named in the measured-binary list. |
| 1 | MODERATE — R2 has no criterion | **Resolved** (r2) | Both init tests named. |
| 1 | MODERATE — R2/R11/R12/R13/R16/R17 orphan requirements | **Resolved** (r2 + r3) | R11, R12, R13 in round two; R16 and R17 gained Gates criteria in round three. No requirement is now uncriterioned. |
| 1 | MODERATE — criteria pre-commit the design | **Resolved** (r2) | "Criteria name behavior, not function names: which function decides the rule is the DESIGN's call." |
| 1 | MODERATE — no-prior-delivery self-entry undiscriminated | **Resolved** (r2) | R1 rekeyed onto the recorded delivery, with a dedicated unit criterion and a Decisions entry. |
| 1 | MINOR — R17's second sentence is DESIGN altitude | **Resolved** (r2) | Cut. |
| 1 | MINOR — one Out of Scope exclusion unexplained | **Partially resolved** (r2) | The visit-count bullet gained its reason; the auto-advanced-phase bullet is still a bare noun phrase, licensed by the section's opening deferral to the BRIEF. Not worth another round. |
| 1 | MINOR — renaming the inverted assertions required only obliquely | **Resolved** (r2) | Both named, with a `git diff -U0` check. |
| 1 | MINOR — orphan criteria | **Partially resolved** | Most criteria now carry an (R…) tag. Still untagged: the two "what must not have moved" test criteria, the baseline criterion (the only check for R14), the first Surfaces criterion, and three Gates criteria. R7, R10 and R14 are each decided by an untagged criterion. Cosmetic — see finding 2. |
| 1 | MINOR — untrusted `Transitioned.from` unsurfaced | **Resolved by construction** (r2) | R1 no longer compares source to target, so the question does not arise. |
| 2 | **BLOCKING — R15's criterion cannot detect R15's violation** | **Resolved** | See the verified-blockers block below. |
| 2 | MODERATE — the R13 criterion is vacuous on the available harness | **Resolved** | See the verified-blockers block below. |
| 2 | MODERATE — baseline and Surfaces criteria can conflict over the fixture | **Resolved** | The fixture criterion now reads "The fixture's `notes` and `description` strings may change, in lockstep with the matching strings in `tests/next_response_baseline.rs`". Verified `notes` is a real top-level key: `json.load` on `tests/fixtures/next-response-baseline/instruction-free.json` returns keys `['notes', 'sequences']`, and the two note strings at `:7` and `:10` mirror `tests/next_response_baseline.rs:266` and `:269`. The one remaining unpermitted string is the sequence *label* `self-transition-arrival` (fixture `:85`), which asserts nothing about the rule and so is not caught by R18 — no conflict left. |
| 2 | MINOR — delivery window undefined with no non-self-entry | **Resolved** | Definitions: "When no such event exists, the window is the whole log." Matches the shipped fallback at `src/engine/persistence.rs:1044`. |
| 2 | MINOR — R16 and R17 have no criterion | **Resolved** | Both gained Gates criteria; R16's cross-version half correctly demoted to a Known Limitation that says plainly it is not gated. |
| 2 | MINOR — R18's exclusion list omits the documents R19 requires to carry the old rule | **Still open** | See finding 1. |

## Findings

### Resolved blockers, verified

**Round two's BLOCKING finding is closed correctly.** The new criterion is the
discriminating case:

> Unit: one synthetic log — entry into a phase from elsewhere, a recorded
> delivery for it, a failed gate evaluation, then a transition from that phase to
> itself and nothing after — yields two different answers. The gate-blocked
> classification reports not blocked … The delivery decision reports already
> delivered … One log, two boundaries, opposite answers.

I traced it against the code rather than accepting it. For the log
`[Transitioned{from:Q,to:P}, InstructionsDelivered{P}, GateEvaluated{P,failed}, Transitioned{from:P,to:P}]`:
`latest_epoch_gate_failed` (`src/engine/persistence.rs:1058-1069`) opens its
epoch at the last entry event naming P — the self-entry — so its slice is empty,
no `GateEvaluated` is found, and `.unwrap_or(false)` yields `false`. The delivery
window opens at the first event and holds the record, so the delivery decision
yields already-delivered. If an implementation widens `occupancy_slice`
(`:1028`) in place, the gate slice reopens at the first event, finds the failed
gate, and returns `true` — the criterion fails, which is exactly what round two
said no check in the document could do. The three diff commands survive as
"Supplementary, not sufficient on their own", with the reason stated, and the
`derive_evidence` clause is correctly justified ("re-implements the evidence
epoch inline rather than sharing it" — confirmed at `:729-741`, it walks the
event list itself).

**Round two's MODERATE on R13 is closed, and the replacement instrument is
real.** The criterion now records a decision instead of a gate override, and
states my reason inside itself. `koto decisions record` exists —
`DecisionsSubcommand::Record`, `src/cli/mod.rs:647-653`, "Record a structured
decision without advancing state", appending `EventPayload::DecisionRecorded` at
`:4866` under a function documented at `:4666` as appending it "without running"
anything else. On `DELIVERY_TEMPLATE` the sequence is expressible with no new
template: arrive at `implement` from `gather` (delivers, records), plain
`koto next` → evidence-required and omits, record a decision, tick again → must
still omit and still carry the pointer. Nothing terminal intervenes, so the
assertion has something to bite on.

**Three further round-three claims check out.** R12's "pinned by the three
existing unit tests in `src/cli/next_types.rs`" names exactly three tests that
exist and cover exactly the three behaviors claimed:
`recovery_pointer_prefix_leaves_terminal_and_error_unchanged` (`:993`,
pointer-prefix), `suppress_terminal_and_error_pass_through_unchanged` (`:1409`,
suppression), `carries_details_false_for_terminal_and_error` (`:1453`,
carries-details). The corrected `--full` justification is right: inside a
self-entry window the arrival that opened the window has already recorded, so the
override's own record cannot be load-bearing there, and the recording clause is
genuinely checked only by
`override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`.
And the R16 Gates criterion is correctly scoped to variants, fields and the
schema constant rather than to a no-diff on `src/engine/types.rs` — which
matters, because the Surfaces criterion requires a doc-comment edit in that same
file (`:781-785`, the stale "**Nothing appends this yet.**"). A no-diff criterion
there would have re-introduced a round-two-style conflict; this one does not.

### 1. MINOR (carried) — R18's exclusion list still omits the two documents R19 requires to carry the old rule as history

R18 exempts "this PRD and its own BRIEF". R19 requires
`docs/prds/PRD-inline-phase-details.md` and
`docs/designs/current/DESIGN-inline-phase-details.md` to "record that it was
reversed, by what, and why" — which cannot be written without stating the
reversed definition. The grep criterion draws the line at "returns no line
**asserting** that a self-transition re-delivers instructions or opens a delivery
window", and assert-versus-record is the right distinction, but it is a
judgement inside a criterion whose form is a shell command.

Round three widened the pattern to `'self.transition|occupancy'`, which makes
this slightly more visible rather than less: `occupancy` matches the predicate's
own name (`instructions_delivered_this_occupancy`,
`src/engine/persistence.rs:1099`, called at `src/cli/mod.rs:3417` and `:4298`,
cross-referenced at `src/cli/next_types.rs:378`) and several test names, all of
which are identifiers rather than statements. The reviewer will be hand-waving
past a dozen hits either way.

**Fix, one clause:** "a line that records a former rule as former is not an
assertion of it; identifier and test-name hits are not statements."

### 2. MINOR (carried) — three requirements are decided only by untagged criteria

Round three tagged most criteria with the requirement they decide, which is a
real improvement. Still untagged: both "what must not have moved" test criteria,
the baseline criterion, the first Surfaces criterion, and three of the Gates
criteria. The consequence is small but specific — R7, R10 and R14 are each
decided by a criterion that does not name them, so a reader working backwards
from a requirement to its check has to reconstruct the mapping for those three.
R14 is the one worth tagging: the baseline criterion is its only check.

### 3. MINOR (new) — the R17 criterion mis-describes the natural-advancement site

> The two response-construction sites keep their current read behavior: the
> directed path still builds its event list in memory and the natural path still
> **reuses the tick's own re-read**. Checkable as a one-line diff review at each
> site. (R17)

The directed half is right (`src/cli/mod.rs:3403-3418` assembles a synthetic
post-append list in memory). The natural half is not: `src/cli/mod.rs:4291-4298`
issues its own `backend.read_events(&name)` inside the instruction-declaring
branch, and the comment immediately above it says so in as many words — "The
events are re-read here (**rather than reusing an earlier read**) because the
advancement loop above may have appended transitions and gate evaluations since
the last read." A reviewer applying the criterion literally will look for a reuse
that does not exist.

The operative half of the criterion still works, and R17's substance — no *new*
read or write — is unaffected: that read is guarded by
`final_template_state.details.is_empty()`, which is what preserves the
instruction-free byte-identity guarantee R14 pins.

**Fix:** "the natural path still takes its single `read_events` inside the
instruction-declaring branch, and the directed path still builds its list in
memory; neither site gains a read or a write."

Two smaller notes on the same criterion, neither worth a round: it sits under
Gates alongside CI commands but is a manual diff review, and it is the one
criterion in the document that names implementation internals — defensible, since
a no-new-IO requirement can only be checked at the sites that do the IO.

## Coverage tables

### Arrival class → requirement → criterion

| # | Arrival class (arrival-paths §Implications) | Requirement | Criterion | Status |
|---|---|---|---|---|
| 1 | Initial entry — file, inline, batch child spawn | R2 | Initial-entry criterion (both tests named) | ok |
| 2 | Conditional / unconditional / `skip_if`, `from != to` | R3 | Rule #1; must-not-move #1 | ok |
| 2d | Directed transition to a different phase | R3 | Rule #3; measured-binary | ok |
| 2e | Loop-back into a previously occupied phase | R3 | must-not-move #1; measured-binary | ok |
| 2f | Same-tick round trip `P → Q → P` | R3 | Rule #4 (added template constant) | ok |
| 3 | Advance-loop self-entry | R1, R4 | Rule #1, #2, #5; measured-binary | ok |
| 4 | Directed self-entry | R1, R5 | Rule #3, #5; status-test #1; measured-binary | ok |
| 5 | Rewind, `from != to` | R6 | must-not-move #1; measured-binary | ok |
| 5b | Rewind, `from == to` | R6 | Rule #7 (unit) **and** #8 (end-to-end); measured-binary | ok |
| 5c | Batch retry rewind on a child | R6 (generic) | — | thin, acceptable |
| — | Self-entry with no delivery record anywhere | R1 | Rule #6 | ok |
| — | Non-advancing tick | R7 | must-not-move #1 (untagged) | ok |
| — | Gate override / decision / evidence / scheduler / respawn / wake | R13 | non-entry-events criterion | ok |
| — | Terminal phase | R12 | pointer/terminal #2 (by construction, three tests cited) | ok |
| — | `--full`, any class | R8 | override #1, #2 | ok |
| — | `koto status`, any class | R9 | override #3, #4 | ok |
| — | Recovery pointer on a suppressing response | R11 | pointer/terminal #1 | ok |
| — | No non-self-entry names the phase | R1 (Definitions fallback) | must-not-move #2, incidentally | ok |
| — | Legacy log, in-flight session on upgrade | R4, R16 | Known Limitations (stated, not gated) | ok |
| — | Gate-blocked epoch across a self-entry | R15 | must-not-move #4 (the discriminating unit log) | ok |
| — | Evidence epoch across a self-entry | R15 | must-not-move #5 (`derive_evidence` diff) | ok |

No arrival class lacks a requirement. No requirement invents a class that does
not exist. Every class now has a criterion.

### Requirement → criterion

None orphaned. R7, R10 and R14 are decided by untagged criteria (finding 2);
R10's is distributed across Rule #1, Rule #3 and the status-test criterion rather
than held by one.

### Criterion → requirement

The plugin template-compilation gate, the docs-workflow gate, the `wip/` gate and
the out-of-draft gate trace to no requirement. Conventional process gates; leave
them.

## Summary

Both round-two findings are resolved, and resolved at the level they were raised:
R15 now turns on a single synthetic log that yields opposite answers from the two
boundaries, which I traced through `latest_epoch_gate_failed` and
`occupancy_slice` and confirmed would fail against an in-place widening of the
shared helper — the exact violation no previous check could see. The R13
criterion swaps the gate override for `koto decisions record`, a command that
exists and appends a non-state-entry event without advancing, so the criterion
now has something to assert against instead of passing vacuously on a terminal
body.

The document is complete. Every requirement has a criterion, every criterion has
a requirement or is a recognized process gate, every arrival class the Phase 2
research enumerates is covered on both sides, and the round-two conflict between
the baseline and Surfaces criteria is gone now that the fixture criterion permits
`notes` as well as `description` — verified as a real top-level key rather than
taken on the word of the criterion. The changes made for the parallel testability
review did not disturb any coverage this review tracks.

PASS. Three MINOR items remain and none warrants another round: R18's grep still
leaves assert-versus-record to judgement while R19 requires two documents to
carry the old rule as history; R7, R10 and R14 are decided by untagged criteria;
and the R17 criterion says the natural path "reuses the tick's own re-read" when
that site issues its own `read_events` and its comment says explicitly that it
does not reuse an earlier one. All three are one-clause edits, safely folded into
whatever touches the document next.
