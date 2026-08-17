# /scope Handoff: self-loop-suppresses-details

## Provenance

Written by `/explore` on 2026-08-17 from
`wip/explore_self-loop-suppresses-details_crystallize.md`. Research files:
`wip/explore_self-loop-suppresses-details_findings.md`,
`wip/explore_self-loop-suppresses-details_decisions.md`, and
`wip/research/explore_self-loop-suppresses-details_r1_lead-*.md`.

One discover-converge round, five leads: the delivery predicate, the delivery
call sites, the durable upstream artifacts, the test/eval/doc blast radius, and
the event-log traces. Convergence narrowed the work from "make self-loops
suppress" to three named cases that move and eleven that must be proven not to,
and settled five design questions the leads surfaced. A second round was not
needed: every open question the leads raised was answerable from what round 1
already found, and nothing contradicted the measured baseline.

## Problem Statement

koto#90 acceptance criterion 3 says a self-loop omits a phase's `details`. Merged
`main` re-delivers them, because PR #197 defined an occupancy as beginning at any
state-entry event and ending at the next one -- which makes `P -> P` a boundary
and hands the agent instructions it already holds. The user has ruled AC 3
governs. Three response cases must start suppressing (a conditional
self-transition, a repeated one, and `koto next --to P` while already at `P`),
eleven adjacent cases must be shown not to move, and two upstream documents that
argue for the behaviour being removed have to be reconciled with the code rather
than left describing it.

## Scope Boundary

### In scope

- The boundary rule the delivery predicate uses, in
  `src/engine/persistence.rs`.
- The two `koto next` response-construction sites in `src/cli/mod.rs`, one of
  which carries a 24-line comment proving something that stops being true.
- Every test, fixture and eval that encodes either the old rule or an adjacent
  case that must not move.
- The koto-user and koto-author skills, `.cursor/rules/koto.mdc`,
  `docs/guides/cli-usage.md`, and `docs/reference/session-feed.md`.
- Reconciling `docs/prds/PRD-inline-phase-details.md` (Done) and
  `docs/designs/current/DESIGN-inline-phase-details.md` (Current) with the
  behaviour that ships.
- `CHANGELOG.md` under `[Unreleased]`.

### Out of scope

- A `koto phase-info` command. AC 4 is met by `koto status`, which returns
  `directive`, `details` and `expects` without moving the workflow; a second
  retrieval surface would be redundant.
- koto#193, #198, #199, #200 and shirabe#328. Filed, real, independent. #199
  touches this work only as a constraint: the predicate must not depend on which
  events a rewind appends.
- `derive_visit_counts` (`src/engine/persistence.rs:981`), the fossil of the rule
  #197 replaced. Not this change's business.
- `CURRENT_SCHEMA_VERSION`, which stays at 1. Nothing new is recorded.
- Auto-advanced intermediate phases surfacing neither directive nor details.

## Decisions Already Settled

- **Fork the occupancy slice; do not edit the shared one.** `occupancy_slice`
  also backs `latest_epoch_gate_failed`, the dashboard and `/workflows` blocked
  badge, which has no unit-test coverage. Widening its window across a self-entry
  would make a just-self-looped session inherit its pre-loop gate verdict. The
  two predicates answer different questions and may legitimately disagree; the
  doc comment claiming they must not has to be rewritten to say why.
- **A rewind always opens a delivery occupancy, whatever its `from`.** The
  sameness test applies to `Transitioned` and `DirectedTransition` only.
  `Rewound { from: P, to: P }` is reachable today, and the brief's own reason for
  rewind delivering -- it is a deliberate "redo this" signal -- holds for a
  self-rewind too. It also decouples the predicate from koto#199.
- **`koto next --to P` while at `P` suppresses.** Ruled by the user. The
  asymmetry with rewind is deliberate and has to be argued, not assumed.
- **Amend the upstream PRD and DESIGN in place; do not supersede.** Only the
  occupancy boundary moves; all four of the DESIGN's decisions stay correct.
  Precedent exists (`70ba97c`), and the documented supersession tool writes a
  body line that fails FC03 on any design carrying `schema:`.
- **Rewrite, do not delete, the DESIGN's "A contradiction in the PRD was
  corrected" passage.** It records the decision being reversed.
