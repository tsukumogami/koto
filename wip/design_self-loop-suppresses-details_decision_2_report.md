# Decision 2: how the reversal is recorded

**Authorship note.** Two decision-researcher agents were dispatched for this
question and both died on transient API errors without writing anything. The
design author performed the analysis directly. Every claim below was checked
first-hand: the precedent commit was read with `git show --stat`, and the
supersession tooling was executed against a throwaway copy outside the repo.

## Question

`docs/prds/PRD-inline-phase-details.md` (Done) and
`docs/designs/current/DESIGN-inline-phase-details.md` (Current) both state the
boundary this work moves, and the DESIGN additionally argues for it. PRD R18 says
neither may be left asserting the old rule; R19 says the record must state that
the rule was reversed, by what, and why. How?

## The passages at issue

**`docs/prds/PRD-inline-phase-details.md`, Definitions** (the normative one):

> **Occupancy.** A phase's occupancy begins when a state-entry event names that
> phase as its target, and ends when the next state-entry event names any phase,
> including the same one. A self-transition therefore ends one occupancy and
> begins another, which makes it behave exactly like a loop-back through other
> phases: the instructions are delivered again on arrival. This is the same
> answer the criteria already require for a loop-back, and treating a
> self-transition differently would make the rule depend on the shape of the loop
> rather than on whether the workflow re-entered the phase.

**Same file, R3:** enumerates "a conditional transition, an unconditional
transition, a directed transition, **a self-transition**, a rewind, or workflow
initialization" as ways an occupancy begins.

**Same file, two acceptance criteria:** the self-transition arrival "carries the
instructions, matching the loop-back case above and the occupancy definition",
and two consecutive directed transitions into the same phase "both carry the
instructions, because each begins a new occupancy".

**`docs/designs/current/DESIGN-inline-phase-details.md`, frontmatter
`rationale`:** "makes rewind, self-transition, directed transition and multi-hop
auto-advance all fall out without special cases".

**Same file, Decision Outcome:** "every way of arriving at a phase — conditional
transition, unconditional transition, directed transition, self-transition,
rewind, initialization — appends one. So each of those starts a fresh occupancy
with no special case in the predicate".

**Same file, the passage that matters most:**

> **A contradiction in the PRD was corrected.** Its Definitions made a
> self-transition begin a new occupancy — so instructions must be delivered —
> while an acceptance criterion required a second consecutive directed transition
> into the same phase to omit them. [...] The Definitions are normative and R3 is
> explicit, so the criterion was rewritten to test what it was plainly reaching
> for: a directed transition followed by a non-advancing tick.

That paragraph is a record of a decision now reversed. It is the one passage that
cannot simply be edited to the new answer, because deleting it erases the fact
that the question was argued at all.

**Same file, Implementation Approach:** lists "a rewind entry, a self-transition
entry" among the unit-test cases, which changes with the tests.

## Options considered

### Option 1: amend both in place

Edit the Definitions paragraph, R3, the two criteria, the frontmatter
`rationale`, the Decision Outcome, and the contradiction passage. Keep both
statuses. Record the amendment in the documents' own Status prose and in the PR.

