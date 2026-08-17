---
schema: brief/v1
status: Done
problem: |
  koto sends a phase's long-form instructions once per occupancy, and it counts
  a phase transitioning to itself as ending one occupancy and beginning another.
  An agent going around a loop it is already inside is re-sent a procedure it
  already holds, every lap, which is the cost the suppression exists to avoid.
outcome: |
  An agent looping inside one phase pays for that phase's procedure once; every
  other arrival still delivers it, and it stays retrievable on demand. A
  template author reading the agent-facing documentation, and a maintainer
  reading the durable design record, both find a rule matching what koto does.
motivating_context: |
  koto#90 asked, in writing, that self-loops omit the instructions, and the
  older output-contract PRD it drew from said the same. The mechanism that
  shipped resolved a contradiction in its own upstream the other way and
  recorded that resolution as a definition, without flagging that it overrode an
  acceptance criterion of the issue it was fixing. The criterion has since been
  ruled to govern, which makes a Done PRD and a Current DESIGN wrong about what
  ships.
---

# BRIEF: a lap around a loop is not a new arrival

## Status

Done

Authored under `/scope`'s chain for koto#90. The framing stops at the problem,
the outcome, the journeys, and the boundary. Which arrivals deliver is settled
here; the downstream PRD owns the requirements that operationalize it, and the
one argument this brief leaves open — why an explicitly targeted transition and
a rewind land on opposite sides, which closes in the PRD's Decisions and
Trade-offs section. The DESIGN owns where the boundary rule lives and what
happens to the durable records that describe the old one.

The Phase 4 jury returned PASS on both reviewers.

## Problem Statement

A koto phase can carry long-form instructions, and koto sends them the first
time an agent arrives and withholds them afterwards. "Afterwards" is bounded by
an *occupancy*: the stretch of the session log that begins when a state-entry
event names the phase and ends at the next state-entry event naming any phase.

A phase that transitions to itself appends a state-entry event naming itself, so
under that definition it ends one occupancy and starts another, and the
instructions are sent again. The agent is not new to the phase. It is on lap two
of a loop it has been executing continuously, holding the same procedure it was
handed on lap one, and it is charged for that procedure again on every lap for
as long as the loop runs. On the workflow that prompted koto#90's audit — a
fourteen-week horizon sweep cycling three phases once per week — that is a
seven-thousand-character block re-sent thirteen times to an agent that never
left.

The definition is not an oversight. It was reached deliberately while scoping
the fix that shipped it, to resolve a contradiction between two upstream
statements, and it was written down as a resolution:
`docs/designs/current/DESIGN-inline-phase-details.md` carries a section headed
"A contradiction in the PRD was corrected", and the definition it cites as
normative sits in `docs/prds/PRD-inline-phase-details.md`. What went unnoticed
is that one of the statements it overrode was an acceptance criterion of the
issue being fixed, koto#90, and the other was a requirement in
`docs/prds/PRD-koto-next-output-contract.md`, an accepted contract that had
already settled the same question the other way. Neither was cited, so the
override reads as a definition rather than as a reversal. The issue's author has
since ruled that the acceptance criterion governs.

The result is a gap between what the issue asked for and what ships, and a pair
of durable documents — that PRD and that DESIGN — which describe the shipped
behavior correctly and the intended behavior not at all. Anyone who reads them
to find out what a self-transition does gets a confident, argued, wrong answer.

## User Outcome

An agent that keeps looping inside one phase receives that phase's procedure
once and is not charged for it again while it stays there. What it costs to run
a long loop stops scaling with the number of laps.

Nothing the agent might still need becomes unreachable. Arriving at a phase from
somewhere else — including coming back to one it visited earlier — still
delivers. Being sent to the phase the workflow already occupies does not, since
the workflow never left. Being rewound into a phase still delivers, including a
rewind that lands on the phase it started from, because a rewind is an
instruction to redo the work rather than to continue it. Asking for the
instructions outright still returns them without moving the workflow, and every
response that withholds them still says where to ask.

A template author reading the agent-facing documentation finds a rule that
matches what the engine does, stated in terms of arrivals rather than of an
internal boundary, so they can predict what their own template will emit.

