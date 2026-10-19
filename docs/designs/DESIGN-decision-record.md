---
status: Proposed
problem: |
  koto has no durable, cross-session record of the decisions dispatched
  sessions make. Decision-like events live in session logs that are deleted
  on success, carry no actor, and can't be overruled from another session.
decision: |
  To be settled in this design.
rationale: |
  To be settled in this design.
upstream: docs/prds/PRD-decision-record.md
---

# DESIGN: The decision record

## Status

Proposed

## Context and Problem Statement

koto today keeps everything about a workflow inside that workflow's session
directory under `~/.koto/sessions/<name>/`: the append-only session log, the
context keys and the run journal. A session is removed when it reaches a
successful terminal unless `--no-cleanup` is passed, so anything recorded
there is gone by the time a coordinator in another session wants to question
it. The PRD asks for the opposite: every decision a dispatched session makes
or receives must stay readable after the session is removed and its
workspace reclaimed (R17), and any session on the machine must be able to
record and read decisions, including ones about another session's workflow
(R18).

The decision-like events koto already writes don't carry what the PRD needs.
`GateOverrideRecorded` and `DecisionRecorded` keep a rationale and no actor;
`DeciderCheckAnswered` implies the agent; the run journal's `check_overridden`
names a gate and nobody. None has a stable identifier another session could
point at, and none can be reversed: an override lasts for one visit and a
later re-evaluation supersedes it. The PRD needs each decision to carry one
of nine kinds (R1) with a subject a reader can resolve later (R2), an
identifier, a UTC time, an outcome and an optional list of decisions it
relies on (R3).

Who decided is three-way (R4): a session plus the agent within it (`main`,
an agent the main agent started, or `unknown`), a person named by a handle
and the surface it came through (`terminal`, `forge`, `relay`) along with the
session that entered it, or an automated check plus the session that ran it.
koto stores a person's handle as an opaque string and has no account concept
(R5). Every identity is labelled `observed` (koto read it from its
environment or the workspace manager's lineage) or `handed` (the caller
supplied it), and both values are kept when they disagree (R6). koto already
reads `CLAUDE_CODE_SESSION_ID` into the run journal as the driver, which is
the one identity it observes today. It cannot tell a sub-agent's invocation
from the main agent's from its environment: only harness hooks see the agent
identifier.

Authority (R9) is what the decider was dispatched with: an authority kind
(`dispatch` or `person`), the dispatch identifier, a content digest of the
instructions handed down, the run mode and the immediate dispatcher, plus,
on reversals, a `verified` or `asserted` standing. The dispatch chain (R10)
runs from the deciding session to a root that a source affirms; missing
lineage is `incomplete`, never a root. The workspace manager doesn't read or
write workflow state and doesn't run koto (R11). Today it publishes lineage
only in two places: each worker's `OTEL_RESOURCE_ATTRIBUTES`
(`niwa.dispatch.id`, `niwa.parent.session.id`) and its per-instance session
mapping, which it deletes at reap. A check run inside a dispatched worker
for this design found neither present, so neither can be relied on in every
launch path as things stand.

Overruling (R12 to R15) is a new decision that points at the original. The
original's chain alone decides who may reverse it; a reversal is final; a
reversed gate pass must stop a still-live workflow from advancing until it is
rewound or the gate is re-passed by someone with standing. Every gate pass by
choice (`koto overrides record`, `koto next --to` past an unmet gate, `koto
decider answer` with a passing answer) must record a `gate-pass` decision;
an unnamed decider defaults to the observed session and is marked
`defaulted`, and a pass with no establishable decider is refused without
writing the command's existing records (R16). The existing commands keep
writing what they write today and also write the decision, atomically (R22,
R26).

Reading (R19, R20) is by session (made or received), subject, kind, decider
kind and time window, ordered oldest first, plus a report that cross-tabulates
decisions made in a window, and reversals recorded in it, by kind and decider
kind. A record of 100,000 decisions over a year must answer a month's listing
and report in under five seconds (R29). Recording never touches the network
(R28), reasons never reach exported telemetry (R27), writes are append-only
and safe under concurrent writers (R25), and earlier decisions can be
imported idempotently (R24).

koto's request store at `~/.koto/requests/<id>/` is the nearest precedent:
append-only, lock-guarded, written by several sessions, keyed by something
other than a session, and kept after sessions end. It has no index and no
pruning.

## Decision Drivers

- **Outlive the session (R17).** The store can't live under a session
  directory or be removed by any session or workspace cleanup.
- **Cross-session access (R18).** Any session on the host writes and reads,
  under concurrent writers, without losing or corrupting an entry (R25).
- **Query and report speed (R29).** A month out of a year of 100,000
  decisions in under five seconds, and reversal status for any decision
  without a full scan.
- **No dependency on the workspace manager calling koto (R11, R28).** Lineage
  is read from what the workspace manager already keeps, or handed by the
  dispatcher, and recording works when neither is available.
- **Backward compatibility (R26).** Existing events, run-journal readers and
  feed stay unchanged; existing commands keep their arguments.
- **Honest identities (R6, R10).** Every identity says where it came from;
  nothing is guessed.
- **Engine enforcement only where koto can act (R15).** A reversed gate pass
  blocks a live workflow; nothing outside koto is changed.
- **Consistency with koto's conventions.** Atomic writes, append-only logs,
  the name-resolution check over user-facing docs, and the koto-skills
  plugin kept in sync with any CLI change.
