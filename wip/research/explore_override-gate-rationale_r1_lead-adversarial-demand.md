---
status: Complete
author: Claude (demand-validation researcher)
date: 2026-03-30
visibility: Public
---

# Research Report: Override Gate Rationale Capture Demand Validation

**Issue:** #108 (feat(engine): require rationale when overriding gates)

**Research Method:** Six-question demand validation across GitHub issues, design documents, codebase, and test fixtures in the koto repository.

---

## Question 1: Is demand real?

**Finding:** YES — Clear, high-confidence evidence from maintainer and downstream workflows

**Confidence:** HIGH

**Evidence:**
- **Issue #108** filed by Dan Gazineu (maintainer, @dangazineu) on 2026-03-30, titled "feat(engine): require rationale when overriding gates"
  - Explicitly identifies the problem: "The agent can separately call `koto decisions record`, but nothing forces this — the override and the rationale are disconnected"
  - Includes three concrete use cases from real workflows
  
- **Work-on template design** (DESIGN-shirabe-work-on-template.md) identifies three deterministic states where agents override gates:
  - `context_injection`: "Override: user provides additional context alongside the issue"
  - `setup_issue_backed`: implicit override path via gate-with-evidence-fallback
  - `setup_free_form`: same override pattern
  - Document states: "each step has a default action (koto executes automatically), an override path (user input changes the default), and a failure path (agent recovers)"

