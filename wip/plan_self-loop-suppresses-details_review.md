# Verdict: FAIL

Round 2. `docs/plans/PLAN-self-loop-suppresses-details.md` re-read from disk at
203 lines → 299 lines, four outlines restructured. Every claim below re-verified
against the working tree; the round-1 findings are dispositioned in the table,
and two new Category D findings are raised against the revision itself.

One blocking finding remains — a new one, introduced by the round-1 fixes.

## Re-review

| # | Round-1 finding | Status |
|---|---|---|
| A1 | Status-retrieval-after-suppression criterion reachable through no outline | **Resolved.** Issue 3 AC6 carries the clause verbatim; `tests/status_phase_retrieval_test.rs` is in Issue 3's `Files`. |
| A2 | R15 supplementary criterion had no home | **Resolved.** Issue 1 AC8 names all four walks and both consumer files, with the reason. |
| A3 | R16 schema gate had no AC | **Resolved.** Folded into Issue 2 AC4 on the file it bounds. |
| A4 | R18's catch-all grep in no outline | **Resolved** as a criterion, but see **D7** — it now sits in the wrong outline. |
| A5 | Output-contract cross-reference had no PRD or BRIEF backing | **Resolved.** Scope Summary names it record-keeping; Issue 4 AC5 repeats why it falls outside R18 and inside R19. |
| B1 | Issues 3 and 4 gave contradictory instructions for the delivery test | **Resolved.** The narrating comments moved to Issue 3 (AC5), which names the same five tests I found; AC4 says comment rewrites are not assertion changes. |
| B2 | "The two unit tests are inverted" — only half of each inverts | **Resolved.** Issue 1 AC5 names the flipping assertion in each and states the other two are already true. Re-verified at `src/engine/persistence.rs:2603`, `:2608`, `:2644`, `:2648` — the plan is now correct on all four. |
| B3 | "Assertions unmodified" unsatisfiable under the chosen rename | **Resolved.** Restated as the asserted boolean, with the twelve-line warning. I counted the call sites in `assert` lines: `:2549, :2566, :2579, :2591, :2603, :2608, :2623, :2631, :2644, :2648, :2656, :2657` — twelve. The plan's number is right. |
| B4 | Issue 4 narrowed R20 to `expected_output`, leaving four occupancy assertions committed | **Resolved.** AC3 now permits the eval names and the assertion strings to be reworded, and holds the line at "no assertion is removed". |
| C1 | "The full behavior set" named nothing | **Resolved.** Thirteen cases enumerated, matching the PRD's list item for item. |
| C2 | Non-entry-event case dropped both discriminating details | **Resolved.** Issue 3 AC2 names the decision record, rules out the gate override with the reason, and keeps the pointer assertion. |
| C3 | Fixture criterion weaker than the PRD's, permitted regeneration | **Resolved.** AC7 carries "nothing in the fixture changes except the one `description` string" and forbids `regenerate_baseline_fixture` by name. |
| C4 | Nine surfaces collapsed into five nouns | **Resolved.** Five plugin paths named individually, with the four-passages-plus-embedded-example detail on response-shapes.md. |
| C5 | CHANGELOG AC unfalsifiable | **Partially resolved.** "Naming the behavior change and the renamed function" — the second half is checkable, the first is not. Minor; no action needed. |
| C6 | Renamed function unnamed in the plan | **Resolved.** `instructions_delivered_this_window` pinned in Issue 1 AC4 and used in Issue 2 AC1. |
| C7 | `--full` case dropped its second half | **Resolved.** "followed by a non-advancing tick that omits". |
| D1 | Issue 4's `Files` omitted seven files | **Resolved.** All twelve listed; I checked each against the ACs and against the tree. |
| D2 | `## Dependency Graph` empty | **Resolved.** See the FC14 note below — the call is right, the stated reason is half right. |
| D3 | Chain claimed to be a property of the change, contradicted by the plan's own text | **Resolved.** Issue 4 is `Dependencies: None`; the Implementation Sequence distinguishes landing order from authoring order and names the one file that would have collided. |
| D4 | No complexity classifications | **Resolved.** critical / testable / testable / simple. |
| D5 | CI gates lived only in plan prose; PR-out-of-draft nowhere | **Partially resolved.** All gates now appear in the Implementation Sequence, out-of-draft included. Two nits, neither actionable on its own: the PRD's second staging-directory clause ("no file this change adds or modifies names a path inside it") is still dropped, and the sentence conflates the two conditions — out-of-draft is what makes the artifact check *run*, an empty directory is what makes it *pass*. |
| — | **D6** (new) | **Open — BLOCKING.** |
| — | **D7** (new) | **Open — MODERATE.** |
| — | **C8** (carried, was a note) | **Open — MINOR.** Baseline `notes` tension. |
| — | **A6** (carried) | **Open — MINOR.** R12's pinning tests still have no AC. |

