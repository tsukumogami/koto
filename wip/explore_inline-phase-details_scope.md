# Explore Scope: inline-phase-details

## Visibility

Public

## Entry Assessment

Not run. The source issue (koto#90) carries no `needs-triage` label -- its only
label is `validation:testable` -- so Phase 0.4 does not apply. Phase 4 scores the
research findings without an entry-assessment input.

## Execution Mode

`--auto`. The session was launched with a standing goal and no interactive
partner, so decision points follow the research-first protocol in
`references/decision-protocol.md`: gather evidence from the repo, form a
recommendation, follow it, and record a decision block. Decisions accumulate in
`wip/explore_inline-phase-details_decisions.md`.

Label pre-gate: the issue carries neither `needs-prd` nor `bug`, and `--auto`
mode defaults to not firing the adversarial demand lead. It is not fired.

## Core Question

koto#90 asks that a workflow phase's full instructions ride inline in the `koto
next` response the first time an agent reaches that phase, and be omitted on
every later visit, with a `koto phase-info` escape hatch for when context
compression drops them. The question is whether that behavior is the right
answer to the overhead it targets, and -- if it is -- what shape it takes in
koto's actual template grammar, visit tracking, and response contract, none of
which match the sketch in the issue body.

## Context

Facts established before dispatching research:

- koto is a Rust codebase (~72k LOC across 74 files in `src/`), not the YAML-
  configured system the issue's "Proposed Template Format" implies. Templates
  are markdown files with YAML front-matter; each `##` heading defines a state
  and `**Transitions**` lines define the edges. The issue's proposed
  `analysis: {directive, details}` YAML block has no counterpart in the real
  grammar, so the template-side shape is genuinely open.
- The engine calls them **states**; the `workflows_surface` module and its
  doc family (`BRIEF`/`PRD-native-workflows-phase-detail`) call them **phases**.
  The issue uses "phase". A naming decision is unavoidable and the existing
  `*-phase-detail` docs are about a different surface (the `/workflows` render),
  so the name is already partly claimed.
- State is derived by replaying an append-only JSONL event log; the template's
  SHA-256 hash is locked at init and `next` fails if the compiled template
  changes. Any new template field interacts with that lock.
- `koto rewind` rolls back to a previous state without losing the audit trail,
  and `koto next` can auto-advance through several states in one call
  (`advanced: true`). Both complicate "first visit".
- The issue names #89 as related and describes it as "auto-advance past advanced
  phases". #89 is actually "eliminate double-call pattern for states without
  accepts block" and was closed 2026-03-27. The issue body is stale on this
  point, so its framing of the surrounding work should not be trusted uncritically.
- Existing artifacts that constrain the response shape:
  `docs/prds/PRD-koto-next-output-contract.md` and
  `docs/prds/PRD-unified-koto-next.md`.

## In Scope

- The `koto next` response contract and whether a details payload belongs in it
- Template grammar for attaching long-form instructions to a state
- Visit tracking derived from the existing JSONL log (the issue forbids new state files)
- A retrieval command for on-demand details, and its naming
- Whether "first visit only" is the right conditioning rule, versus alternatives
- Backward compatibility: templates without details, in-flight sessions, template hash
- Downstream skill impact (`koto-author`, `koto-user`) per the repo's maintenance rule

## Out of Scope

- Auto-advance semantics themselves (#89 is closed; changing `advanced` is separate work)
- The `/workflows` render surface and its phase-detail doc family
- Cloud sync, dashboard, batch child spawning, except where a details field touches them
- Rewriting the template format wholesale

## Research Leads

1. **What is koto's real template grammar and `koto next` response shape today, and where could a details payload attach?**
   The issue's proposed format does not exist. Before judging the feature we need
   the actual state-body grammar, the compile pipeline, the `NextResponse` type,
   and what the template-hash lock does to any new field.

2. **Can "first visit to this state" be derived reliably from the existing JSONL event log?**
   The acceptance criteria forbid new state files. Retries, self-loops, `rewind`,
   multi-state auto-advance, resumed sessions, and child workflows each test the
   assumption that a repeat visit means "the agent already has it in context".
   We need to know which of those the log can actually distinguish.

3. **Is the premise real -- do koto-backed skills pay a Read call per phase today, and how large is the payload it would replace?**
   The issue asserts "pure overhead". Check the shipped skills, plugin templates,
   and real workflow templates in this workspace for directives that point at
   files, and size what would move inline.

4. **What would `koto phase-info` be, and does an existing command already cover it?**
   `koto template compile`, session inspection, and the dashboard detail seam all
   read template and state data. A new subcommand needs to justify itself against
   those, and the state-versus-phase naming split has to be resolved.

5. **What are the alternatives to conditioning on first visit, and how do they compare?**
   Always inline; agent-requested via a flag; details-only-by-command; expansion
   keyed on the agent declaring lost context. Compare on token cost, correctness
   after context compaction, response-contract churn, and failure mode when the
   condition guesses wrong.

6. **What do the existing output-contract and unified-next PRDs commit koto to, and what would adding a details field break?**
   Response schema stability, JSONL schema version, template hash locking,
   the `workflows_surface` projection, and existing sessions in flight.

7. **How do comparable agent-orchestration and skill systems deliver long instructions -- inline, on demand, or progressively?**
   Progressive disclosure is a known pattern in agent skill systems and MCP.
   Knowing what shape others landed on, and what failed, informs both the
   conditioning rule and the escape hatch.

## Research Leads -- Round 2

Round 1 established that the feature shipped in PR #109 and that the live
surface is two halves of one defect. Round 2 closes the gaps that decide how
big the work is. See `wip/explore_inline-phase-details_findings.md` for the
round 1 synthesis and `wip/explore_inline-phase-details_decisions.md` (D4) for
why this round runs.

8. **Which of the five suspected defects actually reproduce on current `main`?**
   Four of them -- rewind suppression, respawn and batch-retry inheriting a
   stale visit count, `--to` bypassing the visit check, and auto-advance
   intermediates never surfacing details -- are code-reading inferences. The
   author measured only the gate-blocked re-tick, and on 0.11.4. The count of
   real defects decides whether this is one fix or four.

9. **Can emission counting be recorded without violating R9's "no new state files"?**
   The fix the author suggests turns a delivery into something koto must
   remember. `DESIGN-koto-next-output-contract.md` already rejected a persisted
   counter file on exactly that constraint. Whether an event in the existing log
   satisfies R9, what it costs in schema version, and what the alternatives are
   is the central constraint on the fix.

10. **What does the read-only recovery path actually cost, in each candidate shape?**
    `koto status --details` against a function already holding the compiled
    template, versus a new `phase-info` subcommand, versus a `next --dry-run`.
    Concrete diff footprint, plus the mandatory downstream work koto's CLAUDE.md
    requires: `koto-author`, `koto-user`, their evals, `cli-usage.md`, and the
    error-code envelope enumeration.
