```yaml
review_result:
  topic: inline-phase-details
  round: 1
  mode: fast-path
  verdict: proceed
  categories:
    A:
      name: Scope Gate
      result: pass
      findings: 0
    B:
      name: Design Fidelity
      result: findings
      findings: 1
      resolved: 1
    C:
      name: AC Discriminability
      result: findings
      findings: 7
      resolved: 6
      dismissed: 1
    D:
      name: Sequencing / Priority Integrity
      result: findings
      findings: 1
      resolved: 1
  critical_findings: []
```

## Verdict

`proceed`. Nine findings were raised across categories B, C and D; eight were
applied to the PLAN in place, and one was dismissed with a reason. None required
a loop-back, because every finding was a criterion-level correction rather than a
decomposition or sequencing error that would invalidate the plan's shape.
Category A returned no findings: the five-issue split, the single-pr mode, and
the coverage of the design's four implementation phases all held.

## What changed, per finding

**B — the pointer's presence condition (issue 4).** The criterion said the
pointer appears "on exactly the responses where they were suppressed", which
narrows the design's ruling and contradicts the criterion above it. The design
keys presence on whether the phase *declares* instructions, which covers carrying
responses and suppressed ones alike. Rewritten to say so, and to state that an
implementation showing the pointer only when instructions were withheld does not
satisfy it.

**C1 — the override criterion passed vacuously (issue 2).** "Recording that
delivery changes nothing observable for existing override callers" is satisfied
by an implementation that never records on the override path at all, and nothing
else exercised the consequence. Added a criterion that an override call followed
by a plain non-advancing tick on the same occupancy returns no instructions,
which is the causal chain the original criterion left untested.

**C2 — the non-batch concurrency criterion did not discriminate (issue 3).** The
advisory lock is taken only for batch-scoped phases, so in a non-batch scenario
there is no lock to contend with under either a correct implementation or one
that wrongly tries to take one; both return immediately. Kept the scenario,
because it is the respawn race the problem statement leads with, and added the
criterion that actually discriminates: no lock syscall on the session state file
at all, verified under `strace`.

**C3 — the mismatch key was underspecified (issue 3).** "A conditionally-present
key naming the divergence" is satisfied by a bare boolean. Now requires the key
to carry both the hash recorded in the session header and the hash of the
template as read.

**C4 — byte-identity did not say which sequences (issue 2).** An implementer
could have diffed one trivial sequence. Now enumerates the same eight arrival and
call sequences the delivery criteria exercise, and says explicitly that the
simplest sequence alone does not satisfy it.

**C5 — issue 1's inertness claim was unfalsifiable.** "No response differs from
before this issue" needed the baseline fixture, which did not exist yet. Replaced
with two checkable claims — the predicate has no call site and the event is
appended nowhere, verified by searching the tree; and the existing suite passes.

**C6 — the eval criterion was satisfiable by gutting (issue 5).** "At least one
eval" plus "updated" permitted deleting the meaningful assertion. Now names what
the updated evals must assert: a second non-advancing tick omitting the
instructions, and a rewind arrival delivering them, with deletion-in-place-of-
replacement called out as a failure.

**C7 — dismissed.** The category's mechanical pattern pass flagged issue 4 as
happy-path-only on the literal absence of trigger keywords, and the reviewer
itself named this the weakest finding in its set. The issue does carry negative
paths — a phase declaring no instructions carries no pointer — and a terminal and
error criterion was added while addressing B, which strengthens it further. The
finding is a false positive of the keyword rule, not a coverage gap.

**D — the baseline capture had no producer.** The instruction to capture it lived
only in end-of-document prose, and issue 2 referred to it as an already-satisfied
fact. Moved into issue 1 as an explicit acceptance criterion, with the reason
stated: issue 1 is the last point at which the working tree still produces
pre-change responses. The Implementation Sequence section was rewritten to match,
and to say why the repository-wide gates sit in issue 5 rather than implying
verification is deferred.
