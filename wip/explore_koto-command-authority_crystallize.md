# Crystallize Decision: koto-command-authority

## Chosen Type

A chain, entering at `/scope` — **folded into the existing
`koto-runs-commands` handoff rather than opening a second chain.**

Decided in `--auto` mode per the lightweight decision protocol.

## Candidacy

- `/execute`: **not a candidate.** The only PLAN on disk in either repo is
  shirabe's `PLAN-work-on-friction-fixes.md`, whose `execution_mode` is
  `multi-pr`, which `/execute` refuses. Nothing in koto's `docs/plans/`.
- Competitive analysis: **not a candidate.** Visibility is Public.

## Rationale

Stage 1 is not close. Six leads produced a body of work — an `--expect-hash`
argument with an open question about where the expected value lives, a rename
whose blast radius is unscoped, an anchoring increment with a named
implementation trap, a bound on the event log's `command` field with an
unresolved cost to the audit trail, a verifying gate for `pr_creation`, and a
conversion set with two live disagreements in it. Architectural choices were made
that need a durable home: reversibility has two axes and the second governs; the
operative authoring question is whether risk lives in a bad success or a bad
failure; runtime containment is not the investment. None of that survives in an
issue body, and `wip/` is deleted before merge.

Stage 2 lands on `/scope` for the same reasons the prior exploration did — one
bounded capability decomposed into small pieces, requirements contested across
leads, architecture open, no thesis to validate — and the `/charter`
multi-feature test fails again: the pieces are one capability's parts, not
separately-deliverable features competing for order.

The consolidation is the decision that matters here. This exploration did not
open a new subject; it re-answered the scope question of the existing chain under
a ruling that arrived after that chain's handoff was written, and it corrected
three of that handoff's premises. Two chains against the same engine surface
would produce conflicting designs — one specifying `capture_stdout_as` and
anchoring, the other specifying `--expect-hash` and a confirmation rename,
neither aware of the other's decisions. So the branches are merged and there is
one handoff.

## Stage 1 Evidence

### Signals Present
- Converged on something someone will build: eight work items with sizes.
- Architecture questions remain open: where the expected hash lives, the rename's
  scope, what bounding the `command` field costs the audit trail.
- Decisions need a durable home: the two-axis reversibility model and the
  bad-success/bad-failure rule are authoring guidance a template author will need
  long after this branch is gone.
- Multiple stakeholders need alignment: work lands in koto, in shirabe's
  templates, and in shirabe's release and marketplace configuration.
- A scope boundary emerged: invest in deciding what runs, not in containing what
  runs.
- The core question is "what do we build, and how?"

### Anti-Signals Checked
- Nothing left to build: not present.
- Output is one choice between named options: not present.
- A feasibility verdict nobody committed to: not present.
- Findings center on external products: not present.
- Conclusion is that the work should not happen: not present.

### Ranking
- A chain: **6**, no anti-signals.
- Spike Report: 3 signals − 2 anti-signals = 1 *(demoted)*. Specific risks were
  tested, but the question was never "can we", and the exploration ranged across
  six subjects rather than time-boxing one.
- Decision Record: 2 − 1 = 1 *(demoted)*. Several decisions, all with work
  attached.
- Rejection Record: 0. Nothing was rejected.
- Competitive Analysis: not a candidate.

## Stage 2 Evidence

### Signals Present
- Requirements contested: two leads reached opposing conclusions and had to be
  reconciled, and two disagreements survive.
- Architecture and integration questions remain: the koto/shirabe split for the
  hash check, and how pinning interacts with plugin updates.
- Multiple viable implementation paths: `--expect-hash` versus a shirabe-side
  content assert versus archive distribution with a `sha256`.
- Architectural decisions made that should be on record: the two-axis model.
- The core question is "what should we build, and how?"

### Anti-Signals Checked
- Multiple independent features whose order affects delivery: **present**, mildly
  — template trust is separable from the conversion work.
- One person can act without a written contract: not present.
- A qualifying PLAN already covers this: not present.
- The exploration produced no work: not present.

### Ranking
- `/scope`: 5 − 1 = **4** *(demoted)*.
- `/charter`: 2 − 2 = 0 *(demoted)*.
- File an issue: 1 − 4 = −3 *(demoted)*.
- `/execute`: not a candidate.

## Tiebreakers Applied

- **`/charter` vs `/scope`, the multi-feature boundary**: one bounded capability
  in parts, not several features competing for order. Branch taken: `/scope`.
- **Consolidation (not a framework rule; recorded as a decision)**: fold into the
  existing chain. The two explorations answer the same question at two moments,
  and the later one supersedes rather than supplements.

## Alternatives Considered

- **A separate chain for template trust.** It has the cleanest boundary of
  anything here and could stand alone. It ranks lower because it is small — one
  koto argument, one shirabe assert, one configuration line — and because its
  scope decisions interact with the conversion work through the same authoring
  guidance. Revisit if it grows.
- **File an issue.** Right for four items: the `pr_creation` gate, the `Path::join`
  absolute-path trap, extending shirabe's release checksums to templates, and
  documenting `sha` pinning. Wrong for the rest, and the anti-signals fire hard.
  The handoff names these as work that should not wait on the chain.
- **Spike Report.** The probes tested real risks, but the exploration answered
  "what should we do" rather than "can we", and committed to work.

## Deferred Type

Not applicable.