## Findings

### D6. BLOCKING — Issue 2's `cargo test` criterion cannot pass until Issue 3, which is blocked by Issue 2.

Issue 2:

> - [ ] `cargo test -- --test-threads=1` passes.
>
> **Dependencies**: Blocked by <<ISSUE:1>>
> **Files**: `src/cli/mod.rs`, `src/cli/next_types.rs`, `src/engine/types.rs`

At the end of Issue 2 the behavior is live on both construction sites and three
integration tests still assert the old rule. All three are fixed in Issue 3, which
declares `Blocked by <<ISSUE:2>>`. Verified:

- `tests/instructions_delivery_test.rs:359` `self_transition_arrival_carries_details_again`
  — ticks `{"loop_again":"yes"}` at `implement` and calls
  `assert_carries(&resp, "Implement instructions.", "self-transition arrival")`
  (`:380`). Fails the moment the delivery check reads `delivery_window`. Fixed by
  Issue 3 AC4.
- `tests/instructions_delivery_test.rs:487` `two_consecutive_directed_transitions_into_same_phase_both_carry`
  — same shape. Fixed by Issue 3 AC4.
- `tests/status_phase_retrieval_test.rs:492` `status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected`
  — its setup ticks `{"route":"go"}` to reach `implement` and then issues
  `--to implement` while already there (`:498-499`), asserting
  `first_implement.get("details").is_some()` (`:500-503`). Fails for the same
  reason. Fixed by Issue 3 AC6.

This is a cycle between the criterion and the graph: Issue 2's AC needs Issue 3's
output, and Issue 3 is blocked by Issue 2. A consumer driving the outlines in order
reaches Issue 2's last criterion, runs the command, gets three red tests, and has to
decide on its own whether to reach into a file Issue 3 owns — which is precisely the
class of ambiguity the B1 fix removed from the other direction.

Note the crate also does not compile at the end of Issue 1: Issue 1 renames the
function while `src/cli/mod.rs:2914` still imports `instructions_delivered_this_occupancy`
and `:3417` and `:4298` still call it. So `cargo test` is first *runnable* at the end
of Issue 2 and first *green* at the end of Issue 3. There is no point in the chain
where Issue 2's criterion is true.

