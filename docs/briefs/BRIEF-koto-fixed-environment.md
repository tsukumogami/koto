---
schema: brief/v1
status: Accepted
problem: |
  Gates and default actions read whatever tools and shell state the ticking
  process carries, so one session gives different verdicts from different
  shells: a `PATH` shim is read silently, an exported function can replace a
  tool, and a non-overridable gate bends to the caller's environment.
outcome: |
  Whoever ticks a session gets the verdict it would give from any shell: its
  commands find the tools they found at start, a shim in the ticking shell
  changes nothing, a move onto other tools is refused by name, and
  credentials still come live without being written into the session.
---

# BRIEF: a session's verdicts don't depend on the shell that ticks it

## Status

Accepted

Framing for the environment koto's gates and default actions run with, from
koto issue #261. Both reviewers passed the draft.

The downstream PRD owns the requirements and two framing questions this brief
leaves open; neither blocks the framing.

- **Default or opt-in.** Whether the fixed environment applies to every
  session by default or only to templates that opt in, and what the
  transition looks like for templates that assume the caller's environment.
- **Older sessions.** What a session created by an older koto does on its
  first tick: adopt the ticking shell's tool path once and record it, or keep
  running as before until it's explicitly bound.

## Problem Statement

A koto session binds a lot at `koto init`: the template, its variables, the
directory its commands start in. It doesn't bind the environment those commands
run with. Every command gate and every default action is started as `sh -c`
with the full environment of whichever process called `koto next`, so what a
check reads depends on the shell that happened to tick it.

The accidental case is the common one. A shell that puts a shim first on
`PATH` -- a stand-in `gh` in an eval fixture, a wrapper a developer installed
last week, a tool manager that rewrites `PATH` per directory -- makes every gate
in that tick read the shim instead of the tool. Nothing says so. The same
session and the same template give one verdict from one terminal and another
from the next, and the difference surfaces, if it surfaces at all, as a gate
that "flaked". On hosts where `/bin/sh` is bash (macOS, Fedora), an exported
shell function goes further: it replaces a tool by name inside every command
koto runs, with no file on disk to find.

The deliberate case follows from the same gap. A template can mark a gate
`overridable: false` so that no evidence and no override record gets past it.
But a caller who changes the environment of a single `koto next` changes what
the gate's command sees, and a one-word `PATH` prefix is enough to make a
failing check pass. A gate the template author declared non-negotiable is,
today, negotiable by anyone who can set an environment variable.

Two things make this harder than "clear the environment". Commands legitimately
need some of it: a gate that calls `gh` needs the caller's token, and a gate
that calls `koto` needs to find the same session store. Those values are often
secrets, and a session's log is plain, append-only and readable, so whatever
fixes the problem can't do it by copying the environment into the session. And
every session in flight today was started by a koto that recorded nothing
about its environment, while its template was written assuming the caller's.

## User Outcome

A developer or agent ticking a session gets the answer the session would give
from any shell. Its gates and actions find the tools they found when the
session started; a shim or an exported function in the ticking shell has no
effect on them, and a verdict that changes does so because the repository
changed, not because the terminal did. When the tools genuinely have moved --
the recorded tool path no longer resolves, or someone tries to attach with a
different one -- koto says so by name rather than quietly running something
else.

A template author who marks a gate non-overridable can rely on it at least as
far as the environment goes: changing the environment of one `koto next` call
no longer changes the verdict. The author knows, from the documentation, which
other channels (files, the session directory, the binaries themselves) remain
open, so the guarantee isn't overstated.

A gate that needs a credential keeps working without anyone thinking about it.
The token is read from the live environment at each run, as it is today, and
nothing about it is written into the session, so an operator can hand a
session log to someone else without auditing it for secrets. And a user who
upgrades koto mid-run finds their existing sessions still advance, with the
change in behaviour announced rather than discovered.

## User Journeys

### Journey 1: An eval author's stand-in tool stops leaking between ticks

