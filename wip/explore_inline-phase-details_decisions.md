# Exploration Decisions: inline-phase-details

Decision blocks follow the lightweight protocol in
`references/decision-protocol.md`. Running in `--auto`, so Step 3 follows the
recommendation and records `status` rather than asking.

| id | artifact | tier | status | question |
|----|----------|------|--------|----------|
| D1 | this file | 2 | confirmed | Is koto#90 still a greenfield feature request? |
| D2 | this file | 2 | confirmed | Whose statement of remaining scope governs? |
| D3 | this file | 2 | confirmed | Does the exploration continue, or close as already-done? |
| D4 | this file | 2 | assumed | Does the exploration need a round 2? |

## Round 1

### D1 -- Is koto#90 still a greenfield feature request?

**Question.** The issue proposes a template `details` field, first-visit-only
inclusion, and an escape-hatch command. Is any of that unbuilt?

**Evidence.** Five of seven research leads independently found the mechanism
shipped. Verified directly: `TemplateState.details` at `src/template/types.rs:57`;
`details: Option<String>` on every non-terminal `NextResponse` variant in
`src/cli/next_types.rs`; `derive_visit_counts` at `src/engine/persistence.rs:981`;
the gate `if full || count <= 1` at `src/cli/mod.rs:3999-4015`; the `--full` flag
at `src/cli/mod.rs:146`. Landed in PR #109, merged 2026-03-30, closing #102 --
never cross-referencing #90, which was filed four days earlier. Codified as
requirement R9 of `PRD-koto-next-output-contract.md`, status Done.

**Decision.** No. The exploration is a reconciliation against shipped code, not
a design from scratch. The issue body's "Proposed Template Format" is fiction:
`details` is delimited by an HTML comment marker `<!-- details -->` inside a
state's markdown body, not by a YAML key. Any downstream artifact must correct
this rather than inherit it.

**Consequence.** Leads 1, 5, 6, and 7 as originally framed are answered or moot.
The live surface is narrow: the gating's remaining defects and the missing
read-only recovery path.

### D2 -- Whose statement of remaining scope governs?

**Question.** The issue body, the research, and the issue author's audit
comments each describe a different remaining scope. Which is authoritative?

**Evidence.** The issue author posted a per-AC audit on 2026-06-07 and a second
on 2026-08-16T16:51Z -- roughly two hours before this exploration began. The
second measures behavior on koto 0.11.4 (`eb626d9`, 2026-08-05) with a
reproducible two-state loop and a tick-by-tick table, and states its own
re-scope: AC3's non-advancing-re-tick gap plus AC4, with AC4 the higher-value
half. The author flags which parts are measured and which are inference.

**Decision.** The 2026-08-16 comment governs. It is the most recent, the only
empirical evidence in play, and it comes from the issue's author.

**Consequence.** The exploration's deliverable is scoped to those two halves
plus whatever the research adds that the author had not tested. The issue body's
original acceptance criteria are superseded.

### D3 -- Does the exploration continue, or close as already-done?

**Question.** Given D1, is the right outcome to close #90 as superseded by #109?

**Evidence.** Two acceptance criteria are demonstrably unmet. AC3 fails on
non-advancing re-ticks -- measured by the author, root-caused by the research to
`derive_visit_counts` counting state entries (`Transitioned`,
`DirectedTransition`, `Rewound`) while a blocked tick appends only
`gate_evaluated`. AC4 has no implementation at all: `koto status` is read-only
but returns no directive or details (`handle_status`, `src/cli/mod.rs:4834-4960`,
verified), and `koto next --full` carries the text but evaluates gates and can
advance. The research added four further defects the author had not tested
(rewind suppression, respawn and batch-retry inheriting a stale count, `--to`
bypassing the check entirely, auto-advance intermediates never surfacing).

**Decision.** Continue. There is real, bounded work.

**Consequence.** A rejection record is off the table; the exploration is
heading toward a chain.

### D4 -- Does the exploration need a round 2?

**Question.** Is round 1's coverage enough to crystallize, or does a gap remain
that would change what gets built?

**Evidence.** Four of the five identified defects were never run -- they are
inferences from reading `handle_rewind`, `respawn.rs`, `retry.rs`, and
`dispatch_next`. Whether they reproduce on current `main` (0.11.6-dev, past the
author's 0.11.4 baseline) decides whether the work is one fix or four, which
changes the shape of any plan. Two further questions are unsettled and
constrain the fix rather than merely informing it: whether emission counting can
be recorded without violating R9's "no new state files", and what the escape
hatch actually costs across the skills, their evals, and the docs koto's own
CLAUDE.md makes mandatory.

**Decision.** Yes -- one more round, three leads: empirical reproduction against
current `main`, emission-counting feasibility under R9, and the escape hatch's
concrete surface and diff footprint.

**Status: assumed.** In interactive mode this would be the author's call. The
evidence favours the round because the count of real defects is not yet known
and it directly determines the size of the work, but a reasonable author who
trusts the code reading could skip it.
