# Verdict: PASS

Re-review of the revised `docs/briefs/BRIEF-self-loop-suppresses-details.md`,
read fresh from disk. Both blocking findings from the first pass are resolved,
and so are the seven others. Seven residual findings below, all non-blocking and
all one-clause edits; two of them are things the revision introduced.

## Re-review

| # | First-pass finding | Status |
|---|---|---|
| 1 | Open Questions contradicted User Outcome on rewind (BLOCKING) | Resolved |
| 2 | The PRD and DESIGN the brief is about were unnamed (BLOCKING) | Resolved |
| 3 | "combinator" — DESIGN altitude in the OUT list | Resolved (over-corrected, see F3) |
| 4 | Normative "must not depend on" inside an OUT item | Resolved |
| 5 | Frontmatter `outcome` covered one of three users | Resolved (see F4) |
| 6 | Journey 3 did not connect to what changes | Resolved |
| 7 | Journey 2 contradicted journey 1 on what a lap carries | Resolved |
| 8 | "the ruling" had no antecedent | Resolved |
| 9 | British `behaviour` against repo convention | Resolved (0 occurrences) |

Detail on the two that were blocking:

**Finding 1.** The User Outcome now reads "Being rewound into a phase still
delivers, including a rewind that lands on the phase it started from, because a
rewind is an instruction to redo the work rather than to continue it", and
journey 4 agrees: "it stays a delivering arrival even when the rewind lands on
the phase the workflow was already standing in." The open question is gone. The
brief now says one thing about the case, and it matches what
`wip/scope_self-loop-suppresses-details_handoff.md` recorded as settled.

**Finding 2.** The Problem Statement's third paragraph now names
`docs/designs/current/DESIGN-inline-phase-details.md`,
`docs/prds/PRD-inline-phase-details.md`,
`docs/prds/PRD-koto-next-output-contract.md`, and koto#90. The scope-IN bullet
names the PRD and DESIGN. References carries all four with a one-line
description each. I checked the four paths and the two quoted anchors: all four
files exist, the DESIGN carries the passage "A contradiction in the PRD was
corrected" (line 246), and the PRD has the Definitions section the reference
attributes the boundary to (line 135). The audit trail now closes inside the
durable document.

Mechanical re-checks, so the next reviewer does not repeat them: `shirabe
validate` exits 0 on the file, and `--check R7 --visibility public` reports "All
checks passed" — the prose-rule family, including em dash density and the banned
word list, is clean. Zero occurrences of `behaviour`.

## Findings

### F1 — MODERATE. The Status prose now claims the PRD owns a decision the brief made

**Status:**

> The downstream PRD owns which arrivals deliver and which do not; the DESIGN
> owns where the boundary rule lives and what happens to the durable records
> that describe the old one.

That sentence was true of the previous draft, where the rewind case sat open. It
is not true of this one. The User Outcome now settles which arrivals deliver:
looping inside a phase suppresses, arriving from elsewhere delivers, a rewind
delivers including a self-rewind, and the read-only retrieval always returns.
The one thing left to the PRD is why an explicitly targeted transition and a
rewind land on opposite sides.

A PRD author who reads Status before the body will think the delivery rule is
theirs to choose. Fix: say the PRD owns the requirements that operationalize the
rule and the asymmetry the brief leaves open, not the rule itself.

### F2 — MODERATE. The directed-transition decision exists only inside the Open Question

**Open Questions:**

> A directed transition into the phase the workflow already occupies is an
> explicit operator instruction, like a rewind, but this brief's outcome puts
> the two on opposite sides of the line. The downstream PRD owns stating why.

Deferring the rationale is a legitimate deferral and the right call — the
handoff records the decision as ruled and the argument as owed. But the outcome
does not "put the two on opposite sides of the line" in so many words. It places
the rewind explicitly and leaves the directed transition to be inferred from the
general rule ("not charged for it again while it stays there", and a directed
transition into the occupied phase is not an arrival "from somewhere else").

So the revision has swapped which of the two mirror cases is implicit. Before,
rewind was the loose end; now it is the directed transition, and the Open
Question describes the User Outcome as more explicit than it is. This matters
because these two are the cases a downstream author is most likely to get wrong,
and the brief is the thing that tells them the answer.

Fix: one clause in the User Outcome's second paragraph — being sent to the phase
the workflow already occupies does not deliver, because the workflow has not
left. Then the Open Question asks only for the argument, which is what it should
ask for.

