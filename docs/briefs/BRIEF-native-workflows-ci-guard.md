---
schema: brief/v1
status: Accepted
problem: |
  koto's /workflows rendering rests on Claude Code internals nobody promised to
  keep: the file shape, the per-session directory layout, and the session
  inputs koto uses to find it. When a release changes one, koto's CI stays
  green and an operator meets an empty /workflows with no error to explain it.
outcome: |
  A koto maintainer learns from koto's own CI, before a release, that a Claude
  Code release broke one of these conventions, and the failing check says which
  one. The maintainer can also run a smoke check against a real Claude Code and
  read whether an entry koto wrote actually renders.
---

# BRIEF: a CI guard over the Claude Code conventions koto's /workflows rendering depends on

## Status

Accepted

Framing for the hardening step of koto's native `/workflows` rendering. The
render itself, its phase detail and its blocked status have landed; this
feature is about knowing when Claude Code moves out from under them. Two
questions are deliberately left to the design: what form the guard takes (a
version check, a fixture check against what Claude Code ships, or something
else), and where the rendered smoke check runs.

## Problem Statement

koto shows its sessions in Claude Code's `/workflows` screen by writing a
`koto-<session-uuid>.json` file into a directory Claude Code reads. Every part
of that arrangement is Claude Code's internal behavior, not a published
interface. koto depends on four things: the JSON shape the screen renders, the
layout that puts a session's transcript at
`~/.claude/projects/<slug>/<session-id>.jsonl` with its `workflows/` directory
beside it, the `CLAUDE_CODE_SESSION_ID` environment variable, and the
`session_id`, `transcript_path` and `cwd` fields a SessionStart hook receives on
stdin. What koto knows about them was read from specific Claude Code releases
and checked against the live screen by hand.

Claude Code ships often. When a release renames a field, moves the directory, or
stops setting the variable, nothing in koto notices. koto's existing shape test
compares koto's output against a fixture koto wrote itself, so it keeps passing
no matter what Claude Code does. The hook script that hands the directory to
koto has no test in CI at all, and its fallback path for hosts without `jq`
already emits invalid JSON (koto#243).

The person who finds the break is an operator. They open `/workflows` and see
nothing where a running koto session should be. koto wrote its file without
error; Claude Code just doesn't read it any more. There is no message, and the
operator can't tell whether koto, their setup or Claude Code is at fault. By the
time a maintainer hears about it, the break has shipped.

## User Outcome

A koto maintainer finds out from koto's own CI, before cutting a release, when a
Claude Code release has changed the file shape, the directory convention or the
session input koto relies on. The failing check names which of those changed,
so the maintainer starts from the broken convention rather than from an
operator's report of an empty screen.

When the maintainer wants to know more than "the conventions still hold", they
can run a smoke check against a real Claude Code and read a plain answer: did
the entry koto wrote render, or not. They know how to run it and what a passing
run looks like, because it's written down next to the check.

For operators the result is an absence: they stop meeting a silent, empty
`/workflows` caused by a Claude Code change nobody noticed. A break either never
ships, or ships with the maintainer already aware of it.

## User Journeys

### A Claude Code release changes a convention

A koto maintainer runs koto's CI against a Claude Code release that renamed a
field the screen reads, moved the per-session directory, or dropped a session
input. Trigger: the CI run, on a pull request or on a check against a newer
Claude Code. Outcome shape: a check fails, and its message names the convention
that changed, not just "fixture mismatch".

### Checking that an entry really renders, before a release

The same maintainer is about to cut a koto release and wants to see the rendered
result, not only the inputs it depends on. Trigger: running the documented smoke
check against an installed, real Claude Code. Outcome shape: a pass or fail for
whether the entry koto wrote shows up, plus the record of one run they can
compare against.

### Changing the file shape on purpose

A koto contributor later adds a field to the workflows file koto writes.
Trigger: their change alters the shape koto emits. Outcome shape: the guard's
fixture is extended in the same change by a documented step, so the guard keeps
describing what koto writes and what Claude Code reads, and the new field is
covered from the day it lands.

### The hook that hands koto its directory

An operator on a host without `jq` enables the SessionStart hook script that
derives the workflows directory. Trigger: a Claude Code session start. Outcome
shape: the hook's output is valid JSON Claude Code accepts, and CI would have
failed if it weren't.

## Scope Boundary

**In:**

- A CI check that fails when Claude Code's workflows file shape, its
  per-session directory convention, or the session inputs koto reads change,
  naming which one changed.
- A way to tell koto's CI which Claude Code release the conventions were last
  confirmed against, so a newer release is something the check can compare.
- A rendered smoke check against a real Claude Code that reports whether a
  koto-written entry renders, with instructions for running it and one recorded
  result.
- Test coverage for the session-start hook that derives the workflows
  directory, including its fallback on hosts that lack a JSON tool (koto#243).
- A written procedure for extending the guard's fixture when koto's file shape
  changes on purpose.

**Out:**

- The `/workflows` list view's blocked bucket. It waits on Claude Code, and the
  guard covers what koto relies on today, not what it hopes Claude Code adds.
- Fan-out and hierarchy: coordinators and their delegates rendering as related
  entries. A later step of the same rendering work owns it.
- Retention and crash staleness of the files koto writes (koto#190).
- Changing what koto writes. The guard observes the conventions; it doesn't
  revise the contract or the directory resolution.
- Any dependency between koto and niwa. A koto user never needs niwa for the
  render or for the guard, and the reverse holds too.

## References

- `docs/designs/current/DESIGN-native-workflows-render.md` -- the initial
  render and its file contract.
- `docs/designs/current/DESIGN-native-workflows-phase-detail.md` -- the enriched
  shape and the fixture this guard builds on.
- `docs/designs/current/DESIGN-koto-workflows-blocked-status.md` -- the most
  recent check of the shape against a live Claude Code.
