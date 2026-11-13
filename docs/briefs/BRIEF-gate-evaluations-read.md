---
schema: brief/v1
status: Accepted
problem: |
  No read-only koto verb prints a session's gate evaluations, so a program
  that counts how often the same gate fails on the same rule across sessions
  has to parse koto's state files itself, and has no answer at all for a
  session in another workspace.
outcome: |
  A program lists a session's gate evaluations through a koto verb, with the
  gate name, outcome, rule ids, template and time of each, and reads a
  session in another workspace the way the other read verbs do, without
  knowing koto's file layouts.
---

# BRIEF: Reading a session's gate evaluations

## Status

Accepted

This brief frames the problem in koto#688 for the design that answers it,
`docs/designs/DESIGN-gate-evaluations-read.md`. It restates the issue and
adds no requirement of its own: the need is the issue's, and the open
questions are all about how to meet it.

## Problem Statement

koto evaluates a state's gates on every `koto next` and records each
evaluation: the session's event log gets a `gate_evaluated` event with the
gate, the outcome, the time and the findings' rule ids, and the run journal
gets matching check records. Neither is reachable through a verb. `koto
status`, `koto dashboard <session> --detail`, `koto overrides list`, `koto
decisions list` and `koto context list` show other things.

So a program that wants to know which gates failed in a session, and on
which rules, parses koto's files. The case that raised this is a sweep that
counts repeats of the same gate failing on the same rule across many
sessions. It also can't reach a session in another workspace, because
`--workspace` serves only the read verbs it lists.

## User Outcome

A program asks koto for a session's gate evaluations and gets, for each
one, the gate name, the outcome, the rule ids of its findings, the template
and the time, in a documented shape. It can group and count them without
reading a state file, and it gets the same answer for a session in another
workspace that the other read verbs give.

## User Journeys

A maintainer runs a sweep over the sessions a store holds. For each
session it lists the gate evaluations, keeps the failed ones, and counts
(template, gate, rule) triples, so a rule that keeps firing stands out. When
a session lives in another workspace, the sweep asks for it with
`--workspace` and gets the same records.

## Scope Boundary

In scope: a read-only way to list one session's gate evaluations with the
fields above, and how it behaves for a session in another workspace.

Out of scope: the sweep itself, how its counts are stored or shown, and
evaluations of sessions koto has already removed.

## References

- koto#688, the issue this brief restates.
- `docs/designs/DESIGN-gate-evaluations-read.md`, the design that answers it.
