```yaml
topic: inline-phase-details
chain_started: 2026-08-16T19:06:07Z
chain_completed: 2026-08-16T22:16:29Z
last_updated: 2026-08-16T22:16:29Z
phase_pointer: phase-3
exit: full-run
exit_artifacts:
  - docs/plans/PLAN-inline-phase-details.md
  - docs/designs/DESIGN-inline-phase-details.md
  - docs/prds/PRD-inline-phase-details.md
  - docs/briefs/BRIEF-inline-phase-details.md
visibility: Public
consumed_handoff: wip/scope_inline-phase-details_handoff.md
planned_chain:
  - brief
  - prd
  - design
  - plan
chain_skipped: []
chain_ran:
  - name: brief
    started_at: 2026-08-16T19:08:30Z
  - name: prd
    started_at: 2026-08-16T19:13:00Z
  - name: design
    started_at: 2026-08-16T21:50:00Z
  - name: plan
    started_at: 2026-08-16T22:00:00Z
child_snapshots:
  brief:
    status: Accepted
    content_hash: f51a048de4c85c2021069a01f35870f461f5a4d4
    captured_at: 2026-08-16T19:12:15Z
  prd:
    status: In Progress
    content_hash: aa7659b85169eb971963d76d07c30a270ba525c7
    captured_at: 2026-08-16T21:45:00Z
  design:
    status: Planned
    content_hash: 0a2a9ab1b898312d9250938349501b844f5ff13b
    captured_at: 2026-08-16T22:16:29Z
  plan:
    status: Active
    content_hash: 3bd6fd3f48ed5ba11da38cf7bf38bc7c45cc277b
    captured_at: 2026-08-16T22:16:29Z
consolidation_judgments:
  - hop: brief->prd
    stage: judgment
    verdict: keep
    finding: >-
      The BRIEF holds the per-exclusion reasoning behind its Scope Boundary,
      and the PRD deliberately does not. The PRD's Out of Scope section cites
      the BRIEF for that reasoning rather than restating it, which the Phase 4
      clarity reviewer required so the two documents would not carry two copies
      that drift apart. The absorb procedure composes a contribution section
      from the survivor's own body, and the survivor's body does not contain
      that reasoning, so it could not be composed and the carry check for the
      ancestor's Scope Boundary would fail. Everything else the BRIEF carries
      is already in the PRD at equal or greater fidelity: the Problem Statement
      is a superset, Goals cover the User Outcome, and the User Stories cover
      all four User Journeys plus two more.
  - hop: prd->design
    stage: judgment
    verdict: keep
    finding: >-
      The PRD holds twenty-five numbered requirements and roughly forty
      acceptance criteria. The DESIGN cites them by number throughout -- R4, R6,
      R11, R16, R18 and others appear in its Decision Drivers and its Decision
      Outcome -- but never states them, which is the citation-not-restatement
      rule working as intended. An absorb composes the contribution section from
      the survivor's own body, and the survivor's body does not contain the
      requirement text or any acceptance criterion, so the carry check for the
      ancestor's Requirements and Acceptance Criteria sections would fail.
      Downstream needs them intact as well: the PLAN decomposes against the
      acceptance criteria, and the implementation is verified against them.
  - hop: design->plan
    stage: judgment
    verdict: keep
    finding: >-
      The DESIGN holds four decisions with their considered options and the
      reasoning that rejected each alternative, the security review's ruling on
      template-hash verification, and the solution architecture. The PLAN holds
      acceptance criteria and a sequence, and cites none of that reasoning. The
      lifetime rule settles it independently and decisively: the PLAN is a
      working artifact the implementation cascade deletes, while the DESIGN
      becomes Current and stays as the durable record of how the approach was
      chosen. Absorbing the DESIGN into the PLAN would destroy that record the
      moment the cascade ran, which is the exact failure the lifetime rule
      exists to prevent -- a link, or in this case a whole body, running from
      the longer-lived document into the shorter-lived one.
plan_execution_mode: single-pr
```

## Phase 0 notes

- Slug `inline-phase-details` validated as provided against `^[a-z0-9-]+$`.
- `shirabe slug-prefix-detect inline-phase-details --docs-root docs` returned
  `no-prevailing-prefix`, so no prefix recommendation was surfaced.
- Visibility read from the `## Repo Visibility: Public` header in `CLAUDE.md`.
- No `--upstream` supplied, and `docs/roadmaps/` does not exist in this repo, so
  `consumed_upstream:` is absent.
- No coordination intent: single repo (koto), no `--coordinated` flag, and no
  `## PR Grouping Policy:` or `## Reviewability Ceiling:` header in CLAUDE.md.
- No prior state file, so the `parent_orchestration:` self-heal had nothing to
  clear.
- Execution mode: `--auto` semantics. The session carries a standing goal and no
  interactive partner, so decision points follow the research-first protocol --
  gather evidence, follow the recommendation, record it -- rather than blocking.
  This does not suppress the R9 hard-finalization check.

## Phase 1 notes

Slot 7 fired: `wip/scope_inline-phase-details_handoff.md` was on disk from an
`/explore` run against koto#90 and was consumed as discovery input. The
cold-start projection is therefore suppressed.

**Framing-shift answer: yes.** Confirmed against the handoff's pre-supplied
answer rather than asked fresh. The handoff's evidence: the exploration's problem
statement inverted twice -- first when round 1 established the feature had shipped
in PR #109 (so the question became "what is wrong" rather than "what should we
build"), and again when round 2 measured `koto next --full` advancing a workflow,
which promoted the read-only recovery path from an optional escape hatch to the
thing that makes any suppression rule safe. The success criterion moved with it,
from "reduce token overhead" to "never withhold instructions from an agent that
lacks them, without re-sending to one that has them."

**Child-doc discovery.** All five canonical paths globbed; none exists. No
re-entry protection fires, `chain_skipped:` is empty, and `child_snapshots:` is
absent because there was no pre-existing durable artifact to snapshot.

**R6 shape-predicate walk.** P1 and P3 accept the handoff's estimate with its
stated reasons; P2 was recomputed against the tree per the Slot 7 clause.

- **P1 -- architectural-alternatives count: fires.** The handoff leaves five
  alternatives explicitly unsettled: what records a delivery (a new
  `EventPayload` variant, an `EvidenceSubmitted` on the reserved-kind
  convention, or an additive `StateFileHeader` field); where the read-only
  recovery lives (`koto status` extension, a new subcommand, or `next
  --dry-run`); what that command is called; where the boundary-aware rewind and
  respawn checks live given `derive_visit_counts` is shared; and how an agent
  discovers the recovery path.
- **P2 -- new-component references: does not fire.** Recomputed against the
  repository. Every candidate lands in an existing module -- `src/cli/` for the
  command surface, `src/engine/` for the event and header work, `src/template/`
  for the marker. Nothing names a new binary, crate, service, or runtime
  substrate.
- **P3 -- Complex classification: fires.** The handoff names contested
  trade-offs requiring settlement rather than research, a sequencing constraint
  from two defects sharing one predicate, an amendment to an already-Done PRD
  requirement (R9 of `PRD-koto-next-output-contract.md`), and mandatory skill
  and eval work triggered by koto's CLAUDE.md rule for changes under `src/cli/`
  and `src/engine/`.

**Pre-authoring upstream notice: fired.** `/brief` will run and Phase 0 recorded
no `consumed_upstream:`, so both conditions held. Emitted verbatim in the chain
proposal.

**Chain proposal: Proceed.** Recorded under `--auto` semantics; the proposal was
emitted with its `Proceed / Adjust / Bail` options and the run followed the
recommendation.
