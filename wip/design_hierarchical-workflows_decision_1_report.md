<!-- decision:start id="children-complete-gate-contract" status="assumed" -->
### Decision: children-complete gate contract

**Context**

koto's gate-transition contract (v0.6.0) established structured gate output with typed fields, where each gate type produces a StructuredGateResult with an outcome and typed output JSON. The exploration for hierarchical workflows decided that a `children-complete` gate type is the right primitive for fan-out convergence -- it requires zero advance loop changes and reuses the existing blocking_conditions, gates.* routing, and override infrastructure.

Five sub-questions needed resolution before implementation: how the gate identifies children, what completion condition it checks, what the output schema looks like, how overrides work, and how agents distinguish temporal blocking (retry later) from corrective blocking (fix something). These sub-questions are tightly coupled -- the child identification strategy constrains the output schema, and the completion condition determines what "override to pretend done" means.

**Assumptions**

- The `parent_workflow: Option<String>` field will be added to StateFileHeader as decided in the exploration. Child discovery depends on this field existing.
- The number of child workflows per parent is small enough (under ~50) that listing all sessions and filtering by parent_workflow is acceptable. Larger fan-outs would need a secondary index.
- The gate evaluator closure in the CLI handler can capture the session backend for cross-workflow queries. Code reading confirms this is feasible through the existing closure injection pattern in `src/cli/mod.rs`.
- Only `"terminal"` completion mode needs to be implemented for the first release. The `completion` field establishes the extensibility contract; `"state:<name>"` and `"context:<key>"` can ship later.

**Chosen: Hybrid Discovery with Configurable Completion (Alternative 4, revised)**

The children-complete gate contract uses parent-pointer discovery as the primary child identification mechanism, configurable completion via a string field, a fixed-shape output schema with per-child detail, BlockingCondition.category for temporal signaling, and a simple all-complete override shape.

**Gate declaration in template:**

```yaml
gates:
  children-done:
    type: children-complete
    completion: "terminal"        # optional, default "terminal"
    name_filter: "research."      # optional, filters discovered children by name prefix
```

**Gate struct changes:** Two new optional fields on the Gate struct:
- `completion: Option<String>` -- completion condition. Values: `"terminal"` (default, child in terminal state), `"state:<name>"` (child in named state), `"context:<key>"` (child has context key). Compiler validates the prefix.
- `name_filter: Option<String>` -- optional workflow name prefix filter applied after parent-pointer discovery.

**Child identification:** The evaluator queries `backend.list()`, reads StateFileHeaders, and filters to workflows where `parent_workflow == Some(current_workflow_name)`. If `name_filter` is set, results are further filtered to workflows whose name starts with the filter value. If zero children match, the gate Fails (prevents vacuous pass).

**Completion condition:** Determined by the `completion` field:
- `"terminal"` (default): a child is complete when its current state is marked `terminal: true` in its compiled template.
- `"state:<name>"`: a child is complete when its current state matches `<name>` exactly.
- `"context:<key>"`: a child is complete when its context store contains `<key>`.

Only `"terminal"` needs to be implemented initially. The compiler rejects unknown prefixes, keeping the DSL closed.

**Output schema:**

```json
{
  "total": 3,
  "completed": 2,
  "pending": 1,
  "all_complete": false,
  "children": [
    {"name": "explore.r1", "state": "done", "complete": true},
    {"name": "explore.r2", "state": "done", "complete": true},
    {"name": "explore.r3", "state": "research", "complete": false}
  ],
  "error": ""
}
```

Top-level aggregate fields (`total`, `completed`, `pending`, `all_complete`) enable quick boolean checks in when-clauses. The `children` array provides per-child detail for routing decisions (e.g., route differently when specific children fail). Each array element has a fixed shape: `{name: string, state: string, complete: boolean}`. The `error` field follows the existing gate convention.

