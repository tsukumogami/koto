---
schema: brief/v1
status: Accepted
problem: |
  koto dashboard's session reader lives inside koto's one package, next to
  storage and network dependencies and to the code that writes and locks the
  store, so a second program that wants the same session answers, such as a
  fleet view in shirabe, would have to copy the logic or link all of it.
outcome: |
  A second program reads the same sessions, states and liveness classes
  koto dashboard shows, from the same code, without linking koto's storage
  and network stack and without being able to write to the store, and
  koto dashboard shows exactly what it showed before.
---

# BRIEF: A session reader koto shares with other programs

## Status

Accepted

This brief frames the problem the spike in koto#661 left open for the
design that answers it, `docs/designs/DESIGN-koto-shared-session-reader.md`.
It restates the problem that design's Context and Scope already describe and
adds no requirement of its own.

## Problem Statement

`koto dashboard` reads every session in the store, works out each one's
current state, notices which sessions changed since the last refresh, and
classifies each one by liveness. A fleet view being built in shirabe wants
the same answers, from koto and nothing else. Its owner set one constraint
over it: status comes from koto, agents write nothing new, and their koto
calls cost what they did. Whatever reads the store on the view's behalf must
not write to it.

Today that code can't be shared. koto is one package, and a program that
depends on it links rust-s3 with rustls, the attohttpc client, the http and
url crates, and the ratatui and crossterm terminal stack. The dashboard's
data layer sits inside koto's CLI module, and the persistence module it reads
logs through also appends to them, takes their locks and writes supersede
fences. A second program would either copy the logic, and drift from koto's
row derivation and liveness, or take on the dependencies and the write code
with them.

The spike in koto#661 showed the logic separates, but it reached koto's
source through `#[path]` attributes that can't be packaged, koto didn't link
the result, and the copy still contained koto's append and lock code. The
split itself, what of the store layout a reader may rely on, and how a
dependent takes the code were left undecided.

## User Outcome

A program outside koto, the fleet view first, gets the answers `koto
dashboard` shows from the code that produces them, not from a copy. It
doesn't link koto's storage, network or terminal dependencies, and it can't
write to, lock or otherwise change the store it reads. Whoever runs `koto
dashboard` sees the same sessions, states and liveness classes on the same
store as before, and an agent's koto calls do the same work they did.

## User Journeys

### Build a second program on koto's reader

A shirabe developer building the fleet view needs each session's state and
liveness. Today the choice is copying koto's derivation or linking all of
koto. The outcome is a dependency on koto's reader alone, pinned to a koto
release, with no storage or network crates in its graph.

### Review that the reader can't write

A reviewer of a change to the shared code needs to know it still can't touch
the store. Today "doesn't write" would hold only because nothing calls the
write code beside it. The outcome is a check that fails CI when the shared
code gains a way to write.

### Run koto dashboard after the split

A koto user runs `koto dashboard` on an existing store after the reader moves
out. The outcome is the same rows, states and liveness classes they saw
before, with nothing to change on their side.

## Scope Boundary

In scope:

- Which of koto's session-reading code is shared and what stays in koto.
- Making the shared code unable to write, in a way a reviewer can check.
- What of the store layout the shared code promises, how it finds the store,
  and how it reads one recorded context value.
- How a dependent takes the shared code and how its versions relate to
  koto's.
- Showing `koto dashboard` unchanged and a minimal second program working.

Out of scope:

- The build itself, which a plan carries after the design.
- Seats, holders and roles in koto, and any read verb for them.
- Any change in shirabe, its install or its release, and cutting a koto
  release.
- The remote rows other machines push to the bucket, which the dashboard
  keeps showing and the shared code doesn't read.
- Recording the external signals koto#556 asks for, beyond leaving room for
  them.

## References

- koto#661, the spike this brief follows, and its report in
  `docs/spikes/SPIKE-koto-shared-session-reader.md`.
- `docs/designs/DESIGN-koto-shared-session-reader.md`, the design that
  answers it.
