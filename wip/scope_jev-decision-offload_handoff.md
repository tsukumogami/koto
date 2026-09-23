# /scope Handoff: jev-decision-offload

## Provenance
Written by `/explore` on 2026-09-23 from `wip/explore_jev-decision-offload_crystallize.md`.
Research files: `wip/explore_jev-decision-offload_findings.md`,
`wip/explore_jev-decision-offload_decisions.md`, and
`wip/research/explore_jev-decision-offload_r*_lead-*.md`.
One discover-converge round with seven leads, one of them a devil's advocate
lead the author added. Along the way the author settled authority, no-key
parity, the decider interface, and the surface, and after reading the case
against, chose a staged posture.

## Problem Statement
Agents running koto workflows spend turns and context on branch decisions that
are closed-set judgments over a small, knowable input: which kind of issue this
is, whether upstream drift changes a plan's intent, whether an acceptance
criterion is concrete. Typed decision models such as Jev answer that kind of
question in ~100-300 ms for a fraction of a cent, with calibrated
probabilities. koto should be able to answer such a decision itself during
`koto next` when a decider is configured, and hand it to the agent exactly as
today when not. Shirabe's workflows should be shaped so their decisions can be
declared that way.

## Scope Boundary
### In scope
- A template-declared typed question on an `accepts` field (choice, score,
  boolean), with per-value criteria, an escape key, thresholds and declared
  inputs. The same declaration renders as the agent prompt on fallback.
- A decider hook at the `NeedsEvidence` point of `advance_until_stop`. It writes
  ordinary evidence plus an additive audit event, sticks for the epoch, and
  falls back to today's `evidence_required` on low confidence, escape, no key,
  or error.
- A generic decider trait with Jev as the first provider. Key handling follows
  the cloud-sync credential pattern.
- Shadow mode as the default, per-value auto with a compiler-enforced `never`
  floor, a concordance ledger that survives session cleanup, and a layered
  kill switch.
- Compiler rules for the declaration, and the floor: no authority over gates,
  overrides, or irreversible or confirmation-guarded edges.
- koto gaps that block useful declarations: `expects` carrying descriptions,
  and possibly visit counts, variable value equality, and prior-state evidence
  references.
- Shirabe: split first-target states into gather / decide / act; move
  deterministic "decisions" into gates; declare the first classifier-eligible
  questions (candidates: `worktree_discipline_check`, `issue_type`,
  `task_validation`, `plan_validation`, `staleness_check`).

### Out of scope
- Using a decider for generative work (drafting, code, rationale text).
- Human gates (`deferral_approval`, `chain_proposal`) and safety halts (CI
  failure handling). These must stay agent- or human-decided.
- Porting prose-only skills (explore, plan, review-plan, validation juries)
  onto templates. That's where the biggest wins live, but it's follow-on work
  once the mechanism exists.
- A standalone decide command. The author ruled it out in favor of steps
  reached through `koto next`.
- Deep vendor comparison.

## Decisions Already Settled
- Authority: the template author marks which decisions are eligible, within a
  floor the compiler enforces (no gates, overrides, or irreversible or
  confirmation-guarded edges).
- No-key parity: with no decider configured, workflows behave exactly as
  today.
- Interface: generic typed-decider vocabulary in templates, Jev first behind a
  trait.
- Surface: the decision is a step reached through `koto next`. koto executes
  it when configured; otherwise the agent answers with evidence. No separate
  command.
- Posture: staged. Shadow is the default mode; auto is enabled per value only
  after this project's own traffic shows adequate concordance and
  minority-class recall. The shirabe reshaping proceeds in parallel.
- Extension point: evidence provider in the `NeedsEvidence` arm. A new gate
  type and a direct transition resolver were considered and rejected.
- Deterministic facts (`batch_outcome`, `pause_decision`, `cascade_status`,
  verification matching, panel aggregation, retry caps) move to gates
  regardless of the decider.
- Payoff: context, wall-clock, consistency and cost all count.

## Coverage Notes
- Calibration on shirabe's own decisions is unmeasured. The promotion bar (N
  paired observations, M minority cases, recall threshold) needs setting, and
  a hand-built fixture set is likely needed because surviving logs are
  happy-path only.
- Variable-length question sets (one question per AC, finding or unit) aren't
  designed. koto states have a fixed field set.
- Where aggregation lives (count, argmax, margin, score banding): koto or
  shirabe scorer scripts.
- Whether the agent ever sees the decider's distribution on fallback (anchoring
  versus helpfulness versus clean shadow labels).
- Field-level versus state-level declaration: an older koto ignores unknown
  field-level keys but rejects unknown state-level keys.
- What gets stored for replay (payload digest plus source manifest, or the full
  payload behind an opt-in), and whether the ledger syncs under the cloud
  backend.
- Whether a decider runs when a gate failed on the state, and whether rewind
  invalidates a sticky result.
- Measured main-thread savings: the devil's advocate put reachable decision
  states at ~8% of agent time in surviving logs. The PRD should state what
  success looks like in measurable terms.
- Vendor terms (versioning, rate limits, deprecation) are unverified.

## Upstream Observations
The exploration read `docs/designs/current/DESIGN-koto-runs-commands.md` and
`docs/briefs/BRIEF-koto-runs-commands.md` (the invisible-success, fallback-via-
existing-shapes precedent this feature copies), and
`docs/designs/current/DESIGN-config-and-cloud-sync.md` (sync HTTP client,
credential handling). It also read `docs/STABILITY.md` (additive events allowed
without a schema bump) and
`docs/designs/current/DESIGN-unified-koto-next.md` (the unimplemented
`integration` hook, which stops the loop and so isn't reusable as-is). In
shirabe it read `docs/designs/current/DESIGN-koto-default-action-adoption.md`
and `references/default-action-conversion.md` (the state-splitting campaign
this extends) and `docs/specs/decision-points.md` (a partly stale decision
manifest, a natural seed for a registry of eligible decisions). No ROADMAP
covers this topic.

## Framing-Shift Answer
**Pre-supplied answer:** yes, the framing shifted
**Evidence:** The request framed this as "decisions made automatically when a
key exists." Round 1 moved it to a staged rollout: shadow by default, auto per
value on evidence. It also surfaced that the mechanism only pays off after
shirabe states are split into gather / decide / act (findings, Key Insights;
decisions file, Round 1 posture).

## Shape Signals
### Architectural alternatives left open
- Declaration placement: field-level `classify` (graceful degradation on older
  koto, inputs repeated per field) versus a state-level block for shared inputs
  (loud failure on older koto, one request per state).
- Aggregation: in koto (declarative count/argmax/margin, one declared unit, a
  larger engine surface) versus shirabe scorer scripts (testable today, logic
  split across repos).
- Per-item questions: koto iterating over structured lists (audit in koto,
  new engine support) versus reviewer subagents calling the decider themselves
  (no engine change, no koto audit trail, and it conflicts with the
  no-standalone-command decision).
- Replay storage: digest plus source manifest (small, can't rebuild the input
  once context is overwritten) versus full payload stored content-addressed
  (auditable, larger, and synced by the cloud backend).

### Complexity signals
- The change spans two repos with a producer/consumer dependency: koto's
  schema has to exist before shirabe can declare questions.
- The devil's advocate lead argued the engine change is subsystem-sized (new
  events, nine or more compile rules, config, key handling, ledger, stub
  servers) in an engine with a written stability contract.
- Cost asymmetry per edge (a false pass ships defects, a false block costs one
  loop) means thresholds are per value, not per question. That's a contested
  trade-off.