A maintainer reading the durable design record finds the same rule, with the
reversal recorded rather than papered over — so the next person to ask why the
behavior is what it is gets the current answer and the history of it, instead of
an argument for behavior the code no longer has.

## User Journeys

### An agent on lap twelve of a weekly sweep

A long-running workflow cycles through the same phase once per iteration. On the
first arrival the agent receives the phase's full procedure and works from it.
On every later lap the phase transitions to itself, and the response carries the
short directive and nothing else. The agent, which has been executing this
procedure continuously, keeps working from what it already has. The per-lap cost
of the loop is the directive, not the directive plus the procedure.

### A template author writing a retry loop

An author adds a self-transition to a phase that carries instructions, then opens
the koto-author reference to find out what koto will repeat. The reference tells
them a lap around the same phase repeats the directive and nothing more, and
that leaving the phase and coming back is what delivers the instructions again.
They can predict the response their template produces without reading the engine.

### An agent that lost the procedure to compaction

An agent resumes a session mid-loop with the phase's procedure no longer in
context. Under the new rule no further lap will bring it back — suppression on a
self-loop turns the read-only retrieval from a backstop into the only route — so
the directive it is holding has to name that retrieval, and it does. The agent
calls it, receives the directive, the instructions and the expectations, and the
workflow has not moved: no event was appended, no lock was taken, and the next
tick behaves exactly as it would have without the call.

### An operator sending an agent back to redo a phase

An operator decides a phase was done wrong and rewinds the workflow into it. The
next response carries the full instructions, so the agent starts the procedure
over with the procedure in front of it. That holds even when the rewind lands on
the phase the workflow was already standing in: the operator asked for a redo,
not for another lap, and gets the same response either way.

### A maintainer auditing why the behavior is what it is

Someone traces the delivery rule back through the durable record. They find the
definition that governs today, and, where the earlier one was reversed, a
statement that it was reversed, by what, and why — rather than an argument for
the behavior the code no longer has.

## Scope Boundary

### In

- The rule that decides whether a response carries a phase's instructions, and
  in particular whether entering a phase from itself counts as an arrival.
- Every user-visible surface that rule reaches: the ordinary `koto next` tick,
  `koto next --to <phase>`, `koto next --full`, and `koto status`.
- The tests, fixtures and evals that encode the current rule, and the adjacent
  behaviors that must be shown not to have moved.
- The agent-facing surfaces that state the rule: the koto-user and koto-author
  skills, the editor rules file, and the CLI usage guide.
- `docs/prds/PRD-inline-phase-details.md` and
  `docs/designs/current/DESIGN-inline-phase-details.md`, to the extent they
  define the boundary being moved, plus the changelog entry.

### Out

- A dedicated command for re-reading a phase's instructions. The read-only
  retrieval already exists on the status command and returns the same text the
  tick would; a second surface for the same answer is redundant, and the issue's
  own wording asks for that command "or similar".
- Any change to what koto records in a session log. This moves a rule that reads
  the log, not the log's contents.
- The other open defects in this area — a required field that does not gate
  advancement, a rewind that oscillates, a session log truncated under concurrent
  writers, and a migration scan that floods stderr. They are filed, real and
  independent. The rewind defect is adjacent rather than in scope: both it and
  this work turn on how rewinds are read.
- Whether a phase the engine passes straight through should surface anything at
  all. That is a separate question about auto-advanced intermediate phases.
- Re-deriving the mechanism that shipped. Recording a delivery when one happens,
  and answering a direct request for a phase's instructions without moving the
  workflow, both stay as they are; only the boundary moves.

## References

- `docs/prds/PRD-inline-phase-details.md` — the accepted requirements for the
  shipped mechanism; its Definitions section is where the occupancy boundary this
  brief moves is stated normatively.
- `docs/designs/current/DESIGN-inline-phase-details.md` — the current design for
  that mechanism, including the passage that resolved koto#90's criterion the
  other way.
- `docs/briefs/BRIEF-inline-phase-details.md` — the framing for the mechanism
  this brief moves a boundary inside.
- `docs/prds/PRD-koto-next-output-contract.md` — the older accepted contract
  whose requirement on subsequent visits this brief's outcome restores.
