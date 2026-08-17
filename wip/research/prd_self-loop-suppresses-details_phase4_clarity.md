# Verdict: PASS

Both blocking findings are resolved, and I confirmed each against the document
and the repository rather than against the change description. Two moderate
findings remain; neither admits two implementations of the rule, and each is a
one-clause fix.

## Re-review

| # | Previous finding | Status |
|---|---|---|
| 1 | BLOCKING — R1 admits two mechanisms (event shape vs. delivery record) | **Resolved** |
| 2 | BLOCKING — "touches only the two tests" forbids the required new tests | **Resolved** |
| 3 | MODERATE — "occupancy" in two senses; R15/R20 say the boundary moves | **Resolved** |
| 4 | MODERATE — "arrival" in two senses, propagated into R18's docs | **Partially resolved** — see finding 3 |
| 5 | MODERATE — "self-entry" undefined, rewind an unstated carve-out | **Resolved** |
| 6 | MODERATE — R11 drops the non-terminal qualifier, collides with R12 | **Resolved** |
| 7 | MODERATE — R7's "follows" ambiguous between immediate and eventual | **Resolved** |
| 8 | MODERATE — R18's predicate does not reach the text in a file it names | **Resolved** |
| 9 | MINOR — Decisions mis-describes what the BRIEF deferred | **Still open** — see finding 5 |
| 10 | MINOR — Out of Scope reproduces the BRIEF's boundary | **Resolved** |

Notes on the resolutions I checked hardest:

- **1.** R1 now reads "A response includes the current phase's instructions when no delivery of that phase's instructions has been recorded inside the phase's current delivery window" — keyed on the record, with "A self-entry does not open a new delivery window" as the change. The new Decisions entry names both candidate rules, names the case that separates them, and says which one R1 is. The discriminating criterion is present and correctly directed: with no record anywhere in the log, a self-entry delivers. This is a complete fix, not a patch over the symptom.
- **2.** The sentence is gone. The replacement — "The PR modifies no existing test in this file other than the two ...; added tests and added template constants are not modifications" — closes both readings, and the added-constant clause anticipates the same-tick-round-trip criterion's own note that `DELIVERY_TEMPLATE` cannot express that case. The two init tests now have their own criterion, tagged (R2).
- **3.** "Occupancy" survives in exactly one place, the Definitions preamble that explains why the word is being split. Nowhere else in the document. **Delivery window** and **Epoch** are separately defined, and R15 now pins the epoch by name rather than by the word that used to mean both.
- **8.** Better than the fix I asked for. The criteria enumerate ten surfaces by path, supply the `git grep` that decides R18, and add a criterion for the two files that claim the event is not emitted yet. I verified both are genuinely stale on merged `main`: `docs/reference/session-feed.md` ("**Not emitted yet.** ... instruction suppression is still keyed on visit count") and `src/engine/types.rs:781` ("**Nothing appends this yet.** ... Until that wiring exists, a session log will never contain one"). Commit `b7b0799` shipped the mechanism, so both are wrong today. The PRD found a defect I had not.

## Validator output

`shirabe validate docs/prds/PRD-self-loop-suppresses-details.md --check R7 --visibility public`
(`/home/dgazineu/.tsuku/tools/current/shirabe`, `shirabe v0.18.0`):

```
```

No output; exit code 0. Clean pass, as on the previous draft.

## Findings

### 1. MODERATE — R2 and R3 state an unconditional "carries" that R7 contradicts

R3:

> **R3.** A response whose phase's **delivery window** was opened by an entry from
> a different phase carries that phase's instructions.

R7:

> **R7.** A response that appends no state-entry event omits the instructions
> when the current delivery window already carries a recorded delivery.

A delivery window is a property that persists across many responses, not a
property of one. So take: arrive at `implement` from `gather` (response A carries
and records the delivery), then a gate-blocked tick (response B, appending no
state-entry event). B's delivery window was opened by an entry from a different
phase, so R3 says B carries. R7 says B omits. R1 says B omits. R3 is simply false
for every response in the window after the first.

R2 has the same shape: "A response for the initial phase of a freshly initialized
workflow carries that phase's instructions" — true of the first response, false of
the second non-advancing one.

The asymmetry is conspicuous because the author applied the fix everywhere else.
R4 carries "when the enclosing delivery window already carries a recorded
delivery"; R7 carries "when the current delivery window already carries a
recorded delivery"; R6 says "the response **after** a rewind", which reads as the
next one. Only R2 and R3 state their half unhedged. And the old R1's "subject to
R7" cross-reference is gone — correctly, since R1 is now the complete rule, but
that removal also removed the hedge a reader of R3 used to inherit.

No implementer is misled: R1 is unambiguous and complete, and the gate-blocked
test is on the must-not-move list. The cost falls on the downstream DESIGN author
who reads R3 as a standalone requirement.

