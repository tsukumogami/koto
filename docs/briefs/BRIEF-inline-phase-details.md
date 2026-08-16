---
schema: brief/v1
status: Accepted
problem: |
  koto decides whether to send a phase's long-form instructions by counting
  entries into a state rather than deliveries of its instructions, so it
  re-sends to an agent that is standing still and withholds from one told to
  redo a step. No log-derived rule can do better, because the log records
  nothing about who is attached to the session -- and there is no read-only way
  back to the instructions once they are gone.
outcome: |
  An agent driving a koto workflow has the current phase's procedure whenever it
  needs it and stops paying for it once it has it. When the procedure is gone --
  to compaction, a respawn, or a fresh process resuming the session -- one call
  returns it without moving the workflow.
motivating_context: |
  koto#90 asked for this behavior as a new feature. It shipped underneath the
  issue in PR #109 four days after it was filed, with no cross-reference, and
  the issue stayed open. Two audits by the issue author and a two-round
  exploration established that the shipped mechanism is wrong in both
  directions, and that the recovery path the issue treated as an escape hatch is
  what makes the suppression safe at all.
---

# BRIEF: phase instructions an agent can rely on

## Status

Accepted

Authored under `/scope`'s chain for koto#90. The framing here stops at the
problem, the outcome, the journeys, and the boundary; the downstream PRD owns
the requirements, and the DESIGN owns what koto records and where the read-only
recovery lives.

## Problem Statement

A koto template can attach long-form instructions to a phase, and koto decides
per response whether to send them. It makes that decision by counting how many
times the workflow has *entered* the state. What it needs to know is how many
times it has *delivered* that state's instructions to whoever is asking. The two
quantities diverge, and because they diverge in both directions, the behavior
inverts the intent on the cases that matter most.

A tick that evaluates gates, fails them, and does not transition enters nothing,
so the count never moves and the instructions are re-sent — on every tick, for
as long as the agent stays blocked. That is precisely the repeat case the
suppression exists to prevent. A rewind, meanwhile, *is* an entry: the state
being rewound into is by construction one the log already recorded, so the count
is already past the threshold and the instructions are withheld from an agent
that has just been told to go back and redo that step. A third path bypasses the
decision entirely — a directed transition always sends them, whatever the count
— so the rule is not even uniform across the ways a workflow moves.

Retuning the count does not reach the deeper problem. koto is being asked to
answer "does the caller still have these instructions?" using a log that records
only where the workflow has been, never who is attached to it. Those are the
same question only while one uninterrupted agent process drives a session from
start to finish. A cold-restart respawn breaks it concretely: a brand-new
subagent with no context continues on its predecessor's log and inherits its
count. Context compaction breaks it worse, because it leaves no event at all —
and the payload in question is a tool result, which the platform documents as
compaction-eligible and not guaranteed to survive a turn.

So the withholding cannot be made correct on its own. It needs a way for an
agent to ask for the instructions back, and koto does not have one. `koto
status` is genuinely read-only but returns neither the directive nor the
instructions. `koto next --full` returns them, and it also evaluates gates,
re-runs any `default_action` shell command, can auto-advance a routing state,
and can trigger cleanup of a terminal session. An agent recovering from context
loss cannot know in advance whether the call it makes to recover will move the
workflow underneath it. The only remaining options are to pass `--full` on every
tick, which discards the entire saving, or to read the template file directly,
which is the file read the feature exists to eliminate.

The cost is not hypothetical. In a recorded run of a fourteen-iteration sweep,
a phase carrying a 7,140-character procedure emitted it once, on the first
iteration. The next thirteen received a 101-character directive and nothing
else, and one of those iterations sat through fourteen consecutive gate-blocked
ticks with the procedure suppressed throughout.

## User Outcome

An agent driving a koto workflow can rely on having the current phase's
procedure. It receives the procedure when it reaches a phase for the first time,
stops receiving it once it demonstrably has it, and gets it back — reliably,
and without moving the workflow — whenever it no longer does.

The three failures that make the current behavior untrustworthy stop happening.
An agent sitting on a blocked gate is not re-sent the same block of text on
every tick. An agent told to rewind and redo a step is given the procedure for
that step. An agent that has lost its context — because it was compacted, or
because it is a fresh process that respawned onto an existing session — has one
call it can make to get the procedure back, and making that call has no effect
on the workflow's state, its gates, or its side effects.

For the template author, the behavior becomes predictable enough to design
against: a phase's instructions arrive when the agent needs them, the response
does not carry the same text tick after tick, and the recovery call is
documented where an agent will find it. A workflow whose loop runs longer than
an agent's context is something an author can now write without choosing
between paying full price on every iteration and having the procedure silently
disappear.

## User Journeys

### An agent stuck on a gate stops re-reading the same instructions

