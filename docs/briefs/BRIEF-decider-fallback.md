---
schema: brief/v1
status: Accepted
problem: |
  A decider check that gets no usable verdict passes its gate. An oversized
  slice, a failed extraction, a provider outage, an unreadable answer, a spent
  cap or a busy lock all end with the criterion judged by nobody, even under
  veto, and nothing exported says how often that happens or why.
outcome: |
  When the decider can't answer a criterion, the agent doing the work is asked
  it and its answer decides the gate, in shadow and veto alike. A maintainer
  tuning slice budgets reads every such fallback in Datadog, by check,
  criterion, reason and slice size.
motivating_context: |
  koto#297's ruling (merged as koto#300, released since v0.15.0) made a
  missing verdict never block, and as a side effect every missing verdict now
  passes. The maintainers ruled on 2026-10-14 that this is a defect: a check
  the decider can't answer must fall back to the agent, the way the routing
  decider already does, and every fallback must leave telemetry that can be
  read in Datadog.
---

# BRIEF: a check the decider can't answer goes to the agent

## Status

Accepted

Authored under `/scope`'s chain. The brief settles the problem, the outcome,
the journeys and the boundary. The PRD owns the requirements, and the DESIGN
owns how the agent is asked, how its answer is recorded, and the exact names
of the new event and its attributes.

## Problem Statement

A decider check grades the agent's own work: koto runs an extraction command,
sends the slice it prints to the decider with one closed question per
criterion, and reads a pass, fail or escape back. In veto mode a fail blocks
the state. Since koto#300, every outcome that isn't a verdict passes the gate.
The criterion is listed under the gate's `output.unanswered`, and its
`decider_checked` event records the reason, but the state moves on.

There are six such reasons in the code today. The slice can be over the
check's byte budget (`over_budget`), or the extraction command can fail
(`extraction_failed`), and in both cases nothing is sent. The provider can
fail twice (`provider_error`) or answer something koto can't read
(`unreadable_response`). The per-call consultation cap can be spent
(`cap_spent`), or another `koto next` can hold the decider lock past the wait
(`busy`). None of these says anything about the work. All of them end with
nobody judging the criterion: the decider didn't, and the agent was never
asked.

So an oversized slice passes silently. A criterion that is withheld from the
agent's instructions because a decider is meant to check it is checked by
nobody whenever its slice outgrows the budget, and a provider outage turns
every opted-in run's checks into passes. koto#297 was right that a missing
verdict shouldn't block on its own, since a checker fault isn't the agent's
fault. The effect was wider than that: a missing verdict now counts as a pass.

The second half of the problem is that the cases can't be told apart where
budgets get tuned. The run journal relays `decider_checked` with its reason,
but not the slice's size, and nothing marks the moments a criterion went
ungraded as a group to count. Someone deciding whether a 2,560-byte budget is
too small has to reconstruct that from scattered records, and can't read the
sizes at all.

## User Outcome

An agent working a state whose decider check couldn't be graded is told which
criterion is waiting on it, with the criterion's own question and its pass,
fail and escape descriptions, and answers it the way it would answer any
closed question koto puts to it. Its answer decides the gate: a fail blocks
until the work changes, a pass or an escape lets the state go on. A veto fail
from the decider still blocks exactly as before, and a template with no
decider checks behaves as it always has.

A maintainer tuning slice budgets and criteria opens Datadog and counts
fallbacks by check, criterion and reason, with each slice's size in bytes. An
over-budget criterion whose slices cluster just past its budget reads
differently from one whose provider keeps timing out, and both read
differently from a check that never falls back.

## User Journeys

### An agent finishes work whose slice is over budget

An agent running a shirabe workflow reaches a review state that declares a
decider check on comment reasons, with the user opted in to the decider. The
change it made adds 4 KB of comments, and the check's budget is 2,560 bytes.
koto sends nothing, records the criterion as unanswered with `over_budget`,
and instead of passing, stops the state and tells the agent the criterion is
waiting on its answer. The agent reads the question, judges its own comments,
answers, and calls `koto next` again. The state advances on a pass; on a fail
it fixes the comments first.

### An agent answers fail during a provider outage

An agent implementing an issue reaches a state whose veto check asks whether
every new public function has a doc comment. The decider's provider is down,
so both attempts fail and the criterion records `provider_error`. Before this
feature the state would have passed. Now the agent is asked, looks at its
diff, sees two undocumented functions, and answers fail. The state stays put,
the agent adds the comments and ticks again. The provider is still down, so
the agent is asked about the new slice and answers pass. The run never needed
an override, and it never moved past a check nobody graded.

### A maintainer tunes a slice budget

A maintainer suspects the comment-reason check's budget is too tight. In
Datadog they filter the koto run-journal records to the new fallback kind,
group by check, criterion and reason, and plot the slice sizes of the
`over_budget` ones. The sizes cluster between 2.6 and 3 KB, so they raise the
budget and re-run the accuracy spike for that size, then watch the fallback
count drop.

### A reviewer audits why a bad change got through

A pull request reviewer finds a comment that only restates the code, in a
change whose run had a veto check on comment reasons. They open the run's
session log to see why the check didn't stop it. The log shows the decider
went unanswered with `over_budget`, the agent was asked, and the agent
answered pass. The reviewer now knows the miss was the agent's own judgment,
not the decider's and not a silent pass, and raises the check's budget
question with the template's owner instead of the decider's accuracy.

## Scope Boundary

### In

- Every reason a decider check goes unanswered today, and any later one,
  falls back to the agent, in shadow and veto alike.
- How the agent is told a criterion waits on it, how it answers, how the
  answer is recorded in the session log, and how the gate follows it.
- One run-journal record per fallback naming the check, the criterion, the
  reason and the slice size, exported the way koto's other run-journal
  records are, with names that collide with no existing attribute path.
- The run-journal reference, the session-feed contract, the decider authoring
  guide and the koto-user skill describing the fallback.
- Proof that 0.14.1-era readers still read logs carrying the new events, the
  way koto's existing compatibility CI jobs prove it.

### Out

- The routing decider's fallback to the agent, which already works and stays
  as it is.
- Changing any check's slice budget or criteria, which the slice-contract work
  owns.
- shirabe adopting the fallback in its templates. It needs a koto release and
  a floor raise first.
- The Datadog measurement definitions that admit the new record and decide
  how an awaiting check's gate is counted. Their owners decide that, and
  this feature names the counting change as left open for them.
- Counting fallbacks or agent answers in `koto decider report` or the ledger.
  The run journal is where this feature's telemetry goes.
- Cutting a koto release.