**Fix.** "The **first** response after a delivery window is opened by an entry
from a different phase carries..." and the same in R2.

### 2. MODERATE — R18 forbids exactly the text R19 requires, and the grep that decides R18 will fire on it

R18:

> **R18.** After this lands, no committed file outside this PRD and its own BRIEF
> states that a self-transition re-delivers a phase's instructions, or defines the
> delivery boundary as closing at a self-entry.

R19:

> **R19.** The durable record says that the rule was reversed, by what, and why.
> **Deleting the passage that argued the other way is not sufficient**: the
> reversal is the fact a future reader needs.

Satisfying R19 means `docs/prds/PRD-inline-phase-details.md` and
`docs/designs/current/DESIGN-inline-phase-details.md` will each contain a sentence
saying what the old rule was — that a self-transition re-delivered. R18 has no
tense or attribution carve-out, and neither file is in the criterion's exclusion
list:

> - [ ] `git grep -nE 'self.transition' -- ':!wip' ':!docs/prds/PRD-self-loop-suppresses-details.md' ':!docs/briefs/BRIEF-self-loop-suppresses-details.md'`
>       returns no line **asserting** that a self-transition re-delivers
>       instructions or opens a delivery window. (R18)

"Asserting" is the word carrying the carve-out, and it is doing the right work —
a reviewer should judge attributed history as not an assertion. But this is the
one place in the document where two of its own requirements pull against each
other, on the requirement that distinguishes this PRD from a quiet correction,
and the criterion never says so. Every other criterion here goes out of its way to
pre-empt this kind of judgment call: the baseline criterion spells out the single
permitted fixture change and names what regenerating it would hide.

Two readings of the grep criterion: historical narration is exempt (intended, and
R19 is satisfiable), or it is not (and R18 and R19 cannot both be met).

**Fix.** R18: "...states, as the behavior that ships, that a self-transition
re-delivers..." and add to the criterion: "a line recording the reversed rule as
history under R19 is not such an assertion."

### 3. MINOR — "arrival" is still used in two senses, in R5 and in the Decisions section

R5, of a directed transition into the occupied phase:

> **This arrival** is reachable only for a template that declares the phase as its
> own transition target.

The Decisions section, of the same event:

> a rewind is a redo and a directed self-transition is **not an arrival at all**.

The operative half of my previous finding is fixed — the requirements now key on
**delivery window** and **self-entry**, both defined, so nothing turns on the
word. What remains is one noun in R5's second sentence contradicting the framing
the same document commits the shipped documentation to.

**Fix.** R5: "This **entry** is reachable only for..."

### 4. MINOR — the Definitions preamble miscounts itself

> **Three terms** carry weight below.

Four are defined: **State-entry event**, **Self-entry**, **Delivery window**,
**Epoch**.

### 5. MINOR — the Decisions entry still mis-describes what the BRIEF deferred (previous finding 9, unchanged)

The PRD, verbatim and identical to the previous draft:

> This is **the question** the upstream BRIEF deferred here.

The BRIEF:

> Which arrivals deliver **is settled here**; the downstream PRD owns the
> requirements that operationalize it, and the one **argument** this brief leaves
> open — why an explicitly targeted transition and a rewind land on opposite
> sides, which closes in the PRD's Decisions and Trade-offs section.

The brief deferred the argument and settled the answer. The PRD reaches the same
answer, so nothing downstream breaks; the sentence just claims more latitude over
its upstream than the upstream granted.

**Fix.** "This is the argument the upstream BRIEF left to the PRD."

### 6. MINOR — the delivery-window definition has an attachment ambiguity

> It opens at the most recent state-entry event naming the phase **that is not a
> self-entry**, and runs to the end of the log.

The nearer antecedent of "that is not a self-entry" is "the phase". A phase cannot
be a self-entry, so a reader recovers, but it is a garden path in the sentence
that carries the whole change. Separately, "naming the phase" is not pinned to the
entered phase — a state-entry event records two phases, and the State-entry event
definition one paragraph up says it "records the phase entered and ... the phase
left". The intended reading is available; it is not stated.

**Fix.** "It opens at the most recent state-entry event that is not a self-entry
and that records the phase as entered."

### 7. MINOR — five requirements carry no criterion tag while the rest do

Nearly every criterion ends with a parenthesized requirement number. R7, R10,
R14, R16 and R17 appear in none of them. R7 and R14 are covered in substance —
the gate-blocked case sits in the must-not-move criterion and R14 in the baseline
criterion — but both of those criteria are among the handful with no tag, so the
coverage is invisible to anyone auditing by tag. R10, R16 and R17 have no
criterion at all.

**Fix.** Tag the two untagged criteria that do the covering; decide whether R10,
R16 and R17 want criteria or are constraints the DESIGN inherits.

## Verified clean

Re-checked on this draft, plus everything the revision newly asserts. Recorded so
the next reviewer does not repeat it.