- **Gate-with-evidence-fallback implementation** (src/cli/next.rs, lines 56-68) explicitly documents the override use case:
  - "If the state has an accepts block, fall through to EvidenceRequired instead of returning GateBlocked. The agent can provide override or recovery evidence when gates fail on a state that accepts evidence"
  - This feature is merged (PR #84 context)

- **Downstream demand from shirabe integration** (issue #73, "feat(shirabe): integrate /work-on skill with koto template")
  - Depends on issue #72 (work-on template), which depends on override handling working reliably
  - Filed by dangazineu, scheduled for milestone "shirabe work-on koto template"

---

## Question 2: What do people do today instead?

**Finding:** SUBOPTIMAL WORKAROUND EXISTS — agents call `koto decisions record` separately, creating disconnection

**Confidence:** MEDIUM

**Evidence:**
- **Documented workaround in issue #108:**
  - "The agent can separately call `koto decisions record`, but nothing forces this — the override and the rationale are disconnected"
  - Implies current practice is split: override evidence via `koto next --with-data {status: override, ...}` + manual `koto decisions record` call

- **Decisions subsystem exists** (issue #66 "feat(engine): implement mid-state decision capture")
  - Implemented in DESIGN-mid-state-decision-capture.md, marked as Current status
  - Schema: `{choice: string, rationale: string, alternatives_considered: [string]}`
  - Invoked via `koto decisions record <name> --with-data '{...}'`
  - Tested in test/functional/features/decisions.feature

- **Manual decision capture is advisory, not enforced:**
  - Design-mid-state-decision-capture.md explicitly chooses advisory over enforcement (Decision 3, "Purely advisory")
  - No `min_decisions` validation, no template enforcement
  - Implies agents can skip decision recording without prevention

- **No evidence of integration between override submission and decision capture:**
  - Override evidence example in simple-gates.md template fixture: `{status: override, detail: ...}` — `detail` is optional
  - No automatic logging of override rationale as a decision event
  - Work-on template evidence schemas (context_injection, setup states) expect `status: override` but don't mandate rationale

- **Rational for current workaround inadequacy:**
  - Two separate API calls (override evidence + decision record) means the operations can be separated by time, or one can be forgotten entirely
  - Decision and override are not atomically linked in the event log
  - Session audit trail shows evidence but rationale is separate, complicating later visualization or redo

---

## Question 3: Who specifically asked?

**Finding:** Maintainer-identified demand in concrete use cases

**Confidence:** HIGH

**Evidence:**
- **Issue #108 author:** Dan Gazineu (@dangazineu)
  - Created: 2026-03-30T00:47:52Z
  - Status: OPEN, no comments yet (fresh issue)

- **Issue #108 content explicitly cites three use cases:**
  1. "Any skill with CI gates: Agent bypasses red CI because 'flaky test unrelated to this change.' The bypass should automatically capture this reasoning"
  2. "shirabe /work-on context_injection: Agent overrides the baseline artifact gate. Why? 'Issue already read via gh issue view, context is in conversation.'"
  3. "shirabe /work-on setup: Agent overrides branch creation. Why? 'Reusing existing branch per user request.'"

- **Cross-reference:** "Ref: tsukumogami/shirabe PRD-koto-adoption.md"
  - Document not found in this repo (external reference), but referenced as motivation source

- **Downstream consumers:**
  - Issue #72 (work-on template) — filed by dangazineu, marked validation:testable
  - Issue #73 (shirabe integration) — filed by dangazineu, depends on #72
  - Both are active (OPEN, scheduled for milestone)

---

## Question 4: What behavior change counts as success?

**Finding:** Explicit acceptance criteria in issue and design exploration

**Confidence:** HIGH

**Evidence:**
- **Issue #108 "Proposed Behavior" section** specifies exact success criteria:
  ```yaml
  accepts:
    status:
      type: enum
      values: [completed, override, blocked]
    rationale:
      type: string
      required_when:
        status: override
  ```
  - Success requires: conditional validation (`required_when`) that mandates rationale field when status=override
  - Rationale stored "in the decision log (same as `koto decisions record`) with the gate name and state as context"
  - Visible via `koto decisions list`

- **Measurable outcomes implied:**
  1. Evidence validation rejects `{status: override}` without rationale
  2. Accepted override evidence is automatically logged as a decision event (not requiring separate call)
  3. `koto decisions list` includes override rationale
  4. Query shows gate name and state context

- **North-star from explore scope** (wip/explore_override-gate-rationale_scope.md):
  - "How should koto capture gate overrides as first-class auditable events with rationale, so they're queryable for visualization and eventually actionable for redo?"
  - Success enables: visualization of overrides, forced redo on disagreed overrides (future)

- **No formal PLAN doc with signed acceptance criteria yet** — proposal is in the feature-request phase, not planning phase

---

## Question 5: Is it already built?

**Finding:** NOT BUILT — but related infrastructure exists partially

**Confidence:** LOW (with positive findings on partial work)

**Evidence:**
- **Conditional validation (`required_when`) NOT IMPLEMENTED:**
  - Grep for "required_when" in src/: zero matches
  - Evidence validation (src/engine/evidence.rs) supports only:
    - Type checking (string, number, boolean, enum)
    - Required field presence (boolean flag)
    - No conditional/dependent field logic
  - Template schema (src/template/types.rs FieldSchema) has no `required_when` or conditional field support

- **Override-specific rationale capture NOT IMPLEMENTED:**
  - No special handling for `status: override` in validation or event logging
  - No automatic decision event generation on override
  - Override is treated like any other evidence value (tested in simple-gates.md fixture, simple transition logic)

- **Decisions subsystem PARTIALLY AVAILABLE:**
  - Fully implemented: `koto decisions record` and `koto decisions list` (issue #66 implementation complete)
  - Schema support: fixed schema with choice, rationale, alternatives_considered (type-checked in code)
  - Event type: `DecisionRecorded` exists in src/engine/types.rs
  - NOT integrated: No auto-logging when evidence includes override

- **Gate-with-evidence-fallback FULLY IMPLEMENTED:**
  - Code: src/cli/next.rs lines 56-68, verified functional
  - Allows agents to submit evidence when gate fails (override path)
  - Test: test/functional/features/gate-with-evidence-fallback.feature validates three scenarios
  - Merged in earlier PR (mentioned in ROADMAP-session-persistence.md context)

- **Existing evidence schemas in work-on template:**
  - context_injection: `{status: enum[completed, override, blocked], detail: string}` — detail optional, no rationale field
  - All deterministic states follow same pattern: status + optional detail, no rationale requirement

---

## Question 6: Is it already planned?

**Finding:** NOT YET FORMALLY PLANNED — exploratory phase only

**Confidence:** MEDIUM

**Evidence:**
- **Issue #108 has no planning artifacts:**
  - No milestone assignment
  - No linked PRs or design docs
  - Status: OPEN with zero comments
  - Created 2026-03-30, this is a fresh issue

- **No design doc yet:**
  - No docs/designs/ entry for override-rationale feature
  - Related designs exist but don't address override-specific capture:
    - DESIGN-mid-state-decision-capture.md (issue #66): advisory decisions, no override coupling
    - DESIGN-shirabe-work-on-template.md (issue #72): describes three-path override model but doesn't propose rationale capture mechanism

- **Explore scope document exists** (wip/explore_override-gate-rationale_scope.md):
  - Status: speculates on six research leads for design
  - Poses questions: event representation, data shape, interaction with decisions subsystem, query patterns, other engines' patterns, demand validation
  - Spawned from issue #108 (noted in document header)
  - This suggests the issue is under investigation but not yet in design phase

- **Related work in progress:**
  - Issue #72 (work-on template): Marked "validation:testable", OPEN, no design doc yet (would be next phase)
  - Issue #73 (shirabe integration): Marked "validation:testable", OPEN, depends on #72
  - Neither issue mentions override-rationale capture as a blocker

- **Roadmap entry:** Not mentioned in ROADMAP-session-persistence.md or other active roadmaps

---

## Research Artifacts Examined

**GitHub Issues:**
- #108 (feat(engine): require rationale when overriding gates) — primary source
- #66 (feat(engine): implement mid-state decision capture) — related infrastructure
- #72 (feat(template): write the work-on koto template) — downstream consumer
- #73 (feat(shirabe): integrate /work-on skill with koto template) — downstream consumer

**Design Documents:**
- docs/designs/DESIGN-shirabe-work-on-template.md — use cases, three-path override model
- docs/designs/current/DESIGN-mid-state-decision-capture.md — decisions subsystem design
- docs/designs/DESIGN-auto-advancement-engine.md — mentions override fallback path
- wip/explore_override-gate-rationale_scope.md — scoping research (spawned from #108)

**Implementation Code:**
- src/engine/evidence.rs — validation logic (no conditional support)
- src/engine/types.rs — event types (DecisionRecorded exists, no OverrideRecorded)
- src/cli/next.rs — dispatch logic (override fallback documented in lines 56-68)
- src/template/types.rs — schema definitions (no required_when field)

**Test Fixtures:**
- test/functional/fixtures/templates/simple-gates.md — override evidence example
- test/functional/features/gate-with-evidence-fallback.feature — override fallback tested
- test/functional/features/decisions.feature — decisions subsystem tested

---

## Calibration: Demand Validation Status

**VERDICT: DEMAND VALIDATED AS REAL, NOT YET PLANNED**

This is NOT ambiguous. The evidence clearly distinguishes between three possible outcomes:

1. **Demand not validated (would need another round):** Would be true if questions returned absent/low confidence with no rejection evidence. NOT the case here — three questions return HIGH confidence.

2. **Demand validated as absent (actively rejected):** Would be true if evidence showed the feature was considered and declined (closed PR with rejection rationale, design doc deferred, maintainer comment declining). NOT found.

3. **Demand validated as real but not planned (this case):** Evidence shows:
   - **Real demand:** Maintainer-filed, use cases cited, downstream workflows depend on override behavior
   - **Not yet planned:** No design doc, no child issue decomposition, no milestone, no acceptance criteria in a PLAN document
   - **Exploratory phase:** Issue exists, explore scope document is researching design questions

**Specific high-confidence findings:**

| Question | Confidence | Summary |
|----------|-----------|---------|
| Is demand real? | HIGH | Maintainer-identified use cases in work-on template, explicit issue, gate-with-evidence-fallback already implemented |
| What do people do today? | MEDIUM | Agents call `koto decisions record` separately; workaround is acknowledged as inadequate in issue #108 |
| Who asked? | HIGH | Dan Gazineu (maintainer), three use cases cited, downstream consumers (issues #72, #73) |
| Success criteria? | HIGH | Explicit proposed behavior in issue #108, conditional validation with rationale required on override |
| Already built? | LOW (partial) | NOT built, but gate-with-evidence-fallback exists and decisions subsystem exists (unintegrated) |
| Already planned? | MEDIUM | Issue exists but in fresh state; explore scope is researching design; no planning artifacts yet |

**Gap Analysis:**

The feature is at the **scoping phase**. To move to **design phase**, the maintainer should:

1. Confirm scope: Is override-rationale capture a special case, or does it motivate general conditional-validation support (`required_when` becomes a template feature)?
2. Event shape: New event type `OverrideRecorded`? Extension of `DecisionRecorded`? Auto-wrapper that turns override evidence into decision events?
3. Integration: How does override-captured rationale interact with the existing `koto decisions list` surface?
4. Query expansion: What cross-epoch/cross-state queries does future visualization need beyond epoch-scoped list?

These questions map to the six research leads posed in wip/explore_override-gate-rationale_scope.md.

---

## Recommendation

**Pursue the feature.** Demand is validated and well-founded:
- Real use cases in active workflows (work-on template, CI gate bypass)
- Maintainer-identified problem with explicit proposed solution
- Related infrastructure mostly exists (gate-with-evidence-fallback, decisions subsystem)
- No rejection evidence or explicit deferral

**Next step:** Promote issue #108 to a design phase by creating a child design doc or PLAN issue that addresses the six research leads above.