Fix: move the full-suite run to Issue 3, or drop it — the Implementation Sequence's
verification gate already names `cargo test -- --test-threads=1` for the plan as a
whole. If Issue 2 wants a checkpoint of its own, `cargo build` plus
`cargo clippy -- -D warnings` is satisfiable there and says something real (clippy is
invoked without `--all-targets` per the PRD's gate, so it will not see the red tests).

### D7. MODERATE — Issue 4's grep backstop depends on all three code outlines, and Issue 4 declares no dependency.

Issue 4:

> - [ ] `git grep -nE 'self.transition|occupancy'`, excluding the staging directory
>       and this chain's own PRD and BRIEF, returns no line asserting that a
>       self-transition re-delivers instructions or opens a delivery window. This
>       is the backstop for the enumeration above: it is what catches the surface
>       nobody listed.
>
> **Dependencies**: None

The grep is repo-wide by construction — that is what makes it a backstop. Its
result therefore depends on comment rewrites owned by every other outline. The lines
it will hit that Issue 4 cannot fix:

- `src/engine/persistence.rs:1000-1004` — "So a self-transition ends one occupancy
  and begins another, and so does a rewind into the phase". Outright assertion of the
  moved boundary; Issue 1 AC9 owns it.
- `src/engine/persistence.rs:1017-1019` — "cannot come to disagree about where an
  occupancy starts". Issue 1 AC9.
- `src/engine/persistence.rs:1082` — "The occupancy is [`occupancy_slice`]'s, so
  self-transition, rewind, and arrival at the initial state all fall out of one
  definition". Issue 1 AC9.
- `src/cli/mod.rs:3377-3402` — the "provably false" proof. Issue 2 AC2.
- `src/cli/next_types.rs:378` and its surrounding doc comment. Issue 2 AC3.
- `tests/instructions_delivery_test.rs:505-507` — "a second directed transition into
  it is valid and is a fresh occupancy", which asserts the boundary directly. Issue 3
  AC5.

Same class as D6, opposite direction: an outline's AC references behavior other
outlines are responsible for, with no dependency edge. It is softer than D6 because
the grep is idempotent and cheap to re-run — but as written, an agent completing
outline 4 first cannot mark it done.

Fix: move the repo-wide grep into the Implementation Sequence's verification gate,
where the other cross-cutting checks already live. That keeps Issue 4 genuinely
dependency-free — which is the point of the D3 fix — and puts the backstop at the
only moment it can actually pass. Leave the enumeration ACs in Issue 4.

### C8. MINOR — the baseline `notes` are hard-frozen while the grep may demand they move.

Issue 3 AC7: "nothing in the fixture changes except the one `description` string
stating the old boundary". The PRD's Surfaces criterion is broader — it lists
"`tests/next_response_baseline.rs`'s **notes** and sequence descriptions".

Verified: `tests/next_response_baseline.rs:266` and `:269` are notes that name a
self-transition among "arrivals" ("the conditional and unconditional and
self-transition arrivals"). Neither states a delivery rule, so on my reading both are
exempt — the same reasoning the DESIGN uses to rule the `self-transition-arrival`
label out of scope. But Issue 3 now *forbids* changing them while Issue 4's grep asks
a reader to judge whether they assert the old boundary, and the plan records no answer.

Fix: one clause in Issue 3 AC7 saying the notes stay because they name call sequences
rather than delivery behavior, matching the DESIGN's argument for the label.

### A6. MINOR — R12's pinning tests still have no AC.

PRD: "It is pinned by the three existing unit tests in `src/cli/next_types.rs`
covering pointer-prefix, suppression and carries-details against the terminal variant,
all of which pass with their assertions unmodified. No new test is required."

Issue 2 edits `src/cli/next_types.rs` (AC3, doc comment) and nothing in the plan
names those three tests. They are at `src/cli/next_types.rs:1367`, `:1377`, `:1389`
and their neighbours in the `with_details_suppressed_unless_full / carries_details`
block (`:1346`). Covered only by the plan-level suite run. Low stakes — a doc-comment
edit cannot break them — but it is the one PRD criterion with no outline at all.

## On the FC14 call

Keep the diagram. I ran the validator read-only and reproduced the notice exactly:

```
::notice file=docs/plans/PLAN-self-loop-suppresses-details.md::[FC14] execution_mode is 'single-pr' but '## Dependency Graph' is populated -- switch the frontmatter to 'multi-pr' or remove the diagram body
```

Exit code 0. The plan's claim that it does not gate CI is correct — `BRIEF-single-pr-plan-validation.md:176` records promotion to error severity as separate future work.

But the plan's stated reason is half right, and worth correcting because it will be
read as "the validator is wrong". This is two shirabe reference files contradicting
each other, not a validator bug:

- `skills/plan/references/phases/phase-7-creation.md:358` lists Dependency Graph as
  required section 5 for single-pr, "same Mermaid format as multi-pr, but nodes use
  internal IDs". That is the contract the plan cites, and it cites it accurately.
- `skills/plan/references/plan-format.md:207-219` says the opposite: outline-shaped
  plans need "neither the table nor the graph", and "the diagram is **barred from
  `single-pr`**".

Given the conflict, keeping the diagram is the better side: four nodes and three
edges is the only place a reader sees the 1→2→3 chain and outline 4's independence at
a glance, and that independence is the whole point of the D3 fix. Reword the closing
paragraph to say the two references disagree and which one the plan follows, rather
than implying the check is spurious. Worth an issue against shirabe to reconcile them.

One cosmetic nit while you are in there: `plan-format.md` asks node labels to match
the work item's link text. The diagram's labels are shortened
(`"1: split the delivery boundary"` against the outline heading
`Issue 1: split the delivery boundary from the gate epoch`). Harmless, but free to fix.

## PRD criterion coverage

Round 2. Changes from round 1 marked.

| # | PRD criterion (abbreviated) | Covered by |
|---|---|---|
| 1 | Arrival from `gather` carries; next self-transition tick omits (R4) | Issue 3 AC4 |
| 2 | Two successive self-transition ticks omit, both asserted (R4) | Issue 3 AC1 |
| 3 | Directed into `implement` carries; second directed while there omits (R5) | Issue 3 AC1 + AC4 |
| 4 | Same-tick round trip carries; new template constant (R3) | Issue 3 AC1 + AC3 |
| 5 | Unit: entry + delivery + self-transition reports delivered; directed variant | Issue 1 AC5 — **was partial, now exact** |
| 6 | Unit: self-entry with no delivery record reports not delivered (R1) | Issue 1 AC7 |
| 7 | Unit: rewind with same source and target reports not delivered (R6) | Issue 1 AC7 |
| 8 | `koto rewind` right after a self-transition, next tick carries (R6) | Issue 3 AC1 |
| 9 | Pointer on both suppressed responses (R11) | Issue 3 AC1 — **was partial, now names both** |
| 10 | R12; three `src/cli/next_types.rs` unit tests unmodified | **GAP** — A6, minor |
| 11 | Two override tests pass with assertions unmodified (R8) | Issue 3 AC4 |
| 12 | `--full` on a suppressing tick; following non-advancing tick omits (R8) | Issue 3 AC1 — **was partial, now whole** |
| 13 | No-writes test's setup rerouted through a genuine arrival (R5, R9) | Issue 3 AC6 |
| 14 | Retrieval after a suppressing tick: returns, byte-identical, next tick omits (R9) | Issue 3 AC6 — **was GAP, now covered** |
| 15 | Init and batch-child first-tick tests unmodified (R2) | Issue 3 AC4 |
| 16 | Decision record does not re-deliver; pointer present (R13) | Issue 3 AC2 — **was partial, now exact** |
| 17 | Six named existing cases unmodified; only two tests change; new constant preferred | Issue 3 AC3 + AC4 |
| 18 | Unit: every existing case unmodified; exactly two invert | Issue 1 AC5 + AC6 — **was partial, now resolved incl. the rename caveat** |
| 19 | Baseline: no change inside `"stdout"`; nothing else in the fixture (R14) | Issue 3 AC7 — **was partial, now exact** |
| 20 | Unit: one log, gate and delivery give opposite answers (R15) | Issue 1 AC7 |
| 21 | `dashboard_data.rs`, `project.rs`, `derive_evidence` unchanged (R15) | Issue 1 AC8 — **was GAP, now covered** |
| 22 | Thirteen cases measured against a built binary, recorded in the PR body | Issue 3 AC8 — **was partial, now enumerated** |
| 23 | Nine named surfaces state the shipped rule | Issue 4 AC1 + AC2, Issue 1 AC9, Issue 2 AC2/AC3, Issue 3 AC5/AC7 — **was partial, now enumerated** |
| 24 | `git grep` returns no line asserting the old rule (R18) | Issue 4 AC7 — **was GAP, now covered but misplaced (D7)** |
| 25 | Both upstream documents record the reversal (R18, R19) | Issue 4 AC4 |
| 26 | `session-feed.md` and the `InstructionsDelivered` comment drop "not emitted yet" | Issue 4 AC2 + Issue 2 AC4 |
| 27 | Eval suite keeps two evals with verdicts, gains one (R20) | Issue 4 AC3 — **was partial, now exact** |
| 28 | `CHANGELOG.md` entry under `[Unreleased]` (R21) | Issue 4 AC6 — partial, C5, minor |
| 29 | fmt, clippy, test, stability, audit all exit 0 | Implementation Sequence; Issue 2 AC5 — **but see D6** |
| 30 | Documentation workflow schema validation passes | Issue 4 AC8 + Implementation Sequence |
| 31 | Plugin workflow template compilation passes | Implementation Sequence — **was partial, now names why it fires** |
| 32 | `src/engine/types.rs` adds no variant, no field, no schema bump (R16) | Issue 2 AC4 — **was GAP, now covered** |
| 33 | Both construction sites keep their read behavior (R17) | Issue 2 AC1 — **now states both halves explicitly** |
| 34 | Staging directory empty; no changed file names a path inside it | Implementation Sequence — second clause still dropped, minor |
| 35 | Pull request out of draft | Implementation Sequence — **was GAP, now covered** |

**Outline criteria with no PRD backing:** none. Issue 4 AC5, the output-contract
cross-reference, now carries its own justification tying it to R19 and the Scope
Summary flags it as record-keeping, which is what A5 asked for.

## Summary

Every round-1 finding is resolved or reduced to a minor: twenty of twenty-two
resolved outright, two partially, and the two PRD criteria that had no home at all
now have one. I re-verified the substantive claims rather than taking the revision's
word for them — the four unit-test assertions in B2, the twelve renamed assert lines
in B3, the five narrating comments in B1, the thirteen enumerated binary cases, and
all twelve files in Issue 4's `Files`. All check out.

The plan fails on one new blocking finding the round-1 fixes introduced. Issue 2's
`cargo test -- --test-threads=1` criterion cannot be true when Issue 2 completes:
three integration tests still assert the old rule at that point, all three are fixed
in Issue 3, and Issue 3 is blocked by Issue 2 — a cycle between the criterion and the
graph. Moving the suite run to Issue 3, or leaving it to the plan-level gate that
already names it, closes it. A second, softer instance of the same class: Issue 4's
repo-wide grep backstop is checkable only after Issues 1, 2 and 3 land their comment
rewrites, while Issue 4 declares `Dependencies: None` — that check belongs in the
verification gate, not in an outline.

On FC14: keep the diagram, but fix the reason. I reproduced the notice and confirmed
exit 0, so the "does not gate CI" claim holds. What the plan calls a validator quirk
is actually two shirabe reference files contradicting each other —
`phase-7-creation.md:358` requires the graph for single-pr,
`plan-format.md:207-219` bars it — and the plan should say it is choosing a side.
