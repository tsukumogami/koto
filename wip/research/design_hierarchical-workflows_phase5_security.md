# Security Review: hierarchical-workflows

## Dimension Analysis

### External Artifact Handling

**Applies:** No

This design does not download, execute, or process external inputs. All data flows are internal to koto's session storage. The `children-complete` gate reads state file headers that koto itself wrote via `backend.list()`. The `koto status` command reads from koto's own event log via `derive_machine_state()`. Template files are already loaded through koto's existing template compilation pipeline, which is out of scope for this design. No new external data ingestion surfaces are introduced.

### Permission Scope

**Applies:** Yes -- low severity

**Risk:** The `children-complete` gate evaluator needs access to the session backend (`backend.list()`) during gate evaluation. Currently, the gate evaluator closure receives a `ContextStore` reference. This design threads the full `SessionBackend` (or at least `list()` + `read_header()`) into the gate evaluator, widening what code running during gate evaluation can access.

Additionally, `koto context get <child> <key>` and `koto status <child>` allow a parent agent to read any workflow's context and state within the same repo-id scope -- not just its declared children. There is no access control restricting reads to parent-child relationships.

**Severity:** Low. koto already operates on a trust model where any agent with filesystem access to the `.koto/` directory can read any session's state file. The session backend's `list()`, `exists()`, and `read_header()` methods are already callable by any code path. This design doesn't escalate permissions beyond what's already available; it just adds a structured way to exercise reads that were already possible.

**Mitigation suggestion:** Document that cross-workflow reads are scoped to the repo-id namespace, not to the parent-child relationship. If future use cases require stricter isolation (multi-tenant, untrusted child templates), a read-authorization layer would need to gate `context get` and `status` calls to verified parent-child pairs. For MVP this is unnecessary -- all workflows in a repo-id scope are controlled by the same user.

### Supply Chain or Dependency Trust

**Applies:** No

This design introduces no new dependencies, package downloads, or external artifact sources. The `children-complete` gate type is a new match arm in the existing gate evaluator. The `koto status` command calls `derive_machine_state()`, which already exists. Template files for child workflows are loaded through the existing template compilation pipeline, and this design explicitly states "external child templates, no implicit state sharing" -- meaning child templates are authored by the same user who authors the parent template. No new trust boundary is crossed.

### Data Exposure

**Applies:** Yes -- low severity

**Risk:** The `children-complete` gate output includes per-child workflow names and current state names in its structured JSON output. This information flows through `blocking_conditions` in the `koto next` response, which agents log and process. If child workflow names or state names contain sensitive information (unlikely but possible), this data would be exposed in gate output.

The `koto status <child>` command exposes `name`, `current_state`, `template_path`, `template_hash`, and `is_terminal`. The `template_path` reveals filesystem structure.

**Severity:** Low. koto's existing `koto workflows` command already exposes all workflow names, template paths, and creation timestamps. `koto query` exposes full state including evidence and decisions. The new commands don't expose data that wasn't already accessible through existing commands. All data stays local to the machine or within the S3 bucket configured for CloudBackend.

**Mitigation suggestion:** None needed beyond existing controls. The repo-id namespace already provides the isolation boundary.

### Cross-Workflow Isolation

**Applies:** Yes -- medium severity (the most relevant dimension for this design)

**Risk 1: Spoofed parent pointer.** The `parent_workflow` header field is written at `koto init` time and validated only for existence (`backend.exists(parent)`). There is no authentication that the caller is actually the parent workflow's agent. Any agent that knows a workflow name can create a child claiming that workflow as parent. A malicious or buggy agent could:
- Create a child under someone else's parent, causing the parent's `children-complete` gate to block unexpectedly (the gate discovers children via parent-pointer scan).
- Flood a parent with fake children that never complete, preventing the parent from advancing past its convergence gate.

**Risk 2: Vacuous pass prevention is one-directional.** The design specifies "if zero children match, return Failed (prevent vacuous pass)." This means a parent workflow whose children haven't been initialized yet will block, which is correct. But it also means an agent must init at least one child before advancing, creating a temporal coupling.

**Risk 3: Sibling reads are unrestricted.** `koto context get <child> <key>` allows any workflow to read any other workflow's context, not just parent-to-child. A child can read sibling context, or an unrelated workflow can read any context key. The design doesn't add any access control based on lineage.

**Risk 4: Orphan accumulation.** Advisory-only lifecycle means parent cleanup leaves children with dangling `parent_workflow` references. While `--orphaned` makes them discoverable, nothing prevents accumulation. Orphans with stale parent pointers could be "adopted" if a new workflow happens to reuse the parent name.

