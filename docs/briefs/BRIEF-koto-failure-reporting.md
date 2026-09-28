---
schema: brief/v1
status: Accepted
problem: |
  When a koto check fails, the agent gets too little to act on: a failing
  command gate arrives as a bare exit code and whatever the check printed is
  lost. Afterwards nobody can tell from the session log how many tries a state
  or a rule took, what context the agent read, or who wrote a key a check read.
outcome: |
  An agent whose check failed learns what failed, where, how serious it is and
  whether its change landed, so its next attempt is a fix rather than a guess.
  Anyone reading the session afterwards can count attempts per state and per
  rule and trace context reads and writers from the session feed alone.
motivating_context: |
  koto workflows increasingly hand deterministic checks to scripts, and the
  agent's only feedback from those checks is pass or fail. A separate effort
  wants to export gate events for measurement, which needs a stable,
  documented event shape to read.
---

# BRIEF: koto failure reporting

## Status

Accepted

Framing for the koto-failure-reporting feature. Both review passes returned
PASS. The downstream PRD owns the requirements, including what counts as one
attempt and which events reset a count, the bound on captured command-gate
output, and the requirement that secrets stay out of it; the design owns the
payload and event shapes and the mechanisms behind those requirements.

## Problem Statement

A koto workflow stops an agent at a gate until a check passes. When the check
fails, the agent is the one who has to fix whatever is wrong, and today it is
told almost nothing about what that is.

A command gate is the clearest case. The check script may print exactly which
file is wrong and why, but koto keeps only the exit code, so the agent sees
`exit_code: 1` and has to rediscover the failure by re-running the check by
hand, reading the script, or guessing. A state's `default_action` already does
better: its failure comes back with up to 64 KiB of the command's output and a
line of fallback prose. Gates, which are where most checks live, never got the
same treatment. The agent can't tell a lint error from a missing file, a
warning from a blocker, or a change that took effect from one that didn't.

The same gap shows up after the fact. The session log records that a gate was
evaluated, but not which rule inside a check failed, how many times the agent
tried a state before it passed, what context the agent read before it acted,
or who wrote the context value a check tripped on. A template author trying to
work out why a state takes six tries, or a maintainer measuring how often
checks bounce agents, has to reconstruct all of it from transcripts, and a
tool that wants to export gate events has no field-level contract to read
against.

## User Outcome

An agent whose check failed gets back, in the same `koto next` response, what
failed, where it failed when the check can say, how serious it is, and whether
the effect it was attempting landed. Its next attempt goes at the actual
problem instead of re-running the check to find out what the problem was.

A template author gets that for free from the check scripts they already
write: a script that prints a rule id and a location has them reach the agent
without the author learning a koto-specific output format or changing the
template.

Someone reading a finished session, whether a person debugging a workflow or a
program exporting its gate events, can see how many attempts each state and
each rule took, what context was read along the way, and who wrote each key,
from the session feed alone and without reading koto's source.

## User Journeys

### An agent fixes a failing check on the first retry

An agent running a workflow submits evidence for a state whose command gate
runs a lint script. The script fails. The agent's `koto next` response names
the rule that failed, the script's own message, the file and line, and that
the evidence was recorded but the transition did not happen. The agent edits
that line and resubmits, and the gate passes on the second attempt rather than
the fifth.

### A template author gets useful failures without learning a new format

A template author wires an existing check script into a gate. The script
already prints a readable error to stdout and exits non-zero. Without changing
the script, the agent now sees that output, bounded and with secrets kept out.
Later the author adds a rule id and a location to the script's output, and
those arrive as structured fields.

### A maintainer finds the state that keeps bouncing agents

A koto maintainer looking at a batch of sessions wants to know which states
take the most tries. They read the session feed, count attempts per state and
per rule, and see that one rule accounts for most retries. They follow the
context reads before those attempts and find the agent reading a key another
state wrote with a stale value, and the feed names that writer.

### An exporter reads gate events without reading koto's source

A separate program exports gate events for measurement. Its author reads the
published session-feed contract, finds each gate event's fields written out
with their types and meanings, and builds the exporter against that. Sessions
from templates that use none of the new reporting still parse, and the
exporter ignores what it doesn't know.

## Scope Boundary

**IN**

- What a failed check returns to the agent: the rule that failed, its message,
  a location when the check can compute one, a severity level, a pointer to
  the rule's full text (carried as an opaque value for now), and whether the
  attempted effect landed.
- Capturing command-gate output, with a stated size bound, and returning it on
  failure the way `default_action` failures already are.
- Keeping secrets that appear in captured output out of both the agent's
  response and the session log.
- Counting attempts per state and per rule, with a written definition of what
  counts as an attempt and when counts reset.
- Logging context reads and recording which writer produced each context key:
  the agent, a state's own action, a transition's assignment, or a child
  workflow.
- A gate-event schema, written out field by field in the published
  session-feed contract and extended under that contract's versioning rules.
- Keeping existing templates working unchanged, so the feature forces no
  minimum koto version on anyone who doesn't use it.

**OUT**

- An escalation ladder that acts on the attempt counts (for example, stepping
  up guidance or handing off after N failures). The counts are recorded here;
  acting on them is a later feature.
- A named exit a workflow takes when a check can't be satisfied. Also later.
- Where rule ids come from and what the rule-text pointer resolves to. Here a
  rule id is an opaque string; a later rule-registry feature defines both.
- Changes to shirabe or any other consumer's templates. Consumers adopt the
  new fields in their own features.
- Waiting on CI from a gate, clearing stale context keys, and routing values
  between states. Each is a separate koto feature.
- The exporter itself. This feature defines the event shape it reads, not the
  program that reads it.

## References

- `docs/reference/session-feed.md` -- the published session-feed contract the
  gate-event schema extends.
- `docs/designs/current/DESIGN-jev-decision-offload.md` -- the decider design
  whose feed additions follow the same versioning rules.
- `src/gate.rs` and `src/engine/advance.rs` -- where gates are evaluated and
  where the `default_action` failure path already returns bounded output.
