# Security Review: Phase 6 -- Review of Phase 5 Security Analysis

## Scope

Independent review of the Phase 5 security analysis against the full
DESIGN-hierarchical-workflows.md document. Evaluates completeness of attack
vector coverage, sufficiency of mitigations, correctness of "not applicable"
justifications, residual risk, and information disclosure from cross-workflow
reads.

---

## 1. Attack Vectors Not Considered in Phase 5

### 1a. Parent name recycling as a targeted attack

Phase 5 mentions orphan adoption via name reuse in passing (Risk 4 under
Cross-Workflow Isolation) but treats it as an accidental scenario. It deserves
treatment as a deliberate attack vector.

**Attack:** An agent cleans up a parent workflow, then creates a new workflow
with the same name. All orphaned children of the original parent silently
become children of the new workflow. The new parent's `children-complete` gate
now includes children it did not spawn, whose state it doesn't control.

**Impact:** The new parent could be blocked by children it can't influence, or
-- worse -- could pass its gate based on children that completed under a
different template with different semantics. Gate routing decisions
(`gates.children-done.all_complete`) would be based on stale, unrelated child
data.

**Phase 5 gap:** The report suggests warning when a parent has `is_terminal:
true` but doesn't address the recycled-name scenario where the parent was
deleted and recreated. A `template_hash` check in the child header helps but
doesn't prevent the attack -- it just enables post-hoc detection.

**Recommendation:** Consider including the parent's session creation timestamp
or a unique session ID in the child's `parent_workflow` reference, so orphans
can't be silently adopted. Alternatively, `koto init --parent` could refuse to
create children for a parent that already has orphaned children from a previous
incarnation.

### 1b. Gate evaluation timing and TOCTOU

The `children-complete` gate reads child headers during gate evaluation. Between
gate evaluation and the parent's state transition, a child's state could change
(advance further, get rewound, or get cleaned up). This is a time-of-check to
time-of-use gap.

**Impact:** Low in practice because koto runs single-threaded per `koto next`
invocation and doesn't hold locks across CLI calls. But in concurrent agent
environments (multiple agents calling `koto next` for different workflows
simultaneously), a child could be cleaned up between the parent's gate check
and the parent's use of the gate output for routing.

**Phase 5 gap:** Not mentioned. The single-user trust model makes this low
priority, but it should be documented as a known limitation for future
concurrent backends.

### 1c. name_filter prefix collision

The `name_filter` field uses prefix matching. If two fan-out stages use
overlapping prefixes (e.g., `research` and `research-deep`), the shorter prefix
will match children from both stages.

**Impact:** A `children-complete` gate scoped to `research.` would also match
`research-deep.agent1` if naming conventions aren't carefully followed. This
could cause premature gate pass (if the broader match set is all terminal) or
unexpected blocking.

**Phase 5 gap:** Not mentioned. The `name_filter` is described as a mitigation
for spoofed parent pointers, but its own failure modes aren't analyzed.

**Recommendation:** Document that `name_filter` uses prefix matching and that
prefixes must be chosen to avoid collisions. Consider supporting glob or regex
in a future release.

### 1d. Context key injection via koto context get

`koto context get <child> <key>` lets a parent read arbitrary context keys from
any workflow. If a child stores structured data (JSON) in a context key and the
parent parses it, a malicious child could craft context values designed to
influence the parent's behavior (e.g., injecting directives into context that
gets included in an agent's prompt).

**Impact:** Medium in multi-agent scenarios. If the parent agent reads child
context and feeds it into its prompt or decision logic without sanitization, the
child controls parent behavior.

**Phase 5 gap:** Phase 5 notes that cross-workflow reads are unrestricted (Risk
3) but focuses on access control rather than content trust. The data flowing
through `context get` is treated as trusted, but in a hierarchical model,
children are a less-trusted tier than the parent.

**Recommendation:** Document that context values from child workflows should be
treated as untrusted input by parent agents. Template authors should validate
and sanitize context data read from children.

---

## 2. Sufficiency of Mitigations

### 2a. Spoofed parent pointer (Phase 5 Risk 1)

**Phase 5 mitigation:** Use `name_filter` to restrict which children affect
gate evaluation.

**Assessment:** Partially sufficient. `name_filter` reduces the attack surface
but doesn't eliminate it. An attacker who knows the naming convention can still
create children matching the filter. The suggested future init token is the
correct long-term fix. For MVP, `name_filter` combined with the single-user
trust model is acceptable.

### 2b. O(N) session scan (Phase 5 DoS Risk 2)

**Phase 5 mitigation:** Document performance characteristics, recommend under
50 sessions, secondary index as long-term fix.

**Assessment:** Sufficient for MVP. The "cache list() within a single advance
loop" suggestion is a good quick win that the design doc's own mitigations
section should adopt. For CloudBackend, S3 ListObjects is paginated at 1000
keys, so the 50-session recommendation has meaningful headroom.

### 2c. Unbounded child creation (Phase 5 DoS Risk 1)

**Phase 5 mitigation:** Configurable `max_children` limit.

