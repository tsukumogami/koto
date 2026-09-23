# Crystallize Decision: jev-decision-offload

## Chosen Type
/scope

## Candidacy
- /execute: not a candidate. There's no `docs/plans/PLAN-*.md` and no `schema: plan/v1` file in koto.
- Competitive analysis: not a candidate (public).

## Rationale
The exploration converged on one feature: typed decisions, declared on
`accepts` fields and answered by a pluggable decider inside `koto next`, with a
shadow-first rollout and a compiler-enforced floor. Shirabe's reshaped states
are the first consumers. What to build is clear. How to build it has open
architecture questions (declaration placement, per-item question sets,
aggregation, ledger and replay storage) and open requirements (what the agent
sees on fallback, promotion bars, which decisions go first). Round 1 made
several decisions that need a durable home.

## Stage 1 Evidence
### Signals Present
- Converged on something someone will build: the decider in the advance loop plus shirabe reshaping.
- Requirements and architecture questions remain open: see findings Gaps and Accumulated Understanding.
- Decisions need a durable home and downstream work: posture, authority floor, no-key parity, generic interface, surface.
- Multiple stakeholders: koto engine and shirabe skill maintainers.
- A scope boundary emerged: in-koto decider, shadow first, human and irreversible gates excluded, deterministic facts moved to gates.
- Core question is "what do we build, and how?"

### Anti-Signals Checked
- Nothing left to build: not present.
- Whole output is one choice: not present (posture is one of several decisions).
- Feasibility verdict nobody acts on: not present.
- Findings center on external products: not present.
- Conclusion is the work should not happen: not present. The devil's advocate case narrowed the posture rather than rejecting the work.

### Ranking
- A chain: 6
- Decision Record: 1 (demoted: multiple interrelated decisions with work attached)
- Spike Report: 0 (demoted: question is "what to build", exploration was broad)
- Rejection Record: 0

## Stage 2 Evidence
### Signals Present
- /scope: single coherent feature; requirements partly unclear; multiple stakeholders; what is clear but how is not; technical decisions between approaches (gate vs evidence provider vs resolver, field- vs state-level declaration); architecture questions remain; multiple viable implementation paths; architectural decisions made that belong on record; core question matches. (9)
- /charter: multiple work items need ordering; dependencies affect delivery order; strategic arguments produced (devil's advocate); partial "should we build this". (4)

### Anti-Signals Checked
- /scope: "multiple independent features whose order affects delivery". Arguably present, since the shirabe reshaping can land independently. (1)
- /charter: project already exists; users and needs identified. (2)
- File an issue: others need documentation; multiple people; architectural decisions made. (3+)

### Ranking
- /scope: 8 (demoted)
- /charter: 2 (demoted)
- File an issue: negative (demoted)

## Tiebreakers Applied
- None needed. /scope leads /charter by more than one point after demotion. The multi-feature boundary was considered: the shirabe work is sequenced consumers of one koto feature, which /scope's PLAN can order.

## Alternatives Considered
- **/charter**: fits a multi-feature reading (decider, routing gaps, reshaping, porting prose skills onto templates, promotion tooling), but the project exists and the core is one feature.
- **Decision Record**: the staged-posture call is one input among several the build needs.
- **File an issue**: architectural decisions were made and more than one repo builds from them. The deterministic-fact moves in shirabe could still be filed individually.
