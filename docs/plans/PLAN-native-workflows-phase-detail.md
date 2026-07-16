---
schema: plan/v1
status: Active
execution_mode: single-pr
upstream: docs/designs/DESIGN-native-workflows-phase-detail.md
milestone: "Native /workflows phase detail (Feature 2)"
issue_count: 5
---

# PLAN: real phase/agent detail in the rendered `/workflows` entry

## Status

Active

Single-PR plan decomposing `DESIGN-native-workflows-phase-detail` (Feature 2 of
the koto-agent-surface-legibility roadmap) into five dependency-ordered issues
implemented on one branch. The plan extends Feature 1's `workflows_surface`
module additively; it does not reopen F1's commit-funnel hook, publish/discover,
atomic write, or opt-in.

## Scope Summary

Enrich Feature 1's single-session `/workflows` projection into the session's
real structure -- ordered phases with the active one marked, the active
directive/label, per-phase evidence and gate outcomes, and a gate-blocked ->
blocked status -- by reusing koto's dashboard detail read seam and extending the
`koto-<uuid>.json` contract additively, with a shape fixture for Feature 4.

## Decomposition Strategy

**Walking skeleton (additive over F1).** The five issues follow the DESIGN's
dependency-ordered implementation approach: the contract shape first (data model
with no behavior change), then the two pure derivations (ordered phases,
per-state outcomes), then the enriched projection that assembles them and reuses
the read seam, then the materialize wiring that emits the enriched file, and
finally the shape fixture plus the extended verification harness that proves the
acceptance criteria and F1's no-regression. Each issue lands its own tests;
issues 1-3 are pure/independent behind issue 4's wiring, and issue 5 depends on
the emitted file being enriched (issue 4).

## Issue Outlines

### Issue 1: feat(workflows): extend the file contract with phases and blocked status

**Goal**: Add the additive `phases[]` and `workflowProgress[]` fields, a
`Blocked` render status, and bump the contract version, without changing F1's
shape or the writer's behavior.

**Acceptance Criteria**:
- [ ] `RenderStatus` gains a `Blocked` variant serializing as `"blocked"`.
- [ ] `WorkflowFile` gains additive `phases: Vec<Phase>` (serialized `phases`)
  and `workflow_progress: Vec<ProgressNode>` (serialized `workflowProgress`);
  a `Phase { title, detail }` type; a `ProgressNode` enum serializing the two
  `type`-tagged shapes (`workflow_phase` with `index`/`title`; `workflow_agent`
  with `index`/`label`/`phaseIndex`/`phaseTitle`/`state`/`promptPreview`/
  `resultPreview`), with `skip_serializing_if` keeping empty optional fields
  absent.
- [ ] `CONTRACT_VERSION` is bumped to 2; F1's top-level fields (`id`, `name`,
  `status`, `startTime`) and `koto` block are unchanged in shape and meaning.
- [ ] Unit tests assert the serialized keys, both progress node types, the
  `blocked` status value, and that an F1-minimal file remains a valid subset.

**Dependencies**: None

**Type**: code
**Files**: `src/workflows_surface/contract.rs`

### Issue 2: feat(workflows): derive ordered phases and per-state outcomes from the log

**Goal**: Add the two pure derivations the enriched projection needs -- the
structural phase ordering and the full-log per-state outcome bucketing.

**Acceptance Criteria**:
- [ ] `ordered_phases(&CompiledTemplate) -> Vec<...>` walks from `initial_state`
  following declared `transitions` in order, dedups visited states, skips
  self-loops, and appends states unreachable from `initial_state` in template
  (`BTreeMap`) order; the result is deterministic.
- [ ] `per_state_outcomes(&[Event]) -> map state -> { latest gate outcome,
  evidence entries }` buckets over the full event log, reusing the same
  `EventPayload` matching the dashboard read seam uses.
- [ ] Unit tests cover linear, branching, self-loop, rewound, and
  unreachable-tail templates for ordering, and gate/evidence bucketing per
  state for the outcomes.

**Dependencies**: None

**Type**: code
**Files**: `src/workflows_surface/project.rs`

### Issue 3: feat(workflows): assemble the enriched projection reusing the detail read seam

**Goal**: Build the enriched projection -- ordered phases with per-phase status
and outcome, the active-phase directive, and the blocked classification -- by
reusing `read_detail`/`DetailData` and the dashboard's blocked predicate.

