# Structural-Format Review: DESIGN-orphaned-session-detection.md

Reviewed against: `skills/design/references/design-format.md` (shirabe 0.15.1-dev),
cross-checked against koto's own `docs/designs/current/` corpus for local
convention (`DESIGN-batch-child-spawning.md`, `DESIGN-hierarchical-workflows.md`,
`DESIGN-visual-workflow-preview.md`, `DESIGN-session-legibility.md`,
`DESIGN-gate-override-mechanism.md`).

## 1. Section presence and order

All nine required sections appear, in canonical order:

Status (26) -> Context and Problem Statement (30) -> Decision Drivers (81) ->
[**Decisions Already Made** (112), extra] -> Considered Options (148) ->
Decision Outcome (353) -> Solution Architecture (421) -> Implementation
Approach (555) -> Security Considerations (621) -> Consequences (661).

Verdict: PASS. The one non-canonical heading, "Decisions Already Made," is
not one of the nine required sections nor one of the three context-aware
sections (Market Context / Required Tactical Designs / Upstream Design
Reference) named in the reference. However it is an established koto-repo
local convention, not an ad hoc addition: the identical heading (or
"Decisions already made") appears in `DESIGN-hierarchical-workflows.md`,
`DESIGN-visual-workflow-preview.md`, `DESIGN-koto-template-authoring-skill.md`,
and `DESIGN-template-variable-substitution.md`, always positioned between
Decision Drivers and Considered Options, always used the same way -- to
record decisions settled without an interactive user present (`--auto` mode)
that don't warrant a full Considered-Options bakeoff. Other current docs use
comparable repo-local extra sections in the same spirit (`Foundational
choices` in batch-child-spawning, `References` in native-workflows-render/
phase-detail). Extra sections interleaved between required ones do not
violate FC15, which checks relative order of the nine required headings, not
absence of other headings. No violation.

## 2. Frontmatter field order

Current frontmatter:
```yaml
---
status: Proposed
problem: |
  ...
---
```

`status` precedes `problem`, matching the canonical order (`schema, status,
problem, decision, rationale`). `problem` uses the correct YAML literal
block scalar (`|`) shape, and its content is one cohesive paragraph
describing the technical gap, consistent with the reference's guidance.

`decision` and `rationale` are absent by design at this phase -- per the
review brief, this is expected (Phase 6 finalization adds them) and is not
flagged as an error. Body-side check: the `## Status` section's first
non-blank line is the bare word `Proposed`, matching the frontmatter
`status` value case-insensitively (FC03-conformant).

One incidental observation, not a flag against this review's four items but
worth noting: the frontmatter also omits `schema: design/v1`. The reference
lists `schema` as a required field, but koto's own `current/` corpus is
mixed on this -- some docs (`DESIGN-session-legibility.md`,
`DESIGN-native-workflows-render.md`) carry it, others (`DESIGN-batch-child-
spawning.md`, `DESIGN-gate-override-mechanism.md`) don't. Since the review
brief scopes "the four required fields" to status/problem/decision/
rationale only, this is reported as a minor observation rather than a
finding, and should resolve naturally once `decision`/`rationale` are
finalized in the same Phase 6 pass (or be added explicitly if this repo has
since made `schema` mandatory going forward).

Verdict: PASS (status/problem correctly shaped and ordered; decision/
rationale absence is expected-at-this-phase per brief).

## 3. Section-altitude conformance

Each section stays at design/architecture altitude:

- **Context and Problem Statement**: states a technical gap (three call
  sites that never check `template_source_dir`) and cites the originating
  issue; does not smuggle a solution into the problem framing.
- **Decision Drivers**: five real constraints (reuse, no incorrect coupling,
  read-only scope, cross-machine correctness, naming/collision avoidance),
  not generic best-practice filler.
- **Considered Options**: two decisions, each with a chosen option and two
  alternatives with genuine rejection depth tied back to the drivers (no
  strawmen -- e.g. the "direct reuse" alternative's rejection cites a
  concrete wire-format-drift consequence, not a vague dismissal).