**Evidence.** The repo has a direct, maintainer-authored precedent. Commit
`70ba97c` ("docs: audit and clean up docs/", PR #169) amended Current designs in
place against the real code — `DESIGN-koto-template-authoring-skill.md` (+23),
`DESIGN-local-dashboard.md`, `DESIGN-local-session-storage.md`,
`DESIGN-backend-state-persistence.md`, `DESIGN-config-and-cloud-sync.md` — and in
the same commit amended settled PRDs in place, including
`PRD-session-persistence-storage.md` (20 lines),
`PRD-hierarchical-workflows.md` and `PRD-unified-koto-next.md`. Its own message
says "Every fix was verified against `src/`; every changed doc passes `shirabe
validate`". So amending a Current DESIGN and a settled PRD to match shipped code,
without a status change, is established practice here rather than an
interpretation of one.

The PRD format reference's "no Superseded state — if requirements change
fundamentally, create a new PRD" is the only rule that could bite. It does not:
R1, R2 and R4 through R25 of that PRD are untouched, R3 loses one item from an
enumeration, and the Definitions paragraph changes one clause. That is a
correction, not a fundamentally different requirements set.

**Consequences.** The two documents keep their identity, their paths and their
inbound links. A reader who lands on either gets the rule that ships. The
amendment costs roughly six edits across two files.

**How it fails.** If the amendment is written as a plain overwrite, the audit
trail thins: the contradiction passage's argument disappears and R19 is unmet.
The option is only correct together with the rule that the contradiction passage
is rewritten into a record of both rulings rather than deleted.

### Option 2: supersede the DESIGN

Author a successor DESIGN and run
`shirabe transition docs/designs/current/DESIGN-inline-phase-details.md Superseded --superseded-by <path>`.

**Evidence, measured.** Executed against a copy at
`$CLAUDE_JOB_DIR/tmp/DESIGN-copy-test.md`. The command succeeds (exit 0), sets
`status: Superseded`, adds `superseded_by:`, and `git mv`s the file to
`docs/designs/archive/`. It also rewrites the body `## Status` first line to:

> Superseded by [DESIGN-self-loop-suppresses-details.md](docs/designs/DESIGN-self-loop-suppresses-details.md)

`shirabe validate` then reports one error on the result:

> [FC03] frontmatter status "Superseded" does not match ## Status body
> "Superseded by [DESIGN-self-loop-suppresses-details.md](...)"

So the documented supersession path produces a document that fails validation.
That is a defect in the tooling rather than an argument about this decision, but
it is a real cost: taking this option means either hand-fixing the body line
after the tool writes it, or shipping a validation failure.

**Consequences.** The clearest possible signal that the old design is history.

**How it fails.** Disproportionate, and it loses more than it records. The
DESIGN's four decisions — record delivery as an event, extend `koto status`,
share one combinator across both response paths, splice the recovery pointer —
are all still correct and all still describe shipped code. A successor would
restate every one of them to change one boundary, and the archive would hold the
only copy of the reasoning behind decisions the code still follows. It also moves
the file out of `docs/designs/current/`, where anyone looking for the current
behavior will look first.

### Option 3: a separate decision record

Leave both bodies and add a standalone record naming the reversal.

**Evidence.** koto has no `docs/decisions/` directory; `ls docs/` returns
`briefs`, `designs`, `guides`, `plans`, `prds`, `reference`, `testing`. This
option would introduce an artifact class to the repo for one entry.

**Consequences.** Cheap to write, and it is the natural home for "reversed, by
what, why".

**How it fails.** It fails R18 outright. The Definitions paragraph would still
assert that a self-transition begins a new occupancy, and a reader arriving at
the accepted PRD has no reason to suspect a record elsewhere contradicts it. A
record that corrects a document only helps readers who already know to look for
it.

### Option 4: hybrid — amend the PRD, supersede the DESIGN

**How it fails.** Inherits Option 2's disproportion and its FC03 defect, and adds
an asymmetry a reader has to explain to themselves: why was one document worth
correcting and its child worth replacing, when the same clause moved in both?

## Recommendation

**Option 1, amend both in place, with the contradiction passage rewritten rather
than deleted.**

Concretely, the amendment does five things:

1. The PRD's Definitions paragraph states the delivery window as it now is,
   using the vocabulary this work's own PRD settles, and its closing sentence —
   which argues *for* the old answer — is replaced by one that records the
   reversal and points at koto#90 and the PRD that governs.
2. R3's enumeration drops "a self-transition" and gains the rewind clause.
3. The two acceptance criteria are inverted, and each says which document ruled.
4. The DESIGN's frontmatter `rationale` and Decision Outcome drop the claim that
   a self-transition falls out without a special case, and say that the delivery
   window skips one deliberately.
5. The contradiction passage is retitled and rewritten to say what actually
   happened: a contradiction was found, it was resolved toward delivery in the
   change that shipped this mechanism, the issue's author later ruled the other
   way, and the resolution was reversed. Both rulings survive in the record and
   the document says which governs.

Option 2 was right that a reader must be able to tell the record changed, and
step 5 is what carries that without archiving four decisions the code still
follows. Option 3 was right that "reversed, by what, why" wants a home of its
own, and step 5 gives it one inside the document that made the original ruling —
where the reader who needs it is already standing.

## The adjacent stale documents

`docs/prds/PRD-koto-next-output-contract.md` **needs nothing.** Its R9 reads:

> **Subsequent visits** (retries, self-loops, polling): `directive` is present,
> `details` is absent (omitted from JSON, not null). The caller already received
> the full instructions on the first visit.

That is the requirement this work restores. It was correct when written, was
overridden without being cited, and becomes true again when this ships. Editing
it would be the mistake.

`docs/designs/current/DESIGN-koto-next-output-contract.md` **is stale, and it is
not this change's staleness.** Its Decision 2 describes computing delivery from
`derive_visit_counts` — a mechanism the inline-phase-details work already
replaced. It says nothing about self-transitions and does not assert the boundary
R18 names, so it is outside R18's surface, and rewriting a superseded mechanism
decision would mean re-litigating something this change does not move.

Recommendation: add one cross-reference sentence to that decision naming the
design that replaced the mechanism, and nothing else. That is proportionate — it
stops the next reader believing a mechanism that is gone — without pulling a
second design's decision history into a boundary change. Say so explicitly in
Consequences so nobody reads the restraint as an oversight.

## Open questions for the design author

None blocking. One note: the FC03 defect in `shirabe transition ... Superseded`
is a real bug in a public repo's tooling, reproducible in one command. It is
worth reporting upstream, and it is not this change's job to fix.

## Summary

The two durable documents are amended in place, keeping their statuses and paths,
because commit `70ba97c` establishes exactly that practice for a Current DESIGN
and a settled PRD corrected against real code, and because the DESIGN's four
decisions all still describe shipped behavior — a successor would restate them to
move one clause. Supersession is disproportionate and, measured against a
throwaway copy, the documented tool writes a `## Status` body line that fails
FC03, so taking it would mean shipping a validation failure or hand-fixing the
tool's output. The passage that resolved koto#90's criterion the other way is
rewritten into a record of both rulings rather than deleted, which is where
R19's "reversed, by what, and why" lands.
