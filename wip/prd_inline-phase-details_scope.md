# PRD Scope: inline-phase-details

## Context

- Input Mode 2: `docs/briefs/BRIEF-inline-phase-details.md`, already `Accepted`,
  so Phase 0's brief transition is a no-op.
- Under `/scope`'s `parent_orchestration:` sentinel, `rationale: fresh-chain`.
  Phase 4's jury takes the serial-self-jury / parallel-peer shape as available;
  Phase 5 finalization takes parent-delegated-approval.
- Visibility: Public. Execution mode: `--auto` semantics.
- Upstream framing is settled by the BRIEF. This scope covers only what the
  requirements need that the BRIEF deliberately did not decide.

## Problem Statement (carried from the BRIEF, not restated)

koto decides whether to send a phase's long-form instructions by counting
entries into a state rather than deliveries of its instructions. The rule
therefore re-sends to an agent that is standing still, withholds from one told
to redo a step, and is bypassed entirely on a directed transition -- and no
log-derived rule can do better, because the log records nothing about who is
attached to the session. There is no read-only way to get the instructions back.

## What the Requirements Need That the Exploration Did Not Settle

The exploration answered the technical landscape thoroughly. Three things remain
open specifically at requirements altitude, and each one changes what the
acceptance criteria say.

1. **The complete behavioral matrix.** The exploration measured five paths.
   There are more paths through `koto next` that can carry the instructions
   field, and at least one -- the accepts-fallthrough where a gated state with
   an `accepts` block returns evidence-required rather than gate-blocked -- was
   explicitly not measured. Acceptance criteria that cover five of eight paths
   would ship a partial fix and call it done.

2. **What the recovery call must return and must provably not do.** The BRIEF
   says "read-only, keyed by the workflow name, changes no state, triggers no
   side effect". Turning that into testable criteria needs the exact payload and
   the exact enumerated non-effects, stated so a reviewer can verify each one.

3. **The mandatory downstream surface.** koto's contributor guide requires that
   changes under `src/cli/` and `src/engine/` be assessed against both shipped
   skills, and CI enforces that every skill has an eval. Requirements that omit
   this leave a merge-blocking obligation to be discovered during
   implementation.

## Research Leads

1. **What is the complete set of response paths that can carry the instructions
   field, and what is the correct behavior on each?**
   Enumerate every code path that constructs a response with the field, not just
   the ones already measured. For each, state the correct behavior under the
   BRIEF's outcome and whether today's behavior matches.

2. **What must the read-only recovery return, and what must it provably not do?**
   Derive the payload from what an agent that has lost its context actually
   needs, and derive the non-effects from what the existing non-read path
   actually does. Both must be stated as things a reviewer can check.

3. **What downstream work does koto's own contributor contract make mandatory,
   and what does CI enforce?**
   Name the specific files, evals, tests, and documentation surfaces, and which
   of them are merge gates rather than conventions.