**Acceptance Criteria**:
- [ ] An enriched projection carries the ordered phases (each tagged
  done/active/upcoming from the current state and visited-state set), each
  completed phase's evidence/gate outcome (issue 2), and the active phase's
  directive/label.
- [ ] The active-phase directive, current-epoch evidence, and latest gate are
  sourced from `read_detail`; the blocked classification reuses koto's
  blocked-in-current-epoch predicate (lifted to shared visibility if private),
  with precedence terminal > blocked > running.
- [ ] Unit tests cover a running session with an active directive, a session
  with completed-phase outcomes, and a gate-blocked session.

**Dependencies**: Blocked by <<ISSUE:1>>, <<ISSUE:2>>

**Type**: code
**Files**: `src/workflows_surface/project.rs`

### Issue 4: feat(workflows): emit the enriched file on the commit funnel

**Goal**: Wire the enriched projection into the materializer so each commit
writes the enriched `koto-<uuid>.json`, leaving F1's resolve/gate/atomic-write
path unchanged.

**Acceptance Criteria**:
- [ ] `materialize_after_commit` builds the enriched `WorkflowFile` (F1 fields +
  `phases` + `workflowProgress` + `status` including `blocked`) from the
  enriched projection; the opt-in gate, directory resolution, stable
  `startTime`, and atomic write are unchanged.
- [ ] Module integration tests drive a real multi-phase koto session and assert:
  AC1 (ordered phases with the active one marked and completed phases' outcomes
  present), AC2 (active directive present), and AC3 (a gate-blocked session
  emits `status: blocked`).
- [ ] F1's existing materialize tests still pass unchanged (no regression).

**Dependencies**: Blocked by <<ISSUE:3>>

**Type**: code
**Files**: `src/workflows_surface/materialize.rs`

### Issue 5: test(workflows): shape fixture and extended end-to-end verification

**Goal**: Pin the enriched shape with a committed fixture for Feature 4's guard,
and extend F1's verification harness/guide to exercise the F2 properties
alongside F1's four no-regression checks.

**Acceptance Criteria**:
- [ ] A committed enriched-shape JSON fixture (a multi-phase session: completed
  phases with outcomes, an active phase with a directive, and a blocked variant)
  plus a contract test asserting the emitted shape against it, documented as
  Feature 4's anchor.
- [ ] `scripts/verify-native-workflows.sh` and
  `docs/guides/native-workflows-verification.md` are extended to exercise AC1-AC3
  and to re-run F1's four checks (single-session render, update on reopen,
  done-on-completion, default-path-untouched) with no regression.
- [ ] `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, and
  `cargo fmt --check` all pass; the koto-user / koto-author skills are assessed
  for any surface change (none expected -- no new CLI surface) and updated if
  needed.

**Dependencies**: Blocked by <<ISSUE:4>>

**Type**: code
**Files**: `scripts/verify-native-workflows.sh`, `docs/guides/native-workflows-verification.md`

## Dependency Graph

Single-PR plan; the dependency structure is a linear join (issues 1 and 2 are
independent roots that merge at issue 3):

- Issue 1 (contract) and Issue 2 (derivations) -- independent, no dependencies.
- Issue 3 (enriched projection) -- depends on Issue 1 and Issue 2.
- Issue 4 (emit on commit) -- depends on Issue 3.
- Issue 5 (fixture + verification) -- depends on Issue 4.

## Implementation Sequence

**Critical path:** #1/#2 -> #3 -> #4 -> #5.

- **Issues 1 and 2** are independent and can be done in parallel (the contract
  shape and the two pure derivations touch different concerns:
  `contract.rs` vs the derivation functions in `project.rs`).
- **Issue 3** joins them: it needs the contract types (1) to shape the enriched
  projection and the derivations (2) to fill it, and it reuses the read seam.
- **Issue 4** wires the enriched projection into the commit funnel and proves
  AC1-AC3 against the emitted file.
- **Issue 5** pins the shape fixture (Feature 4 anchor) and extends the
  end-to-end verification harness for the F2 properties plus F1's
  no-regression checks. It comes last because it verifies the emitted enriched
  file (issue 4).

Single-PR: all five land on one branch; the PLAN is deleted and the upstream
BRIEF/PRD/DESIGN transition to their terminal states in the same commit set that
readies the PR.
