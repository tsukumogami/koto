---
schema: brief/v1
status: Accepted
problem: |
  A template author can't hand koto a mechanical step and trust it. The
  engine runs a command when a state is entered, but the output is
  discarded, a failure changes nothing, the command runs wherever
  `koto next` was typed, and no rule says which steps qualify.
outcome: |
  An author moves a mechanical step into the state and the workflow gets
  more reliable for it: the command's answer reaches the states that
  follow, a failure stops the run and hands the agent the output plus the
  by-hand instructions, and a session ticked from the wrong tree refuses.
---

# BRIEF: koto runs the mechanical commands

## Status

Accepted

Framing for koto's command-execution surface: what a template state can
be trusted to run, and what has to be true around the command before an
author will move one there.

The downstream PRD owns the requirements, along with three framing
questions this brief deliberately leaves open. None of them blocks the
framing.

- **Does a write to koto's own context store count as a side effect
  under the authoring rule?** The rule is stated against changes to the
  repository and to remote services; a write to the engine's own store
  is a different risk class and nobody has classified it. The answer
  decides how much of an existing workflow is convertible.
- **What happens to sessions that already exist when anchoring lands?**
  They have no recorded tree, so the refuse-on-mismatch behavior has
  nothing to compare against. Refuse until someone binds them, warn once
  and adopt the tree found, or bind silently? New-session behavior is
  settled; this is the migration story, and it decides whether the
  feature ships as additive or as breaking.
- **Is pushing commits to a pull request that's already open quiet
  enough for the engine to do?** Opening one is clearly not; pushing to
  a branch nobody watches clearly is. The case between rests on one
  checkable claim about how loudly a push to an open pull request
  notifies, and the answer moves a handful of commands across the
  boundary.

## Problem Statement

A koto template can already declare a `default_action` -- a shell command the
engine runs itself when a workflow enters a state. It shipped in March 2026, it
works in the current release, and no template in use has ever declared one. The
workflows that drive koto hardest, in the shirabe plugin that is koto's main
consumer, instead spell the mechanics out in prose for the agent to copy: "then
run `git rev-parse --abbrev-ref HEAD` to get the current branch." No design,
issue, or pull request in either project records a decision to skip the
capability. It was dropped, not declined.

It stayed unused because an author who reaches for it finds that the command
runs and nothing else does.

**A command's answer has nowhere to go.** koto captures what the command prints
and throws it away. Any step whose whole value is its output -- the branch name,
the issue number, the count that decides what happens next -- can't move into a
state, because the state that follows has no way to read what the previous one
learned. The steps most worth automating are exactly the ones the surface can't
carry.

**A failure changes nothing.** A state that declares an action and no gates has
no failure detection at all: the command exits non-zero and the workflow advances
as though it worked. Even where a gate does catch the problem, the response
doesn't carry the action's output, so the agent is told the state is blocked and
not why. That leaves an author with no fallback to design around -- no way to say
"run this, and if it doesn't work, here's how to do it by hand" -- which is the
first thing anyone wants before trusting an engine with a step. The shape of that
risk isn't hypothetical: twelve child workflows once got dispatched against a
branch nobody had created, because the error that would have stopped them was
filtered away.

**The command runs wherever `koto next` was typed.** Nothing binds a session to
the tree it was created in. Open a second terminal in a second checkout, tick the
same session, and the branch command reports on a tree that has a different
branch checked out. This one isn't specific to actions -- the gates shipping
today run against the same unbound directory -- so it predates the question and
would outlive an answer that only covered actions.

Underneath all three sit two defects in code that gates and actions share. One
mishandles a command that produces a lot of output, so the step waiting on it can
stall rather than finish. The other is the path that carries a session between
machines, which never records where the session was running. Gate authors are
exposed to both today, before any action exists, which is why fixing them once
fixes both surfaces.

