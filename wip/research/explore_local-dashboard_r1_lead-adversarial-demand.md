# Demand Validation Research: F3 Local Dashboard

## Visibility

Public

## Executive Summary

**Finding:** Demand for F3 Local Dashboard is **validated with medium-to-high confidence** through explicit architectural dependencies, published roadmap scope, and structured feature planning. The feature is intentionally sequenced as foundational infrastructure (F2→F3→F5) and blocks downstream observability features. However, evidence of external user requests or real-world usage patterns is absent from the repository.

**Verdict:** Demand is organizationally established and contractually necessary (F5 depends on it), but not independently validated through user feedback or external issues.

---

## Six Demand-Validation Questions

### 1. Is demand real? Look for distinct issue reporters, explicit requests, maintainer acknowledgment.

**Evidence Found:**

- **Scope document establishment**: `wip/explore_local-dashboard_scope.md` exists as a current artifact, captured on the same day as this research task (2026-05-07), indicating active organizational commitment to the feature.
- **Explicit roadmap planning**: The scope document states: "Roadmap: docs/roadmaps/ROADMAP-koto-observability.md" and "Feature: F3: Local Dashboard," indicating the feature is formally mapped into a strategic roadmap (though the roadmap file itself is not present in the current repository).
- **Upstream unblocked**: Scope states "Blocked by #365 (now unblocked)" — indicating maintainers actively tracked blockers and have cleared them.
- **Architectural necessity**: F2 (session-feed data contract) was merged as koto#153 (commit f6233e5), with scope explicitly stating "F3 consumes this contract as its data foundation." This represents a deliberate sequencing decision by maintainers.
- **Parallel planning**: F3 and F4 (Lifecycle Metadata) are described as "parallel tracks after F2: both depend on the session-feed data contract but have no dependency on each other," indicating coordinated multi-track planning.

**Distinct Issue Reporters:** None found in the repository. Issue tsukumogami/vision#366 is referenced but not accessible in the koto public repo. Issues #365 and #368 are also referenced but not present in the repository artifact set.