- **Decision Outcome / Solution Architecture / Implementation Approach**:
  names concrete types, file paths, function signatures, and a five-phase
  rollout with per-phase deliverables. Implementation Approach stays at
  phase/batch granularity ("Phase 3: SessionInfo and both list() backends")
  -- it does not decompose into atomic, assignable issues (no issue
  numbers, no per-issue acceptance criteria, no estimate/owner fields),
  which is the PLAN's job, not the DESIGN's. No PLAN-altitude drift.
- **Security Considerations / Consequences**: attack-surface and trade-off
  analysis stays at the same altitude as the rest -- no new requirements
  are introduced anywhere (issue #189 and the existing `StaleTemplateSource
  Dir` precedent are cited as motivation, not restated as new R-numbered
  requirements), consistent with content-boundary guidance that a DESIGN
  cites requirements but does not introduce them.

Verdict: PASS. No PRD-altitude requirement articulation, no PLAN-altitude
atomic issue decomposition found anywhere in the document.

## 4. R19 budget-vs-spec (section-length heuristic)

The design-format reference does not state fixed numeric budgets per
section, and this document contains no self-declared budget claims
("approximately N lines" or similar) to check against -- per the R19
mechanism as implemented elsewhere (natural-language budget-claim parsing),
there is nothing to overshoot against in this document.

As a proportion-based heuristic instead, approximate section sizes (702
total lines):

| Section | Lines | Share |
|---|---|---|
| Status | 4 | 0.6% |
| Context and Problem Statement | 50 | 7% |
| Decision Drivers | 30 | 4% |
| Decisions Already Made | 35 | 5% |
| Considered Options | 204 | 29% |
| Decision Outcome | 67 | 10% |
| Solution Architecture | 133 | 19% |
| Implementation Approach | 65 | 9% |
| Security Considerations | 39 | 6% |
| Consequences | 41 | 6% |

Considered Options (~204 lines) is the largest section by a wide margin, but
this tracks its content, not bloat: it carries two full decision bakeoffs
(signal shape/wiring, and backend differentiation), each with a chosen
option and two alternatives with genuine depth -- exactly what the Quality
Guidance for this section asks for ("each rejected alternative has genuine
depth"). Solution Architecture (~133 lines) is the second largest, again
proportionate to a seven-component change touching five existing files plus
one new module. No section reads as padded relative to its job, and none
shows symptoms of altitude drift (e.g., Considered Options does not balloon
with PRD-style requirement listing; Implementation Approach does not
balloon with PLAN-style issue enumeration).

Verdict: PASS. No overshoot to flag; no section's length signals it belongs
at a different altitude.

## Hygiene check: wip/ references

```
git grep -nE 'wip/' -- docs/designs/DESIGN-orphaned-session-detection.md
```

Two hits, both on adjacent lines in the "Decisions Already Made" section:

```
141:See `wip/explore_orphaned-session-detection_findings.md` and
142:`wip/research/explore_orphaned-session-detection_r1_lead-*.md` for the full
```

Both are the single known "see wip/... for full exploration" pointer called
out in the review brief as expected/fine at this stage (Phase 5/6 cleanup
removes it before merge, per the workspace's wip-hygiene convention and the
design skill's R25 carve-out for path-shaped references that are expected to
be scrubbed downstream, not the rule-statement-prose carve-out). No other
`wip/` references exist anywhere else in the document -- no frontmatter
`upstream:` pointing at `wip/`, no other prose or code-comment references.

Verdict: PASS. No hygiene violation beyond the known, expected pointer.

## Summary

All four structural-format checks pass. The one non-canonical section
("Decisions Already Made") is a recognized koto-repo convention, not a
violation. Frontmatter status/problem are correctly shaped and ordered;
decision/rationale absence is expected at this phase. No altitude drift
toward PRD or PLAN content. No section-length overshoot to flag. The
`wip/` hygiene grep found only the single expected exploration pointer.
