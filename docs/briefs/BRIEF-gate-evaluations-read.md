---
schema: brief/v1
status: Accepted
problem: |
  No read-only koto verb prints a session's gate evaluations, so a program
  that counts how often the same gate fails on the same rule across sessions
  has to parse koto's files itself, and no verb reads another workspace's
  gate evaluations either.
outcome: |
  A read-only way to list a session's gate evaluations with, for each one,
  the gate name, the outcome, the rule ids of its findings, the template,
  and the time.
---

# BRIEF: Reading a session's gate evaluations

## Status

Accepted

This brief frames the problem in koto#688 for the design that answers it,
`docs/designs/DESIGN-gate-evaluations-read.md`. It restates the issue and
adds no requirement of its own: the need is the issue's, and how it's met
is the design's question.

## Problem Statement

No read-only koto verb prints a session's gate evaluations: the gate name,
the rule id of each finding, the template, and the time of each evaluation.
`koto status`, `koto overrides list`, `koto dashboard <session> --detail`,
`koto decisions list` and `koto context list` print none of them.

The data lives in files, not verbs. Each `gate_evaluated` event in the
session's event log carries the gate, the outcome, the timestamp and the
findings' rule ids, with the template named only in the session header. The
per-host run journal holds the same evaluations as check records.

So a program that wants to count how often the same gate fails on the same
rule across sessions has to parse koto's files itself. `--workspace`
accepts only `status`, `context get`, `context exists`, `context list`,
`decisions list`, `decisions show` and `overrides list`, so no verb reads
another workspace's gate evaluations either.

## User Outcome

A read-only way to list a session's gate evaluations with, for each one,
the gate name, the outcome, the rule ids of its findings, the template, and
the time.

## User Journeys

A program counts how often the same gate fails on the same rule across
sessions. It lists each session's gate evaluations through koto rather than
parsing koto's files.

## Scope Boundary

In scope: a read-only way to list a session's gate evaluations with the
fields above.

Out of scope: how it's exposed, which is the design's question and not
settled here.

## References

- koto#688, the issue this brief restates.
- `docs/designs/DESIGN-gate-evaluations-read.md`, the design that answers it.
