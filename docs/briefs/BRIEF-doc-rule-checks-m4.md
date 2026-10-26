---
schema: brief/v1
status: Accepted
problem: |
  The rules a decider check could grade hold one unit at a time, but a check
  grades one slice per evaluation, so a section's units can't be graded on
  their own, and an agent whose work the decider fails gets no rule to fix
  against. Nothing records which unit was graded or which rule text went out.
outcome: |
  A template author grades every unit of a section in one state visit. An
  agent sees which unit failed and the broken rule, summary first and full
  text on a repeat; an ungradable unit goes to the agent while the rest are
  graded; and telemetry shows each unit's answer, fallback and delivery.
---

# BRIEF: per-unit decider checks that hand back the broken rule

## Status

Accepted

## Problem Statement

A decider check asks the decider one closed question per criterion about one
slice of the agent's work. The slice is everything the check's extraction
command prints, and it has to fit a budget of about 2.5 KB. One slice per check fits
a criterion that judges one small artifact, such as the comments a change
added. It doesn't fit the rules a document is held to. Those rules hold per
unit: is this acceptance criterion binary, does this user path name a user,
a trigger and an outcome, does this scope bullet stay at the right altitude.
The sections that hold the units run 3 to 17 KB, while a single acceptance
criterion is about 200 bytes. A template author who wants the decider to
judge each acceptance criterion today has two bad choices: cram the whole
section into one slice, which goes over budget and gets no verdict, or
declare one gate per unit, which nobody can do for a section whose length
isn't known when the template is written. With at most four criteria a
state and four consultations a `koto next`, even a fixed set of units
wouldn't fit.

When the decider does fail something, the agent learns little. The check's
finding names the criterion and its fail description in one line. A command
gate in the same workflow does better: its first failure puts the broken
rule's summary in the `koto next` response, and a repeat failure on the
same rule puts the rule's full text there, right where the agent reads
before it acts. A decider check records no output streams, so it can carry
neither. The agent whose third acceptance criterion isn't binary is told
"criterion failed" about the whole section, with no pointer to the unit and
no rule to fix against.

A maintainer deciding whether a decider criterion can ever be trusted
beyond shadow has nothing to decide with. Every decider check ships in
shadow until its verdicts are shown to agree with human reviewers on the
same units, and that comparison needs to know, from telemetry on every
host, which document, section and unit each verdict was about, how
confident the decider was, and what rule text the agent was then given.
The check's records carry the criterion and its outcome and stop there, so
no criterion can be compared unit by unit, and none can leave shadow.

## User Outcome

A template author writing a document-drafting workflow can point one
decider check at a section and have each unit in it graded on its own, in
the same state visit, under the budget the decider was validated at. They
don't have to know how many units the section will hold, and they don't
spend a gate per unit.

An agent drafting that document gets told exactly where it went wrong. When
two of its ten acceptance criteria aren't binary, the `koto next` response
names those two and gives the rule's summary. If it fixes them badly and
they fail again, it gets the rule's full text instead. A unit that couldn't
be graded, because it's too long for the budget or the decider didn't
answer, comes back to the agent to judge for itself, while the other units
still get the decider's verdict. The agent never has to guess which unit a
failure refers to, and never has the whole section held up by one unit the
decider couldn't read.

One failing unit is enough to fail the check, the same way one failing
criterion fails it today, and the agent is told which unit it was.

A maintainer on any host can see, from telemetry alone, what the decider
said about each unit and what the agent was handed back, and can line that
up against what human reviewers said about the same units. That's the
evidence a criterion needs before anyone trusts it to block.

## User Journeys

### A template author grades a PRD's acceptance criteria one by one

A maintainer adding a check state to a document-drafting workflow wants the
decider to judge whether each acceptance criterion in a PRD is binary. They
write one check that hands the decider the section's criteria one at a
time, and attach the rule an agent should get back when a criterion fails.
They don't know how many criteria the PRD will hold, and don't need to:
when the state runs, every criterion in the section is graded on its own in
the same visit.

### An agent fixes the two units that failed, then fixes one again

An agent drafting a PRD submits its draft and the check state runs. Eight
of ten acceptance criteria pass; two aren't binary. The `koto next`
response names those two units by key and gives the rule's summary. The
agent rewrites both and submits again. One now passes; the other still
fails, and this time the response carries the rule's full text. The agent
reads it, rewrites the criterion, and the state moves on.

### An agent gets one oversized unit back while the rest are graded

An agent's PRD has one acceptance criterion long enough to exceed the
per-unit budget. The check grades the other nine units and hands that one
back to the agent with the reason, its text and the criterion's question,
the way an unanswered criterion already falls back today. The agent records
its own answer for that unit; the decider's verdicts on the rest stand.

### A maintainer on another host reads the per-unit evidence

A maintainer who ran none of the sessions has to decide whether a per-unit
criterion agrees with human reviewers well enough to be trusted beyond
shadow. They run the telemetry readout for this feature and see,
for every graded unit on every host, the document, section and unit it
belongs to, the decider's answer and confidence, any fallback and its
reason, and each delivery of a failed rule at summary or full level. None of
it comes from a file on one host.

## Scope Boundary

### IN

- A way for a decider check to grade many units of one section in one
  evaluation, each held to the same per-slice budget a single slice is held
  to today, with an upper bound on how many units one check grades.
- Naming the failing unit, by its key, in the `koto next` response.
- Handing back the failed criterion's rule: its summary on the first
  failure in a visit, its full text on a repeat, from text the template
  declares.
- Per-unit fallback: a unit with no verdict goes to the agent through the
  existing fallback path while the other units are graded.
- Telemetry for each unit's answer, confidence, fallback and reason, and
  each rule delivery with its level, carried through the run journal so it
  exports, plus the definitions and readout that read it.
- Updates to the koto-author and koto-user skills for the new surface.

### OUT

- Moving `decider.mode`, or any criterion, out of shadow. Every criterion
  still ships in shadow; a per-unit fail blocks only where a template says
  veto and a user opts into it, exactly as today.
- Writing the document criteria themselves (binary acceptance criteria,
  user-path shape, altitude per bullet). This feature gives criteria a place
  to run; choosing and validating them is later work.
- A check state in shirabe's document skills. Where these checks run in
  /scope or /charter is separate work.
- A third rung that names an exit after repeated failures. Command gates
  get that from shirabe's scripts; a decider check stops at the full text.
- Batching several units or criteria into one decider request. One request
  per unit and criterion stays the configuration the decider was measured
  at.
- Changing the per-unit budget or the decider's accuracy threshold.
- The telemetry pipeline's own limits on how many series it keeps; the
  readout reads exported events, not new derived series.