- **Sweep the two stale "not emitted yet" claims** in
  `docs/reference/session-feed.md` and `src/engine/types.rs`, which describe this
  feature and are wrong on `main` today.

## Coverage Notes

- The exploration did not decide what `advanced: true` with no `details` reads
  like to an agent that has never seen that combination. It is a real shape
  change on the response contract and nothing in the chain has argued it yet.
- It did not check whether `koto status`'s recovery pointer text, which claims to
  appear "precisely when it's needed, on the very responses that suppressed the
  details", still reads correctly when the suppressed response is a self-loop.
  The claim looks stronger, not weaker, but nobody re-read it against the new rule.
- It did not establish whether `docs/designs/current/DESIGN-koto-next-output-contract.md`
  Decision 3 -- which describes the visit-count mechanism #197 already replaced
  -- should be corrected here or left as separate debt. It is stale on `main`
  independently of this change.
- It did not enumerate how many `koto` templates in the plugin tree contain a
  self-loop on a phase that declares details. The one shipped `koto-author`
  template does not, but `koto template compile` across the tree has not been run.

## Upstream Observations

`docs/briefs/BRIEF-inline-phase-details.md` (Done) never uses the word
"occupancy" and never mentions self-transitions; the term enters the chain at the
PRD, which halves the reconciliation surface.

`docs/prds/PRD-inline-phase-details.md` (Done) carries the normative Definitions
paragraph that makes a self-transition begin a new occupancy, plus one clause of
R3 and two acceptance criteria that follow from it.

`docs/designs/current/DESIGN-inline-phase-details.md` (Current) cites those
Definitions as normative and carries a section headed "A contradiction in the PRD
was corrected", which resolved this exact conflict in favour of re-delivery and
rewrote the acceptance criterion that said otherwise. The reversal is not
correcting an oversight; it is overturning an argued decision, and the record has
to say so.

Working against that: the older Done PRD
`docs/prds/PRD-koto-next-output-contract.md` R9 already said "Subsequent visits
(retries, self-loops, polling): `directive` is present, `details` is absent" --
the literal source of koto#90's AC 3. The ruling restores an older written
contract rather than inventing one.

None of these is passed on the command line. `/scope` accepts a ROADMAP on
`--upstream` and the exploration found none.

## Framing-Shift Answer

**Pre-supplied answer:** no signal surfaced.

**Evidence:** the problem shape, audience and success criterion are all unchanged
from koto#90 as filed -- an agent revisiting a phase it is already executing
should not be re-sent instructions it holds. What moved is the answer to one
sub-question inside that framing (whether `P -> P` counts as a revisit), not the
framing. The BRIEF that would be affected does not mention occupancy at all.

## Shape Signals

### Architectural alternatives left open

- **Fork versus parameterise the slice.** Convergence settled that the shared
  `occupancy_slice` must not change meaning, but not *how*: a boundary-policy
  parameter on the existing function keeps one scan and makes its "shared so they
  cannot disagree" comment self-contradictory; a sibling function is more honest
  and adds a third near-duplicate backwards scan to the two that already exist
  (`derive_overrides` and `derive_last_gate_evaluated` each open-code the same
  walk). Both are cheap; they differ in what the code claims about itself.
- **Where the amendment record lives.** Amending in place is settled; the form is
  not. The BRIEF's convention (Status-section prose) is the only in-document
  precedent, and neither PRD nor DESIGN has an equivalent. CHANGELOG plus the PR
  body is the alternative, matching how `70ba97c` recorded its in-place design
  correction.

### Complexity signals

- The code change is one closure and a guard, but its correctness argument spans
  six arrival paths and two consumers of a shared helper, only one of which wants
  the new semantics.
- A contested trade-off needs settling in writing: a Current DESIGN argues, at
  length and with reasons, for the behaviour being removed.
- One reachable case (`Rewound { from: P, to: P }`) is absent from the issue, the
  PRD, the DESIGN and the brief's own semantics list, and a naive rule answers it
  wrongly by default.
- The verification surface is wide relative to the diff: a byte-identity fixture
  whose prose is embedded in a `.rs` file, an eval suite CI only counts rather
  than runs, and eleven behaviours that must be measured against a built binary
  to show they did not move.
