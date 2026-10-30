---
schema: brief/v1
status: Accepted
problem: |
  A developer who runs koto sessions on several machines can't see them
  together. Each session already syncs to the developer's own bucket, but
  the dashboard on one machine shows another machine's session as a
  placeholder with no state, and no session says which machine produced it.
outcome: |
  From any one of their machines, a developer opens the dashboard or reads
  its text output and sees the sessions they run elsewhere, each with its
  current state and the machine it runs on, read from their own bucket
  with no managed service in between.
---

# BRIEF: Sessions from every machine in one dashboard

## Status

Accepted

Framing for reading sessions across a developer's machines through the
S3-compatible bucket koto already syncs to. The bucket mechanics it rests
on (sync to a user-configured bucket, pins and successors kept across
hosts, reads of a named session from any directory) already ship; this
brief frames what the dashboard does with them. The downstream PRD owns the
requirements and settles the questions the brief left open: the credential
model, how sessions from other machines are found, how the producing
machine is stamped, and the text output's columns.

## Problem Statement

Some developers run coding agents on more than one machine at once: a
laptop for interactive work, a workstation or a cloud VM for long runs,
sometimes a second VM to keep a run going while the first is busy. koto
already lets each of those machines sync its sessions to an S3-compatible
bucket the developer configures, and over the last several releases that
bucket has become a record the machines share. A pin set on one host keeps
the session on every host, a successor is followed through the bucket, and
a named session is read at its own copy from any directory.

None of that reaches the place a developer looks first. On any one machine,
`koto dashboard` answers "which session needs me now?" only for the sessions
that machine runs. A session that exists only in the bucket shows up as a
placeholder row with no state: the developer can see that something is
there, but not whether it's advancing, blocked at a gate, or finished. And
no row says which machine a session came from. The `origin` koto records
today names the directory a session is anchored in and the store it writes
to, which is enough to locate a copy but not to tell a developer "this one
is on the build VM".

So the developer goes and looks. They SSH into the other machine, or open a
terminal there, and run the dashboard locally, once per machine, to find the
one run that's waiting on them. The more machines they use, the more often
the session that needs them is the one they can't see, which is exactly the
failure the dashboard was built to prevent on a single machine.

## User Outcome

A developer working across several machines stops having to visit each one
to learn what their sessions are doing. From whichever machine is in front
of them, the dashboard shows the sessions they run on all of them, with the
same attention ordering it already gives local sessions: a session blocked
at a gate on the build VM ranks next to one blocked on the laptop, and each
row names the machine it runs on, so the developer knows where to go to act
on it.

The same picture is available as text, so a developer in a terminal, or a
script watching for blocked sessions, reads the same sessions, states and
machines without opening the interactive view.

All of it comes from the bucket the developer already set up. There's no
account to create, no service to run, and no data leaving storage the
developer controls; a developer on one machine with no bucket sees the
dashboard exactly as it is today.

## User Journeys

### Check on a run from another machine

A developer starts a long session on their workstation and moves to their
laptop. The session reaches a gate and stops. On the laptop they open
`koto dashboard` and see the session near the top of the list, shown as
blocked at that gate, with the workstation named as the machine it runs on.
They know it needs them and where to go to answer it, without having opened
a terminal on the workstation.

### Read the same view as text

A developer keeps a small shell loop on their laptop that warns them when
any session, on any machine, is waiting at a gate. The loop calls
`koto dashboard --once` every few minutes. When a run on a cloud VM stops at
a gate, the loop's next pass finds it in the text output with the same
state and the same producing machine the interactive view would show, in
columns a script can parse and a person can scan, and the warning names the
VM. Nobody had to open the TUI or log into the VM to notice.

### Keep working on one machine

A developer who uses koto on one machine, with the local backend and no
bucket, opens the dashboard after upgrading. Nothing about their view
changes: no new prompts, no empty machine column demanding setup, no
network calls. Cross-machine reading is something the bucket enables, not
something every user has to configure around.

## Scope Boundary

**IN:**
- The dashboard and its text output (`koto dashboard --once`) showing
  sessions another machine wrote to the developer's bucket, with their
  current state rather than a placeholder.
- Each session naming the machine that produced it, recorded when the
  session is written rather than guessed when it's read.
- The way the dashboard finds sessions other machines wrote to the bucket.
- The credentials the reading machine uses, within the user-level bucket
  configuration koto already has.
- The text output's columns, including how the producing machine appears.

**OUT:**
- A hosted or managed service of any kind, including a shared web
  dashboard; everything is read from the developer's own bucket.
- Acting on another machine's session from the dashboard (advancing it,
  answering its gate, cancelling it). This feature makes the session
  visible; acting on it stays on the machine that runs it.
- Requests, review threads, messages or external signals in the dashboard.
  The dashboard renders sessions.
- Session continuity mechanics in the bucket: pins, successors, imports,
  prune rules and shared-bucket reads. Those already exist and are built
  on, not changed; anything new this feature needs from them is named as a
  dependency rather than built here.
- Project-level session-storage settings. This feature reads the bucket
  configuration koto already resolves at user level.
- Several developers sharing one bucket. The reader is one developer
  looking at their own machines.
