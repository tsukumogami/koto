---
schema: brief/v1
status: Accepted
problem: |
  koto can't tell that a step uses something scarce on the host. When
  several sessions on one machine reach steps that run heavy test suites,
  they all start at once, the machine overloads, suites fail on timing and
  the 30-second action limit cuts unrelated work, so people queue by hand.
outcome: |
  A template author marks a step as drawing from a named host pool, an
  operator sizes that pool once per machine, and sessions from any
  workspace queue for it on their own. With no pool defined, nothing
  changes, and nobody has to configure anything to keep working.
---

# BRIEF: host resource pools

## Status

Accepted

Framing for a koto feature that lets a workflow step take a slot from a
named pool shared by every session on one machine. The downstream PRD owns
the requirements, along with three framing questions this brief leaves open.
None of them blocks the framing.

- **Can a workspace cap its own share of a host pool?** The host sizes a
  pool; whether a workspace may hold itself below that size, and which value
  wins when two consumers configure the same pool differently, is open.
- **What happens to a background step that holds a slot when its session
  moves to another machine or syncs through the cloud backend?**
- **Does a step that holds a slot and then runs something asking for the
  same pool get it automatically, or wait like everyone else?**

## Problem Statement

koto runs many sessions side by side. Agents on one machine each drive their
own session, often from separate workspaces that know nothing about each
other. Most steps are cheap and run happily in parallel. A few are not: a step
that runs a project's full local test suite can take from a few minutes to the
better part of an hour and use most of the machine while it does.

Nothing in koto knows the difference. A template can't say "this step uses
something the host has little of", and koto has no notion of a resource shared
across sessions, so when two or three sessions reach such a step together, all
of them start. The machine overloads, the suites fail each other on timing,
and the engine's fixed 30-second limit on ordinary actions starts cutting
steps that pass on an idle machine. One session's heavy step damages every
other session on the host.

People route around it by hand. Coordinators queue every koto command through
a person, one at a time across the host, because nobody can predict which tick
will start a suite: a single `koto next` can pass through several states whose
gates already pass and land on the heavy one. That costs a hundred or more
messages per run and leaves sessions idle behind each other. A script-level
lock inside one plugin's test runner covers that plugin's suites, but only
those, sized by an environment variable, and koto never sees it.

The heavy step also outlives the command that starts it. Because koto gives an
action 30 seconds, a long suite is launched by a quick action into a detached
background process, and a later tick reads its result. Any limit that holds a
slot only while the action runs would release it the moment the suite starts.

## User Outcome

A template author marks the heavy step of a workflow with the name of a host
pool, and an operator who wants that step limited on a machine sets the pool's
size once, in that machine's koto config. From then on, sessions from any
workspace on the machine that reach the step take a slot or wait for one on
their own; a waiting session's tick says it is queued and who holds the pool,
and a later tick picks it up without failing the run. The slot is held for as
long as the heavy work really runs, and it comes back when that work ends or
crashes.

Nobody else has to do anything. A machine with no pool defined runs every step
exactly as it does today, and an older koto that doesn't know the keyword runs
the template unchanged. Agents and coordinators stop asking permission before
ticks, because no tick can start a second suite.

## User Journeys

### A template author marks the heavy step

A maintainer of a workflow template notices that the verification state's
suite overloads shared machines. They add the pool name to that state's action
and ship the template. On machines that define the pool the step queues; on
every other machine, and on hosts still running an older koto, the template
behaves as before, so the change needs no coordination with its users.

### An operator sizes a pool on a shared machine

Someone running several agent sessions on one workstation decides two suites
at once is the most the machine can take. They add the pool and a size of 2 to
the koto config in their home directory, once, without touching any workspace
or template. Every workspace on the machine, including ones created later,
shares those two slots.

### A session reaches the heavy step while the pool is full

An agent's single tick carries its session from review through two passing
gates into verification while two other sessions hold both slots. The tick
does not start a third suite and does not fail. It answers that the run is
queued, names the sessions holding the pool and how long it has waited, and
the agent's next tick takes the free slot and starts the suite.

### A holder crashes

An agent's session holding a slot is killed mid-suite, or the machine's
memory guard stops it. Its slot comes back within seconds without anyone
deleting a lock file, and the agent of the next waiting session proceeds on
its next tick. When the killed session's agent resumes, its run is reported as
lost rather than as passed or failed, and it can start the step again.

### An operator checks who holds what

A person sees sessions waiting and wants to know why. One command lists each
pool, its size, the sessions holding slots with their workspace and how long
they have held them, and the sessions queued behind them in order.

## Scope Boundary

### In

- A way for a template to name the host pool a state's action draws from
- Pools and their sizes defined per machine in the user's koto config, with an
  undefined pool meaning no limit at all
- Slots shared by every session and every independent workspace of the user on
  one machine, released when their holder ends or crashes
- Heavy work that outlives a tick holding its slot for its real length, with
  its result reaching a later tick
- A full pool reported as queued, never as a failure, with holders named and
  the run resumable on a later tick
- Fair ordering among waiters, and a way to see holders and waiters
- Moving shirabe's work-on verification step onto the feature

### Out

- Pools shared across machines or across users on one machine: pools are per
  user per host
- Sizes set by a template or by a project's own config: only the machine's
  operator sizes a pool
- Limiting CPU or memory directly, as cgroups or a scheduler would: a pool
  counts concurrent steps, not resources consumed
- The per-session state-log write lock: two writers on one session are a
  separate problem with its own fix
- Keeping the script-level suite lock in shirabe: it is replaced, not extended
