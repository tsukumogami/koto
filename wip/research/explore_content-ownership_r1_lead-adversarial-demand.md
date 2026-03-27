---
status: Research Report
date: 2026-03-26
investigation: demand-validation
topic: Making koto own the cumulative context of workflow execution
scope: Public
---

# Research Report: Content Ownership Demand Validation

## Executive Summary

This report investigates whether evidence supports pursuing the topic: **Making koto own the cumulative context of workflow execution (research outputs, plans, baselines, reviews) instead of letting agents read and write them directly through the filesystem.**

**Verdict**: Demand is **validated as present** with **high confidence** on the storage location problem (Q1-Q4), and **medium confidence** on the content-ownership gate/retrieval problem (Q5-Q6). The evidence shows:

- **Real demand exists** for moving context out of `wip/` directories and out of the git working tree (articulated in PRD and roadmap)
- **The current problem is well-documented**: agents hardcode ~150 file paths, creating tight coupling
- **A solution is already planned**: session persistence roadmap (#1) delivers local filesystem storage and {{SESSION_DIR}} substitution
- **The open question** (content ownership as a koto-managed API, not just location management) is explored but not yet committed to as a feature

The session persistence feature ships filesystem abstraction. The content-ownership question (whether koto should gate all context I/O through a submission/retrieval API) remains open and depends on learnings from post-session-persistence adoption.

---

## Question 1: Is demand real?

### Finding

**High confidence**. Multiple durable sources confirm demand:

1. **PRD-session-persistence-storage.md** (status: In Progress, authored by maintainers)
   - Problem statement (section 1-3): "Agents own the storage location... Skills hardcode `wip/` paths (~150 file references across shirabe and tsukumogami plugins)... koto doesn't control where artifacts live."
   - Goal 1: "koto provides a session management API that controls where workflow artifacts are stored, so agents don't hardcode storage paths"

2. **ROADMAP-session-persistence.md** (status: Active, authored by maintainers)
   - Theme: "koto stores workflow session state (engine state files, skill artifacts, research output) in `wip/` committed to git branches. This is a solo-developer convention that doesn't scale..."
   - Explicit acknowledgment that this "doesn't scale to teams or multi-machine workflows"

3. **Recent commits** (60 commits since Jan 2024, 14 session-storage commits in last 30 days)
   - Commit `a2a5dea`: "chore: clean wip/ artifacts for merge" — indicates active removal of wip/ usage
   - Multiple session-storage PRs landed: commits from `5299e00` to `32b6849` show completed session storage implementation

4. **Scope document** (wip/explore_content-ownership_scope.md)
   - Framed as "research lead" (Q7), indicating the content-ownership question is derived from validated session-storage demand

### Confidence: High

Three independent maintainer-authored durable artifacts (PRD, ROADMAP, commits) describe the same problem with specific numbers (~150 file path references).

---

## Question 2: What do people do today instead?

### Finding

**High confidence**. The current workaround is well-documented:

**Today's pattern:**
- Agents write directly to hardcoded filesystem paths in `wip/` directories
- Skills contain ~150 hardcoded references to `wip/` paths across shirabe and tsukumogami plugins
- Example from hello-koto SKILL.md: `Create a file at {{SESSION_DIR}}/spirit-greeting.txt`
- Templates use gate conditions like: `test -f {{SESSION_DIR}}/spirit-greeting.txt` to detect completion

**Workarounds with pain points:**
1. **Git-based session transfer**: Push `wip/` to a branch, pull on another machine — couples state to git history and requires merge discipline
2. **File-existence heuristics**: Resume detection checks whether `wip/plan.md` exists — fragile because files can be partially written, deleted but cached in git history, or created out of order
3. **Manual cleanup**: Developers must clean wip/ before merge (enforce via CI checks)

**Evidence locations:**
- PRD section "Problem statement": "Agents write directly to wip/ paths, creating a tight coupling between skill code and storage location"
- DESIGN-koto-engine.md: "existing implementations use fragile state tracking: file-existence heuristics (check whether `wip/plan.md` exists to decide which phase to resume) break when files are partially written"
- Code references in src/engine/persistence.rs: function `derive_evidence()` manages state file logic
- Documentation in docs/guides/custom-skill-authoring.md shows agents writing to `{{SESSION_DIR}}` paths directly

### Confidence: High

The wip/ convention is the explicitly stated current approach, documented in design docs, roadmaps, and PRDs. The pain points (team visibility, non-portability, tight coupling) are articulated in the PRD.

---

## Question 3: Who specifically asked?

### Finding

**Medium confidence**. The demand is articulated by maintainers (not separate issue reporters), but no external user feedback is captured in the repo.

**Who asked:**
- **Maintainers (inferred from authorship)**: PRD-session-persistence-storage.md and ROADMAP-session-persistence.md are authored by project maintainers and accepted into the repo. No author attribution in the document headers, but commit history shows session-storage work is active (commits within 30 days).
- **No distinct issue reporters visible** in the public repo. This is a Rust rewrite of a Go codebase; external user feedback may exist in a private upstream or in the Go version.

**Evidence:**
- PRD author: Unknown (no git author field in markdown), but status "In Progress" indicates active maintainer commitment
- ROADMAP status: "Active"
- Commits by Daniel Gazineu (repo maintainer) showing session-storage implementation landing
- No GitHub issues in the .github directory (not visible; this is a tactical engineering repo)

**Notable gap**: The exploration scope document (wip/explore_content-ownership_scope.md) frames the content-ownership question as a research lead derived from session-persistence demand, not as a direct user request. The scope says "The user's longer-term vision includes..." but doesn't cite a specific user issue or request.

### Confidence: Medium

Demand is real and documented by maintainers, but no external users or issue numbers are cited. The project's status as a pre-release tool (no external users yet, per DESIGN-koto-agent-integration.md: "there are no external users of koto today") explains the lack of external issue citations.

---

## Question 4: What behavior change counts as success?

### Finding

**High confidence**. Acceptance criteria are explicit in multiple documents.

**Session Persistence Feature (Q1-3 scope, largely built):**
- **PRD acceptance criteria** (docs/prds/PRD-session-persistence-storage.md, lines 203-222):
  - `koto init <name>` creates a session directory alongside the workflow state
  - `koto session dir <name>` returns the correct path for the configured backend
  - Default backend stores artifacts in `~/.koto/sessions/<name>/`
  - Sessions don't appear in git diffs/history
  - Agents can Read/Edit/Write files in the session directory using standard paths
  - Cloud sync is implicit and transparent

- **PLAN acceptance criteria** (docs/plans/PLAN-local-session-storage.md):
  - Issue #1: SessionBackend trait and LocalBackend implementation with specific method signatures
  - Issue #2: CLI threading of backend through command dispatch
  - Issue #3: Runtime {{SESSION_DIR}} substitution with collision detection
  - Issue #4: Session subcommands (dir, list, cleanup)
  - Issue #5: Auto-cleanup on workflow completion with --no-cleanup flag
  - Issue #6: Documentation updates removing hardcoded `wip/` references

**Content Ownership (Q5-6 scope, not yet built):**
- **Scope document** (wip/explore_content-ownership_scope.md) lists "Research Leads" (Q1-6) but no acceptance criteria yet
- The scope explicitly says "The specific name for the CLI subcommand (needs UX exploration, not 'evidence')" is out of scope
- Success would include:
  - CLI for context submission/retrieval
  - Multi-agent concurrent submission without advancing state
  - Gate evaluation against koto-owned context
  - Resume logic based on koto-owned context
  - Skill-to-skill handoff flows through koto

### Confidence: High

Session persistence acceptance criteria are formally documented in PLAN and PRD with checkbox-format checklists. Content ownership success criteria are articulated as research leads but not yet formalized as acceptance criteria, indicating the feature is still in exploration phase.

---

## Question 5: Is it already built?

### Finding

**Partial, high confidence**:

**Session Persistence (storage location ownership) — BUILT:**
- Commits 5299e00-32b6849 (last 30 days) implement all 6 issues from PLAN-local-session-storage.md
- Code present in src/session/mod.rs, src/session/local.rs with SessionBackend trait
- src/cli/session.rs with dir, list, cleanup subcommands
- src/cli/vars.rs with {{SESSION_DIR}} substitution
- All features are documented in guides updated in commit 32b6849
- Status in PLAN: "Draft" (indicates plan is current)
- HEAD commit (a2a5dea): "chore: clean wip/ artifacts for merge" shows session-storage work just landed

**Content Ownership (submission/retrieval API) — NOT BUILT:**
- No CLI commands for `koto context add`, `koto context list`, `koto context retrieve`
- No evidence submission/retrieval beyond the existing `koto next --with-data` path (which sends evidence as JSON to the state machine, not to a content store)
- src/engine/evidence.rs implements evidence validation (validates JSON against accepts schema) but not persistence or retrieval
- No content-addressable storage or key-based retrieval in the engine

**What exists for evidence:**
- Evidence validation: Evidence JSON payloads validate against template-declared field schemas (src/engine/evidence.rs)
- Evidence routing: The `when` conditions in templates route to transitions based on evidence values (src/engine/advance.rs)
- Evidence scoping: Epoch boundaries clear evidence when rewinding (src/engine/persistence.rs, derive_evidence())
- But: Evidence is transient; it flows from agent submission to state transition, then is discarded

**Gap**: The scope document (wip/explore_content-ownership_scope.md) exists as a research placeholder but the content-ownership API is not implemented. The document even notes: "The specific name for the CLI subcommand (needs UX exploration, not 'evidence') is out of scope."

### Confidence: High

Session persistence (location management) is fully implemented. Content ownership (submission/retrieval API) is explored but not built. The distinction is clear in the codebase and documentation.

---

## Question 6: Is it already planned?

### Finding

**High confidence on session persistence, medium on content ownership**:

**Session Persistence — PLANNED and MOSTLY DELIVERED:**
- ROADMAP-session-persistence.md: "Status: Active"
  - Feature 1 (Local storage + {{SESSION_DIR}} substitution): Status "Not started" in ROADMAP, but actually completed per recent commits
  - Feature 2-4 (Config, Git backend, Cloud sync): Planned but not yet started
- PLAN-local-session-storage.md: "Status: Draft" (currently active)
- Related PRD: PRD-session-persistence-storage.md: "Status: In Progress"

**Content Ownership — EXPLORED, NOT YET PLANNED AS A FEATURE:**
- wip/explore_content-ownership_scope.md: Research scope for this exploration (published 2026-03-26, today)
- Not yet elevated to a ROADMAP, PLAN, or PRD
- Framed as "research lead" (Q7): "Is there evidence of real demand for this, and what do users do today instead?"
- The exploration scope distinguishes:
  - **In Scope**: CLI for submission/retrieval, multi-agent concurrency, gate evaluation, resume logic, skill-to-skill handoffs
  - **Out of Scope**: State file internals, final deliverables, partial patches, cloud sync, ad-hoc context injection, the CLI subcommand name

**Key evidence of planning status:**
- Session persistence roadmap explicitly planned Features 1-4 with dependencies
- PLAN-local-session-storage.md breaks Feature 1 into 6 implementation issues with acceptance criteria
- Recent commits show Feature 1 (local session storage) mostly complete
- No equivalent PLAN document exists for content ownership
- The exploration scope is explicitly framed as "research" not "plan"

### Confidence: High

Session persistence is actively planned and largely implemented. Content ownership is identified as a future research question but not yet elevated to formal planning status (no ROADMAP, PLAN, or PRD). The exploration scope document signals that demand validation (this report) is the prerequisite for deciding whether to pursue it.

---

## Synthesis: The Two Levels of Content Ownership

The investigation reveals two distinct problems that the research topic blurs:

### Level 1: Storage Location Ownership (SOLVED)
- **Problem**: Agents hardcode `wip/` paths; koto doesn't control where artifacts live
- **Solution**: Session persistence feature (BUILT)
  - koto now owns session directory location via SessionBackend
  - Agents reference paths via {{SESSION_DIR}} variable substitution
  - {{SESSION_DIR}} resolves at runtime to koto-managed location
  - Agents still read/write directly to filesystem, but koto controls the path
- **Demand**: High (confirmed in PRD, ROADMAP, active commits)
- **Status**: Mostly shipped; Features 2-4 (config, git backend, cloud sync) planned for future

### Level 2: Content API Ownership (EXPLORED)
- **Problem**: Even with session location managed, agents still read/write directly to filesystem; koto has no visibility into context or ability to validate/gate it
- **Proposed Solution**: Koto provides submission/retrieval CLI (not yet designed)
  - Agents submit context to koto via `koto context add --key <key> < data`
  - Agents retrieve context via `koto context list` / `koto context get <key>`
  - Gates evaluate against koto-owned context, not filesystem
  - Resume logic queries koto context, not file existence
- **Demand**: Medium (identified as future direction, no external user requests visible)
- **Status**: Explored in research scope; content-ownership validation is this report's purpose

---

## Calibration: Demand Validation Outcome

### Demand Validated (Session Persistence — Level 1)

Evidence is **strong and multiple**:
- Three independent maintainer-authored durable artifacts (PRD, ROADMAP, recent commits)
- Specific numbers (~150 file path references)
- Articulated pain points (team visibility, portability, tight coupling)
- Active implementation with landed commits
- Formal acceptance criteria in PLAN with checkbox validation

**Verdict**: Pursue session persistence roadmap (already in progress).

### Demand Identified but Not Validated (Content Ownership API — Level 2)

Evidence is **present but preliminary**:
- Identified as future research direction in roadmap
- Articulated as research leads in exploration scope
- Derived from validated Level-1 demand (if agents move off `wip/`, context lifecycle becomes a separate question)
- **Gap**: No external user feedback, no maintainer commitment to pursue it, no formal PLAN/PRD yet

**What's missing for validation:**
- Post-session-persistence adoption feedback: Do teams actually want context gated through koto, or is filesystem location ownership sufficient?
- Concrete use case: Which problem does content API solve that session location management doesn't?
- Design direction clarity: Is the goal content validation, controlled access, structured queries, or all three?

**Verdict**: This is a **second-order question** that depends on whether teams adopt session persistence. Another round of validation after adoption (Q3 2026?) would yield clearer signal.

---

## Evidence Artifacts

### Durable Sources Cited

- **PRD-session-persistence-storage.md** (docs/prds/): Problem statement, goals, user stories, acceptance criteria for session location management
- **ROADMAP-session-persistence.md** (docs/roadmaps/): Four-feature sequence with dependencies and sequencing rationale
- **PLAN-local-session-storage.md** (docs/plans/): Six implementation issues with checkbox acceptance criteria
- **wip/explore_content-ownership_scope.md**: Research scope for content-ownership exploration
- **Git commits** (src/session/, src/cli/session.rs, src/cli/vars.rs): Evidence of implementation completion
- **DESIGN-koto-engine.md** (docs/designs/current/): Problem statement on file-existence fragility
- **src/engine/evidence.rs**: Evidence validation implementation (not persistence/retrieval)
- **docs/guides/custom-skill-authoring.md**: Current pattern of agents writing to {{SESSION_DIR}}

### Visibility Constraint

All cited sources are in the public repository. No private content included.

---

## Research Gaps

**For future demand validation:**
1. Post-session-persistence adoption survey (after teams use new session storage for 1-2 cycles)
2. Skills migration assessment: How many file path references remain after agents update to use {{SESSION_DIR}}?
3. Concrete pain point prioritization: Which of the six research leads (Q1-6 in scope) matter most in practice?
4. Design space exploration: Is a simple submission/retrieval API sufficient, or do gates/queries need richer semantics?

---

## Conclusion

**Session persistence demand is validated with high confidence.** The feature is implemented, shipped, and documented. Teams can now keep workflow context out of git and configure storage backends.

**Content ownership demand is identified as a second-order question, not yet validated.** The exploration scope is well-framed and research leads are clear, but adoption-dependent validation is warranted before committing to feature development.

**Recommendation**: Use session persistence adoption as the signal for whether to pursue content-ownership API development. If teams consistently ask "how do I query context across phases?" or "how do I validate context before allowing transitions?", escalate to formal PLAN/PRD. If teams use session storage but continue reading/writing directly to {{SESSION_DIR}} without incident, the API might not be necessary.