A coding agent reaches a phase whose gate checks for a condition that is not yet
true. It receives the phase's directive and its full procedure, does the work,
and calls `koto next` again. The gate still fails. Today the response carries the
entire procedure again, and again on every subsequent tick, because nothing
transitioned and so nothing counted. After this feature, the second and every
later blocked tick carries the directive and omits the procedure, because koto
is tracking that it already delivered it rather than that the workflow moved.

### An agent told to redo a step is given the step's procedure

An operator or a coordinator rewinds a session to an earlier phase because the
work done there needs redoing. The agent calls `koto next` and lands on that
phase. Today the procedure is suppressed, because the rewind target is by
definition a state the log has already entered — so the one moment an agent is
explicitly being asked to re-execute a phase is the moment koto decides it does
not need the instructions. After this feature, arriving at a phase by rewind
delivers the procedure.

### An agent that lost its context gets the procedure back without moving

A long-running workflow loops for many iterations. Partway through, the driving
agent's context is compacted and the phase procedure it received on iteration
one is gone — or the agent crashed and a fresh subagent respawned onto the same
session with no context at all. The agent needs the procedure and must not
advance the workflow, re-run the phase's side-effecting command, or trip a gate
to get it. It makes a single read-only call keyed by the workflow name, receives
the current phase's directive and procedure, and continues from exactly where
the workflow was. The call it needs to make is discoverable from something koto
sends on every tick, so an agent that has lost everything else still knows to
reach for it.

### A template author stops routing procedures through a separate file

An author is writing a phase whose procedure runs to several thousand
characters. Today the choice is between putting it in the phase body, where a
long loop will either repeat it on every blocked tick or lose it entirely after
the first arrival, and doing what the largest templates in use already do:
making the directive a one-line instruction to go read a file. The file read is
the cost this mechanism was built to remove, and authors keep paying it because
the mechanism cannot be trusted over a loop. After this feature the author puts
the procedure in the phase and stops maintaining a parallel file, because the
delivery rule holds for as long as the loop runs. An author who attaches no
procedure sees behavior identical to today's.

## Scope Boundary

### In scope

- The rule that decides whether a phase's instructions ride in a response,
  including what koto records in order to decide and where that record lives.
- The gate-blocked non-advancing re-tick, which re-sends the instructions
  indefinitely.
- The rewind case, which suppresses the instructions on the phase an agent is
  being told to redo.
- The directed-transition path, which applies no rule at all. Making the
  contract uniform is in scope; which reading wins — defect or deliberate
  carve-out for explicit operator intent — is the DESIGN's to settle and record.
- The cold-restart respawn case, where a zero-context agent continues on its
  predecessor's session log.
- A read-only way to retrieve the current phase's directive and instructions,
  keyed by the workflow name, that changes no workflow state and triggers no
  side effect.
- How an agent that has lost its context learns that the recovery call exists.
- The downstream work koto's own contributor guide makes mandatory for changes
  under `src/cli/` and `src/engine/`: the `koto-author` and `koto-user` skills,
  their evals, and the CLI usage guide.

### Out of scope

- **Auto-advance discarding the phases it crosses.** A `koto next` that advances
  through an intermediate phase surfaces neither that phase's instructions nor
  its directive. It predates this mechanism and is broader than it — the honest
  framing is that auto-advance has never surfaced intermediate instructions at
  all, and folding it in here would disguise that as a details regression.
  Filed separately.
- **Two consecutive rewinds moving the session forward.** A correctness bug in
  how the rewind target is selected, which makes an early phase unreachable once
  the workflow has passed it. Adjacent — a rewind-aware change here touches the
  same function — but unrelated to instruction delivery. Filed separately.
- **`accepts:` not gating advancement.** A transition with no condition fires
  regardless of any `accepts` block, so a chain of phases an author believes are
  interactive can run to completion in one call. A template-grammar and
  documentation problem. Filed separately.
- **The migration scan's output flood.** Every invocation against a populated
  session store re-runs a scan that prints one skip line per legacy session.
  It obstructed measurement during the exploration and is very likely the
  already-open issue #193.
- **Retrofitting existing templates onto the mechanism.** The largest workflow
  templates in use still point each phase at a separate file to read. Moving
  them across is adoption work in other repositories, downstream of anything
  koto changes here.
- **Changing the shared visit-count derivation itself.** It has a second
  consumer unrelated to instruction delivery, so its semantics are a constraint
  on the solution rather than a target of it.

## References

- `docs/prds/PRD-koto-next-output-contract.md` — the Done requirement (R9) that
  introduced the instructions field, the first-arrival rule, and the `--full`
  override. This feature amends it rather than replacing it.
- `docs/designs/current/DESIGN-koto-next-output-contract.md` — the matching
  design decision on visit-count computation, including a persisted-counter
  alternative it rejected and the constraint behind that rejection.
- `docs/guides/cli-usage.md` — where the current behavior is documented to
  authors and agents.