**Assessment:** Good suggestion but the design doesn't adopt it. Without it,
the only protection is filesystem limits. For MVP, documenting the risk is
sufficient since all agents are controlled by the same user. The
`max_children` suggestion should be tracked as a follow-up issue.

### 2d. Orphan accumulation (Phase 5 Risk 4)

**Phase 5 mitigation:** Warn on init when parent is terminal, check
template_hash.

**Assessment:** Insufficient as a standalone mitigation. The design's own
`--orphaned` flag is the primary defense but requires agents to actively use
it. No automated cleanup exists. Acceptable for MVP but should be paired with
documentation that template authors include orphan cleanup in their
convergence states.

---

## 3. "Not Applicable" Justification Review

### 3a. External Artifact Handling -- "Not applicable"

**Assessment: Correct.** The design processes only koto-internal state files
and headers. Template files go through the existing compilation pipeline.
No new external data surfaces.

### 3b. Supply Chain or Dependency Trust -- "Not applicable"

**Assessment: Correct.** No new dependencies or external artifact sources.
The new gate type is a match arm in existing code. Child templates are authored
by the same user.

**Caveat:** If future work allows passing template paths via CLI flags for
child init (rather than requiring pre-existing template files), this
justification would need revisiting. Currently out of scope.

---

## 4. Residual Risk Assessment

### Risks that warrant escalation

**None require design changes.** All identified risks operate within koto's
existing single-user trust model. The design's Security Considerations section
(already present in the final doc) covers the primary risks adequately.

### Risks that warrant documentation or follow-up issues

| Risk | Severity | Action |
|------|----------|--------|
| Parent name recycling as targeted attack | Medium | Add to Security Considerations: warn that name reuse after cleanup silently adopts orphans |
| Context key injection from child to parent | Medium | Document that child context should be treated as untrusted input by parent agents |
| name_filter prefix collision | Low | Document prefix matching semantics and collision risk in template authoring guide |
| TOCTOU in gate evaluation | Low | Document as known limitation for future concurrent backends |
| No max_children enforcement | Low | Track as follow-up issue for post-MVP |

---

## 5. Cross-Workflow Read Information Disclosure

The `children-complete` gate introduces structured cross-workflow reads: the
gate evaluator reads `StateFileHeader` fields from other workflows' state
files. The question is whether this creates information disclosure risks beyond
what already exists.

### What the gate reads

The evaluator calls `backend.list()` and reads headers. Headers contain:
`name`, `template_path`, `template_hash`, `parent_workflow`, `schema_version`,
and `repo_id`. The gate output exposes `name` and `current_state` per child.
`current_state` comes from `derive_machine_state()`, which replays the event
log.

### What's new vs. what already existed

- `koto workflows` already exposes all workflow names, template paths, and
  creation timestamps. Not new.
- `koto query` already exposes full state including evidence and decisions.
  Not new.
- `koto context get <workflow> <key>` already works cross-workflow (the
  context store is per-workflow but the CLI accepts any workflow name). Not new
  in capability, but newly promoted as a pattern.
- `koto status <child>` is new but exposes a strict subset of `koto query`.

### Disclosure risk assessment

**Within the same repo-id scope:** No meaningful new disclosure. All data was
already accessible through existing commands. The `children-complete` gate
structures reads that were already possible, making them more systematic but
not more permissive.

**Across repo-id scopes:** Not possible. The session backend is scoped to a
repo-id, and nothing in the design changes that scoping. A workflow in repo A
cannot read headers from repo B.

**Through gate output in blocking_conditions:** The `children-complete` gate
output includes child workflow names and current state names in the JSON
response to `koto next`. This data flows to the calling agent. In a scenario
where:
- A parent agent is less trusted than the system running child workflows, OR
- Gate output is logged to a less-secure location than the session store

...child state names could leak. However, koto's trust model assumes the agent
calling `koto next` is the authorized operator of that workflow, so this is
not a meaningful escalation.

### Conclusion on information disclosure

The cross-workflow read pattern does not create new information disclosure
risks. It structures access patterns that were already available through
existing CLI commands. The repo-id scope boundary remains intact. The main
caution is that promoting `koto context get <child> <key>` as a standard
pattern increases the likelihood that agents routinely read cross-workflow
data, which matters only if koto ever moves to a multi-tenant model with
per-workflow access control.

---

## Summary

Phase 5 provides solid coverage of the primary security dimensions. Four gaps
deserve attention:

1. **Parent name recycling** should be documented as a deliberate attack vector,
   not just an accidental edge case. Orphan adoption via name reuse can corrupt
   gate evaluation with unrelated child data.

2. **Context key injection** from children to parents is an untreated content
   trust issue. Phase 5 focuses on access control for cross-workflow reads but
   not on the trustworthiness of the data flowing through them.

3. **name_filter prefix collisions** can cause gates to include unintended
   children. The matching semantics need documentation.

4. **Cross-workflow reads do not create new disclosure risks.** The
   `children-complete` gate structures access that was already possible.
   Repo-id scoping remains the isolation boundary.

No risks warrant design changes. Two warrant additions to the design doc's
Security Considerations section (name recycling, context trust). Two warrant
documentation in authoring guides (prefix collisions, TOCTOU).
