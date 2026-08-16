```yaml
topic: inline-phase-details
chain_started: 2026-08-16T19:06:07Z
last_updated: 2026-08-16T19:08:00Z
phase_pointer: phase-2
exit: UNSET
exit_artifacts: []
visibility: Public
consumed_handoff: wip/scope_inline-phase-details_handoff.md
planned_chain:
  - brief
  - prd
  - design
  - plan
chain_skipped: []
chain_ran: []
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
