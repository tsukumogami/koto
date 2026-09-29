---
schema: brief/v1
status: Accepted
problem: |
  A template can hold a variable's value but can't route on it: a `when`
  clause only asks whether `vars.NAME` is set. Branching on the value
  falls to directive prose the agent must read and obey, which nothing
  checks and the session log never records.
outcome: |
  An author writes a branch on a variable's value as a transition. koto
  takes it from the value it already holds, the compiler catches a
  mistyped name or value, and the log says which variable and value
  chose the path.
motivating_context: |
  Workflow skills built on koto carry prose such as "if the mode is auto,
  skip the confirmation" in directives, because koto can't branch on a
  value. That prose costs agent attention on every run, and it's the kind
  of instruction a later canary wants to measure by splitting runs on an
  init-time variable.
---

# BRIEF: koto value routing

## Status

Accepted

Framing for the downstream PRD. The requirements, the matcher syntax, and
the compile-time checks are the PRD's and the design's to settle. Two
questions this brief left open, whether the canary needs value routing at
all and whether a routed variable must be immutable after init, are
resolved in the PRD's decisions.

## Problem Statement

A koto template declares variables that `koto init --var` fills in: an
execution mode, whether to merge, which plan document to follow. koto holds
those values for the whole session and substitutes them into directives and
gate commands. What it can't do is choose a path with them. A transition's
`when` clause matches submitted evidence and gate output by value, but for a
template variable it can only ask `vars.NAME: {is_set: true}`. Any other value
on a `vars.*` key is a compile error.

So a template whose path depends on a variable's value has one place to put
that dependency: directive prose. "If the execution mode is auto, record the
default and continue; otherwise ask the author." The agent has to read the
value out of its own directive and act on it, on every run. Three things go
wrong with that. The agent can misread the prose or skip it, and nothing
enforces the choice. A mistyped value in the prose never surfaces, because
nothing compiles prose. And the session log shows which state the run went to
next but not that a variable's value sent it there, so anyone comparing runs
has to reconstruct the reason from the directive text.

The cost compounds for anyone who wants to split runs on purpose. A canary
that sends runs holding one value down a different path needs the split to
be taken by koto and recorded by koto; a split the agent performs from prose
is neither.

## User Outcome

A template author who wants a run to go one way when `MODE` is `auto` and
another when it's `interactive` writes two transitions, and koto takes the
right one from the value set at init. The directive no longer explains the
branch to the agent, so that prose is gone rather than merely unused. When the
author misspells the variable or names a value the variable can't hold, they
find out while compiling the template, told which variable and which value,
instead of watching a run quietly fall through to a default.

Someone reading a finished session's log can tell which variable and value
sent the run down its path, so runs can be grouped by the path their
variable chose without reading any prose. Templates that never
route on a value compile and run exactly as before.

## User Journeys

### A skill author deletes a branch the agent used to resolve

A maintainer of a workflow skill has a state whose directive says "if the
mode is auto, submit the recommended default; if interactive, ask". They
replace that paragraph with two `when` routes on `vars.MODE`, one per value,
each leading to a state whose directive says only what to do on that path.
The agent never sees the branch, and the run goes where the value says.

### An author mistypes a value and the compiler catches it

An author writes `vars.MODE: atuo` on a transition. `koto template compile`
refuses the template with an error code that names the transition, the
variable, and the values `MODE` allows. They fix the typo before any session
runs, rather than discovering weeks later that the route never fired.

### An analyst explains why two runs of one template diverged

A workflow maintainer notices that two finished sessions of the same template
took different paths and wants to know why. They read both session logs. The
transition where the paths split carries the variable name and the value it
matched in each run, so the cause is on the page: one run started with
`MODE` set to `auto`, the other to `interactive`. They don't open the
template or interpret any directive text to find it.

### An experiment owner splits runs on an init-time variable

A maintainer running a canary wants some runs to include an instruction and
others to omit it, then compare the two groups. They start each run with a
variable set at `koto init` to one arm or the other. Whether the split is a
value route to a different state or a directive that names a different
reference file through the variable is the design's call, made against
how koto substitutes variables into directives today; either way koto takes the split rather than the agent, and the
maintainer groups finished runs by the arm each one started with. How the arm
is assigned belongs to the canary feature, not this one.

## Scope Boundary

**IN:**

- A `when` clause that matches a template variable's value by exact
  comparison, alongside the existing `is_set` matcher, which keeps working.
- Compile-time refusals for a value condition on an undeclared variable, a
  value outside a variable's declared allowed values, and value routes out
  of one state that can never match or that overlap.
- Recording, on the transition event, which variable and value a value route
  matched.
- A stated rule for what happens when a routed variable's value could change
  after init (a rebind or a capture), and how that is logged.
- Documentation in the template-format and session-feed references, and proof
  that an older koto still reads the new logs.
- An inventory of the prose in shirabe's skills and templates that this makes
  deletable, named by file and section, for shirabe to adopt later.

**OUT:**

- Any change to shirabe. Its templates adopt value routing in their own
  release, which is also where they raise their koto floor. A template that
  doesn't use value routing needs no floor change at all.
- Expressions beyond equality: ranges, regular expressions, arithmetic,
  negation across values.
- Routing on a decider's answer, or any change to the decider floor; a
  decider still never chooses a route.
- Canary sampling, withholding instructions from a run, and how a canary
  assigns its variable.
- Conditional text inside a directive. koto routes between states; it
  doesn't template directive prose on a condition.

## References

- `plugins/koto-skills/skills/koto-author/references/template-format.md` --
  the `when`, variables, and `skip_if` grammar this extends.
- `docs/reference/session-feed.md` -- the event contract the transition
  record extends.