There's a fourth gap, and it's the one that doesn't show up in a stack trace. An
author deciding whether to move a step also has to decide whether the step is one
the engine should be running at all. Some commands are safe to hand over and some
are permanently not, the difference isn't obvious from the command itself, and
nothing in the project says which is which. Today the entire authoring surface
for `default_action` is one row in a format table and a single integration test,
so every author reaching for it re-derives the judgment alone and reaches a
different answer.

The result is that the mechanical work stays with the agent. Turns go on
`git rev-parse` and `gh pr list`; prose instructions grow to cover shell the
engine could run; and each place the prose drifts from what the repository
actually needs becomes a failure nobody sees until much later.

## User Outcome

A template author moves a mechanical step out of the prose and into the state,
and the workflow gets more reliable for it. The command's answer is available to
the states that come after it, so a step can be automated because of its output
rather than in spite of it. A command that fails stops the workflow where it
stands and hands the agent the command's own output alongside the instructions
for doing the step by hand, so a broken step becomes a handoff instead of a
silent advance. And a session ticked from a tree it doesn't belong to refuses
and says which tree it does belong to.

The author also stops re-deriving the judgment call. Where the boundary sits
between a command the engine should run and one the agent should keep is
something an author looks up rather than something each author invents, and the
reasoning behind it is recorded alongside it. Two authors arriving at the same
question arrive at the same answer.

For the agent running the workflow, the change is what it spends turns on. The
bookkeeping steps stop arriving as instructions to carry out and start arriving
as facts already established, and what's left in the prose is the work that
actually needed an agent. For the developer watching the session, the change is
that the workflow either does the mechanical step or says clearly that it
couldn't -- and never, quietly, does it somewhere else.

## User Journeys

### Journey 1: A template author moves a mechanical step into the state

A maintainer of a koto-backed workflow is revising a state whose prose tells the
agent to run a command and hold on to the answer for later. They declare the
command on the state, name where its answer lands, and write the later state's
prose to read that name. The instruction telling an agent to run shell comes out
of the template entirely. When they run the workflow, the state advances with the
value already in hand, and the state that needed it never had to ask.

This is the journey the whole feature exists for; it fails today because the
output has nowhere to land.

### Journey 2: An agent hits a failing action and falls back to prose

A coding agent driving a workflow calls `koto next` and enters a state whose
command fails -- a tool that isn't installed, a tree that isn't clean, a branch
that isn't there. The session doesn't advance. The agent gets the exit status,
the command's own error text, and the state's written fallback instructions, and
does the step by hand before continuing. The developer watching sees the failure
named and handled in one turn, rather than seeing a run that looks healthy until
it isn't.

This is the path that has to exist before an author will move anything into a
state at all.

### Journey 3: A developer ticks a session from the wrong tree

A developer keeping two checkouts of the same repository -- a worktree for the
current branch, the main clone for everything else -- picks up a session in the
second terminal and runs `koto next` there. koto refuses and names the directory
the session is bound to. Nothing runs against the wrong tree, and the developer
either moves or deliberately rebinds the session.

This journey is reachable without any of the others: today's gates already run
unanchored, so the exposure exists whether or not a single action is ever
declared.

### Journey 4: A template author decides an outward-facing command doesn't qualify

An author wants to put `gh pr create` in a state so the engine opens the pull
request. The authoring guidance gives them the test -- does the risk live in a
bad success, or only in a bad failure? -- and `gh pr create` fails it: a
successful run is itself the unrecallable event, notifying reviewers and
consuming a number, and no signal that arrives afterward can take it back. They
leave the command with the agent and move on, without spending an afternoon
re-deriving the boundary. A later author reading the same guidance stops at the
same place.

This journey produces no automation, and it's the one that keeps the others from
being applied where they shouldn't be.

## Scope Boundary

### In scope

The feature covers koto's command-execution surface end to end -- what runs,
where it runs, what happens to the output, and what happens when it fails. In
parts:

- **Output routing** -- a command's output reaching the states that come after
  the one that ran it, rather than being captured and discarded.