For `gate_type_schema()`, the registered fields are: `total` (Number), `completed` (Number), `pending` (Number), `all_complete` (Boolean), `error` (Str). The `children` array is not part of the static schema (it contains nested objects, which the current schema system doesn't model), but it's always present in the output.

**Override shape:**

```json
{"total": 0, "completed": 0, "pending": 0, "all_complete": true, "children": [], "error": ""}
```

This is the built-in default returned by `built_in_default("children-complete")` and used when `koto overrides record` is called without `--with-data`. It represents "pretend all children are done with nothing pending." The template author can customize via `override_default` on the gate definition.

**Temporal signaling:** Add a `category` field to BlockingCondition with two values:
- `"corrective"` -- the agent must take action to unblock (fix a failing command, submit missing evidence). Default for existing gate types.
- `"temporal"` -- the blocking condition will resolve on its own with time (children completing, external processes finishing). The agent should retry later.

The `category` field is set by a new function `gate_blocking_category(gate_type: &str) -> &str` which returns `"temporal"` for `children-complete` and `"corrective"` for all other gate types. This is a backward-compatible addition to the BlockingCondition struct.

**Rationale**

All four validators converged on parent-pointer discovery as the correct child identification mechanism, given the exploration's prior commitment to the parent_workflow header. Dynamic discovery (finding all children via header scan) is strictly superior to static child lists for the primary use case of dynamic fan-out, where the number of children depends on runtime data.

Configurable completion was the strongest consensus point across all validators. Terminal-only completion was initially proposed as "sufficient for MVP" but all validators agreed it's insufficient for workflows where children have multiple terminal states with different meanings. The `completion` string field with a closed set of prefixes provides extensibility without becoming an expression language -- the compiler rejects unknown prefixes, so new modes require a koto release.

The fixed outer schema with a per-child children array resolves the tension between aggregate simplicity and per-child detail. A per-child map (Alt 2) was rejected because it creates variable-shaped output that breaks `gate_type_schema()`. A per-child array has fixed outer structure while still enabling when-clause routing on child status.

BlockingCondition.category was the unanimous choice for temporal signaling after the revision rounds. Output-level hints (`retry_hint`, `retry_after_secs`) were rejected because they don't establish a pattern -- each gate type would invent its own hint. A structural field on BlockingCondition gives a single, typed mechanism that works generically across all gate types.

**Alternatives Considered**

- **Alt 1: Parent-Pointer Discovery with Terminal-Only Completion.** Minimal implementation -- zero new Gate struct fields, terminal-only completion, aggregate-only output. Rejected because terminal-only completion is insufficient for real workflows and would require a contract-breaking extension within a release or two. Its validator conceded this limitation.

- **Alt 2: Explicit Child List with State-Name Completion.** Template-declared child names with per-child status map output. Rejected because static child lists don't support dynamic fan-out (the primary use case) and the variable-shaped output map breaks gate_type_schema(). Its validator conceded both points.

- **Alt 3: Naming-Convention Discovery with Evidence-Based Completion.** Zero new fields by repurposing `pattern` and `key` for name prefix filtering and context-key completion. Rejected because naming convention is a weaker contract than parent-pointer metadata, and field reuse creates semantic confusion. Context-key completion is preserved as a completion mode within Alt 4.

**Consequences**

The Gate struct grows by two optional fields (completion, name_filter), bringing it to eight fields total. Template authors who don't use children-complete gates are unaffected -- both fields default to absent.

BlockingCondition gains a category field, which changes the JSON shape of gate_blocked and evidence_required responses. Agents must tolerate the new field. This is backward-compatible (additive), but agents that strictly validate response shapes will need updates.

The gate evaluator gains a dependency on the session backend for cross-workflow queries. This is wired through the existing closure capture pattern, so evaluate_gates() and advance_until_stop() signatures don't change.

Only terminal completion needs implementation initially. The completion field's closed-prefix design means `"state:X"` and `"context:X"` can ship in follow-up releases without changing the Gate struct or the output schema.

Per-child arrays in the output can grow large for massive fan-outs. If this becomes a problem, a future enhancement could cap the array or make it opt-in. For the expected use cases (under 50 children), the array size is manageable.
<!-- decision:end -->
