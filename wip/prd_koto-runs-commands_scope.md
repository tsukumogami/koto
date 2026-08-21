# /prd Scope: koto-runs-commands

Scoped non-interactively from the accepted upstream BRIEF
(`docs/briefs/BRIEF-koto-runs-commands.md`) and the `/scope` handoff
(`wip/scope_koto-runs-commands_handoff.md`). The brief settled the framing;
this phase's job was to pick the research leads that turn that framing into
testable requirements, not to re-open it.

## Problem Statement

A koto template author cannot hand the engine a mechanical step and trust it.
`default_action` ships and works, but the command's output is discarded, a
non-zero exit changes nothing, the command runs in whatever directory `koto
next` was typed in, and nothing states which commands qualify. The affected
people are template authors (who write prose telling agents to run shell koto
could run), the agents driving those workflows (who spend turns on
bookkeeping), and the developers watching them (who see a run look healthy
until it isn't). It matters now because the capability is shipped and unused,
and two of the four gaps already bite gate authors before any action is
declared.

## Initial Scope

### In Scope

- Output routing: a command's output reaching later states.
- A failure path: a failing action stopping the run and delivering its own
  output plus the state's written fallback.
- Execution anchoring: binding a session to its tree, refusing elsewhere,
  and a deliberate rebind.
- Two shared-path defects: the pipe-buffer deadlock and koto's per-invocation
  warning volume, both of which reach gates today.
- A durable authoring rule plus real `default_action` documentation.

### Out of Scope

Carried from the brief's out-list unchanged: shirabe's template rewrite,
non-koto skill commands, CI monitoring as a typed integration, template
integrity of any kind, bounding what the event log records, renaming
`requires_confirmation`, runtime containment, routing engine-run commands
through the agent's permission layer, and retiring shirabe's retry-clearing
instructions.

## Research Leads

1. **Output routing and the failure path, current behavior** (codebase
   analyst): where output is captured and dropped, what the `koto next`
   response variants carry, what "a later state reads a value" mechanisms
   already exist, and the exact shape of the gate-less no-detection claim.
   Needed because every output and failure requirement has to name a
   user-visible change against a precisely known baseline.
2. **Anchoring and the two shared-path defects** (architecture and ops
   perspective): what the session record holds today, what the CLI surface
   offers, how the deadlock actually fires, and how the warning volume
   connects to it. Needed to state the anchoring promise accurately —
   overclaiming containment is the identified failure mode — and to write
   defect requirements that cover gates as well as actions.
3. **Authoring guidance and the brief's three open questions** (maintainer
   perspective): where `default_action` is documented, what koto's context
   store actually is, whether GitHub's documentation settles the
   push-to-open-PR loudness claim, and whether any rule statement already
   exists to amend rather than invent.

## Coverage Notes

- The brief leaves three framing questions to this PRD (context-store writes
  as side effects, the migration story for pre-existing sessions, and the
  push-to-open-PR case). Lead 3 gathers the facts; the answers land in
  Decisions and Trade-offs.
- Four approach options exist for output routing and three design questions
  are open on anchoring. `/scope` classified this PRD as complex with a
  DESIGN to follow, so requirements state the user-visible obligation and
  leave the mechanism to that DESIGN.
- Running non-interactively: the checkpoint and the Phase 3 decision
  presentation are made as recorded author calls rather than as questions.
