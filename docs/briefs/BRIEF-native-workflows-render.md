---
schema: brief/v1
status: Done
problem: |
  When an operator drives a koto workflow inside Claude Code, the workflow's
  live state is invisible in the surface the operator is already watching.
  Claude Code's `/workflows` screen renders its own multi-agent runs, but a
  koto session shows up nowhere there -- to see workflow state the operator
  must leave the TUI and open koto's separate dashboard. One session, two
  surfaces, two attention contexts.
outcome: |
  The operator drives a koto session inside Claude Code, opens `/workflows`,
  and sees that session as a native entry -- its name and current state, with
  no separate command, skill, or window. The entry reflects where the session
  is and refreshes to the newer state when the operator reopens the screen,
  and reads done rather than a stuck spinner once the session finishes.
---

# BRIEF: koto sessions render natively in Claude Code's `/workflows`

## Status

Done

Framing for the walking-skeleton slice of koto's native Claude Code
`/workflows` rendering. The surface decision is settled: koto produces the
artifacts Claude Code's `/workflows` renders natively -- there is no koto
skill, reader, or parallel surface. This brief captures that framing so the
downstream PRD and DESIGN pin the mechanism. The requirements (what to build)
belong to `PRD-native-workflows-render`; the mechanism (how) belongs to
`DESIGN-native-workflows-render`.

## Problem Statement

An operator running long-form agentic work inside Claude Code watches the
agent in the TUI, including `/workflows`, which renders multi-agent runs as a
tree of phases and agents with their state. When that work is driven through a
koto workflow, the workflow's state is not among those entries. To answer
"where is my workflow right now?" the operator has to leave the surface they
are already in and open koto's dashboard -- a separate window, a separate
attention context, for the same session.

The problem is the split, not the absence of a viewer: koto already renders
this state richly in its own dashboard. What is missing is legibility *where
the operator already is*. The cost of the split is a per-glance context switch
that the local-first, single-surface thesis says should not exist.

This brief scopes only the thinnest end-to-end slice that closes the split for
one session: proving the whole path (a hosting hook makes the location known,
koto writes its state there as it advances, `/workflows` renders it) works at
all, before richer detail, hierarchies, hardening, and file lifecycle are
built on top of it.

## User Outcome

An operator drives a single koto session inside a Claude Code session, opens
`/workflows`, and finds that session listed as a native entry -- identified by
its name and showing its current state -- with no command to run, no skill to
invoke, and no second window to open. As the session advances and the operator
reopens `/workflows`, the entry shows the newer state. When the session
finishes, the entry reads *done*, not a permanently-spinning *running*. If the
operator is not inside a participating Claude Code session, nothing changes:
koto behaves exactly as it does today and `/workflows` is unaffected.

## User Journeys

### Drive a koto session and see it appear

An operator, working inside a Claude Code session that has the koto agent
surface enabled, starts a koto workflow and advances it. Trigger: the session
commits its first state. Outcome shape: opening `/workflows` shows an entry for
that koto session, labeled by its name, showing the state it is currently in.

### Advance the session and see the entry move

The same operator advances the session another step or two. Trigger: each
state-commit. Outcome shape: reopening `/workflows` shows the entry now at the
newer state -- the display is current as of the session's last advance.

### Finish the session and see it settle

The operator drives the session to completion. Trigger: the session reaches a
terminal state. Outcome shape: the `/workflows` entry reads *done* (or
*failed* for a failure-terminal), not a stuck *running* spinner.

### Run koto with no participating host

An operator runs koto in a plain terminal, or in a Claude Code session that
has not enabled the surface. Trigger: any state-commit. Outcome shape: koto
writes no `/workflows` artifact, its default behavior and cost are unchanged,
and any existing `/workflows` screen is untouched.

## Scope Boundary

**In:**

- One non-hierarchical koto session rendering as a single native `/workflows`
  entry, driven inside one Claude Code session.
- A minimal projection: session name, current state, and running/done status.
- The shared foundation the later features build on: an *extensible*
  `/workflows` file contract (a minimal valid shape later slices add fields
  to), the single commit-funnel hook point that materializes on every
  state-commit, and a context-store publish/discover mechanism whose key
  schema already admits Feature 3's nearest-published-ancestor walk.
- Creating the target `/workflows` directory when it is absent.
- Opt-in by the presence of a published location (koto's default path is
  untouched when no location is published).

**Out:**

- Richer per-phase / per-agent detail in the entry (Feature 2).
- Hierarchies -- coordinator and delegates each rendering (Feature 3).
- Hardening against the undocumented surface: the version/fixture guard, the
  rendered smoke check, and atomic-write-as-hardening (Feature 4).
- File lifecycle: retention/rotation and crash-staleness reconciliation
  (Feature 5).
- Any koto skill, reader, MCP server, or parallel surface (settled out of
  scope).
- Re-deciding the settled surface decision.

## References

- `PRD-native-workflows-render` -- the requirements derived from this brief.
- `DESIGN-native-workflows-render` -- the mechanism that satisfies them.