**Severity:** Medium overall. Risks 1 and 4 are the most concerning. Risk 1 because the gate's parent-pointer discovery mechanism means any workflow claiming a parent relationship affects the parent's gate evaluation. Risk 4 because name reuse after cleanup could create unintended parent-child relationships.

**Mitigation suggestions:**

- **Risk 1:** Consider adding an optional `expected_children` count or `name_filter` requirement on the `children-complete` gate to limit which children can affect evaluation. The existing `name_filter` field partially addresses this -- if set, only children matching the prefix are counted. Document that `name_filter` should be set when the parent doesn't control all child initialization. For stronger guarantees, a future release could add a shared secret (init token) that the parent generates and children must present.

- **Risk 4:** Consider warning when `koto init --parent <name>` creates a child whose parent has `is_terminal: true` (parent already completed). Prevent accidental adoption by checking if the parent workflow was recently cleaned up and recreated. Alternatively, include the parent's `template_hash` in the child's header for post-hoc verification.

### Denial of Service

**Applies:** Yes -- medium severity

**Risk 1: Unbounded child creation.** Nothing in the design limits how many children a parent can spawn. Since each child creates a session directory with a state file, an agent in a loop could create thousands of sessions, exhausting disk space or inode limits.

**Risk 2: O(N) session scan per gate evaluation.** Every `children-complete` gate evaluation calls `backend.list()` and reads all session headers. With many sessions (not just children -- all sessions in the repo-id scope), this becomes expensive. The design acknowledges this in Consequences: "For large session stores this could be slow." A pathological case: a workflow with a `children-complete` gate in a repo with 10,000 sessions would scan all 10,000 headers on every `koto next` call. For CloudBackend, this means S3 ListObjects + GetObject calls per evaluation.

**Risk 3: Recursive nesting.** The design supports multi-level hierarchies (parent -> child -> grandchild). Each level can fan out, creating exponential session counts. A three-level hierarchy with fan-out of 10 at each level produces 1,111 sessions.

**Severity:** Medium. Risk 2 is the most immediate concern because it affects every gate evaluation, not just pathological cases. The CloudBackend amplifies this with network I/O per session.

**Mitigation suggestions:**

- **Risk 1:** Add a configurable `max_children` limit, either as a template-level gate field or a global koto configuration. Default to a reasonable cap (e.g., 100). The `children-complete` gate evaluator should fail with a clear error when the limit is exceeded.

- **Risk 2:** The design's own mitigation (secondary parent-child index) is the right long-term fix. For MVP, document the performance characteristic and recommend keeping session counts under 50 per repo-id (as the design already suggests). Consider caching the `list()` result within a single advance loop invocation if multiple gates reference children.

- **Risk 3:** Document a recommended maximum nesting depth. Consider adding a `depth` counter to headers (parent's depth + 1) so tooling can warn when nesting exceeds a threshold.

## Recommended Outcome

**OPTION 2 - Document considerations**: The design should include a Security Considerations section covering cross-workflow isolation and resource exhaustion. Draft:

---

### Security Considerations

**Cross-workflow isolation.** The `parent_workflow` header field is self-declared by the child at init time. koto validates that the parent exists but does not authenticate that the initializing agent is authorized by the parent. Any agent with access to the session backend can create a child claiming any existing workflow as its parent, which affects the parent's `children-complete` gate evaluation. In the current trust model (single user per repo-id scope), this is acceptable. Multi-tenant deployments would require an authorization mechanism for parent-child registration.

Use `name_filter` on `children-complete` gates to restrict which children affect evaluation. Without it, any workflow declaring the parent relationship will be counted.

**Resource bounds.** Child creation is unbounded by default. Each child creates a session directory and state file. The `children-complete` gate scans all sessions in the repo-id scope on every evaluation (O(N) in total sessions, not just children). Keep session counts under 50 per repo-id for predictable performance. Clean up completed children promptly. If performance degrades, a secondary parent-child index can be added without changing the gate contract.

**Orphan lifecycle.** Parent cleanup does not cascade to children. Orphaned children retain a `parent_workflow` reference to a deleted session. If a new workflow reuses the deleted parent's name, orphaned children will appear as its children. Use `koto workflows --orphaned` in cleanup routines to prevent accumulation.

---

## Summary

The design's most significant security surface is cross-workflow isolation: the self-declared `parent_workflow` header lets any workflow claim any parent, affecting that parent's gate evaluation. Combined with unbounded child creation and O(N) session scans during gate evaluation, there are resource exhaustion paths that should be documented. None of these rise to the level of requiring design changes -- they are inherent to koto's existing single-user trust model and flat session storage -- but a Security Considerations section should make the constraints explicit so implementers and template authors understand the boundaries.