- Requirements are contiguous R1–R21, no gaps or duplicates. Every cross-reference in prose and in the criteria resolves.
- **The status-test claim is correct, and it is a sharp one.** `tests/status_phase_retrieval_test.rs:492` `status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected` submits `{"route":"go"}`, which under `PHASES_TEMPLATE` (`initial_state: gather`, `gather -> implement` on `route: go`) lands the workflow at `implement`; it then issues `--to implement`. That is a directed self-transition, `implement` declares `target: implement`, and the following assertion `first_implement.get("details").is_some()` is exactly what R5 inverts. The PRD is right that this is the third file whose existing assertions the change disturbs.
- **The pointer claim is correct.** `RECOVERY_POINTER` (`src/cli/next_types.rs:166`) is spliced "via `with_directive_prefix`" — the head of the directive, as the criterion says — and "gated on whether the *phase* declares instructions rather than on whether *this response* carries them", which is what makes R11 hold on a suppressed response. The same doc comment states "Terminal variants have no directive and are returned unchanged", corroborating R12's no-pointer carve-out from source as well as from the built binary.
- **The baseline-fixture limitation is exact.** `tests/next_response_baseline.rs:362` holds `description: "`implement` transitions to itself, ending one occupancy and beginning another."` and byte 84 of `tests/fixtures/next-response-baseline/instruction-free.json` holds the same prose. One sentence, two files, in lockstep — as Known Limitations describes and the criterion permits.
- **The visit-count Out of Scope entry is right.** `derive_visit_counts` has exactly one consumer outside `src/engine/persistence.rs`: `src/workflows_surface/project.rs:286`, the `/workflows` projection. That is also one of the two files R15's criterion pins as unchanged, and the two claims are consistent.
- **Every Gates claim matches CI.** `.github/workflows/validate.yml` runs `cargo test -- --test-threads=1` (40), `cargo test -p koto-stability-tests -- --test-threads=1` (62), `cargo fmt --check` (77), `cargo clippy -- -D warnings` with no `--all-targets` (92), and `cargo audit` (108). `check-artifacts` is draft-gated at line 13 (`if: ${{ github.event.pull_request.draft != true }}`), so the out-of-draft criterion is well founded. `validate-plugins.yml:39` compiles templates via `find plugins/koto-skills/skills/ -path '*/koto-templates/*.md'`, matching the criterion's wording.
- All newly cited paths exist: `src/workflows_surface/project.rs`, `src/cli/next_types.rs`, `src/engine/types.rs`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`, `plugins/koto-skills/skills/koto-author/SKILL.md`, `plugins/koto-skills/skills/koto-author/references/template-format.md`, alongside the ten from the previous review. `derive_evidence` is at `src/engine/persistence.rs:722`; `DELIVERY_TEMPLATE` at `tests/instructions_delivery_test.rs:81`.
- The two unit tests the criteria name as inverting are the right two, and the rewind case is correctly excluded: `instructions_delivered_resets_on_arrival_by_rewind` uses `rewound(5, "implement", "gather")`, not a self-rewind, so it belongs with the cases that do not move.
- R16's rewrite fixed a real error I had not flagged. The previous draft claimed an old log yields "the same delivery answers"; it now says the answers are "the answers this PRD specifies, not the answers the old binary gave", with a matching Known Limitations entry for the in-flight upgrade. That is correct and was wrong before.
- No workspace banned words. 30 em dashes across 519 lines, the same ratio as the previous draft, and the validator's density check passes. Zero contractions, consistent with `PRD-inline-phase-details.md` and the upstream BRIEF, which also have zero — house register, not a tell.
- No `wip/` path is referenced from the PRD.

## Summary

PASS. Both blocking findings are properly resolved rather than papered over: R1 is now keyed on the recorded delivery with a Decisions entry naming the rejected alternative and a criterion that discriminates them, and the regression criterion's contradiction is gone along with the two init tests it used to omit. Six of the eight moderate and minor findings are closed, most of them by structural fixes — a Definitions section that splits delivery window from epoch, which retires "occupancy" from the requirements entirely.

Two moderate findings remain, both wording. R2 and R3 state an unconditional "carries" that R7 and R1 contradict for every response in a window after the first, which misleads a reader rather than an implementer. And R18's prohibition has no carve-out for the historical record R19 demands, so the `git grep` that decides R18 will surface exactly the sentences R19 requires; the word "asserting" carries the exemption but the criterion never says so.

Everything checkable held, including several claims that would have been easy to get wrong. The status test really does issue `--to implement` while already at `implement`; the pointer really is prefixed to the directive and gated on the phase rather than the response; the baseline fixture really does embed its generating test's prose; the visit-count helper really has exactly one consumer left. The revision also caught a stale doc comment and an incorrect compatibility claim in its own previous draft that I had missed.