An author of a workflow eval puts a stand-in `gh` on `PATH` so the scenario can
run without a network. Today the scenario passes or fails depending on which
shell ticked which state: the harness's shell reads the stand-in, a later tick
from the agent's own shell reads the real `gh`. After the change the stand-in
is part of the session from `koto init` onward, every tick reads it, and a
tick from a different shell reads it too. The eval's result stops depending on
who ticked it.

### Journey 2: An agent tries to talk its way past a non-overridable gate

A coding agent is blocked on a gate the template marks `overridable: false`.
It prefixes `PATH` with a directory holding a script that prints the expected
answer, or, on a bash-`sh` host, exports a function under the tool's name, and
calls `koto next` again. The gate's command runs with the session's recorded
tool path and without the exported function, so it reads the real tool and the
session stays where it was. The agent's options are the legitimate ones the
directive names.

### Journey 3: A developer resumes a session from a shell whose tools have moved

A developer returns to a session a day later in a new terminal. Their tool
manager has since moved a tool, or they are on a machine where the session's
recorded tool path doesn't exist. Instead of a gate silently reading whatever
the new shell finds, or failing with a bare "command not found", koto tells
them which recorded path no longer resolves and what to do about it. A
deliberate change of tools is an explicit step, not a side effect of opening a
terminal.

### Journey 4: A shirabe user upgrades koto with sessions in flight

A user of the shirabe workflows upgrades koto while a `/work-on` run is
half-way through. The run's session was created by the older koto and recorded
nothing about its environment. Its next tick still advances; whatever koto does
for an older session is announced once in the response, and the gates that
call `gh`, `jq`, `git` and `koto` keep finding them. New runs started after the
upgrade get the fixed environment from their first tick.

### Journey 5: An operator shares a session log

An operator sends a failing session's log to a maintainer. The environment
the session recorded is in the log -- a tool path and a list of variable
names -- and none of the values that could hold a credential are. The
maintainer can see exactly which tools the gates ran with without the operator
having to scrub anything first.

## Scope Boundary

### In scope

- **The environment of every command koto runs for a session**: command gates
  and default actions, including polled actions and commands run for child
  sessions, all through the same path.
- **What the session records at init**: the tool search path, and which
  variables its commands may see, recorded so no credential value is ever
  written into session state.
- **Variables read live**: a named set of variables whose values keep coming
  from the ticking process, so credentials and caller-controlled settings
  continue to reach commands.
- **Removing shell-injected behaviour**: exported shell functions and the
  shell start-up hooks that bash and POSIX `sh` read from the environment.
- **Attach and rebind**: what an attaching `koto init` does when the caller's
  tool path differs from the recorded one, including the named refusal.
- **Older sessions**: what a session created before this change does on its
  first tick after the upgrade, and how templates and their main consumer,
  shirabe, move across.
- **Stating the limit**: documentation that says which bypass channel this
  closes and which it doesn't.

### Out of scope

- **File, session-directory and repository tampering.** An agent with a shell
  can edit the files a gate reads, the session's own directory, or the binary
  at the recorded tool path. None of that goes through the environment; this
  work closes one channel and says so rather than claiming gates are
  tamper-proof.
- **`koto next --to` and overrides.** Other ways around a gate have their own
  treatment; this work neither changes them nor depends on them.
- **Sandboxing commands.** No restriction on what a running command can read,
  write or execute beyond the environment it starts with.
- **Snapshotting variable values.** Only the tool path is fixed; the values of
  allow-listed variables stay caller-controlled by design, so a caller who
  changes a live variable still changes what a command sees.
- **Changes to shirabe.** What shirabe's templates and skills need to do is
  proposed from here; the change itself is shirabe's own work.

## References

- `docs/designs/current/DESIGN-koto-runs-commands.md` -- the execution anchor,
  the precedent for recording at init what a session's commands run with, and
  for adopting it on an older session's first tick.
- `docs/designs/current/DESIGN-gate-override-mechanism.md` -- what
  `overridable: false` promises today.