### F3 — MINOR. The OUT list lost more than the altitude problem

**Scope Boundary, Out:**

> Re-deriving the mechanism that shipped. It stays as it is; only the boundary
> moves.

My first-pass finding named one clause — "sharing one combinator between the two
response paths" — and said the other two, "recording delivery as an event" and
"extending the status command", were fine because both describe observable
behavior rather than structure. All three went.

What remains is a real exclusion but a generic one. A downstream author reading
it learns that the mechanism stays, not which parts of it. Restoring the two
behavioral items gives the boundary back its teeth without bringing the altitude
problem back.

### F4 — MINOR. The frontmatter outcome dropped the retrieval

The rewrite covers all three users, which is what the first-pass finding asked
for. In making room it cut the clause about the read-only retrieval returning
the instructions on demand.

The body does not treat that as incidental: the User Outcome's second paragraph
ends on it, journey 3 is entirely about it, and the first OUT item is justified
by it. A summary that omits the property three other sections lean on is the
same mismatch as before, one size smaller. The frontmatter is at its four-line
budget, so this costs a compression somewhere — "and every other arrival still
delivers it" could absorb "with the instructions always retrievable on demand".

### F5 — MINOR. A redundant tail in the rewind sentence

**User Outcome:**

> ... because a rewind is an instruction to redo the work rather than to
> continue it, and an agent asked to start a procedure over is handed the
> procedure.

The second clause restates the first. It reads like text absorbed from the old
journey 4 when journey 4 was rewritten, and it leaves a 46-word sentence in a
paragraph that is already the densest in the section. Cut the tail.

### F6 — MINOR. Journey 4 narrates its own role in the document

> An operator decides a phase was done wrong and rewinds the workflow into it.
> The next response carries the full instructions. This journey marks the
> delivering side of the line the feature moves: being sent back to redo work is
> not the same signal as continuing a lap ...

The first two sentences are the journey. The rest is the author explaining to
the reviewer why the journey is in the document. Journeys 1, 2, 3 and 5 all stay
inside the user's story and let the point land on its own; this one steps
outside. The content is right — the self-rewind clause at the end is the part
that had to be added — so this is a rephrasing, not a cut: keep the operator in
frame and let the delivering-side point come from what happens to them.

### F7 — MINOR. The IN list paraphrases where naming would be shorter

> Every user-visible surface that rule reaches: the ordinary `koto next` tick,
> an explicitly targeted transition, the flag that forces delivery, and the
> read-only retrieval.

Better than the internal-path enumeration it replaced. But "an explicitly
targeted transition" and "the flag that forces delivery" are periphrasis for
`koto next --to <phase>` and `koto next --full`, which are user-visible surfaces
and therefore fair game at this altitude — the first item in the same list
already names `koto next`. Naming them is both shorter and less ambiguous.

## What is good

Everything the first pass praised survived. The Problem Statement's first two
paragraphs are intact, including the occupancy definition, the "lap two of a loop
it has been executing continuously" framing, and the measured cost — a
seven-thousand-character block re-sent thirteen times across a fourteen-week
sweep. The fourth paragraph still refuses to call the shipped definition a
mistake, which is what makes the maintainer journey and the reversal-recording
scope item coherent.

Two things the revision made better rather than merely compliant. Journey 3 now
carries the argument that suppression on a self-loop "turns the read-only
retrieval from a backstop into the only route" — that is a genuine consequence of
the rule change, and it is the reason the journey belongs in this brief rather
than the sibling one. And splitting the User Outcome's third paragraph into a
template-author paragraph and a maintainer paragraph gave the documentation
outcome the weight its scope footprint always implied.

The OUT list is still real work item by item, References is now the strongest
section-for-its-size in the document, and the five journeys remain five distinct
entry points.

## Summary

PASS. All nine findings from the first pass are resolved, including both blocking
ones: the rewind contradiction is settled toward delivery in both the outcome and
journey 4, and the PRD and DESIGN the brief is about are now named in the prose,
the scope-IN list, and References, with the quoted anchors verified against the
files.

The most important residual is that the revision swapped which mirror case is
implicit — the directed transition into the already-occupied phase is now the one
the User Outcome leaves to inference, while the Open Question describes it as
already stated there.

Not blocking. That and the six other residuals are one-clause edits, and none of
them reopens the framing.