- **A failure path** -- a failing action stopping the workflow and delivering the
  action's own output to the agent along with the state's written fallback,
  including for states that declare no gates and today detect nothing.
- **Execution anchoring** -- binding a session to the tree it was created in and
  refusing when it's ticked from somewhere else. The promise is the directory a
  workflow starts in, stated as exactly that.
- **A deliberate rebind** -- a way for a developer who genuinely moved a checkout
  to point a session at its new tree, so a refusal is a speed bump rather than a
  dead end.
- **Two shared-path defects that make the surface unreliable today** -- a command
  producing a lot of output can hang the workflow that's waiting on it, and a
  session that moves between machines arrives with nothing recording where it
  ran. Both bite gate authors right now, before a single action is declared.
- **A durable authoring rule** for which commands an engine should run and which
  belong to the agent, written where an author will find it, together with real
  authoring documentation for `default_action`.

### Out of scope

- **Rewriting the shirabe plugin's templates to use any of this.** It depends on
  this work and belongs in that project, afterward. Nothing about that rewrite is
  blocked on analysis; it's blocked on the engine.
- **The hardcoded commands in shirabe skills that aren't koto-backed.**
  `default_action` doesn't exist outside a koto template, so no engine change
  reaches them.
- **CI monitoring as a typed koto integration.**
  `docs/designs/current/DESIGN-default-action-execution.md` names that as the
  right long-term home for the work, and it's a separate feature with its own
  shape.
- **Template integrity of any kind.** Decided, not deferred: rely on the pinning
  agent harnesses already provide. No expected-hash argument on `koto init`, no
  manifest of template digests, no koto-side trust store, no extension of release
  checksums to templates. The space was mapped and then closed on purpose; it is
  not simply undone.
- **Bounding what the event log records.** Command strings, gate-override
  payloads, and init-time variables go into the log unbounded and unredacted.
  That's a real problem about what gets written down, and it isn't this feature's
  problem. It has no issue yet, and filing one is a follow-up this feature owes --
  the exposure is zero only while no template declares an action, which is the
  condition this feature is designed to end.
- **Renaming `requires_confirmation`.** The flag executes the command and only
  then asks, which makes its name wrong. The authoring rule this feature adopts
  makes the flag unnecessary rather than dangerous, so the rename is tidying and
  its blast radius has never been counted.
- **Containing what an authorized command can reach.** Anchoring says where a
  command starts, not where it can go; a command can name absolute paths or
  change directory regardless. Every mechanism that would close that gap either
  collapses into documentation nothing enforces or breaks koto's single-binary,
  no-sudo, four-platform distribution. Recorded as settled so it isn't reopened
  from scratch.
- **Routing engine-run commands through the agent's permission layer, or
  confirming each one before it runs.** Loading a workflow is itself the grant:
  invoking a koto-backed workflow authorizes the commands that workflow bakes in,
  deliberately. Relocating consent from per-command prompting to the decision to
  run the workflow is what lets koto carry mechanical work at all, and building a
  preview-and-approve step would reproduce the prose-plus-gate pattern this
  feature exists to replace.
- **Retiring the retry-clearing instructions in the shirabe workflows.** That
  waits on a decision in that project about
  `tsukumogami/shirabe:docs/designs/current/DESIGN-work-on-retry-clearing.md`,
  which is marked Current and chose its approach deliberately. No engine change
  retires it.

## References

- `docs/designs/current/DESIGN-default-action-execution.md` -- the design that
  shipped `default_action`, and the origin of the automation-first intent. Its
  Consequences section already concedes two of the risks this brief inherits.
- `docs/designs/current/DESIGN-shirabe-work-on-template.md` -- carries the
  workflow model whose states are the conversion candidates, including the two it
  names as targets, which don't survive the authoring rule as drawn.
- koto issue #71 -- commissioned `default_action`, with an acceptance criterion
  the shipped `requires_confirmation` flag does not meet.
- koto issues #193 and #204 -- the migration-warning volume, and context
  assignments being discarded while koto's own warning recommends them.