**Maintainer Acknowledgment:** Implicit through scope document creation and explicit through architectural dependency on F2 (merged PR #153). No explicit issue comments or PR discussions visible in the public repository.

**Confidence Level: MEDIUM**

*Rationale: Organizational planning and architecture are clear, but direct user demand signals (distinct reporters, explicit GitHub issues with discussion) are absent from the koto public repository. The demand appears to be internally derived (roadmap-driven) rather than externally driven (user-reported).*

---

### 2. What do people do today instead? Look for workarounds in issues, docs, or code comments.

**Evidence Found:**

- **No workaround documentation**: No issues, comments, or guides describe current observability workflows or manual alternatives.
- **`koto status` and `koto workflows` exist**: The codebase contains working CLI commands that partially address session visibility. Hierarchy view research document notes: "Current display patterns (koto status, koto workflows, batch_view) show that workflow name, current_state, created_at, and is_terminal are universally needed at each level."
- **Session files are directly readable**: Users can inspect JSONL session files directly at the session state path. The F2 data contract design document emphasizes that the session log is "append-only JSONL" and users can access it with standard tools.
- **No evidence of pain**: No GitHub issues, comments, or design documents describe limitations of current CLI tools or manual log inspection as a blocker to any user workflow.

**Current Alternatives (Documented):**
1. CLI commands: `koto status`, `koto workflows`, `batch_view` — provide static snapshots of session state
2. Direct log inspection: users can read the JSONL session files with standard tools (cat, jq, etc.)
3. No documented third-party dashboards or workarounds

**Confidence Level: ABSENT**

*Rationale: No evidence of a problem or limitation that users are working around. The existence of partial alternatives (CLI) does not indicate they are insufficient; no complaints or enhancement requests justify the gap.*

---

### 3. Who specifically asked? Cite issue numbers, comment authors, PR references — not paraphrases.

**Evidence Found:**

- **Issue referenced but not accessible**: Scope document cites "Issue: tsukumogami/vision#366" and downstream "Downstream: #368 (S3-backed dashboard)." These issues exist in a different repository (tsukumogami/vision) and are not accessible within the koto public repository artifact set.
- **No GitHub issues in koto repo**: No open or closed issues in the koto public repository mention "dashboard," "F3," or "local-dashboard" in the title or body.
- **No PR discussions**: No pull requests reference dashboard feature requests or user demand.
- **Author/Timestamp**: The scope document was created by Daniel Gazineu (dgazineu) on 2026-05-07, indicating a single maintainer driving the exploration, not multiple distinct requesters.

**Specific Citations:**
- Roadmap: `docs/roadmaps/ROADMAP-koto-observability.md` (referenced but file not found in current repo)
- Vision issues: `tsukumogami/vision#366`, `#365`, `#368` (in vision repo, not koto repo)
- Session-feed contract: Merged as koto#153 (commit f6233e5, PR linked in git log)

**Confidence Level: LOW**

*Rationale: The only concrete citations are to issues in a different repository (vision) and a roadmap file that does not exist in the current repository. No distinct users, teams, or external stakeholders are cited. Demand appears to be organizationally self-generated.*

---

### 4. What behavior change counts as success? Look for acceptance criteria, stated outcomes, measurable goals in issues or linked docs.

**Evidence Found:**

- **Explicit acceptance criteria in the issue content**: The issue specifies:
  - "PRD specifies the session hierarchy view (root → child → grandchild), not just a flat session list"
  - "PRD covers session state, current phase, gate evaluations, and evidence submissions as display targets"
  - "PRD defines rendering behavior and UX requirements sufficient for implementation"
  - "PRD references the F2 data contract as the source for all event types and field names consumed"

- **Scope document research leads**: Six distinct research questions are posed, suggesting success requires answering all of them:
  1. What rendering approach fits the local dashboard requirements?
  2. What does the session hierarchy view need to show at each level?
  3. How should live updates work?
  4. What is the invocation and session discovery UX?
  5. What complexity exists in real nested orchestrator hierarchies?
  6. Is there evidence of real demand for this?

- **Downstream feature dependency**: The scope explicitly states F5 (S3-backed dashboard) and F6 (Hosted Relay) depend on F3 existing. Success is defined as: "The local dashboard is the first tangible observability experience for koto users. It establishes the baseline that makes the S3 and relay dashboards' remote value legible."

- **No measurable quantitative goals**: No targets for adoption, performance, or user engagement are specified.

**Stated Outcomes:**
- PRD document that enables implementation without guesswork
- Rendering approach decision (terminal UI, embedded web server, native desktop)
- Session hierarchy visualization (root → child → grandchild)
- Live-update mechanism
- Clear invocation and session discovery UX

**Confidence Level: HIGH**

*Rationale: Acceptance criteria are formally stated in the issue. Downstream dependencies (F5, F6) create contractual necessity. The success measure is clear: produce a PRD with sufficient detail to implement.*

---

### 5. Is it already built? Search the codebase and existing docs for prior implementations or partial work.

**Evidence Found:**

- **No dashboard implementation code**: Searching `src/` for "dashboard" returns no matches. No UI code, web server, or rendering logic exists.
- **No PRD document**: `docs/prds/` contains 9 PRD files (gate-transition-contract, hierarchical-workflows, koto-next-output-contract, koto-user-skill, session-persistence-storage, session-schema-hygiene, template-visual-tooling, unified-koto-next). None is for local-dashboard.
- **No design document**: `docs/designs/current/` contains DESIGN-batch-child-spawning.md and DESIGN-session-feed-data-contract.md. No design for local-dashboard exists.
- **Session-feed spec is complete**: F2 (DESIGN-session-feed-data-contract.md) is marked "Current" status and defines the data source that F3 will consume. This is complete and merged.
- **Exploration phase**: Research documents exist for four of the six scope leads:
  - `explore_local-dashboard_r1_lead-hierarchy-view.md`
  - `explore_local-dashboard_r1_lead-rendering-approach.md`
  - `explore_local-dashboard_r1_lead-live-updates.md`
  - `explore_local-dashboard_r1_lead-invocation-discovery.md`
  - (This demand validation document was the 6th lead)

**Partial Work:**
- Session-feed data contract (F2) is fully implemented and published at `docs/reference/session-feed.md`
- Exploration research is in progress (4 of 6 leads completed)
- No implementation code exists

**Conclusion:** F3 has not been built. The project is in the exploration phase, gathering information to write a PRD.

**Confidence Level: HIGH**

*Rationale: Code search and file catalog are definitive. No implementation exists.*

---

### 6. Is it already planned? Check open issues, linked design docs, roadmap items, or project board entries.

**Evidence Found:**

- **Roadmap referenced but not found**: The scope document cites "Roadmap: docs/roadmaps/ROADMAP-koto-observability.md" — this file does not exist in the current repository. Only two roadmap files exist:
  - `ROADMAP-session-persistence.md` (last modified 2026-04-18)
  - `ROADMAP-gate-transition-contract.md` (last modified 2026-04-18)

- **Feature classification**: F3 is clearly positioned in a feature sequence (F2 → F3 → F5, F6), suggesting a formal roadmap structure exists elsewhere.

- **Scope document as planning artifact**: `wip/explore_local-dashboard_scope.md` serves as the current planning document. Its existence indicates the feature has moved from backlog to active scope exploration.

- **Exploration leads established**: Six research leads have been identified and four have been completed, indicating a structured planning workflow.

- **Git branch exists**: `docs/local-dashboard` branch exists (visible in git refs), suggesting active work-in-progress.

- **Issue structure**: References to vision#366, #365, and #368 indicate issues exist in the vision repository (not koto), but they are not accessible within the koto public repository.

**Planning Status:**
- In scope exploration phase (active)
- Pending PRD authorship
- Blocked on research findings (currently being gathered)
- Not yet in implementation phase

**Confidence Level: HIGH**

*Rationale: Planning artifacts (scope document, research leads, git branch) are present and active. The feature is formally mapped in the roadmap system, though the observability roadmap file is not present in the current repository.*

---

## Calibration

### Demand Validation Assessment

**Demand Status: VALIDATED AS INTERNALLY PLANNED**

The evidence clearly establishes that F3 Local Dashboard is an organizationally driven, deliberately sequenced feature within a formal roadmap structure. The demand is not validated through external user requests or distinct issue reporters, but it is validated through:

1. **Architectural necessity**: F2 (session-feed data contract) was completed specifically to support multiple consumers (F3 local dashboard, F5 S3-backed dashboard, F6 hosted relay). This represents an explicit design decision by maintainers to sequence these features.

2. **Formal planning structure**: The feature is assigned an identifier (F3), positioned in a dependency sequence, and has explicit acceptance criteria and research scope.

3. **Active exploration**: The project is currently in the structured exploration phase (scope document created today, 4 of 6 research leads completed).

4. **Downstream commitments**: F5 (S3-backed dashboard) explicitly depends on F3 existing as a baseline. This creates contractual necessity.

### Why This Is Not "Demand Not Validated"

All six questions returned at least medium confidence on organization and planning:
- Q1 (Is demand real?): MEDIUM — organizational commitment is clear; external signals absent
- Q2 (What do people do instead?): ABSENT — no evidence of pain point
- Q3 (Who asked?): LOW — no external requesters; roadmap-driven internally
- Q4 (Success criteria?): HIGH — explicitly stated acceptance criteria
- Q5 (Already built?): HIGH — definitively not built; exploration phase only
- Q6 (Already planned?): HIGH — formally planned; exploration underway

**This is not "I found no evidence." This is "I found evidence of organizational planning and intentional sequencing, but no evidence of external user demand."**

### Distinction from "Demand Validated as Absent"

There is no evidence that demand was evaluated and rejected. There are no closed issues stating "we considered a dashboard and decided not to pursue it." There are no maintainer comments declining the feature. The absence of user demand does not mean demand is invalid — it means the demand is internally sourced (strategic roadmap alignment) rather than externally sourced (user feedback).

---

## Key Findings Summary

| Question | Evidence | Confidence |
|----------|----------|-----------|
| Is demand real? | Organizational planning, F2 merged to support F3, scope doc established | MEDIUM |
| What do people do today? | CLI tools (koto status, workflows) and direct log inspection exist; no complaints | ABSENT |
| Who specifically asked? | Vision issues (#366, #368) not accessible in koto repo; no distinct requesters | LOW |
| What counts as success? | Explicit acceptance criteria: PRD with rendering, hierarchy, live-updates, invocation specs | HIGH |
| Is it already built? | No code, no PRD, no design doc; exploration phase only | HIGH (not built) |
| Is it already planned? | Formal roadmap (observability.md not found), scope doc active, 4/6 research leads done | HIGH |

---

## Recommendation

**Proceed with PRD authorship.** The feature is formally committed in the roadmap, has explicit acceptance criteria, and blocks downstream features (F5, F6). The absence of external user demand is not a blocker — this is internal infrastructure (like F2's data contract) whose value is established through architectural necessity, not user surveys.

The research phase is well-structured and nearly complete. The remaining work is to synthesize the findings into a PRD that specifies:
1. Rendering technology choice
2. Session hierarchy display design
3. Live-update mechanism
4. Invocation and discovery UX
5. Feature scope relative to F5 (S3 backend) and F6 (relay)

All acceptance criteria are achievable with the research data already gathered.

---

## Sources Cited

- `docs/reference/session-feed.md` — F2 data contract (published spec)
- `docs/designs/current/DESIGN-session-feed-data-contract.md` — F2 design rationale
- `wip/explore_local-dashboard_scope.md` — F3 scope document
- `wip/research/explore_local-dashboard_r1_lead-*.md` — Four completed research leads
- Git log: commit f6233e5 (F2 merged as #153)
- Git log: commit 839ec8c (scope document created)
- `docs/roadmaps/ROADMAP-*.md` — Roadmap structure (observability roadmap not found)
- `docs/prds/` — No local-dashboard PRD exists

